// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared helper for tests in this crate that need a *real* Postgres pool.
//!
//! Every `test_pool()` in this crate builds a `sqlite::memory:` pool, so
//! functions that branch on `DbPool::Postgres(..)` vs `DbPool::Sqlite(..)`
//! and emit genuinely different SQL per arm (`= ANY($1)` + array bind on
//! Postgres, `IN (?,?,?)` with individual binds on SQLite) have their
//! Postgres arm type-checked but never executed by this crate's test suite
//! — see KYO-292. This module is the one place that opens a real Postgres
//! connection for those tests, so the connect-or-skip logic exists exactly
//! once rather than once per test module (see `docs/CODING_STANDARDS.md`'s
//! "third copy is the extraction trigger" rule).
//!
//! [`kyomi_core::test_db::connect_test_pool`] is the per-worktree harness
//! built for KYO-242: it provisions (and self-heals) a private Postgres
//! database for this worktree rather than pointing every worktree at one
//! shared database, and serializes that provisioning with a Postgres
//! advisory lock. Read its module docs before adding a second caller
//! pattern here — misusing it (e.g. holding your own connection across the
//! provisioning step) can reintroduce the cross-worktree poisoning that
//! module fixed.
//!
//! # The non-negotiable constraint
//!
//! `cargo test -p kyomi-auth` with no Postgres reachable MUST still pass,
//! and MUST NOT acquire a hard dependency on a running container — that's
//! how this suite is run locally. So by default [`postgres_test_pool_or_skip`]
//! never panics or fails a test on connection failure: it returns `None`,
//! and callers early-return.
//!
//! That skip is only **visible** if the harness is told to show it: Rust's
//! test harness captures and discards `eprintln!` output for *passing*
//! tests unless you pass `--nocapture`/`--show-output`. Run locally without
//! either flag, a skip is silent-but-passing — the bare `SKIP:` line never
//! reaches the terminal, only the harness's captured-output buffer, which is
//! discarded because the test itself reports `ok`.
//!
//! CI closes that gap a different way: it sets `KYOMI_REQUIRE_POSTGRES_TESTS=1`
//! for the one job that runs these tests
//! (`.github/workflows/ci.yml`'s `cargo test (unit + integration tests)`
//! step), which has a `pgvector/pgvector:pg15` service on port 5434 matching
//! this crate's `DEFAULT_TEST_SERVER`. When that variable is set,
//! [`postgres_test_pool_or_skip`] panics instead of returning `None`, so if
//! Postgres ever stopped being reachable in CI the run would fail loudly
//! instead of silently reporting `ok` for coverage that never executed. The
//! variable is deliberately its own opt-in rather than the generic `CI` var,
//! which contributors commonly have set in their own shells — that would
//! turn "no local Postgres" into a hard failure for anyone who happens to
//! export `CI=true`.
//!
//! Net effect: locally, no Postgres → skip, tests still pass (the
//! non-negotiable constraint above). In CI, no Postgres → hard failure.

use kyomi_core::DbPool;

/// Env var that, when set to `"1"`, turns a Postgres-unreachable skip into a
/// panic. Set by CI (`.github/workflows/ci.yml`) on the one step that runs
/// these tests against a real `pgvector/pgvector:pg15` service; intentionally
/// not the generic `CI` var, which contributors commonly already have set.
const REQUIRE_POSTGRES_ENV_VAR: &str = "KYOMI_REQUIRE_POSTGRES_TESTS";

/// Connect to this worktree's private Postgres test database, or return
/// `None` with a visible-under-`--show-output` skip line if Postgres isn't
/// reachable (or `DATABASE_URL` has been pointed at a non-Postgres backend).
///
/// `test_name` should be the calling `#[tokio::test]` function's own name,
/// so the skip line names exactly which test's Postgres arm did not run —
/// callers must treat `None` as "this test did not execute", never as
/// "this test passed".
///
/// # Panics
///
/// Panics instead of returning `None` when the `KYOMI_REQUIRE_POSTGRES_TESTS`
/// env var is set to `"1"` — see the module docs for why CI sets it.
pub(crate) async fn postgres_test_pool_or_skip(test_name: &str) -> Option<DbPool> {
    let pool = match kyomi_core::test_db::connect_test_pool().await {
        Ok(pool) => pool,
        Err(e) => {
            require_postgres_or_skip(test_name, &format!("Postgres unavailable ({e})"));
            return None;
        }
    };

    if !pool.is_postgres() {
        require_postgres_or_skip(test_name, "DATABASE_URL points at a non-Postgres backend");
        return None;
    }

    Some(pool)
}

/// Panic if `KYOMI_REQUIRE_POSTGRES_TESTS=1`, otherwise print a `SKIP:` line
/// (visible locally only with `--nocapture`/`--show-output`) and return.
fn require_postgres_or_skip(test_name: &str, reason: &str) {
    if std::env::var(REQUIRE_POSTGRES_ENV_VAR).as_deref() == Ok("1") {
        panic!(
            "{test_name}: {REQUIRE_POSTGRES_ENV_VAR}=1 but Postgres was required and \
             unreachable: {reason}"
        );
    }
    eprintln!("SKIP: {test_name} — {reason}, the Postgres arm of this test did NOT run");
}

/// Extract the Postgres pool out of a `DbPool`, panicking if it turns out to
/// be a Sqlite pool. Every caller gets its `DbPool` from
/// [`postgres_test_pool_or_skip`] first (which already returned `None` and
/// caused an early return for anything non-Postgres), so reaching the
/// `Sqlite` arm here means a caller skipped that step.
///
/// Shared across `chat_service`, `workspace_service`, and
/// `datasource_service`'s Postgres-coverage tests — see
/// `docs/CODING_STANDARDS.md`'s "third copy is the extraction trigger" rule.
pub(crate) fn postgres_pool(db: &DbPool) -> &sqlx::PgPool {
    match db {
        DbPool::Postgres(pg) => pg,
        DbPool::Sqlite(_) => panic!("test requires a postgres pool"),
    }
}

/// Insert a minimal `users` row for Postgres-coverage tests.
///
/// Shared across `chat_service`, `workspace_service`, and
/// `datasource_service`'s Postgres-coverage tests — see
/// `docs/CODING_STANDARDS.md`'s "third copy is the extraction trigger" rule.
pub(crate) async fn seed_user_pg(pg: &sqlx::PgPool, user_id: &str, email: &str) {
    sqlx::query("INSERT INTO users (user_id, email) VALUES ($1, $2)")
        .bind(user_id)
        .bind(email)
        .execute(pg)
        .await
        .expect("insert user (postgres)");
}

/// Insert a minimal `workspaces` row for Postgres-coverage tests.
///
/// Shared across `chat_service`, `workspace_service`, and
/// `datasource_service`'s Postgres-coverage tests — see
/// `docs/CODING_STANDARDS.md`'s "third copy is the extraction trigger" rule.
pub(crate) async fn seed_workspace_pg(pg: &sqlx::PgPool, workspace_id: &str, owner_user_id: &str) {
    sqlx::query("INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $2, $3)")
        .bind(workspace_id)
        .bind(format!("Workspace {workspace_id}"))
        .bind(owner_user_id)
        .execute(pg)
        .await
        .expect("insert workspace (postgres)");
}

/// A short, collision-resistant id for fixture rows in the shared
/// per-worktree Postgres test database (`character varying(50)` columns
/// throughout the schema, so this stays well under that limit even with a
/// `-m1`/`-a`-style suffix appended by the caller).
///
/// Every Postgres test in this crate inserts into the same persistent
/// database rather than a fresh `sqlite::memory:` pool, and `cargo test`
/// runs `#[tokio::test]`s in parallel — so fixture ids must be unique both
/// across concurrent tests in one run and across repeated local runs (in
/// case an earlier run's cleanup didn't complete, e.g. after a panic).
pub(crate) fn unique_test_id(tag: &str) -> String {
    format!("k292-{tag}-{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

/// Delete a workspace row and one or more user rows — the tail every
/// `cleanup_pg` in this crate shares byte-for-byte. `workspaces` is deleted
/// first because `workspaces.owner_user_id REFERENCES users(user_id)`.
///
/// Callers with module-specific rows to remove first (chat sessions and
/// messages, datasource configs and table cache, workspace_users
/// memberships) delete those in their own `cleanup_pg` before calling this
/// — this helper owns only the part that was identical across all four
/// copies, per `docs/CODING_STANDARDS.md`'s "third copy is the extraction
/// trigger" rule (KYO-293 review: `collection_service`'s
/// `cleanup_created_by_test_pg` was the fourth near-identical copy,
/// alongside `chat_service`, `datasource_service`, and `workspace_service`).
pub(crate) async fn cleanup_workspace_and_users_pg(
    pg: &sqlx::PgPool,
    workspace_id: &str,
    user_ids: &[&str],
) {
    sqlx::query("DELETE FROM workspaces WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(pg)
        .await
        .expect("cleanup workspaces (postgres)");
    for user_id in user_ids {
        sqlx::query("DELETE FROM users WHERE user_id = $1")
            .bind(user_id)
            .execute(pg)
            .await
            .expect("cleanup users (postgres)");
    }
}
