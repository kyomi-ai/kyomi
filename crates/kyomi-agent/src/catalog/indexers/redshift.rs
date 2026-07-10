// SPDX-License-Identifier: AGPL-3.0-or-later

//! Amazon Redshift catalog indexer.
//!
//! Mirrors Python's `datasources/redshift/indexer.py`.
//! Schema discovery uses `svv_all_schemas` rather than `information_schema.schemata`:
//! on Redshift, `information_schema.schemata` only returns schemas owned by the
//! current user, silently omitting schemas the user can read but doesn't own.
//! Table discovery still uses `information_schema.tables`, and column metadata
//! (including descriptions) comes from `svv_columns`.

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

/// System schemas excluded from Redshift catalog indexing.
const SYSTEM_SCHEMAS: &[&str] = &[
    "pg_catalog",
    "pg_internal",
    "information_schema",
    "pg_toast",
    "pg_automv",
    "pg_auto_copy",
    "pg_mv",
    "pg_s3",
    "catalog_history",
];

/// Prefixes for temp schemas excluded dynamically.
const SYSTEM_SCHEMA_PREFIXES: &[&str] = &["pg_temp_"];

/// Check if a schema name is a Redshift system schema (case-insensitive).
fn is_system_schema(name: &str) -> bool {
    let lower = name.to_lowercase();
    SYSTEM_SCHEMAS.iter().any(|s| lower == *s)
        || SYSTEM_SCHEMA_PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// Amazon Redshift catalog indexer.
pub struct RedshiftIndexer;

#[async_trait]
impl CatalogIndexer for RedshiftIndexer {
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
impl SQLCatalogIndexer for RedshiftIndexer {
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
            &DatasourceType::Redshift,
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
                 FROM svv_all_schemas \
                 WHERE database_name = current_database() \
                   AND schema_name NOT IN ('information_schema', 'pg_catalog', 'pg_internal', 'pg_toast', \
                                           'pg_automv', 'pg_auto_copy', 'pg_mv', 'pg_s3', 'catalog_history') \
                 ORDER BY schema_name",
                None,
                None,
                false,
                None,
            )
            .await?;

        let names = extract_string_column(&result, 0);
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
               AND table_type IN ('BASE TABLE', 'VIEW') \
             ORDER BY table_name {limit_clause}"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;
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
        let schema_escaped = sql_escape(container_name);
        let table_escaped = sql_escape(table_name);

        let sql = format!(
            "SELECT \
                column_name, \
                data_type, \
                is_nullable, \
                COALESCE(remarks, '') as description \
             FROM svv_columns \
             WHERE table_schema = '{schema_escaped}' \
               AND table_name = '{table_escaped}' \
             ORDER BY ordinal_position"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;
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

    fn get_project_id(&self, ctx: &IndexerContext) -> String {
        ctx.connection_config
            .get("database")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_schema_detection() {
        assert!(is_system_schema("pg_catalog"));
        assert!(is_system_schema("PG_CATALOG"));
        assert!(is_system_schema("pg_internal"));
        assert!(is_system_schema("information_schema"));
        assert!(is_system_schema("pg_toast"));
        assert!(is_system_schema("pg_temp_1"));
        assert!(is_system_schema("pg_temp_99"));
        assert!(is_system_schema("pg_automv"));
        assert!(is_system_schema("PG_AUTOMV"));
        assert!(is_system_schema("pg_auto_copy"));
        assert!(is_system_schema("pg_mv"));
        assert!(is_system_schema("pg_s3"));
        assert!(is_system_schema("catalog_history"));

        assert!(!is_system_schema("public"));
        assert!(!is_system_schema("myschema"));
        assert!(!is_system_schema("analytics"));
    }

    #[test]
    fn container_label_is_schema() {
        let indexer = RedshiftIndexer;
        assert_eq!(indexer.container_label(), "schema");
        assert_eq!(indexer.container_config_key(), "catalog_schemas");
    }

    #[test]
    fn build_full_table_id_default() {
        let indexer = RedshiftIndexer;
        assert_eq!(
            SQLCatalogIndexer::build_full_table_id(&indexer, "public", "orders"),
            "public.orders"
        );
    }

    #[test]
    fn project_id_from_connection_config() {
        let indexer = RedshiftIndexer;
        let ctx = IndexerContext {
            workspace_id: "ws".into(),
            datasource_config_id: "ds".into(),
            connection_config: serde_json::json!({"database": "analytics"}),
            encryption_key: std::sync::Arc::new([0u8; 32]),
        };
        assert_eq!(indexer.get_project_id(&ctx), "analytics");
    }
}
