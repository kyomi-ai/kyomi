// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exercise the actual execution loader, including every persisted timezone state.

use super::*;

async fn assert_execution_loader_timezones(db: &DbPool) {
    // A connection-local table isolates this projection test from other tests.
    let (json_type, timestamp_type) = if db.is_postgres() {
        ("JSONB", "TIMESTAMPTZ")
    } else {
        ("TEXT", "TEXT")
    };
    let schema = format!(
        "CREATE TEMPORARY TABLE watches (
            watch_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL,
            created_by TEXT NOT NULL, name TEXT NOT NULL, prompt TEXT NOT NULL,
            schedule TEXT NOT NULL, timezone TEXT, mode TEXT NOT NULL,
            datasource_hints {json_type}, queries {json_type}, alert_emails TEXT,
            alert_emails_enabled BOOLEAN NOT NULL, enabled BOOLEAN NOT NULL,
            last_run_at {timestamp_type}, last_run_status TEXT,
            next_run_at {timestamp_type}, created_at {timestamp_type} NOT NULL,
            updated_at {timestamp_type} NOT NULL
        )"
    );
    kyomi_core::db_execute!(db, &schema).unwrap();

    let next_run: chrono::DateTime<Utc> = "2026-10-04T22:00:00Z".parse().unwrap();
    let created_at: chrono::DateTime<Utc> = "2026-10-03T00:00:00Z".parse().unwrap();
    for (id, timezone, schedule) in [
        ("legacy", None, "0 23 * * 0"),
        ("utc", Some("UTC"), "0 9 * * 1"),
        ("sydney", Some("Australia/Sydney"), "0 9 * * 1"),
    ] {
        kyomi_core::db_execute!(
            db,
            "INSERT INTO watches (
                watch_id, workspace_id, created_by, name, prompt, schedule,
                timezone, mode, alert_emails_enabled, enabled, next_run_at,
                created_at, updated_at
            ) VALUES ($1, 'loader-ws', 'loader-user', 'Execution loader',
                'Report weekly revenue trends', $2, $3, 'report', false, true,
                $4, $5, $5)",
            id,
            schedule,
            timezone,
            next_run,
            created_at
        )
        .unwrap();
        // This is the private function called before both manual and scheduled
        // execution, rather than the CRUD service or a second query in the test.
        let watch = load_watch(db, id).await.unwrap().unwrap();
        assert_eq!(watch.watch_id, id);
        assert_eq!(watch.timezone.as_deref(), timezone);
        assert_eq!(watch.schedule, schedule);
        assert_eq!(watch.mode, WatchMode::Report);
        assert_eq!(watch.next_run_at, Some(next_run));
        assert_eq!(watch.created_at, created_at);
    }
    assert!(load_watch(db, "missing").await.unwrap().is_none());
}

#[tokio::test]
async fn execution_loader_timezones_sqlite() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    assert_execution_loader_timezones(&DbPool::Sqlite(pool)).await;
}

#[tokio::test]
async fn execution_loader_timezones_postgres() {
    let db = match kyomi_core::test_db::connect_test_pool().await {
        Ok(DbPool::Postgres(pool)) => pool,
        result => {
            assert_ne!(
                std::env::var("KYOMI_REQUIRE_POSTGRES_TESTS").as_deref(),
                Ok("1"),
                "PostgreSQL execution-loader coverage required: {result:?}"
            );
            eprintln!(
                "SKIP: execution_loader_timezones_postgres — PostgreSQL unavailable: {result:?}"
            );
            return;
        }
    };
    // Provision first, then open one dedicated connection so its temporary
    // table shadows only this test's watches table and is dropped on close.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*db.connect_options()).clone())
        .await
        .unwrap();
    assert_execution_loader_timezones(&DbPool::Postgres(pool.clone())).await;
    pool.close().await;
}
