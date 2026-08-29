// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared test fixtures for this crate's tool tests.
//!
//! [`test_pool`] and [`build_ctx`] were duplicated four times across this
//! crate before KYO-537 — `tools/catalog.rs` (twice, as `test_pool` /
//! `build_ctx`), `agent.rs`'s `test_tool_context`, `execution.rs`'s
//! `test_tool_context_for_roles`, and `tools/watch.rs`'s `unused_ctx` — see
//! `docs/CODING_STANDARDS.md`'s "third copy of a test helper is the
//! extraction trigger" rule. This module is the one place that builds a
//! migrated in-memory SQLite [`kyomi_core::DbPool`] and a fully-populated
//! [`ToolContext`] for this crate's tool tests, so that boilerplate exists
//! exactly once.
//!
//! # What this harness covers
//!
//! [`test_pool`] runs the *real* `apps/server/migrations-sqlite` migration
//! set against a fresh `sqlite::memory:` database — every table and index a
//! production SQLite deployment would have, exercised through the actual
//! tool `execute()` path end-to-end (mirrors the pattern this crate already
//! used in `tools/catalog.rs` and `tools/watch.rs`'s `broadcast_routing`
//! tests).
//!
//! [`build_ctx`] populates every field of [`ToolContext`] with a workable
//! default: user `"user-a"` in workspace `"ws-1"`, an in-memory KV store, a
//! zeroed AES key, an *unloaded* [`kyomi_embed::LazyEmbedding`] (call
//! [`loaded_embedding`] and overwrite `ctx.embedding` for any tool whose
//! `execute()` calls `wait_ready()`), no Connect registry, and an empty
//! platform registry. Every field is `pub`, so a test that needs a
//! non-default value (a different `user_id`, `workspace_roles`, a real
//! `ws_manager` wired to a `WebSocketManager` under test) overwrites just
//! that field on the returned struct rather than reimplementing the other
//! thirteen.
//!
//! # What this harness does NOT cover
//!
//! - **Postgres.** Every pool here is SQLite; this harness does not exercise
//!   this crate's Postgres-only SQL arms (e.g. the pgvector `<=>` queries in
//!   `tools/knowledge.rs::search_knowledge_chunks`). See
//!   `kyomi_auth::test_pg` for the crate that carries the real-Postgres
//!   pattern for its own service-layer tests.
//! - **The embedding model.** `build_ctx`'s `embedding` field is unloaded by
//!   default — a tool that calls `wait_ready()` on it without a test first
//!   swapping in [`loaded_embedding`] will hang forever (no background
//!   loader is ever spawned in-process). This is deliberate: loading the
//!   real embedding model is measurably slow, so tests that don't need it
//!   shouldn't pay for it.
//! - **Multi-connection WebSocket fan-out.** `build_ctx`'s `ws_manager` is a
//!   fresh, connectionless [`kyomi_auth::websocket::WebSocketManager`] — a
//!   test asserting *what* gets broadcast (not just that a DB write
//!   succeeded) must call `.connect(user_id)` itself and read from the
//!   returned receiver, as `tools/watch.rs`'s `broadcast_routing` tests do.

use std::sync::Arc;

use sqlx::sqlite::SqlitePoolOptions;

use crate::tools::ToolContext;

/// Build a migrated in-memory SQLite [`kyomi_core::DbPool`] for tests.
///
/// Runs the real `apps/server/migrations-sqlite` migration set against a
/// fresh `sqlite::memory:` database with foreign keys enabled, so tool
/// `execute()` methods can be driven end-to-end against real schema rather
/// than a hand-rolled subset of it.
pub(crate) async fn test_pool() -> kyomi_core::DbPool {
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

    kyomi_core::DbPool::Sqlite(pool)
}

/// Build a fully-populated [`ToolContext`] over `db`, for user `"user-a"`
/// in workspace `"ws-1"`. Every field is `pub`; overwrite whichever ones a
/// given test needs to differ from these defaults (see module docs).
pub(crate) fn build_ctx(db: kyomi_core::DbPool) -> ToolContext {
    ToolContext {
        ws_manager: kyomi_auth::websocket::WebSocketManager::new(None, db.clone()),
        db,
        kv: kyomi_core::kv_store_memory::InMemoryKVStore::new_pool(),
        user_id: "user-a".to_string(),
        workspace_id: "ws-1".to_string(),
        encryption_key: Arc::new([0u8; 32]),
        embedding: kyomi_embed::LazyEmbedding::new(),
        config: Arc::new(kyomi_core::Config::test_config()),
        session_id: None,
        supports_mcp_apps: false,
        workspace_roles: Vec::new(),
        connect_registry: None,
        platforms: Arc::new(kyomi_core::platform::PlatformRegistry::new()),
        user_display_name: "User A".to_string(),
    }
}

/// A [`kyomi_embed::LazyEmbedding`] with the real model already loaded,
/// shared process-wide behind a `OnceLock` so the (measurably slow) model
/// load happens at most once per test binary run, no matter how many tests
/// across this crate need it.
///
/// Assign the result to `ctx.embedding` for any tool whose `execute()` calls
/// `ctx.embedding.wait_ready()` — `build_ctx`'s default `embedding` is
/// deliberately unloaded (see module docs) and `wait_ready()` on it hangs
/// forever in a test binary, since nothing ever calls `.set()`.
pub(crate) fn loaded_embedding() -> kyomi_embed::LazyEmbedding {
    static EMBED: std::sync::OnceLock<kyomi_embed::LazyEmbedding> = std::sync::OnceLock::new();
    EMBED
        .get_or_init(|| {
            kyomi_embed::LazyEmbedding::loaded(
                kyomi_embed::EmbeddingService::new().expect("load embedding model for tests"),
            )
        })
        .clone()
}

/// Insert a minimal `users` + `workspaces` + `workspace_users` row set: user
/// `"user-a"` owning workspace `"ws-1"` — the identities [`build_ctx`]
/// defaults to. Shared seeding step for tests across `tools/dashboard.rs`,
/// `tools/knowledge.rs`, and `tools/copilot.rs` that need a real
/// owner/workspace row to satisfy `dashboard_service`'s ownership and
/// free-tier-limit lookups. The `workspace_users` row matters even for
/// single-user tests: `WebSocketManager::broadcast_to_workspace` (which
/// every dashboard create/modify/delete broadcast goes through) looks up
/// recipients from that table, not from `workspaces.owner_user_id` — a test
/// that omits it will seed a workspace whose broadcasts silently reach
/// nobody, not even the owner.
pub(crate) async fn seed_user_and_workspace(db: &kyomi_core::DbPool) {
    let sq = match db {
        kyomi_core::DbPool::Sqlite(sq) => sq,
        kyomi_core::DbPool::Postgres(_) => unreachable!("test pool is always sqlite"),
    };

    sqlx::query("INSERT INTO users (user_id, email) VALUES ('user-a', 'a@test.local')")
        .execute(sq)
        .await
        .expect("insert user-a");
    sqlx::query(
        "INSERT INTO workspaces (workspace_id, name, owner_user_id) \
         VALUES ('ws-1', 'Workspace', 'user-a')",
    )
    .execute(sq)
    .await
    .expect("insert workspace ws-1");
    sqlx::query(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
         VALUES ('ws-1', 'user-a', 'workspace_admin', 1)",
    )
    .execute(sq)
    .await
    .expect("insert workspace_users user-a");
}
