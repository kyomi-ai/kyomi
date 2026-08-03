// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-worktree test database provisioning (KYO-242).
//!
//! `apps/server/tests/contract_*.rs` and [`crate::config::Config::test_config`]
//! historically pointed every worktree on the machine at ONE long-lived
//! Postgres database (`postgres://kyomi_test:test@localhost:5434/kyomi_test`,
//! in the shared `kyomi-postgres-test` container) and ran `sqlx::migrate!`
//! against it. Because that container is shared across every git worktree,
//! one worktree applying a migration that hasn't been merged poisons every
//! other worktree's test suite: the poisoned worktree fails every test at
//! startup with a bare `Migrate(VersionMissing(<version>))` naming a
//! migration it has never heard of, and recovery requires manually dropping
//! the shared database — something nobody guesses from that error.
//!
//! This module gives each worktree its own private database, derived
//! deterministically from the crate's manifest path (see
//! [`test_database_url`] / [`derive_test_db_name`]), and self-heals that
//! private database if a previously-checked-out branch left behind
//! migrations the current branch doesn't recognize (see
//! [`connect_test_pool`]). `DATABASE_URL`, when set, always overrides this
//! scheme and disables the self-heal — an explicit override is never ours to
//! destroy.
//!
//! Provisioning (create-if-absent, migrate, heal) is serialized with a
//! Postgres advisory lock (see [`advisory_lock_key`]). Every `#[tokio::test]`
//! in a `contract_*` binary calls [`connect_test_pool`] independently and the
//! test harness runs them on parallel threads, so without a lock, one
//! caller's heal — which force-drops the database out from under everyone —
//! can land while other callers are mid-`DbPool::connect`, trading a clear
//! `VersionMissing` for a much more confusing `3D000: database ... does not
//! exist`. The lock is server-wide (Postgres tracks advisory locks by
//! session, across client processes, not just within one), so it also
//! serializes separate test binaries and worktrees racing on the same
//! database name.

use sha2::{Digest, Sha256};
use sqlx::postgres::{PgConnection, PgPoolOptions};

use crate::db::DbPool;

/// Server (no database name) that worktree-derived test databases live on.
const DEFAULT_TEST_SERVER: &str = "postgres://kyomi_test:test@localhost:5434";

/// Suffix stripped from `CARGO_MANIFEST_DIR` to recover the workspace root.
const MANIFEST_SUFFIX: &str = "/crates/kyomi-core";

/// Cap on the human-readable portion of a derived database name, so
/// `kyomi_test_<basename>_<hash>` always stays comfortably under Postgres's
/// 63-byte identifier limit.
const MAX_BASENAME_LEN: usize = 24;

/// Resolve the Postgres URL this test binary should connect to.
///
/// If `DATABASE_URL` is set, it is returned verbatim — an explicit override
/// always wins (CI, manual debugging against a specific database). Otherwise
/// each worktree gets its own private database on the shared test Postgres
/// server (`localhost:5434`), named deterministically from this crate's
/// manifest directory so two worktrees never collide and the same worktree
/// always reconnects to the same database.
pub fn test_database_url() -> String {
    if let Ok(url) = std::env::var("DATABASE_URL") {
        return url;
    }
    let db_name = derive_test_db_name(env!("CARGO_MANIFEST_DIR"));
    format!("{DEFAULT_TEST_SERVER}/{db_name}")
}

/// Split a `postgres://.../dbname` URL into its server part (scheme, auth,
/// host, port) and database name.
///
/// Shared between [`connect_test_pool`] and `crates/kyomi-core/tests/schema_parity.rs`'s
/// scratch-database setup so the two can never drift apart.
///
/// # Panics
/// Panics if `url` has no `/` after the authority, i.e. isn't a full
/// `postgres://.../dbname` URL.
pub fn split_database_url(url: &str) -> (&str, &str) {
    let slash = url.rfind('/').unwrap_or_else(|| {
        panic!(
            "database URL must be of the form postgres://user:pass@host:port/dbname, got: {url}"
        )
    });
    (&url[..slash], &url[slash + 1..])
}

/// Derive a stable, readable, collision-resistant Postgres database name for
/// the workspace whose `kyomi-core` crate lives at `manifest_dir` (expected
/// to be `kyomi-core`'s own `CARGO_MANIFEST_DIR`, i.e.
/// `<workspace_root>/crates/kyomi-core`).
///
/// The name is `kyomi_test_<sanitized-basename>_<hash>`:
/// - `<sanitized-basename>` is the workspace root's final path component,
///   lowercased, with every non-alphanumeric byte replaced by `_`, capped at
///   [`MAX_BASENAME_LEN`] characters — deliberately readable, so a developer
///   running `\l` in `psql` can tell at a glance which worktree owns which
///   database.
/// - `<hash>` is the first 8 hex characters of `sha256(workspace_root)` —
///   guarantees no collision between same-named checkouts under different
///   parent directories (e.g. two machines both naming a worktree `kyomi`).
///
/// Pure function — no I/O, no database access — so it's unit-testable
/// without a running Postgres.
fn derive_test_db_name(manifest_dir: &str) -> String {
    let workspace_root = manifest_dir.strip_suffix(MANIFEST_SUFFIX).unwrap_or(manifest_dir);

    let basename = workspace_root
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("workspace");

    let mut sanitized = String::with_capacity(MAX_BASENAME_LEN);
    for c in basename.chars().take(MAX_BASENAME_LEN) {
        sanitized.push(if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' });
    }
    if sanitized.is_empty() {
        sanitized.push_str("workspace");
    }

    let hash = Sha256::digest(workspace_root.as_bytes());
    let hash_suffix = hex::encode(&hash[..4]);

    format!("kyomi_test_{sanitized}_{hash_suffix}")
}

/// Postgres advisory-lock key that serializes all provisioning work for
/// `db_name` — see the module docs for why this is needed.
///
/// Derived by hashing `db_name` directly, rather than reusing
/// `derive_test_db_name`'s internal hash, because this key must also cover
/// the `DATABASE_URL`-override path, where `derive_test_db_name` is never
/// called.
fn advisory_lock_key(db_name: &str) -> i64 {
    let digest = Sha256::digest(db_name.as_bytes());
    i64::from_be_bytes(digest[..8].try_into().expect("sha256 digest is at least 8 bytes"))
}

/// `true` if `err` is the shape of failure `sqlx::migrate!` produces when the
/// database's `_sqlx_migrations` table records a migration this branch's
/// migration source doesn't contain (or contains in modified/dirty form) —
/// i.e. exactly the "a different branch migrated this database" signature,
/// as opposed to a connectivity failure, permissions error, or an actual
/// migration bug in the current branch's own chain.
fn is_migration_mismatch(err: &crate::Error) -> bool {
    matches!(
        err,
        crate::Error::Migrate(
            sqlx::migrate::MigrateError::VersionMissing(_)
                | sqlx::migrate::MigrateError::VersionMismatch(_)
                | sqlx::migrate::MigrateError::Dirty(_)
        )
    )
}

/// Create `db_name` on `conn`'s server if it doesn't already exist.
///
/// Tolerant of a race, as defense in depth even though callers going through
/// [`connect_test_pool`]'s advisory lock should never trigger it: Postgres
/// documents `42P04` (`duplicate_database`) for "already exists", but under
/// true concurrency two racing `CREATE DATABASE` statements are resolved via
/// a unique index on the `pg_database` catalog, which surfaces as `23505`
/// (`unique_violation`) instead — observed directly while testing this
/// module's self-heal path pre-lock. Both mean the same thing here: someone
/// else already created it.
async fn create_database_if_absent(conn: &mut PgConnection, db_name: &str) -> crate::Result<()> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(db_name)
            .fetch_one(&mut *conn)
            .await?;

    if exists {
        return Ok(());
    }

    // Postgres doesn't support binding identifiers as query parameters, so
    // the database name is interpolated directly — the same trust boundary
    // `schema_parity.rs`'s scratch-database creation already relies on.
    // `db_name` is either sanitized to `[a-z0-9_]` by `derive_test_db_name`,
    // or comes from a developer-supplied `DATABASE_URL` env var — never from
    // untrusted request input.
    match sqlx::query(&format!(r#"CREATE DATABASE "{db_name}""#)).execute(&mut *conn).await {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(e))
            if matches!(e.code().as_deref(), Some("42P04") | Some("23505")) =>
        {
            Ok(())
        }
        Err(e) => Err(crate::Error::Sqlx(e)),
    }
}

/// Drop and recreate `db_name` on `conn`'s server.
///
/// `WITH (FORCE)` terminates any lingering connections (e.g. from the pool
/// that just failed to migrate) so the drop can't itself fail with
/// "database is being accessed by other users".
///
/// # Safety
/// Callers must only invoke this for a database name this module derived
/// itself via [`derive_test_db_name`] — never for a `DATABASE_URL` the
/// caller supplied. Multiple agents and CI runs share this Postgres server
/// concurrently; dropping a database we don't own trades a confusing test
/// failure for a much worse one.
async fn recreate_database(conn: &mut PgConnection, db_name: &str) -> crate::Result<()> {
    sqlx::query(&format!(r#"DROP DATABASE "{db_name}" WITH (FORCE)"#)).execute(&mut *conn).await?;
    sqlx::query(&format!(r#"CREATE DATABASE "{db_name}""#)).execute(&mut *conn).await?;
    Ok(())
}

/// Runs inside the advisory lock held by [`connect_test_pool`]: create the
/// database if absent, connect (running migrations), and self-heal once on
/// a migration mismatch if we're allowed to.
///
/// Behavior:
/// - The target database is created on first use if it doesn't exist yet.
/// - If connecting fails with a migration-mismatch error (see
///   [`is_migration_mismatch`]) **and** the URL was derived by this module
///   (i.e. `DATABASE_URL` was not set), the database is treated as this
///   worktree's own disposable state: it is dropped, recreated, and
///   reconnected once, with a `tracing::warn!` explaining what happened.
///   This never loops — a second consecutive failure propagates as an error.
/// - If `DATABASE_URL` **was** set, the database is not ours to destroy. A
///   migration mismatch is returned as an error naming the cause (a
///   different branch migrated this database) and the remedy (drop it, or
///   unset `DATABASE_URL` to use this worktree's private database) — never a
///   bare `VersionMissing` panic.
async fn provision_and_connect(
    lock_conn: &mut PgConnection,
    url: &str,
    db_name: &str,
    explicit_override: bool,
) -> crate::Result<DbPool> {
    create_database_if_absent(lock_conn, db_name).await?;

    match DbPool::connect(url).await {
        Ok(pool) => Ok(pool),
        Err(e) if is_migration_mismatch(&e) && !explicit_override => {
            tracing::warn!(
                database = %db_name,
                error = %e,
                "this worktree's private test database has migrations from a branch \
                 previously checked out here that the current branch's migration source \
                 doesn't contain — recreating it"
            );
            recreate_database(lock_conn, db_name).await?;
            DbPool::connect(url).await
        }
        Err(e) if is_migration_mismatch(&e) => Err(crate::Error::Internal(format!(
            "the database at {url} has a migration applied that no file in \
             apps/server/migrations matches ({e}) — it was migrated by a different branch. \
             Drop it, or unset DATABASE_URL to use this worktree's private test database instead."
        ))),
        Err(e) => Err(e),
    }
}

/// Connect to this worktree's private test database, provisioning it if
/// necessary, and self-healing it if it was poisoned by a different branch's
/// migrations.
///
/// Non-Postgres URLs (e.g. `sqlite::memory:`) are passed straight through to
/// [`DbPool::connect`] — everything below only applies to Postgres. See the
/// module docs for why provisioning is serialized with an advisory lock, and
/// [`provision_and_connect`] for the create/connect/heal behavior itself.
pub async fn connect_test_pool() -> crate::Result<DbPool> {
    let url = test_database_url();

    if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
        return DbPool::connect(&url).await;
    }

    let explicit_override = std::env::var("DATABASE_URL").is_ok();
    let (server_url, db_name) = split_database_url(&url);

    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&format!("{server_url}/postgres"))
        .await
        .map_err(|e| {
            crate::Error::Internal(format!(
                "could not reach the test Postgres server at {server_url} ({e}). \
                 Is the `kyomi-postgres-test` container running? Start it with \
                 `docker-compose -f docker-compose.test.yml up -d postgres-test`, \
                 or see BUILD_AND_TESTING.md."
            ))
        })?;

    // Hold a single dedicated connection for the entire provisioning
    // critical section (including the migration-check `DbPool::connect`
    // inside `provision_and_connect`), guarded by a Postgres advisory lock
    // keyed on `db_name` — see the module docs for why. `max_connections(1)`
    // on `admin_pool` is intentional and safe: this function never needs a
    // second admin connection concurrently, since the whole section runs
    // against this one held connection.
    let mut lock_conn = admin_pool.acquire().await.map_err(crate::Error::Sqlx)?;
    let lock_key = advisory_lock_key(db_name);

    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(lock_key)
        .execute(&mut *lock_conn)
        .await
        .map_err(crate::Error::Sqlx)?;

    let result = provision_and_connect(&mut lock_conn, &url, db_name, explicit_override).await;

    // Always release the lock, even on failure — an unreleased lock would
    // strand every other test binary/worktree waiting on this database name.
    // (Advisory locks also release when the session ends, but that's a
    // backstop, not something to rely on as the only release path.)
    if let Err(e) =
        sqlx::query("SELECT pg_advisory_unlock($1)").bind(lock_key).execute(&mut *lock_conn).await
    {
        tracing::warn!(database = %db_name, error = %e, "failed to release test-db advisory lock");
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_same_input_same_name() {
        let a = derive_test_db_name("/home/jason/repos/kyomi/crates/kyomi-core");
        let b = derive_test_db_name("/home/jason/repos/kyomi/crates/kyomi-core");
        assert_eq!(a, b);
    }

    #[test]
    fn different_roots_different_names() {
        let a = derive_test_db_name("/home/jason/repos/kyomi/crates/kyomi-core");
        let b = derive_test_db_name("/home/jason/repos/kyomi-wt-kyo-242-test-db/crates/kyomi-core");
        assert_ne!(a, b);
    }

    #[test]
    fn output_is_valid_postgres_identifier() {
        let name = derive_test_db_name("/home/jason/repos/kyomi-wt-kyo-242-test-db/crates/kyomi-core");
        assert!(name.len() <= 63, "name {name:?} exceeds Postgres's 63-byte identifier limit");
        assert!(
            name.chars().next().is_some_and(|c| c.is_ascii_lowercase()),
            "name {name:?} must start with a lowercase letter"
        );
        assert!(
            name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "name {name:?} must match ^[a-z][a-z0-9_]*$"
        );
    }

    #[test]
    fn sanitizes_awkward_characters() {
        let name = derive_test_db_name("/Users/Jason.Adams/repos/My-Cool.Worktree/crates/kyomi-core");
        assert!(
            name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "name {name:?} should only contain lowercase alphanumerics and underscores"
        );
        assert!(name.contains("my_cool_worktree"), "name {name:?} should sanitize dashes/dots/case");
    }

    #[test]
    fn strips_crates_kyomi_core_suffix_so_worktree_root_and_crate_dir_match() {
        let from_crate_dir = derive_test_db_name("/home/jason/repos/kyomi/crates/kyomi-core");
        let from_workspace_root = derive_test_db_name("/home/jason/repos/kyomi");
        assert_eq!(from_crate_dir, from_workspace_root);
    }

    #[test]
    fn falls_back_to_input_when_suffix_absent() {
        // No `/crates/kyomi-core` suffix — used as-is (basename is the
        // input's own final path component) rather than panicking.
        let name = derive_test_db_name("/some/other/layout");
        assert!(name.starts_with("kyomi_test_layout_"), "name was {name:?}");
    }

    #[test]
    fn empty_basename_falls_back_to_workspace() {
        let name = derive_test_db_name("/crates/kyomi-core");
        assert!(name.starts_with("kyomi_test_workspace_"), "name was {name:?}");
    }

    #[test]
    fn split_database_url_separates_server_and_dbname() {
        let (server, db) = split_database_url("postgres://kyomi_test:test@localhost:5434/kyomi_test_foo_abcd1234");
        assert_eq!(server, "postgres://kyomi_test:test@localhost:5434");
        assert_eq!(db, "kyomi_test_foo_abcd1234");
    }

    #[test]
    #[should_panic(expected = "must be of the form")]
    fn split_database_url_panics_without_db_name() {
        let _ = split_database_url("not-a-url-at-all");
    }

    #[test]
    fn advisory_lock_key_is_deterministic_and_name_specific() {
        let a = advisory_lock_key("kyomi_test_foo_abcd1234");
        let b = advisory_lock_key("kyomi_test_foo_abcd1234");
        let c = advisory_lock_key("kyomi_test_bar_abcd1234");
        assert_eq!(a, b, "same database name must always produce the same lock key");
        assert_ne!(a, c, "different database names should (overwhelmingly likely) differ");
    }
}
