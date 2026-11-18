// SPDX-License-Identifier: AGPL-3.0-or-later

//! Durable state for batch-mode watch runs (KYO-973).
//!
//! A batch-mode run can wait minutes to hours on a provider batch, so it must
//! survive a server restart and be safe with several instances. Two pieces of
//! state make that true:
//!
//! * **A lease** on the `watch_executions` row (`lease_owner`,
//!   `lease_expires_at`). The instance driving the run renews it; when it
//!   lapses any instance may claim the run with a compare-and-swap and resume
//!   it, and exactly one wins.
//! * **A per-call response cache**, `watch_batch_calls`, keyed by
//!   `(execution_id, call_index)`: the nth model call of the run. A resumed run
//!   replays from the start; its nth call is answered from the nth row (a
//!   finished response from the cache, a submitted-but-unfinished call by
//!   re-polling its vendor batch id), and only a call with no row is
//!   submitted. Keying by position rather than by request hash is what keeps a
//!   resume from resubmitting when replayed tool results, or context such as
//!   recent alerts, differ from the first attempt. Response bodies are
//!   encrypted at rest like chat messages.

use chrono::{DateTime, Utc};
use kyomi_core::models::ApiUsageInsert;
use kyomi_core::DbPool;

use crate::encryption;

/// Lifecycle of one batch call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchCallState {
    /// Row written before the vendor submit; a crash here leaves no vendor id.
    Submitting,
    /// Submitted; waiting on the vendor.
    Pending,
    Succeeded,
    Failed,
    Expired,
    /// The run stopped waiting (deadline / fallback); a late result is ignored.
    Abandoned,
}

impl BatchCallState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submitting => "submitting",
            Self::Pending => "pending",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Expired => "expired",
            Self::Abandoned => "abandoned",
        }
    }

    fn parse(s: &str) -> kyomi_core::Result<Self> {
        Ok(match s {
            "submitting" => Self::Submitting,
            "pending" => Self::Pending,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "expired" => Self::Expired,
            "abandoned" => Self::Abandoned,
            other => {
                return Err(kyomi_core::Error::Internal(format!(
                    "unknown watch batch call state {other:?}"
                )))
            }
        })
    }

    /// Whether the call has an outcome that never changes.
    pub fn is_final(self) -> bool {
        !matches!(self, Self::Submitting | Self::Pending)
    }
}

/// One LLM call of a batch-mode run.
#[derive(Debug, Clone)]
pub struct BatchCall {
    pub call_id: String,
    pub execution_id: i32,
    /// Position of the call within the run (0-based).
    pub call_index: i32,
    pub provider: String,
    pub model: String,
    pub vendor_batch_id: Option<String>,
    pub state: BatchCallState,
    /// The vendor's response body, decrypted; present when `Succeeded`.
    pub response: Option<serde_json::Value>,
    pub error_message: Option<String>,
}

#[derive(sqlx::FromRow)]
struct CallRow {
    call_id: String,
    execution_id: i32,
    call_index: i32,
    provider: String,
    model: String,
    vendor_batch_id: Option<String>,
    state: String,
    response_encrypted: Option<String>,
    error_message: Option<String>,
}

const CALL_COLUMNS: &str = "call_id, execution_id, call_index, provider, model, \
     vendor_batch_id, state, response_encrypted, error_message";

fn to_call(row: CallRow, key: &[u8; 32]) -> kyomi_core::Result<BatchCall> {
    Ok(BatchCall {
        call_id: row.call_id,
        execution_id: row.execution_id,
        call_index: row.call_index,
        provider: row.provider,
        model: row.model,
        vendor_batch_id: row.vendor_batch_id,
        state: BatchCallState::parse(&row.state)?,
        response: row
            .response_encrypted
            .as_deref()
            .map(|c| encryption::decrypt_json(c, key))
            .transpose()?,
        error_message: row.error_message,
    })
}

fn db_err(what: &str, e: impl std::fmt::Display) -> kyomi_core::Error {
    kyomi_core::Error::Internal(format!("watch batch {what}: {e}"))
}

/// The call recorded at position `call_index` of the run, if any.
pub async fn find_call(
    db: &DbPool,
    key: &[u8; 32],
    execution_id: i32,
    call_index: i32,
) -> kyomi_core::Result<Option<BatchCall>> {
    let sql = format!(
        "SELECT {CALL_COLUMNS} FROM watch_batch_calls \
         WHERE execution_id = $1 AND call_index = $2"
    );
    kyomi_core::db_fetch_optional!(db, CallRow, &sql, execution_id, call_index)
        .map_err(|e| db_err("lookup", e))?
        .map(|r| to_call(r, key))
        .transpose()
}

/// Record that the call at `call_index` is about to be submitted. Returns the
/// existing row when this run (before a crash) or another writer already
/// recorded that position, so a position is never recorded twice.
pub async fn begin_call(
    db: &DbPool,
    key: &[u8; 32],
    execution_id: i32,
    call_index: i32,
    provider: &str,
    model: &str,
) -> kyomi_core::Result<BatchCall> {
    let call_id = uuid::Uuid::new_v4().to_string();
    kyomi_core::db_execute!(
        db,
        "INSERT INTO watch_batch_calls \
            (call_id, execution_id, call_index, provider, model, state, submitted_at) \
         VALUES ($1, $2, $3, $4, $5, 'submitting', $6) \
         ON CONFLICT (execution_id, call_index) DO NOTHING",
        &call_id,
        execution_id,
        call_index,
        provider,
        model,
        Utc::now()
    )
    .map_err(|e| db_err("record call", e))?;
    find_call(db, key, execution_id, call_index)
        .await?
        .ok_or_else(|| db_err("record call", "row vanished after insert"))
}

/// SQL predicate: the run is not a batch-mode run. A batch run waits hours for its
/// provider, so the short-lived-run rules (stale expiry, orphan recovery, shutdown
/// cleanup) must not time it out; it has its own deadline and reaper.
pub const NOT_A_BATCH_RUN: &str = "COALESCE(llm_mode, 'realtime') <> 'batch'";

/// Who a run's usage is billed to.
pub struct UsageBill<'a> {
    /// Decrypts the stored vendor responses.
    pub encryption_key: &'a [u8; 32],
    pub user_id: &'a str,
    pub workspace_id: &'a str,
    pub session_id: &'a str,
}

/// A vendor response stored on a call row (a batch result, or the realtime
/// answer that replaced a failed batch), with the provider and model it was
/// requested from.
pub struct StoredResponse {
    pub provider: String,
    pub model: String,
    pub response: serde_json::Value,
}

/// Who may bill a run.
#[derive(Clone, Copy)]
pub enum BillingFence<'a> {
    /// The instance holding the run's live lease (a run in progress or just ending).
    LeaseOwner(&'a str),
    /// Anyone, for a run that has already been stopped (completed) by the reaper
    /// and so has no driver: bills what was paid before the stop.
    StoppedRun,
}

/// Bill a batch-mode run's usage exactly once: claim the billing, read the
/// run's stored responses and write the `api_usage_log` row **in one
/// transaction**. A crash or abort at any point rolls the whole thing back (the
/// run can still be billed later) and a committed bill can never be repeated, so
/// usage is neither lost to a half-done claim nor billed twice. Only the
/// instance that holds the run's live lease can bill, so a displaced driver
/// never bills a partial run.
///
/// This function owns the claim and the write, not the price: `build_row` turns
/// the stored responses (one per call of the run, whichever attempt or instance
/// made it) into the row, and the caller makes it through the same usage ledger
/// every other LLM call is billed with, so pricing lives in one place. It
/// returns `None` when no call used any tokens. Returns whether this call billed.
pub async fn bill_run_once(
    db: &DbPool,
    execution_id: i32,
    fence: BillingFence<'_>,
    now: DateTime<Utc>,
    bill: &UsageBill<'_>,
    build_row: impl Fn(&[StoredResponse]) -> kyomi_core::Result<Option<ApiUsageInsert>>,
) -> kyomi_core::Result<bool> {
    let insert_sql = ApiUsageInsert::insert_sql(matches!(db, DbPool::Postgres(_)));
    kyomi_core::db_with_pool!(db, |p| {
        let mut tx = p.begin().await.map_err(|e| db_err("begin billing", e))?;
        let claimed = match fence {
            BillingFence::LeaseOwner(owner) => sqlx::query(
                "UPDATE watch_executions SET usage_billed_at = $1 \
                 WHERE id = $2 AND usage_billed_at IS NULL AND lease_owner = $3 \
                   AND lease_expires_at > $1 AND llm_mode = 'batch'",
            )
            .bind(now)
            .bind(execution_id)
            .bind(owner),
            BillingFence::StoppedRun => sqlx::query(
                "UPDATE watch_executions SET usage_billed_at = $1 \
                 WHERE id = $2 AND usage_billed_at IS NULL \
                   AND completed_at IS NOT NULL AND llm_mode = 'batch'",
            )
            .bind(now)
            .bind(execution_id),
        }
        .execute(&mut *tx)
        .await
        .map_err(|e| db_err("claim usage billing", e))?
        .rows_affected()
            == 1;
        if !claimed {
            return Ok(false);
        }
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT provider, model, response_encrypted FROM watch_batch_calls \
             WHERE execution_id = $1 AND response_encrypted IS NOT NULL ORDER BY call_index",
        )
        .bind(execution_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| db_err("read run responses", e))?;
        let calls = rows
            .into_iter()
            .map(|(provider, model, encrypted)| {
                Ok(StoredResponse {
                    provider,
                    model,
                    response: encryption::decrypt_json(&encrypted, bill.encryption_key)?,
                })
            })
            .collect::<kyomi_core::Result<Vec<_>>>()?;
        if let Some(usage) = build_row(&calls)? {
            sqlx::query(&insert_sql)
                .bind(bill.user_id)
                .bind(bill.workspace_id)
                .bind(bill.session_id)
                .bind(now)
                .bind(&usage.provider)
                .bind(&usage.model)
                .bind(usage.input_tokens)
                .bind(usage.output_tokens)
                .bind(usage.total_tokens)
                .bind(usage.cost_estimate)
                .bind(&usage.component)
                .bind(usage.provider_cost_usd)
                .bind(usage.cache_creation_input_tokens)
                .bind(usage.cache_read_input_tokens)
                .bind(&usage.extra_metadata)
                .bind(&usage.request_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| db_err("write usage", e))?;
        }
        tx.commit().await.map_err(|e| db_err("commit billing", e))?;
        Ok(true)
    })
}

/// Keep the realtime response that replaced a failed or abandoned batch, so a
/// resumed run is answered from it instead of calling the model again.
pub async fn attach_fallback_response(
    db: &DbPool,
    key: &[u8; 32],
    call_id: &str,
    response: &serde_json::Value,
) -> kyomi_core::Result<bool> {
    let encrypted = encryption::encrypt_json(response, key)?;
    let res = kyomi_core::db_execute!(
        db,
        "UPDATE watch_batch_calls \
         SET response_encrypted = $1 \
         WHERE call_id = $2 AND state IN ('failed', 'expired', 'abandoned') \
           AND response_encrypted IS NULL",
        encrypted,
        call_id
    )
    .map_err(|e| db_err("attach fallback response", e))?;
    Ok(res.rows_affected() == 1)
}

/// Attach the vendor batch id once the submit succeeded.
pub async fn mark_submitted(
    db: &DbPool,
    call_id: &str,
    vendor_batch_id: &str,
) -> kyomi_core::Result<()> {
    kyomi_core::db_execute!(
        db,
        "UPDATE watch_batch_calls SET vendor_batch_id = $1, state = 'pending' \
         WHERE call_id = $2 AND state = 'submitting'",
        vendor_batch_id,
        call_id
    )
    .map_err(|e| db_err("mark submitted", e))?;
    Ok(())
}

/// Finish a call, recording its response (what the run is billed from) in the
/// same write as its state. Only a non-final call is changed, so a late result can
/// never overwrite a run's earlier decision (`Abandoned`) or a recorded response.
pub async fn finish_call(
    db: &DbPool,
    key: &[u8; 32],
    call_id: &str,
    state: BatchCallState,
    response: Option<&serde_json::Value>,
    error_message: Option<&str>,
) -> kyomi_core::Result<bool> {
    debug_assert!(state.is_final());
    let encrypted = response
        .map(|r| encryption::encrypt_json(r, key))
        .transpose()?;
    let res = kyomi_core::db_execute!(
        db,
        "UPDATE watch_batch_calls \
         SET state = $1, response_encrypted = $2, error_message = $3, completed_at = $4 \
         WHERE call_id = $5 AND state IN ('submitting', 'pending')",
        state.as_str(),
        encrypted,
        error_message,
        Utc::now(),
        call_id
    )
    .map_err(|e| db_err("finish call", e))?;
    Ok(res.rows_affected() == 1)
}

#[derive(sqlx::FromRow)]
struct BatchIdRow {
    vendor_batch_id: String,
}

/// Vendor batch ids submitted by a run, oldest first.
pub async fn batch_ids(db: &DbPool, execution_id: i32) -> kyomi_core::Result<Vec<String>> {
    let rows = kyomi_core::db_fetch_all!(
        db,
        BatchIdRow,
        "SELECT vendor_batch_id FROM watch_batch_calls \
         WHERE execution_id = $1 AND vendor_batch_id IS NOT NULL \
         ORDER BY submitted_at, call_id",
        execution_id
    )
    .map_err(|e| db_err("list batch ids", e))?;
    Ok(rows.into_iter().map(|r| r.vendor_batch_id).collect())
}

// ---------------------------------------------------------------------------
// Execution record: LLM facts, status, lease
// ---------------------------------------------------------------------------

/// The LLM facts recorded on an execution.
#[derive(Debug, Clone)]
pub struct RunLlm<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    /// `"realtime"` or `"batch"`.
    pub mode: &'a str,
    pub deadline: Option<DateTime<Utc>>,
    pub fallback_reason: Option<&'a str>,
    /// `(owner, expires_at)`: taken for a batch run so it is resumable from
    /// its first instant.
    pub lease: Option<(&'a str, DateTime<Utc>)>,
    /// Whether usage is billed to the workspace's own key (not Kyomi credits);
    /// kept so a run stopped by the reaper can still be billed correctly.
    pub workspace_billed: bool,
}

/// Record which provider/model/mode a run uses.
pub async fn record_run_llm(
    db: &DbPool,
    execution_id: i32,
    run: &RunLlm<'_>,
) -> kyomi_core::Result<()> {
    kyomi_core::db_execute!(
        db,
        "UPDATE watch_executions SET llm_provider = $1, llm_model = $2, llm_mode = $3, \
             batch_deadline_at = $4, batch_fallback_reason = $5, \
             lease_owner = $6, lease_expires_at = $7, llm_workspace_billed = $8 \
         WHERE id = $9",
        run.provider,
        run.model,
        run.mode,
        run.deadline,
        run.fallback_reason,
        run.lease.map(|l| l.0),
        run.lease.map(|l| l.1),
        run.workspace_billed,
        execution_id
    )
    .map_err(|e| db_err("record run llm", e))?;
    Ok(())
}

/// Write the vendor batch ids into the execution record.
pub async fn sync_batch_ids(db: &DbPool, execution_id: i32) -> kyomi_core::Result<()> {
    let ids = serde_json::to_string(&batch_ids(db, execution_id).await?)
        .map_err(|e| db_err("serialize batch ids", e))?;
    kyomi_core::db_execute!(
        db,
        "UPDATE watch_executions SET batch_ids = $1 WHERE id = $2",
        &ids,
        execution_id
    )
    .map_err(|e| db_err("sync batch ids", e))?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    session_id: Option<String>,
}

/// The chat session recorded on an execution, if any.
pub async fn execution_session(db: &DbPool, execution_id: i32) -> kyomi_core::Result<Option<String>> {
    Ok(kyomi_core::db_fetch_optional!(
        db,
        SessionRow,
        "SELECT session_id FROM watch_executions WHERE id = $1",
        execution_id
    )
    .map_err(|e| db_err("load session", e))?
    .and_then(|r| r.session_id))
}

/// Record the chat session an execution's current attempt runs in.
pub async fn set_execution_session(
    db: &DbPool,
    execution_id: i32,
    session_id: &str,
) -> kyomi_core::Result<()> {
    kyomi_core::db_execute!(
        db,
        "UPDATE watch_executions SET session_id = $1 WHERE id = $2",
        session_id,
        execution_id
    )
    .map_err(|e| db_err("set session", e))?;
    Ok(())
}

/// Note that part of a run ran realtime, keeping the first reason.
pub async fn note_fallback(
    db: &DbPool,
    execution_id: i32,
    reason: &str,
) -> kyomi_core::Result<()> {
    kyomi_core::db_execute!(
        db,
        "UPDATE watch_executions SET batch_fallback_reason = COALESCE(batch_fallback_reason, $1) \
         WHERE id = $2",
        reason,
        execution_id
    )
    .map_err(|e| db_err("note fallback", e))?;
    Ok(())
}

/// Flip a run between `running` and `waiting_batch`. Only the named source
/// status is changed, so a terminal status is never overwritten.
pub async fn set_waiting(db: &DbPool, execution_id: i32, waiting: bool) -> kyomi_core::Result<()> {
    let (from, to) = if waiting {
        ("running", "waiting_batch")
    } else {
        ("waiting_batch", "running")
    };
    kyomi_core::db_execute!(
        db,
        "UPDATE watch_executions SET status = $1 WHERE id = $2 AND status = $3",
        to,
        execution_id,
        from
    )
    .map_err(|e| db_err("set waiting", e))?;
    Ok(())
}

/// Claim (or renew, when already the owner) the run's lease. Exactly one
/// caller wins a lapsed lease. Returns whether this caller now holds it.
pub async fn claim_lease(
    db: &DbPool,
    execution_id: i32,
    owner: &str,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> kyomi_core::Result<bool> {
    let res = kyomi_core::db_execute!(
        db,
        "UPDATE watch_executions SET lease_owner = $1, lease_expires_at = $2 \
         WHERE id = $3 AND llm_mode = 'batch' AND completed_at IS NULL \
           AND (lease_owner IS NULL OR lease_owner = $1 OR lease_expires_at IS NULL \
                OR lease_expires_at < $4)",
        owner,
        expires_at,
        execution_id,
        now
    )
    .map_err(|e| db_err("claim lease", e))?;
    Ok(res.rows_affected() == 1)
}

/// Take over a run whose lease has lapsed or was released. Unlike
/// [`claim_lease`] there is no "already mine" shortcut: only a genuinely
/// lapsed lease can be taken, and exactly one caller wins it.
pub async fn take_over_lease(
    db: &DbPool,
    execution_id: i32,
    owner: &str,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> kyomi_core::Result<bool> {
    let res = kyomi_core::db_execute!(
        db,
        "UPDATE watch_executions SET lease_owner = $1, lease_expires_at = $2 \
         WHERE id = $3 AND llm_mode = 'batch' AND completed_at IS NULL \
           AND (lease_expires_at IS NULL OR lease_expires_at < $4)",
        owner,
        expires_at,
        execution_id,
        now
    )
    .map_err(|e| db_err("take over lease", e))?;
    Ok(res.rows_affected() == 1)
}

/// Give the lease up (graceful shutdown) so another instance resumes at once.
pub async fn release_lease(
    db: &DbPool,
    execution_id: i32,
    owner: &str,
    now: DateTime<Utc>,
) -> kyomi_core::Result<()> {
    kyomi_core::db_execute!(
        db,
        "UPDATE watch_executions SET lease_expires_at = $1 \
         WHERE id = $2 AND lease_owner = $3 AND completed_at IS NULL",
        now,
        execution_id,
        owner
    )
    .map_err(|e| db_err("release lease", e))?;
    Ok(())
}

/// Release every lease this instance holds (graceful shutdown), so other
/// instances resume the runs immediately instead of after the lease lapses.
pub async fn release_all_leases(
    db: &DbPool,
    owner: &str,
    now: DateTime<Utc>,
) -> kyomi_core::Result<u64> {
    let res = kyomi_core::db_execute!(
        db,
        "UPDATE watch_executions SET lease_expires_at = $1 \
         WHERE lease_owner = $2 AND llm_mode = 'batch' AND completed_at IS NULL",
        now,
        owner
    )
    .map_err(|e| db_err("release all leases", e))?;
    Ok(res.rows_affected())
}

#[derive(sqlx::FromRow)]
struct CountRow {
    n: i64,
}

/// Whether the watch has a batch-mode run that has not finished. The
/// scheduler must not start the next occurrence while one is outstanding.
pub async fn has_unfinished_batch_run(db: &DbPool, watch_id: &str) -> kyomi_core::Result<bool> {
    let row = kyomi_core::db_fetch_one!(
        db,
        CountRow,
        "SELECT COUNT(*) AS n FROM watch_executions \
         WHERE watch_id = $1 AND llm_mode = 'batch' AND completed_at IS NULL \
           AND status IN ('running', 'waiting_batch')",
        watch_id
    )
    .map_err(|e| db_err("count unfinished runs", e))?;
    Ok(row.n > 0)
}

/// What a resumed run needs from its execution record.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RunInfo {
    pub id: i32,
    pub watch_id: Option<String>,
    pub started_at: DateTime<Utc>,
    pub batch_deadline_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct ProviderRow {
    llm_provider: Option<String>,
}

/// The provider a run recorded when it started.
pub async fn run_provider(db: &DbPool, execution_id: i32) -> kyomi_core::Result<Option<String>> {
    Ok(kyomi_core::db_fetch_optional!(
        db,
        ProviderRow,
        "SELECT llm_provider FROM watch_executions WHERE id = $1",
        execution_id
    )
    .map_err(|e| db_err("load run provider", e))?
    .and_then(|r| r.llm_provider))
}

/// A batch run the reaper stopped, with what is needed to bill its usage.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReapedRun {
    pub id: i32,
    pub watch_id: Option<String>,
    pub created_by: Option<String>,
    pub workspace_id: Option<String>,
    pub session_id: Option<String>,
    pub llm_provider: Option<String>,
    pub llm_model: Option<String>,
    pub llm_workspace_billed: bool,
}

/// Fail batch-mode runs that are far past their deadline and still unfinished:
/// a driver that is alive (its heartbeat keeps the lease) but wedged, or runs
/// no instance can resume. Without this a hung run would block the watch's
/// future occurrences and manual runs forever. Returns the runs it stopped, so
/// the caller can bill what they paid for and stop a local driver.
pub async fn reap_overdue_runs(
    db: &DbPool,
    now: DateTime<Utc>,
    grace: chrono::Duration,
) -> kyomi_core::Result<Vec<ReapedRun>> {
    let cutoff = now - grace;
    let overdue = kyomi_core::db_fetch_all!(
        db,
        ReapedRun,
        "SELECT id, watch_id, created_by, workspace_id, session_id, llm_provider, llm_model, \
                llm_workspace_billed \
         FROM watch_executions \
         WHERE llm_mode = 'batch' AND completed_at IS NULL \
           AND status IN ('running', 'waiting_batch') \
           AND batch_deadline_at IS NOT NULL AND batch_deadline_at < $1",
        cutoff
    )
    .map_err(|e| db_err("list overdue runs", e))?;
    let is_pg = db.is_postgres();
    let sql = format!(
        "UPDATE watch_executions \
         SET status = 'error', completed_at = {now_fn}, \
             error_message = 'Batch run exceeded its deadline and was stopped' \
         WHERE id = $1 AND completed_at IS NULL",
        now_fn = kyomi_core::sql_compat::now(is_pg)
    );
    let mut stopped = Vec::new();
    for run in overdue {
        // Per run, so a driver that finished in between is not overwritten.
        let res = kyomi_core::db_execute!(db, &sql, run.id).map_err(|e| db_err("reap overdue run", e))?;
        if res.rows_affected() == 1 {
            stopped.push(run);
        }
    }
    Ok(stopped)
}

/// Load the record of an unfinished batch-mode run (`None` once it finished).
pub async fn unfinished_run(db: &DbPool, execution_id: i32) -> kyomi_core::Result<Option<RunInfo>> {
    kyomi_core::db_fetch_optional!(
        db,
        RunInfo,
        "SELECT id, watch_id, started_at, batch_deadline_at FROM watch_executions \
         WHERE id = $1 AND llm_mode = 'batch' AND completed_at IS NULL",
        execution_id
    )
    .map_err(|e| db_err("load run", e))
}

/// Watch ids of this instance's unfinished batch-mode runs.
pub async fn my_batch_watch_ids(db: &DbPool, owner: &str) -> kyomi_core::Result<Vec<String>> {
    #[derive(sqlx::FromRow)]
    struct W {
        watch_id: Option<String>,
    }
    let rows = kyomi_core::db_fetch_all!(
        db,
        W,
        "SELECT watch_id FROM watch_executions \
         WHERE lease_owner = $1 AND llm_mode = 'batch' AND completed_at IS NULL",
        owner
    )
    .map_err(|e| db_err("list own batch runs", e))?;
    Ok(rows.into_iter().filter_map(|r| r.watch_id).collect())
}

/// An unfinished batch-mode run nobody is driving.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ResumableRun {
    pub id: i32,
    pub watch_id: Option<String>,
}

/// Unfinished batch-mode runs whose lease lapsed, oldest first.
pub async fn resumable_runs(
    db: &DbPool,
    now: DateTime<Utc>,
    limit: i64,
) -> kyomi_core::Result<Vec<ResumableRun>> {
    kyomi_core::db_fetch_all!(
        db,
        ResumableRun,
        "SELECT id, watch_id FROM watch_executions \
         WHERE llm_mode = 'batch' AND completed_at IS NULL \
           AND status IN ('running', 'waiting_batch') \
           AND (lease_expires_at IS NULL OR lease_expires_at < $1) \
         ORDER BY started_at LIMIT $2",
        now,
        limit
    )
    .map_err(|e| db_err("list resumable runs", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use kyomi_test_db::{seed_user, seed_workspace, sqlite_pool, test_pool};

    const KEY: [u8; 32] = [3u8; 32];

    async fn execution(db: &DbPool) -> i32 {
        let sq = sqlite_pool(db);
        seed_user(sq, "u1", "u1@test.local").await;
        seed_workspace(sq, "ws1", "u1").await;
        sqlx::query(
            "INSERT INTO watches (watch_id, workspace_id, created_by, name, prompt, schedule, enabled) \
             VALUES ('w1', 'ws1', 'u1', 'W', 'p', '0 9 * * *', 1)",
        )
        .execute(sq)
        .await
        .unwrap();
        crate::watch_service::create_execution(db, "w1", "W", "ws1", kyomi_core::WatchMode::Alert, "u1")
            .await
            .unwrap()
            .id
    }

    /// A further batch run of the already-seeded watch `w1`.
    async fn another_batch_run(db: &DbPool, owner: &str, lease_for: Duration) -> i32 {
        let e = crate::watch_service::create_execution(db, "w1", "W", "ws1", kyomi_core::WatchMode::Alert, "u1").await.unwrap();
        let now = Utc::now();
        record_run_llm(db, e.id, &RunLlm { provider: "anthropic", model: "m", mode: "batch", deadline: Some(now + Duration::hours(3)), fallback_reason: None, lease: Some((owner, now + lease_for)), workspace_billed: false }).await.unwrap();
        e.id
    }

    async fn batch_run(db: &DbPool, owner: &str, lease_for: Duration) -> i32 {
        let id = execution(db).await;
        let now = Utc::now();
        record_run_llm(db, id, &RunLlm { provider: "anthropic", model: "m", mode: "batch", deadline: Some(now + Duration::hours(3)), fallback_reason: None, lease: Some((owner, now + lease_for)), workspace_billed: false })
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn begin_call_is_idempotent_per_position_and_distinct_across_positions() {
        let db = test_pool().await;
        let id = execution(&db).await;
        let a = begin_call(&db, &KEY, id, 0, "anthropic", "m").await.unwrap();
        // The same position with a *different* request (replayed tool results
        // changed) still resolves to the same call: that is the point.
        let again = begin_call(&db, &KEY, id, 0, "anthropic", "m").await.unwrap();
        assert_eq!(a.call_id, again.call_id, "a position must not be recorded twice");
        let b = begin_call(&db, &KEY, id, 1, "anthropic", "m").await.unwrap();
        assert_ne!(a.call_id, b.call_id);
        assert_eq!((a.state, b.call_index), (BatchCallState::Submitting, 1));
    }

    #[tokio::test]
    async fn a_finished_call_round_trips_its_response_encrypted_and_is_never_overwritten() {
        let db = test_pool().await;
        let id = execution(&db).await;
        let c = begin_call(&db, &KEY, id, 0, "anthropic", "m").await.unwrap();
        mark_submitted(&db, &c.call_id, "msgbatch_1").await.unwrap();
        let body = serde_json::json!({"content": [{"type": "text", "text": "secret-finding"}]});
        assert!(finish_call(&db, &KEY, &c.call_id, BatchCallState::Succeeded, Some(&body), None).await.unwrap());

        let got = find_call(&db, &KEY, id, 0).await.unwrap().unwrap();
        assert_eq!(got.state, BatchCallState::Succeeded);
        assert_eq!(got.vendor_batch_id.as_deref(), Some("msgbatch_1"));
        assert_eq!(got.response, Some(body));

        let raw: (Option<String>,) = sqlx::query_as("SELECT response_encrypted FROM watch_batch_calls WHERE call_id = $1")
            .bind(&c.call_id)
            .fetch_one(sqlite_pool(&db))
            .await
            .unwrap();
        assert!(!raw.0.unwrap().contains("secret-finding"), "responses must be encrypted at rest");

        // A late failure / abandon cannot overwrite the recorded response.
        assert!(!finish_call(&db, &KEY, &c.call_id, BatchCallState::Abandoned, None, Some("late")).await.unwrap());
        assert_eq!(find_call(&db, &KEY, id, 0).await.unwrap().unwrap().state, BatchCallState::Succeeded);
    }

    #[tokio::test]
    async fn a_fallback_response_is_kept_once_for_an_abandoned_call_and_never_for_a_succeeded_one() {
        let db = test_pool().await;
        let id = execution(&db).await;
        let abandoned = begin_call(&db, &KEY, id, 0, "anthropic", "m").await.unwrap();
        finish_call(&db, &KEY, &abandoned.call_id, BatchCallState::Abandoned, None, Some("deadline")).await.unwrap();
        let first = serde_json::json!({"content": "realtime one"});
        attach_fallback_response(&db, &KEY, &abandoned.call_id, &first).await.unwrap();
        assert!(!attach_fallback_response(&db, &KEY, &abandoned.call_id, &serde_json::json!({"content": "second"})).await.unwrap(), "a second answer for the same call is not attached");
        let got = find_call(&db, &KEY, id, 0).await.unwrap().unwrap();
        assert_eq!((got.state, got.response), (BatchCallState::Abandoned, Some(first)), "the first fallback response wins");

        let ok = begin_call(&db, &KEY, id, 1, "anthropic", "m").await.unwrap();
        finish_call(&db, &KEY, &ok.call_id, BatchCallState::Succeeded, Some(&serde_json::json!({"a": 1})), None).await.unwrap();
        attach_fallback_response(&db, &KEY, &ok.call_id, &serde_json::json!({"a": 2})).await.unwrap();
        assert_eq!(find_call(&db, &KEY, id, 1).await.unwrap().unwrap().response, Some(serde_json::json!({"a": 1})));
    }

    #[tokio::test]
    async fn batch_ids_are_listed_in_submission_order_and_synced_to_the_execution() {
        let db = test_pool().await;
        let id = execution(&db).await;
        for (i, b) in [(0, "b1"), (1, "b2")] {
            let c = begin_call(&db, &KEY, id, i, "anthropic", "m").await.unwrap();
            mark_submitted(&db, &c.call_id, b).await.unwrap();
        }
        let _unsubmitted = begin_call(&db, &KEY, id, 2, "anthropic", "m").await.unwrap();
        assert_eq!(batch_ids(&db, id).await.unwrap(), vec!["b1", "b2"]);
        sync_batch_ids(&db, id).await.unwrap();
        let e = crate::watch_service::get_execution_by_id_only(&db, id, "ws1", "u1").await.unwrap().unwrap();
        assert_eq!(e.batch_ids.as_deref(), Some(r#"["b1","b2"]"#));
    }

    #[tokio::test]
    async fn exactly_one_of_many_concurrent_claimers_wins_a_lapsed_lease() {
        let db = test_pool().await;
        let id = batch_run(&db, "dead-instance", Duration::seconds(-5)).await;
        let mut tasks = Vec::new();
        for n in 0..8 {
            let db = db.clone();
            tasks.push(tokio::spawn(async move {
                let now = Utc::now();
                claim_lease(&db, id, &format!("instance-{n}"), now + Duration::seconds(90), now).await.unwrap()
            }));
        }
        let mut winners = 0;
        for t in tasks {
            if t.await.unwrap() {
                winners += 1;
            }
        }
        assert_eq!(winners, 1, "a lapsed lease must be claimed by exactly one instance");
    }

    #[tokio::test]
    async fn a_live_lease_cannot_be_claimed_by_another_instance_but_its_owner_can_renew() {
        let db = test_pool().await;
        let id = batch_run(&db, "a", Duration::seconds(60)).await;
        let now = Utc::now();
        assert!(!claim_lease(&db, id, "b", now + Duration::seconds(90), now).await.unwrap());
        assert!(claim_lease(&db, id, "a", now + Duration::seconds(90), now).await.unwrap(), "the owner renews");
        assert!(resumable_runs(&db, now, 10).await.unwrap().is_empty(), "a live lease is not resumable");
    }

    #[tokio::test]
    async fn a_lapsed_or_released_lease_makes_the_run_resumable_once_and_finished_runs_never() {
        let db = test_pool().await;
        let id = batch_run(&db, "a", Duration::seconds(60)).await;
        let now = Utc::now();
        release_lease(&db, id, "a", now - Duration::seconds(1)).await.unwrap();
        let runs = resumable_runs(&db, now, 10).await.unwrap();
        assert_eq!(runs.iter().map(|r| r.id).collect::<Vec<_>>(), vec![id]);
        assert_eq!(runs[0].watch_id.as_deref(), Some("w1"));

        // Another instance's release does nothing.
        let id2 = {
            // Reuse the same watch for a second execution.
            let e = crate::watch_service::create_execution(&db, "w1", "W", "ws1", kyomi_core::WatchMode::Alert, "u1").await.unwrap();
            record_run_llm(&db, e.id, &RunLlm { provider: "anthropic", model: "m", mode: "batch", deadline: None, fallback_reason: None, lease: Some(("a", now + Duration::seconds(60))), workspace_billed: false }).await.unwrap();
            e.id
        };
        release_lease(&db, id2, "someone-else", now - Duration::seconds(1)).await.unwrap();
        assert_eq!(resumable_runs(&db, now, 10).await.unwrap().len(), 1);

        crate::watch_service::fail_execution_minimal(&db, id, "done").await.unwrap();
        assert!(resumable_runs(&db, now, 10).await.unwrap().iter().all(|r| r.id != id), "a finished run is never resumed");
        assert!(!claim_lease(&db, id, "b", now + Duration::seconds(90), now).await.unwrap(), "nor claimed");
    }

    #[tokio::test]
    async fn realtime_runs_are_never_resumable() {
        let db = test_pool().await;
        let id = execution(&db).await;
        record_run_llm(&db, id, &RunLlm { provider: "anthropic", model: "m", mode: "realtime", deadline: None, fallback_reason: Some("schedule gap 30m is shorter than the 60m batch window"), lease: None, workspace_billed: false }).await.unwrap();
        assert!(resumable_runs(&db, Utc::now(), 10).await.unwrap().is_empty());
        let e = crate::watch_service::get_execution_by_id_only(&db, id, "ws1", "u1").await.unwrap().unwrap();
        assert_eq!(e.llm_mode.as_deref(), Some("realtime"));
        assert!(e.batch_fallback_reason.unwrap().contains("schedule gap"));
    }

    #[tokio::test]
    async fn waiting_flips_only_between_running_and_waiting_and_never_touches_a_terminal_status() {
        let db = test_pool().await;
        let id = execution(&db).await;
        let status = |db: DbPool| async move {
            crate::watch_service::get_execution_by_id_only(&db, id, "ws1", "u1").await.unwrap().unwrap().status
        };
        set_waiting(&db, id, true).await.unwrap();
        assert_eq!(status(db.clone()).await, kyomi_core::WatchExecutionStatus::WaitingBatch);
        set_waiting(&db, id, false).await.unwrap();
        assert_eq!(status(db.clone()).await, kyomi_core::WatchExecutionStatus::Running);
        crate::watch_service::fail_execution_minimal(&db, id, "boom").await.unwrap();
        set_waiting(&db, id, true).await.unwrap();
        assert_eq!(status(db.clone()).await, kyomi_core::WatchExecutionStatus::Error, "a terminal status is not resurrected");
    }

    #[tokio::test]
    async fn note_fallback_keeps_the_first_reason() {
        let db = test_pool().await;
        let id = execution(&db).await;
        note_fallback(&db, id, "first").await.unwrap();
        note_fallback(&db, id, "second").await.unwrap();
        let e = crate::watch_service::get_execution_by_id_only(&db, id, "ws1", "u1").await.unwrap().unwrap();
        assert_eq!(e.batch_fallback_reason.as_deref(), Some("first"));
    }

    #[tokio::test]
    async fn calls_are_removed_with_their_execution() {
        let db = test_pool().await;
        let id = execution(&db).await;
        begin_call(&db, &KEY, id, 0, "anthropic", "m").await.unwrap();
        sqlx::query("PRAGMA foreign_keys = ON").execute(sqlite_pool(&db)).await.unwrap();
        sqlx::query("DELETE FROM watch_executions WHERE id = $1").bind(id).execute(sqlite_pool(&db)).await.unwrap();
        assert!(find_call(&db, &KEY, id, 0).await.unwrap().is_none());
    }

    // -- "Run now" must neither kill nor run beside a batch run ----------------

    async fn backdate(db: &DbPool, id: i32, minutes: i64) {
        sqlx::query("UPDATE watch_executions SET started_at = $1 WHERE id = $2")
            .bind(Utc::now() - Duration::minutes(minutes))
            .bind(id)
            .execute(sqlite_pool(db))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn run_now_does_not_expire_a_long_running_batch_run_and_is_refused_while_it_runs() {
        let db = test_pool().await;
        let id = batch_run(&db, "me", Duration::seconds(120)).await;
        backdate(&db, id, 180).await; // hours old, as a batch run legitimately is

        let (ok, why) = crate::watch_service::can_run_watch_now(&db, "w1", "ws1", "u1").await.unwrap();
        assert!(!ok, "a live batch run blocks a manual run");
        assert_eq!(why, "Watch is already running");
        let e = crate::watch_service::get_execution_by_id_only(&db, id, "ws1", "u1").await.unwrap().unwrap();
        assert_eq!(e.status, kyomi_core::WatchExecutionStatus::Running, "the 5-minute expiry must not kill a batch run");
        assert!(e.completed_at.is_none());

        set_waiting(&db, id, true).await.unwrap();
        let (ok, _) = crate::watch_service::can_run_watch_now(&db, "w1", "ws1", "u1").await.unwrap();
        assert!(!ok, "a run waiting on a batch also blocks a manual run");
    }

    #[tokio::test]
    async fn run_now_still_expires_a_stuck_realtime_run() {
        let db = test_pool().await;
        let id = execution(&db).await;
        backdate(&db, id, 10).await;
        let (ok, _) = crate::watch_service::can_run_watch_now(&db, "w1", "ws1", "u1").await.unwrap();
        assert!(ok, "the existing stale-realtime recovery is unchanged");
        let e = crate::watch_service::get_execution_by_id_only(&db, id, "ws1", "u1").await.unwrap().unwrap();
        assert_eq!(e.status, kyomi_core::WatchExecutionStatus::Error);
    }

    // -- Hung-run reaping and run facts ----------------------------------------

    async fn set_deadline(db: &DbPool, id: i32, ago: Duration) {
        sqlx::query("UPDATE watch_executions SET batch_deadline_at = $1 WHERE id = $2")
            .bind(Utc::now() - ago)
            .bind(id)
            .execute(sqlite_pool(db))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_batch_run_far_past_its_deadline_is_reaped_even_with_a_live_lease_and_others_are_not() {
        let db = test_pool().await;
        let id = batch_run(&db, "wedged-but-heartbeating", Duration::seconds(120)).await;
        let grace = Duration::minutes(40);

        // Within the grace window: left alone.
        set_deadline(&db, id, Duration::minutes(10)).await;
        assert!(reap_overdue_runs(&db, Utc::now(), grace).await.unwrap().is_empty());

        // Far past: stopped, despite its lease being renewed.
        set_deadline(&db, id, Duration::minutes(90)).await;
        let reaped = reap_overdue_runs(&db, Utc::now(), grace).await.unwrap();
        assert_eq!(reaped.iter().map(|r| (r.id, r.watch_id.clone())).collect::<Vec<_>>(), vec![(id, Some("w1".to_string()))]);
        let e = crate::watch_service::get_execution_by_id_only(&db, id, "ws1", "u1").await.unwrap().unwrap();
        assert_eq!(e.status, kyomi_core::WatchExecutionStatus::Error);
        assert!(e.completed_at.is_some());
        assert!(e.error_message.unwrap().contains("exceeded its deadline"));
        // The wedged driver is displaced: its next lease renewal fails.
        let now = Utc::now();
        assert!(!claim_lease(&db, id, "wedged-but-heartbeating", now + Duration::seconds(90), now).await.unwrap());
        // And the watch can run again.
        let (ok, _) = crate::watch_service::can_run_watch_now(&db, "w1", "ws1", "u1").await.unwrap();
        assert!(ok);
    }

    #[tokio::test]
    async fn realtime_runs_are_never_reaped_by_the_batch_deadline_rule() {
        let db = test_pool().await;
        let id = execution(&db).await;
        record_run_llm(&db, id, &RunLlm { provider: "anthropic", model: "m", mode: "realtime", deadline: Some(Utc::now() - Duration::days(3)), fallback_reason: None, lease: None, workspace_billed: false }).await.unwrap();
        assert!(reap_overdue_runs(&db, Utc::now(), Duration::minutes(40)).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_provider_and_own_batch_runs_are_reported() {
        let db = test_pool().await;
        let id = batch_run(&db, "me", Duration::seconds(120)).await;
        assert_eq!(run_provider(&db, id).await.unwrap().as_deref(), Some("anthropic"));
        assert_eq!(my_batch_watch_ids(&db, "me").await.unwrap(), vec!["w1".to_string()]);
        assert!(my_batch_watch_ids(&db, "someone-else").await.unwrap().is_empty());
    }

    /// A row builder over responses shaped `{"in": n, "out": m}`: stands in for the
    /// usage ledger (the agent crate's, which builds the real row) so these tests
    /// exercise the claim/fence/transaction, not pricing.
    fn tokens_row(calls: &[StoredResponse]) -> kyomi_core::Result<Option<ApiUsageInsert>> {
        let sum = |key: &str| calls.iter().map(|c| c.response[key].as_i64().unwrap_or(0)).sum::<i64>() as i32;
        let (input, output) = (sum("in"), sum("out"));
        if input == 0 && output == 0 {
            return Ok(None);
        }
        Ok(Some(ApiUsageInsert {
            provider: "anthropic".into(),
            model: "m".into(),
            input_tokens: input,
            output_tokens: output,
            total_tokens: input + output,
            cost_estimate: 0.5,
            component: "kyomi_watch".into(),
            provider_cost_usd: None,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            extra_metadata: Some("{}".into()),
            request_id: None,
        }))
    }

    fn bill() -> UsageBill<'static> {
        UsageBill { encryption_key: &KEY, user_id: "u1", workspace_id: "ws1", session_id: "s" }
    }

    #[tokio::test]
    async fn usage_is_written_with_the_result_summed_over_the_run_and_billed_at_most_once_by_the_lease_owner() {
        let db = test_pool().await;
        let id = batch_run(&db, "me", Duration::seconds(120)).await;
        let a = begin_call(&db, &KEY, id, 0, "anthropic", "m").await.unwrap();
        let b = begin_call(&db, &KEY, id, 1, "anthropic", "m").await.unwrap();
        finish_call(&db, &KEY, &a.call_id, BatchCallState::Succeeded, Some(&serde_json::json!({"in": 20, "out": 9})), None).await.unwrap();
        finish_call(&db, &KEY, &b.call_id, BatchCallState::Abandoned, None, Some("deadline")).await.unwrap();
        assert!(attach_fallback_response(&db, &KEY, &b.call_id, &serde_json::json!({"in": 15, "out": 5})).await.unwrap());

        // Billing is one transaction: claim + read the stored responses + write.
        // Only the live lease owner can bill, once.
        let now = Utc::now();
        assert!(!bill_run_once(&db, id, BillingFence::LeaseOwner("someone-else"), now, &bill(), tokens_row).await.unwrap(), "a displaced driver must not bill");
        let rows = |db: &DbPool| { let db = db.clone(); async move { sqlx::query_as::<_, (i64, i64)>("SELECT input_tokens, output_tokens FROM api_usage_log").fetch_all(sqlite_pool(&db)).await.unwrap() } };
        assert!(rows(&db).await.is_empty(), "a refused claim writes nothing");
        assert!(bill_run_once(&db, id, BillingFence::LeaseOwner("me"), now, &bill(), tokens_row).await.unwrap());
        assert_eq!(rows(&db).await, vec![(35, 14)], "every call's stored response, including the realtime fallback, is in the one row");
        assert!(!bill_run_once(&db, id, BillingFence::LeaseOwner("me"), now, &bill(), tokens_row).await.unwrap(), "a run is billed once, however many attempts it took");
        assert_eq!(rows(&db).await.len(), 1);

        // A lapsed lease cannot bill either.
        let id2 = another_batch_run(&db, "me", Duration::seconds(-5)).await;
        assert!(!bill_run_once(&db, id2, BillingFence::LeaseOwner("me"), Utc::now(), &bill(), tokens_row).await.unwrap());

        // A failed row build rolls the claim back, so the run can still be billed.
        let id3 = another_batch_run(&db, "me", Duration::seconds(120)).await;
        let c = begin_call(&db, &KEY, id3, 0, "anthropic", "m").await.unwrap();
        finish_call(&db, &KEY, &c.call_id, BatchCallState::Succeeded, Some(&serde_json::json!({"in": 3, "out": 1})), None).await.unwrap();
        let failing = |_: &[StoredResponse]| Err(kyomi_core::Error::Internal("cannot price".into()));
        assert!(bill_run_once(&db, id3, BillingFence::LeaseOwner("me"), Utc::now(), &bill(), failing).await.is_err());
        assert!(bill_run_once(&db, id3, BillingFence::LeaseOwner("me"), Utc::now(), &bill(), tokens_row).await.unwrap(), "the failed attempt left no claim behind");
    }

    #[tokio::test]
    async fn a_reaped_run_is_billed_for_what_it_paid_before_the_stop_and_only_once() {
        let db = test_pool().await;
        let id = batch_run(&db, "wedged", Duration::seconds(120)).await;
        let c = begin_call(&db, &KEY, id, 0, "anthropic", "m").await.unwrap();
        finish_call(&db, &KEY, &c.call_id, BatchCallState::Succeeded, Some(&serde_json::json!({"in": 11, "out": 4})), None).await.unwrap();

        // A live (not yet stopped) run cannot be billed by the StoppedRun fence.
        assert!(!bill_run_once(&db, id, BillingFence::StoppedRun, Utc::now(), &bill(), tokens_row).await.unwrap());

        set_deadline(&db, id, Duration::minutes(90)).await;
        assert_eq!(reap_overdue_runs(&db, Utc::now(), Duration::minutes(40)).await.unwrap().len(), 1);
        assert!(bill_run_once(&db, id, BillingFence::StoppedRun, Utc::now(), &bill(), tokens_row).await.unwrap(), "the reaper bills what the run paid");
        assert!(!bill_run_once(&db, id, BillingFence::StoppedRun, Utc::now(), &bill(), tokens_row).await.unwrap(), "only once");
        let rows: Vec<(i64, i64)> = sqlx::query_as("SELECT input_tokens, output_tokens FROM api_usage_log").fetch_all(sqlite_pool(&db)).await.unwrap();
        assert_eq!(rows, vec![(11, 4)]);
    }
}

#[cfg(test)]
#[path = "watch_batch_service_pg_tests.rs"]
mod pg_tests;
