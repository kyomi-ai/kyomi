// SPDX-License-Identifier: AGPL-3.0-or-later

//! Regression test for
//! `apps/server/migrations-sqlite/00037_verify_pre_existing_unverified_users.sql`
//! (KYO-683 phase 1, database half).
//!
//! Before this ticket, SaaS email signup wrote an unverified `users` row
//! immediately and only flipped `verified` to true once the address was
//! confirmed. That flow is removed elsewhere in this ticket (a `users` row
//! is now created with `verified = true` only when the verification token
//! is redeemed), but it left behind unverified rows that are permanently
//! stranded under the new flow: they squat `users.email`'s UNIQUE index, so
//! re-signup for that address can't take a clean path, and
//! `recovery_start_service` (`crates/kyomi-auth/src/auth_service.rs`)
//! refuses `/account/recover` for any `!verified` user, so recovery can't
//! self-heal them either.
//!
//! 00037 reconciles this by force-verifying every unverified row in place
//! and deleting nothing — see the migration file and
//! `docs/standards/data-state-management/enumerate-every-referencing-table-before-an-irreversible-migration.md`
//! for why deletion was rejected (`users(user_id)` has 26 declared foreign
//! keys across 25 tables in this SQLite chain, plus unenforced `_by`
//! columns like `dashboards.updated_by` with no `REFERENCES` behind them at
//! all).
//!
//! This test seeds a realistic mix of verified/unverified rows before
//! migrating — including a dependent row in a referencing table on one of
//! the unverified users, to prove the row (and its dependent) survive
//! rather than being deleted — then asserts:
//!
//!   * zero rows remain with `verified = 0`;
//!   * the row count is unchanged (nothing was deleted);
//!   * the dependent row still exists;
//!   * every column on the touched rows is byte-identical to what was
//!     seeded, except `updated_at` (the one column the migration is
//!     allowed to touch) — this is the assertion that actually proves the
//!     migration didn't clobber anything it wasn't supposed to;
//!   * `PRAGMA foreign_key_check` reports no violations.
//!
//! A second test proves idempotence: running the chain to 37 against a
//! database with zero unverified rows leaves every row, including
//! `updated_at`, untouched.
//!
//! Unlike `schema_parity.rs`, this only needs SQLite: the migration is a
//! plain data-only `UPDATE` identical in shape on both dialects, and the
//! Postgres file was reviewed by inspection (see the PR body) rather than
//! executed, since this test suite has no Postgres-backed harness for a
//! single-migration-chain run the way it does for SQLite.

use std::borrow::Cow;
use std::path::Path;

use sqlx::migrate::Migrator;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::Row;

/// Version of `00037_verify_pre_existing_unverified_users.sql`, per
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

/// Snapshot of every `users` column this migration must leave untouched
/// (i.e. every column except `updated_at`, which the migration is allowed
/// to bump on rows it force-verifies).
#[derive(Debug, PartialEq, Eq)]
struct UserSnapshot {
    user_id: String,
    email: String,
    name: Option<String>,
    created_at: String,
    last_login: Option<String>,
    active: i64,
    verified: i64,
    terms_accepted_at: Option<String>,
    terms_accepted_version: Option<String>,
    marketing_consent: i64,
    oauth_data: Option<String>,
    extra_metadata: Option<String>,
    chartml_config: Option<String>,
    last_workspace_id: Option<String>,
    knowledge: Option<String>,
    billing_project: Option<String>,
    default_project: Option<String>,
    query_size_limit_gb: i64,
}

async fn fetch_user_snapshot(pool: &sqlx::SqlitePool, user_id: &str) -> UserSnapshot {
    let row = sqlx::query(
        "SELECT user_id, email, name, created_at, last_login, active, verified, \
         terms_accepted_at, terms_accepted_version, marketing_consent, oauth_data, \
         extra_metadata, chartml_config, last_workspace_id, knowledge, billing_project, \
         default_project, query_size_limit_gb FROM users WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("fetch snapshot for {user_id}: {e}"));

    UserSnapshot {
        user_id: row.get("user_id"),
        email: row.get("email"),
        name: row.get("name"),
        created_at: row.get("created_at"),
        last_login: row.get("last_login"),
        active: row.get("active"),
        verified: row.get("verified"),
        terms_accepted_at: row.get("terms_accepted_at"),
        terms_accepted_version: row.get("terms_accepted_version"),
        marketing_consent: row.get("marketing_consent"),
        oauth_data: row.get("oauth_data"),
        extra_metadata: row.get("extra_metadata"),
        chartml_config: row.get("chartml_config"),
        last_workspace_id: row.get("last_workspace_id"),
        knowledge: row.get("knowledge"),
        billing_project: row.get("billing_project"),
        default_project: row.get("default_project"),
        query_size_limit_gb: row.get("query_size_limit_gb"),
    }
}

/// Seed the three-user fixture used by both tests: two unverified users
/// (one with a dependent `workspaces` row, one with a dependent
/// `refresh_tokens` row) and one already-verified user, each with distinct
/// `name`/`email`/`terms_accepted_at`/`created_at` so a snapshot comparison
/// can't pass by accident from two rows sharing a value.
async fn seed_fixture(pool: &sqlx::SqlitePool) {
    sqlx::query(
        "INSERT INTO users \
         (user_id, email, name, created_at, updated_at, verified, terms_accepted_at, \
          terms_accepted_version) \
         VALUES ('user-unverified-1', 'alice@example.com', 'Alice Unverified', \
                 '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 0, \
                 '2024-01-01T00:00:00Z', '2024-01-01')",
    )
    .execute(pool)
    .await
    .expect("seed user-unverified-1");

    sqlx::query(
        "INSERT INTO users \
         (user_id, email, name, created_at, updated_at, verified, terms_accepted_at, \
          terms_accepted_version) \
         VALUES ('user-unverified-2', 'bob@example.com', 'Bob Unverified', \
                 '2024-02-02T00:00:00Z', '2024-02-02T00:00:00Z', 0, NULL, NULL)",
    )
    .execute(pool)
    .await
    .expect("seed user-unverified-2");

    sqlx::query(
        "INSERT INTO users \
         (user_id, email, name, created_at, updated_at, verified, terms_accepted_at, \
          terms_accepted_version) \
         VALUES ('user-verified-1', 'carol@example.com', 'Carol Verified', \
                 '2024-03-03T00:00:00Z', '2024-03-03T00:00:00Z', 1, \
                 '2024-03-03T00:00:00Z', '2024-03-03')",
    )
    .execute(pool)
    .await
    .expect("seed user-verified-1");

    // Dependent row on an *unverified* user via a declared, enforced FK —
    // this is what proves the migration doesn't delete the row it's
    // reconciling. workspaces.owner_user_id is NOT NULL REFERENCES
    // users(user_id).
    sqlx::query(
        "INSERT INTO workspaces (workspace_id, name, owner_user_id) \
         VALUES ('ws-owned-by-unverified', 'Bob''s Workspace', 'user-unverified-2')",
    )
    .execute(pool)
    .await
    .expect("seed dependent workspace for user-unverified-2");

    // Second dependent row, on the other unverified user, via a different
    // referencing table (refresh_tokens.user_id), post-00034's
    // family_id NOT NULL rebuild.
    sqlx::query(
        "INSERT INTO refresh_tokens \
         (token_id, user_id, token_hash, expires_at, family_id) \
         VALUES ('rt-unverified-1', 'user-unverified-1', 'hash-1', \
                 '2030-01-01T00:00:00Z', 'rt-unverified-1')",
    )
    .execute(pool)
    .await
    .expect("seed dependent refresh_token for user-unverified-1");
}

#[tokio::test]
async fn migration_00037_force_verifies_unverified_rows_without_deleting_anything() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .expect("enable foreign keys");

    sqlite_migrator_up_to(TARGET_MIGRATION_VERSION - 1)
        .await
        .run(&pool)
        .await
        .expect("run migrations up to 00036");

    seed_fixture(&pool).await;

    let pre_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .expect("count users before migrating");
    assert_eq!(pre_count, 3, "sanity check: all three fixture rows must exist before migrating");

    let pre_unverified_1 = fetch_user_snapshot(&pool, "user-unverified-1").await;
    let pre_unverified_2 = fetch_user_snapshot(&pool, "user-unverified-2").await;
    let pre_verified_1 = fetch_user_snapshot(&pool, "user-verified-1").await;
    let pre_updated_at_verified_1: String =
        sqlx::query_scalar("SELECT updated_at FROM users WHERE user_id = 'user-verified-1'")
            .fetch_one(&pool)
            .await
            .expect("fetch pre-migration updated_at for user-verified-1");

    // Apply 00037.
    sqlite_migrator_up_to(TARGET_MIGRATION_VERSION)
        .await
        .run(&pool)
        .await
        .expect("run migration 00037");

    // 1. Zero rows remain unverified.
    let remaining_unverified: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE verified = 0")
            .fetch_one(&pool)
            .await
            .expect("count remaining unverified users");
    assert_eq!(remaining_unverified, 0, "no user row may remain unverified after 00037");

    // 2. Nothing was deleted.
    let post_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .expect("count users after migrating");
    assert_eq!(post_count, pre_count, "00037 must not delete any user row");

    // 3. The previously-unverified rows are now verified, and every other
    //    column is byte-identical to what was seeded — this is the
    //    assertion that proves the migration didn't clobber anything else.
    let post_unverified_1 = fetch_user_snapshot(&pool, "user-unverified-1").await;
    assert_eq!(post_unverified_1.verified, 1, "user-unverified-1 must be force-verified");
    assert_eq!(
        UserSnapshot { verified: 1, ..pre_unverified_1 },
        post_unverified_1,
        "only `verified` (and updated_at, checked separately) may change on user-unverified-1"
    );

    let post_unverified_2 = fetch_user_snapshot(&pool, "user-unverified-2").await;
    assert_eq!(post_unverified_2.verified, 1, "user-unverified-2 must be force-verified");
    assert_eq!(
        UserSnapshot { verified: 1, ..pre_unverified_2 },
        post_unverified_2,
        "only `verified` (and updated_at, checked separately) may change on user-unverified-2"
    );

    // 4. The already-verified control row is completely untouched,
    //    including updated_at — it never matched `WHERE verified = false`.
    let post_verified_1 = fetch_user_snapshot(&pool, "user-verified-1").await;
    assert_eq!(
        pre_verified_1, post_verified_1,
        "an already-verified row must not be touched at all by 00037"
    );
    let post_updated_at_verified_1: String =
        sqlx::query_scalar("SELECT updated_at FROM users WHERE user_id = 'user-verified-1'")
            .fetch_one(&pool)
            .await
            .expect("fetch post-migration updated_at for user-verified-1");
    assert_eq!(
        pre_updated_at_verified_1, post_updated_at_verified_1,
        "updated_at on an already-verified row must not change"
    );

    // 5. Both dependent rows survive — the migration deletes nothing.
    let workspace_owner: String = sqlx::query_scalar(
        "SELECT owner_user_id FROM workspaces WHERE workspace_id = 'ws-owned-by-unverified'",
    )
    .fetch_one(&pool)
    .await
    .expect("dependent workspace row must survive 00037");
    assert_eq!(workspace_owner, "user-unverified-2");

    let refresh_token_user: String = sqlx::query_scalar(
        "SELECT user_id FROM refresh_tokens WHERE token_id = 'rt-unverified-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("dependent refresh_tokens row must survive 00037");
    assert_eq!(refresh_token_user, "user-unverified-1");

    // 6. No dangling FK left behind.
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
async fn migration_00037_is_idempotent_when_no_unverified_rows_remain() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .expect("enable foreign keys");

    sqlite_migrator_up_to(TARGET_MIGRATION_VERSION - 1)
        .await
        .run(&pool)
        .await
        .expect("run migrations up to 00036");

    // A database with no unverified rows at all — e.g. one created fresh
    // under the post-KYO-683 signup flow, or one that already ran 00037
    // once. Every column, including updated_at, must be untouched by a
    // second application of the `UPDATE ... WHERE verified = false`.
    sqlx::query(
        "INSERT INTO users \
         (user_id, email, name, created_at, updated_at, verified, terms_accepted_at, \
          terms_accepted_version) \
         VALUES ('user-already-verified', 'dana@example.com', 'Dana Verified', \
                 '2024-04-04T00:00:00Z', '2024-04-04T00:00:00Z', 1, \
                 '2024-04-04T00:00:00Z', '2024-04-04')",
    )
    .execute(&pool)
    .await
    .expect("seed already-verified user");

    let pre = fetch_user_snapshot(&pool, "user-already-verified").await;
    let pre_updated_at: String =
        sqlx::query_scalar("SELECT updated_at FROM users WHERE user_id = 'user-already-verified'")
            .fetch_one(&pool)
            .await
            .expect("fetch pre-migration updated_at");

    sqlite_migrator_up_to(TARGET_MIGRATION_VERSION)
        .await
        .run(&pool)
        .await
        .expect("run migration 00037 against a database with no unverified rows");

    let post = fetch_user_snapshot(&pool, "user-already-verified").await;
    assert_eq!(pre, post, "00037 must be a no-op when no row has verified = 0");
    let post_updated_at: String =
        sqlx::query_scalar("SELECT updated_at FROM users WHERE user_id = 'user-already-verified'")
            .fetch_one(&pool)
            .await
            .expect("fetch post-migration updated_at");
    assert_eq!(pre_updated_at, post_updated_at, "updated_at must not change on a no-op run");
}
