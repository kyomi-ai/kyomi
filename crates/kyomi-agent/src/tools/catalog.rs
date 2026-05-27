// SPDX-License-Identifier: AGPL-3.0-or-later

//! Catalog tools — table info retrieval by exact name.

use async_trait::async_trait;

use kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID;
use kyomi_core::enums::DatasourceType;

use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read a boolean from a `connection_config` JSON value that may be stored as
/// a JSON bool (`true`) or a JSON string (`"true"`).  The UI and some
/// migration paths write the string form, so `.as_bool()` alone always returns
/// `None` for those entries.
fn config_bool(val: Option<&serde_json::Value>, default: bool) -> bool {
    match val {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => default,
    }
}

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
        // workspace when the datasource is BigQuery with include_public_datasets enabled.
        let table_metadata = if let Some(metadata) = table_metadata {
            metadata
        } else if ds.datasource_type == DatasourceType::Bigquery
            && config_bool(ds.connection_config.get("include_public_datasets"), true)
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
const BROWSE_CATALOG_LIMIT: usize = 500;

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
        "Browse all tables in a datasource, grouped by schema/dataset. Returns the \
         same hierarchical view the user sees in the catalog UI. Use this to explore \
         what tables exist before querying."
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
                    "description": "Include column names and types (default: false). Use get_table_info for detailed column info on specific tables.",
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

        let columns_select = if include_columns {
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

        // BigQuery public datasets: include if enabled (defaults to true).
        if ds.datasource_type == DatasourceType::Bigquery
            && config_bool(ds.connection_config.get("include_public_datasets"), true)
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

            let description = row.description.as_deref().unwrap_or("");

            let mut table_obj = serde_json::json!({
                "name": full_name,
                "description": description,
            });
            if let Some(rc) = row.row_count {
                table_obj["row_count"] = serde_json::json!(rc);
            }
            if include_columns
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
                "Showing first 500 tables. Use the 'schema' parameter to filter to a specific dataset."
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
