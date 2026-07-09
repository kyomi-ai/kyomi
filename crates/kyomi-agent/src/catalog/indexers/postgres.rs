// SPDX-License-Identifier: AGPL-3.0-or-later

//! PostgreSQL catalog indexer.
//!
//! Mirrors Python's `datasources/postgres/indexer.py`.
//! Uses `information_schema` for table discovery and `pg_catalog` for column descriptions.

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

/// System schemas excluded from PostgreSQL catalog indexing.
const SYSTEM_SCHEMAS: &[&str] = &[
    "pg_catalog",
    "information_schema",
    "pg_toast",
];

/// Prefixes for temp/toast schemas excluded dynamically.
const SYSTEM_SCHEMA_PREFIXES: &[&str] = &["pg_temp_", "pg_toast_temp_"];

/// Check if a schema name is a PostgreSQL system schema.
fn is_system_schema(name: &str) -> bool {
    let lower = name.to_lowercase();
    SYSTEM_SCHEMAS.iter().any(|s| lower == *s)
        || SYSTEM_SCHEMA_PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// PostgreSQL catalog indexer.
pub struct PostgresIndexer;

#[async_trait]
impl CatalogIndexer for PostgresIndexer {
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
impl SQLCatalogIndexer for PostgresIndexer {
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
            &DatasourceType::Postgres,
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
                 WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast') \
                   AND schema_name NOT LIKE 'pg_temp_%' \
                   AND schema_name NOT LIKE 'pg_toast_temp_%' \
                 ORDER BY schema_name",
                None,
                None,
                false,
                None,
            )
            .await?;

        let names = extract_string_column(&result, 0)?;
        // Double-filter: SQL handles most, but also filter in Rust for robustness
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
                c.column_name, \
                c.data_type, \
                c.is_nullable, \
                COALESCE(pgd.description, '') as description \
             FROM information_schema.columns c \
             LEFT JOIN pg_catalog.pg_statio_all_tables st \
                 ON c.table_schema = st.schemaname \
                 AND c.table_name = st.relname \
             LEFT JOIN pg_catalog.pg_description pgd \
                 ON pgd.objoid = st.relid \
                 AND pgd.objsubid = c.ordinal_position \
             WHERE c.table_schema = '{schema_escaped}' \
               AND c.table_name = '{table_escaped}' \
             ORDER BY c.ordinal_position"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;
        let rows = extract_rows_from_batch(&result)?;

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
        assert!(is_system_schema("information_schema"));
        assert!(is_system_schema("pg_toast"));
        assert!(is_system_schema("pg_temp_1"));
        assert!(is_system_schema("pg_temp_42"));
        assert!(is_system_schema("pg_toast_temp_1"));

        assert!(!is_system_schema("public"));
        assert!(!is_system_schema("myschema"));
        assert!(!is_system_schema("pg_custom"));
    }

    #[test]
    fn container_label_is_schema() {
        let indexer = PostgresIndexer;
        assert_eq!(indexer.container_label(), "schema");
        assert_eq!(indexer.container_config_key(), "catalog_schemas");
    }

    #[test]
    fn build_full_table_id_default() {
        let indexer = PostgresIndexer;
        assert_eq!(
            SQLCatalogIndexer::build_full_table_id(&indexer, "public", "users"),
            "public.users"
        );
    }

    #[test]
    fn project_id_from_connection_config() {
        let indexer = PostgresIndexer;
        let ctx = IndexerContext {
            workspace_id: "ws".into(),
            datasource_config_id: "ds".into(),
            connection_config: serde_json::json!({"database": "mydb"}),
            encryption_key: std::sync::Arc::new([0u8; 32]),
        };
        assert_eq!(indexer.get_project_id(&ctx), "mydb");
    }

    #[test]
    fn project_id_missing_database() {
        let indexer = PostgresIndexer;
        let ctx = IndexerContext {
            workspace_id: "ws".into(),
            datasource_config_id: "ds".into(),
            connection_config: serde_json::json!({}),
            encryption_key: std::sync::Arc::new([0u8; 32]),
        };
        assert_eq!(indexer.get_project_id(&ctx), "");
    }
}
