// SPDX-License-Identifier: AGPL-3.0-or-later

//! FlareDB catalog indexer.
//!
//! FlareDB exposes DataFusion's `information_schema` for catalog discovery.
//! Hierarchy: schema > table (no database concept; connection is already scoped).

use async_trait::async_trait;
use kyomi_core::datasource_registry::DatasourceType;
use kyomi_core::Result;
use kyomi_datasource_server::DatasourceProvider;
use kyomi_embed::EmbeddingService;
use serde_json::Value;

use super::{extract_rows_from_batch, extract_string_column, sql_escape};
use crate::catalog::traits::{
    index_catalog_sql, CatalogIndexer, SQLCatalogIndexer,
};
use kyomi_auth::catalog::helpers::IndexerContext;
use kyomi_auth::catalog::types::{CatalogIndexResult, ColumnEntry, TableEntry};

/// System schemas exposed by DataFusion that should not be indexed.
const SYSTEM_SCHEMAS: &[&str] = &[
    "information_schema",
    "pg_catalog",
];

/// Check if a schema name is a FlareDB/DataFusion system schema.
fn is_system_schema(name: &str) -> bool {
    let lower = name.to_lowercase();
    SYSTEM_SCHEMAS.iter().any(|s| lower == *s)
}

/// FlareDB catalog indexer.
pub struct FlareDbIndexer;

#[async_trait]
impl CatalogIndexer for FlareDbIndexer {
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
impl SQLCatalogIndexer for FlareDbIndexer {
    fn container_label(&self) -> &str {
        "schema"
    }

    fn container_config_key(&self) -> &str {
        "catalog_schemas"
    }

    async fn create_provider(
        &self,
        connection_config: &Value,
        credentials: &Value,
    ) -> Result<Box<dyn DatasourceProvider>> {
        kyomi_datasource_server::create_provider(
            &DatasourceType::FlareDb,
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
            .execute_query(
                "SELECT schema_name \
                 FROM information_schema.schemata \
                 WHERE schema_name NOT IN ('information_schema', 'pg_catalog') \
                 ORDER BY schema_name",
                None,
                None,
                false,
                None,
            )
            .await?;

        let names = extract_string_column(&result, 0)?;
        // Double-filter in Rust for robustness
        Ok(names.into_iter().filter(|n| !is_system_schema(n)).collect())
    }

    async fn get_tables_in_container(
        &self,
        provider: &dyn DatasourceProvider,
        container_name: &str,
        max_tables: Option<usize>,
    ) -> Result<Vec<TableEntry>> {
        let schema_escaped = sql_escape(container_name);
        let limit_clause = max_tables
            .map(|n| format!("LIMIT {n}"))
            .unwrap_or_default();

        let sql = format!(
            "SELECT table_name, table_type \
             FROM information_schema.tables \
             WHERE table_schema = '{schema_escaped}' \
             ORDER BY table_name {limit_clause}"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;
        let rows = extract_rows_from_batch(&result)?;

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
        let schema_escaped = sql_escape(container_name);
        let table_escaped = sql_escape(table_name);

        let sql = format!(
            "SELECT \
                column_name, \
                data_type \
             FROM information_schema.columns \
             WHERE table_schema = '{schema_escaped}' \
               AND table_name = '{table_escaped}' \
             ORDER BY ordinal_position"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;
        let rows = extract_rows_from_batch(&result)?;

        Ok(rows
            .iter()
            .filter_map(|row| {
                let name = row.first()?.as_str()?.to_string();
                let native_type = row.get(1).and_then(|v| v.as_str()).map(String::from);

                Some(ColumnEntry {
                    name,
                    col_type: native_type.clone(),
                    native_type,
                    description: None,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_schema_detection() {
        assert!(is_system_schema("information_schema"));
        assert!(is_system_schema("INFORMATION_SCHEMA"));
        assert!(is_system_schema("pg_catalog"));
        assert!(is_system_schema("PG_CATALOG"));

        assert!(!is_system_schema("public"));
        assert!(!is_system_schema("myschema"));
        assert!(!is_system_schema("analytics"));
    }

    #[test]
    fn container_label_is_schema() {
        let indexer = FlareDbIndexer;
        assert_eq!(indexer.container_label(), "schema");
        assert_eq!(indexer.container_config_key(), "catalog_schemas");
    }

    #[test]
    fn build_full_table_id_default() {
        let indexer = FlareDbIndexer;
        assert_eq!(
            SQLCatalogIndexer::build_full_table_id(&indexer, "public", "events"),
            "public.events"
        );
    }
}
