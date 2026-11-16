// SPDX-License-Identifier: AGPL-3.0-or-later

//! The batch-mode LLM provider for a watch run (KYO-973).
//!
//! [`BatchingProvider`] wraps the run's realtime provider. Each `complete()`
//! call is the *nth model call of the run*; it is answered from the run's
//! durable call row at position n if there is one (a finished response, or a
//! vendor batch still being polled), and otherwise submitted as a new batch and
//! waited for.
//!
//! Because calls are keyed by position, not by request hash, a run that is
//! resumed after a restart simply replays from the start: nothing already paid
//! for is resubmitted even if replayed tool results (or context such as recent
//! alerts) differ from the first attempt. The request hash is recorded only so
//! drift can be logged.
//!
//! The wait is a lightweight poll loop. The run's lease is renewed by a
//! heartbeat for the whole run ([`LeaseHeartbeat`]) as well as on every call; if
//! it is lost to another instance this instance stops quietly
//! ([`BatchRunContext::lease_lost`]), because the record belongs to the new
//! owner. A sibling pattern for chat runs is `durable_chat`; this is a smaller
//! compare-and-swap lease on `watch_executions` because a watch run has no
//! conversation event store.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kyomi_auth::watch_batch_service::{self as store, BatchCall, BatchCallState};
use kyomi_core::watch_llm::BatchFailurePolicy;
use kyomi_core::DbPool;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::batch::{BatchLLMProvider, BatchPoll};
use crate::provider::{LLMProvider, RequestOptions};
use crate::types::{LLMResponse, Message, Tool};

/// How often a waiting run polls the vendor (both vendors suggest ~1/minute).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// How long a lease lasts without renewal. Renewed by the heartbeat.
pub const LEASE_TTL_SECONDS: i64 = 180;

/// How often the heartbeat renews the lease: a sixth of the TTL, so several
/// consecutive renewals can fail before the lease lapses.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// The poll interval a run uses. A test can shorten it process-wide; no
/// production path does.
pub fn poll_interval() -> Duration {
    #[cfg(test)]
    {
        let ms = TEST_POLL_MS.load(Ordering::Relaxed);
        if ms > 0 {
            return Duration::from_millis(ms);
        }
    }
    DEFAULT_POLL_INTERVAL
}

/// The heartbeat interval; shortened with the poll interval under test.
pub fn heartbeat_interval() -> Duration {
    poll_interval().min(HEARTBEAT_INTERVAL)
}

#[cfg(test)]
pub(crate) static TEST_POLL_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Process-wide instance identity used as the lease owner.
pub fn instance_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| format!("{}:{}", std::process::id(), uuid::Uuid::new_v4()))
}

fn lease_expiry() -> DateTime<Utc> {
    Utc::now() + chrono::Duration::seconds(LEASE_TTL_SECONDS)
}

/// Told when the run starts or stops waiting on a batch, so the UI can show it
/// live. Best-effort: a failure here never affects the run.
pub type WaitingHook = Arc<dyn Fn(bool) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

/// Everything a batch-mode run needs besides the provider.
#[derive(Clone)]
pub struct BatchRunContext {
    pub db: DbPool,
    pub encryption_key: Arc<[u8; 32]>,
    pub execution_id: i32,
    pub owner: String,
    /// Latest time the run may still wait on a batch.
    pub deadline: DateTime<Utc>,
    pub failure_policy: BatchFailurePolicy,
    pub poll_interval: Duration,
    /// Set when this instance lost the run's lease to another instance.
    pub lease_lost: Arc<AtomicBool>,
    pub on_waiting: Option<WaitingHook>,
    /// Position of the next model call of the run, shared by every provider of
    /// one attempt (the main turn and the JSON-correction retry).
    pub call_seq: Arc<AtomicI32>,
}

/// Renews a batch run's lease for as long as it is alive, whatever the run is
/// doing (waiting, replaying tool calls, or finishing in realtime). Sets
/// `lease_lost` when another instance owns the run.
pub struct LeaseHeartbeat {
    task: tokio::task::JoinHandle<()>,
}

impl LeaseHeartbeat {
    pub fn start(db: DbPool, execution_id: i32, owner: String, lease_lost: Arc<AtomicBool>) -> Self {
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(heartbeat_interval()).await;
                match store::claim_lease(&db, execution_id, &owner, lease_expiry(), Utc::now()).await {
                    Ok(true) => {}
                    Ok(false) => {
                        lease_lost.store(true, Ordering::SeqCst);
                        return;
                    }
                    // A transient DB error must not end a healthy run: the lease
                    // only lapses after several missed renewals.
                    Err(e) => warn!(execution_id, error = %e, "failed to renew the batch run lease; will retry"),
                }
            }
        });
        Self { task }
    }
}

impl Drop for LeaseHeartbeat {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub struct BatchingProvider {
    inner: Box<dyn LLMProvider>,
    provider_name: String,
    ctx: BatchRunContext,
}

/// Whether this instance still owns the run.
enum Lease {
    Held,
    Lost,
}

impl BatchingProvider {
    pub fn new(inner: Box<dyn LLMProvider>, provider_name: &str, ctx: BatchRunContext) -> Self {
        Self {
            inner,
            provider_name: provider_name.to_string(),
            ctx,
        }
    }

    async fn set_waiting(&self, waiting: bool) {
        if let Err(e) = store::set_waiting(&self.ctx.db, self.ctx.execution_id, waiting).await {
            warn!(execution_id = self.ctx.execution_id, error = %e, "failed to update the waiting status");
        }
        if let Some(hook) = &self.ctx.on_waiting {
            hook(waiting).await;
        }
    }

    /// Renew the lease. A lost lease is final; a database error is not (the
    /// run keeps going and the lease only lapses after several misses).
    async fn hold_lease(&self) -> Lease {
        if self.ctx.lease_lost.load(Ordering::SeqCst) {
            return Lease::Lost;
        }
        match store::claim_lease(&self.ctx.db, self.ctx.execution_id, &self.ctx.owner, lease_expiry(), Utc::now()).await {
            Ok(true) => Lease::Held,
            Ok(false) => {
                self.ctx.lease_lost.store(true, Ordering::SeqCst);
                Lease::Lost
            }
            Err(e) => {
                warn!(execution_id = self.ctx.execution_id, error = %e, "failed to renew the batch run lease; will retry");
                Lease::Held
            }
        }
    }

    fn lease_lost_error() -> kyomi_core::Error {
        kyomi_core::Error::Internal("watch run lease was taken by another instance".into())
    }

    /// Realtime call that also keeps its response for replay.
    async fn realtime_and_keep(
        &self,
        call: Option<&BatchCall>,
        messages: &[Message],
        tools: &[Tool],
        options: &RequestOptions,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        let response = self.inner.complete(messages, tools, options, user_names).await?;
        if let Some(call) = call
            && let Err(e) = store::attach_fallback_response(&self.ctx.db, &self.ctx.encryption_key, &call.call_id, &response.raw_response).await
        {
            warn!(execution_id = self.ctx.execution_id, error = %e, "failed to keep the realtime fallback response");
        }
        Ok(response)
    }

    /// Apply the failure policy to a batch that did not produce a response.
    async fn on_batch_failure(
        &self,
        call: &BatchCall,
        reason: String,
        messages: &[Message],
        tools: &[Tool],
        options: &RequestOptions,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        warn!(execution_id = self.ctx.execution_id, %reason, policy = ?self.ctx.failure_policy, "watch batch did not produce a response");
        store::note_fallback(&self.ctx.db, self.ctx.execution_id, &reason).await?;
        match self.ctx.failure_policy {
            BatchFailurePolicy::RealtimeRetry => {
                self.realtime_and_keep(Some(call), messages, tools, options, user_names).await
            }
            BatchFailurePolicy::Error => Err(kyomi_core::Error::Internal(format!(
                "watch batch failed: {reason}"
            ))),
        }
    }

    /// The nth call hit the run's deadline: stop waiting, abandon the batch (a
    /// late result is ignored) and answer in realtime. This is the guard that
    /// keeps a slow batch from delaying or dropping an alert, so it applies
    /// whatever the failure policy.
    async fn on_deadline(
        &self,
        batch: &dyn BatchLLMProvider,
        call: &BatchCall,
        messages: &[Message],
        tools: &[Tool],
        options: &RequestOptions,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        let reason = "batch not finished before the next scheduled run was due; \
                      cancelled and re-run in realtime"
            .to_string();
        store::finish_call(&self.ctx.db, &self.ctx.encryption_key, &call.call_id, BatchCallState::Abandoned, None, Some(&reason)).await?;
        if let Some(vendor_id) = &call.vendor_batch_id
            && let Err(e) = batch.cancel(vendor_id).await
        {
            warn!(batch_id = %vendor_id, error = %e, "failed to cancel the abandoned batch");
        }
        store::note_fallback(&self.ctx.db, self.ctx.execution_id, &reason).await?;
        self.realtime_and_keep(Some(call), messages, tools, options, user_names).await
    }
}

fn request_hash(body: &serde_json::Value) -> String {
    hex::encode(Sha256::digest(body.to_string().as_bytes()))
}

#[async_trait]
impl LLMProvider for BatchingProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[Tool],
        options: &RequestOptions,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        let batch = self.inner.batch().ok_or_else(|| {
            kyomi_core::Error::Internal(format!(
                "batch mode is not supported by the {} provider; use realtime",
                self.provider_name
            ))
        })?;
        let index = self.ctx.call_seq.fetch_add(1, Ordering::SeqCst);
        let key = &self.ctx.encryption_key;
        if matches!(self.hold_lease().await, Lease::Lost) {
            return Err(Self::lease_lost_error());
        }

        let body = batch.build_body(messages, tools, options, user_names);
        let hash = request_hash(&body);
        let past_deadline = Utc::now() >= self.ctx.deadline;

        // The nth call already has a row: this is a replay of an earlier attempt.
        let existing = store::find_call(&self.ctx.db, key, self.ctx.execution_id, index).await?;
        let call = match existing {
            Some(call) => {
                if call.request_hash != hash {
                    info!(execution_id = self.ctx.execution_id, index, "replayed request differs from the recorded one; answering from the recorded call");
                }
                match call.state {
                    BatchCallState::Succeeded => {
                        let response = call.response.as_ref().ok_or_else(|| {
                            kyomi_core::Error::Internal("succeeded batch call has no stored response".into())
                        })?;
                        return batch.parse_response(response);
                    }
                    // A realtime answer was already obtained for this position.
                    BatchCallState::Failed | BatchCallState::Expired | BatchCallState::Abandoned => {
                        if let Some(response) = &call.response {
                            return batch.parse_response(response);
                        }
                        if call.state == BatchCallState::Abandoned {
                            // The deadline fallback, not a failure: no policy applies.
                            return self.realtime_and_keep(Some(&call), messages, tools, options, user_names).await;
                        }
                        let reason = call.error_message.clone().unwrap_or_else(|| format!("earlier batch call ended {}", call.state.as_str()));
                        return self.on_batch_failure(&call, reason, messages, tools, options, user_names).await;
                    }
                    BatchCallState::Submitting | BatchCallState::Pending => call,
                }
            }
            None => {
                if past_deadline {
                    // No time left to wait: this and every later call is realtime,
                    // and nothing is submitted.
                    store::note_fallback(&self.ctx.db, self.ctx.execution_id, "run reached its batch deadline; remaining calls ran in realtime").await?;
                    return self.inner.complete(messages, tools, options, user_names).await;
                }
                store::begin_call(&self.ctx.db, key, self.ctx.execution_id, index, &hash, &self.provider_name, self.inner.model()).await?
            }
        };

        let vendor_id = match &call.vendor_batch_id {
            Some(id) => id.clone(),
            None if past_deadline => {
                // Recorded but never submitted, and out of time.
                return self.on_deadline(batch, &call, messages, tools, options, user_names).await;
            }
            None => match batch.submit(&call.call_id, &body).await {
                Ok(id) => {
                    store::mark_submitted(&self.ctx.db, &call.call_id, &id).await?;
                    info!(execution_id = self.ctx.execution_id, batch_id = %id, "submitted watch batch");
                    id
                }
                Err(e) => {
                    // Never submitted: no vendor batch to wait for.
                    store::finish_call(&self.ctx.db, key, &call.call_id, BatchCallState::Failed, None, Some(&e.to_string())).await?;
                    return self
                        .on_batch_failure(&call, format!("batch submit failed: {e}"), messages, tools, options, user_names)
                        .await;
                }
            },
        };
        let call = BatchCall { vendor_batch_id: Some(vendor_id.clone()), ..call };
        if let Err(e) = store::sync_batch_ids(&self.ctx.db, self.ctx.execution_id).await {
            warn!(execution_id = self.ctx.execution_id, error = %e, "failed to record the batch id on the execution");
        }

        self.set_waiting(true).await;
        let outcome = loop {
            if Utc::now() >= self.ctx.deadline {
                break None;
            }
            if matches!(self.hold_lease().await, Lease::Lost) {
                return Err(Self::lease_lost_error());
            }
            match batch.poll(&vendor_id, &call.call_id).await {
                Ok(BatchPoll::Pending) => tokio::time::sleep(self.ctx.poll_interval).await,
                Ok(other) => break Some(other),
                // A transient poll error (already retried) must not lose a
                // run that is otherwise fine; keep waiting until the deadline.
                Err(e) if e.is_transient() => {
                    warn!(execution_id = self.ctx.execution_id, error = %e, "batch poll failed; will retry");
                    tokio::time::sleep(self.ctx.poll_interval).await;
                }
                Err(e) => break Some(BatchPoll::Failed(e.to_string())),
            }
        };
        self.set_waiting(false).await;

        match outcome {
            Some(BatchPoll::Succeeded(response)) => {
                store::finish_call(&self.ctx.db, key, &call.call_id, BatchCallState::Succeeded, Some(&response), None).await?;
                batch.parse_response(&response)
            }
            Some(BatchPoll::Failed(reason)) => {
                store::finish_call(&self.ctx.db, key, &call.call_id, BatchCallState::Failed, None, Some(&reason)).await?;
                self.on_batch_failure(&call, reason, messages, tools, options, user_names).await
            }
            Some(BatchPoll::Expired) => {
                let reason = "the provider batch expired before completing".to_string();
                store::finish_call(&self.ctx.db, key, &call.call_id, BatchCallState::Expired, None, Some(&reason)).await?;
                self.on_batch_failure(&call, reason, messages, tools, options, user_names).await
            }
            Some(BatchPoll::Pending) => unreachable!("pending never breaks the poll loop"),
            None => self.on_deadline(batch, &call, messages, tools, options, user_names).await,
        }
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    fn context_window(&self) -> u32 {
        self.inner.context_window()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::AnthropicClient;
    use crate::test_support::{anthropic_text_reply, FakeHttpServer, RecordedRequest};
    use serde_json::{json, Value};
    use std::sync::atomic::AtomicUsize;

    const KEY: [u8; 32] = [7u8; 32];

    struct Fixture {
        db: DbPool,
        execution_id: i32,
    }

    async fn fixture() -> Fixture {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        kyomi_core::db_execute!(
            db,
            "INSERT INTO watches (watch_id, workspace_id, created_by, name, prompt, schedule, enabled) \
             VALUES ('w1', 'ws-1', 'user-a', 'W', 'p', '0 9 * * *', 1)"
        )
        .unwrap();
        let e = kyomi_auth::watch_service::create_execution(&db, "w1", "W", "ws-1", kyomi_core::WatchMode::Alert, "user-a")
            .await
            .unwrap();
        store::record_run_llm(
            &db,
            e.id,
            &store::RunLlm {
                provider: "anthropic",
                model: "claude-x",
                mode: "batch",
                deadline: None,
                fallback_reason: None,
                lease: Some(("me", Utc::now() + chrono::Duration::seconds(LEASE_TTL_SECONDS))),
            },
        )
        .await
        .unwrap();
        Fixture { db, execution_id: e.id }
    }

    fn ctx(f: &Fixture, deadline_in: chrono::Duration, policy: BatchFailurePolicy) -> BatchRunContext {
        BatchRunContext {
            db: f.db.clone(),
            encryption_key: Arc::new(KEY),
            execution_id: f.execution_id,
            owner: "me".into(),
            deadline: Utc::now() + deadline_in,
            failure_policy: policy,
            poll_interval: Duration::from_millis(5),
            lease_lost: Arc::new(AtomicBool::new(false)),
            on_waiting: None,
            call_seq: Arc::new(AtomicI32::new(0)),
        }
    }

    fn provider(server: &FakeHttpServer, ctx: BatchRunContext) -> BatchingProvider {
        let client = AnthropicClient::with_base_url("k".into(), Some("claude-x".into()), server.base_url.clone()).unwrap();
        BatchingProvider::new(Box::new(client), "anthropic", ctx)
    }

    fn msgs() -> Vec<Message> {
        vec![Message::system("sys"), Message::user("check")]
    }

    async fn call(p: &BatchingProvider) -> kyomi_core::Result<LLMResponse> {
        p.complete(&msgs(), &[], &RequestOptions::new(256), &HashMap::new()).await
    }

    /// An Anthropic fake: `polls_pending` in-progress polls, then `end`.
    fn anthropic_batches(polls_pending: usize, end: &'static str) -> (impl Fn(&RecordedRequest) -> (u16, String), Arc<AtomicUsize>) {
        let polls = Arc::new(AtomicUsize::new(0));
        let p = polls.clone();
        (
            move |req: &RecordedRequest| match (req.method.as_str(), req.path.as_str()) {
                ("POST", "/v1/messages/batches") => (200, json!({"id": "msgbatch_1", "processing_status": "in_progress"}).to_string()),
                ("GET", "/v1/messages/batches/msgbatch_1") => {
                    if p.fetch_add(1, Ordering::SeqCst) < polls_pending {
                        (200, json!({"processing_status": "in_progress"}).to_string())
                    } else {
                        let host = req.headers.get("host").cloned().unwrap_or_default();
                        (200, json!({"processing_status": "ended", "results_url": format!("http://{host}/results")}).to_string())
                    }
                }
                ("GET", "/results") => {
                    let id = req.headers.get("x-test-custom-id").cloned();
                    let _ = id;
                    (200, end.to_string())
                }
                ("POST", "/v1/messages") => (200, anthropic_text_reply("realtime answer")),
                ("POST", "/v1/messages/batches/msgbatch_1/cancel") => (200, json!({"processing_status": "canceling"}).to_string()),
                _ => (404, "{}".into()),
            },
            polls,
        )
    }

    /// The results line must carry the call's own custom_id; look it up from the DB.
    async fn ended_line(f: &Fixture, text: &str) -> &'static str {
        let row: (String,) = sqlx::query_as("SELECT call_id FROM watch_batch_calls LIMIT 1")
            .fetch_one(kyomi_test_db::sqlite_pool(&f.db))
            .await
            .unwrap();
        Box::leak(
            json!({"custom_id": row.0, "result": {"type": "succeeded", "message": serde_json::from_str::<Value>(&anthropic_text_reply(text)).unwrap()}})
                .to_string()
                .into_boxed_str(),
        )
    }

    fn count(server: &FakeHttpServer, method: &str, path: &str) -> usize {
        server.requests().iter().filter(|r| r.method == method && r.path == path).count()
    }

    #[tokio::test]
    async fn a_batch_call_submits_waits_returns_the_response_and_records_it() {
        let f = fixture().await;
        // The custom_id is only known after the row exists, so serve the
        // results line lazily from the DB via a pre-registered call id.
        let line = {
            let body = BatchingProvider::new(
                Box::new(AnthropicClient::with_base_url("k".into(), Some("claude-x".into()), "http://unused".into()).unwrap()),
                "anthropic",
                ctx(&f, chrono::Duration::hours(1), BatchFailurePolicy::RealtimeRetry),
            );
            let b = body.inner.batch().unwrap().build_body(&msgs(), &[], &RequestOptions::new(256), &HashMap::new());
            let c = store::begin_call(&f.db, &KEY, f.execution_id, 0, &request_hash(&b), "anthropic", "claude-x").await.unwrap();
            Box::leak(
                json!({"custom_id": c.call_id, "result": {"type": "succeeded", "message": serde_json::from_str::<Value>(&anthropic_text_reply("batched answer")).unwrap()}})
                    .to_string()
                    .into_boxed_str(),
            ) as &'static str
        };
        let (handler, polls) = anthropic_batches(2, line);
        let server = FakeHttpServer::start(handler).await;
        let p = provider(&server, ctx(&f, chrono::Duration::hours(1), BatchFailurePolicy::RealtimeRetry));

        let r = call(&p).await.unwrap();
        assert_eq!(r.content, "batched answer");
        assert_eq!(polls.load(Ordering::SeqCst), 3, "two in-progress polls then the ended poll");
        assert_eq!(count(&server, "POST", "/v1/messages/batches"), 1);
        assert_eq!(count(&server, "POST", "/v1/messages"), 0, "no realtime call on the happy path");

        let e = kyomi_auth::watch_service::get_execution_by_id_only(&f.db, f.execution_id, "ws-1", "user-a").await.unwrap().unwrap();
        assert_eq!(e.status, kyomi_core::WatchExecutionStatus::Running, "back to running after the wait");
        assert_eq!(e.batch_ids.as_deref(), Some(r#"["msgbatch_1"]"#));
        assert_eq!(e.llm_mode.as_deref(), Some("batch"));
    }

    #[tokio::test]
    async fn the_run_shows_waiting_on_batch_while_the_batch_is_pending() {
        let f = fixture().await;
        let (handler, _) = anthropic_batches(usize::MAX, "");
        let server = FakeHttpServer::start(handler).await;
        let p = provider(&server, ctx(&f, chrono::Duration::hours(1), BatchFailurePolicy::RealtimeRetry));
        let task = tokio::spawn(async move { call(&p).await });
        let status = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let e = kyomi_auth::watch_service::get_execution_by_id_only(&f.db, f.execution_id, "ws-1", "user-a").await.unwrap().unwrap();
                if e.status == kyomi_core::WatchExecutionStatus::WaitingBatch {
                    break e.status;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the run never showed waiting_batch");
        assert_eq!(status, kyomi_core::WatchExecutionStatus::WaitingBatch);
        task.abort();
    }

    #[tokio::test]
    async fn a_restarted_run_answers_finished_calls_from_the_cache_without_any_request() {
        let f = fixture().await;
        let b = AnthropicClient::with_base_url("k".into(), Some("claude-x".into()), "http://unused".into()).unwrap();
        let body = crate::batch::BatchLLMProvider::build_body(&b, &msgs(), &[], &RequestOptions::new(256), &HashMap::new());
        let c = store::begin_call(&f.db, &KEY, f.execution_id, 0, &request_hash(&body), "anthropic", "claude-x").await.unwrap();
        store::mark_submitted(&f.db, &c.call_id, "msgbatch_old").await.unwrap();
        store::finish_call(&f.db, &KEY, &c.call_id, BatchCallState::Succeeded, Some(&serde_json::from_str(&anthropic_text_reply("from the first life")).unwrap()), None).await.unwrap();

        let server = FakeHttpServer::start(|_| (500, "{}".into())).await;
        let p = provider(&server, ctx(&f, chrono::Duration::hours(1), BatchFailurePolicy::RealtimeRetry));
        assert_eq!(call(&p).await.unwrap().content, "from the first life");
        assert!(server.requests().is_empty(), "a cached call must not touch the network");
    }

    #[tokio::test]
    async fn a_restart_mid_wait_resumes_polling_the_same_vendor_batch_and_never_resubmits() {
        let f = fixture().await;
        // First life: the batch is submitted and never finishes before the "crash".
        let (h1, _) = anthropic_batches(usize::MAX, "");
        let s1 = FakeHttpServer::start(h1).await;
        let p1 = provider(&s1, ctx(&f, chrono::Duration::hours(1), BatchFailurePolicy::RealtimeRetry));
        let t = tokio::spawn(async move { call(&p1).await });
        tokio::time::timeout(Duration::from_secs(20), async {
            while count(&s1, "POST", "/v1/messages/batches") == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the first life never submitted a batch");
        t.abort(); // the process dies mid-wait
        assert_eq!(count(&s1, "POST", "/v1/messages/batches"), 1);

        // Second life (a new provider over the same durable state): the batch has ended.
        let line = ended_line(&f, "after the restart").await;
        let (h2, _) = anthropic_batches(0, line);
        let s2 = FakeHttpServer::start(h2).await;
        let p2 = provider(&s2, ctx(&f, chrono::Duration::hours(1), BatchFailurePolicy::RealtimeRetry));
        assert_eq!(call(&p2).await.unwrap().content, "after the restart");
        assert_eq!(count(&s2, "POST", "/v1/messages/batches"), 0, "the resumed run must not submit a second batch");
        assert_eq!(store::batch_ids(&f.db, f.execution_id).await.unwrap(), vec!["msgbatch_1"]);
    }

    #[tokio::test]
    async fn a_failed_batch_retries_once_in_realtime_under_the_default_policy_and_records_why() {
        let f = fixture().await;
        let (handler, _) = anthropic_batches(0, "");
        let server = FakeHttpServer::start(move |req: &RecordedRequest| {
            if req.path == "/results" {
                // custom_id patched below via DB lookup is not possible here; an
                // empty results body means "no result for this request" -> failed.
                (200, String::new())
            } else {
                handler(req)
            }
        })
        .await;
        let p = provider(&server, ctx(&f, chrono::Duration::hours(1), BatchFailurePolicy::RealtimeRetry));
        let r = call(&p).await.unwrap();
        assert_eq!(r.content, "realtime answer");
        assert_eq!(count(&server, "POST", "/v1/messages"), 1, "exactly one realtime retry");
        let e = kyomi_auth::watch_service::get_execution_by_id_only(&f.db, f.execution_id, "ws-1", "user-a").await.unwrap().unwrap();
        assert!(e.batch_fallback_reason.unwrap().contains("no result"));
    }

    #[tokio::test]
    async fn a_failed_batch_fails_the_call_under_the_error_policy_and_never_goes_realtime() {
        let f = fixture().await;
        let (handler, _) = anthropic_batches(0, "");
        let server = FakeHttpServer::start(move |req: &RecordedRequest| if req.path == "/results" { (200, String::new()) } else { handler(req) }).await;
        let p = provider(&server, ctx(&f, chrono::Duration::hours(1), BatchFailurePolicy::Error));
        let err = call(&p).await.unwrap_err();
        assert!(err.to_string().contains("watch batch failed"), "{err}");
        assert_eq!(count(&server, "POST", "/v1/messages"), 0);
    }

    #[tokio::test]
    async fn a_batch_still_pending_at_the_deadline_is_cancelled_and_the_rest_of_the_run_is_realtime() {
        let f = fixture().await;
        let (handler, _) = anthropic_batches(usize::MAX, "");
        let server = FakeHttpServer::start(handler).await;
        let p = provider(&server, ctx(&f, chrono::Duration::milliseconds(1500), BatchFailurePolicy::Error));
        // Even under the Error policy the deadline guard falls back: it is not a failure.
        assert_eq!(call(&p).await.unwrap().content, "realtime answer");
        assert_eq!(count(&server, "POST", "/v1/messages/batches/msgbatch_1/cancel"), 1, "the batch is cancelled");
        assert_eq!(count(&server, "POST", "/v1/messages"), 1);

        // Later calls of the same run never batch again.
        let other = vec![Message::system("sys"), Message::user("a different second request")];
        p.complete(&other, &[], &RequestOptions::new(256), &HashMap::new()).await.unwrap();
        assert_eq!(count(&server, "POST", "/v1/messages/batches"), 1, "no new batch after the deadline fallback");
        assert_eq!(count(&server, "POST", "/v1/messages"), 2);
        let e = kyomi_auth::watch_service::get_execution_by_id_only(&f.db, f.execution_id, "ws-1", "user-a").await.unwrap().unwrap();
        assert!(e.batch_fallback_reason.unwrap().contains("next scheduled run"));
    }

    #[tokio::test]
    async fn a_run_whose_lease_was_taken_stops_without_touching_the_provider_or_the_record() {
        let f = fixture().await;
        // Another instance claimed the run.
        kyomi_core::db_execute!(
            f.db,
            "UPDATE watch_executions SET lease_owner = 'someone-else', lease_expires_at = $1 WHERE id = $2",
            Utc::now() + chrono::Duration::seconds(120),
            f.execution_id
        )
        .unwrap();
        let (handler, _) = anthropic_batches(0, "");
        let server = FakeHttpServer::start(handler).await;
        let c = ctx(&f, chrono::Duration::hours(1), BatchFailurePolicy::RealtimeRetry);
        let flag = c.lease_lost.clone();
        let p = provider(&server, c);
        assert!(call(&p).await.is_err());
        assert!(flag.load(Ordering::SeqCst), "the lease loss is signalled to the caller");
        assert!(server.requests().is_empty(), "a displaced instance must not submit or call realtime");
    }

    #[tokio::test]
    async fn an_unsupported_provider_is_an_error_not_a_silent_realtime_call() {
        let f = fixture().await;
        let server = FakeHttpServer::start(|_| (200, "{}".into())).await;
        // An OpenAI-compatible endpoint that is not OpenRouter has no batch API.
        let plain = crate::openai::OpenAIProvider::new("k".into(), Some("m".into()), Some(server.base_url.clone())).unwrap();
        let p = BatchingProvider::new(Box::new(plain), "openai", ctx(&f, chrono::Duration::hours(1), BatchFailurePolicy::RealtimeRetry));
        assert!(call(&p).await.unwrap_err().to_string().contains("not supported"));
        assert!(server.requests().is_empty());
    }

    #[tokio::test]
    async fn a_replayed_call_whose_request_drifted_still_resumes_the_same_batch_and_never_resubmits() {
        let f = fixture().await;
        // First life submitted the call for position 0 with one request ...
        let (h1, _) = anthropic_batches(usize::MAX, "");
        let s1 = FakeHttpServer::start(h1).await;
        let p1 = provider(&s1, ctx(&f, chrono::Duration::hours(1), BatchFailurePolicy::RealtimeRetry));
        let t = tokio::spawn(async move { call(&p1).await });
        tokio::time::timeout(Duration::from_secs(20), async {
            while count(&s1, "POST", "/v1/messages/batches") == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("never submitted");
        t.abort();

        // ... and the replay builds a *different* request for position 0 (a
        // replayed tool result changed). It must poll the recorded batch.
        let line = ended_line(&f, "answer to the original").await;
        let (h2, _) = anthropic_batches(0, line);
        let s2 = FakeHttpServer::start(h2).await;
        let p2 = provider(&s2, ctx(&f, chrono::Duration::hours(1), BatchFailurePolicy::RealtimeRetry));
        let drifted = vec![Message::system("sys"), Message::user("check, with different replayed data")];
        let r = p2.complete(&drifted, &[], &RequestOptions::new(256), &HashMap::new()).await.unwrap();
        assert_eq!(r.content, "answer to the original");
        assert_eq!(count(&s2, "POST", "/v1/messages/batches"), 0, "a drifted request must not resubmit a paid batch");
        assert_eq!(store::batch_ids(&f.db, f.execution_id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_replayed_deadline_fallback_is_answered_from_the_stored_realtime_response_even_under_the_error_policy() {
        let f = fixture().await;
        // Life 1: deadline hit, answered in realtime, response kept.
        let (handler, _) = anthropic_batches(usize::MAX, "");
        let s1 = FakeHttpServer::start(handler).await;
        let p1 = provider(&s1, ctx(&f, chrono::Duration::milliseconds(1500), BatchFailurePolicy::Error));
        assert_eq!(call(&p1).await.unwrap().content, "realtime answer");
        assert_eq!(count(&s1, "POST", "/v1/messages"), 1);

        // Life 2 (a resume): the same position is served from the row, no HTTP at all.
        let s2 = FakeHttpServer::start(|_| (500, "{}".into())).await;
        let p2 = provider(&s2, ctx(&f, chrono::Duration::milliseconds(1500), BatchFailurePolicy::Error));
        assert_eq!(call(&p2).await.unwrap().content, "realtime answer");
        assert!(s2.requests().is_empty(), "the kept fallback response must be replayed without a model call");
    }

    #[tokio::test]
    async fn a_call_with_no_time_left_goes_straight_to_realtime_without_submitting() {
        let f = fixture().await;
        let (handler, _) = anthropic_batches(0, "");
        let server = FakeHttpServer::start(handler).await;
        let p = provider(&server, ctx(&f, chrono::Duration::seconds(-1), BatchFailurePolicy::RealtimeRetry));
        assert_eq!(call(&p).await.unwrap().content, "realtime answer");
        assert_eq!(count(&server, "POST", "/v1/messages/batches"), 0, "nothing is submitted after the deadline");
        assert_eq!(count(&server, "POST", "/v1/messages"), 1);
    }

    #[tokio::test]
    async fn the_heartbeat_keeps_the_lease_alive_and_signals_when_another_instance_takes_it() {
        let f = fixture().await;
        TEST_POLL_MS.store(5, Ordering::Relaxed);
        let lost = Arc::new(AtomicBool::new(false));
        let _hb = LeaseHeartbeat::start(f.db.clone(), f.execution_id, "me".into(), lost.clone());
        // Let the lease nearly lapse; the heartbeat must extend it.
        kyomi_core::db_execute!(f.db, "UPDATE watch_executions SET lease_expires_at = $1 WHERE id = $2", Utc::now() + chrono::Duration::milliseconds(50), f.execution_id).unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let runs = store::resumable_runs(&f.db, Utc::now(), 10).await.unwrap();
        assert!(runs.is_empty(), "a heartbeating run is never resumable");
        assert!(!lost.load(Ordering::SeqCst));

        // Another instance takes over: the heartbeat reports the loss.
        kyomi_core::db_execute!(f.db, "UPDATE watch_executions SET lease_owner = 'someone-else', lease_expires_at = $1 WHERE id = $2", Utc::now() + chrono::Duration::seconds(120), f.execution_id).unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            while !lost.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the heartbeat never noticed the takeover");
    }
}
