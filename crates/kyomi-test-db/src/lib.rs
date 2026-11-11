// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared SQLite test fixtures for workspace crates (KYO-751).
//!
//! Tests use the real server migration chain, one in-memory connection, and
//! foreign-key enforcement matching production. Centralizing these settings
//! prevents the drift previously seen across per-module fixtures (KYO-271).
//!
//! # Extending this module
//!
//! Add a helper here only when a **second** caller needs the exact same
//! generic scaffolding (a `users`/`workspaces`/`workspace_users` row, or the
//! pool itself). A helper that seeds one module's own domain-specific
//! entities (a chat session, a dashboard, a watch, a datasource) belongs in
//! that module, built on top of [`test_pool`]/[`seed_user`]/
//! [`seed_workspace`]/[`sqlite_pool`], not here — see
//! `docs/standards/code-organization/third-copy-of-test-helper-is-extraction-trigger.md`.

use kyomi_core::DbPool;
use sqlx::sqlite::SqlitePoolOptions;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../apps/server/migrations-sqlite");

/// Build an in-memory SQLite pool with the full server migration chain
/// applied and foreign-key enforcement on. A single connection
/// (`max_connections(1)`) is used because SQLite `:memory:` databases are
/// per-connection — every query in a test must hit the same pool.
pub async fn test_pool() -> DbPool {
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

    MIGRATOR.run(&pool).await.expect("run sqlite migrations");

    DbPool::Sqlite(pool)
}

/// Extract the inner `SqlitePool` from a `DbPool` built by [`test_pool`].
///
/// Panics if given a `Postgres` pool — every caller of this function is a
/// SQLite-only unit test.
pub fn sqlite_pool(db: &DbPool) -> &sqlx::SqlitePool {
    match db {
        DbPool::Sqlite(sq) => sq,
        DbPool::Postgres(_) => panic!("test requires sqlite pool"),
    }
}

/// Insert a minimal `users` row. Active by default; use
/// [`seed_user_with_active`] for tests that need to vary the `active`
/// column.
pub async fn seed_user(sq: &sqlx::SqlitePool, user_id: &str, email: &str) {
    seed_user_with_active(sq, user_id, email, true).await;
}

/// Insert a `users` row with an explicit `active` value — the column
/// [`seed_user`]'s callers don't vary but a few tests (session/workspace
/// membership gating) do.
pub async fn seed_user_with_active(
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
pub async fn seed_workspace(sq: &sqlx::SqlitePool, workspace_id: &str, owner_user_id: &str) {
    seed_workspace_with_name(
        sq,
        workspace_id,
        owner_user_id,
        &format!("Workspace {workspace_id}"),
    )
    .await;
}

/// Insert a workspace with an explicit display name.
///
/// KYO-751: preserves the agent fixture's existing `"Workspace"` name while
/// sharing the same generic insert as fixtures using the default name.
pub async fn seed_workspace_with_name(
    sq: &sqlx::SqlitePool,
    workspace_id: &str,
    owner_user_id: &str,
    name: &str,
) {
    sqlx::query("INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $2, $3)")
        .bind(workspace_id)
        .bind(name)
        .bind(owner_user_id)
        .execute(sq)
        .await
        .expect("insert workspace");
}

/// Insert a `workspace_users` membership row with an explicit role.
pub async fn seed_membership(
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

#[cfg(test)]
mod tests;
