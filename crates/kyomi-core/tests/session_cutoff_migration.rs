// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-341: execute the actual additive SQL against pre-existing user rows.

const SQLITE_SQL: &str =
    include_str!("../../../apps/server/migrations-sqlite/00040_add_sessions_valid_from.sql");
const POSTGRES_SQL: &str =
    include_str!("../../../apps/server/migrations/20261002000000_add_sessions_valid_from.sql");
const PRECISE_CUTOFF: i64 = 1_791_000_000_123_456;

#[tokio::test]
async fn session_cutoff_migration_sqlite_preserves_legacy_and_microseconds() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE users (user_id TEXT PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users VALUES ('existing-user')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(SQLITE_SQL).execute(&pool).await.unwrap();
    let cutoff: Option<i64> =
        sqlx::query_scalar("SELECT sessions_valid_from FROM users WHERE user_id = 'existing-user'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cutoff, None);
    sqlx::query("UPDATE users SET sessions_valid_from = $1")
        .bind(PRECISE_CUTOFF)
        .execute(&pool)
        .await
        .unwrap();
    let cutoff: i64 = sqlx::query_scalar("SELECT sessions_valid_from FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        cutoff, PRECISE_CUTOFF,
        "integer microseconds must round-trip exactly"
    );
}

#[tokio::test]
async fn session_cutoff_migration_postgres_preserves_legacy_and_microseconds() {
    let db = match kyomi_core::test_db::connect_test_pool().await {
        Ok(db) => db,
        Err(error) => {
            assert_ne!(
                std::env::var("KYOMI_REQUIRE_POSTGRES_TESTS").as_deref(),
                Ok("1"),
                "required Postgres unavailable: {error}"
            );
            eprintln!("SKIP: session_cutoff_migration_postgres — Postgres unavailable: {error}");
            return;
        }
    };
    let kyomi_core::DbPool::Postgres(pool) = db else {
        panic!("Postgres required");
    };
    let mut tx = pool.begin().await.unwrap();
    // Connection-local temp users shadow the production table so this test can
    // exercise the real migration on a pre-column schema without touching it.
    sqlx::query("CREATE TEMP TABLE users (user_id TEXT PRIMARY KEY) ON COMMIT DROP")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users VALUES ('existing-user')")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::raw_sql(POSTGRES_SQL).execute(&mut *tx).await.unwrap();
    let cutoff: Option<i64> =
        sqlx::query_scalar("SELECT sessions_valid_from FROM users WHERE user_id = 'existing-user'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(cutoff, None);
    sqlx::query("UPDATE users SET sessions_valid_from = $1")
        .bind(PRECISE_CUTOFF)
        .execute(&mut *tx)
        .await
        .unwrap();
    let cutoff: i64 = sqlx::query_scalar("SELECT sessions_valid_from FROM users")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        cutoff, PRECISE_CUTOFF,
        "integer microseconds must round-trip exactly"
    );
    tx.commit().await.unwrap();
}
