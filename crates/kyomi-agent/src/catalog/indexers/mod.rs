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

/// Fail if a query result carries a driver-level error status.
///
/// Catalog-discovery providers report permission/timeout errors as
/// `QueryResult { status: Error, error: Some(..), record_batch: None }` — an
/// `Ok` value, not a transport error. A caller that reads only `record_batch`
/// would silently treat such a failure as "0 rows", so a permission-denied
/// introspection query would produce an empty-but-"successful" catalog.
/// Propagating the error here makes the indexing pipeline surface the real
/// failure (e.g. a `failed` refresh status) instead of a silent empty catalog
/// (KYO-126).
fn ensure_query_ok(result: &QueryResult) -> kyomi_core::Result<()> {
    if result.status == QueryStatus::Error {
        return Err(kyomi_core::Error::Internal(
            result
                .error
                .clone()
                .unwrap_or_else(|| "catalog discovery query failed".to_string()),
        ));
    }
    Ok(())
}

/// Extract a column of string values from a query result.
///
/// Returns `Err` if the query itself failed (see [`ensure_query_ok`]);
/// otherwise all non-null string values from the specified column index.
/// Used by indexers to extract schema/database/table names from discovery queries.
pub fn extract_string_column(
    result: &QueryResult,
    col_index: usize,
) -> kyomi_core::Result<Vec<String>> {
    ensure_query_ok(result)?;
    Ok(kyomi_datasource_server::extract_string_col_from_batch(
        result.record_batch.as_ref(),
        col_index,
    ))
}

/// Extract all rows from a query result as a Vec of JSON value rows.
///
/// Returns `Err` if the query itself failed (see [`ensure_query_ok`]).
/// Delegates to [`crate::tools::query_utils::record_batch_to_rows`] which
/// handles all Arrow column types (numbers, booleans, dates, timestamps,
/// strings, nulls).
pub fn extract_rows_from_batch(
    result: &QueryResult,
) -> kyomi_core::Result<Vec<Vec<serde_json::Value>>> {
    ensure_query_ok(result)?;
    Ok(result
        .record_batch
        .as_ref()
        .map(crate::tools::query_utils::record_batch_to_rows)
        .unwrap_or_default())
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
        assert!(extract_string_column(&result, 0).unwrap().is_empty());
    }

    #[test]
    fn extractors_error_on_error_status() {
        // A driver-level error (e.g. permission denied) is returned as
        // Ok(QueryResult{status: Error}), not a transport Err. The extractors
        // must surface it so catalog discovery fails loudly instead of
        // silently indexing zero tables (KYO-126).
        let result = QueryResult::error("permission denied for relation svv_table_info");
        let err = extract_string_column(&result, 0).unwrap_err();
        assert!(err.to_string().contains("permission denied"));
        let err = extract_rows_from_batch(&result).unwrap_err();
        assert!(err.to_string().contains("permission denied"));
    }
}
