// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL-based catalog indexer implementations.
//!
//! Each indexer implements [`super::traits::SQLCatalogIndexer`] and
//! [`super::traits::CatalogIndexer`], providing provider-specific SQL
//! queries for container discovery, table listing, and column inspection.
//!
//! The shared template method [`super::traits::index_catalog_sql`] orchestrates
//! the full indexing flow using these provider-specific details.
//!
//! ## BigQuery
//!
//! BigQuery uses a REST API for catalog indexing, not SQL. The
//! [`BigQueryIndexer`] resolves an access token based on the configured
//! `auth_mode` (kyomi_oauth, enterprise_oauth, or service_account) and
//! delegates to `UserDatasetIndexer::index_workspace_catalog()` for the
//! actual REST API work (list datasets → list tables → get schema → cache).

mod bigquery;
mod clickhouse;
mod connect;
mod databricks;
mod flaredb;
mod mysql;
mod postgres;
mod redshift;
mod snowflake;
mod sqlserver;
mod synapse;

// Re-export indexer structs for use by CatalogIndexingService.
pub use bigquery::BigQueryIndexer;
pub use clickhouse::ClickHouseIndexer;
pub use connect::ConnectIndexer;
pub use databricks::DatabricksIndexer;
pub use flaredb::FlareDbIndexer;
pub use mysql::MySqlIndexer;
pub use postgres::PostgresIndexer;
pub use redshift::RedshiftIndexer;
pub use snowflake::SnowflakeIndexer;
pub use sqlserver::SqlServerIndexer;
pub use synapse::SynapseIndexer;

// ─── Shared helpers used by multiple SQL indexers ────────────────────────────

use kyomi_datasource_server::{QueryResult, QueryStatus};

/// Convert a query-level failure into a Rust error.
///
/// Providers report permission errors, timeouts, and bad SQL as
/// `Ok(QueryResult { status: QueryStatus::Error, error: Some(..) })` rather
/// than `Err`. Reading `record_batch` without checking `status` silently
/// turns a permission denial into "0 rows discovered" — both
/// [`extract_string_column`] and [`extract_rows_from_batch`] read only
/// `record_batch`, which is `None` on an error result, so a schema the role
/// can't read looks identical to a schema that is genuinely empty (KYO-126).
///
/// Callers must call this immediately after `execute_query` and before
/// calling either extractor, so the real database message propagates up
/// through `discover_all_containers`/`get_tables_in_container`/
/// `get_table_columns` instead of being silently swallowed.
///
/// Uses [`kyomi_core::Error::DatasourceConnection`] rather than
/// `Error::Internal`: a provider-reported query failure (permission denied,
/// bad SQL, timeout) is a datasource-side problem the user can act on, not a
/// server bug. `DatasourceConnection`'s `Display` is prefix-free and its
/// message is meant to surface verbatim in a client-facing message —
/// `Internal`'s message is intentionally hidden from the client, which would
/// re-create the exact silence this fix removes.
pub fn ensure_query_ok(result: &QueryResult, context: &str) -> kyomi_core::Result<()> {
    if result.status == QueryStatus::Error {
        let msg = result
            .error
            .as_deref()
            .unwrap_or("query failed with no error message");
        return Err(kyomi_core::Error::DatasourceConnection(format!(
            "{context}: {msg}"
        )));
    }
    Ok(())
}

/// Extract a column of string values from a query result.
///
/// Returns all non-null string values from the specified column index.
/// Used by indexers to extract schema/database/table names from discovery queries.
///
/// Callers must check [`ensure_query_ok`] first — this function has no way
/// to distinguish a query error from a genuinely empty result, since both
/// leave `record_batch` as `None`.
pub fn extract_string_column(result: &QueryResult, col_index: usize) -> Vec<String> {
    kyomi_datasource_server::extract_string_col_from_batch(
        result.record_batch.as_ref(),
        col_index,
    )
}

/// Extract all rows from a query result as a Vec of JSON value rows.
///
/// Delegates to [`crate::tools::query_utils::record_batch_to_rows`] which
/// handles all Arrow column types (numbers, booleans, dates, timestamps,
/// strings, nulls).
///
/// Callers must check [`ensure_query_ok`] first — this function has no way
/// to distinguish a query error from a genuinely empty result, since both
/// leave `record_batch` as `None`.
pub fn extract_rows_from_batch(result: &QueryResult) -> Vec<Vec<serde_json::Value>> {
    result
        .record_batch
        .as_ref()
        .map(crate::tools::query_utils::record_batch_to_rows)
        .unwrap_or_default()
}

/// Escape single quotes in SQL string literals.
///
/// Replaces `'` with `''` (SQL standard escaping). Used for interpolating
/// schema/table names into catalog discovery queries.
pub fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// System schemas shared by SQL Server and Azure Synapse (T-SQL engines).
pub const TSQL_SYSTEM_SCHEMAS: &[&str] = &[
    "sys",
    "INFORMATION_SCHEMA",
    "db_owner",
    "db_accessadmin",
    "db_securityadmin",
    "db_ddladmin",
    "db_backupoperator",
    "db_datareader",
    "db_datawriter",
    "db_denydatareader",
    "db_denydatawriter",
    "guest",
];

/// Check if a schema name is a T-SQL system schema (case-insensitive).
///
/// Used by both the SQL Server and Synapse indexers.
pub fn is_tsql_system_schema(name: &str) -> bool {
    let lower = name.to_lowercase();
    TSQL_SYSTEM_SCHEMAS
        .iter()
        .any(|s| lower == s.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_escape_single_quotes() {
        assert_eq!(sql_escape("O'Reilly"), "O''Reilly");
        assert_eq!(sql_escape("no quotes"), "no quotes");
        assert_eq!(sql_escape("a'b'c"), "a''b''c");
    }

    #[test]
    fn tsql_system_schema_detection() {
        assert!(is_tsql_system_schema("sys"));
        assert!(is_tsql_system_schema("SYS"));
        assert!(is_tsql_system_schema("INFORMATION_SCHEMA"));
        assert!(is_tsql_system_schema("information_schema"));
        assert!(is_tsql_system_schema("db_owner"));
        assert!(is_tsql_system_schema("guest"));

        assert!(!is_tsql_system_schema("dbo"));
        assert!(!is_tsql_system_schema("public"));
        assert!(!is_tsql_system_schema("my_schema"));
    }

    #[test]
    fn extract_string_column_empty_result() {
        let result = QueryResult::success_empty();
        assert!(extract_string_column(&result, 0).is_empty());
    }

    // ── ensure_query_ok (KYO-126) ─────────────────────────────────────────

    #[test]
    fn ensure_query_ok_errors_on_error_status_with_provider_message() {
        let mut result = QueryResult::success_empty();
        result.status = QueryStatus::Error;
        result.error = Some("permission denied for schema analytics".to_string());

        let err = ensure_query_ok(&result, "listing schemas")
            .expect_err("an error-status result must be rejected");
        assert_eq!(
            err.to_string(),
            "listing schemas: permission denied for schema analytics"
        );
    }

    #[test]
    fn ensure_query_ok_falls_back_to_generic_message_when_provider_gives_none() {
        let mut result = QueryResult::success_empty();
        result.status = QueryStatus::Error;
        result.error = None;

        let err = ensure_query_ok(&result, "listing schemas")
            .expect_err("an error-status result must be rejected even with no message");
        assert_eq!(
            err.to_string(),
            "listing schemas: query failed with no error message"
        );
    }

    #[test]
    fn ensure_query_ok_accepts_success_status() {
        let result = QueryResult::success_empty();
        assert!(ensure_query_ok(&result, "listing schemas").is_ok());
    }

    #[test]
    fn ensure_query_ok_accepts_genuinely_empty_success_result() {
        // Regression guard (KYO-126): a schema the role CAN read but which
        // simply has zero tables must still succeed. `success_empty()` has
        // no `record_batch`, same as an error result — `ensure_query_ok`
        // must key off `status`, not the presence of rows, or it would
        // reintroduce a false positive on every legitimately empty schema.
        let result = QueryResult::success_empty();
        assert!(extract_rows_from_batch(&result).is_empty());
        assert!(ensure_query_ok(&result, "listing tables in schema 'empty_schema'").is_ok());
    }
}
