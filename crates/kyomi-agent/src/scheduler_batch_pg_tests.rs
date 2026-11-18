// SPDX-License-Identifier: AGPL-3.0-or-later

//! The scheduler's raw batch-run SQL on a **real Postgres** (KYO-973): orphan
//! recovery's batch exemption and shutdown's lease handover. The SQLite e2e
//! tests in `scheduler_batch_tests.rs` prove the behaviour; these prove
//! Postgres accepts the statements. Skipped when no Postgres is reachable
//! (required in CI via `KYOMI_REQUIRE_POSTGRES_TESTS=1`).

use std::sync::Arc;

use chrono::{Duration, Utc};
use kyomi_auth::watch_batch_service as store;
use kyomi_core::{Config, DbPool};
use tokio_util::sync::CancellationToken;

use super::*;

async fn pg_or_skip(name: &str) -> Option<DbPool> {
    match kyomi_core::test_db::connect_test_pool().await {
        Ok(db) if db.is_postgres() => Some(db),
        other => {
            if std::env::var("KYOMI_REQUIRE_POSTGRES_TESTS").as_deref() == Ok("1") {
                panic!("{name}: KYOMI_REQUIRE_POSTGRES_TESTS=1 but Postgres is unavailable ({:?})", other.err());
            }
            eprintln!("SKIP: {name} — Postgres unavailable");
            None
        }
    }
}

fn pg(db: &DbPool) -> &sqlx::PgPool {
    match db {
        DbPool::Postgres(p) => p,
        DbPool::Sqlite(_) => unreachable!("checked by pg_or_skip"),
    }
}

async fn scheduler(db: &DbPool) -> Arc<WatchScheduler> {
    Arc::new(WatchScheduler::new(WatchSchedulerDeps {
        db: db.clone(),
        kv: kyomi_core::kv_store_memory::InMemoryKVStore::new_pool(),
        encryption_key: Arc::new([7u8; 32]),
        embedding: crate::test_support::loaded_embedding(),
        ws_manager: kyomi_auth::websocket::WebSocketManager::new(None, db.clone()),
        config: Arc::new(Config::test_config()),
        connect_registry: None,
        platforms: Arc::new(kyomi_core::platform::PlatformRegistry::new()),
        cancel: CancellationToken::new(),
    }))
}

struct Fx {
    db: DbPool,
    ws: String,
    user: String,
    watch: String,
}

async fn fixture(db: DbPool) -> Fx {
    let id = &uuid::Uuid::new_v4().simple().to_string()[..10];
    let (ws, user, watch) = (format!("k973-w-{id}"), format!("k973-u-{id}"), format!("k973-h-{id}"));
    let p = pg(&db);
    sqlx::query("INSERT INTO users (user_id, email) VALUES ($1, $2)").bind(&user).bind(format!("{user}@example.com")).execute(p).await.unwrap();
    sqlx::query("INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, 'W', $2)").bind(&ws).bind(&user).execute(p).await.unwrap();
    sqlx::query("INSERT INTO watches (watch_id, workspace_id, created_by, name, prompt, schedule, mode) VALUES ($1, $2, $3, 'W', 'p', '0 9 * * *', 'alert')").bind(&watch).bind(&ws).bind(&user).execute(p).await.unwrap();
    Fx { db, ws, user, watch }
}

impl Fx {
    async fn execution(&self, mode: &str, started_ago: Duration, owner: Option<&str>) -> i32 {
        let e = kyomi_auth::watch_service::create_execution(&self.db, &self.watch, "W", &self.ws, kyomi_core::WatchMode::Alert, &self.user).await.unwrap();
        sqlx::query("UPDATE watch_executions SET started_at = $1 WHERE id = $2").bind(Utc::now() - started_ago).bind(e.id).execute(pg(&self.db)).await.unwrap();
        let now = Utc::now();
        store::record_run_llm(&self.db, e.id, &store::RunLlm { provider: "anthropic", model: "m", mode, deadline: (mode == "batch").then_some(now + Duration::hours(3)), fallback_reason: None, lease: owner.map(|o| (o, now + Duration::seconds(120))), workspace_billed: false }).await.unwrap();
        e.id
    }

    async fn status(&self, id: i32) -> String {
        sqlx::query_as::<_, (String,)>("SELECT status FROM watch_executions WHERE id = $1").bind(id).fetch_one(pg(&self.db)).await.unwrap().0
    }

    async fn cleanup(&self) {
        let p = pg(&self.db);
        let _ = sqlx::query("DELETE FROM watch_executions WHERE workspace_id = $1").bind(&self.ws).execute(p).await;
        let _ = sqlx::query("DELETE FROM watches WHERE workspace_id = $1").bind(&self.ws).execute(p).await;
        let _ = sqlx::query("DELETE FROM workspaces WHERE workspace_id = $1").bind(&self.ws).execute(p).await;
        let _ = sqlx::query("DELETE FROM users WHERE user_id = $1").bind(&self.user).execute(p).await;
    }
}

#[tokio::test]
async fn postgres_orphan_recovery_times_out_realtime_runs_but_never_batch_runs() {
    let Some(db) = pg_or_skip("postgres_orphan_recovery_times_out_realtime_runs_but_never_batch_runs").await else { return };
    let f = fixture(db).await;
    let realtime = f.execution("realtime", Duration::hours(2), None).await;
    let batch = f.execution("batch", Duration::hours(5), Some("someone")).await;
    store::set_waiting(&f.db, batch, true).await.unwrap();

    scheduler(&f.db).await.recover_orphaned_executions().await;

    assert_eq!(f.status(realtime).await, "error", "the existing 30-minute orphan rule still applies to realtime runs");
    assert_eq!(f.status(batch).await, "waiting_batch", "a batch run is exempt, however long it waits");
    f.cleanup().await;
}

#[tokio::test]
async fn postgres_shutdown_stops_batch_drivers_and_hands_their_leases_back() {
    let Some(db) = pg_or_skip("postgres_shutdown_stops_batch_drivers_and_hands_their_leases_back").await else { return };
    let f = fixture(db).await;
    let id = f.execution("batch", Duration::minutes(10), Some(crate::batch_run::instance_id())).await;
    let s = scheduler(&f.db).await;
    // A driver for this watch that never finishes by itself.
    let handle = tokio::spawn(async { std::future::pending::<()>().await });
    s.active_executions.lock().await.insert(f.watch.clone(), handle);

    s.shutdown().await;

    assert_eq!(f.status(id).await, "running", "shutdown must not fail a batch run");
    let resumable = store::resumable_runs(&f.db, Utc::now() + Duration::seconds(1), 100).await.unwrap();
    assert!(resumable.iter().any(|r| r.id == id), "the released run is resumable at once");
    f.cleanup().await;
}
