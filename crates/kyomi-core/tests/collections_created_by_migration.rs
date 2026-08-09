// SPDX-License-Identifier: AGPL-3.0-or-later

//! Regression test for
//! `apps/server/migrations-sqlite/00033_fix_collections_created_by_constraints.sql`
//! (KYO-293).
//!
//! `collections.created_by` was added by
//! `00022_add_created_by_to_collections.sql` as `NOT NULL DEFAULT ''`, with
//! no foreign key — diverging from Postgres's
//! `20260609000000_add_created_by_to_collections.sql`, which backfills then
//! sets `NOT NULL` with no default plus a `collections_created_by_fkey` FK.
//! 00033 fixes this by rebuilding the `collections` table (SQLite can't
//! `ALTER COLUMN` to drop a default or add a FK to an existing column).
//!
//! That rebuild is the dangerous part: `collection_dashboards` has
//! `FOREIGN KEY (collection_id) REFERENCES collections(id) ON DELETE
//! CASCADE`, and SQLite fires that cascade during the rebuild's `DROP TABLE
//! collections` (an implicit `DELETE FROM collection_dashboards` under the
//! hood) — silently wiping every dashboard<->collection association on any
//! self-hosted install, with no error. Verified by hand against a scratch
//! database before 00033 backed its target table up: a naive
//! create-copy-drop-rename lost the row every time. This test is the
//! regression guard for that: it seeds a `collection_dashboards` row before
//! migrating and asserts it still exists after.
//!
//! It also asserts the backfill itself: 00022's `UPDATE ... SET created_by
//! = (subquery)` had no `COALESCE` fallback for a workspace with zero
//! `workspace_users` members (yielding `NULL`, which violated its own `NOT
//! NULL` — a second, unticketed divergence from Postgres's two-step
//! `COALESCE`). 00033 mirrors Postgres's COALESCE exactly, so both the
//! member and memberless paths are covered here.
//!
//! Unlike `schema_parity.rs`, this test only needs SQLite — the divergence
//! being fixed, and the migration fixing it, are both SQLite-only; Postgres
//! was already correct and untouched by 00033. `crates/kyomi-auth/src/
//! collection_service.rs` carries the "both backends reject a bad insert"
//! regression tests (`sqlite_insert_omitting_created_by_fails` and its
//! siblings) for the two failure-mode assertions the KYO-293 ticket also
//! asked for; this file only covers the migration's data-safety behavior,
//! which needs direct control over which migrations have run — something a
//! service-layer test calling `DbPool::connect` (which always runs the full
//! chain) can't express.

use std::borrow::Cow;
use std::path::Path;

use sqlx::migrate::Migrator;
use sqlx::sqlite::SqlitePoolOptions;

/// Version of `00033_fix_collections_created_by_constraints.sql`, per
/// `sqlx::migrate!`'s filename-prefix convention.
const TARGET_MIGRATION_VERSION: i64 = 33;

/// Resolve every migration in `apps/server/migrations-sqlite` at runtime
/// (not the compile-time `sqlx::migrate!` macro `db.rs`/other tests use —
/// that always embeds and runs the *full* chain, which can't stop short of
/// 00033) and return a [`Migrator`] restricted to versions `<= version_limit`.
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

#[tokio::test]
async fn migration_00033_preserves_collection_dashboards_across_the_rebuild() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&pool)
        .await
        .expect("enable foreign keys");

    // 1. Migrate to exactly the pre-00033 schema: created_by NOT NULL
    //    DEFAULT '', no FK.
    sqlite_migrator_up_to(TARGET_MIGRATION_VERSION - 1)
        .await
        .run(&pool)
        .await
        .expect("run migrations up to 00032");

    // 2. Construct the exact scenario the ticket calls out: a collection
    //    left with '' by 00022 (the earliest-workspace_users backfill can
    //    leave this behind for a memberless workspace, since 00022 had no
    //    COALESCE fallback), plus a collection_dashboards row pointing at
    //    it — the row 00033 must not lose.
    sqlx::query(
        "INSERT INTO users (user_id, email, created_at) \
         VALUES ('user-early', 'early@example.com', '2020-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("seed user");
    sqlx::query(
        "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ('ws-1', 'WS1', 'user-early')",
    )
    .execute(&pool)
    .await
    .expect("seed workspace");
    sqlx::query(
        "INSERT INTO workspace_users (workspace_id, user_id, role, created_at) \
         VALUES ('ws-1', 'user-early', 'owner', '2020-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("seed workspace_users");
    sqlx::query(
        "INSERT INTO collections (id, workspace_id, name, created_by) \
         VALUES ('coll-empty', 'ws-1', 'Empty Creator Collection', '')",
    )
    .execute(&pool)
    .await
    .expect("seed collection with '' created_by, simulating a pre-00033 database");
    sqlx::query(
        "INSERT INTO dashboards (dashboard_id, user_id, workspace_id, title) \
         VALUES ('dash-1', 'user-early', 'ws-1', 'Dash1')",
    )
    .execute(&pool)
    .await
    .expect("seed dashboard");
    sqlx::query(
        "INSERT INTO collection_dashboards (collection_id, dashboard_id) VALUES ('coll-empty', 'dash-1')",
    )
    .execute(&pool)
    .await
    .expect("seed collection_dashboards row");

    let pre_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM collection_dashboards")
        .fetch_one(&pool)
        .await
        .expect("count collection_dashboards before migrating");
    assert_eq!(pre_count, 1, "sanity check: the fixture row must exist before migrating");

    // 3. Apply 00033.
    sqlite_migrator_up_to(TARGET_MIGRATION_VERSION)
        .await
        .run(&pool)
        .await
        .expect("run migration 00033 against a database containing a pre-existing '' row");

    // 4. The '' row must be backfilled to the earliest workspace_users
    //    member, mirroring Postgres's COALESCE.
    let created_by: String =
        sqlx::query_scalar("SELECT created_by FROM collections WHERE id = 'coll-empty'")
            .fetch_one(&pool)
            .await
            .expect("fetch backfilled created_by");
    assert_eq!(
        created_by, "user-early",
        "'' must be backfilled to the earliest workspace_users member, not left empty"
    );

    // 5. THE regression assertion: collection_dashboards must have
    //    survived the collections table rebuild. Without the temp-table
    //    backup/restore, DROP TABLE collections' implicit cascade delete
    //    wipes this row silently (no error) — this is the only assertion
    //    in this suite that would catch that regression reappearing.
    let post_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM collection_dashboards")
        .fetch_one(&pool)
        .await
        .expect("count collection_dashboards after migrating");
    assert_eq!(
        post_count, 1,
        "collection_dashboards row must survive the collections table rebuild \
         (regression: ON DELETE CASCADE fires during DROP TABLE collections)"
    );

    let (collection_id, dashboard_id): (String, String) =
        sqlx::query_as("SELECT collection_id, dashboard_id FROM collection_dashboards")
            .fetch_one(&pool)
            .await
            .expect("fetch the surviving collection_dashboards row");
    assert_eq!(collection_id, "coll-empty");
    assert_eq!(dashboard_id, "dash-1");

    // 6. No dangling FK left behind by the rebuild.
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .expect("run foreign_key_check");
    assert!(
        violations.is_empty(),
        "migration must leave no FK violations, found {} row(s)",
        violations.len()
    );
}

#[tokio::test]
async fn migration_00033_backfills_memberless_workspace_via_earliest_user_fallback() {
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
        .expect("run migrations up to 00032");

    // Two users, an earlier one and a later one, and a workspace with
    // *zero* workspace_users rows — 00022's backfill had no fallback for
    // this case and would have left created_by = '' forever (or, on a
    // NULL-yielding subquery, violated its own NOT NULL at UPDATE time).
    // 00033 must fall back to the earliest user overall, exactly like
    // Postgres's `COALESCE(..., (SELECT user_id FROM users ORDER BY
    // created_at ASC LIMIT 1))`.
    sqlx::query(
        "INSERT INTO users (user_id, email, created_at) \
         VALUES ('user-earliest', 'earliest@example.com', '2019-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("seed earliest user");
    sqlx::query(
        "INSERT INTO users (user_id, email, created_at) \
         VALUES ('user-later', 'later@example.com', '2021-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("seed later user");
    sqlx::query(
        "INSERT INTO workspaces (workspace_id, name, owner_user_id) \
         VALUES ('ws-memberless', 'WS Memberless', 'user-earliest')",
    )
    .execute(&pool)
    .await
    .expect("seed memberless workspace");
    sqlx::query(
        "INSERT INTO collections (id, workspace_id, name, created_by) \
         VALUES ('coll-memberless', 'ws-memberless', 'Memberless', '')",
    )
    .execute(&pool)
    .await
    .expect("seed collection in a workspace with no workspace_users rows");

    sqlite_migrator_up_to(TARGET_MIGRATION_VERSION)
        .await
        .run(&pool)
        .await
        .expect("run migration 00033 against a memberless workspace");

    let created_by: String =
        sqlx::query_scalar("SELECT created_by FROM collections WHERE id = 'coll-memberless'")
            .fetch_one(&pool)
            .await
            .expect("fetch backfilled created_by");
    assert_eq!(
        created_by, "user-earliest",
        "a memberless workspace's collection must fall back to the earliest user overall, \
         not the later one and not stay empty"
    );
}
