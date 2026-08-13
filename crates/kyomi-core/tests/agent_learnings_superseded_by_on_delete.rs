// SPDX-License-Identifier: AGPL-3.0-or-later

//! Regression test for
//! `apps/server/migrations/20260814000000_fix_agent_learnings_superseded_by_on_delete.sql`
//! (KYO-346).
//!
//! `agent_learnings.superseded_by` is a self-referential FK: when a learning
//! is superseded by a newer one, `superseded_by` points at the newer row's
//! `learning_id`. The baseline
//! (`apps/server/migrations/20260215000000_baseline.sql:3302`) declared this
//! constraint `ON DELETE SET NULL` — deleting the superseding row should
//! null out `superseded_by` on whatever pointed at it, not block the delete.
//! SQLite's baseline
//! (`apps/server/migrations-sqlite/00001_baseline.sql:97`) agrees.
//!
//! `apps/server/migrations/20260315000000_uuid_columns_to_text.sql` dropped
//! `agent_learnings_superseded_by_fkey` to convert `superseded_by` from
//! `uuid` to `text`, then re-added it (lines 31-33) without the delete
//! action, so it silently defaulted to `NO ACTION`. That migration is
//! already applied to deployed databases and must not be edited — 20260814
//! is a forward-fixing migration that re-adds the constraint with
//! `ON DELETE SET NULL` restored.
//!
//! This test needs Postgres specifically (SQLite was never affected) and
//! needs to run against a database that has applied the *entire* migration
//! chain including 20260814, so it uses the same hermetic
//! create-scratch-database-and-run-the-real-chain pattern as
//! `schema_parity.rs`'s `migration_chains_produce_matching_schemas` —
//! `kyomi_core::test_db::test_database_url()` /
//! `kyomi_core::test_db::split_database_url()` to find the shared test
//! Postgres server, then a throwaway `CREATE DATABASE`/`DROP DATABASE` pair
//! so this test can never touch, or be poisoned by, the shared
//! `contract_*` test database (KYO-242).
//!
//! Unlike a unit test against the SQL text, this exercises the constraint
//! actually installed by `kyomi_core::db::DbPool::connect` — the same entry
//! point production uses — so it can only pass if the real migration chain
//! produces the fixed behavior, not just if the new file's SQL parses.

use sqlx::Row;

#[tokio::test]
async fn deleting_superseding_learning_nulls_out_superseded_by_instead_of_erroring() {
    let base_url = kyomi_core::test_db::test_database_url();
    let (server_url, _) = kyomi_core::test_db::split_database_url(&base_url);

    // KYO-242: never touch the shared contract-test database — create and
    // drop our own scratch database on the same server, exactly as
    // schema_parity.rs's migration_chains_produce_matching_schemas does.
    let scratch_db = format!("kyomi_superseded_by_fk_{}", uuid::Uuid::new_v4().simple());

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

    // Everything below is captured into plain values rather than
    // asserted/unwrapped inline, so the scratch database is dropped
    // unconditionally afterwards — including on the very failure this test
    // exists to catch (an FK violation on the DELETE).
    let outcome = run_against_migrated_database(&scratch_url).await;

    // Drop the scratch database before evaluating the outcome, so cleanup
    // runs regardless of whether the assertions below pass or fail — same
    // ordering schema_parity.rs uses and for the same reason.
    sqlx::query(&format!("DROP DATABASE \"{scratch_db}\""))
        .execute(&admin_pool)
        .await
        .unwrap_or_else(|e| panic!("drop scratch database `{scratch_db}`: {e}"));

    let TestOutcome { delete_result, superseded_by_after_delete } = outcome;

    // THE regression assertion: deleting the superseding row must succeed.
    // Before 20260814 this fails with a Postgres FK violation
    // (`update or delete on table "agent_learnings" violates foreign key
    // constraint "agent_learnings_superseded_by_fkey"`) because the
    // constraint re-added by 20260315000000_uuid_columns_to_text.sql has no
    // delete action, which defaults to NO ACTION.
    delete_result.unwrap_or_else(|e| {
        panic!(
            "DELETE of the superseding agent_learnings row must succeed via \
             ON DELETE SET NULL, not raise a foreign key violation: {e}"
        )
    });

    // The superseded row must survive the delete with superseded_by nulled
    // out — not left dangling, and not deleted itself (SET NULL, not
    // CASCADE).
    let superseded_by_after_delete = superseded_by_after_delete
        .expect("the superseded agent_learnings row must still exist after the superseding row is deleted");
    assert_eq!(
        superseded_by_after_delete, None,
        "superseded_by must be nulled out by ON DELETE SET NULL when the row it \
         pointed at is deleted"
    );
}

struct TestOutcome {
    delete_result: Result<(), sqlx::Error>,
    /// `None` if the superseded row itself no longer exists (would indicate
    /// an unexpected CASCADE); `Some(None)` is the expected passing state.
    superseded_by_after_delete: Option<Option<String>>,
}

/// Run the real migration chain against `scratch_url`, seed a superseding
/// and a superseded `agent_learnings` row, delete the superseding row, and
/// report what happened — without panicking on the failure this test is
/// designed to observe, so the caller can drop the scratch database first.
async fn run_against_migrated_database(scratch_url: &str) -> TestOutcome {
    // Run the real embedded Postgres migration chain
    // (crates/kyomi-core/src/db.rs) against the scratch database — the same
    // chain `DbPool::connect` runs in production, including
    // 20260814000000_fix_agent_learnings_superseded_by_on_delete.sql.
    let pool = kyomi_core::db::DbPool::connect(scratch_url)
        .await
        .expect("run Postgres migration chain against scratch database");
    let pg_pool = pool.pg_pool();

    // Seed the parent rows agent_learnings requires: a user (owns the
    // workspace) and a workspace (owns the learnings via
    // agent_learnings_workspace_id_fkey).
    sqlx::query("INSERT INTO users (user_id, email) VALUES ('kyo346-user', 'kyo346@example.com')")
        .execute(pg_pool)
        .await
        .expect("seed users row");

    sqlx::query(
        "INSERT INTO workspaces (workspace_id, owner_user_id) VALUES ('kyo346-ws', 'kyo346-user')",
    )
    .execute(pg_pool)
    .await
    .expect("seed workspaces row");

    // The superseding learning — the one that will be deleted.
    sqlx::query(
        "INSERT INTO agent_learnings (learning_id, workspace_id, insight) \
         VALUES ('kyo346-superseding', 'kyo346-ws', 'the newer, correct insight')",
    )
    .execute(pg_pool)
    .await
    .expect("seed superseding agent_learnings row");

    // The superseded learning — points at the superseding row via
    // superseded_by. This is the row whose superseded_by must become NULL.
    sqlx::query(
        "INSERT INTO agent_learnings (learning_id, workspace_id, insight, superseded_by) \
         VALUES ('kyo346-superseded', 'kyo346-ws', 'the older, replaced insight', 'kyo346-superseding')",
    )
    .execute(pg_pool)
    .await
    .expect("seed superseded agent_learnings row pointing at the superseding row");

    // Sanity check before the DELETE under test: confirm the FK is actually
    // wired the way the test assumes.
    let pre_delete_superseded_by: Option<String> = sqlx::query_scalar(
        "SELECT superseded_by FROM agent_learnings WHERE learning_id = 'kyo346-superseded'",
    )
    .fetch_one(pg_pool)
    .await
    .expect("fetch pre-delete superseded_by");
    assert_eq!(
        pre_delete_superseded_by.as_deref(),
        Some("kyo346-superseding"),
        "sanity check: the seeded superseded row must point at the superseding row before the delete"
    );

    // THE operation under test.
    let delete_result = sqlx::query("DELETE FROM agent_learnings WHERE learning_id = 'kyo346-superseding'")
        .execute(pg_pool)
        .await
        .map(|_| ());

    // Read back the superseded row's state regardless of whether the
    // delete succeeded, so the caller has something to report either way.
    let superseded_by_after_delete: Option<Option<String>> = sqlx::query(
        "SELECT superseded_by FROM agent_learnings WHERE learning_id = 'kyo346-superseded'",
    )
    .fetch_optional(pg_pool)
    .await
    .expect("query superseded row after delete attempt")
    .map(|row| row.get::<Option<String>, _>("superseded_by"));

    pg_pool.close().await;

    TestOutcome { delete_result, superseded_by_after_delete }
}
