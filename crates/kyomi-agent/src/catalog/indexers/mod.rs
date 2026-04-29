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
mod databricks;
mod mysql;
mod postgres;
mod redshift;
mod snowflake;
mod sqlserver;
mod synapse;

// Re-export indexer structs for use by CatalogIndexingService.
pub use bigquery::BigQueryIndexer;
pub use clickhouse::ClickHouseIndexer;
pub use databricks::DatabricksIndexer;
pub use mysql::MySqlIndexer;
pub use postgres::PostgresIndexer;
pub use redshift::RedshiftIndexer;
pub use snowflake::SnowflakeIndexer;
pub use sqlserver::SqlServerIndexer;
pub use synapse::SynapseIndexer;

// ─── Shared helpers used by multiple SQL indexers ────────────────────────────

use kyomi_datasource_server::QueryResult;

/// Extract a column of string values from a query result.
///
/// Returns all non-null string values from the specified column index.
/// Used by indexers to extract schema/database/table names from discovery queries.
pub fn extract_string_column(result: &QueryResult, col_index: usize) -> Vec<String> {
    kyomi_datasource_server::extract_string_col_from_batch(
        result.record_batch.as_ref(),
        col_index,
    )
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
}
