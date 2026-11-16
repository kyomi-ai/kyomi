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
//!   `(execution_id, request_hash)`. A resumed run replays from the start; a
//!   call it already made (same request body, same hash) is answered from the
//!   cache, a submitted-but-unfinished call is re-polled by its vendor batch
//!   id, and only a genuinely new request is submitted. Response bodies are
//!   encrypted at rest like chat messages.

use chrono::{DateTime, Utc};
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
    pub request_hash: String,
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
    request_hash: String,
    provider: String,
    model: String,
    vendor_batch_id: Option<String>,
    state: String,
    response_encrypted: Option<String>,
    error_message: Option<String>,
}

const CALL_COLUMNS: &str = "call_id, execution_id, request_hash, provider, model, \
     vendor_batch_id, state, response_encrypted, error_message";

fn to_call(row: CallRow, key: &[u8; 32]) -> kyomi_core::Result<BatchCall> {
    Ok(BatchCall {
        call_id: row.call_id,
        execution_id: row.execution_id,
        request_hash: row.request_hash,
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

/// The call recorded for `(execution_id, request_hash)`, if any.
pub async fn find_call(
    db: &DbPool,
    key: &[u8; 32],
    execution_id: i32,
    request_hash: &str,
) -> kyomi_core::Result<Option<BatchCall>> {
    let sql = format!(
        "SELECT {CALL_COLUMNS} FROM watch_batch_calls \
         WHERE execution_id = $1 AND request_hash = $2"
    );
    kyomi_core::db_fetch_optional!(db, CallRow, &sql, execution_id, request_hash)
        .map_err(|e| db_err("lookup", e))?
        .map(|r| to_call(r, key))
        .transpose()
}

/// Record that a call is about to be submitted. Returns the existing row when
/// another writer (or this run before a crash) already recorded the same
/// request, so a request is never recorded twice.
pub async fn begin_call(
    db: &DbPool,
    key: &[u8; 32],
    execution_id: i32,
    request_hash: &str,
    provider: &str,
    model: &str,
) -> kyomi_core::Result<BatchCall> {
    let call_id = uuid::Uuid::new_v4().to_string();
    kyomi_core::db_execute!(
        db,
        "INSERT INTO watch_batch_calls \
            (call_id, execution_id, request_hash, provider, model, state, submitted_at) \
         VALUES ($1, $2, $3, $4, $5, 'submitting', $6) \
         ON CONFLICT (execution_id, request_hash) DO NOTHING",
        &call_id,
        execution_id,
        request_hash,
        provider,
        model,
        Utc::now()
    )
    .map_err(|e| db_err("record call", e))?;
    find_call(db, key, execution_id, request_hash)
        .await?
        .ok_or_else(|| db_err("record call", "row vanished after insert"))
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

/// Finish a call. Only a non-final call is changed, so a late result can never
/// overwrite a run's earlier decision (`Abandoned`) or a recorded response.
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
             lease_owner = $6, lease_expires_at = $7 \
         WHERE id = $8",
        run.provider,
        run.model,
        run.mode,
        run.deadline,
        run.fallback_reason,
        run.lease.map(|l| l.0),
        run.lease.map(|l| l.1),
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

    async fn batch_run(db: &DbPool, owner: &str, lease_for: Duration) -> i32 {
        let id = execution(db).await;
        let now = Utc::now();
        record_run_llm(db, id, &RunLlm { provider: "anthropic", model: "m", mode: "batch", deadline: Some(now + Duration::hours(3)), fallback_reason: None, lease: Some((owner, now + lease_for)) })
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn begin_call_is_idempotent_per_request_and_distinct_per_hash() {
        let db = test_pool().await;
        let id = execution(&db).await;
        let a = begin_call(&db, &KEY, id, "h1", "anthropic", "m").await.unwrap();
        let again = begin_call(&db, &KEY, id, "h1", "anthropic", "m").await.unwrap();
        assert_eq!(a.call_id, again.call_id, "the same request must not be recorded twice");
        let b = begin_call(&db, &KEY, id, "h2", "anthropic", "m").await.unwrap();
        assert_ne!(a.call_id, b.call_id);
        assert_eq!(a.state, BatchCallState::Submitting);
    }

    #[tokio::test]
    async fn a_finished_call_round_trips_its_response_encrypted_and_is_never_overwritten() {
        let db = test_pool().await;
        let id = execution(&db).await;
        let c = begin_call(&db, &KEY, id, "h1", "anthropic", "m").await.unwrap();
        mark_submitted(&db, &c.call_id, "msgbatch_1").await.unwrap();
        let body = serde_json::json!({"content": [{"type": "text", "text": "secret-finding"}]});
        assert!(finish_call(&db, &KEY, &c.call_id, BatchCallState::Succeeded, Some(&body), None).await.unwrap());

        let got = find_call(&db, &KEY, id, "h1").await.unwrap().unwrap();
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
        assert_eq!(find_call(&db, &KEY, id, "h1").await.unwrap().unwrap().state, BatchCallState::Succeeded);
    }

    #[tokio::test]
    async fn batch_ids_are_listed_in_submission_order_and_synced_to_the_execution() {
        let db = test_pool().await;
        let id = execution(&db).await;
        for (h, b) in [("h1", "b1"), ("h2", "b2")] {
            let c = begin_call(&db, &KEY, id, h, "anthropic", "m").await.unwrap();
            mark_submitted(&db, &c.call_id, b).await.unwrap();
        }
        let _unsubmitted = begin_call(&db, &KEY, id, "h3", "anthropic", "m").await.unwrap();
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
            record_run_llm(&db, e.id, &RunLlm { provider: "anthropic", model: "m", mode: "batch", deadline: None, fallback_reason: None, lease: Some(("a", now + Duration::seconds(60))) }).await.unwrap();
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
        record_run_llm(&db, id, &RunLlm { provider: "anthropic", model: "m", mode: "realtime", deadline: None, fallback_reason: Some("schedule gap 30m is shorter than the 60m batch window"), lease: None }).await.unwrap();
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
        begin_call(&db, &KEY, id, "h1", "anthropic", "m").await.unwrap();
        sqlx::query("PRAGMA foreign_keys = ON").execute(sqlite_pool(&db)).await.unwrap();
        sqlx::query("DELETE FROM watch_executions WHERE id = $1").bind(id).execute(sqlite_pool(&db)).await.unwrap();
        assert!(find_call(&db, &KEY, id, "h1").await.unwrap().is_none());
    }
}
