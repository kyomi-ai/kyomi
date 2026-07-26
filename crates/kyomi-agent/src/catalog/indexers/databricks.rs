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
use kyomi_datasource_server::DatasourceProvider;
use kyomi_embed::EmbeddingService;
use serde_json::Value;

use tracing::warn;

use super::{ensure_query_ok, extract_rows_from_batch};
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
            .execute_query("SHOW CATALOGS", None, None, false, None)
            .await?;
        ensure_query_ok(&result, "discovering catalogs")?;

        let rows = extract_rows_from_batch(&result);
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
        let (tables, partial_failures) =
            discover_tables_in_catalog(provider, container_name, max_tables).await?;
        // Any per-schema partial failures are dropped here — callers that
        // need them (the `index_catalog_sql` template) go through
        // `get_tables_in_container_with_partial_failures` instead, which
        // shares this same implementation. This plain method exists only to
        // satisfy the `SQLCatalogIndexer` trait contract for callers that
        // don't need the partial-failure channel.
        let _ = partial_failures;
        Ok(tables)
    }

    async fn get_tables_in_container_with_partial_failures(
        &self,
        provider: &dyn DatasourceProvider,
        container_name: &str,
        max_tables: Option<usize>,
    ) -> Result<(Vec<TableEntry>, Vec<String>)> {
        discover_tables_in_catalog(provider, container_name, max_tables).await
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

        let result = provider.execute_query(&sql, None, None, false, None).await?;
        ensure_query_ok(
            &result,
            &format!("listing columns for '{catalog_name}.{schema_name}.{table_name}'"),
        )?;
        let rows = extract_rows_from_batch(&result);

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

/// Discover all tables across all (non-system) schemas in a Databricks
/// catalog.
///
/// Shared by both `get_tables_in_container` and
/// `get_tables_in_container_with_partial_failures` — the two trait methods
/// differ only in whether the caller wants the per-schema partial-failure
/// messages surfaced.
///
/// A failure to list schemas at all aborts the whole catalog (propagated via
/// `?`) — matching how the outer loop in `index_catalog_sql` treats a
/// `get_tables_in_container` failure: the container is recorded as errored
/// and the crawl moves on to the next one.
///
/// A single inaccessible schema (permission denied, transient failure) does
/// NOT abort the whole catalog crawl: it is recorded as a message in the
/// returned `Vec<String>` and the loop continues to the next schema. Before
/// KYO-126's second pass, these failures were only `warn!`-logged and
/// silently dropped — `get_tables_in_container` returned `Ok(vec![])`, which
/// `index_catalog_sql` counts as a successful, empty result. A Databricks
/// catalog where the service principal has catalog-level `USE` but zero
/// table-level `SELECT` grants made every schema iteration warn-and-skip
/// while still reporting a clean `Ok(vec![])` — silent success reintroduced
/// one level below `discover_all_containers`. Returning the messages here
/// lets `index_catalog_sql` fold them into the same `errors` accumulator
/// every other indexer's `Err` path already feeds, so `resolve_final_status`
/// can tell "every schema is permission-denied" apart from "genuinely empty
/// catalog".
async fn discover_tables_in_catalog(
    provider: &dyn DatasourceProvider,
    container_name: &str,
    max_tables: Option<usize>,
) -> Result<(Vec<TableEntry>, Vec<String>)> {
    let schema_sql = format!("SHOW SCHEMAS IN `{container_name}`");
    let schema_result = provider
        .execute_query(&schema_sql, None, None, false, None)
        .await?;
    ensure_query_ok(
        &schema_result,
        &format!("listing schemas in catalog '{container_name}'"),
    )?;

    let schema_rows = extract_rows_from_batch(&schema_result);
    let mut tables = Vec::new();
    let mut partial_failures = Vec::new();

    for schema_row in &schema_rows {
        let Some(schema_name) = schema_row.first().and_then(|v| v.as_str()) else {
            continue;
        };

        if is_system_schema(schema_name) {
            continue;
        }

        let table_sql = format!("SHOW TABLES IN `{container_name}`.`{schema_name}`");
        let table_result = match provider
            .execute_query(&table_sql, None, None, false, None)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                partial_failures.push(schema_table_listing_failed(
                    container_name,
                    schema_name,
                    &e.to_string(),
                ));
                continue;
            }
        };

        if let Err(e) = ensure_query_ok(
            &table_result,
            &format!("listing tables in schema '{container_name}.{schema_name}'"),
        ) {
            partial_failures.push(schema_table_listing_failed(
                container_name,
                schema_name,
                &e.to_string(),
            ));
            continue;
        }

        let table_rows = extract_rows_from_batch(&table_result);

        for row in &table_rows {
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

            if let Some(max) = max_tables
                && tables.len() >= max
            {
                return Ok((tables, partial_failures));
            }
        }
    }

    Ok((tables, partial_failures))
}

/// Log a per-schema table-listing failure and build its message, in one
/// place shared by both the transport-`Err` and `ensure_query_ok`-`Err`
/// branches of [`discover_tables_in_catalog`] — they were previously two
/// near-identical `warn!` blocks.
///
/// The returned message uses the same `"Failed to list tables in {label}
/// '{container}': {error}"` shape as the top-level container-failure message
/// built in `index_catalog_sql`, so `resolve_final_status`'s multi-error
/// summary reads consistently regardless of which layer produced the error.
fn schema_table_listing_failed(catalog_name: &str, schema_name: &str, error: &str) -> String {
    warn!(
        catalog = catalog_name,
        schema = schema_name,
        error,
        "failed to list tables in schema, skipping"
    );
    format!("Failed to list tables in schema '{catalog_name}.{schema_name}': {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kyomi_datasource_server::QueryResult;

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

    // ── discover_tables_in_catalog (KYO-126, second pass) ─────────────────
    //
    // Databricks discovers tables via a `SHOW SCHEMAS` query followed by a
    // per-schema `SHOW TABLES` sub-loop. Before this fix, a `SHOW TABLES`
    // failure (either a transport `Err` or a provider-reported
    // `QueryStatus::Error`) was `warn!`-logged and skipped with no way for
    // the caller to learn it happened — `get_tables_in_container` returned
    // `Ok(vec![])`, indistinguishable from a schema that is genuinely empty.
    // These tests exercise the fix at the shared implementation both trait
    // methods delegate to.

    /// Per-schema behavior for `SHOW TABLES IN` in [`SchemaTableMockProvider`].
    enum TableListBehavior {
        /// The query succeeds and returns these table names.
        Tables(Vec<&'static str>),
        /// `execute_query` itself returns `Err` (transport-level failure).
        TransportError(String),
        /// `execute_query` returns `Ok(QueryResult { status: Error, .. })`
        /// (provider-reported query failure, e.g. permission denied).
        QueryError(String),
    }

    /// Responds to `SHOW SCHEMAS IN` with a fixed schema list, and to
    /// `SHOW TABLES IN` per the configured [`TableListBehavior`] for the
    /// schema named in the query.
    struct SchemaTableMockProvider {
        schemas: Vec<&'static str>,
        behaviors: std::collections::HashMap<&'static str, TableListBehavior>,
    }

    fn schema_names_batch(names: &[&str]) -> QueryResult {
        let schema = std::sync::Arc::new(arrow_schema::Schema::new(vec![arrow_schema::Field::new(
            "databaseName",
            arrow_schema::DataType::Utf8,
            false,
        )]));
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![std::sync::Arc::new(arrow_array::StringArray::from(
                names.to_vec(),
            ))],
        )
        .expect("valid record batch");
        let mut result = QueryResult::success_empty();
        result.record_batch = Some(batch);
        result
    }

    fn table_names_batch(schema_name: &str, tables: &[&str]) -> QueryResult {
        let schema = std::sync::Arc::new(arrow_schema::Schema::new(vec![
            arrow_schema::Field::new("database", arrow_schema::DataType::Utf8, false),
            arrow_schema::Field::new("tableName", arrow_schema::DataType::Utf8, false),
            arrow_schema::Field::new("isTemporary", arrow_schema::DataType::Utf8, false),
        ]));
        let n = tables.len();
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![
                std::sync::Arc::new(arrow_array::StringArray::from(vec![schema_name; n])),
                std::sync::Arc::new(arrow_array::StringArray::from(tables.to_vec())),
                std::sync::Arc::new(arrow_array::StringArray::from(vec!["false"; n])),
            ],
        )
        .expect("valid record batch");
        let mut result = QueryResult::success_empty();
        result.record_batch = Some(batch);
        result
    }

    #[async_trait]
    impl DatasourceProvider for SchemaTableMockProvider {
        async fn test_connection(&self) -> kyomi_connect_protocol::Result<bool> {
            Ok(true)
        }

        async fn execute_query(
            &self,
            sql: &str,
            _limit: Option<u32>,
            _offset: Option<u32>,
            _include_total: bool,
            _job_id: Option<&str>,
        ) -> kyomi_connect_protocol::Result<QueryResult> {
            if sql.starts_with("SHOW SCHEMAS") {
                return Ok(schema_names_batch(&self.schemas));
            }

            // `SHOW TABLES IN `{catalog}`.`{schema}`` — the schema name is
            // the second-to-last backtick-delimited segment.
            let schema_name = sql
                .rsplit('`')
                .nth(1)
                .expect("schema name is backtick-quoted in SHOW TABLES");

            match self.behaviors.get(schema_name) {
                Some(TableListBehavior::Tables(names)) => {
                    Ok(table_names_batch(schema_name, names))
                }
                Some(TableListBehavior::QueryError(msg)) => Ok(QueryResult::error(msg.clone())),
                Some(TableListBehavior::TransportError(msg)) => {
                    Err(kyomi_connect_protocol::Error::Internal(msg.clone()))
                }
                None => panic!("no behavior configured for schema '{schema_name}'"),
            }
        }

        async fn close(&self) {}
    }

    /// All schemas in the catalog fail to list — one via a transport error,
    /// one via a provider-reported query error. Both failure shapes must
    /// surface as messages, and zero tables must be discovered.
    #[tokio::test]
    async fn all_schemas_failing_returns_no_tables_and_partial_failure_messages() {
        let provider = SchemaTableMockProvider {
            schemas: vec!["sales", "marketing"],
            behaviors: std::collections::HashMap::from([
                (
                    "sales",
                    TableListBehavior::QueryError(
                        "permission denied for schema sales".to_string(),
                    ),
                ),
                (
                    "marketing",
                    TableListBehavior::TransportError("connection reset".to_string()),
                ),
            ]),
        };

        let (tables, partial_failures) = discover_tables_in_catalog(&provider, "main", None)
            .await
            .expect("SHOW SCHEMAS itself succeeds");

        assert!(
            tables.is_empty(),
            "no tables should be discovered when every schema fails"
        );
        assert_eq!(
            partial_failures.len(),
            2,
            "each failing schema must contribute exactly one message"
        );
        assert!(partial_failures
            .iter()
            .any(|m| m.contains("sales") && m.contains("permission denied for schema sales")));
        assert!(partial_failures
            .iter()
            .any(|m| m.contains("marketing") && m.contains("connection reset")));
    }

    /// One schema fails, the other succeeds — the failure must not abort
    /// the catalog crawl, and the successful schema's tables must still be
    /// returned alongside the one partial-failure message.
    #[tokio::test]
    async fn one_failing_schema_does_not_abort_the_others() {
        let provider = SchemaTableMockProvider {
            schemas: vec!["sales", "marketing"],
            behaviors: std::collections::HashMap::from([
                (
                    "sales",
                    TableListBehavior::Tables(vec!["orders", "customers"]),
                ),
                (
                    "marketing",
                    TableListBehavior::QueryError(
                        "permission denied for schema marketing".to_string(),
                    ),
                ),
            ]),
        };

        let (tables, partial_failures) = discover_tables_in_catalog(&provider, "main", None)
            .await
            .expect("SHOW SCHEMAS itself succeeds");

        assert_eq!(
            tables.len(),
            2,
            "the accessible schema's tables must still be discovered"
        );
        assert!(tables
            .iter()
            .all(|t| t.dataset_override.as_deref() == Some("main.sales")));
        assert_eq!(partial_failures.len(), 1);
        assert!(partial_failures[0].contains("marketing"));
    }
}
