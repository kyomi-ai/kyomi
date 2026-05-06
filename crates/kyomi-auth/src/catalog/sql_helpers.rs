// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pure SQL helper functions for catalog discovery queries.
//!
//! These functions generate dialect-aware SQL for listing tables and columns
//! across all supported datasource types. They are used by both the REST API
//! handler and the Leptos server function catalog refresh pipelines.

/// Escape a SQL string literal by doubling single quotes.
///
/// This is the standard SQL escaping approach — `O'Brien` becomes `O''Brien`.
/// Safe for use in `WHERE col = '{escaped}'` clauses across all SQL dialects.
pub fn escape_sql_literal(s: &str) -> String {
    s.replace('\'', "''")
}

/// Escape a SQL identifier for safe interpolation into FROM clauses.
///
/// Unlike string literals (which use single-quote escaping), identifiers used in
/// `FROM <identifier>.INFORMATION_SCHEMA.TABLES` must be quoted with the dialect's
/// identifier quoting mechanism to prevent SQL injection.
///
/// - **Snowflake**: Double-quoted identifiers. Internal `"` are escaped as `""`.
///   Result: `"MY_DATABASE".INFORMATION_SCHEMA.TABLES`
/// - **Databricks**: Backtick-quoted identifiers. Internal `` ` `` are escaped as ``` `` ```.
///   Result: `` `my_catalog`.INFORMATION_SCHEMA.TABLES ``
pub fn escape_sql_identifier(s: &str, dialect: &str) -> String {
    match dialect {
        "snowflake" => {
            let escaped = s.replace('"', "\"\"");
            format!("\"{}\"", escaped)
        }
        "databricks" => {
            let escaped = s.replace('`', "``");
            format!("`{}`", escaped)
        }
        _ => s.to_string(),
    }
}

/// Return the SQL query to list tables in a specific container (schema/database).
///
/// The SQL must return at least one column: the table name.
/// Container names are escaped via single-quote doubling to prevent SQL injection.
pub fn get_tables_in_container_sql(type_id: &str, container: &str) -> Option<String> {
    let escaped = escape_sql_literal(container);

    match type_id {
        "postgres" | "redshift" => Some(format!(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = '{escaped}' ORDER BY table_name"
        )),
        "mysql" => Some(format!(
            "SELECT TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = '{escaped}' ORDER BY TABLE_NAME"
        )),
        "clickhouse" => Some(format!(
            "SELECT name FROM system.tables \
             WHERE database = '{escaped}' ORDER BY name"
        )),
        "sqlserver" | "synapse" => Some(format!(
            "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA = '{escaped}' ORDER BY TABLE_NAME"
        )),
        // Snowflake: container is a database name; list tables across all schemas.
        // Returns 3 columns: TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE.
        // The indexing loop handles the multi-column result.
        "snowflake" => {
            let quoted = escape_sql_identifier(container, "snowflake");
            Some(format!(
                "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE \
                 FROM {quoted}.INFORMATION_SCHEMA.TABLES \
                 WHERE TABLE_SCHEMA NOT IN ('INFORMATION_SCHEMA') \
                   AND TABLE_TYPE IN ('BASE TABLE', 'VIEW') \
                 ORDER BY TABLE_SCHEMA, TABLE_NAME"
            ))
        }
        // Databricks: container is a catalog name; list tables across all schemas.
        // Returns 3 columns: TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE.
        "databricks" => {
            let quoted = escape_sql_identifier(container, "databricks");
            Some(format!(
                "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE \
                 FROM {quoted}.INFORMATION_SCHEMA.TABLES \
                 WHERE TABLE_SCHEMA NOT IN ('information_schema') \
                   AND TABLE_TYPE IN ('BASE TABLE', 'VIEW', 'MANAGED', 'EXTERNAL') \
                 ORDER BY TABLE_SCHEMA, TABLE_NAME"
            ))
        }
        // BigQuery: container is "project_id.dataset_id"
        "bigquery" => Some(format!(
            "SELECT table_name FROM `{escaped}`.INFORMATION_SCHEMA.TABLES ORDER BY table_name"
        )),
        _ => None,
    }
}

/// Return the SQL query to list columns for a specific table.
///
/// Returns rows with: column_name, data_type, description (if available).
pub fn get_columns_sql(type_id: &str, container: &str, table_name: &str) -> Option<String> {
    let esc_container = escape_sql_literal(container);
    let esc_table = escape_sql_literal(table_name);

    match type_id {
        "postgres" | "redshift" => Some(format!(
            "SELECT column_name, data_type, '' as description \
             FROM information_schema.columns \
             WHERE table_schema = '{esc_container}' AND table_name = '{esc_table}' \
             ORDER BY ordinal_position"
        )),
        "mysql" => Some(format!(
            "SELECT COLUMN_NAME, DATA_TYPE, COLUMN_COMMENT \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = '{esc_container}' AND TABLE_NAME = '{esc_table}' \
             ORDER BY ORDINAL_POSITION"
        )),
        "clickhouse" => Some(format!(
            "SELECT name, type, comment \
             FROM system.columns \
             WHERE database = '{esc_container}' AND table = '{esc_table}' \
             ORDER BY position"
        )),
        "sqlserver" | "synapse" => Some(format!(
            "SELECT COLUMN_NAME, DATA_TYPE, '' as description \
             FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = '{esc_container}' AND TABLE_NAME = '{esc_table}' \
             ORDER BY ORDINAL_POSITION"
        )),
        // Snowflake: container is "database.schema" (set by the indexing loop).
        // Split to use database as identifier prefix and schema in WHERE clause.
        "snowflake" => {
            let parts: Vec<&str> = container.splitn(2, '.').collect();
            let (db_name, schema_name) = if parts.len() == 2 {
                (parts[0], parts[1])
            } else {
                (container, "PUBLIC")
            };
            let quoted_db = escape_sql_identifier(db_name, "snowflake");
            let esc_schema = escape_sql_literal(schema_name);
            Some(format!(
                "SELECT COLUMN_NAME, DATA_TYPE, COMMENT \
                 FROM {quoted_db}.INFORMATION_SCHEMA.COLUMNS \
                 WHERE TABLE_SCHEMA = '{esc_schema}' AND TABLE_NAME = '{esc_table}' \
                 ORDER BY ORDINAL_POSITION"
            ))
        }
        // Databricks: container is "catalog.schema" (set by the indexing loop).
        // Split to use catalog as identifier prefix and schema in WHERE clause.
        "databricks" => {
            let parts: Vec<&str> = container.splitn(2, '.').collect();
            let (catalog_name, schema_name) = if parts.len() == 2 {
                (parts[0], parts[1])
            } else {
                (container, "default")
            };
            let quoted_catalog = escape_sql_identifier(catalog_name, "databricks");
            let esc_schema = escape_sql_literal(schema_name);
            Some(format!(
                "SELECT COLUMN_NAME, DATA_TYPE, COMMENT \
                 FROM {quoted_catalog}.INFORMATION_SCHEMA.COLUMNS \
                 WHERE TABLE_SCHEMA = '{esc_schema}' AND TABLE_NAME = '{esc_table}' \
                 ORDER BY ORDINAL_POSITION"
            ))
        }
        // BigQuery: container is "project_id.dataset_id"
        "bigquery" => Some(format!(
            "SELECT column_name, data_type, description \
             FROM `{esc_container}`.INFORMATION_SCHEMA.COLUMN_FIELD_PATHS \
             WHERE table_name = '{esc_table}' AND field_path = column_name \
             ORDER BY ordinal_position"
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_sql_literal_handles_single_quotes() {
        assert_eq!(escape_sql_literal("test's_db"), "test''s_db");
    }

    #[test]
    fn escape_sql_literal_preserves_hyphens() {
        assert_eq!(escape_sql_literal("my-database"), "my-database");
    }

    #[test]
    fn escape_sql_literal_preserves_spaces() {
        assert_eq!(escape_sql_literal("my database"), "my database");
    }

    #[test]
    fn escape_sql_literal_no_change_for_simple_identifiers() {
        assert_eq!(escape_sql_literal("public"), "public");
        assert_eq!(escape_sql_literal("my_schema"), "my_schema");
    }

    #[test]
    fn escape_sql_literal_doubles_multiple_quotes() {
        assert_eq!(escape_sql_literal("it''s"), "it''''s");
    }

    #[test]
    fn get_tables_sql_uses_escaped_container() {
        let sql = get_tables_in_container_sql("postgres", "test's schema").unwrap();
        assert!(sql.contains("test''s schema"), "should escape quotes in SQL: {sql}");
    }

    #[test]
    fn get_columns_sql_uses_escaped_values() {
        let sql = get_columns_sql("clickhouse", "my-db", "my-table").unwrap();
        assert!(sql.contains("my-db"), "should preserve hyphens: {sql}");
        assert!(sql.contains("my-table"), "should preserve hyphens in table name: {sql}");
    }

    #[test]
    fn snowflake_tables_sql_uses_quoted_identifier() {
        let sql = get_tables_in_container_sql("snowflake", "MY_DATABASE").unwrap();
        assert!(sql.contains("\"MY_DATABASE\".INFORMATION_SCHEMA.TABLES"), "should use double-quoted database prefix: {sql}");
        assert!(sql.contains("TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE"), "should select schema + name: {sql}");
        assert!(sql.contains("INFORMATION_SCHEMA"), "should exclude INFORMATION_SCHEMA: {sql}");
    }

    #[test]
    fn snowflake_columns_sql_uses_quoted_identifier() {
        let sql = get_columns_sql("snowflake", "MY_DB.PUBLIC", "users").unwrap();
        assert!(sql.contains("\"MY_DB\".INFORMATION_SCHEMA.COLUMNS"), "should use double-quoted database prefix: {sql}");
        assert!(sql.contains("TABLE_SCHEMA = 'PUBLIC'"), "should filter by schema: {sql}");
        assert!(sql.contains("TABLE_NAME = 'users'"), "should filter by table: {sql}");
    }

    #[test]
    fn databricks_tables_sql_uses_quoted_identifier() {
        let sql = get_tables_in_container_sql("databricks", "my_catalog").unwrap();
        assert!(sql.contains("`my_catalog`.INFORMATION_SCHEMA.TABLES"), "should use backtick-quoted catalog prefix: {sql}");
        assert!(sql.contains("TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE"), "should select schema + name: {sql}");
    }

    #[test]
    fn databricks_columns_sql_uses_quoted_identifier() {
        let sql = get_columns_sql("databricks", "my_catalog.my_schema", "orders").unwrap();
        assert!(sql.contains("`my_catalog`.INFORMATION_SCHEMA.COLUMNS"), "should use backtick-quoted catalog prefix: {sql}");
        assert!(sql.contains("TABLE_SCHEMA = 'my_schema'"), "should filter by schema: {sql}");
        assert!(sql.contains("TABLE_NAME = 'orders'"), "should filter by table: {sql}");
    }

    // -- escape_sql_identifier tests --

    #[test]
    fn escape_sql_identifier_snowflake_simple() {
        assert_eq!(escape_sql_identifier("MY_DATABASE", "snowflake"), "\"MY_DATABASE\"");
    }

    #[test]
    fn escape_sql_identifier_snowflake_escapes_double_quotes() {
        assert_eq!(
            escape_sql_identifier("my\"db", "snowflake"),
            "\"my\"\"db\""
        );
    }

    #[test]
    fn escape_sql_identifier_snowflake_injection_attempt() {
        // An attacker tries: MY_DB".INFORMATION_SCHEMA.TABLES; DROP TABLE users; --
        let malicious = "MY_DB\".INFORMATION_SCHEMA.TABLES; DROP TABLE users; --";
        let escaped = escape_sql_identifier(malicious, "snowflake");
        assert_eq!(
            escaped,
            "\"MY_DB\"\".INFORMATION_SCHEMA.TABLES; DROP TABLE users; --\""
        );
        // The entire string is inside one quoted identifier, so the injection is neutralized.
        assert!(!escaped.starts_with("\"MY_DB\"."), "injection must not break out of identifier: {escaped}");
    }

    #[test]
    fn escape_sql_identifier_databricks_simple() {
        assert_eq!(escape_sql_identifier("my_catalog", "databricks"), "`my_catalog`");
    }

    #[test]
    fn escape_sql_identifier_databricks_escapes_backticks() {
        assert_eq!(
            escape_sql_identifier("my`catalog", "databricks"),
            "`my``catalog`"
        );
    }

    #[test]
    fn escape_sql_identifier_databricks_injection_attempt() {
        // An attacker tries: my_catalog`.INFORMATION_SCHEMA.TABLES; DROP TABLE users; --
        let malicious = "my_catalog`.INFORMATION_SCHEMA.TABLES; DROP TABLE users; --";
        let escaped = escape_sql_identifier(malicious, "databricks");
        assert_eq!(
            escaped,
            "`my_catalog``.INFORMATION_SCHEMA.TABLES; DROP TABLE users; --`"
        );
        // The entire string is inside one backtick-quoted identifier.
        assert!(!escaped.starts_with("`my_catalog`."), "injection must not break out of identifier: {escaped}");
    }

    #[test]
    fn escape_sql_identifier_unknown_dialect_passthrough() {
        // Unknown dialects pass through unchanged (those types don't use identifier interpolation)
        assert_eq!(escape_sql_identifier("test", "postgres"), "test");
    }

    #[test]
    fn snowflake_tables_sql_injection_prevented() {
        // Verify the full SQL query is safe when container name contains injection attempts
        let malicious = "MY_DB\".INFORMATION_SCHEMA.TABLES; DROP TABLE users; --";
        let sql = get_tables_in_container_sql("snowflake", malicious).unwrap();
        // The malicious identifier should be fully contained within double quotes
        assert!(sql.contains("\"MY_DB\"\".INFORMATION_SCHEMA.TABLES; DROP TABLE users; --\".INFORMATION_SCHEMA.TABLES"),
            "malicious input should be safely quoted: {sql}");
    }

    #[test]
    fn databricks_tables_sql_injection_prevented() {
        let malicious = "my_catalog`.INFORMATION_SCHEMA.TABLES; DROP TABLE users; --";
        let sql = get_tables_in_container_sql("databricks", malicious).unwrap();
        assert!(sql.contains("`my_catalog``.INFORMATION_SCHEMA.TABLES; DROP TABLE users; --`.INFORMATION_SCHEMA.TABLES"),
            "malicious input should be safely quoted: {sql}");
    }
}
