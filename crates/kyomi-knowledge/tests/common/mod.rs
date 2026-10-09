// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared scratch-Postgres test harness (KYO-242 pattern) for
//! `kyomi-knowledge` integration tests.
//!
//! Home of the create-scratch-database / migrate / seed-workspace machinery
//! for this crate's integration tests, starting with KYO-809's
//! `expansion_leaf_failures_are_observable.rs`. Living here from the start
//! (rather than inline in a single test file) means a second consuming test
//! file never has to duplicate it or extract it later. A `tests/common/mod.rs`
//! (directory + `mod.rs`, as opposed to a plain `tests/common.rs`) is the
//! standard way to share code between Rust integration test binaries without
//! cargo treating this file as a test binary of its own -- every other `.rs`
//! file directly under `tests/` is compiled and run as an independent test
//! crate, but `tests/common/` is not auto-discovered, so each consumer opts
//! in explicitly with `mod common;`.

use kyomi_core::db::DbPool;
use std::future::Future;

/// Create a throwaway scratch Postgres database, run the real migration chain
/// against it, seed a user + workspace, hand `(db, workspace_id)` to `body`, then
/// drop the scratch database -- regardless of whether `body` panics, so a failing
/// assertion can never leak a database (KYO-242).
///
/// Mirrors `agent_learnings_superseded_by_on_delete.rs` / `schema_parity.rs`'s
/// create-scratch-database pattern.
pub async fn with_scratch_workspace<F, Fut, T>(test_name: &str, body: F) -> T
where
    F: FnOnce(DbPool, String) -> Fut,
    Fut: Future<Output = T>,
{
    let base_url = kyomi_core::test_db::test_database_url();
    let (server_url, _) = kyomi_core::test_db::split_database_url(&base_url);

    let scratch_db = format!("kyoknow_{test_name}_{}", uuid::Uuid::new_v4().simple());

    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&format!("{server_url}/postgres"))
        .await
        .unwrap_or_else(|e| {
            panic!(
                "connect to Postgres admin database at {server_url}/postgres \
                 (is the test Postgres container running? see CLAUDE.md): {e}"
            )
        });

    sqlx::query(&format!("CREATE DATABASE \"{scratch_db}\""))
        .execute(&admin_pool)
        .await
        .unwrap_or_else(|e| panic!("create scratch database `{scratch_db}`: {e}"));

    let scratch_url = format!("{server_url}/{scratch_db}");

    // Run the real embedded Postgres migration chain (crates/kyomi-core/src/db.rs)
    // against the scratch database -- the same entry point `DbPool::connect` gives
    // production, so this can only pass if the real migrated schema supports the
    // query under test, not just if the SQL text parses.
    let db = DbPool::connect(&scratch_url)
        .await
        .expect("run Postgres migration chain against scratch database");

    let workspace_id = format!("kyoknow-ws-{test_name}");
    let user_id = format!("kyoknow-user-{test_name}");

    sqlx::query("INSERT INTO users (user_id, email) VALUES ($1, $2)")
        .bind(&user_id)
        .bind(format!("{user_id}@example.com"))
        .execute(db.pg_pool())
        .await
        .expect("seed users row");

    sqlx::query("INSERT INTO workspaces (workspace_id, owner_user_id) VALUES ($1, $2)")
        .bind(&workspace_id)
        .bind(&user_id)
        .execute(db.pg_pool())
        .await
        .expect("seed workspaces row");

    // Run the test body, capturing its outcome without panicking on it, so the
    // scratch database is dropped below regardless of whether the caller's
    // assertions (made after this function returns) pass or fail.
    let outcome = body(db.clone(), workspace_id).await;

    db.pg_pool().close().await;

    // `WITH (FORCE)` terminates any lingering connections so the drop can't
    // itself fail with "database is being accessed by other users" -- same
    // rationale as `kyomi_core::test_db::recreate_database`.
    sqlx::query(&format!("DROP DATABASE \"{scratch_db}\" WITH (FORCE)"))
        .execute(&admin_pool)
        .await
        .unwrap_or_else(|e| panic!("drop scratch database `{scratch_db}`: {e}"));

    outcome
}

/// Insert an `agent_learnings` row and return its `learning_id`.
///
/// `learning_id` is bound explicitly (as a `uuid::Uuid::new_v4()` string) rather
/// than left to a column default: `apps/server/migrations/20260315000000_uuid_columns_to_text.sql`
/// converted `learning_id` from `uuid DEFAULT gen_random_uuid()` to `TEXT` and
/// dropped the default, so callers must supply it -- exactly as production code does.
pub async fn seed_learning(
    db: &DbPool,
    workspace_id: &str,
    insight: &str,
    enabled: bool,
    superseded_by: Option<&str>,
    structured_metadata: Option<serde_json::Value>,
) -> String {
    let learning_id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO agent_learnings \
           (learning_id, workspace_id, insight, enabled, superseded_by, structured_metadata) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&learning_id)
    .bind(workspace_id)
    .bind(insight)
    .bind(enabled)
    .bind(superseded_by)
    .bind(structured_metadata)
    .execute(db.pg_pool())
    .await
    .expect("seed agent_learnings row");

    learning_id
}

// Deliberately NOT shared here: helpers that seed `datasource_table_cache`,
// `column_embeddings`, or `learning_references` directly are used by only one
// consumer (`expansion_leaf_failures_are_observable.rs`, KYO-809) -- adding
// them to this shared module would make them dead code (and thus a `-D
// warnings` clippy failure, see `scripts/preflight-clippy.sh`) in every other
// test binary that pulls in `mod common;` but never calls them. Only
// `with_scratch_workspace` and `seed_learning` above are used by more than
// one test file today; a helper earns a place here when a second consumer
// actually needs it, not preemptively.
