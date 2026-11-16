// SPDX-License-Identifier: AGPL-3.0-or-later

//! End-to-end tests for batch-mode watch runs through the real scheduler and
//! `execute_watch_with` (KYO-973): restart survival, multi-instance claiming,
//! the scheduler not double-starting a run that is waiting on a batch, and the
//! schedule-gap fallback. Only the vendor HTTP API is faked (a loopback server
//! speaking the documented OpenRouter batch shapes); everything else is real.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use kyomi_auth::watch_batch_service as store;
use kyomi_core::watch_llm::WatchLlmMode;
use kyomi_core::{Config, DbPool, WatchExecutionStatus};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::test_support::{FakeHttpServer, RecordedRequest};

const VERDICT: &str = r#"{"should_alert": false, "reason": "all normal"}"#;

/// The fake model's answer to a request: when the conversation does not yet
/// end in a tool result and `tools` is on, call a tool; otherwise give the
/// verdict. This makes a run several model calls long, like a real one.
fn model_answer(request_body: &serde_json::Value, tools: bool) -> serde_json::Value {
    let last_role = request_body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .and_then(|m| m["role"].as_str())
        .unwrap_or_default();
    let message = if tools && last_role != "tool" {
        json!({"content": null, "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "list_datasources", "arguments": "{}"}}]})
    } else {
        json!({"content": VERDICT})
    };
    let finish = if tools && last_role != "tool" { "tool_calls" } else { "stop" };
    json!({"choices": [{"message": message, "finish_reason": finish}], "usage": {"prompt_tokens": 20, "completion_tokens": 9}})
}

/// A fake OpenRouter batch API (documented shapes). Batches are numbered
/// `batch_1`, `batch_2`, ... and stay in progress until allowed to complete.
struct FakeOpenRouter {
    server: FakeHttpServer,
    /// Batches numbered up to this value complete (usize::MAX = all).
    complete_upto: Arc<AtomicUsize>,
    /// Report every batch as failed instead.
    fail: Arc<std::sync::atomic::AtomicBool>,
    realtime_calls: Arc<AtomicUsize>,
}

impl FakeOpenRouter {
    /// `tools`: each run is two model calls (a tool call, then the verdict).
    async fn start(tools: bool) -> Self {
        let complete_upto = Arc::new(AtomicUsize::new(0));
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let submitted: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let realtime_calls = Arc::new(AtomicUsize::new(0));
        let (u, f, sub, r) = (complete_upto.clone(), fail.clone(), submitted.clone(), realtime_calls.clone());
        let server = FakeHttpServer::start(move |req: &RecordedRequest| {
            let path = req.path.trim_start_matches("/openrouter.ai/api/v1");
            match (req.method.as_str(), path) {
                ("POST", "/batches") => {
                    let body = req.json();
                    let r0 = &body["requests"][0];
                    let mut sub = sub.lock().unwrap();
                    sub.push((r0["custom_id"].as_str().unwrap_or_default().to_string(), model_answer(&r0["body"], tools)));
                    (202, json!({"id": format!("batch_{}", sub.len()), "status": "validating"}).to_string())
                }
                ("GET", p) if p.starts_with("/batches/batch_") => {
                    let n: usize = p.trim_start_matches("/batches/batch_").parse().unwrap_or(0);
                    if f.load(Ordering::SeqCst) {
                        return (200, json!({"status": "failed", "error": {"message": "no batch variant for this model"}}).to_string());
                    }
                    if n <= u.load(Ordering::SeqCst) {
                        let (id, answer) = sub.lock().unwrap()[n - 1].clone();
                        (200, json!({"status": "completed", "results": [{"custom_id": id, "response": {"status_code": 200, "body": answer}, "error": null}]}).to_string())
                    } else {
                        (200, json!({"status": "in_progress"}).to_string())
                    }
                }
                ("POST", "/chat/completions") => {
                    r.fetch_add(1, Ordering::SeqCst);
                    (200, model_answer(&req.json(), tools).to_string())
                }
                _ => (404, "{}".into()),
            }
        })
        .await;
        Self { server, complete_upto, fail, realtime_calls }
    }

    fn finish_all(&self) {
        self.complete_upto.store(usize::MAX, Ordering::SeqCst);
    }

    fn base_url(&self) -> String {
        // OpenRouter is recognised by host name; under test a path segment on
        // the loopback server stands in so the real adapter selection runs.
        format!("{}/openrouter.ai/api/v1", self.server.base_url)
    }

    fn posts(&self, path: &str) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|r| r.method == "POST" && r.path.ends_with(path))
            .count()
    }
}

fn config(fake: &FakeOpenRouter, window_minutes: u32) -> Arc<Config> {
    let mut c = Config::test_config();
    c.self_hosted = true;
    c.llm_provider = Some("openai".into());
    c.llm_api_key = Some("k".into());
    c.llm_model = Some("anthropic/claude-haiku-5.5".into());
    c.llm_base_url = Some(fake.base_url());
    c.llm_watch.mode = Some(WatchLlmMode::Batch);
    c.llm_watch.batch_window_minutes = Some(window_minutes);
    Arc::new(c)
}

async fn scheduler(db: &DbPool, config: Arc<Config>) -> Arc<WatchScheduler> {
    crate::batch_run::TEST_POLL_MS.store(5, Ordering::Relaxed);
    Arc::new(WatchScheduler::new(WatchSchedulerDeps {
        db: db.clone(),
        kv: kyomi_core::kv_store_memory::InMemoryKVStore::new_pool(),
        encryption_key: Arc::new([7u8; 32]),
        embedding: crate::test_support::loaded_embedding(),
        ws_manager: kyomi_auth::websocket::WebSocketManager::new(None, db.clone()),
        config,
        connect_registry: None,
        platforms: Arc::new(kyomi_core::platform::PlatformRegistry::new()),
        cancel: CancellationToken::new(),
    }))
}

async fn insert_watch(db: &DbPool, next_run_at: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO watches (watch_id, workspace_id, created_by, name, prompt, schedule, enabled, next_run_at) \
         VALUES ('w1', 'ws-1', 'user-a', 'Revenue', 'check revenue', '0 */6 * * *', 1, $1)",
    )
    .bind(next_run_at)
    .execute(kyomi_test_db::sqlite_pool(db))
    .await
    .unwrap();
}

async fn executions(db: &DbPool) -> Vec<(i32, String, Option<String>, Option<String>)> {
    sqlx::query_as("SELECT id, status, llm_mode, batch_fallback_reason FROM watch_executions ORDER BY id")
        .fetch_all(kyomi_test_db::sqlite_pool(db))
        .await
        .unwrap()
}

async fn until<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    tokio::time::timeout(Duration::from_secs(60), async {
        while !check().await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for: {what}"));
}

async fn status_of(db: &DbPool, id: i32) -> String {
    let r: (String,) = sqlx::query_as("SELECT status FROM watch_executions WHERE id = $1")
        .bind(id)
        .fetch_one(kyomi_test_db::sqlite_pool(db))
        .await
        .unwrap();
    r.0
}

/// Start a scheduled batch run in the background and wait until it is waiting.
async fn start_waiting_run(db: &DbPool, config: Arc<Config>) -> (tokio::task::JoinHandle<()>, i32) {
    let s = scheduler(db, config).await;
    s.spawn_watch_run("w1", RunKind::Scheduled).await;
    let handle = s.active_executions.lock().await.remove("w1").unwrap();
    until("the run to wait on its batch", || async { executions(db).await.iter().any(|e| e.1 == "waiting_batch") }).await;
    (handle, executions(db).await[0].0)
}

#[tokio::test]
async fn a_batch_watch_survives_a_restart_mid_wait_resumes_and_completes_with_one_batch() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    insert_watch(&db, Utc::now() + chrono::Duration::hours(6)).await;
    let fake = FakeOpenRouter::start(false).await;
    let config = config(&fake, 60);

    // First life: the run submits its batch and waits.
    let (first_life, id) = start_waiting_run(&db, config.clone()).await;
    assert_eq!(fake.posts("/batches"), 1);
    assert_eq!(executions(&db).await[0].2.as_deref(), Some("batch"));

    // The process dies mid-wait; its lease is no longer renewed and lapses.
    first_life.abort();
    let _ = first_life.await;
    sqlx::query("UPDATE watch_executions SET lease_expires_at = $1 WHERE id = $2")
        .bind(Utc::now() - chrono::Duration::seconds(1))
        .bind(id)
        .execute(kyomi_test_db::sqlite_pool(&db))
        .await
        .unwrap();
    assert_eq!(status_of(&db, id).await, "waiting_batch", "a restart must not fail the run");

    // The vendor finishes while nothing is running.
    fake.finish_all();

    // Second life: a fresh scheduler finds the run and resumes it.
    let second = scheduler(&db, config).await;
    second.resume_batch_runs().await;
    until("the resumed run to finish", || async { status_of(&db, id).await == "no_alert" }).await;

    assert_eq!(fake.posts("/batches"), 1, "the resumed run must reuse the original batch, not submit another");
    assert_eq!(fake.realtime_calls.load(Ordering::SeqCst), 0, "no realtime call on the happy path");
    let all = executions(&db).await;
    assert_eq!(all.len(), 1, "resuming must not create a second execution record");
    let e = kyomi_auth::watch_service::get_execution_by_id_only(&db, id, "ws-1", "user-a").await.unwrap().unwrap();
    assert_eq!(e.llm_mode.as_deref(), Some("batch"));
    assert_eq!(e.batch_ids.as_deref(), Some(r#"["batch_1"]"#));
    assert_eq!(e.llm_provider.as_deref(), Some("openai"));
    assert!(e.input_tokens > 0 && e.output_tokens > 0, "usage is recorded for the batch run");
}

#[tokio::test]
async fn two_instances_resuming_the_same_run_start_it_exactly_once() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    insert_watch(&db, Utc::now() + chrono::Duration::hours(6)).await;
    let fake = FakeOpenRouter::start(false).await;
    let config = config(&fake, 60);
    let (first_life, id) = start_waiting_run(&db, config.clone()).await;
    first_life.abort();
    let _ = first_life.await;
    sqlx::query("UPDATE watch_executions SET lease_expires_at = $1 WHERE id = $2")
        .bind(Utc::now() - chrono::Duration::seconds(1))
        .bind(id)
        .execute(kyomi_test_db::sqlite_pool(&db))
        .await
        .unwrap();

    // Two instances' scheduler ticks race for the lapsed run.
    let a = scheduler(&db, config.clone()).await;
    let b = scheduler(&db, config.clone()).await;
    tokio::join!(a.resume_batch_runs(), b.resume_batch_runs());
    let started = a.active_executions.lock().await.len() + b.active_executions.lock().await.len();
    assert_eq!(started, 1, "exactly one instance may resume a lapsed run");

    fake.finish_all();
    until("the run to finish", || async { status_of(&db, id).await == "no_alert" }).await;
    assert_eq!(fake.posts("/batches"), 1);
    assert_eq!(executions(&db).await.len(), 1);
}

#[tokio::test]
async fn the_scheduler_does_not_start_the_next_occurrence_or_time_out_a_run_waiting_on_a_batch() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    insert_watch(&db, Utc::now() + chrono::Duration::hours(6)).await;
    let fake = FakeOpenRouter::start(false).await;
    let config = config(&fake, 60);
    let (run, id) = start_waiting_run(&db, config.clone()).await;

    // The watch comes due again (another instance's tick, so this instance's
    // in-memory active map does not know about the waiting run).
    let due = Utc::now() - chrono::Duration::minutes(1);
    sqlx::query("UPDATE watches SET next_run_at = $1 WHERE watch_id = 'w1'")
        .bind(due)
        .execute(kyomi_test_db::sqlite_pool(&db))
        .await
        .unwrap();
    let other_instance = scheduler(&db, config).await;
    other_instance.poll_due_watches().await.unwrap();
    assert_eq!(executions(&db).await.len(), 1, "no second execution while one waits on a batch");
    assert!(other_instance.active_executions.lock().await.is_empty(), "nothing was spawned");
    let next: (DateTime<Utc>,) = sqlx::query_as("SELECT next_run_at FROM watches WHERE watch_id = 'w1'")
        .fetch_one(kyomi_test_db::sqlite_pool(&db))
        .await
        .unwrap();
    assert_eq!(next.0, due, "the occurrence stays due; it is not advanced or lost");

    // Orphan recovery must not time out a batch run, however long it waits.
    sqlx::query("UPDATE watch_executions SET started_at = $1 WHERE id = $2")
        .bind(Utc::now() - chrono::Duration::hours(5))
        .bind(id)
        .execute(kyomi_test_db::sqlite_pool(&db))
        .await
        .unwrap();
    other_instance.recover_orphaned_executions().await;
    assert_eq!(status_of(&db, id).await, "waiting_batch");

    run.abort();
}

#[tokio::test]
async fn shutdown_hands_the_lease_back_instead_of_failing_the_batch_run() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    insert_watch(&db, Utc::now() + chrono::Duration::hours(6)).await;
    let fake = FakeOpenRouter::start(false).await;
    let config = config(&fake, 60);
    let s = scheduler(&db, config).await;
    s.spawn_watch_run("w1", RunKind::Scheduled).await;
    until("the run to wait", || async { executions(&db).await.iter().any(|e| e.1 == "waiting_batch") }).await;
    let id = executions(&db).await[0].0;

    s.shutdown().await;

    assert_eq!(status_of(&db, id).await, "waiting_batch", "shutdown must not mark a batch run as failed");
    let runs = store::resumable_runs(&db, Utc::now() + chrono::Duration::seconds(1), 10).await.unwrap();
    assert_eq!(runs.iter().map(|r| r.id).collect::<Vec<_>>(), vec![id], "the released run is immediately resumable");
}

#[tokio::test]
async fn a_schedule_gap_shorter_than_the_window_runs_that_occurrence_realtime() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    insert_watch(&db, Utc::now() + chrono::Duration::minutes(30)).await;
    let fake = FakeOpenRouter::start(false).await;
    let s = scheduler(&db, config(&fake, 60)).await;
    s.spawn_watch_run("w1", RunKind::Scheduled).await;
    let handle = s.active_executions.lock().await.remove("w1").unwrap();
    handle.await.unwrap();

    let all = executions(&db).await;
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].1, "no_alert");
    assert_eq!(all[0].2.as_deref(), Some("realtime"));
    assert!(all[0].3.as_deref().unwrap().contains("shorter than the 60m batch window"), "{:?}", all[0].3);
    assert_eq!(fake.posts("/batches"), 0, "no batch is submitted when the gap is too short");
    assert_eq!(fake.realtime_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_manual_run_now_is_always_realtime_even_in_batch_mode() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    insert_watch(&db, Utc::now() + chrono::Duration::hours(6)).await;
    let fake = FakeOpenRouter::start(false).await;
    let s = scheduler(&db, config(&fake, 60)).await;
    s.spawn_watch_run("w1", RunKind::Manual).await;
    let handle = s.active_executions.lock().await.remove("w1").unwrap();
    handle.await.unwrap();

    let all = executions(&db).await;
    assert_eq!(all[0].2.as_deref(), Some("realtime"));
    assert_eq!(fake.posts("/batches"), 0);
    assert_eq!(fake.realtime_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_batch_still_pending_when_the_next_run_is_due_is_abandoned_and_the_run_finishes_realtime() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    // The gap equals the window, so the run batches, with a short deadline.
    insert_watch(&db, Utc::now() + chrono::Duration::minutes(65)).await;
    let fake = FakeOpenRouter::start(false).await;
    let (run, id) = start_waiting_run(&db, config(&fake, 60)).await;

    // Time passes: the stored deadline is now in the past.
    sqlx::query("UPDATE watch_executions SET batch_deadline_at = $1 WHERE id = $2")
        .bind(Utc::now() - chrono::Duration::seconds(1))
        .bind(id)
        .execute(kyomi_test_db::sqlite_pool(&db))
        .await
        .unwrap();
    // The in-memory run already captured its deadline; resume on a fresh
    // scheduler after the old driver is gone, as a restart past the deadline.
    run.abort();
    let _ = run.await;
    sqlx::query("UPDATE watch_executions SET lease_expires_at = $1 WHERE id = $2")
        .bind(Utc::now() - chrono::Duration::seconds(1))
        .bind(id)
        .execute(kyomi_test_db::sqlite_pool(&db))
        .await
        .unwrap();
    // The first (never-finished) batch call is reused by hash; with the deadline
    // passed the resumed run stops waiting, abandons it and goes realtime.
    let s = scheduler(&db, config(&fake, 60)).await;
    s.resume_batch_runs().await;
    until("the run to finish realtime", || async { status_of(&db, id).await == "no_alert" }).await;

    assert_eq!(fake.posts("/batches"), 1, "no second batch");
    assert!(fake.realtime_calls.load(Ordering::SeqCst) >= 1, "the run completed in realtime");
    let e = kyomi_auth::watch_service::get_execution_by_id_only(&db, id, "ws-1", "user-a").await.unwrap().unwrap();
    assert!(e.batch_fallback_reason.unwrap().contains("next scheduled run"));
    assert_eq!(e.status, WatchExecutionStatus::NoAlert);
}

// -- Multi-call runs, failure policy, displaced driver -----------------------

fn config_with(fake: &FakeOpenRouter, window_minutes: u32, failure: kyomi_core::watch_llm::BatchFailurePolicy) -> Arc<Config> {
    let mut c = (*config(fake, window_minutes)).clone();
    c.llm_watch.batch_failure = Some(failure);
    Arc::new(c)
}

/// Simulate the process dying: stop the driver and let its lease lapse.
async fn crash(db: &DbPool, handle: tokio::task::JoinHandle<()>, id: i32) {
    handle.abort();
    let _ = handle.await;
    sqlx::query("UPDATE watch_executions SET lease_expires_at = $1 WHERE id = $2")
        .bind(Utc::now() - chrono::Duration::seconds(1))
        .bind(id)
        .execute(kyomi_test_db::sqlite_pool(db))
        .await
        .unwrap();
}

/// Resume with a fresh scheduler and return the new driver.
async fn resume(db: &DbPool, config: Arc<Config>) -> (Arc<WatchScheduler>, tokio::task::JoinHandle<()>) {
    let s = scheduler(db, config).await;
    s.resume_batch_runs().await;
    let h = s.active_executions.lock().await.remove("w1").expect("the run was resumed");
    (s, h)
}

#[tokio::test]
async fn a_tool_using_batch_run_survives_two_restarts_and_never_resubmits_a_finished_call() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    insert_watch(&db, Utc::now() + chrono::Duration::hours(6)).await;
    let fake = FakeOpenRouter::start(true).await;
    let config = config(&fake, 60);

    // Life 1: call 0 (the tool call) is submitted and pending.
    let (life1, id) = start_waiting_run(&db, config.clone()).await;
    assert_eq!(fake.posts("/batches"), 1);
    crash(&db, life1, id).await;

    // Life 2: call 0 has finished meanwhile; the replay re-runs the tool and
    // submits call 1, which stays pending.
    fake.complete_upto.store(1, Ordering::SeqCst);
    let (_s2, life2) = resume(&db, config.clone()).await;
    until("call 1 to be submitted", || async { fake.posts("/batches") == 2 }).await;
    until("the run to wait again", || async { status_of(&db, id).await == "waiting_batch" }).await;
    life2.abort();
    let _ = life2.await;
    crash(&db, tokio::spawn(async {}), id).await;

    // Life 3: everything finishes. Call 0 is answered from the cache, call 1
    // from its batch.
    fake.finish_all();
    let (_s3, life3) = resume(&db, config).await;
    life3.await.unwrap();

    assert_eq!(status_of(&db, id).await, "no_alert");
    assert_eq!(fake.posts("/batches"), 2, "two model calls, two batches, across three process lives");
    assert_eq!(fake.realtime_calls.load(Ordering::SeqCst), 0);
    assert_eq!(executions(&db).await.len(), 1);
    let e = kyomi_auth::watch_service::get_execution_by_id_only(&db, id, "ws-1", "user-a").await.unwrap().unwrap();
    assert_eq!(e.batch_ids.as_deref(), Some(r#"["batch_1","batch_2"]"#));
    let sessions: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM chat_sessions WHERE session_type = 'watch_execution'")
        .fetch_one(kyomi_test_db::sqlite_pool(&db))
        .await
        .unwrap();
    assert_eq!(sessions.0, 1, "each resumed attempt removes the interrupted attempt's session");
}

#[tokio::test]
async fn under_the_error_policy_a_failed_batch_ends_the_run_as_an_error_not_a_quiet_no_alert() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    insert_watch(&db, Utc::now() + chrono::Duration::hours(6)).await;
    let fake = FakeOpenRouter::start(false).await;
    fake.fail.store(true, Ordering::SeqCst);
    let s = scheduler(&db, config_with(&fake, 60, kyomi_core::watch_llm::BatchFailurePolicy::Error)).await;
    s.spawn_watch_run("w1", RunKind::Scheduled).await;
    s.active_executions.lock().await.remove("w1").unwrap().await.unwrap();

    let all = executions(&db).await;
    assert_eq!(all[0].1, "error", "the failure policy must fail the run through the watch error path");
    let e = kyomi_auth::watch_service::get_execution_by_id_only(&db, all[0].0, "ws-1", "user-a").await.unwrap().unwrap();
    assert!(e.error_message.unwrap().contains("no batch variant"));
    assert_eq!(fake.posts("/batches"), 1, "no JSON-correction retry submits further batches");
    assert_eq!(fake.realtime_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn under_the_default_policy_a_failed_batch_is_retried_once_in_realtime_and_the_run_completes() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    insert_watch(&db, Utc::now() + chrono::Duration::hours(6)).await;
    let fake = FakeOpenRouter::start(false).await;
    fake.fail.store(true, Ordering::SeqCst);
    let s = scheduler(&db, config(&fake, 60)).await;
    s.spawn_watch_run("w1", RunKind::Scheduled).await;
    s.active_executions.lock().await.remove("w1").unwrap().await.unwrap();

    let all = executions(&db).await;
    assert_eq!(all[0].1, "no_alert");
    assert!(all[0].3.as_deref().unwrap().contains("no batch variant"), "{:?}", all[0].3);
    assert_eq!(fake.realtime_calls.load(Ordering::SeqCst), 1, "exactly one realtime retry");
}

#[tokio::test]
async fn a_displaced_driver_stops_without_writing_a_result_or_calling_the_model() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    insert_watch(&db, Utc::now() + chrono::Duration::hours(6)).await;
    let fake = FakeOpenRouter::start(false).await;
    let (run, id) = start_waiting_run(&db, config(&fake, 60)).await;

    // Another instance takes the run (its lease lapsed from this one's view).
    sqlx::query("UPDATE watch_executions SET lease_owner = 'another-instance', lease_expires_at = $1 WHERE id = $2")
        .bind(Utc::now() + chrono::Duration::seconds(120))
        .bind(id)
        .execute(kyomi_test_db::sqlite_pool(&db))
        .await
        .unwrap();
    // The vendor even finishes: the displaced driver must still not deliver.
    fake.finish_all();
    tokio::time::timeout(Duration::from_secs(20), run).await.expect("the displaced driver never stopped").unwrap();

    let e = kyomi_auth::watch_service::get_execution_by_id_only(&db, id, "ws-1", "user-a").await.unwrap().unwrap();
    assert!(e.completed_at.is_none(), "a displaced driver must not complete the run");
    assert_ne!(e.status, WatchExecutionStatus::Error, "nor fail it");
    assert_ne!(e.status, WatchExecutionStatus::NoAlert);
    assert_eq!(fake.realtime_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_tool_using_run_that_hits_its_deadline_finishes_its_realtime_tail() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    insert_watch(&db, Utc::now() + chrono::Duration::minutes(65)).await;
    let fake = FakeOpenRouter::start(true).await;
    let (run, id) = start_waiting_run(&db, config(&fake, 60)).await;

    // The deadline passes while call 0 is pending; the driver dies and the run
    // is resumed after it. Call 0 falls back to realtime, which asks for a
    // tool, and call 1 must still have time to run and deliver the verdict.
    sqlx::query("UPDATE watch_executions SET batch_deadline_at = $1 WHERE id = $2")
        .bind(Utc::now() - chrono::Duration::seconds(1))
        .bind(id)
        .execute(kyomi_test_db::sqlite_pool(&db))
        .await
        .unwrap();
    crash(&db, run, id).await;
    let (_s, driver) = resume(&db, config(&fake, 60)).await;
    driver.await.unwrap();

    assert_eq!(status_of(&db, id).await, "no_alert", "the realtime tail must complete, not be cut off");
    assert_eq!(fake.posts("/batches"), 1, "no batch after the deadline");
    assert_eq!(fake.realtime_calls.load(Ordering::SeqCst), 2, "the tool call and the verdict ran in realtime");
}
