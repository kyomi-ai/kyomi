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

/// A fake OpenRouter batch API. The batch stays in progress until `finish` is
/// set, then completes with the request's own `custom_id`.
struct FakeOpenRouter {
    server: FakeHttpServer,
    finish: Arc<std::sync::atomic::AtomicBool>,
    realtime_calls: Arc<AtomicUsize>,
}

impl FakeOpenRouter {
    async fn start() -> Self {
        let finish = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let custom_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let realtime_calls = Arc::new(AtomicUsize::new(0));
        let (f, c, r) = (finish.clone(), custom_id.clone(), realtime_calls.clone());
        let completion = json!({"choices": [{"message": {"content": VERDICT}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 20, "completion_tokens": 9}});
        let server = FakeHttpServer::start(move |req: &RecordedRequest| {
            let path = req.path.trim_start_matches("/openrouter.ai/api/v1");
            match (req.method.as_str(), path) {
                ("POST", "/batches") => {
                    *c.lock().unwrap() = req.json()["requests"][0]["custom_id"].as_str().map(str::to_string);
                    (202, json!({"id": "batch_1", "status": "validating"}).to_string())
                }
                ("GET", "/batches/batch_1") => {
                    if f.load(Ordering::SeqCst) {
                        let id = c.lock().unwrap().clone().unwrap_or_default();
                        (200, json!({"status": "completed", "results": [{"custom_id": id, "response": {"status_code": 200, "body": completion.clone()}, "error": null}]}).to_string())
                    } else {
                        (200, json!({"status": "in_progress"}).to_string())
                    }
                }
                ("POST", "/chat/completions") => {
                    r.fetch_add(1, Ordering::SeqCst);
                    (200, completion.to_string())
                }
                _ => (404, "{}".into()),
            }
        })
        .await;
        Self { server, finish, realtime_calls }
    }

    fn base_url(&self) -> String {
        // OpenRouter is recognised by host name; carry it in the path so the
        // real adapter selection is exercised against a loopback server.
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
    let fake = FakeOpenRouter::start().await;
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
    fake.finish.store(true, Ordering::SeqCst);

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
    let fake = FakeOpenRouter::start().await;
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

    fake.finish.store(true, Ordering::SeqCst);
    until("the run to finish", || async { status_of(&db, id).await == "no_alert" }).await;
    assert_eq!(fake.posts("/batches"), 1);
    assert_eq!(executions(&db).await.len(), 1);
}

#[tokio::test]
async fn the_scheduler_does_not_start_the_next_occurrence_or_time_out_a_run_waiting_on_a_batch() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    insert_watch(&db, Utc::now() + chrono::Duration::hours(6)).await;
    let fake = FakeOpenRouter::start().await;
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
    let fake = FakeOpenRouter::start().await;
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
    let fake = FakeOpenRouter::start().await;
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
    let fake = FakeOpenRouter::start().await;
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
    let fake = FakeOpenRouter::start().await;
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
