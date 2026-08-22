// SPDX-License-Identifier: AGPL-3.0-or-later

//! Catalog tools — table info retrieval by exact name.

use async_trait::async_trait;

use kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID;
use kyomi_core::enums::DatasourceType;
use kyomi_core::json_utils::bigquery_include_public;

use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct TableMetadataRow {
    table_metadata: serde_json::Value,
}

// ---------------------------------------------------------------------------
// GetTableInfoTool
// ---------------------------------------------------------------------------

/// Get detailed information about a table from any datasource.
pub struct GetTableInfoTool;

#[async_trait]
impl AgentTool for GetTableInfoTool {
    fn name(&self) -> &str {
        "get_table_info"
    }

    fn description(&self) -> &str {
        "Get detailed information about a table from any datasource. Returns \
         schema, column names, types, descriptions, and metadata."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "table_name": {
                    "type": "string",
                    "description": "Fully qualified table name"
                },
                "datasource": {
                    "type": "string",
                    "description": "Datasource slug"
                }
            },
            "required": ["table_name", "datasource"]
        })
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let table_name = args
            .get("table_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'table_name'".into(),
                )
            })?;
        let datasource_slug = args
            .get("datasource")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'datasource'".into(),
                )
            })?;

        // Resolve datasource and check if it's a sample datasource
        let ds = kyomi_auth::datasource_service::resolve_datasource(
            &ctx.db,
            datasource_slug,
            &ctx.workspace_id,
            false,
        )
        .await?;

        // Look up cached table info for this datasource.
        //
        // The table_name parameter is a fully qualified name like
        // "acme_analytics.orders" or "project.dataset.table".
        // We reconstruct the full name from stored project_id/dataset_id/table_id.
        //
        // Sample datasources used to require a sentinel-workspace branch
        // here; they now index into the user's workspace via the generic
        // per-workspace indexer, so `datasource_config_id` works for every
        // datasource type.
        let is_pg = ctx.db.is_postgres();
        let bool_false = kyomi_core::sql_compat::bool_false(is_pg);
        let full_name_expr = kyomi_core::sql_compat::full_table_name_expr_prefixed(is_pg, "dtc");
        let ds_sql = format!(
            "SELECT dtc.table_metadata \
             FROM datasource_table_cache dtc \
             WHERE dtc.datasource_config_id = $1 \
               AND dtc.is_archived = {bool_false} \
               AND {full_name_expr} = $2"
        );
        let cached: Option<TableMetadataRow> = kyomi_core::db_fetch_optional!(
            ctx.db, TableMetadataRow,
            &ds_sql,
            &ds.id,
            table_name
        )?;
        let table_metadata = cached.map(|row| row.table_metadata);

        // If not found in the user's datasource, try the BigQuery public dataset
        // workspace when the datasource is BigQuery with include_public_datasets
        // enabled (absent key defaults to disabled — see
        // kyomi_core::json_utils::bigquery_include_public).
        let table_metadata = if let Some(metadata) = table_metadata {
            metadata
        } else if ds.datasource_type == DatasourceType::Bigquery
            && bigquery_include_public(&ds.connection_config)
        {
            let public_sql = format!(
                "SELECT dtc.table_metadata \
                 FROM datasource_table_cache dtc \
                 WHERE dtc.workspace_id = $1 \
                   AND dtc.is_archived = {bool_false} \
                   AND {full_name_expr} = $2"
            );
            let public_cached: Option<TableMetadataRow> = kyomi_core::db_fetch_optional!(
                ctx.db, TableMetadataRow,
                &public_sql,
                PUBLIC_DATA_WORKSPACE_ID,
                table_name
            )?;
            match public_cached {
                Some(row) => row.table_metadata,
                None => {
                    return Ok(serde_json::json!({
                        "error": format!(
                            "Table '{}' not found in catalog cache for datasource '{}'. \
                             Try running a catalog refresh.",
                            table_name, datasource_slug
                        )
                    })
                    .to_string());
                }
            }
        } else {
            // Table not in cache — return informative error
            return Ok(serde_json::json!({
                "error": format!(
                    "Table '{}' not found in catalog cache for datasource '{}'. \
                     Try running a catalog refresh.",
                    table_name, datasource_slug
                )
            })
            .to_string());
        };

        Ok(format_table_info_response(
            table_name,
            datasource_slug,
            &table_metadata,
        ))
    }
}

// ---------------------------------------------------------------------------
// BrowseCatalogTool
// ---------------------------------------------------------------------------

/// Maximum number of tables returned by `browse_catalog` in a single call.
/// Queries fetch one extra row (LIMIT + 1) to detect truncation.
const BROWSE_CATALOG_LIMIT: usize = 100;

#[derive(sqlx::FromRow)]
struct CatalogBrowseLiteRow {
    project_id: String,
    dataset_id: String,
    table_id: String,
    description: Option<String>,
    row_count: Option<i64>,
    columns_json: Option<String>,
}

/// Browse all tables in a datasource, grouped by schema/dataset.
pub struct BrowseCatalogTool;

#[async_trait]
impl AgentTool for BrowseCatalogTool {
    fn name(&self) -> &str {
        "browse_catalog"
    }

    fn description(&self) -> &str {
        "Browse tables in a datasource, grouped by schema/dataset. Without a schema \
         filter, returns a compact listing of table names. With a schema filter, returns \
         descriptions and row counts. Use get_table_info for detailed column info on \
         specific tables."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "datasource": {
                    "type": "string",
                    "description": "Datasource slug (from list_datasources)"
                },
                "schema": {
                    "type": "string",
                    "description": "Optional: filter to a specific schema/dataset"
                },
                "include_columns": {
                    "type": "boolean",
                    "description": "Include column names and types (requires 'schema' parameter to be set). Use get_table_info for detailed column info on specific tables.",
                    "default": false
                }
            },
            "required": ["datasource"]
        })
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let datasource_slug = args
            .get("datasource")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'datasource'".into())
            })?;
        let schema_filter = args.get("schema").and_then(|v| v.as_str());
        let include_columns = args
            .get("include_columns")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let is_pg = ctx.db.is_postgres();
        let bool_false = kyomi_core::sql_compat::bool_false(is_pg);

        let ds = kyomi_auth::datasource_service::resolve_datasource(
            &ctx.db,
            datasource_slug,
            &ctx.workspace_id,
            false,
        )
        .await?;

        // Build projected SQL expressions — only fetch the fields we need to
        // avoid deserializing the full table_metadata JSON blob (~57 MB for
        // BigQuery with public datasets).
        let desc_expr = format!(
            "COALESCE({}, {})",
            kyomi_core::sql_compat::json_extract_text(is_pg, "table_metadata", "table_description"),
            kyomi_core::sql_compat::json_extract_text(is_pg, "table_metadata", "description"),
        );

        let row_count_text =
            kyomi_core::sql_compat::json_extract_text(is_pg, "table_metadata", "row_count");
        let row_count_expr = if is_pg {
            format!("({row_count_text})::bigint")
        } else {
            format!("CAST({row_count_text} AS INTEGER)")
        };

        let columns_select = if schema_filter.is_some() && include_columns {
            format!(
                ", {} as columns_json",
                kyomi_core::sql_compat::json_extract_text(is_pg, "table_metadata", "columns")
            )
        } else if is_pg {
            ", NULL::text as columns_json".to_string()
        } else {
            ", NULL as columns_json".to_string()
        };

        let limit = BROWSE_CATALOG_LIMIT + 1;

        // Query by `datasource_config_id` uniformly — sample datasources now
        // index into the user's workspace like any other datasource.
        // Schema filter is applied in SQL to avoid fetching rows we'll discard.
        let sql = format!(
            "SELECT project_id, dataset_id, table_id, \
                    {desc_expr} as description, \
                    {row_count_expr} as row_count \
                    {columns_select} \
             FROM datasource_table_cache \
             WHERE datasource_config_id = $1 AND is_archived = {bool_false} \
               AND ($2 IS NULL OR dataset_id = $2) \
             ORDER BY dataset_id, table_id \
             LIMIT {limit}"
        );
        let mut rows: Vec<CatalogBrowseLiteRow> =
            kyomi_core::db_fetch_all!(ctx.db, CatalogBrowseLiteRow, &sql, &ds.id, &schema_filter)
                .unwrap_or_default();

        // BigQuery public datasets: include if enabled (absent key defaults
        // to disabled — see kyomi_core::json_utils::bigquery_include_public).
        if ds.datasource_type == DatasourceType::Bigquery
            && bigquery_include_public(&ds.connection_config)
        {
            let public_sql = format!(
                "SELECT project_id, dataset_id, table_id, \
                        {desc_expr} as description, \
                        {row_count_expr} as row_count \
                        {columns_select} \
                 FROM datasource_table_cache \
                 WHERE workspace_id = $1 AND is_archived = {bool_false} \
                   AND ($2 IS NULL OR dataset_id = $2) \
                 ORDER BY dataset_id, table_id \
                 LIMIT {limit}"
            );
            let public_rows: Vec<CatalogBrowseLiteRow> = kyomi_core::db_fetch_all!(
                ctx.db,
                CatalogBrowseLiteRow,
                &public_sql,
                PUBLIC_DATA_WORKSPACE_ID,
                &schema_filter
            )
            .unwrap_or_default();
            rows.extend(public_rows);
        }

        // Detect truncation: we fetched up to LIMIT+1 rows per query.
        let truncated = rows.len() > BROWSE_CATALOG_LIMIT;
        if truncated {
            rows.truncate(BROWSE_CATALOG_LIMIT);
        }

        let mut schemas: std::collections::BTreeMap<String, Vec<serde_json::Value>> =
            std::collections::BTreeMap::new();
        for row in &rows {
            let full_name = if row.project_id.is_empty() {
                if row.dataset_id.is_empty() {
                    row.table_id.clone()
                } else {
                    format!("{}.{}", row.dataset_id, row.table_id)
                }
            } else {
                format!("{}.{}.{}", row.project_id, row.dataset_id, row.table_id)
            };

            let mut table_obj = if schema_filter.is_some() {
                // Schema filter applied — include description for targeted browsing
                let description = row.description.as_deref().unwrap_or("");
                serde_json::json!({
                    "name": full_name,
                    "description": description,
                })
            } else {
                // No schema filter — compact listing, just names
                serde_json::json!({
                    "name": full_name,
                })
            };
            if let Some(rc) = row.row_count {
                table_obj["row_count"] = serde_json::json!(rc);
            }
            // Only include columns when a schema filter narrows the scope
            if schema_filter.is_some()
                && include_columns
                && let Some(ref json_str) = row.columns_json
                && let Ok(cols_value) = serde_json::from_str::<serde_json::Value>(json_str)
            {
                let cols: Vec<serde_json::Value> = cols_value
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .map(|col| {
                                serde_json::json!({
                                    "name": col.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                                    "type": col.get("type").and_then(|v| v.as_str()).unwrap_or(""),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                table_obj["columns"] = serde_json::json!(cols);
            }

            let schema_key = if row.dataset_id.is_empty() {
                "(default)".to_string()
            } else {
                row.dataset_id.clone()
            };
            schemas.entry(schema_key).or_default().push(table_obj);
        }

        let schema_list: Vec<serde_json::Value> = schemas
            .into_iter()
            .map(|(name, tables)| {
                serde_json::json!({
                    "name": name,
                    "table_count": tables.len(),
                    "tables": tables,
                })
            })
            .collect();
        let total_tables: usize = schema_list
            .iter()
            .map(|s| s["table_count"].as_u64().unwrap_or(0) as usize)
            .sum();

        let mut response = serde_json::json!({
            "datasource": datasource_slug,
            "type": ds.datasource_type,
            "total_tables": total_tables,
            "schemas": schema_list,
        });
        if truncated {
            response["truncated"] = serde_json::json!(true);
            response["note"] = serde_json::json!(
                "Results truncated to 100 tables. Use the 'schema' parameter to filter \
                 to a specific dataset/schema for complete results with descriptions."
            );
        }

        Ok(response.to_string())
    }
}

/// Transform cached `table_metadata` into the flat format the frontend expects:
/// `{ table, desc, rows, cols: [{name, type, desc}] }`
///
/// This matches the Python provider's `get_table_info()` return format.
fn format_table_info_response(
    table_name: &str,
    datasource_slug: &str,
    table_metadata: &serde_json::Value,
) -> String {
    let description = table_metadata
        .get("table_description")
        .or_else(|| table_metadata.get("description"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let row_count = table_metadata.get("row_count").and_then(|v| v.as_u64());

    // Convert columns from indexer format {name, type, description}
    // to flat format {name, type, desc}
    let cols: Vec<serde_json::Value> = table_metadata
        .get("columns")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|col| {
                    serde_json::json!({
                        "name": col.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        "type": col.get("type").and_then(|v| v.as_str()).unwrap_or(""),
                        "desc": col.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let mut result = serde_json::json!({
        "table": table_name,
        "datasource": datasource_slug,
        "desc": description,
        "cols": cols,
    });

    if let Some(rows) = row_count {
        result["rows"] = serde_json::json!(rows);
    }

    result.to_string()
}

// ---------------------------------------------------------------------------
// Tests — KYO-446: BrowseCatalogTool / GetTableInfoTool must honor an
// absent, `false`, and `true` `include_public_datasets` key identically
// (absent and `false` both exclude public rows; only `true` includes them).
//
// Real in-memory SQLite pool with full migrations applied, exercising the
// actual tool `execute()` path end-to-end — mirrors the pattern in
// `tools::watch::tests::broadcast_routing`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_pool() -> kyomi_core::DbPool {
        let _ = kyomi_core::constants::load_with_fallback();

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .expect("enable foreign keys");

        sqlx::migrate!("../../apps/server/migrations-sqlite")
            .run(&pool)
            .await
            .expect("run sqlite migrations");

        kyomi_core::DbPool::Sqlite(pool)
    }

    fn build_ctx(db: kyomi_core::DbPool) -> ToolContext {
        ToolContext {
            ws_manager: kyomi_auth::websocket::WebSocketManager::new(None, db.clone()),
            db,
            kv: kyomi_core::kv_store_memory::InMemoryKVStore::new_pool(),
            user_id: "user-a".to_string(),
            workspace_id: "ws-1".to_string(),
            encryption_key: std::sync::Arc::new([0u8; 32]),
            embedding: kyomi_embed::LazyEmbedding::new(),
            config: std::sync::Arc::new(kyomi_core::Config::test_config()),
            session_id: None,
            supports_mcp_apps: false,
            workspace_roles: Vec::new(),
            connect_registry: None,
            platforms: std::sync::Arc::new(kyomi_core::platform::PlatformRegistry::new()),
            user_display_name: "User A".to_string(),
        }
    }

    /// Seed workspace "ws-1" with one active BigQuery datasource (slug
    /// "bq") whose `connection_config` is exactly `connection_config_json`,
    /// plus:
    ///  - one cache row under that datasource's own `datasource_config_id`
    ///    ("acme.sales.orders") — must be visible regardless of the toggle.
    ///  - one cache row under the shared public-data sentinel workspace
    ///    ("bigquery-public-data.hacker_news.full") — must be visible only
    ///    when `include_public_datasets` resolves to `true`.
    async fn seed_bigquery_datasource(db: &kyomi_core::DbPool, connection_config_json: &str) {
        let sq = match db {
            kyomi_core::DbPool::Sqlite(sq) => sq,
            _ => unreachable!("test pool is always sqlite"),
        };

        sqlx::query("INSERT INTO users (user_id, email) VALUES ('user-a', 'a@test.local')")
            .execute(sq)
            .await
            .expect("insert user-a");
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) \
             VALUES ('ws-1', 'Workspace', 'user-a')",
        )
        .execute(sq)
        .await
        .expect("insert workspace");

        sqlx::query(
            "INSERT INTO datasource_configs \
             (id, workspace_id, name, datasource_type, connection_config, active, slug) \
             VALUES ('ds-1', 'ws-1', 'BQ', 'bigquery', ?, 1, 'bq')",
        )
        .bind(connection_config_json)
        .execute(sq)
        .await
        .expect("insert datasource");

        sqlx::query(
            "INSERT INTO datasource_table_cache \
             (workspace_id, project_id, dataset_id, table_id, table_metadata, datasource_config_id) \
             VALUES ('ws-1', 'acme', 'sales', 'orders', '{}', 'ds-1')",
        )
        .execute(sq)
        .await
        .expect("insert own table cache row");

        sqlx::query(
            "INSERT INTO datasource_table_cache \
             (workspace_id, project_id, dataset_id, table_id, table_metadata) \
             VALUES (?, 'bigquery-public-data', 'hacker_news', 'full', '{}')",
        )
        .bind(PUBLIC_DATA_WORKSPACE_ID)
        .execute(sq)
        .await
        .expect("insert public table cache row");
    }

    /// Full table names (`project.dataset.table`) present in a
    /// `browse_catalog` response for the "bq" datasource.
    async fn browse_tables(ctx: &ToolContext) -> Vec<String> {
        let result = BrowseCatalogTool
            .execute(serde_json::json!({"datasource": "bq"}), ctx)
            .await
            .expect("browse_catalog execute");
        let parsed: serde_json::Value =
            serde_json::from_str(&result).expect("browse_catalog result is JSON");
        parsed["schemas"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|s| s["tables"].as_array().cloned().unwrap_or_default())
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect()
    }

    #[tokio::test]
    async fn browse_catalog_excludes_public_datasets_when_key_absent() {
        let db = test_pool().await;
        seed_bigquery_datasource(&db, "{}").await;
        let ctx = build_ctx(db);

        let tables = browse_tables(&ctx).await;

        assert_eq!(
            tables,
            vec!["acme.sales.orders".to_string()],
            "an absent include_public_datasets key must default to disabled, not enabled"
        );
    }

    #[tokio::test]
    async fn browse_catalog_excludes_public_datasets_when_false() {
        let db = test_pool().await;
        seed_bigquery_datasource(&db, r#"{"include_public_datasets": false}"#).await;
        let ctx = build_ctx(db);

        let tables = browse_tables(&ctx).await;

        assert_eq!(tables, vec!["acme.sales.orders".to_string()]);
    }

    #[tokio::test]
    async fn browse_catalog_includes_public_datasets_when_true() {
        let db = test_pool().await;
        seed_bigquery_datasource(&db, r#"{"include_public_datasets": true}"#).await;
        let ctx = build_ctx(db);

        let mut tables = browse_tables(&ctx).await;
        tables.sort();

        assert_eq!(
            tables,
            vec![
                "acme.sales.orders".to_string(),
                "bigquery-public-data.hacker_news.full".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn get_table_info_cannot_find_public_table_when_key_absent() {
        let db = test_pool().await;
        seed_bigquery_datasource(&db, "{}").await;
        let ctx = build_ctx(db);

        let result = GetTableInfoTool
            .execute(
                serde_json::json!({
                    "table_name": "bigquery-public-data.hacker_news.full",
                    "datasource": "bq",
                }),
                &ctx,
            )
            .await
            .expect("get_table_info execute");
        let parsed: serde_json::Value =
            serde_json::from_str(&result).expect("get_table_info result is JSON");

        assert!(
            parsed.get("error").is_some(),
            "a public table must not be found when include_public_datasets is absent: {result}"
        );
    }

    #[tokio::test]
    async fn get_table_info_cannot_find_public_table_when_false() {
        let db = test_pool().await;
        seed_bigquery_datasource(&db, r#"{"include_public_datasets": false}"#).await;
        let ctx = build_ctx(db);

        let result = GetTableInfoTool
            .execute(
                serde_json::json!({
                    "table_name": "bigquery-public-data.hacker_news.full",
                    "datasource": "bq",
                }),
                &ctx,
            )
            .await
            .expect("get_table_info execute");
        let parsed: serde_json::Value =
            serde_json::from_str(&result).expect("get_table_info result is JSON");

        assert!(
            parsed.get("error").is_some(),
            "a public table must not be found when include_public_datasets is false: {result}"
        );
    }

    #[tokio::test]
    async fn get_table_info_finds_public_table_when_true() {
        let db = test_pool().await;
        seed_bigquery_datasource(&db, r#"{"include_public_datasets": true}"#).await;
        let ctx = build_ctx(db);

        let result = GetTableInfoTool
            .execute(
                serde_json::json!({
                    "table_name": "bigquery-public-data.hacker_news.full",
                    "datasource": "bq",
                }),
                &ctx,
            )
            .await
            .expect("get_table_info execute");
        let parsed: serde_json::Value =
            serde_json::from_str(&result).expect("get_table_info result is JSON");

        assert!(
            parsed.get("error").is_none(),
            "a public table must be found when include_public_datasets is true: {result}"
        );
        assert_eq!(parsed["table"], serde_json::json!("bigquery-public-data.hacker_news.full"));
    }
}
