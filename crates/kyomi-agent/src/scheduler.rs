// SPDX-License-Identifier: AGPL-3.0-or-later

//! Watch scheduler — background poller that finds due watches and executes them.
//!
//! Ports Python's `WatchPoller` from `watch_scheduler.py`.
//!
//! # Architecture
//!
//! The scheduler runs as a `tokio::spawn` background task inside the API server.
//! It polls the database every 30 seconds for watches whose `next_run_at` has passed,
//! claims them via compare-and-swap on `next_run_at`, and spawns execution tasks.
//!
//! ## Multi-pod safety
//!
//! Compare-and-swap on `watches.next_run_at` guarantees at-most-once execution
//! across multiple Kubernetes replicas. No Redis locks needed.
//!
//! ## Lifecycle
//!
//! 1. On startup: `check_missed_executions()` — catches up watches that were missed
//!    while the server was down (within last 24h), scheduling them with random jitter
//!    to avoid thundering herd.
//! 2. Poll loop: Every 30s, find due watches, claim via CAS, spawn executions.
//! 3. Daily cleanup: Permanently delete alerts soft-deleted >30 days ago, clean up
//!    expired refresh tokens.
//! 4. Graceful shutdown: Cancel token stops the loop, wait for active executions
//!    (30s timeout), mark any still-running executions as "error".

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rand::Rng;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use kyomi_auth::watch_service;
use kyomi_auth::websocket::WebSocketManager;
use kyomi_core::platform::PlatformRegistry;
use kyomi_core::{Config, DbPool, KVPool};
use kyomi_embed::LazyEmbedding;

use crate::watch_execution;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// How often the scheduler polls for due watches (in seconds).
const POLL_INTERVAL_SECONDS: u64 = 30;

/// Maximum time to wait for active executions during shutdown.
const SHUTDOWN_TIMEOUT_SECONDS: u64 = 30;

/// How old a missed execution can be before we ignore it (24 hours).
const MISSED_EXECUTION_CUTOFF_HOURS: i64 = 24;

/// Minimum random jitter for catch-up scheduling (seconds).
const CATCHUP_JITTER_MIN: u64 = 30;

/// Maximum random jitter for catch-up scheduling (seconds).
const CATCHUP_JITTER_MAX: u64 = 90;

/// How often to run cleanup (24 hours in seconds).
const CLEANUP_INTERVAL_SECONDS: u64 = 86400;

/// Soft-deleted alerts older than this are permanently deleted (30 days).
const DELETED_ALERT_RETENTION_DAYS: i64 = 30;

/// Executions stuck in 'running' longer than this are considered orphaned (minutes).
const ORPHAN_EXECUTION_THRESHOLD_MINUTES: i64 = 30;

// ---------------------------------------------------------------------------
// WatchScheduler
// ---------------------------------------------------------------------------

/// Background watch scheduler with compare-and-swap distributed locking.
///
/// Polls the database every 30s for due watches, claims them via CAS on
/// `watches.next_run_at`, and spawns execution tasks.
pub struct WatchScheduler {
    db: DbPool,
    kv: KVPool,
    encryption_key: Arc<[u8; 32]>,
    embedding: LazyEmbedding,
    ws_manager: WebSocketManager,
    config: Arc<Config>,
    connect_registry: Option<kyomi_datasource_server::ConnectRegistry>,
    platforms: Arc<PlatformRegistry>,
    cancel: CancellationToken,
    /// Tracks running watch execution tasks by watch_id.
    active_executions: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    /// Tracks when we last ran cleanup.
    last_cleanup: Arc<Mutex<Option<Instant>>>,
}

/// Dependencies required to construct a [`WatchScheduler`].
///
/// Packaged into a struct to keep the constructor signature under clippy's
/// `too_many_arguments` threshold while keeping every field explicit at
/// the call site.
pub struct WatchSchedulerDeps {
    pub db: DbPool,
    pub kv: KVPool,
    pub encryption_key: Arc<[u8; 32]>,
    pub embedding: LazyEmbedding,
    pub ws_manager: WebSocketManager,
    pub config: Arc<Config>,
    pub connect_registry: Option<kyomi_datasource_server::ConnectRegistry>,
    pub platforms: Arc<PlatformRegistry>,
    pub cancel: CancellationToken,
}

impl WatchScheduler {
    /// Create a new `WatchScheduler`.
    pub fn new(deps: WatchSchedulerDeps) -> Self {
        Self {
            db: deps.db,
            kv: deps.kv,
            encryption_key: deps.encryption_key,
            embedding: deps.embedding,
            ws_manager: deps.ws_manager,
            config: deps.config,
            connect_registry: deps.connect_registry,
            platforms: deps.platforms,
            cancel: deps.cancel,
            active_executions: Arc::new(Mutex::new(HashMap::new())),
            last_cleanup: Arc::new(Mutex::new(None)),
        }
    }

    /// Start the scheduler as a background task.
    ///
    /// Runs missed execution recovery, then enters the poll loop.
    /// Returns a `JoinHandle` that completes when the scheduler exits.
    pub fn start(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            // Run missed execution recovery before entering the poll loop
            self.check_missed_executions().await;
            self.recover_orphaned_executions().await;

            info!("Watch scheduler started (poll interval: {POLL_INTERVAL_SECONDS}s)");

            self.poll_loop().await;

            info!("Watch scheduler exited");
        })
    }

    /// Main poll loop — runs until the cancel token is triggered.
    ///
    /// Each iteration:
    /// 1. Clean up finished tasks from `active_executions`
    /// 2. Poll for due watches and claim them via CAS
    /// 3. Run daily cleanup if due
    /// 4. Sleep for POLL_INTERVAL or until cancelled
    async fn poll_loop(self: &Arc<Self>) {
        loop {
            // Run one poll cycle
            if let Err(e) = self.poll_due_watches().await {
                error!(error = %e, "Error in watch poll loop");
            }

            // Recover any executions orphaned by a crashed pod
            self.recover_orphaned_executions().await;

            // Run cleanup if due
            if let Err(e) = self.maybe_run_cleanup().await {
                error!(error = %e, "Error in watch cleanup");
            }

            // Wait for the next poll interval, or cancel
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    info!("Watch scheduler received cancel signal");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECONDS)) => {
                    // Normal timeout — continue polling
                }
            }
        }
    }

    /// Find watches where `next_run_at <= NOW()` and claim them via compare-and-swap.
    ///
    /// For each due watch:
    /// 1. Skip if already in `active_executions`
    /// 2. Compute new `next_run_at` from cron schedule
    /// 3. CAS: `UPDATE watches SET next_run_at = :new WHERE watch_id = :wid AND next_run_at = :old`
    /// 4. If `rows_affected == 1`: spawn `execute_watch()`, track in `active_executions`
    /// 5. If `rows_affected == 0`: another pod claimed it, skip
    async fn poll_due_watches(&self) -> Result<(), String> {
        // Clean up finished executions from tracking map
        self.clean_finished_executions().await;

        // Query for due watches, filtering out any whose workspace's
        // billing is lapsed (KYO-805) — see `due_watches_to_execute`.
        let due_watches: Vec<DueWatch> =
            due_watches_to_execute(&self.db, self.config.self_hosted, Utc::now()).await?;

        if due_watches.is_empty() {
            return Ok(());
        }

        info!(count = due_watches.len(), "Found due watches");

        for watch in due_watches {
            let watch_id = &watch.watch_id;

            // Skip if already executing
            {
                let active = self.active_executions.lock().await;
                if active.contains_key(watch_id) {
                    continue;
                }
            }

            // Compute new next_run_at from cron schedule
            let new_next_run = match watch_service::calculate_next_run(&watch.schedule) {
                Ok(next) => next,
                Err(e) => {
                    error!(watch_id = %watch_id, error = %e, "Invalid cron schedule for watch");
                    continue;
                }
            };

            // Compare-and-swap: only this pod gets to execute if it wins
            let result = kyomi_core::db_execute!(
                self.db,
                "UPDATE watches \
                 SET next_run_at = $1 \
                 WHERE watch_id = $2 \
                   AND next_run_at = $3",
                new_next_run,
                watch_id as &str,
                watch.next_run_at
            );

            match result {
                Ok(res) if res.rows_affected() == 1 => {
                    // We won the claim — spawn execution
                    info!(watch_id = %watch_id, "Claimed watch, spawning execution");

                    let db = self.db.clone();
                    let kv = self.kv.clone();
                    let encryption_key = self.encryption_key.clone();
                    // Wait for embedding model (fast path: already loaded after ~440ms)
                    let embedding = match self.embedding.wait_ready().await {
                        Ok(emb) => emb.clone(),
                        Err(e) => {
                            tracing::error!(error = %e, "Embedding service not ready for watch execution");
                            continue;
                        }
                    };
                    let ws_manager = self.ws_manager.clone();
                    let app_config = self.config.clone();
                    let cr = self.connect_registry.clone();
                    let platforms = self.platforms.clone();
                    let wid = watch_id.clone();

                    let handle = tokio::spawn(async move {
                        if let Err(e) = watch_execution::execute_watch(
                            &db,
                            &kv,
                            &encryption_key,
                            &embedding,
                            &ws_manager,
                            &app_config,
                            cr,
                            &platforms,
                            &wid,
                        )
                        .await
                        {
                            error!(watch_id = %wid, error = %e, "Watch execution failed");
                        }
                    });

                    let mut active = self.active_executions.lock().await;
                    active.insert(watch_id.clone(), handle);
                }
                Ok(_) => {
                    // rows_affected == 0: another pod claimed it, skip silently
                }
                Err(e) => {
                    error!(
                        watch_id = %watch_id,
                        error = %e,
                        "CAS update failed for watch"
                    );
                }
            }
        }

        Ok(())
    }

    /// Remove completed executions from the active tracking map.
    async fn clean_finished_executions(&self) {
        let mut active = self.active_executions.lock().await;
        let finished: Vec<String> = active
            .iter()
            .filter(|(_, handle)| handle.is_finished())
            .map(|(wid, _)| wid.clone())
            .collect();

        for wid in finished {
            active.remove(&wid);
        }
    }

    /// On startup, fix watches with stale `next_run_at` (in the past, within 24h).
    ///
    /// Sets `next_run_at = now + random(30..90)s` via CAS so the poll loop picks them
    /// up naturally. The random jitter prevents thundering herd after server restart.
    async fn check_missed_executions(&self) {
        let now = Utc::now();
        let cutoff = now - chrono::Duration::hours(MISSED_EXECUTION_CUTOFF_HOURS);

        let is_pg = self.db.is_postgres();
        let bool_true = kyomi_core::sql_compat::bool_true(is_pg);
        let missed_sql = format!(
            "SELECT watch_id, name, next_run_at \
             FROM watches \
             WHERE enabled = {bool_true} \
               AND next_run_at IS NOT NULL \
               AND next_run_at < $1 \
               AND next_run_at >= $2"
        );
        let rows = match kyomi_core::db_fetch_all!(
            self.db, MissedWatch,
            &missed_sql,
            now,
            cutoff
        ) {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "Failed to check missed executions");
                return;
            }
        };

        if rows.is_empty() {
            info!("No missed watch executions to catch up on");
            return;
        }

        info!(count = rows.len(), "Found watches with missed executions, scheduling catch-up");

        for watch in rows {
            // Generate jitter inside a block so ThreadRng doesn't live across await
            let delay_secs = {
                let mut rng = rand::rng();
                rng.random_range(CATCHUP_JITTER_MIN..=CATCHUP_JITTER_MAX)
            };
            let catch_up_time = now + chrono::Duration::seconds(delay_secs as i64);

            match kyomi_core::db_execute!(
                self.db,
                "UPDATE watches \
                 SET next_run_at = $1 \
                 WHERE watch_id = $2 \
                   AND next_run_at = $3",
                catch_up_time,
                &watch.watch_id,
                watch.next_run_at
            ) {
                Ok(res) if res.rows_affected() == 1 => {
                    info!(
                        watch_id = %watch.watch_id,
                        name = %watch.name,
                        delay_secs = delay_secs,
                        "Missed watch execution, catch-up scheduled"
                    );
                }
                Ok(_) => {
                    // Another pod already handled this watch
                }
                Err(e) => {
                    error!(
                        watch_id = %watch.watch_id,
                        error = %e,
                        "Failed to schedule catch-up for watch"
                    );
                }
            }
        }
    }

    /// Find watch executions stuck in 'running' for over 30 minutes and mark them
    /// as errors.
    ///
    /// Handles the case where the backend crashes mid-execution (SIGKILL, OOM, etc.)
    /// before cleanup can run. In a multi-pod deployment, any pod can clean up
    /// orphaned executions from any other pod because we use a time threshold —
    /// legitimate executions complete in minutes, not 30+ minutes.
    ///
    /// Called on startup and during each poll cycle.
    async fn recover_orphaned_executions(&self) {
        let threshold =
            Utc::now() - chrono::Duration::minutes(ORPHAN_EXECUTION_THRESHOLD_MINUTES);

        let is_pg = self.db.is_postgres();
        let now_fn = kyomi_core::sql_compat::now(is_pg);
        let orphan_sql = format!(
            "UPDATE watch_executions \
             SET status = 'error', \
                 error_message = 'Execution orphaned — recovered after server restart', \
                 completed_at = {now_fn} \
             WHERE status = 'running' \
               AND started_at < $1"
        );
        match kyomi_core::db_execute!(
            self.db,
            &orphan_sql,
            threshold
        ) {
            Ok(result) => {
                let count = result.rows_affected();
                if count > 0 {
                    warn!(
                        count = count,
                        "Recovered orphaned watch executions (stuck in 'running' for >30 minutes)"
                    );
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to recover orphaned executions");
            }
        }
    }

    /// Run daily cleanup if more than 24 hours since last run.
    ///
    /// Cleanup tasks:
    /// 1. Permanently delete `watch_executions` that were soft-deleted >30 days ago
    /// 2. Delete expired refresh tokens
    /// 3. Clean up stale push subscriptions (>5 failures or >90 days unused)
    async fn maybe_run_cleanup(&self) -> Result<(), String> {
        let should_run = {
            let last = self.last_cleanup.lock().await;
            match *last {
                None => true,
                Some(t) => t.elapsed().as_secs() >= CLEANUP_INTERVAL_SECONDS,
            }
        };

        if !should_run {
            return Ok(());
        }

        // Update last_cleanup timestamp
        {
            let mut last = self.last_cleanup.lock().await;
            *last = Some(Instant::now());
        }

        info!("Running daily watch cleanup");

        // 1. Delete old soft-deleted alerts
        self.cleanup_old_deleted_alerts().await;

        // 2. Delete expired refresh tokens
        self.cleanup_expired_tokens().await;

        // 3. Clean up orphaned active refresh tokens from concurrent grace-period rotations
        self.cleanup_orphaned_active_tokens().await;

        // 4. Clean up stale push subscriptions (>5 failures or >90 days unused)
        kyomi_auth::push_service::cleanup_stale(&self.db).await;

        Ok(())
    }

    /// Permanently delete watch executions that were soft-deleted more than 30 days ago.
    async fn cleanup_old_deleted_alerts(&self) {
        let cutoff = Utc::now() - chrono::Duration::days(DELETED_ALERT_RETENTION_DAYS);

        match kyomi_core::db_execute!(
            self.db,
            "DELETE FROM watch_executions \
             WHERE deleted_at IS NOT NULL \
               AND deleted_at < $1",
            cutoff
        ) {
            Ok(result) => {
                let count = result.rows_affected();
                if count > 0 {
                    info!(count = count, "Cleaned up deleted alerts older than 30 days");
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to cleanup deleted alerts");
            }
        }
    }

    /// Clean up orphaned active refresh tokens created by concurrent grace-period rotations.
    ///
    /// When multiple tabs refresh simultaneously during the grace period, each gets a new
    /// token in the same family. The "losing" tokens (not held by any client) are never
    /// used again. This cleans up active tokens in families with multiple active tokens
    /// where the token hasn't been used in over 1 hour.
    async fn cleanup_orphaned_active_tokens(&self) {
        let is_pg = self.db.is_postgres();
        let bool_true = kyomi_core::sql_compat::bool_true(is_pg);
        let one_hour_ago = chrono::Utc::now() - chrono::Duration::hours(1);

        // Use a cross-database approach: keep the newest token per family_id
        // SQLite doesn't support DISTINCT ON, so use MAX(created_at) subquery instead
        let sql = format!(
            "DELETE FROM refresh_tokens \
             WHERE is_active = {bool_true} \
               AND replaced_at IS NULL \
               AND last_used < $1 \
               AND family_id IN ( \
                 SELECT family_id FROM refresh_tokens \
                 WHERE is_active = {bool_true} AND replaced_at IS NULL \
                 GROUP BY family_id \
                 HAVING COUNT(*) > 1 \
               ) \
               AND token_id NOT IN ( \
                 SELECT t2.token_id FROM refresh_tokens t2 \
                 INNER JOIN ( \
                   SELECT family_id, MAX(created_at) AS max_created \
                   FROM refresh_tokens \
                   WHERE is_active = {bool_true} AND replaced_at IS NULL \
                   GROUP BY family_id \
                 ) latest ON t2.family_id = latest.family_id AND t2.created_at = latest.max_created \
               )"
        );
        match kyomi_core::db_execute!(
            self.db,
            &sql,
            one_hour_ago
        ) {
            Ok(result) => {
                let count = result.rows_affected();
                if count > 0 {
                    info!(count = count, "Cleaned up orphaned active refresh tokens from concurrent rotations");
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to cleanup orphaned active refresh tokens");
            }
        }
    }

    /// Delete expired refresh tokens.
    async fn cleanup_expired_tokens(&self) {
        let is_pg = self.db.is_postgres();
        let now_fn = kyomi_core::sql_compat::now(is_pg);
        let sql = format!("DELETE FROM refresh_tokens WHERE expires_at < {now_fn}");
        match kyomi_core::db_execute!(self.db, &sql) {
            Ok(result) => {
                let count = result.rows_affected();
                if count > 0 {
                    info!(count = count, "Cleaned up expired refresh tokens");
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to cleanup expired refresh tokens");
            }
        }
    }

    /// Gracefully shut down the scheduler.
    ///
    /// 1. Signal the cancel token to stop the poll loop
    /// 2. Wait for active executions (timeout after 30s)
    /// 3. Mark any still-running executions as "error" with "interrupted by shutdown"
    pub async fn shutdown(&self) {
        info!("Shutting down watch scheduler");

        // Signal the poll loop to stop
        self.cancel.cancel();

        // Wait for active executions with timeout
        let active = {
            let active = self.active_executions.lock().await;
            active.len()
        };

        if active > 0 {
            info!(count = active, "Waiting for active watch executions to complete");

            let deadline = Instant::now() + Duration::from_secs(SHUTDOWN_TIMEOUT_SECONDS);

            loop {
                // Check if all done
                let remaining = {
                    let active = self.active_executions.lock().await;
                    active.iter().filter(|(_, h)| !h.is_finished()).count()
                };

                if remaining == 0 {
                    break;
                }

                if Instant::now() >= deadline {
                    warn!(
                        remaining = remaining,
                        "Shutdown timeout reached, {remaining} executions still running"
                    );
                    // Abort remaining tasks
                    let mut active = self.active_executions.lock().await;
                    for (wid, handle) in active.drain() {
                        if !handle.is_finished() {
                            warn!(watch_id = %wid, "Aborting watch execution on shutdown");
                            handle.abort();
                        }
                    }
                    break;
                }

                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }

        // Mark this pod's tracked watch executions as "error" if they were aborted.
        // We only clean up watches that this pod was tracking, NOT all running
        // executions globally (other pods may still be executing them).
        let aborted_watch_ids: Vec<String> = {
            let active = self.active_executions.lock().await;
            active.keys().cloned().collect()
        };

        if !aborted_watch_ids.is_empty() {
            let is_pg = self.db.is_postgres();
            let now_fn = kyomi_core::sql_compat::now(is_pg);
            let mut total_marked: u64 = 0;
            // Iterate individually — the list is small (only this pod's tracked executions)
            for wid in &aborted_watch_ids {
                let sql = format!(
                    "UPDATE watch_executions \
                     SET status = 'error', \
                         error_message = 'Execution interrupted by scheduler shutdown', \
                         completed_at = {now_fn} \
                     WHERE status = 'running' \
                       AND watch_id = $1"
                );
                match kyomi_core::db_execute!(self.db, &sql, wid) {
                    Ok(res) => total_marked += res.rows_affected(),
                    Err(e) => {
                        error!(watch_id = %wid, error = %e, "Failed to clean up running execution on shutdown");
                    }
                }
            }
            if total_marked > 0 {
                warn!(
                    count = total_marked,
                    "Marked running executions as interrupted by shutdown"
                );
            }
        }

        info!("Watch scheduler shutdown complete");
    }
}

// ---------------------------------------------------------------------------
// Query helper types
// ---------------------------------------------------------------------------

/// Row type for the due watches query.
#[derive(Debug, sqlx::FromRow)]
struct DueWatch {
    watch_id: String,
    workspace_id: String,
    schedule: String,
    next_run_at: DateTime<Utc>,
}

/// Query watches due for execution and filter out any belonging to a
/// billing-lapsed SaaS workspace (KYO-805) — split out from
/// `WatchScheduler::poll_due_watches` so the gating logic is testable
/// without constructing a full `WatchScheduler` (KV store, embedding
/// model, `WebSocketManager`, Connect registry, platform registry — none
/// of which this query or its filter need).
///
/// Routes each due watch's workspace through
/// `kyomi_auth::billing_gate::check_workspace_billing_gate` — the single
/// "load workspace, apply the KYO-805 gate, fail closed" implementation
/// also used by the WS sync handlers (`apps/server/src/routes/websocket.rs`)
/// and the catalog refresh scheduler
/// (`kyomi_agent::catalog_scheduler::CatalogRefreshScheduler`), extracted
/// (KYO-805 follow-up) so none of the three can drift from each other —
/// never re-derives lapsed-ness from a status string in SQL. A workspace
/// that fails to load, or doesn't exist, is skipped rather than executed:
/// the shared helper fails closed and logs the reason itself.
async fn due_watches_to_execute(
    db: &DbPool,
    self_hosted: bool,
    now: DateTime<Utc>,
) -> Result<Vec<DueWatch>, String> {
    let is_pg = db.is_postgres();
    let bool_true = kyomi_core::sql_compat::bool_true(is_pg);
    let due_sql = format!(
        "SELECT watch_id, workspace_id, schedule, next_run_at \
         FROM watches \
         WHERE enabled = {bool_true} \
           AND next_run_at IS NOT NULL \
           AND next_run_at <= $1"
    );
    let due_watches: Vec<DueWatch> = kyomi_core::db_fetch_all!(db, DueWatch, &due_sql, now)
        .map_err(|e| format!("failed to query due watches: {e}"))?;

    let mut executable = Vec::with_capacity(due_watches.len());
    for watch in due_watches {
        if kyomi_auth::billing_gate::check_workspace_billing_gate(
            db,
            &watch.workspace_id,
            self_hosted,
            now,
        )
        .await
        .blocks()
        {
            info!(
                watch_id = %watch.watch_id,
                workspace_id = %watch.workspace_id,
                "Skipping watch execution — workspace billing is lapsed or could not be verified"
            );
            continue;
        }
        executable.push(watch);
    }

    Ok(executable)
}

/// Row type for the missed watches query.
#[derive(sqlx::FromRow)]
struct MissedWatch {
    watch_id: String,
    name: String,
    next_run_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_interval_is_30_seconds() {
        assert_eq!(POLL_INTERVAL_SECONDS, 30);
    }

    #[test]
    fn shutdown_timeout_is_30_seconds() {
        assert_eq!(SHUTDOWN_TIMEOUT_SECONDS, 30);
    }

    #[test]
    fn missed_execution_cutoff_is_24_hours() {
        assert_eq!(MISSED_EXECUTION_CUTOFF_HOURS, 24);
    }

    #[test]
    fn catchup_jitter_range_is_valid() {
        const { assert!(CATCHUP_JITTER_MIN < CATCHUP_JITTER_MAX) };
        assert_eq!(CATCHUP_JITTER_MIN, 30);
        assert_eq!(CATCHUP_JITTER_MAX, 90);
    }

    #[test]
    fn cleanup_interval_is_24_hours() {
        assert_eq!(CLEANUP_INTERVAL_SECONDS, 86400);
    }

    #[test]
    fn deleted_alert_retention_is_30_days() {
        assert_eq!(DELETED_ALERT_RETENTION_DAYS, 30);
    }

    // -- DueWatch and MissedWatch query types --

    #[test]
    fn due_watch_has_required_fields() {
        // Verify the DueWatch struct has the fields we expect for the poll query
        let now = Utc::now();
        let watch = DueWatch {
            watch_id: "watch-abc".into(),
            workspace_id: "ws-1".into(),
            schedule: "0 9 * * *".into(),
            next_run_at: now,
        };
        assert_eq!(watch.watch_id, "watch-abc");
        assert_eq!(watch.workspace_id, "ws-1");
        assert_eq!(watch.schedule, "0 9 * * *");
        assert_eq!(watch.next_run_at, now);
    }

    #[test]
    fn missed_watch_has_required_fields() {
        let now = Utc::now();
        let watch = MissedWatch {
            watch_id: "watch-xyz".into(),
            name: "Revenue Monitor".into(),
            next_run_at: now,
        };
        assert_eq!(watch.watch_id, "watch-xyz");
        assert_eq!(watch.name, "Revenue Monitor");
        assert_eq!(watch.next_run_at, now);
    }

    // -- Catch-up jitter range validation --

    #[test]
    fn catchup_jitter_range_reasonable() {
        // Jitter min should be at least 10s to avoid immediate thundering herd
        const { assert!(CATCHUP_JITTER_MIN >= 10) };
        // Jitter max should be under 2 minutes for reasonable catch-up time
        const { assert!(CATCHUP_JITTER_MAX <= 120) };
        // Range should allow meaningful spread
        const { assert!(CATCHUP_JITTER_MAX - CATCHUP_JITTER_MIN >= 30) };
    }

    // -- Poll interval and cleanup interval relationship --

    #[test]
    fn cleanup_runs_much_less_frequently_than_polling() {
        // Cleanup should run at least 100x less frequently than polling
        const { assert!(CLEANUP_INTERVAL_SECONDS / POLL_INTERVAL_SECONDS >= 100) };
    }

    // -- Missed execution cutoff is in hours --

    #[test]
    fn missed_execution_cutoff_reasonable() {
        // Cutoff should be at least 1 hour and at most 48 hours
        const { assert!(MISSED_EXECUTION_CUTOFF_HOURS >= 1) };
        const { assert!(MISSED_EXECUTION_CUTOFF_HOURS <= 48) };
    }

    // -- due_watches_to_execute (KYO-805: skip lapsed workspaces) --

    mod due_watches_to_execute_tests {
        use super::*;
        use crate::test_support::{seed_user_and_workspace, test_pool};

        fn sqlite(db: &DbPool) -> &sqlx::SqlitePool {
            match db {
                DbPool::Sqlite(sq) => sq,
                DbPool::Postgres(_) => unreachable!("test pool is always sqlite"),
            }
        }

        async fn insert_watch(
            db: &DbPool,
            watch_id: &str,
            workspace_id: &str,
            owner: &str,
            next_run_at: DateTime<Utc>,
        ) {
            sqlx::query(
                "INSERT INTO watches \
                 (watch_id, workspace_id, created_by, name, prompt, schedule, enabled, next_run_at) \
                 VALUES ($1, $2, $3, 'Test Watch', 'test prompt', '0 9 * * *', 1, $4)",
            )
            .bind(watch_id)
            .bind(workspace_id)
            .bind(owner)
            .bind(next_run_at)
            .execute(sqlite(db))
            .await
            .expect("insert watch");
        }

        async fn insert_workspace(db: &DbPool, workspace_id: &str, owner: &str, status: &str) {
            sqlx::query(
                "INSERT INTO workspaces (workspace_id, name, owner_user_id, subscription_status) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(workspace_id)
            .bind(format!("Workspace {workspace_id}"))
            .bind(owner)
            .bind(status)
            .execute(sqlite(db))
            .await
            .expect("insert workspace");
        }

        async fn set_subscription_status(db: &DbPool, workspace_id: &str, status: &str) {
            sqlx::query("UPDATE workspaces SET subscription_status = $1 WHERE workspace_id = $2")
                .bind(status)
                .bind(workspace_id)
                .execute(sqlite(db))
                .await
                .expect("update subscription_status");
        }

        #[tokio::test]
        async fn active_workspace_watch_is_included() {
            let db = test_pool().await;
            seed_user_and_workspace(&db).await;
            let now = Utc::now();
            insert_watch(&db, "watch-1", "ws-1", "user-a", now - chrono::Duration::minutes(1)).await;

            let due = due_watches_to_execute(&db, false, now)
                .await
                .expect("query due watches");
            assert_eq!(
                due.iter().map(|w| w.watch_id.as_str()).collect::<Vec<_>>(),
                vec!["watch-1"]
            );
        }

        #[tokio::test]
        async fn lapsed_workspace_watch_is_skipped() {
            let db = test_pool().await;
            seed_user_and_workspace(&db).await;
            set_subscription_status(&db, "ws-1", "past_due").await;
            let now = Utc::now();
            insert_watch(&db, "watch-1", "ws-1", "user-a", now - chrono::Duration::minutes(1)).await;

            let due = due_watches_to_execute(&db, false, now)
                .await
                .expect("query due watches");
            assert!(
                due.is_empty(),
                "a watch belonging to a billing-lapsed workspace must be skipped, got {due:?}"
            );
        }

        #[tokio::test]
        async fn self_hosted_never_skips_even_with_past_due_workspace() {
            let db = test_pool().await;
            seed_user_and_workspace(&db).await;
            set_subscription_status(&db, "ws-1", "past_due").await;
            let now = Utc::now();
            insert_watch(&db, "watch-1", "ws-1", "user-a", now - chrono::Duration::minutes(1)).await;

            let due = due_watches_to_execute(&db, true, now)
                .await
                .expect("query due watches");
            assert_eq!(
                due.len(),
                1,
                "self-hosted/personal mode must never skip a watch on billing status"
            );
        }

        #[tokio::test]
        async fn mixed_batch_only_skips_the_lapsed_workspaces_watch() {
            let db = test_pool().await;
            seed_user_and_workspace(&db).await;
            insert_workspace(&db, "ws-2", "user-a", "past_due").await;
            let now = Utc::now();
            insert_watch(&db, "watch-active", "ws-1", "user-a", now - chrono::Duration::minutes(1)).await;
            insert_watch(&db, "watch-lapsed", "ws-2", "user-a", now - chrono::Duration::minutes(1)).await;

            let due = due_watches_to_execute(&db, false, now)
                .await
                .expect("query due watches");
            assert_eq!(
                due.iter().map(|w| w.watch_id.as_str()).collect::<Vec<_>>(),
                vec!["watch-active"]
            );
        }

        #[tokio::test]
        async fn workspace_load_error_fails_closed() {
            let db = test_pool().await;
            seed_user_and_workspace(&db).await;
            // Corrupt subscription_status so decoding the workspace row
            // fails — same technique as
            // `kyomi_auth::middleware::tests::db_error_loading_workspace_fails_closed`.
            // A DB/decode error must skip the watch, not execute it.
            set_subscription_status(&db, "ws-1", "not-a-real-status").await;
            let now = Utc::now();
            insert_watch(&db, "watch-1", "ws-1", "user-a", now - chrono::Duration::minutes(1)).await;

            let due = due_watches_to_execute(&db, false, now)
                .await
                .expect("query due watches");
            assert!(
                due.is_empty(),
                "a workspace load/decode error must fail closed (skip), got {due:?}"
            );
        }

        #[tokio::test]
        async fn not_yet_due_watch_is_excluded_regardless_of_billing() {
            let db = test_pool().await;
            seed_user_and_workspace(&db).await;
            let now = Utc::now();
            insert_watch(&db, "watch-future", "ws-1", "user-a", now + chrono::Duration::hours(1)).await;

            let due = due_watches_to_execute(&db, false, now)
                .await
                .expect("query due watches");
            assert!(due.is_empty());
        }
    }
}
