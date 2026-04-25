// SPDX-License-Identifier: AGPL-3.0-or-later

//! Database pool abstraction — supports Postgres and SQLite at runtime.

use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

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
            sqlx::migrate!("../../apps/server/migrations").run(&pool).await?;
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
            sqlx::migrate!("../../apps/server/migrations-sqlite")
                .run(&pool)
                .await?;
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
