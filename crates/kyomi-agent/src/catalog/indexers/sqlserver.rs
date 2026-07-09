// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL Server catalog indexer.
//!
//! Mirrors Python's `datasources/sqlserver/indexer.py`.
//! Uses `INFORMATION_SCHEMA` for schema/table/column discovery with
//! `sys.extended_properties` for column descriptions.
//!
//! Shares T-SQL system schema exclusions with the Synapse indexer.

use async_trait::async_trait;
use kyomi_core::datasource_registry::DatasourceType;
use kyomi_core::Result;
use kyomi_datasource_server::DatasourceProvider;
use kyomi_embed::EmbeddingService;
use serde_json::Value;

use super::{extract_rows_from_batch, is_tsql_system_schema, sql_escape, TSQL_SYSTEM_SCHEMAS};
use crate::catalog::traits::{
    index_catalog_sql, CatalogIndexer, SQLCatalogIndexer,
};
use kyomi_auth::catalog::helpers::IndexerContext;
use kyomi_auth::catalog::types::{CatalogIndexResult, ColumnEntry, TableEntry};

/// SQL Server catalog indexer.
pub struct SqlServerIndexer;

#[async_trait]
impl CatalogIndexer for SqlServerIndexer {
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
impl SQLCatalogIndexer for SqlServerIndexer {
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
            &DatasourceType::SqlServer,
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
        let exclusions = TSQL_SYSTEM_SCHEMAS
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "SELECT schema_name \
             FROM INFORMATION_SCHEMA.SCHEMATA \
             WHERE schema_name NOT IN ({exclusions}) \
             ORDER BY schema_name"
        );

        let result = provider.execute_query(&sql, None, None, false, None).await?;
        let rows = extract_rows_from_batch(&result)?;

        Ok(rows
            .iter()
            .filter_map(|row| {
                let name = row.first()?.as_str()?.to_string();
                if is_tsql_system_schema(&name) {
                    None
                } else {
                    Some(name)
                }
            })
            .collect())
    }

    async fn get_tables_in_container(
        &self,
        provider: &dyn DatasourceProvider,
        container_name: &str,
        max_tables: Option<usize>,
    ) -> Result<Vec<TableEntry>> {
        let schema_escaped = sql_escape(container_name);

        // SQL Server uses TOP instead of LIMIT
        let top_clause = max_tables
            .map(|n| format!("TOP {n}"))
            .unwrap_or_default();

        let sql = format!(
            "SELECT {top_clause} TABLE_NAME, TABLE_TYPE \
             FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA = '{schema_escaped}' \
               AND TABLE_TYPE IN ('BASE TABLE', 'VIEW') \
             ORDER BY TABLE_NAME"
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
                c.COLUMN_NAME, \
                c.DATA_TYPE, \
                ISNULL(ep.value, '') AS description \
             FROM INFORMATION_SCHEMA.COLUMNS c \
             LEFT JOIN sys.columns sc ON sc.name = c.COLUMN_NAME \
             LEFT JOIN sys.tables st ON sc.object_id = st.object_id AND st.name = c.TABLE_NAME \
             LEFT JOIN sys.schemas ss ON st.schema_id = ss.schema_id AND ss.name = c.TABLE_SCHEMA \
             LEFT JOIN sys.extended_properties ep ON ep.major_id = st.object_id \
                 AND ep.minor_id = sc.column_id AND ep.name = 'MS_Description' \
             WHERE c.TABLE_SCHEMA = '{schema_escaped}' AND c.TABLE_NAME = '{table_escaped}' \
             ORDER BY c.ORDINAL_POSITION"
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
    fn container_label_is_schema() {
        let indexer = SqlServerIndexer;
        assert_eq!(indexer.container_label(), "schema");
        assert_eq!(indexer.container_config_key(), "catalog_schemas");
    }

    #[test]
    fn build_full_table_id_default() {
        let indexer = SqlServerIndexer;
        assert_eq!(
            SQLCatalogIndexer::build_full_table_id(&indexer, "dbo", "users"),
            "dbo.users"
        );
    }
}
