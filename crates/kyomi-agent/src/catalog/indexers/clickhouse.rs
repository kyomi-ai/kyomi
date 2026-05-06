// SPDX-License-Identifier: AGPL-3.0-or-later

//! ClickHouse catalog indexer.
//!
//! Mirrors Python's `datasources/clickhouse/indexer.py`.
//! Uses `system.databases`, `system.tables`, `system.columns` for discovery.
//!
//! ClickHouse hierarchy: database > table (no schema concept).

use async_trait::async_trait;
use kyomi_core::datasource_registry::DatasourceType;
use kyomi_core::Result;
use kyomi_datasource_server::{DatasourceProvider, QueryStatus};
use kyomi_embed::EmbeddingService;
use serde_json::Value;

use super::{extract_string_column, sql_escape};
use crate::catalog::traits::{
    index_catalog_sql, CatalogIndexer, SQLCatalogIndexer,
};
use kyomi_auth::catalog::helpers::IndexerContext;
use kyomi_auth::catalog::types::{CatalogIndexResult, ColumnEntry, TableEntry};

/// System databases excluded from ClickHouse catalog indexing.
const SYSTEM_DATABASES: &[&str] = &[
    "system",
    "information_schema",
    "INFORMATION_SCHEMA",
];

/// Check if a database name is a ClickHouse system database.
fn is_system_database(name: &str) -> bool {
    SYSTEM_DATABASES.contains(&name)
}

/// ClickHouse catalog indexer.
pub struct ClickHouseIndexer;

#[async_trait]
impl CatalogIndexer for ClickHouseIndexer {
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
impl SQLCatalogIndexer for ClickHouseIndexer {
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
            &DatasourceType::ClickHouse,
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
        let exclusions = SYSTEM_DATABASES
            .iter()
            .map(|db| format!("'{db}'"))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "SELECT name \
             FROM system.databases \
             WHERE name NOT IN ({exclusions}) \
             ORDER BY name"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;
        let names = extract_string_column(&result, 0);
        // Double-filter in Rust for case-sensitive ClickHouse names
        Ok(names
            .into_iter()
            .filter(|n| !is_system_database(n))
            .collect())
    }

    async fn get_tables_in_container(
        &self,
        provider: &dyn DatasourceProvider,
        container_name: &str,
        max_tables: Option<usize>,
    ) -> Result<Vec<TableEntry>> {
        let db_escaped = sql_escape(container_name);
        let limit_clause = max_tables
            .map(|n| format!("LIMIT {n}"))
            .unwrap_or_default();

        let sql = format!(
            "SELECT name, engine \
             FROM system.tables \
             WHERE database = '{db_escaped}' \
               AND name NOT LIKE '.inner_id.%' \
             ORDER BY name {limit_clause}"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;

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
                // ClickHouse uses engine type instead of TABLE/VIEW
                let table_type = row.get(1).and_then(|v| v.as_str()).map(String::from);
                Some(TableEntry { name, table_type, dataset_override: None })
            })
            .collect())
    }

    async fn get_table_columns(
        &self,
        provider: &dyn DatasourceProvider,
        container_name: &str,
        table_name: &str,
    ) -> Result<Vec<ColumnEntry>> {
        let db_escaped = sql_escape(container_name);
        let table_escaped = sql_escape(table_name);

        let sql = format!(
            "SELECT \
                name, \
                type, \
                comment \
             FROM system.columns \
             WHERE database = '{db_escaped}' \
               AND table = '{table_escaped}' \
             ORDER BY position"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;

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
        assert!(is_system_database("system"));
        assert!(is_system_database("information_schema"));
        assert!(is_system_database("INFORMATION_SCHEMA"));

        // ClickHouse is case-sensitive, so these should NOT match
        assert!(!is_system_database("System"));
        assert!(!is_system_database("SYSTEM"));
        assert!(!is_system_database("mydb"));
    }

    #[test]
    fn container_label_is_database() {
        let indexer = ClickHouseIndexer;
        assert_eq!(indexer.container_label(), "database");
        assert_eq!(indexer.container_config_key(), "catalog_databases");
    }

    #[test]
    fn build_full_table_id_default() {
        let indexer = ClickHouseIndexer;
        assert_eq!(
            SQLCatalogIndexer::build_full_table_id(&indexer, "default", "events"),
            "default.events"
        );
    }
}
