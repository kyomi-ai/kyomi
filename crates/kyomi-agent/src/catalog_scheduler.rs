// SPDX-License-Identifier: AGPL-3.0-or-later

//! Catalog Refresh Scheduler — background tasks for catalog re-indexing and housekeeping.
//!
//! Ports Python's `CatalogRefreshScheduler` from `catalog_refresh_scheduler.py`.
//!
//! # Architecture
//!
//! Five periodic tasks run as `tokio::spawn` background tasks inside the API server:
//!
//! 1. **Hourly catalog refresh** — Iterates all workspaces and their active datasources,
//!    checks if last refresh was >24h ago, and triggers re-indexing. Uses a Redis
//!    distributed lock (SETNX with TTL) to prevent concurrent runs across replicas.
//!
//! 2. **Daily token cleanup** — Deletes expired refresh tokens and verification tokens.
//!    Also uses a Redis distributed lock.
//!
//! 3. **Daily knowledge maintenance** — Contradiction + staleness detection across workspace knowledge.
//!
//! 4. **Daily query history cleanup** — Deletes unstarred queries older than retention period.
//!
//! 5. **Daily public dataset refresh** — Indexes curated BigQuery public datasets
//!    (bigquery-public-data) into a sentinel workspace. Requires a BigQuery datasource
//!    with `auth_mode=service_account` for GCP token.
//!
//! # Credential Resolution Priority
//!
//! 1. `indexing_credentials` from connection_config (if valid and non-OAuth)
//! 2. Shared credentials from connection_config
//! 3. Workspace owner's stored credentials
//!
//! # Multi-replica Safety
//!
//! Both tasks use Redis SETNX with TTL for distributed locking. Only one replica
//! runs the refresh or cleanup at a time.
//!
//! # Graceful Shutdown
//!
//! Uses a shared `CancellationToken` (same as `WatchScheduler`) for coordinated
//! shutdown on SIGTERM/SIGINT.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use kyomi_auth::catalog::helpers as catalog_helpers;
use kyomi_core::{DbPool, KVPool};
use kyomi_embed::LazyEmbedding;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// How often the catalog refresh check runs (1 hour).
const REFRESH_INTERVAL_SECONDS: u64 = 3600;

/// Initial delay before the first refresh run (1 minute).
///
/// Allows the application to fully initialize before starting background work.
const REFRESH_INITIAL_DELAY_SECONDS: u64 = 60;

/// How often the token cleanup runs (24 hours).
const CLEANUP_INTERVAL_SECONDS: u64 = 86400;

/// Initial delay before the first cleanup run (12 hours).
///
/// No rush to clean up expired tokens — they are harmless until cleaned.
const CLEANUP_INITIAL_DELAY_SECONDS: u64 = 43200;

/// How often knowledge maintenance runs (24 hours).
const KNOWLEDGE_MAINTENANCE_INTERVAL_SECONDS: u64 = 86400;

/// Initial delay before the first knowledge maintenance run (6 hours).
///
/// Stagger with existing tasks for load distribution.
const KNOWLEDGE_MAINTENANCE_INITIAL_DELAY_SECONDS: u64 = 21600;

/// How often query history cleanup runs (24 hours).
const QUERY_HISTORY_CLEANUP_INTERVAL_SECONDS: u64 = 86400;

/// Initial delay before the first query history cleanup run (3 hours).
///
/// Stagger with token cleanup (12h) and knowledge maintenance (6h) for load distribution.
const QUERY_HISTORY_CLEANUP_INITIAL_DELAY_SECONDS: u64 = 10800;

/// Redis lock key for query history cleanup.
const QUERY_HISTORY_CLEANUP_LOCK_KEY: &str = "query_history_cleanup_lock";

/// Redis lock TTL for query history cleanup (1 hour).
const QUERY_HISTORY_CLEANUP_LOCK_TTL: u64 = 3600;

/// Default retention period in days for query history entries.
const DEFAULT_QUERY_HISTORY_RETENTION_DAYS: i64 = 30;

/// Redis lock key for knowledge maintenance.
const KNOWLEDGE_MAINTENANCE_LOCK_KEY: &str = "knowledge_maintenance_lock";

/// Redis lock TTL for knowledge maintenance (1 hour).
const KNOWLEDGE_MAINTENANCE_LOCK_TTL: u64 = 3600;

/// Redis lock key for catalog refresh. Only one replica runs at a time.
const CATALOG_REFRESH_LOCK_KEY: &str = "catalog_refresh_lock";

/// Redis lock TTL (1 hour — matches the check interval).
const CATALOG_REFRESH_LOCK_TTL: u64 = 3600;

/// Redis lock key for token cleanup.
const TOKEN_CLEANUP_LOCK_KEY: &str = "token_cleanup_lock";

/// Redis lock TTL for token cleanup (1 hour).
const TOKEN_CLEANUP_LOCK_TTL: u64 = 3600;

/// How often the public dataset refresh runs (24 hours).
const PUBLIC_DATASET_REFRESH_INTERVAL_SECONDS: u64 = 86400;

/// Initial delay before the first public dataset refresh (5 minutes).
///
/// Stagger after the regular catalog refresh (1 min) so the embedding
/// model is loaded by the time we need it.
const PUBLIC_DATASET_REFRESH_INITIAL_DELAY_SECONDS: u64 = 300;

/// Redis lock key for public dataset refresh.
const PUBLIC_DATASET_REFRESH_LOCK_KEY: &str = "catalog:public_dataset_refresh_lock";

/// Redis lock TTL for public dataset refresh (30 minutes).
const PUBLIC_DATASET_REFRESH_LOCK_TTL: u64 = 1800;

/// Per-datasource refresh threshold in hours.
const REFRESH_HOURS_THRESHOLD: i64 = 24;

// ---------------------------------------------------------------------------
// CatalogRefreshScheduler
// ---------------------------------------------------------------------------

/// Background scheduler for catalog re-indexing and housekeeping.
///
/// Spawns two periodic tasks:
/// 1. Hourly catalog refresh check (iterates workspaces, checks staleness, triggers re-index)
/// 2. Daily token cleanup (expired refresh tokens + verification tokens)
///
/// Both tasks use Redis distributed locks for multi-replica safety and respect
/// a shared `CancellationToken` for graceful shutdown.
pub struct CatalogRefreshScheduler {
    db: DbPool,
    kv: KVPool,
    encryption_key: std::sync::Arc<[u8; 32]>,
    embedding: LazyEmbedding,
    cancel: CancellationToken,
}

impl CatalogRefreshScheduler {
    /// Create a new `CatalogRefreshScheduler`.
    pub fn new(
        db: DbPool,
        kv: KVPool,
        encryption_key: std::sync::Arc<[u8; 32]>,
        embedding: LazyEmbedding,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            db,
            kv,
            encryption_key,
            embedding,
            cancel,
        }
    }

    /// Start all background tasks.
    ///
    /// Returns `JoinHandle`s for all tasks. The tasks run until the cancel
    /// token is triggered.
    #[allow(clippy::type_complexity)]
    pub fn start(self: Arc<Self>) -> (JoinHandle<()>, JoinHandle<()>, JoinHandle<()>, JoinHandle<()>, JoinHandle<()>) {
        let refresh_handle = {
            let this = self.clone();
            tokio::spawn(async move {
                this.refresh_loop().await;
            })
        };

        let cleanup_handle = {
            let this = self.clone();
            tokio::spawn(async move {
                this.cleanup_loop().await;
            })
        };

        let maintenance_handle = {
            let this = self.clone();
            tokio::spawn(async move {
                this.knowledge_maintenance_loop().await;
            })
        };

        let query_history_cleanup_handle = {
            let this = self.clone();
            tokio::spawn(async move {
                this.query_history_cleanup_loop().await;
            })
        };

        let public_dataset_handle = {
            let this = self.clone();
            tokio::spawn(async move {
                this.public_dataset_refresh_loop().await;
            })
        };

        (refresh_handle, cleanup_handle, maintenance_handle, query_history_cleanup_handle, public_dataset_handle)
    }

    // -----------------------------------------------------------------------
    // Catalog refresh loop
    // -----------------------------------------------------------------------

    /// Hourly catalog refresh loop.
    ///
    /// After an initial delay, runs every hour until cancelled. Each iteration
    /// acquires a Redis lock, iterates all workspaces and their datasources,
    /// and triggers re-indexing for stale datasources (>24h since last refresh).
    async fn refresh_loop(self: &Arc<Self>) {
        // Initial delay — allow app to fully initialize
        tokio::select! {
            _ = self.cancel.cancelled() => {
                info!("Catalog refresh scheduler cancelled during initial delay");
                return;
            }
            _ = tokio::time::sleep(Duration::from_secs(REFRESH_INITIAL_DELAY_SECONDS)) => {}
        }

        info!("Catalog refresh scheduler started (hourly checks)");

        loop {
            // Try to acquire distributed lock
            if try_acquire_lock(&self.kv, CATALOG_REFRESH_LOCK_KEY, CATALOG_REFRESH_LOCK_TTL)
                .await
            {
                info!("Starting hourly catalog refresh check");

                self.refresh_all_workspaces().await;

                // Release the lock after completion
                release_lock(&self.kv, CATALOG_REFRESH_LOCK_KEY).await;
            } else {
                debug!("Catalog refresh lock held by another replica, skipping");
            }

            // Wait for the next interval, or cancel
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    info!("Catalog refresh scheduler received cancel signal");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(REFRESH_INTERVAL_SECONDS)) => {
                    // Normal timeout — continue polling
                }
            }
        }

        info!("Catalog refresh scheduler exited");
    }

    /// Refresh catalog for all eligible datasources across all workspaces.
    ///
    /// For each workspace:
    /// 1. Query all active datasources
    /// 2. For each datasource: check if last refresh was >24h ago
    /// 3. If stale: trigger re-indexing via `CatalogIndexingService::index_datasource`
    async fn refresh_all_workspaces(&self) {
        // Get all workspaces
        #[derive(sqlx::FromRow)]
        struct WsRow { workspace_id: String }

        let workspaces = match kyomi_core::db_fetch_all!(
            self.db, WsRow,
            "SELECT workspace_id FROM workspaces"
        ) {
            Ok(ws) => ws,
            Err(e) => {
                warn!(error = %e, "Failed to list workspaces for catalog refresh");
                return;
            }
        };

        let mut refreshed_count = 0usize;
        let mut skipped_count = 0usize;
        let mut failed_count = 0usize;

        for ws_row in &workspaces {
            let workspace_id = &ws_row.workspace_id;

            // Get all active datasources for this workspace
            #[derive(sqlx::FromRow)]
            struct DsRow {
                id: String,
                datasource_type: String,
                connection_config: Value,
                name: String,
            }

            let is_pg = self.db.is_postgres();
            let bool_true = kyomi_core::sql_compat::bool_true(is_pg);
            let ds_sql = format!(
                "SELECT id, datasource_type, connection_config, name \
                 FROM datasource_configs \
                 WHERE workspace_id = $1 AND active = {bool_true}"
            );
            let datasources = match kyomi_core::db_fetch_all!(
                self.db, DsRow,
                &ds_sql,
                workspace_id
            ) {
                Ok(ds) => ds,
                Err(e) => {
                    warn!(
                        workspace_id,
                        error = %e,
                        "Failed to list datasources for workspace"
                    );
                    continue;
                }
            };

            if datasources.is_empty() {
                debug!(workspace_id, "No active datasources, skipping");
                continue;
            }

            // Get workspace owner's email (needed for credential lookup)
            let owner_email =
                catalog_helpers::get_workspace_owner_email(&self.db, workspace_id).await;

            for ds_row in &datasources {
                let ds_id = &ds_row.id;
                let ds_type = &ds_row.datasource_type;
                let ds_name = &ds_row.name;
                let connection_config = &ds_row.connection_config;

                match self
                    .refresh_datasource(
                        workspace_id,
                        ds_id,
                        ds_type,
                        ds_name,
                        connection_config,
                        owner_email.as_deref(),
                    )
                    .await
                {
                    RefreshResult::Refreshed => refreshed_count += 1,
                    RefreshResult::Skipped => skipped_count += 1,
                    RefreshResult::Failed => failed_count += 1,
                }
            }
        }

        info!(
            refreshed = refreshed_count,
            skipped = skipped_count,
            failed = failed_count,
            "Hourly catalog refresh check completed"
        );
    }

    /// Refresh a single datasource's catalog.
    ///
    /// Checks rate limiting, resolves credentials, and triggers re-indexing.
    async fn refresh_datasource(
        &self,
        workspace_id: &str,
        datasource_config_id: &str,
        datasource_type: &str,
        datasource_name: &str,
        connection_config: &Value,
        owner_email: Option<&str>,
    ) -> RefreshResult {
        // Check if datasource can be refreshed (24hr threshold)
        if !catalog_helpers::can_refresh_now(&self.db, datasource_config_id, REFRESH_HOURS_THRESHOLD)
            .await
        {
            debug!(
                workspace_id,
                datasource = datasource_name,
                "Rate limited or recently refreshed, skipping"
            );
            return RefreshResult::Skipped;
        }

        // Get indexing credentials
        let (_user_email, indexing_creds) = get_indexing_credentials(connection_config);

        // Determine effective email for credential lookup
        let effective_email = owner_email.unwrap_or("");

        if effective_email.is_empty() && indexing_creds.is_none() {
            debug!(
                workspace_id,
                datasource = datasource_name,
                "No credentials available, skipping"
            );
            return RefreshResult::Skipped;
        }

        // Wait for the embedding service to be ready
        let embedding = match self.embedding.wait_ready().await {
            Ok(e) => e,
            Err(e) => {
                warn!(
                    workspace_id,
                    datasource = datasource_name,
                    error = %e,
                    "Embedding service not ready, skipping catalog refresh"
                );
                return RefreshResult::Skipped;
            }
        };

        let result = crate::catalog::indexing_service::CatalogIndexingService::index_datasource(
            crate::catalog::indexing_service::IndexDatasourceParams {
                db: &self.db,
                encryption_key: self.encryption_key.clone(),
                embedding,
                workspace_id,
                datasource_config_id,
                user_email: Some(effective_email),
                credentials: indexing_creds.as_ref(),
                max_tables_per_dataset: None,
                force: false,
            },
        )
        .await;

        match result.status.as_str() {
            "completed" => {
                info!(
                    workspace_id,
                    datasource = datasource_name,
                    datasource_type,
                    tables_indexed = result.tables_indexed,
                    tables_archived = result.tables_archived,
                    "Catalog refresh completed"
                );
            }
            "skipped" => {
                debug!(
                    workspace_id,
                    datasource = datasource_name,
                    datasource_type,
                    errors = ?result.errors,
                    "Catalog refresh skipped"
                );
                return RefreshResult::Skipped;
            }
            "error" => {
                warn!(
                    workspace_id,
                    datasource = datasource_name,
                    datasource_type,
                    errors = ?result.errors,
                    "Catalog refresh failed"
                );
                return RefreshResult::Failed;
            }
            other => {
                warn!(
                    workspace_id,
                    datasource = datasource_name,
                    status = other,
                    "Unexpected catalog refresh status"
                );
            }
        }

        // Populate graph with the freshly indexed catalog data
        self.populate_embeddings_for_datasource(workspace_id, datasource_config_id, datasource_name)
            .await;

        RefreshResult::Refreshed
    }

    /// Fire-and-forget graph population for a datasource after catalog indexing.
    ///
    /// Connects to the workspace graph and populates table/column nodes from
    /// the freshly cached catalog data in PostgreSQL. Failures are logged as
    /// warnings -- they never fail the indexing operation.
    async fn populate_embeddings_for_datasource(
        &self,
        workspace_id: &str,
        datasource_config_id: &str,
        datasource_name: &str,
    ) {
        let embed = match self.embedding.wait_ready().await {
            Ok(e) => e,
            Err(e) => {
                warn!(
                    workspace_id,
                    datasource = datasource_name,
                    error = %e,
                    "Embedding service not available, skipping embedding population"
                );
                return;
            }
        };

        match kyomi_knowledge::populate::populate_table_embeddings(
            &self.db,
            embed,
            workspace_id,
            datasource_config_id,
        )
        .await
        {
            Ok(table_count) => {
                match kyomi_knowledge::populate::populate_column_embeddings(
                    &self.db,
                    embed,
                    workspace_id,
                    datasource_config_id,
                )
                .await
                {
                    Ok(col_count) => {
                        info!(
                            workspace_id,
                            datasource = datasource_name,
                            tables = table_count,
                            columns = col_count,
                            "Embeddings populated for datasource after catalog refresh"
                        );
                    }
                    Err(e) => {
                        warn!(
                            error = %e,
                            "Column embedding population failed, continuing"
                        );
                    }
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Table embedding population failed, continuing"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // BigQuery public dataset refresh loop
    // -----------------------------------------------------------------------

    /// Daily public dataset refresh loop.
    ///
    /// After an initial delay (5 minutes), runs every 24 hours until cancelled.
    /// Indexes curated BigQuery public datasets (bigquery-public-data) into
    /// a sentinel workspace (`public-data-workspace`) so they appear in search
    /// results and catalog trees for workspaces that have `include_public_datasets` enabled.
    ///
    /// Requires at least one BigQuery datasource with `auth_mode = "service_account"`
    /// to obtain a GCP access token. If none exists, the loop skips gracefully.
    async fn public_dataset_refresh_loop(self: &Arc<Self>) {
        // Initial delay — allow regular catalog refresh to start first
        tokio::select! {
            _ = self.cancel.cancelled() => {
                info!("Public dataset refresh scheduler cancelled during initial delay");
                return;
            }
            _ = tokio::time::sleep(Duration::from_secs(PUBLIC_DATASET_REFRESH_INITIAL_DELAY_SECONDS)) => {}
        }

        info!("Public dataset refresh scheduler started (daily)");

        loop {
            if try_acquire_lock(
                &self.kv,
                PUBLIC_DATASET_REFRESH_LOCK_KEY,
                PUBLIC_DATASET_REFRESH_LOCK_TTL,
            )
            .await
            {
                self.refresh_public_datasets().await;

                release_lock(&self.kv, PUBLIC_DATASET_REFRESH_LOCK_KEY).await;
            } else {
                debug!("Public dataset refresh lock held by another replica, skipping");
            }

            tokio::select! {
                _ = self.cancel.cancelled() => {
                    info!("Public dataset refresh scheduler received cancel signal");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(PUBLIC_DATASET_REFRESH_INTERVAL_SECONDS)) => {}
            }
        }

        info!("Public dataset refresh scheduler exited");
    }

    /// Refresh BigQuery public datasets if stale.
    ///
    /// 1. Check if public datasets need refreshing (24h threshold)
    /// 2. Find a BigQuery datasource with service account credentials for token
    /// 3. Exchange service account JWT for access token
    /// 4. Run the public dataset indexer
    /// 5. Populate the knowledge graph for the public workspace
    async fn refresh_public_datasets(&self) {
        use kyomi_auth::catalog::indexers::bigquery_public::BigQueryPublicIndexer;

        // Check if we need to refresh
        if !BigQueryPublicIndexer::needs_refresh(&self.db, REFRESH_HOURS_THRESHOLD).await {
            debug!("Public datasets recently refreshed, skipping");
            return;
        }

        info!("Public datasets are stale, starting refresh");

        // Find a BigQuery datasource with service account auth to get a GCP token.
        // Public datasets are publicly readable — any valid GCP token works.
        let access_token = match self.resolve_service_account_token().await {
            Some(token) => token,
            None => {
                info!(
                    "No BigQuery service account datasource found — \
                     skipping public dataset indexing. Configure a BigQuery \
                     datasource with auth_mode=service_account to enable."
                );
                return;
            }
        };

        // Wait for the embedding model
        let embedding = match self.embedding.wait_ready().await {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "Embedding service not ready, skipping public dataset refresh");
                return;
            }
        };

        // Run the indexer
        let result = BigQueryPublicIndexer::index_public_datasets(
            &self.db,
            embedding,
            &access_token,
            None, // max_tables_per_dataset — use default
        )
        .await;

        match result.status.as_str() {
            "completed" => {
                info!(
                    tables_indexed = result.tables_indexed,
                    tables_archived = result.tables_archived,
                    "Public dataset indexing completed"
                );
            }
            "skipped" => {
                debug!(errors = ?result.errors, "Public dataset indexing skipped");
                return;
            }
            "error" => {
                warn!(errors = ?result.errors, "Public dataset indexing failed");
                return;
            }
            _ => {}
        }

        // Populate graph for the public dataset workspace
        self.populate_embeddings_for_datasource(
            kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID,
            "bigquery-public-indexer",
            "BigQuery Public Datasets",
        )
        .await;
    }

    /// Find a BigQuery datasource with service account auth and exchange for an access token.
    ///
    /// Scans all active BigQuery datasources across all workspaces for one with
    /// `auth_mode = "service_account"` and `service_account_json` in its connection_config.
    async fn resolve_service_account_token(&self) -> Option<String> {
        // Query for any BigQuery datasource with service_account auth
        #[derive(sqlx::FromRow)]
        struct ConfigRow { connection_config: Value }

        let is_pg = self.db.is_postgres();
        let bool_true = kyomi_core::sql_compat::bool_true(is_pg);
        let auth_mode = kyomi_core::sql_compat::json_extract_text(is_pg, "connection_config", "auth_mode");
        let sa_json = kyomi_core::sql_compat::json_extract_text(is_pg, "connection_config", "service_account_json");
        let sa_sql = format!(
            "SELECT connection_config \
             FROM datasource_configs \
             WHERE datasource_type = 'bigquery' \
               AND active = {bool_true} \
               AND {auth_mode} = 'service_account' \
               AND {sa_json} IS NOT NULL \
             LIMIT 1"
        );
        let row = kyomi_core::db_fetch_optional!(
            self.db, ConfigRow,
            &sa_sql
        )
        .ok()
        .flatten();

        let row = row?;

        // Exchange service account JWT for access token
        let client = match kyomi_datasource_server::http_client() {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "Failed to create HTTP client for service account token exchange");
                return None;
            }
        };

        match kyomi_datasource_server::providers::bigquery::exchange_service_account_jwt(
            &client,
            &row.connection_config,
        )
        .await
        {
            Ok((token, _project_id)) => Some(token),
            Err(e) => {
                warn!(error = %e, "Failed to exchange service account JWT for public dataset indexing");
                None
            }
        }
    }

    // -----------------------------------------------------------------------
    // Query history cleanup loop
    // -----------------------------------------------------------------------

    /// Daily query history cleanup loop.
    ///
    /// After an initial delay (3 hours), runs every 24 hours until cancelled.
    /// Each iteration acquires a Redis lock, then for each user:
    /// - Reads `extra_metadata->'query_history_retention_days'` (default 30)
    /// - Deletes unstarred queries older than the retention period
    /// - Starred queries (`is_saved = true`) are never deleted
    ///
    /// Matches Python's `QueryHistoryCleanupScheduler._cleanup_query_history`.
    async fn query_history_cleanup_loop(self: &Arc<Self>) {
        // Initial delay — stagger with existing tasks
        tokio::select! {
            _ = self.cancel.cancelled() => {
                info!("Query history cleanup scheduler cancelled during initial delay");
                return;
            }
            _ = tokio::time::sleep(Duration::from_secs(QUERY_HISTORY_CLEANUP_INITIAL_DELAY_SECONDS)) => {}
        }

        info!("Query history cleanup scheduler started (daily)");

        loop {
            if try_acquire_lock(
                &self.kv,
                QUERY_HISTORY_CLEANUP_LOCK_KEY,
                QUERY_HISTORY_CLEANUP_LOCK_TTL,
            )
            .await
            {
                info!("Starting daily query history cleanup");

                cleanup_query_history(&self.db).await;

                release_lock(&self.kv, QUERY_HISTORY_CLEANUP_LOCK_KEY).await;
            } else {
                debug!("Query history cleanup lock held by another replica, skipping");
            }

            tokio::select! {
                _ = self.cancel.cancelled() => {
                    info!("Query history cleanup scheduler received cancel signal");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(QUERY_HISTORY_CLEANUP_INTERVAL_SECONDS)) => {}
            }
        }

        info!("Query history cleanup scheduler exited");
    }

    // -----------------------------------------------------------------------
    // Token cleanup loop
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Graph maintenance loop
    // -----------------------------------------------------------------------

    /// Daily graph maintenance loop.
    ///
    /// After an initial delay (6 hours), runs every 24 hours until cancelled.
    /// Each iteration acquires a Redis lock, then for each workspace:
    /// - Runs contradiction detection (metrics with multiple conflicting definitions)
    /// - Runs staleness detection (learnings older than 90 days)
    ///
    /// Failures are logged as warnings — they never block the scheduler.
    async fn knowledge_maintenance_loop(self: &Arc<Self>) {
        // Initial delay — stagger with existing tasks
        tokio::select! {
            _ = self.cancel.cancelled() => {
                info!("Knowledge maintenance scheduler cancelled during initial delay");
                return;
            }
            _ = tokio::time::sleep(Duration::from_secs(KNOWLEDGE_MAINTENANCE_INITIAL_DELAY_SECONDS)) => {}
        }

        info!("Knowledge maintenance scheduler started (daily)");

        loop {
            if try_acquire_lock(&self.kv, KNOWLEDGE_MAINTENANCE_LOCK_KEY, KNOWLEDGE_MAINTENANCE_LOCK_TTL)
                .await
            {
                info!("Starting daily knowledge maintenance");

                self.run_knowledge_maintenance().await;

                release_lock(&self.kv, KNOWLEDGE_MAINTENANCE_LOCK_KEY).await;
            } else {
                debug!("Knowledge maintenance lock held by another replica, skipping");
            }

            tokio::select! {
                _ = self.cancel.cancelled() => {
                    info!("Knowledge maintenance scheduler received cancel signal");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(KNOWLEDGE_MAINTENANCE_INTERVAL_SECONDS)) => {}
            }
        }

        info!("Knowledge maintenance scheduler exited");
    }

    /// Run contradiction + staleness detection across all workspaces.
    async fn run_knowledge_maintenance(&self) {
        #[derive(sqlx::FromRow)]
        struct WsRow { workspace_id: String }

        let workspaces = match kyomi_core::db_fetch_all!(
            self.db, WsRow,
            "SELECT workspace_id FROM workspaces"
        ) {
            Ok(ws) => ws,
            Err(e) => {
                warn!(error = %e, "Failed to list workspaces for knowledge maintenance");
                return;
            }
        };

        let mut contradiction_total = 0usize;
        let mut stale_total = 0usize;

        for ws_row in &workspaces {
            let workspace_id = &ws_row.workspace_id;

            // Contradiction detection
            match kyomi_knowledge::episodic::detect_contradictions(&self.db, workspace_id).await {
                Ok(contradictions) if !contradictions.is_empty() => {
                    for c in &contradictions {
                        warn!(
                            workspace_id,
                            metric = %c.metric_name,
                            definitions = c.conflicts.len(),
                            "Metric has conflicting definitions"
                        );
                    }
                    contradiction_total += contradictions.len();
                }
                Err(e) => {
                    warn!(
                        workspace_id,
                        error = %e,
                        "Contradiction detection failed"
                    );
                }
                _ => {}
            }

            // Backfill learning references (ensures pre-migration learnings get reference rows)
            if let Err(e) = kyomi_knowledge::references::backfill_all_references(&self.db, workspace_id).await {
                warn!(
                    workspace_id,
                    error = %e,
                    "Learning references backfill failed"
                );
            }

            // Staleness detection
            match kyomi_knowledge::episodic::detect_stale_learnings(&self.db, workspace_id, 90).await {
                Ok(stale) if !stale.is_empty() => {
                    info!(
                        workspace_id,
                        count = stale.len(),
                        "Found stale learnings (>90 days old)"
                    );
                    stale_total += stale.len();
                }
                Err(e) => {
                    warn!(
                        workspace_id,
                        error = %e,
                        "Staleness detection failed"
                    );
                }
                _ => {}
            }
        }

        info!(
            contradictions = contradiction_total,
            stale_learnings = stale_total,
            "Knowledge maintenance completed"
        );
    }

    // -----------------------------------------------------------------------
    // Token cleanup loop
    // -----------------------------------------------------------------------

    /// Daily token cleanup loop.
    ///
    /// After an initial delay (12 hours), runs every 24 hours until cancelled.
    /// Each iteration acquires a Redis lock and deletes expired tokens.
    async fn cleanup_loop(self: &Arc<Self>) {
        // Initial delay — no rush to clean up expired tokens
        tokio::select! {
            _ = self.cancel.cancelled() => {
                info!("Token cleanup scheduler cancelled during initial delay");
                return;
            }
            _ = tokio::time::sleep(Duration::from_secs(CLEANUP_INITIAL_DELAY_SECONDS)) => {}
        }

        info!("Token cleanup scheduler started (daily)");

        loop {
            // Try to acquire distributed lock
            if try_acquire_lock(&self.kv, TOKEN_CLEANUP_LOCK_KEY, TOKEN_CLEANUP_LOCK_TTL).await {
                info!("Starting daily token cleanup");

                cleanup_expired_tokens(&self.db).await;

                release_lock(&self.kv, TOKEN_CLEANUP_LOCK_KEY).await;
            } else {
                debug!("Token cleanup lock held by another replica, skipping");
            }

            // Wait for the next interval, or cancel
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    info!("Token cleanup scheduler received cancel signal");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(CLEANUP_INTERVAL_SECONDS)) => {
                    // Normal timeout — continue
                }
            }
        }

        info!("Token cleanup scheduler exited");
    }
}

// ---------------------------------------------------------------------------
// Refresh result
// ---------------------------------------------------------------------------

/// Outcome of a single datasource refresh attempt.
enum RefreshResult {
    /// Successfully refreshed.
    Refreshed,
    /// Skipped (rate limited, no credentials, etc.).
    Skipped,
    /// Failed (error during refresh).
    Failed,
}

// ---------------------------------------------------------------------------
// Redis distributed locking
// ---------------------------------------------------------------------------

/// Try to acquire a distributed lock using the KV store.
///
/// Uses a simple SET-if-absent pattern. In single-instance mode (in-memory KV),
/// this still provides mutual exclusion within the process. In multi-replica mode
/// (Redis-backed KV), it provides cross-replica locking.
///
/// Returns `true` if the lock was acquired, `false` if already held.
async fn try_acquire_lock(kv: &KVPool, key: &str, ttl_seconds: u64) -> bool {
    // Check if the key already exists (approximates SET NX behavior).
    // In single-instance mode this is sufficient; in multi-replica mode
    // there's a small race window, but the catalog scheduler tolerates
    // occasional duplicate runs (operations are idempotent).
    match kv.get(key).await {
        Ok(Some(_)) => false, // lock already held
        Ok(None) => {
            // Try to set the lock
            kv.set(key, "1", Some(ttl_seconds)).await.is_ok()
        }
        Err(_) => false, // on error, don't acquire
    }
}

/// Release a distributed lock.
async fn release_lock(kv: &KVPool, key: &str) {
    let _ = kv.del(key).await;
}

// ---------------------------------------------------------------------------
// Credential resolution
// ---------------------------------------------------------------------------

/// Get indexing credentials from the datasource's connection_config.
///
/// Checks for dedicated `indexing_credentials` in the connection_config.
/// Rejects OAuth credentials (cannot refresh tokens in background jobs).
///
/// Returns `(user_email, credentials)` where credentials are `None` if
/// the indexer should use the workspace owner's credentials.
fn get_indexing_credentials(connection_config: &Value) -> (Option<String>, Option<Value>) {
    let indexing_creds = connection_config.get("indexing_credentials");

    let Some(creds) = indexing_creds else {
        return (None, None);
    };

    if !creds.is_object() {
        return (None, None);
    }

    let cred_type = creds
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if cred_type.is_empty() {
        warn!("indexing_credentials missing 'type' field, falling back to owner credentials");
        return (None, None);
    }

    // Reject OAuth — no automated token refresh for background jobs
    let auth_type = creds
        .get("auth_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if cred_type == "oauth" || auth_type == "oauth" {
        warn!(
            "OAuth indexing credentials not supported for background jobs, \
             falling back to owner credentials"
        );
        return (None, None);
    }

    debug!(cred_type, "Using dedicated indexing credentials");

    (None, Some(creds.clone()))
}

// ---------------------------------------------------------------------------
// Token cleanup
// ---------------------------------------------------------------------------

/// Clean up expired tokens from the database.
///
/// Removes:
/// - Expired refresh tokens (sessions)
/// - Replaced refresh tokens past the grace period (1 hour buffer)
/// - Revoked refresh tokens older than 30 days
/// - Expired verification tokens
async fn cleanup_expired_tokens(db: &DbPool) {
    let now = Utc::now();

    // Clean up expired refresh tokens (sessions)
    match kyomi_core::db_execute!(
        db,
        "DELETE FROM refresh_tokens WHERE expires_at < $1",
        now
    ) {
        Ok(result) => {
            let count = result.rows_affected();
            if count > 0 {
                info!(count, "Cleaned up expired sessions");
            } else {
                debug!("No expired sessions to clean up");
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to clean up expired sessions");
        }
    }

    // Clean up replaced tokens past the grace period (1 hour buffer, well past 30s grace)
    let one_hour_ago = now - chrono::Duration::hours(1);
    match kyomi_core::db_execute!(
        db,
        "DELETE FROM refresh_tokens WHERE replaced_at IS NOT NULL AND replaced_at < $1",
        one_hour_ago
    ) {
        Ok(result) => {
            let count = result.rows_affected();
            if count > 0 {
                info!(count, "Cleaned up replaced refresh tokens past grace period");
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to clean up replaced refresh tokens");
        }
    }

    // Clean up revoked tokens older than 30 days
    let thirty_days_ago = now - chrono::Duration::days(30);
    let is_pg = db.is_postgres();
    let bool_false = kyomi_core::sql_compat::bool_false(is_pg);
    let revoked_sql = format!(
        "DELETE FROM refresh_tokens WHERE is_active = {bool_false} AND revoked_at IS NOT NULL AND revoked_at < $1"
    );
    match kyomi_core::db_execute!(
        db,
        &revoked_sql,
        thirty_days_ago
    ) {
        Ok(result) => {
            let count = result.rows_affected();
            if count > 0 {
                info!(count, "Cleaned up revoked refresh tokens older than 30 days");
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to clean up revoked refresh tokens");
        }
    }

    // Clean up expired verification tokens
    match kyomi_core::db_execute!(
        db,
        "DELETE FROM verification_tokens WHERE expires_at < $1",
        now
    ) {
        Ok(result) => {
            let count = result.rows_affected();
            if count > 0 {
                info!(count, "Cleaned up expired verification tokens");
            }
        }
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("does not exist") || err_msg.contains("no such table") {
                debug!(
                    error = %e,
                    "verification_tokens table does not exist, skipping cleanup"
                );
            } else {
                warn!(error = %e, "Failed to clean up expired verification tokens");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Query history cleanup
// ---------------------------------------------------------------------------

/// Clean up old query history entries for all users.
///
/// For each user:
/// - Gets their `query_history_retention_days` preference from `extra_metadata` (default 30)
/// - Deletes unstarred queries (`is_saved = false`) older than the retention period
/// - Never deletes starred/saved queries
///
/// Matches Python's `QueryHistoryCleanupScheduler._cleanup_query_history`.
async fn cleanup_query_history(db: &DbPool) {
    // Get all users with their extra_metadata
    #[derive(sqlx::FromRow)]
    struct UserRow {
        user_id: String,
        extra_metadata: Option<Value>,
    }

    let users = match kyomi_core::db_fetch_all!(
        db, UserRow,
        "SELECT user_id, extra_metadata FROM users"
    ) {
        Ok(u) => u,
        Err(e) => {
            warn!(error = %e, "Failed to list users for query history cleanup");
            return;
        }
    };

    let mut total_deleted: u64 = 0;
    let mut users_processed: u64 = 0;
    let mut users_skipped: u64 = 0;

    for user_row in &users {
        let user_id = &user_row.user_id;

        // Get user's retention preference (default 30 days)
        let retention_days = user_row
            .extra_metadata
            .as_ref()
            .and_then(|m| m.get("query_history_retention_days"))
            .and_then(|v| v.as_i64())
            .unwrap_or(DEFAULT_QUERY_HISTORY_RETENTION_DAYS);

        // Calculate cutoff date
        let cutoff = Utc::now() - chrono::Duration::days(retention_days);

        // Delete unstarred queries older than cutoff
        let is_pg = db.is_postgres();
        let bool_false = kyomi_core::sql_compat::bool_false(is_pg);
        let del_sql = format!(
            "DELETE FROM sql_query_history \
             WHERE user_id = $1 AND is_saved = {bool_false} AND executed_at < $2"
        );
        match kyomi_core::db_execute!(
            db,
            &del_sql,
            user_id,
            cutoff
        ) {
            Ok(result) => {
                let count = result.rows_affected();
                if count > 0 {
                    total_deleted += count;
                    users_processed += 1;
                    debug!(
                        user_id,
                        deleted = count,
                        retention_days,
                        "Deleted old query history entries"
                    );
                } else {
                    users_skipped += 1;
                }
            }
            Err(e) => {
                error!(
                    user_id,
                    error = %e,
                    "Failed to clean query history for user"
                );
            }
        }
    }

    info!(
        total_deleted,
        users_processed,
        users_skipped,
        "Query history cleanup completed"
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_interval_is_1_hour() {
        assert_eq!(REFRESH_INTERVAL_SECONDS, 3600);
    }

    #[test]
    fn refresh_initial_delay_is_1_minute() {
        assert_eq!(REFRESH_INITIAL_DELAY_SECONDS, 60);
    }

    #[test]
    fn cleanup_interval_is_24_hours() {
        assert_eq!(CLEANUP_INTERVAL_SECONDS, 86400);
    }

    #[test]
    fn cleanup_initial_delay_is_12_hours() {
        assert_eq!(CLEANUP_INITIAL_DELAY_SECONDS, 43200);
    }

    #[test]
    fn lock_key_constants_are_stable() {
        assert_eq!(CATALOG_REFRESH_LOCK_KEY, "catalog_refresh_lock");
        assert_eq!(TOKEN_CLEANUP_LOCK_KEY, "token_cleanup_lock");
        assert_eq!(QUERY_HISTORY_CLEANUP_LOCK_KEY, "query_history_cleanup_lock");
        assert_eq!(
            PUBLIC_DATASET_REFRESH_LOCK_KEY,
            "catalog:public_dataset_refresh_lock"
        );
    }

    #[test]
    fn lock_ttls_match_check_intervals() {
        assert_eq!(CATALOG_REFRESH_LOCK_TTL, REFRESH_INTERVAL_SECONDS);
        assert_eq!(TOKEN_CLEANUP_LOCK_TTL, REFRESH_INTERVAL_SECONDS);
    }

    #[test]
    fn public_dataset_refresh_interval_is_24_hours() {
        assert_eq!(PUBLIC_DATASET_REFRESH_INTERVAL_SECONDS, 86400);
    }

    #[test]
    fn public_dataset_refresh_initial_delay_is_5_minutes() {
        assert_eq!(PUBLIC_DATASET_REFRESH_INITIAL_DELAY_SECONDS, 300);
    }

    #[test]
    fn public_dataset_refresh_initial_delay_is_less_than_interval() {
        assert!(PUBLIC_DATASET_REFRESH_INITIAL_DELAY_SECONDS < PUBLIC_DATASET_REFRESH_INTERVAL_SECONDS);
    }

    #[test]
    fn refresh_threshold_is_24_hours() {
        assert_eq!(REFRESH_HOURS_THRESHOLD, 24);
    }

    #[test]
    fn get_indexing_credentials_returns_none_for_empty_config() {
        let config = serde_json::json!({});
        let (email, creds) = get_indexing_credentials(&config);
        assert!(email.is_none());
        assert!(creds.is_none());
    }

    #[test]
    fn get_indexing_credentials_rejects_oauth() {
        let config = serde_json::json!({
            "indexing_credentials": {
                "type": "oauth",
                "access_token": "test"
            }
        });
        let (email, creds) = get_indexing_credentials(&config);
        assert!(email.is_none());
        assert!(creds.is_none());
    }

    #[test]
    fn get_indexing_credentials_rejects_oauth_auth_type() {
        let config = serde_json::json!({
            "indexing_credentials": {
                "type": "bigquery",
                "auth_type": "oauth",
                "access_token": "test"
            }
        });
        let (email, creds) = get_indexing_credentials(&config);
        assert!(email.is_none());
        assert!(creds.is_none());
    }

    #[test]
    fn get_indexing_credentials_accepts_service_account() {
        let config = serde_json::json!({
            "indexing_credentials": {
                "type": "service_account",
                "project_id": "my-project",
                "client_email": "sa@project.iam.gserviceaccount.com"
            }
        });
        let (email, creds) = get_indexing_credentials(&config);
        assert!(email.is_none());
        assert!(creds.is_some());
        let creds = creds.expect("should have credentials");
        assert_eq!(creds["type"], "service_account");
    }

    #[test]
    fn get_indexing_credentials_rejects_missing_type() {
        let config = serde_json::json!({
            "indexing_credentials": {
                "access_token": "test"
            }
        });
        let (email, creds) = get_indexing_credentials(&config);
        assert!(email.is_none());
        assert!(creds.is_none());
    }

    #[test]
    fn get_indexing_credentials_rejects_non_object() {
        let config = serde_json::json!({
            "indexing_credentials": "not_an_object"
        });
        let (email, creds) = get_indexing_credentials(&config);
        assert!(email.is_none());
        assert!(creds.is_none());
    }

    // -- Additional get_indexing_credentials edge cases --

    #[test]
    fn get_indexing_credentials_rejects_null() {
        let config = serde_json::json!({
            "indexing_credentials": null
        });
        let (email, creds) = get_indexing_credentials(&config);
        assert!(email.is_none());
        assert!(creds.is_none());
    }

    #[test]
    fn get_indexing_credentials_rejects_array() {
        let config = serde_json::json!({
            "indexing_credentials": [1, 2, 3]
        });
        let (email, creds) = get_indexing_credentials(&config);
        assert!(email.is_none());
        assert!(creds.is_none());
    }

    #[test]
    fn get_indexing_credentials_rejects_empty_type() {
        let config = serde_json::json!({
            "indexing_credentials": {
                "type": "",
                "project_id": "my-project"
            }
        });
        let (email, creds) = get_indexing_credentials(&config);
        assert!(email.is_none());
        assert!(creds.is_none());
    }

    #[test]
    fn get_indexing_credentials_accepts_postgres_type() {
        let config = serde_json::json!({
            "indexing_credentials": {
                "type": "postgres",
                "host": "localhost",
                "port": 5432,
                "database": "mydb",
                "user": "admin",
                "password": "secret"
            }
        });
        let (email, creds) = get_indexing_credentials(&config);
        assert!(email.is_none());
        assert!(creds.is_some());
        let creds = creds.unwrap();
        assert_eq!(creds["type"], "postgres");
        assert_eq!(creds["host"], "localhost");
    }

    #[test]
    fn get_indexing_credentials_rejects_oauth_via_auth_type_only() {
        // Even if type is "bigquery", if auth_type is "oauth", reject it
        let config = serde_json::json!({
            "indexing_credentials": {
                "type": "bigquery",
                "auth_type": "oauth"
            }
        });
        let (_, creds) = get_indexing_credentials(&config);
        assert!(creds.is_none());
    }

    // -- RefreshResult enum exists and is used --

    #[test]
    fn refresh_result_variants_exist() {
        // Verify the RefreshResult enum has the expected variants
        let _refreshed = RefreshResult::Refreshed;
        let _skipped = RefreshResult::Skipped;
        let _failed = RefreshResult::Failed;
    }

    // -- Interval relationships --

    #[test]
    fn refresh_interval_is_one_hour() {
        assert_eq!(REFRESH_INTERVAL_SECONDS, 3600);
    }

    #[test]
    fn initial_delay_is_less_than_interval() {
        // Initial delay should be less than the refresh interval
        assert!(REFRESH_INITIAL_DELAY_SECONDS < REFRESH_INTERVAL_SECONDS);
    }

    #[test]
    fn cleanup_initial_delay_is_reasonable() {
        // Cleanup initial delay should be less than or equal to the cleanup interval
        assert!(CLEANUP_INITIAL_DELAY_SECONDS <= CLEANUP_INTERVAL_SECONDS);
        // And at least 1 hour (no rush for cleanup)
        assert!(CLEANUP_INITIAL_DELAY_SECONDS >= 3600);
    }

    #[test]
    fn lock_ttl_prevents_deadlock() {
        // Lock TTL should be at most equal to the check interval
        // so locks auto-expire even if the holder crashes
        assert!(CATALOG_REFRESH_LOCK_TTL <= REFRESH_INTERVAL_SECONDS);
        assert!(TOKEN_CLEANUP_LOCK_TTL <= CLEANUP_INTERVAL_SECONDS);
        assert!(QUERY_HISTORY_CLEANUP_LOCK_TTL <= QUERY_HISTORY_CLEANUP_INTERVAL_SECONDS);
    }

    // -- Query history cleanup constants --

    #[test]
    fn query_history_cleanup_interval_is_24_hours() {
        assert_eq!(QUERY_HISTORY_CLEANUP_INTERVAL_SECONDS, 86400);
    }

    #[test]
    fn query_history_cleanup_initial_delay_is_3_hours() {
        assert_eq!(QUERY_HISTORY_CLEANUP_INITIAL_DELAY_SECONDS, 10800);
    }

    #[test]
    fn query_history_cleanup_initial_delay_is_less_than_interval() {
        assert!(QUERY_HISTORY_CLEANUP_INITIAL_DELAY_SECONDS < QUERY_HISTORY_CLEANUP_INTERVAL_SECONDS);
    }

    #[test]
    fn default_retention_is_30_days() {
        assert_eq!(DEFAULT_QUERY_HISTORY_RETENTION_DAYS, 30);
    }
}
