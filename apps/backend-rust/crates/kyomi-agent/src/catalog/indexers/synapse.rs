// SPDX-License-Identifier: AGPL-3.0-or-later

//! Azure Synapse catalog indexer.
//!
//! Mirrors Python's `datasources/synapse/indexer.py`.
//! Uses the same T-SQL queries as SQL Server -- same system schema exclusions,
//! same INFORMATION_SCHEMA queries, same extended property lookups.
//!
//! The only difference from SQL Server is the `DatasourceType::Synapse` used
//! for provider creation and the column description query (uses OBJECT_ID
//! function instead of multi-table join for extended properties).

use async_trait::async_trait;
use kyomi_core::datasource_registry::DatasourceType;
use kyomi_core::Result;
use kyomi_datasource_server::{DatasourceProvider, QueryStatus};
use kyomi_embed::EmbeddingService;
use serde_json::Value;

use super::{is_tsql_system_schema, sql_escape, TSQL_SYSTEM_SCHEMAS};
use crate::catalog::traits::{
    index_catalog_sql, CatalogIndexer, SQLCatalogIndexer,
};
use kyomi_auth::catalog::helpers::IndexerContext;
use kyomi_auth::catalog::types::{CatalogIndexResult, ColumnEntry, TableEntry};

/// Azure Synapse catalog indexer.
pub struct SynapseIndexer;

#[async_trait]
impl CatalogIndexer for SynapseIndexer {
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
impl SQLCatalogIndexer for SynapseIndexer {
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
            &DatasourceType::Synapse,
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
            "SELECT SCHEMA_NAME \
             FROM INFORMATION_SCHEMA.SCHEMATA \
             WHERE SCHEMA_NAME NOT IN ({exclusions}) \
             ORDER BY SCHEMA_NAME"
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

        // T-SQL uses TOP instead of LIMIT
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

        // Synapse uses OBJECT_ID function for extended property lookups
        let sql = format!(
            "SELECT \
                c.COLUMN_NAME, \
                c.DATA_TYPE, \
                ISNULL(CAST(ep.value AS NVARCHAR(MAX)), '') AS column_description \
             FROM INFORMATION_SCHEMA.COLUMNS c \
             LEFT JOIN sys.columns sc ON sc.name = c.COLUMN_NAME \
                 AND sc.object_id = OBJECT_ID('{schema_escaped}.{table_escaped}') \
             LEFT JOIN sys.extended_properties ep ON ep.major_id = sc.object_id \
                 AND ep.minor_id = sc.column_id AND ep.name = 'MS_Description' \
             WHERE c.TABLE_SCHEMA = '{schema_escaped}' AND c.TABLE_NAME = '{table_escaped}' \
             ORDER BY c.ORDINAL_POSITION"
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
        let indexer = SynapseIndexer;
        assert_eq!(indexer.container_label(), "schema");
        assert_eq!(indexer.container_config_key(), "catalog_schemas");
    }

    #[test]
    fn build_full_table_id_default() {
        let indexer = SynapseIndexer;
        assert_eq!(
            SQLCatalogIndexer::build_full_table_id(&indexer, "dbo", "sales"),
            "dbo.sales"
        );
    }
}
