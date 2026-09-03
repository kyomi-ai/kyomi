// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared SQLite test-fixture scaffolding for `kyomi-auth`'s unit tests.
//!
//! Every test module in this crate that needed a database used to hand-roll
//! its own copy of "connect to an in-memory SQLite pool, run migrations,
//! insert a user/workspace row" — 19 files, ~84 near-identical helper
//! definitions between them (KYO-271, KYO-368). That duplication meant the
//! copies could — and did — drift: some enabled `PRAGMA foreign_keys=ON` and
//! some didn't, so a test could pass against a fixture that silently
//! permitted a foreign-key violation that would fail in production. This
//! module is the one place that scaffolding lives now.
//!
//! # Foreign keys are always on
//!
//! [`test_pool`] unconditionally executes `PRAGMA foreign_keys=ON`, matching
//! what production connections do (see `kyomi_core::DbPool::connect`).
//! Three of the pre-extraction copies did not:
//!
//! - `catalog::helpers` built its pool via `DbPool::connect("sqlite::memory:")`
//!   directly rather than the hand-rolled `test_pool()` pattern — but
//!   `DbPool::connect` itself sets this pragma, so those tests already had
//!   foreign-key enforcement on. Migrating them to this module changes
//!   nothing observable.
//! - `push_service`'s two hand-rolled pools did not set the pragma. Turning
//!   it on surfaced no failure: both tests insert the referenced `users` row
//!   before the `push_subscriptions` row that references it, so enforcement
//!   was already satisfied — it was simply never checked.
//! - `test_pg.rs` is not part of this consolidation: it is the crate's
//!   existing shared harness for tests that need a *real* Postgres pool (see
//!   its own module docs), where a SQLite pragma does not apply. Postgres
//!   enforces declared foreign keys unconditionally.
//!
//! # Extending this module
//!
//! Add a helper here only when a **second** caller needs the exact same
//! generic scaffolding (a `users`/`workspaces`/`workspace_users` row, or the
//! pool itself). A helper that seeds one module's own domain-specific
//! entities (a chat session, a dashboard, a watch, a datasource) belongs in
//! that module, built on top of [`test_pool`]/[`seed_user`]/
//! [`seed_workspace`]/[`sqlite_pool`], not here — see
//! `docs/CODING_STANDARDS.md`'s "third copy is the extraction trigger" rule.

use kyomi_core::DbPool;
use sqlx::sqlite::SqlitePoolOptions;

/// Build an in-memory SQLite pool with the full server migration chain
/// applied and foreign-key enforcement on. A single connection
/// (`max_connections(1)`) is used because SQLite `:memory:` databases are
/// per-connection — every query in a test must hit the same pool.
pub(crate) async fn test_pool() -> DbPool {
    let _ = kyomi_core::constants::load_with_fallback();

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");

    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&pool)
        .await
        .expect("enable foreign keys");

    sqlx::migrate!("../../apps/server/migrations-sqlite")
        .run(&pool)
        .await
        .expect("run sqlite migrations");

    DbPool::Sqlite(pool)
}

/// Extract the inner `SqlitePool` from a `DbPool` built by [`test_pool`].
///
/// Panics if given a `Postgres` pool — every caller of this function is a
/// SQLite-only unit test.
pub(crate) fn sqlite_pool(db: &DbPool) -> &sqlx::SqlitePool {
    match db {
        DbPool::Sqlite(sq) => sq,
        DbPool::Postgres(_) => panic!("test requires sqlite pool"),
    }
}

/// Insert a minimal `users` row. Active by default; use
/// [`seed_user_with_active`] for tests that need to vary the `active`
/// column.
pub(crate) async fn seed_user(sq: &sqlx::SqlitePool, user_id: &str, email: &str) {
    seed_user_with_active(sq, user_id, email, true).await;
}

/// Insert a `users` row with an explicit `active` value — the column
/// [`seed_user`]'s callers don't vary but a few tests (session/workspace
/// membership gating) do.
pub(crate) async fn seed_user_with_active(
    sq: &sqlx::SqlitePool,
    user_id: &str,
    email: &str,
    active: bool,
) {
    sqlx::query("INSERT INTO users (user_id, email, active) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(email)
        .bind(active)
        .execute(sq)
        .await
        .expect("insert user");
}

/// Insert a minimal `workspaces` row.
pub(crate) async fn seed_workspace(sq: &sqlx::SqlitePool, workspace_id: &str, owner_user_id: &str) {
    sqlx::query("INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $2, $3)")
        .bind(workspace_id)
        .bind(format!("Workspace {workspace_id}"))
        .bind(owner_user_id)
        .execute(sq)
        .await
        .expect("insert workspace");
}

/// Insert a `workspace_users` membership row with an explicit role.
pub(crate) async fn seed_membership(
    sq: &sqlx::SqlitePool,
    workspace_id: &str,
    user_id: &str,
    role: &str,
    active: bool,
) {
    sqlx::query(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(role)
    .bind(active)
    .execute(sq)
    .await
    .expect("insert membership");
}

/// Insert a `workspace_users` membership row with the schema's default role
/// (`workspace_user`) and an explicit `created_at`, for tests that assert on
/// membership ordering.
pub(crate) async fn seed_membership_at(
    sq: &sqlx::SqlitePool,
    workspace_id: &str,
    user_id: &str,
    active: bool,
    created_at: &str,
) {
    sqlx::query(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active, created_at) \
         VALUES ($1, $2, 'workspace_user', $3, $4)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(active)
    .bind(created_at)
    .execute(sq)
    .await
    .expect("insert membership");
}

/// Seed two users (`"user-a"`, `"user-b"`) into one workspace (`"ws-1"`),
/// with user-a as `workspace_admin`/owner and user-b as a plain member —
/// **including their `workspace_users` membership rows**, not just the
/// `users`/`workspaces` rows.
///
/// Do not drop those membership inserts to "simplify" this fixture:
/// `sync_log_service::get_entries_since`'s visibility filter — and every
/// endpoint that calls it — depends on the caller already being a
/// `workspace_users` member of the workspace it's querying. That had to be
/// rediscovered once already when writing KYO-258's tests (see KYO-271); a
/// caller that only needs the `users`/`workspaces` rows without membership
/// should call [`seed_user`]/[`seed_workspace`] directly rather than strip
/// this helper down.
///
/// `user_a_email`/`user_b_email` are parameters, not literals, because the
/// two pre-extraction copies (`chat_service`, `collection_service`) used
/// different email strings for no reason tied to what they were testing.
pub(crate) async fn seed_two_users_one_workspace(
    sq: &sqlx::SqlitePool,
    user_a_email: &str,
    user_b_email: &str,
) {
    seed_user(sq, "user-a", user_a_email).await;
    seed_user(sq, "user-b", user_b_email).await;
    seed_workspace(sq, "ws-1", "user-a").await;
    seed_membership(sq, "ws-1", "user-a", "workspace_admin", true).await;
    seed_membership(sq, "ws-1", "user-b", "user", true).await;
}

/// A fixed AES-256 test key — deliberately not random so encryption tests
/// are reproducible byte-for-byte.
pub(crate) fn test_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    key[..16].copy_from_slice(b"test-key-1234567");
    key[16..].copy_from_slice(b"8901234567890123");
    key
}
