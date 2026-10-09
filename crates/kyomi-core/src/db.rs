// SPDX-License-Identifier: AGPL-3.0-or-later

//! Database pool abstraction — supports Postgres and SQLite at runtime.

use sqlx::migrate::Migrator;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

/// Embedded Postgres migrations, bound to a `static` rather than the
/// temporary `sqlx::migrate!(...).run(&pool)` used to write before KYO-716.
/// Binding it is what makes the embedded version set reachable at all: a
/// temporary is dropped the moment `.run()` returns, so nothing downstream
/// (in particular [`detect_migration_drift`], called independently of any
/// in-flight `connect()`) could ever ask it "what versions do you embed?".
static PG_MIGRATOR: Migrator = sqlx::migrate!("../../apps/server/migrations");

/// Embedded SQLite migrations — see [`PG_MIGRATOR`].
static SQLITE_MIGRATOR: Migrator = sqlx::migrate!("../../apps/server/migrations-sqlite");

/// Runtime-selected database pool.
///
/// `DATABASE_URL` prefix determines which backend is used:
/// - `postgresql://` or `postgres://` → Postgres
/// - Anything else (e.g. `sqlite://path.db`) → SQLite
#[derive(Clone, Debug)]
pub enum DbPool {
    Postgres(PgPool),
    Sqlite(SqlitePool),
}

impl DbPool {
    /// Connect to the database and run migrations.
    pub async fn connect(url: &str) -> crate::Result<Self> {
        if url.starts_with("postgresql://") || url.starts_with("postgres://") {
            let pool = PgPoolOptions::new()
                .max_connections(10)
                .acquire_timeout(std::time::Duration::from_secs(5))
                .connect(url)
                .await?;
            warn_on_migration_drift(&Self::Postgres(pool.clone())).await;
            PG_MIGRATOR.run(&pool).await?;
            tracing::info!("PostgreSQL pool connected, migrations applied");
            Ok(Self::Postgres(pool))
        } else {
            // SQLite WAL mode serialises writes; a single connection avoids contention.
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect(url)
                .await?;
            sqlx::query("PRAGMA journal_mode=WAL")
                .execute(&pool)
                .await
                .map_err(crate::Error::Sqlx)?;
            sqlx::query("PRAGMA foreign_keys=ON")
                .execute(&pool)
                .await
                .map_err(crate::Error::Sqlx)?;
            warn_on_migration_drift(&Self::Sqlite(pool.clone())).await;
            SQLITE_MIGRATOR.run(&pool).await?;
            tracing::info!("SQLite pool connected, migrations applied");
            Ok(Self::Sqlite(pool))
        }
    }

    pub fn is_postgres(&self) -> bool {
        matches!(self, Self::Postgres(_))
    }

    pub fn is_sqlite(&self) -> bool {
        matches!(self, Self::Sqlite(_))
    }

    /// Extract the inner `PgPool` for Postgres-only code paths.
    ///
    /// Panics if called on a SQLite pool.
    pub fn pg_pool(&self) -> &PgPool {
        match self {
            Self::Postgres(pg) => pg,
            Self::Sqlite(_) => panic!("pg_pool() called on SQLite pool"),
        }
    }
}

/// Backwards-compatible pool constructor for tests.
pub async fn create_pool(url: &str) -> crate::Result<DbPool> {
    DbPool::connect(url).await
}

/// Versions recorded in the database's `_sqlx_migrations` table that this
/// binary does not embed.
///
/// Non-empty means the database is AHEAD of this binary: something else
/// (typically a newer binary, deployed while this process kept its
/// long-lived pool open) has applied migrations this binary's embedded
/// migration source doesn't contain. That means this process's own
/// migration check — the `.run()` call inside [`DbPool::connect`] — will
/// fail with `Migrate(VersionMissing(..))` the next time it runs, e.g. after
/// a restart, crash, OOM, or host reboot. A binary that is at or ahead of
/// the schema (the ordinary case) always reports empty here — a binary
/// newer than the schema is fine, it migrates on boot. See KYO-716.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationDrift {
    /// Versions present in `_sqlx_migrations` but absent from this binary's
    /// embedded migration source, sorted ascending so any message built from
    /// this is deterministic.
    pub missing_versions: Vec<i64>,
}

impl MigrationDrift {
    /// `true` when there is no drift — this binary embeds every migration
    /// version the database has recorded.
    pub fn is_empty(&self) -> bool {
        self.missing_versions.is_empty()
    }
}

/// Row shape of `SELECT version FROM _sqlx_migrations`.
#[derive(Debug, sqlx::FromRow)]
struct MigrationVersionRow {
    version: i64,
}

/// `true` if `err` is the specific "relation/table does not exist" failure
/// each backend produces when `_sqlx_migrations` has never been created —
/// i.e. a brand-new, not-yet-migrated database — as opposed to any other
/// query failure (connectivity, permissions, or a genuinely different
/// problem), which must propagate as `Err` rather than be read as "no
/// drift". Swallowing anything broader here would be exactly the failure
/// mode `docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md`
/// warns about: a real failure degrading to an empty result that a caller
/// then reads as a genuine answer — here, "schema is current" forever, even
/// on a database this process can no longer reach.
fn is_migrations_table_absent(pool: &DbPool, err: &sqlx::Error) -> bool {
    let sqlx::Error::Database(db_err) = err else {
        return false;
    };
    match pool {
        // Postgres reports a stable, specific SQLSTATE for this: 42P01
        // (`undefined_table`). Matching on it is precise regardless of
        // which relation was missing.
        DbPool::Postgres(_) => db_err.code().as_deref() == Some("42P01"),
        // SQLite has no SQLSTATE-equivalent code for this — `code()` returns
        // the raw numeric SQLITE_ERROR (1), which is far too generic a
        // signal to gate on (many unrelated failures share it). The message
        // is the only specific signal available. Since this query only ever
        // names one table, requiring both substrings is precise without
        // being a brittle exact-string match that would break if SQLite
        // ever schema-qualifies the message differently (e.g. "no such
        // table: main._sqlx_migrations").
        DbPool::Sqlite(_) => {
            let msg = db_err.message();
            msg.contains("no such table") && msg.contains("_sqlx_migrations")
        }
    }
}

/// Detect drift between the migrations recorded in the database's
/// `_sqlx_migrations` table and the migrations embedded in this binary —
/// see [`MigrationDrift`].
///
/// A brand-new database, where `_sqlx_migrations` doesn't exist yet, reports
/// no drift (`Ok(MigrationDrift::default())`) rather than an error — see
/// [`is_migrations_table_absent`]. Any other query failure (connectivity,
/// permissions, a corrupted `_sqlx_migrations` table, ...) is returned as
/// `Err`; a caller must not treat that as "no drift".
pub async fn detect_migration_drift(pool: &DbPool) -> crate::Result<MigrationDrift> {
    let migrator: &Migrator = match pool {
        DbPool::Postgres(_) => &PG_MIGRATOR,
        DbPool::Sqlite(_) => &SQLITE_MIGRATOR,
    };

    let rows = match crate::db_fetch_all!(
        pool,
        MigrationVersionRow,
        "SELECT version FROM _sqlx_migrations"
    ) {
        Ok(rows) => rows,
        Err(e) if is_migrations_table_absent(pool, &e) => return Ok(MigrationDrift::default()),
        Err(e) => return Err(crate::Error::Sqlx(e)),
    };

    let applied_versions = rows.into_iter().map(|row| row.version).collect();
    let missing_versions = missing_versions_sorted(applied_versions, migrator);

    Ok(MigrationDrift { missing_versions })
}

/// Given the versions applied to the database (in whatever order the query
/// returned them — unspecified in general, and specifically NOT guaranteed
/// to be ascending on Postgres for a bare `SELECT` with no `ORDER BY`),
/// return the ones `migrator` does not embed, sorted ascending.
///
/// Extracted from [`detect_migration_drift`] as its own function — rather
/// than left inline — specifically so a test can feed it a deliberately
/// out-of-order input and pin that the sort is real: on SQLite specifically,
/// `version` is declared `BIGINT PRIMARY KEY`, not the literal `INTEGER` that
/// SQLite's rowid-aliasing rule requires, so it is not a rowid alias; the
/// ascending order is instead an artifact of the covering scan SQLite runs
/// over the automatic unique index it creates for that `PRIMARY KEY`. Either
/// way, a test going through the full `detect_migration_drift` query path
/// cannot distinguish "sorted on purpose" from "happened to already be
/// sorted" — it would pass identically with the `sort_unstable()` below
/// deleted. This function is the actual production path
/// (`detect_migration_drift` calls it, not a stand-in for it), so testing it
/// directly closes that gap without weakening the test.
fn missing_versions_sorted(applied_versions: Vec<i64>, migrator: &Migrator) -> Vec<i64> {
    let mut missing: Vec<i64> =
        applied_versions.into_iter().filter(|version| !migrator.version_exists(*version)).collect();
    missing.sort_unstable();
    missing
}

/// Check for migration drift immediately before the migration run below,
/// and log a full diagnosis — every missing version, not just one — if the
/// database is ahead of this binary.
///
/// This does not change whether the `.run()` call after it succeeds or
/// fails: a schema ahead of this binary must still refuse to (re)connect,
/// exactly as `sqlx` already enforces. It only makes that refusal legible
/// *before* it happens, instead of a bare `Migrate(VersionMissing(v))`
/// naming just the first of possibly several unrecognised migrations — see
/// KYO-716. Errors from the detector itself never block startup: connect()
/// proceeds to `.run()` regardless, exactly as it always has.
async fn warn_on_migration_drift(pool: &DbPool) {
    match detect_migration_drift(pool).await {
        Ok(drift) if !drift.is_empty() => {
            tracing::error!(
                missing_versions = ?drift.missing_versions,
                "database schema is AHEAD of this binary: {} migration(s) recorded in \
                 _sqlx_migrations are not embedded in this build ({:?}). This process cannot \
                 (re)connect to the database until a binary built from a commit that includes \
                 them is deployed.",
                drift.missing_versions.len(),
                drift.missing_versions,
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(
                error = %e,
                "migration drift check failed before connecting (non-fatal, continuing)"
            );
        }
    }
}

/// Run a quick connectivity check — useful for health endpoints.
pub async fn ping(pool: &DbPool) -> crate::Result<()> {
    match pool {
        DbPool::Postgres(pg) => {
            sqlx::query_scalar::<_, i32>("SELECT 1")
                .fetch_one(pg)
                .await?;
        }
        DbPool::Sqlite(sq) => {
            sqlx::query_scalar::<_, i32>("SELECT 1")
                .fetch_one(sq)
                .await?;
        }
    }
    Ok(())
}

/// Backend-agnostic query result for `db_execute!`.
///
/// Wraps the `rows_affected()` value from either `PgQueryResult` or
/// `SqliteQueryResult` so callers don't need to care about the backend.
pub struct DbQueryResult {
    rows: u64,
}

impl DbQueryResult {
    pub fn from_pg(r: sqlx::postgres::PgQueryResult) -> Self {
        Self { rows: r.rows_affected() }
    }
    pub fn from_sqlite(r: sqlx::sqlite::SqliteQueryResult) -> Self {
        Self { rows: r.rows_affected() }
    }
    pub fn rows_affected(&self) -> u64 {
        self.rows
    }
}

/// Execute a query that returns typed rows via `query_as`. Fetches all rows.
#[macro_export]
macro_rules! db_fetch_all {
    ($pool:expr, $type:ty, $query:expr $(, $bind:expr)*) => {
        match &$pool {
            $crate::db::DbPool::Postgres(pg) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_all(pg).await,
            $crate::db::DbPool::Sqlite(sq) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_all(sq).await,
        }
    }
}

/// Fetch exactly one row.
#[macro_export]
macro_rules! db_fetch_one {
    ($pool:expr, $type:ty, $query:expr $(, $bind:expr)*) => {
        match &$pool {
            $crate::db::DbPool::Postgres(pg) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_one(pg).await,
            $crate::db::DbPool::Sqlite(sq) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_one(sq).await,
        }
    }
}

/// Fetch zero or one row.
#[macro_export]
macro_rules! db_fetch_optional {
    ($pool:expr, $type:ty, $query:expr $(, $bind:expr)*) => {
        match &$pool {
            $crate::db::DbPool::Postgres(pg) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_optional(pg).await,
            $crate::db::DbPool::Sqlite(sq) =>
                sqlx::query_as::<_, $type>($query)$(.bind($bind))*.fetch_optional(sq).await,
        }
    }
}

/// Execute a query without returning rows (INSERT, UPDATE, DELETE).
///
/// Returns `Result<DbQueryResult, sqlx::Error>` which provides
/// `rows_affected()` regardless of the backend.
#[macro_export]
macro_rules! db_execute {
    ($pool:expr, $query:expr $(, $bind:expr)*) => {
        match &$pool {
            $crate::db::DbPool::Postgres(pg) =>
                sqlx::query($query)$(.bind($bind))*.execute(pg).await
                    .map($crate::db::DbQueryResult::from_pg),
            $crate::db::DbPool::Sqlite(sq) =>
                sqlx::query($query)$(.bind($bind))*.execute(sq).await
                    .map($crate::db::DbQueryResult::from_sqlite),
        }
    }
}

/// Dispatch to whichever pool variant is active.
///
/// Use this when both the Postgres and SQLite arms would be **identical**
/// except for the pool variable name.  The closure receives `p` which is
/// either a `&PgPool` or a `&SqlitePool`.
///
/// ```rust,ignore
/// let row = db_with_pool!(pool, |p| {
///     sqlx::query("SELECT 1").fetch_one(p).await?
/// });
/// ```
#[macro_export]
macro_rules! db_with_pool {
    ($pool:expr, |$p:ident| $body:expr) => {
        match &$pool {
            $crate::db::DbPool::Postgres($p) => { $body }
            $crate::db::DbPool::Sqlite($p) => { $body }
        }
    }
}

/// Fetch a single scalar value.
#[macro_export]
macro_rules! db_fetch_scalar {
    ($pool:expr, $type:ty, $query:expr $(, $bind:expr)*) => {
        match &$pool {
            $crate::db::DbPool::Postgres(pg) =>
                sqlx::query_scalar::<_, $type>($query)$(.bind($bind))*.fetch_one(pg).await,
            $crate::db::DbPool::Sqlite(sq) =>
                sqlx::query_scalar::<_, $type>($query)$(.bind($bind))*.fetch_one(sq).await,
        }
    }
}

/// Build a SQL IN clause with numbered placeholders starting at `start_idx`.
/// Returns the clause `($N, $N+1, ...)` and the next available index.
pub fn in_clause_placeholders(count: usize, start_idx: usize) -> (String, usize) {
    let placeholders: Vec<String> = (0..count)
        .map(|i| format!("${}", start_idx + i))
        .collect();
    let clause = format!("({})", placeholders.join(", "));
    (clause, start_idx + count)
}

#[cfg(test)]
mod migration_drift_tests {
    use super::*;
    use tracing::Level;

    /// A version far beyond anything this workspace will ever embed for
    /// real, used across these tests as the "the schema is ahead of this
    /// binary" version. Chosen the same way the ticket's own reproduction
    /// does — a year-2999 timestamp prefix, matching this repo's
    /// timestamp-prefixed Postgres migration naming.
    const FUTURE_VERSION_A: i64 = 29990101000000;
    const FUTURE_VERSION_B: i64 = 29990202000000;

    async fn insert_migration_row(pool: &DbPool, version: i64) {
        crate::db_execute!(
            pool,
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES ($1, $2, TRUE, $3, -1)",
            version,
            "kyo-716-test-injected-migration",
            Vec::<u8>::new()
        )
        .expect("insert synthetic _sqlx_migrations row");
    }

    // ── 1. Drift is detected ────────────────────────────────────────────

    #[tokio::test]
    async fn drift_is_detected_for_a_single_injected_future_version() {
        let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
        insert_migration_row(&pool, FUTURE_VERSION_A).await;

        let drift = detect_migration_drift(&pool).await.expect("detect_migration_drift");

        assert_eq!(drift.missing_versions, vec![FUTURE_VERSION_A]);
        assert!(!drift.is_empty());
    }

    // ── 2. Multiple missing versions are all reported, sorted ──────────
    //
    // The reported incident (KYO-716) had two drifted versions and the
    // `sqlx` panic named only one — this test pins the actual improvement:
    // every missing version comes back, in a deterministic order, not just
    // the first one `sqlx::Migrator::run` happens to trip over.

    #[tokio::test]
    async fn multiple_missing_versions_are_all_reported_sorted_ascending() {
        let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
        // Inserted out of order deliberately. Note this does NOT by itself
        // prove the sort is doing anything on SQLite specifically — see
        // `missing_versions_sorted_reorders_out_of_order_input` below for
        // why, and for the test that actually pins the sort against a
        // regression via mutation.
        insert_migration_row(&pool, FUTURE_VERSION_B).await;
        insert_migration_row(&pool, FUTURE_VERSION_A).await;

        let drift = detect_migration_drift(&pool).await.expect("detect_migration_drift");

        assert_eq!(drift.missing_versions, vec![FUTURE_VERSION_A, FUTURE_VERSION_B]);
    }

    /// Direct, mutation-provable test of the sort: feeds
    /// `missing_versions_sorted` (the actual function `detect_migration_drift`
    /// calls) a deliberately descending input. Unlike the end-to-end test
    /// above, this cannot pass "by accident" — the input order here is
    /// fully controlled by the test, not by SQLite's incidental table-scan
    /// order. Verified by mutation: deleting the `sort_unstable()` call in
    /// `missing_versions_sorted` makes this test fail with
    /// `[FUTURE_VERSION_B, FUTURE_VERSION_A]`, while the end-to-end test
    /// above keeps passing under that same mutation.
    #[test]
    fn missing_versions_sorted_reorders_out_of_order_input() {
        let result =
            missing_versions_sorted(vec![FUTURE_VERSION_B, FUTURE_VERSION_A], &SQLITE_MIGRATOR);
        assert_eq!(result, vec![FUTURE_VERSION_A, FUTURE_VERSION_B]);
    }

    // ── 3. A current database reports no drift and logs nothing ────────

    #[tokio::test]
    async fn a_current_database_reports_no_drift() {
        let pool = DbPool::connect("sqlite::memory:").await.expect("connect");

        let drift = detect_migration_drift(&pool).await.expect("detect_migration_drift");

        assert!(drift.is_empty());
        assert_eq!(drift.missing_versions, Vec::<i64>::new());
    }

    #[tokio::test]
    async fn a_current_database_produces_no_warn_or_error_log() {
        let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
        let logs = kyomi_test_tracing::capture_tracing();

        // Exercise the actual boot-signal code path (KYO-716 part B), not
        // just the pure detector — this is the function that decides
        // whether anything gets logged.
        warn_on_migration_drift(&pool).await;

        assert!(
            logs.events_at(Level::WARN).is_empty(),
            "a current database must not log any WARN: {:?}",
            logs.events_at(Level::WARN)
        );
        assert!(
            logs.events_at(Level::ERROR).is_empty(),
            "a current database must not log any ERROR: {:?}",
            logs.events_at(Level::ERROR)
        );
    }

    // ── 4. Fresh-install path: table absent is not confused with error ─

    #[tokio::test]
    async fn a_database_with_the_migrations_table_absent_reports_no_drift() {
        // Deliberately not `DbPool::connect` — that always runs migrations
        // first, which is exactly the case this test must NOT cover. A raw
        // pool against a fresh in-memory database has no `_sqlx_migrations`
        // table at all, the brand-new-database / fresh-install case.
        let raw_pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect raw sqlite pool");
        let pool = DbPool::Sqlite(raw_pool);

        let drift = detect_migration_drift(&pool)
            .await
            .expect("a missing _sqlx_migrations table must be reported as Ok(no drift), not Err");

        assert!(drift.is_empty());
    }

    // ── 5. A genuine query error still returns Err ──────────────────────
    //
    // Pins the boundary of `is_migrations_table_absent`: a query failure
    // against `_sqlx_migrations` that is NOT "the table doesn't exist" must
    // still propagate as `Err`, so the "table absent" special-case above
    // can never widen into a blanket "any error means no drift".

    #[tokio::test]
    async fn a_genuine_query_error_against_an_existing_table_still_returns_err() {
        let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
        // `_sqlx_migrations` exists (migrations just ran), but the column
        // `detect_migration_drift` queries does not — a real, reproducible
        // failure distinct in shape from "no such table".
        crate::db_execute!(
            pool,
            "ALTER TABLE _sqlx_migrations RENAME COLUMN version TO version_renamed_for_test"
        )
        .expect("rename column to induce a genuine query error");

        let result = detect_migration_drift(&pool).await;

        assert!(
            result.is_err(),
            "a query failure that is not 'table absent' must propagate as Err, not be read as no drift"
        );
    }

    // ── Boot signal: every missing version is named, not just the first ─

    #[tokio::test]
    async fn warn_on_migration_drift_names_every_missing_version() {
        let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
        insert_migration_row(&pool, FUTURE_VERSION_A).await;
        insert_migration_row(&pool, FUTURE_VERSION_B).await;
        let logs = kyomi_test_tracing::capture_tracing();

        warn_on_migration_drift(&pool).await;

        let errors = logs.events_at(Level::ERROR);
        assert_eq!(errors.len(), 1, "expected exactly one ERROR log; got {errors:?}");
        let (_, message) = &errors[0];
        assert!(
            message.contains(&FUTURE_VERSION_A.to_string()),
            "log must name {FUTURE_VERSION_A}; got: {message}"
        );
        assert!(
            message.contains(&FUTURE_VERSION_B.to_string()),
            "log must name {FUTURE_VERSION_B}; got: {message}"
        );
    }

    #[tokio::test]
    async fn is_migrations_table_absent_does_not_match_a_different_missing_relation() {
        let pool = DbPool::connect("sqlite::memory:").await.expect("connect");
        let err = crate::db_fetch_all!(
            pool,
            MigrationVersionRow,
            "SELECT version FROM this_table_was_never_created"
        )
        .expect_err("querying a nonexistent, unrelated table must fail");

        // The predicate must only recognise the absence of
        // `_sqlx_migrations` specifically — not "any missing table" — so it
        // can never be satisfied by an unrelated schema problem.
        assert!(!is_migrations_table_absent(&pool, &err));
    }
}
