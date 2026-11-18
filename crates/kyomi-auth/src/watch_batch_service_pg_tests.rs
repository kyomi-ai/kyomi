// SPDX-License-Identifier: AGPL-3.0-or-later

//! The batch-run SQL (call cache, lease CAS, resume, reaping, one-transaction
//! billing, "Run now") executed against a **real Postgres** (KYO-973). The
//! sibling SQLite tests prove the logic; these prove the production database
//! accepts the statements, types and transaction semantics. Skipped when no
//! Postgres is reachable (set `KYOMI_REQUIRE_POSTGRES_TESTS=1` to make that an
//! error, as CI does).

use chrono::{Duration, Utc};
use kyomi_core::DbPool;

use super::*;
use crate::test_pg::{cleanup_workspace_and_users_pg, postgres_pool, postgres_test_pool_or_skip, seed_user_pg, seed_workspace_pg, unique_test_id};

const KEY: [u8; 32] = [5u8; 32];

struct Fx {
    db: DbPool,
    ws: String,
    user: String,
    watch: String,
}

async fn fixture(name: &str) -> Option<Fx> {
    let db = postgres_test_pool_or_skip(name).await?;
    let (ws, user, watch) = (unique_test_id("bw"), unique_test_id("bu"), unique_test_id("bh"));
    let pg = postgres_pool(&db);
    seed_user_pg(pg, &user, &format!("{user}@example.com")).await;
    seed_workspace_pg(pg, &ws, &user).await;
    sqlx::query("INSERT INTO watches (watch_id, workspace_id, created_by, name, prompt, schedule, mode) VALUES ($1, $2, $3, 'W', 'p', '0 9 * * *', 'alert')")
        .bind(&watch).bind(&ws).bind(&user).execute(pg).await.expect("seed watch");
    Some(Fx { db, ws, user, watch })
}

impl Fx {
    async fn run(&self, owner: &str, lease_for: Duration) -> i32 {
        let e = crate::watch_service::create_execution(&self.db, &self.watch, "W", &self.ws, kyomi_core::WatchMode::Alert, &self.user).await.unwrap();
        let now = Utc::now();
        record_run_llm(&self.db, e.id, &RunLlm { provider: "anthropic", model: "m", mode: "batch", deadline: Some(now + Duration::hours(3)), fallback_reason: None, lease: Some((owner, now + lease_for)), workspace_billed: false }).await.unwrap();
        e.id
    }

    async fn cleanup(&self) {
        let pg = postgres_pool(&self.db);
        let _ = sqlx::query("DELETE FROM api_usage_log WHERE workspace_id = $1").bind(&self.ws).execute(pg).await;
        let _ = sqlx::query("DELETE FROM watch_executions WHERE workspace_id = $1").bind(&self.ws).execute(pg).await;
        let _ = sqlx::query("DELETE FROM watches WHERE workspace_id = $1").bind(&self.ws).execute(pg).await;
        cleanup_workspace_and_users_pg(pg, &self.ws, &[&self.user]).await;
    }
}

/// Stands in for the usage ledger (the agent crate's): sums `{"in", "out"}`
/// responses into a row, so these tests exercise the claim and transaction.
fn tokens_row(calls: &[StoredResponse]) -> kyomi_core::Result<Option<kyomi_core::models::ApiUsageInsert>> {
    let sum = |key: &str| calls.iter().map(|c| c.response[key].as_i64().unwrap_or(0)).sum::<i64>() as i32;
    let (input, output) = (sum("in"), sum("out"));
    if input == 0 && output == 0 {
        return Ok(None);
    }
    Ok(Some(kyomi_core::models::ApiUsageInsert {
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

#[tokio::test]
async fn postgres_call_cache_lease_resume_and_reap() {
    let Some(f) = fixture("postgres_call_cache_lease_resume_and_reap").await else { return };
    let id = f.run("dead-instance", Duration::seconds(-5)).await;

    // Call cache: ON CONFLICT idempotency per position, result + usage in one write, encrypted at rest.
    let a = begin_call(&f.db, &KEY, id, 0, "anthropic", "m").await.unwrap();
    let again = begin_call(&f.db, &KEY, id, 0, "anthropic", "m").await.unwrap();
    assert_eq!(a.call_id, again.call_id);
    mark_submitted(&f.db, &a.call_id, "msgbatch_1").await.unwrap();
    let body = serde_json::json!({"content": [{"text": "secret-finding"}]});
    assert!(finish_call(&f.db, &KEY, &a.call_id, BatchCallState::Succeeded, Some(&body), None).await.unwrap());
    assert!(!finish_call(&f.db, &KEY, &a.call_id, BatchCallState::Abandoned, None, None).await.unwrap(), "a final call is not overwritten");
    assert_eq!(find_call(&f.db, &KEY, id, 0).await.unwrap().unwrap().response, Some(body));
    let b = begin_call(&f.db, &KEY, id, 1, "anthropic", "m").await.unwrap();
    finish_call(&f.db, &KEY, &b.call_id, BatchCallState::Abandoned, None, Some("deadline")).await.unwrap();
    assert!(attach_fallback_response(&f.db, &KEY, &b.call_id, &serde_json::json!({"x": 1})).await.unwrap());
    assert!(!attach_fallback_response(&f.db, &KEY, &b.call_id, &serde_json::json!({"x": 2})).await.unwrap());
    sync_batch_ids(&f.db, id).await.unwrap();
    assert_eq!(batch_ids(&f.db, id).await.unwrap(), vec!["msgbatch_1"]);

    // Lease CAS: a lapsed lease is resumable and exactly one of many concurrent takers wins.
    assert!(resumable_runs(&f.db, Utc::now(), 100).await.unwrap().iter().any(|r| r.id == id));
    let mut tasks = Vec::new();
    for n in 0..8 {
        let db = f.db.clone();
        tasks.push(tokio::spawn(async move {
            let now = Utc::now();
            take_over_lease(&db, id, &format!("instance-{n}"), now + Duration::seconds(90), now).await.unwrap()
        }));
    }
    let mut winners = 0;
    for t in tasks {
        winners += usize::from(t.await.unwrap());
    }
    assert_eq!(winners, 1, "exactly one instance may take over a lapsed lease on Postgres");
    assert!(!resumable_runs(&f.db, Utc::now(), 100).await.unwrap().iter().any(|r| r.id == id));

    // Run facts and scheduler-facing queries.
    assert_eq!(run_provider(&f.db, id).await.unwrap().as_deref(), Some("anthropic"));
    assert!(has_unfinished_batch_run(&f.db, &f.watch).await.unwrap());
    assert_eq!(unfinished_run(&f.db, id).await.unwrap().unwrap().id, id);
    assert!(my_batch_watch_ids(&f.db, "instance-0").await.unwrap().len() + my_batch_watch_ids(&f.db, "instance-1").await.unwrap().len() + my_batch_watch_ids(&f.db, "instance-2").await.unwrap().len() + my_batch_watch_ids(&f.db, "instance-3").await.unwrap().len() + my_batch_watch_ids(&f.db, "instance-4").await.unwrap().len() + my_batch_watch_ids(&f.db, "instance-5").await.unwrap().len() + my_batch_watch_ids(&f.db, "instance-6").await.unwrap().len() + my_batch_watch_ids(&f.db, "instance-7").await.unwrap().len() >= 1);
    note_fallback(&f.db, id, "first").await.unwrap();
    note_fallback(&f.db, id, "second").await.unwrap();
    assert_eq!(release_all_leases(&f.db, "nobody", Utc::now()).await.unwrap(), 0);

    // Status flips, lease release.
    set_waiting(&f.db, id, true).await.unwrap();
    assert!(crate::watch_service::get_execution_by_id_only(&f.db, id, &f.ws, &f.user).await.unwrap().unwrap().status == kyomi_core::WatchExecutionStatus::WaitingBatch);
    let (ok, _) = crate::watch_service::can_run_watch_now(&f.db, &f.watch, &f.ws, &f.user).await.unwrap();
    assert!(!ok, "Run now is refused while a batch run waits");
    let reaped_none = reap_overdue_runs(&f.db, Utc::now(), Duration::minutes(40)).await.unwrap();
    assert!(!reaped_none.iter().any(|r| r.watch_id.as_deref() == Some(f.watch.as_str())));
    sqlx::query("UPDATE watch_executions SET batch_deadline_at = $1 WHERE id = $2").bind(Utc::now() - Duration::minutes(90)).bind(id).execute(postgres_pool(&f.db)).await.unwrap();
    let reaped = reap_overdue_runs(&f.db, Utc::now(), Duration::minutes(40)).await.unwrap();
    assert!(reaped.iter().any(|r| r.watch_id.as_deref() == Some(f.watch.as_str())));
    let e = crate::watch_service::get_execution_by_id_only(&f.db, id, &f.ws, &f.user).await.unwrap().unwrap();
    assert_eq!(e.status, kyomi_core::WatchExecutionStatus::Error);
    // The reaped run is billed for what it paid, once, via the StoppedRun fence.
    let bill = UsageBill { encryption_key: &KEY, user_id: &f.user, workspace_id: &f.ws, session_id: "s" };
    assert!(bill_run_once(&f.db, id, BillingFence::StoppedRun, Utc::now(), &bill, tokens_row).await.unwrap());
    assert!(!bill_run_once(&f.db, id, BillingFence::StoppedRun, Utc::now(), &bill, tokens_row).await.unwrap());
    f.cleanup().await;
}

#[tokio::test]
async fn postgres_billing_is_one_atomic_lease_fenced_transaction() {
    let Some(f) = fixture("postgres_billing_is_one_atomic_lease_fenced_transaction").await else { return };
    let id = f.run("me", Duration::seconds(120)).await;
    let c = begin_call(&f.db, &KEY, id, 0, "anthropic", "m").await.unwrap();
    finish_call(&f.db, &KEY, &c.call_id, BatchCallState::Succeeded, Some(&serde_json::json!({"in": 7, "out": 3})), None).await.unwrap();

    let bill = UsageBill { encryption_key: &KEY, user_id: &f.user, workspace_id: &f.ws, session_id: "s" };
    let count = |db: &DbPool, ws: &str| { let (db, ws) = (db.clone(), ws.to_string()); async move { sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM api_usage_log WHERE workspace_id = $1").bind(ws).fetch_one(postgres_pool(&db)).await.unwrap().0 } };

    assert!(!bill_run_once(&f.db, id, BillingFence::LeaseOwner("someone-else"), Utc::now(), &bill, tokens_row).await.unwrap());
    // A failed write rolls the claim back on Postgres (the whole transaction aborts).
    let failing = |_: &[StoredResponse]| Err(kyomi_core::Error::Internal("cannot price".into()));
    assert!(bill_run_once(&f.db, id, BillingFence::LeaseOwner("me"), Utc::now(), &bill, failing).await.is_err());
    assert_eq!(count(&f.db, &f.ws).await, 0);
    // Concurrent billers: exactly one row, one winner.
    let mut tasks = Vec::new();
    for _ in 0..6 {
        let (db, user, ws) = (f.db.clone(), f.user.clone(), f.ws.clone());
        tasks.push(tokio::spawn(async move {
            let bill = UsageBill { encryption_key: &KEY, user_id: &user, workspace_id: &ws, session_id: "s" };
            bill_run_once(&db, id, BillingFence::LeaseOwner("me"), Utc::now(), &bill, tokens_row).await.unwrap()
        }));
    }
    let mut winners = 0;
    for t in tasks {
        winners += usize::from(t.await.unwrap());
    }
    assert_eq!(winners, 1);
    assert_eq!(count(&f.db, &f.ws).await, 1, "one usage row for the run on Postgres");
    let (tin, tout): (i32, i32) = sqlx::query_as("SELECT input_tokens, output_tokens FROM api_usage_log WHERE workspace_id = $1").bind(&f.ws).fetch_one(postgres_pool(&f.db)).await.unwrap();
    assert_eq!((tin, tout), (7, 3));
    f.cleanup().await;
}
