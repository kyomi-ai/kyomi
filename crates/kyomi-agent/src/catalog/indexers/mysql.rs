// SPDX-License-Identifier: AGPL-3.0-or-later

//! MySQL catalog indexer.
//!
//! Mirrors Python's `datasources/mysql/indexer.py`.
//! Uses `information_schema` for database, table, and column discovery.

use async_trait::async_trait;
use kyomi_core::datasource_registry::DatasourceType;
use kyomi_core::Result;
use kyomi_datasource_server::DatasourceProvider;
use kyomi_embed::EmbeddingService;
use serde_json::Value;

use super::{ensure_query_ok, extract_rows_from_batch, extract_string_column, sql_escape};
use crate::catalog::traits::{
    index_catalog_sql, CatalogIndexer, SQLCatalogIndexer,
};
use kyomi_auth::catalog::helpers::IndexerContext;
use kyomi_auth::catalog::types::{CatalogIndexResult, ColumnEntry, TableEntry};

/// System databases excluded from MySQL catalog indexing.
const SYSTEM_DATABASES: &[&str] = &[
    "information_schema",
    "mysql",
    "performance_schema",
    "sys",
];

/// Check if a database name is a MySQL system database (case-insensitive).
fn is_system_database(name: &str) -> bool {
    let lower = name.to_lowercase();
    SYSTEM_DATABASES.iter().any(|s| lower == *s)
}

/// MySQL catalog indexer.
pub struct MySqlIndexer;

#[async_trait]
impl CatalogIndexer for MySqlIndexer {
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
impl SQLCatalogIndexer for MySqlIndexer {
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
            &DatasourceType::MySQL,
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
            "SELECT schema_name \
             FROM information_schema.schemata \
             WHERE schema_name NOT IN ({exclusions}) \
             ORDER BY schema_name"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;
        ensure_query_ok(&result, "discovering databases")?;
        let names = extract_string_column(&result, 0);
        // Double-filter in Rust for robustness
        Ok(names.into_iter().filter(|n| !is_system_database(n)).collect())
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
            "SELECT table_name, table_type \
             FROM information_schema.tables \
             WHERE table_schema = '{db_escaped}' \
               AND table_type IN ('BASE TABLE', 'VIEW') \
             ORDER BY table_name {limit_clause}"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;
        ensure_query_ok(&result, &format!("listing tables in database '{container_name}'"))?;
        let rows = extract_rows_from_batch(&result);

        Ok(rows
            .iter()
            .filter_map(|row| {
                let name = row.first()?.as_str()?.to_string();
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
                column_name, \
                data_type, \
                is_nullable, \
                column_comment \
             FROM information_schema.columns \
             WHERE table_schema = '{db_escaped}' \
               AND table_name = '{table_escaped}' \
             ORDER BY ordinal_position"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;
        ensure_query_ok(
            &result,
            &format!("listing columns for '{container_name}.{table_name}'"),
        )?;
        let rows = extract_rows_from_batch(&result);

        Ok(rows
            .iter()
            .filter_map(|row| {
                let name = row.first()?.as_str()?.to_string();
                let native_type = row.get(1).and_then(|v| v.as_str()).map(String::from);
                let description = row
                    .get(3)
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
        assert!(is_system_database("information_schema"));
        assert!(is_system_database("INFORMATION_SCHEMA"));
        assert!(is_system_database("mysql"));
        assert!(is_system_database("performance_schema"));
        assert!(is_system_database("sys"));

        assert!(!is_system_database("mydb"));
        assert!(!is_system_database("app_data"));
    }

    #[test]
    fn container_label_is_database() {
        let indexer = MySqlIndexer;
        assert_eq!(indexer.container_label(), "database");
        assert_eq!(indexer.container_config_key(), "catalog_databases");
    }

    #[test]
    fn build_full_table_id_default() {
        let indexer = MySqlIndexer;
        assert_eq!(
            SQLCatalogIndexer::build_full_table_id(&indexer, "mydb", "users"),
            "mydb.users"
        );
    }
}
