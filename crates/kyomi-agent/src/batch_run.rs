// SPDX-License-Identifier: AGPL-3.0-or-later

//! The batch-mode LLM provider for a watch run (KYO-973).
//!
//! [`BatchingProvider`] wraps the run's realtime provider. Each `complete()`
//! call becomes: build the exact realtime request body, look it up in the
//! run's durable call cache, and either return the cached response, re-poll a
//! batch already submitted for it, or submit a new one and wait.
//!
//! Because every call is keyed by the hash of its request body, a run that is
//! resumed after a restart simply replays from the start: calls it already
//! made are answered from the cache and nothing is resubmitted; the first call
//! it had not finished is re-polled by its vendor batch id.
//!
//! The wait is a lightweight poll loop that renews the run's lease. If the
//! lease is lost to another instance this instance stops quietly
//! ([`BatchRunContext::lease_lost`]); the record belongs to the new owner.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kyomi_auth::watch_batch_service::{self as store, BatchCallState};
use kyomi_core::watch_llm::BatchFailurePolicy;
use kyomi_core::DbPool;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::batch::BatchPoll;
use crate::provider::{LLMProvider, RequestOptions};
use crate::types::{LLMResponse, Message, Tool};

/// How often a waiting run polls the vendor (both vendors suggest ~1/minute).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// How long a lease lasts without renewal. Renewed every poll.
pub const LEASE_TTL_SECONDS: i64 = 180;

/// Process-wide instance identity used as the lease owner.
pub fn instance_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| format!("{}:{}", std::process::id(), uuid::Uuid::new_v4()))
}

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
}

pub struct BatchingProvider {
    inner: Box<dyn LLMProvider>,
    provider_name: String,
    ctx: BatchRunContext,
    /// Once the run stops batching (deadline), every later call is realtime.
    realtime_only: AtomicBool,
}

impl BatchingProvider {
    pub fn new(inner: Box<dyn LLMProvider>, provider_name: &str, ctx: BatchRunContext) -> Self {
        Self {
            inner,
            provider_name: provider_name.to_string(),
            ctx,
            realtime_only: AtomicBool::new(false),
        }
    }

    fn lease_expiry() -> DateTime<Utc> {
        Utc::now() + chrono::Duration::seconds(LEASE_TTL_SECONDS)
    }

    /// Renew the lease; `Err` when another instance owns the run now.
    async fn hold_lease(&self) -> kyomi_core::Result<()> {
        let held = store::claim_lease(
            &self.ctx.db,
            self.ctx.execution_id,
            &self.ctx.owner,
            Self::lease_expiry(),
            Utc::now(),
        )
        .await?;
        if held {
            Ok(())
        } else {
            self.ctx.lease_lost.store(true, Ordering::SeqCst);
            Err(kyomi_core::Error::Internal(
                "watch run lease was taken by another instance".into(),
            ))
        }
    }

    /// Apply the failure policy to a batch that did not produce a response.
    async fn on_batch_failure(
        &self,
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
                self.inner.complete(messages, tools, options, user_names).await
            }
            BatchFailurePolicy::Error => Err(kyomi_core::Error::Internal(format!(
                "watch batch failed: {reason}"
            ))),
        }
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
        if self.realtime_only.load(Ordering::SeqCst) {
            return self.inner.complete(messages, tools, options, user_names).await;
        }
        let batch = self.inner.batch().ok_or_else(|| {
            kyomi_core::Error::Internal(format!(
                "batch mode is not supported by the {} provider; use realtime",
                self.provider_name
            ))
        })?;
        let body = batch.build_body(messages, tools, options, user_names);
        let hash = request_hash(&body);
        let call = store::begin_call(
            &self.ctx.db,
            &self.ctx.encryption_key,
            self.ctx.execution_id,
            &hash,
            &self.provider_name,
            self.inner.model(),
        )
        .await?;

        // Answered by an earlier attempt of this run: no network, no cost.
        match call.state {
            BatchCallState::Succeeded => {
                let response = call.response.as_ref().ok_or_else(|| {
                    kyomi_core::Error::Internal("succeeded batch call has no stored response".into())
                })?;
                return batch.parse_response(response);
            }
            BatchCallState::Failed | BatchCallState::Expired | BatchCallState::Abandoned => {
                let reason = call
                    .error_message
                    .clone()
                    .unwrap_or_else(|| format!("earlier batch call ended {}", call.state.as_str()));
                return self.on_batch_failure(reason, messages, tools, options, user_names).await;
            }
            BatchCallState::Submitting | BatchCallState::Pending => {}
        }

        self.hold_lease().await?;
        let vendor_id = match &call.vendor_batch_id {
            Some(id) => id.clone(),
            None => {
                let id = batch.submit(&call.call_id, &body).await;
                let id = match id {
                    Ok(id) => id,
                    Err(e) => {
                        // Never submitted: no vendor batch to wait for.
                        store::finish_call(
                            &self.ctx.db,
                            &self.ctx.encryption_key,
                            &call.call_id,
                            BatchCallState::Failed,
                            None,
                            Some(&e.to_string()),
                        )
                        .await?;
                        return self
                            .on_batch_failure(format!("batch submit failed: {e}"), messages, tools, options, user_names)
                            .await;
                    }
                };
                store::mark_submitted(&self.ctx.db, &call.call_id, &id).await?;
                store::sync_batch_ids(&self.ctx.db, self.ctx.execution_id).await?;
                info!(execution_id = self.ctx.execution_id, batch_id = %id, "submitted watch batch");
                id
            }
        };

        store::set_waiting(&self.ctx.db, self.ctx.execution_id, true).await?;
        let outcome = loop {
            if Utc::now() >= self.ctx.deadline {
                break None;
            }
            self.hold_lease().await?;
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
        store::set_waiting(&self.ctx.db, self.ctx.execution_id, false).await?;

        match outcome {
            Some(BatchPoll::Succeeded(response)) => {
                store::finish_call(&self.ctx.db, &self.ctx.encryption_key, &call.call_id, BatchCallState::Succeeded, Some(&response), None).await?;
                batch.parse_response(&response)
            }
            Some(BatchPoll::Failed(reason)) => {
                store::finish_call(&self.ctx.db, &self.ctx.encryption_key, &call.call_id, BatchCallState::Failed, None, Some(&reason)).await?;
                self.on_batch_failure(reason, messages, tools, options, user_names).await
            }
            Some(BatchPoll::Expired) => {
                let reason = "the provider batch expired before completing".to_string();
                store::finish_call(&self.ctx.db, &self.ctx.encryption_key, &call.call_id, BatchCallState::Expired, None, Some(&reason)).await?;
                self.on_batch_failure(reason, messages, tools, options, user_names).await
            }
            Some(BatchPoll::Pending) => unreachable!("pending never breaks the poll loop"),
            // Deadline: the next scheduled run is too close to keep waiting.
            // Abandon the batch (a late result is ignored), and finish this
            // run, and every remaining call of it, in realtime so a slow
            // batch can never delay or drop an alert.
            None => {
                let reason = "batch not finished before the next scheduled run was due; \
                              cancelled and re-run in realtime"
                    .to_string();
                store::finish_call(&self.ctx.db, &self.ctx.encryption_key, &call.call_id, BatchCallState::Abandoned, None, Some(&reason)).await?;
                if let Err(e) = batch.cancel(&vendor_id).await {
                    warn!(batch_id = %vendor_id, error = %e, "failed to cancel the abandoned batch");
                }
                store::note_fallback(&self.ctx.db, self.ctx.execution_id, &reason).await?;
                self.realtime_only.store(true, Ordering::SeqCst);
                self.inner.complete(messages, tools, options, user_names).await
            }
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
            let c = store::begin_call(&f.db, &KEY, f.execution_id, &request_hash(&b), "anthropic", "claude-x").await.unwrap();
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
        let c = store::begin_call(&f.db, &KEY, f.execution_id, &request_hash(&body), "anthropic", "claude-x").await.unwrap();
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
        let p = provider(&server, ctx(&f, chrono::Duration::milliseconds(60), BatchFailurePolicy::Error));
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
}
