// SPDX-License-Identifier: AGPL-3.0-or-later

//! Snowflake catalog indexer.
//!
//! Mirrors Python's `datasources/snowflake/indexer.py`.
//!
//! Snowflake hierarchy: Account > Database > Schema > Table
//! Storage: dataset_id = "database.schema", table_id = "table_name"
//! Uses INFORMATION_SCHEMA for table and column discovery.

use async_trait::async_trait;
use kyomi_core::datasource_registry::DatasourceType;
use kyomi_core::Result;
use kyomi_datasource_server::DatasourceProvider;
use kyomi_embed::EmbeddingService;
use serde_json::Value;

use super::{extract_rows_from_batch, sql_escape};
use crate::catalog::traits::{
    index_catalog_sql, CatalogIndexer, SQLCatalogIndexer,
};
use kyomi_auth::catalog::helpers::IndexerContext;
use kyomi_auth::catalog::types::{CatalogIndexResult, ColumnEntry, TableEntry};

/// System databases excluded from Snowflake catalog indexing.
const SYSTEM_DATABASES: &[&str] = &["SNOWFLAKE", "SNOWFLAKE_SAMPLE_DATA"];

/// System schemas excluded from Snowflake catalog indexing.
const SYSTEM_SCHEMAS: &[&str] = &["INFORMATION_SCHEMA"];

/// Check if a database name is a Snowflake system database (case-insensitive).
fn is_system_database(name: &str) -> bool {
    let upper = name.to_uppercase();
    SYSTEM_DATABASES.iter().any(|s| upper == *s)
}

/// Check if a schema name is a Snowflake system schema (case-insensitive).
fn is_system_schema(name: &str) -> bool {
    let upper = name.to_uppercase();
    SYSTEM_SCHEMAS.iter().any(|s| upper == *s)
}

/// Snowflake catalog indexer.
///
/// Container = database (top-level). Within each database, discovers schemas
/// and tables. Stores `dataset_id = "database.schema"` for 3-part naming.
pub struct SnowflakeIndexer;

#[async_trait]
impl CatalogIndexer for SnowflakeIndexer {
    async fn index_catalog(
        &self,
        ctx: &IndexerContext,
        db: &kyomi_core::DbPool,
        embedding: &EmbeddingService,
        user_email: Option<&str>,
        credentials: Option<&Value>,
        max_tables_per_dataset: Option<usize>,
    ) -> CatalogIndexResult {
        index_catalog_sql(self, ctx, db, embedding, user_email, credentials, max_tables_per_dataset)
            .await
    }
}

#[async_trait]
impl SQLCatalogIndexer for SnowflakeIndexer {
    fn container_label(&self) -> &str {
        "database"
    }

    fn container_config_key(&self) -> &str {
        "catalog_databases"
    }

    async fn create_provider(
        &self,
        connection_config: &Value,
        credentials: &Value,
    ) -> Result<Box<dyn DatasourceProvider>> {
        kyomi_datasource_server::create_provider(
            &DatasourceType::Snowflake,
            connection_config,
            credentials,
            None,
        )
        .await
    }

    async fn discover_all_containers(
        &self,
        provider: &dyn DatasourceProvider,
    ) -> Result<Vec<String>> {
        let result = provider
            .execute_query("SHOW DATABASES", None, None, false, None)
            .await?;

        // SHOW DATABASES returns: (created_on, name, is_default, is_current, ...)
        let rows = extract_rows_from_batch(&result)?;
        let mut databases: Vec<String> = rows
            .iter()
            .filter_map(|row| {
                let db_name = row
                    .get(1)
                    .and_then(|v| v.as_str())
                    .or_else(|| row.first().and_then(|v| v.as_str()))?
                    .to_string();
                if is_system_database(&db_name) {
                    None
                } else {
                    Some(db_name)
                }
            })
            .collect();

        databases.sort();
        Ok(databases)
    }

    async fn get_tables_in_container(
        &self,
        provider: &dyn DatasourceProvider,
        container_name: &str,
        max_tables: Option<usize>,
    ) -> Result<Vec<TableEntry>> {
        let limit_clause = max_tables
            .map(|n| format!("LIMIT {n}"))
            .unwrap_or_default();

        // Query all tables across all schemas in this database
        let sql = format!(
            "SELECT \
                TABLE_SCHEMA, \
                TABLE_NAME, \
                TABLE_TYPE \
             FROM {container_name}.INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA NOT IN ('INFORMATION_SCHEMA') \
               AND TABLE_TYPE IN ('BASE TABLE', 'VIEW') \
             ORDER BY TABLE_SCHEMA, TABLE_NAME {limit_clause}"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;
        let rows = extract_rows_from_batch(&result)?;

        Ok(rows
            .iter()
            .filter_map(|row| {
                let schema_name = row.first()?.as_str()?;
                let table_name = row.get(1)?.as_str()?.to_string();
                let table_type = row.get(2).and_then(|v| v.as_str()).map(String::from);

                if is_system_schema(schema_name) {
                    return None;
                }

                Some(TableEntry {
                    name: table_name,
                    table_type,
                    dataset_override: Some(format!("{container_name}.{schema_name}")),
                })
            })
            .collect())
    }

    async fn get_table_columns(
        &self,
        provider: &dyn DatasourceProvider,
        container_name: &str,
        table_name: &str,
    ) -> Result<Vec<ColumnEntry>> {
        // container_name is "database.schema" (from dataset_override)
        let (database_name, schema_name) = match container_name.split_once('.') {
            Some((db, schema)) => (db, schema),
            None => (container_name, "PUBLIC"),
        };

        let schema_escaped = sql_escape(schema_name);
        let table_escaped = sql_escape(table_name);

        let sql = format!(
            "SELECT \
                COLUMN_NAME, \
                DATA_TYPE, \
                COMMENT \
             FROM {database_name}.INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = '{schema_escaped}' \
               AND TABLE_NAME = '{table_escaped}' \
             ORDER BY ORDINAL_POSITION"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;
        let rows = extract_rows_from_batch(&result)?;

        Ok(rows
            .iter()
            .filter_map(|row| {
                let name = row.first()?.as_str()?.to_string();
                let native_type = row.get(1).and_then(|v| v.as_str()).map(String::from);
                let description = row
                    .get(2)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from);

                Some(ColumnEntry {
                    name,
                    col_type: native_type.clone(),
                    native_type,
                    description,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_database_detection() {
        assert!(is_system_database("SNOWFLAKE"));
        assert!(is_system_database("snowflake"));
        assert!(is_system_database("SNOWFLAKE_SAMPLE_DATA"));
        assert!(is_system_database("Snowflake_Sample_Data"));

        assert!(!is_system_database("my_database"));
        assert!(!is_system_database("analytics"));
    }

    #[test]
    fn system_schema_detection() {
        assert!(is_system_schema("INFORMATION_SCHEMA"));
        assert!(is_system_schema("information_schema"));

        assert!(!is_system_schema("PUBLIC"));
        assert!(!is_system_schema("my_schema"));
    }

    #[test]
    fn container_label_is_database() {
        let indexer = SnowflakeIndexer;
        assert_eq!(indexer.container_label(), "database");
        assert_eq!(indexer.container_config_key(), "catalog_databases");
    }

    #[test]
    fn build_full_table_id_default() {
        let indexer = SnowflakeIndexer;
        // dataset_id = "MY_DB.PUBLIC", table = "orders"
        assert_eq!(
            SQLCatalogIndexer::build_full_table_id(&indexer, "MY_DB.PUBLIC", "orders"),
            "MY_DB.PUBLIC.orders"
        );
    }
}
