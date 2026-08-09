// SPDX-License-Identifier: AGPL-3.0-or-later

//! Regression test for
//! `apps/server/migrations-sqlite/00034_refresh_tokens_family_id_not_null.sql`
//! (KYO-294).
//!
//! `refresh_tokens.family_id` was added by
//! `00003_refresh_token_rotation.sql` as a nullable `TEXT` column, backfilled
//! (`family_id = token_id` for every pre-existing row) but never given a
//! `NOT NULL` constraint — diverging from Postgres's
//! `20260216121703_add_refresh_token_rotation.sql`, which runs the identical
//! backfill and then `ALTER COLUMN family_id SET NOT NULL`. 00034 fixes this
//! by rebuilding the `refresh_tokens` table (SQLite can't `ALTER COLUMN` to
//! add `NOT NULL` to an existing column).
//!
//! Unlike KYO-293's `collections` rebuild, no other table references
//! `refresh_tokens` via a foreign key (verified against the full
//! accumulated schema), so there is no `ON DELETE CASCADE` data-loss trap
//! and no temp-table backup/restore step is needed here.
//!
//! This test constructs the exact scenario the ticket calls out: a row left
//! with `family_id = NULL` by a pre-00034 database (the state 00003 alone
//! produces before any application code writes to the row — the migration
//! must not assume every row has already been backfilled by application
//! logic), and asserts it survives 00034 with `family_id` backfilled to its
//! own `token_id`, mirroring Postgres's rule exactly.
//!
//! Unlike `schema_parity.rs`, this test only needs SQLite — the divergence
//! being fixed, and the migration fixing it, are both SQLite-only; Postgres
//! was already correct and untouched by 00034. `crates/kyomi-auth/src/
//! token_service.rs` carries the "both backends reject a bad insert"
//! regression tests for the two failure-mode assertions (an INSERT omitting
//! `family_id` must fail on both backends); this file only covers the
//! migration's backfill-and-rebuild behavior, which needs direct control
//! over which migrations have run — something a service-layer test calling
//! `DbPool::connect` (which always runs the full chain) can't express.

use std::borrow::Cow;
use std::path::Path;

use sqlx::migrate::Migrator;
use sqlx::sqlite::SqlitePoolOptions;

/// Version of `00034_refresh_tokens_family_id_not_null.sql`, per
/// `sqlx::migrate!`'s filename-prefix convention.
const TARGET_MIGRATION_VERSION: i64 = 34;

/// Resolve every migration in `apps/server/migrations-sqlite` at runtime
/// (not the compile-time `sqlx::migrate!` macro `db.rs`/other tests use —
/// that always embeds and runs the *full* chain, which can't stop short of
/// 00034) and return a [`Migrator`] restricted to versions `<= version_limit`.
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
async fn migration_00034_backfills_null_family_id_and_survives_rebuild() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&pool)
        .await
        .expect("enable foreign keys");

    // 1. Migrate to exactly the pre-00034 schema: family_id nullable, no
    //    NOT NULL constraint.
    sqlite_migrator_up_to(TARGET_MIGRATION_VERSION - 1)
        .await
        .run(&pool)
        .await
        .expect("run migrations up to 00033");

    // 2. Construct the exact scenario the ticket calls out: a refresh_tokens
    //    row with family_id left NULL, plus every other NOT NULL column
    //    populated so the insert succeeds against the pre-00034 schema.
    sqlx::query(
        "INSERT INTO users (user_id, email, created_at) \
         VALUES ('user-1', 'user1@example.com', '2020-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("seed user");
    sqlx::query(
        "INSERT INTO refresh_tokens \
         (token_id, user_id, token_hash, expires_at, is_active, created_at) \
         VALUES ('rt-null-family', 'user-1', 'hash-1', '2030-01-01T00:00:00Z', 1, '2020-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("seed refresh_tokens row with NULL family_id, simulating a pre-00034 database");

    let pre_family_id: Option<String> =
        sqlx::query_scalar("SELECT family_id FROM refresh_tokens WHERE token_id = 'rt-null-family'")
            .fetch_one(&pool)
            .await
            .expect("fetch pre-migration family_id");
    assert_eq!(
        pre_family_id, None,
        "sanity check: the fixture row must have NULL family_id before migrating"
    );

    // 3. Apply 00034.
    sqlite_migrator_up_to(TARGET_MIGRATION_VERSION)
        .await
        .run(&pool)
        .await
        .expect("run migration 00034 against a database containing a pre-existing NULL family_id row");

    // 4. THE regression assertion: the NULL row must survive the rebuild,
    //    backfilled to its own token_id — mirroring Postgres's
    //    `UPDATE refresh_tokens SET family_id = token_id WHERE family_id IS NULL`.
    let (token_id, family_id): (String, String) = sqlx::query_as(
        "SELECT token_id, family_id FROM refresh_tokens WHERE token_id = 'rt-null-family'",
    )
    .fetch_one(&pool)
    .await
    .expect("fetch the surviving refresh_tokens row after migrating");
    assert_eq!(token_id, "rt-null-family");
    assert_eq!(
        family_id, "rt-null-family",
        "a NULL family_id must be backfilled to the row's own token_id, mirroring Postgres's rule"
    );

    // 5. family_id must now reject NULL at the schema level.
    let result = sqlx::query(
        "INSERT INTO refresh_tokens \
         (token_id, user_id, token_hash, expires_at, is_active, created_at) \
         VALUES ('rt-no-family', 'user-1', 'hash-2', '2030-01-01T00:00:00Z', 1, '2020-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await;
    assert!(
        result.is_err(),
        "omitting family_id must be rejected by the NOT NULL constraint after 00034, got: {result:?}"
    );

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
