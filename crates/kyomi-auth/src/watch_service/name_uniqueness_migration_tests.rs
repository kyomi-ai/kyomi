// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use sqlx::migrate::Migrator;
use std::{borrow::Cow, path::Path};

const SQLITE_VERSION: i64 = 39;
const POSTGRES_VERSION: i64 = 20261003000000;

async fn migrator(postgres: bool, before: bool) -> Migrator {
    let directory = if postgres {
        "migrations"
    } else {
        "migrations-sqlite"
    };
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/server")
        .join(directory);
    let full = Migrator::new(path).await.expect("resolve migration chain");
    let target = if postgres {
        POSTGRES_VERSION
    } else {
        SQLITE_VERSION
    };
    assert!(
        full.iter().any(|m| m.version == target),
        "name uniqueness migration must exist"
    );
    if before {
        let migrations = full
            .iter()
            .filter(|m| m.version < target)
            .cloned()
            .collect();
        Migrator {
            migrations: Cow::Owned(migrations),
            ..full
        }
    } else {
        full
    }
}

async fn run_migrations(db: &DbPool, before: bool) {
    let migrations = migrator(db.is_postgres(), before).await;
    match db {
        DbPool::Postgres(pool) => migrations.run(pool).await.expect("migrate postgres"),
        DbPool::Sqlite(pool) => migrations.run(pool).await.expect("migrate sqlite"),
    }
}

async fn watch_indexes(db: &DbPool) -> Vec<String> {
    let sql = if db.is_postgres() {
        "SELECT indexname FROM pg_indexes WHERE schemaname = 'public' AND tablename = 'watches' ORDER BY indexname"
    } else {
        "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'watches' ORDER BY name"
    };
    match db {
        DbPool::Postgres(pool) => sqlx::query_scalar(sql)
            .fetch_all(pool)
            .await
            .expect("postgres indexes"),
        DbPool::Sqlite(pool) => sqlx::query_scalar(sql)
            .fetch_all(pool)
            .await
            .expect("sqlite indexes"),
    }
}

/// Read a watch using the pre-timezone projection.
///
/// `get_watch` selects `timezone`, but this test's `before` state runs only
/// the migrations below *this branch's* target (SQLite 39 /
/// Postgres 20261003000000) so the forward migration still has work to do —
/// and that target is numbered *earlier* than main's timezone migration
/// (SQLite 00045 / Postgres 20261005000000). The before-schema therefore has
/// no `timezone` column at all, and `get_watch` fails with "no such column"
/// before the comparison is ever reached.
///
/// `CAST(NULL AS TEXT) AS timezone` supplies the column name `models::Watch`
/// expects without ever referencing the real one, so this works on both the
/// pre- and post-timezone schemas. The field is trivially equal across the
/// migration: the seeded row is inserted without a timezone, and
/// `ALTER TABLE watches ADD COLUMN timezone TEXT` is nullable with no default,
/// so the after-state decodes to `None` as well. Timezone preservation is not
/// this test's subject — the name-uniqueness migration never touches it.
async fn get_watch_before_timezone(
    db: &DbPool,
    watch_id: &str,
    workspace_id: &str,
    user_id: &str,
) -> Result<Option<kyomi_core::models::Watch>> {
    let sql = r#"
        SELECT watch_id, workspace_id, created_by, name, prompt, schedule,
               CAST(NULL AS TEXT) AS timezone,
               mode, datasource_hints, queries, alert_emails,
               alert_emails_enabled, enabled, last_run_at, last_run_status,
               next_run_at, created_at, updated_at
        FROM watches
        WHERE watch_id = $1 AND workspace_id = $2 AND created_by = $3
    "#;

    let watch = kyomi_core::db_fetch_optional!(
        db,
        kyomi_core::models::Watch,
        sql,
        watch_id,
        workspace_id,
        user_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get watch: {e}")))?;

    Ok(watch)
}

async fn assert_migration_preserves_rows_and_indexes(db: &DbPool) {
    run_migrations(db, true).await;
    let before_indexes = watch_indexes(db).await;
    assert!(
        before_indexes
            .iter()
            .any(|i| i == "idx_watches_name_workspace_unique"),
        "deployed schema must start with the old unique index"
    );
    kyomi_core::db_execute!(
        db,
        "INSERT INTO users (user_id, email) VALUES ('name-user', 'name-user@example.com')"
    )
    .expect("seed user");
    kyomi_core::db_execute!(db, "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ('name-ws', 'Names', 'name-user')")
        .expect("seed workspace");
    kyomi_core::db_execute!(db, "INSERT INTO watches (watch_id, workspace_id, created_by, name, prompt, schedule, mode) VALUES ('name-watch', 'name-ws', 'name-user', 'Revenue Alert', 'Original monitoring prompt', '0 9 * * *', 'alert')")
        .expect("seed existing watch");
    kyomi_core::db_execute!(db, "INSERT INTO watch_executions (watch_id, workspace_id, created_by, status, alert_triggered) VALUES ('name-watch', 'name-ws', 'name-user', 'success', $1)", true)
        .expect("seed existing execution");
    let before = get_watch_before_timezone(db, "name-watch", "name-ws", "name-user")
        .await
        .expect("read before")
        .expect("existing watch");
    let alerts_before = get_alerts_history(db, "name-ws", None, 50, 0, false, "name-user")
        .await
        .expect("alerts before");

    run_migrations(db, false).await;
    let expected_indexes: Vec<_> = before_indexes
        .into_iter()
        .filter(|i| i != "idx_watches_name_workspace_unique")
        .collect();
    assert_eq!(
        watch_indexes(db).await,
        expected_indexes,
        "drop only name uniqueness, preserving the primary key and useful nonunique indexes"
    );
    let after = get_watch(db, "name-watch", "name-ws", "name-user")
        .await
        .expect("read after")
        .expect("preserved watch");
    assert_eq!(
        serde_json::to_value(after).expect("serialize watch"),
        serde_json::to_value(before).expect("serialize watch"),
        "migration must preserve all watch fields"
    );
    let alerts_after = get_alerts_history(db, "name-ws", None, 50, 0, false, "name-user")
        .await
        .expect("alerts after");
    assert_eq!(
        serde_json::to_value(alerts_after).expect("serialize alerts"),
        serde_json::to_value(alerts_before).expect("serialize alerts"),
        "migration must preserve execution rows"
    );
    for id in ["name-duplicate", "name-case"] {
        let name = if id == "name-case" {
            "revenue alert"
        } else {
            "Revenue Alert"
        };
        kyomi_core::db_execute!(db, "INSERT INTO watches (watch_id, workspace_id, created_by, name, prompt, schedule, mode) VALUES ($1, 'name-ws', 'name-user', $2, 'Monitoring prompt', '0 9 * * *', 'alert')", id, name)
            .expect("database accepts duplicate labels after forward migration");
    }
    // Running the full migrator again exercises SQLx's deployed version ledger.
    run_migrations(db, false).await;
}

#[tokio::test]
async fn sqlite_watch_name_forward_migration_preserves_rows_and_indexes() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool");
    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&pool)
        .await
        .expect("foreign keys");
    assert_migration_preserves_rows_and_indexes(&DbPool::Sqlite(pool)).await;
    let fresh = crate::test_support::test_pool().await;
    assert!(
        !watch_indexes(&fresh)
            .await
            .iter()
            .any(|i| i == "idx_watches_name_workspace_unique"),
        "fresh SQLite migration chain must remove name uniqueness"
    );
}

#[tokio::test]
async fn postgres_watch_name_forward_migration_preserves_rows_and_indexes() {
    // Scratch provisioning is restricted to the dedicated local test server.
    // An inherited dev/production DATABASE_URL must never authorize migrations.
    let url = kyomi_core::test_db::test_database_url();
    let parsed = url::Url::parse(&url).expect("test database URL");
    assert!(
        matches!(parsed.host_str(), Some("localhost" | "127.0.0.1"))
            && parsed.port() == Some(5434)
            && parsed
                .path()
                .trim_start_matches('/')
                .starts_with("kyomi_test"),
        "scratch migration tests require the local PostgreSQL test server on port 5434; unset DATABASE_URL"
    );
    // Reuse the hardened availability gate and the same disposable test server;
    // migrations need a separate empty database to exercise an upgrade path.
    let Some(_) = crate::test_pg::postgres_test_pool_or_skip(
        "postgres_watch_name_forward_migration_preserves_rows_and_indexes",
    )
    .await
    else {
        return;
    };
    let (server, _) = kyomi_core::test_db::split_database_url(&url);
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&format!("{server}/postgres"))
        .await
        .expect("test database admin connection");
    let name = format!("kyomi_watch_names_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(&admin)
        .await
        .expect("create scratch database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&format!("{server}/{name}"))
        .await
        .expect("connect scratch database");
    let db = DbPool::Postgres(pool.clone());
    assert_migration_preserves_rows_and_indexes(&db).await;
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE \"{name}\""))
        .execute(&admin)
        .await
        .expect("drop upgraded scratch database");
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(&admin)
        .await
        .expect("create fresh scratch database");
    let fresh_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&format!("{server}/{name}"))
        .await
        .expect("connect fresh scratch database");
    let fresh = DbPool::Postgres(fresh_pool.clone());
    run_migrations(&fresh, false).await;
    assert!(
        !watch_indexes(&fresh)
            .await
            .iter()
            .any(|i| i == "idx_watches_name_workspace_unique"),
        "fresh Postgres migration chain must remove name uniqueness"
    );
    fresh_pool.close().await;
    sqlx::query(&format!("DROP DATABASE \"{name}\""))
        .execute(&admin)
        .await
        .expect("drop scratch database");
}
