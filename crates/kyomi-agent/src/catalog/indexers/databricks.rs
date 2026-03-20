// SPDX-License-Identifier: AGPL-3.0-or-later

//! Databricks catalog indexer.
//!
//! Mirrors Python's `datasources/databricks/indexer.py`.
//!
//! Databricks Unity Catalog hierarchy: Catalog > Schema > Table
//! Storage: dataset_id = "catalog.schema", table_id = "table"
//! Uses SHOW CATALOGS, SHOW SCHEMAS, SHOW TABLES, DESCRIBE TABLE.

use async_trait::async_trait;
use kyomi_core::datasource_registry::DatasourceType;
use kyomi_core::Result;
use kyomi_datasource_server::{DatasourceProvider, QueryStatus};
use kyomi_embed::EmbeddingService;
use serde_json::Value;

use crate::catalog::traits::{
    index_catalog_sql, CatalogIndexer, SQLCatalogIndexer,
};
use kyomi_auth::catalog::helpers::IndexerContext;
use kyomi_auth::catalog::types::{CatalogIndexResult, ColumnEntry, TableEntry};

/// System catalogs excluded from Databricks catalog indexing.
const SYSTEM_CATALOGS: &[&str] = &["system", "hive_metastore"];

/// System schemas excluded from Databricks catalog indexing.
const SYSTEM_SCHEMAS: &[&str] = &["information_schema", "__internal"];

/// Check if a catalog name is a Databricks system catalog (case-insensitive).
fn is_system_catalog(name: &str) -> bool {
    let lower = name.to_lowercase();
    SYSTEM_CATALOGS.iter().any(|s| lower == *s)
}

/// Check if a schema name is a Databricks system schema (case-insensitive).
fn is_system_schema(name: &str) -> bool {
    let lower = name.to_lowercase();
    SYSTEM_SCHEMAS.iter().any(|s| lower == *s)
}

/// Databricks catalog indexer.
///
/// Container = catalog (top-level). Within each catalog, discovers schemas
/// and tables. Stores `dataset_id = "catalog.schema"` for 3-part naming.
pub struct DatabricksIndexer;

#[async_trait]
impl CatalogIndexer for DatabricksIndexer {
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
impl SQLCatalogIndexer for DatabricksIndexer {
    fn container_label(&self) -> &str {
        "catalog"
    }

    fn container_config_key(&self) -> &str {
        "catalog_catalogs"
    }

    async fn create_provider(
        &self,
        connection_config: &Value,
        credentials: &Value,
    ) -> Result<Box<dyn DatasourceProvider>> {
        kyomi_datasource_server::create_provider(
            &DatasourceType::Databricks,
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
            .execute_query("SHOW CATALOGS", None, None, false)
            .await?;

        if result.status != QueryStatus::Success {
            return Ok(Vec::new());
        }
        let Some(rows) = &result.rows else {
            return Ok(Vec::new());
        };

        let mut catalogs: Vec<String> = rows
            .iter()
            .filter_map(|row| {
                let name = row.first()?.as_str()?.to_string();
                if is_system_catalog(&name) {
                    None
                } else {
                    Some(name)
                }
            })
            .collect();

        catalogs.sort();
        Ok(catalogs)
    }

    async fn get_tables_in_container(
        &self,
        provider: &dyn DatasourceProvider,
        container_name: &str,
        max_tables: Option<usize>,
    ) -> Result<Vec<TableEntry>> {
        // First get all schemas in the catalog
        let schema_sql = format!("SHOW SCHEMAS IN `{container_name}`");
        let schema_result = provider
            .execute_query(&schema_sql, None, None, false)
            .await?;

        if schema_result.status != QueryStatus::Success {
            return Ok(Vec::new());
        }
        let Some(schema_rows) = &schema_result.rows else {
            return Ok(Vec::new());
        };

        let mut tables = Vec::new();

        for schema_row in schema_rows {
            let Some(schema_name) = schema_row.first().and_then(|v| v.as_str()) else {
                continue;
            };

            if is_system_schema(schema_name) {
                continue;
            }

            // Get tables in this schema
            let table_sql = format!("SHOW TABLES IN `{container_name}`.`{schema_name}`");
            let table_result = provider
                .execute_query(&table_sql, None, None, false)
                .await;

            let table_result = match table_result {
                Ok(r) => r,
                Err(_) => continue,
            };

            if table_result.status != QueryStatus::Success {
                continue;
            }
            let Some(table_rows) = &table_result.rows else {
                continue;
            };

            for row in table_rows {
                // SHOW TABLES returns: (database, tableName, isTemporary)
                let table_name = row
                    .get(1)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                if table_name.is_empty() {
                    continue;
                }

                tables.push(TableEntry {
                    name: table_name,
                    table_type: Some("TABLE".into()),
                    dataset_override: Some(format!("{container_name}.{schema_name}")),
                });

                if let Some(max) = max_tables {
                    if tables.len() >= max {
                        return Ok(tables);
                    }
                }
            }
        }

        Ok(tables)
    }

    async fn get_table_columns(
        &self,
        provider: &dyn DatasourceProvider,
        container_name: &str,
        table_name: &str,
    ) -> Result<Vec<ColumnEntry>> {
        // container_name is "catalog.schema" (from dataset_override)
        let Some((catalog_name, schema_name)) = container_name.split_once('.') else {
            return Ok(Vec::new());
        };

        let sql = format!(
            "DESCRIBE TABLE `{catalog_name}`.`{schema_name}`.`{table_name}`"
        );

        let result = provider.execute_query(&sql, None, None, false).await?;

        if result.status != QueryStatus::Success {
            return Ok(Vec::new());
        }
        let Some(rows) = &result.rows else {
            return Ok(Vec::new());
        };

        Ok(rows
            .iter()
            .filter_map(|row| {
                let name = row.first()?.as_str()?.to_string();

                // Skip partition info rows (start with '#')
                if name.starts_with('#') {
                    return None;
                }

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
    fn system_catalog_detection() {
        assert!(is_system_catalog("system"));
        assert!(is_system_catalog("SYSTEM"));
        assert!(is_system_catalog("hive_metastore"));
        assert!(is_system_catalog("HIVE_METASTORE"));

        assert!(!is_system_catalog("main"));
        assert!(!is_system_catalog("my_catalog"));
    }

    #[test]
    fn system_schema_detection() {
        assert!(is_system_schema("information_schema"));
        assert!(is_system_schema("INFORMATION_SCHEMA"));
        assert!(is_system_schema("__internal"));

        assert!(!is_system_schema("default"));
        assert!(!is_system_schema("my_schema"));
    }

    #[test]
    fn container_label_is_catalog() {
        let indexer = DatabricksIndexer;
        assert_eq!(indexer.container_label(), "catalog");
        assert_eq!(indexer.container_config_key(), "catalog_catalogs");
    }

    #[test]
    fn build_full_table_id_default() {
        let indexer = DatabricksIndexer;
        // dataset_id = "main.default", table = "orders"
        assert_eq!(
            SQLCatalogIndexer::build_full_table_id(&indexer, "main.default", "orders"),
            "main.default.orders"
        );
    }
}
