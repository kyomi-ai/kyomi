// SPDX-License-Identifier: AGPL-3.0-or-later

//! Regression test for
//! `apps/server/migrations-sqlite/00037_reconcile_unverified_user_rows.sql`
//! (KYO-683), the SQLite twin of
//! `apps/server/migrations/20260910120000_reconcile_unverified_user_rows.sql`.
//!
//! This migration is pure DML, not schema — it touches no column or
//! constraint, so `schema_parity.rs` (which only compares the *shape* of
//! the two migration chains) gives it zero coverage. It also deletes
//! `users` rows in production, so it needs its own behavioural test rather
//! than relying on the migration having merely applied without a SQL error.
//!
//! Table and column names match 1:1 between the two chains (see the
//! Postgres file's header comment for the full referencing-table sweep and
//! rationale); this test only exercises the SQLite twin, the same choice
//! `collections_created_by_migration.rs` and
//! `refresh_tokens_family_id_migration.rs` make for their own DML/rebuild
//! migrations — both files' DML is identical modulo SQLite's `0/1` booleans
//! vs Postgres's `true/false`.
//!
//! Four scenarios, matching the migration's two dispositions:
//!
//!  1. An unverified row referenced via `watches.created_by` (one of the
//!     original, pre-KYO-683-review columns) must end up `verified = 1`.
//!  2. An unverified row referenced via `watch_executions.deleted_by` — one
//!     of the columns added to close the code-review gap this same ticket
//!     found (the reviewer's finding #1: `dashboards.created_by`,
//!     `dashboards.updated_by`, and `watch_executions.deleted_by` were
//!     missing from the original sweep) — must also end up `verified = 1`.
//!     This is the regression guard for that fix: reverting just the
//!     `deleted_by` clauses makes this test fail (see the migration file's
//!     own history for the before/after run this test was built against).
//!  3. An unverified row referenced by nothing must be deleted.
//!  4. An already-`verified = 1` row must be left completely untouched —
//!     the migration's `WHERE verified = 0`/`WHERE verified = false` guard
//!     must exclude it from both the UPDATE and the DELETE.

use std::borrow::Cow;
use std::path::Path;

use sqlx::migrate::Migrator;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::Row;

/// Version of `00037_reconcile_unverified_user_rows.sql`, per
/// `sqlx::migrate!`'s filename-prefix convention.
const TARGET_MIGRATION_VERSION: i64 = 37;

/// Resolve every migration in `apps/server/migrations-sqlite` at runtime
/// (not the compile-time `sqlx::migrate!` macro `db.rs`/other tests use —
/// that always embeds and runs the *full* chain, which can't stop short of
/// 00037) and return a [`Migrator`] restricted to versions `<= version_limit`.
async fn sqlite_migrator_up_to(version_limit: i64) -> Migrator {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/server/migrations-sqlite");
    let full = Migrator::new(Path::new(dir))
        .await
        .expect("resolve apps/server/migrations-sqlite at runtime");

    let restricted: Vec<_> =
        full.migrations.iter().filter(|m| m.version <= version_limit).cloned().collect();
    assert!(
        !restricted.is_empty(),
        "version_limit {version_limit} excluded every migration — check TARGET_MIGRATION_VERSION"
    );

    Migrator { migrations: Cow::Owned(restricted), ..full }
}

/// Fresh in-memory SQLite database migrated up to (but not including)
/// 00037, with FK enforcement on.
async fn pool_before_reconcile_migration() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&pool)
        .await
        .expect("enable foreign keys");
    sqlite_migrator_up_to(TARGET_MIGRATION_VERSION - 1)
        .await
        .run(&pool)
        .await
        .expect("run migrations up to 00036");
    pool
}

async fn run_reconcile_migration(pool: &sqlx::SqlitePool) {
    sqlite_migrator_up_to(TARGET_MIGRATION_VERSION)
        .await
        .run(pool)
        .await
        .expect("run migration 00037 against the seeded fixture database");
}

/// Seed a user row. `verified` is SQLite's `0`/`1` integer boolean.
async fn seed_user(pool: &sqlx::SqlitePool, user_id: &str, verified: i64) {
    sqlx::query(
        "INSERT INTO users (user_id, email, created_at, verified) \
         VALUES (?1, ?2, '2020-01-01T00:00:00Z', ?3)",
    )
    .bind(user_id)
    .bind(format!("{user_id}@example.com"))
    .bind(verified)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("seed user {user_id}: {e}"));
}

async fn fetch_verified(pool: &sqlx::SqlitePool, user_id: &str) -> Option<i64> {
    sqlx::query("SELECT verified FROM users WHERE user_id = ?1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .expect("query users row")
        .map(|row| row.get::<i64, _>("verified"))
}

#[tokio::test]
async fn migration_00037_verifies_row_referenced_via_watches_created_by() {
    let pool = pool_before_reconcile_migration().await;

    // A stable owner for the workspace FK chain — verified and otherwise
    // uninvolved in the assertion, so the only thing that can explain the
    // test user ending up verified is the watches.created_by reference.
    seed_user(&pool, "owner-1", 1).await;
    sqlx::query(
        "INSERT INTO workspaces (workspace_id, name, owner_user_id) \
         VALUES ('ws-1', 'WS1', 'owner-1')",
    )
    .execute(&pool)
    .await
    .expect("seed workspace");

    seed_user(&pool, "user-referenced-via-watch", 0).await;
    sqlx::query(
        "INSERT INTO watches (watch_id, workspace_id, created_by, name, prompt, schedule) \
         VALUES ('watch-1', 'ws-1', 'user-referenced-via-watch', 'Watch', 'prompt', 'daily')",
    )
    .execute(&pool)
    .await
    .expect("seed watches row referencing the test user via created_by");

    run_reconcile_migration(&pool).await;

    assert_eq!(
        fetch_verified(&pool, "user-referenced-via-watch").await,
        Some(1),
        "a row referenced via watches.created_by must be force-verified, not deleted"
    );
}

#[tokio::test]
async fn migration_00037_verifies_row_referenced_via_watch_executions_deleted_by() {
    // Regression guard for code-review finding #1: watch_executions.deleted_by
    // was missing from the original sweep. Reverting just the deleted_by
    // EXISTS clauses added to close that gap makes this test fail (the row
    // gets deleted instead of verified) — see this migration's PR for the
    // recorded before/after run.
    let pool = pool_before_reconcile_migration().await;

    seed_user(&pool, "user-referenced-via-exec", 0).await;
    // watch_id is nullable (ON DELETE SET NULL) and only `status` is
    // NOT NULL, so this row needs no workspace/watch fixture at all.
    sqlx::query(
        "INSERT INTO watch_executions (status, deleted_at, deleted_by) \
         VALUES ('completed', '2020-06-01T00:00:00Z', 'user-referenced-via-exec')",
    )
    .execute(&pool)
    .await
    .expect("seed watch_executions row referencing the test user via deleted_by");

    run_reconcile_migration(&pool).await;

    assert_eq!(
        fetch_verified(&pool, "user-referenced-via-exec").await,
        Some(1),
        "a row referenced via watch_executions.deleted_by must be force-verified, not deleted \
         (KYO-683 code review finding #1 regression guard)"
    );
}

#[tokio::test]
async fn migration_00037_deletes_unreferenced_row() {
    let pool = pool_before_reconcile_migration().await;

    seed_user(&pool, "user-orphan", 0).await;

    run_reconcile_migration(&pool).await;

    assert_eq!(
        fetch_verified(&pool, "user-orphan").await,
        None,
        "an unverified row referenced by nothing must be deleted, not left behind"
    );
}

#[tokio::test]
async fn migration_00037_leaves_already_verified_row_untouched() {
    let pool = pool_before_reconcile_migration().await;

    sqlx::query(
        "INSERT INTO users (user_id, email, created_at, updated_at, verified) \
         VALUES ('user-already-verified', 'already@example.com', \
                 '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z', 1)",
    )
    .execute(&pool)
    .await
    .expect("seed already-verified user with a fixed updated_at");

    run_reconcile_migration(&pool).await;

    let (verified, updated_at): (i64, String) = sqlx::query_as(
        "SELECT verified, updated_at FROM users WHERE user_id = 'user-already-verified'",
    )
    .fetch_one(&pool)
    .await
    .expect("fetch the already-verified row after migrating");
    assert_eq!(verified, 1, "an already-verified row must remain verified");
    assert_eq!(
        updated_at, "2020-01-01T00:00:00Z",
        "an already-verified row must not be touched by this migration at all — \
         WHERE verified = 0 must exclude it from the UPDATE"
    );
}
