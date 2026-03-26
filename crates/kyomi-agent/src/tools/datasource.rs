// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource tools — list, query, and validate SQL against datasources.

use async_trait::async_trait;
use kyomi_datasource_server::DatasourceProvider;

use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

// ---------------------------------------------------------------------------
// ListDatasourcesTool
// ---------------------------------------------------------------------------

/// List all available datasources in the current workspace.
pub struct ListDatasourcesTool;

#[async_trait]
impl AgentTool for ListDatasourcesTool {
    fn name(&self) -> &str {
        "list_datasources"
    }

    fn description(&self) -> &str {
        "List all available datasources in the current workspace. Returns each \
         datasource's slug, name, type, and number of indexed tables."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
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
        _args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        if ctx.is_trial {
            return Ok(serde_json::json!({
                "datasources": [{
                    "slug": "acme-analytics",
                    "name": "Acme Analytics (Sample)",
                    "type": "clickhouse",
                    "tables_indexed": 4
                }]
            })
            .to_string());
        }

        let datasources =
            kyomi_auth::datasource_service::list_datasources(&ctx.db, &ctx.workspace_id, false)
                .await?;

        let mut results = Vec::new();
        for ds in &datasources {
            let is_sample = ds
                .connection_config
                .get("is_sample")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let table_count: i64 = if is_sample {
                let sample_ws_id =
                    kyomi_auth::catalog::indexers::sample_data::SAMPLE_DATA_WORKSPACE_ID;
                kyomi_core::db_fetch_scalar!(
                    ctx.db, i64,
                    "SELECT COUNT(*) FROM datasource_table_cache WHERE workspace_id = $1",
                    sample_ws_id
                )
                .unwrap_or(0)
            } else {
                kyomi_core::db_fetch_scalar!(
                    ctx.db, i64,
                    "SELECT COUNT(*) FROM datasource_table_cache WHERE datasource_config_id = $1",
                    &ds.id
                )
                .unwrap_or(0)
            };

            results.push(serde_json::json!({
                "slug": ds.slug,
                "name": ds.name,
                "type": ds.datasource_type,
                "tables_indexed": table_count,
            }));
        }

        Ok(serde_json::json!({ "datasources": results }).to_string())
    }
}

// ---------------------------------------------------------------------------
// QueryDatasourceTool
// ---------------------------------------------------------------------------

/// Execute a SQL query against a datasource (max 20 rows).
pub struct QueryDatasourceTool;

#[async_trait]
impl AgentTool for QueryDatasourceTool {
    fn name(&self) -> &str {
        "query_datasource"
    }

    fn description(&self) -> &str {
        "Execute a SQL query against any datasource and return results. \
         HARD LIMIT: 20 rows maximum. Use fully qualified table names."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "sql_query": {
                    "type": "string",
                    "description": "SQL query to execute"
                },
                "datasource": {
                    "type": "string",
                    "description": "Datasource slug (e.g., 'production-postgres'). Required for non-BigQuery."
                }
            },
            "required": ["sql_query"]
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
        let sql = args
            .get("sql_query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'sql_query'".into())
            })?;
        let datasource_slug = args.get("datasource").and_then(|v| v.as_str());

        if ctx.is_trial {
            // Validate datasource slug — only "acme-analytics" is allowed in trial mode
            if datasource_slug.is_some() && datasource_slug != Some("acme-analytics") {
                return Ok(serde_json::json!({
                    "error": "Trial mode only supports the 'acme-analytics' datasource"
                })
                .to_string());
            }

            if !sql.trim().to_ascii_uppercase().starts_with("SELECT") {
                return Ok(serde_json::json!({
                    "error": "Trial mode only supports SELECT queries"
                })
                .to_string());
            }

            // Load sample ClickHouse config and execute real query
            let ch_config = kyomi_auth::catalog::indexers::sample_data::SampleClickHouseConfig::from_env()
                .ok_or_else(|| {
                    kyomi_core::Error::Internal(
                        "Sample ClickHouse not configured (SAMPLE_CLICKHOUSE_HOST)".into(),
                    )
                })?;

            let connection_config = ch_config.connection_config_json();
            let credentials = ch_config.credentials_json();

            let provider =
                kyomi_datasource_server::providers::clickhouse::ClickHouseProvider::new(
                    &connection_config,
                    &credentials,
                )
                .await?;

            let result = provider.execute_query(sql, Some(20), None, false).await;
            provider.close().await;
            let result = result?;

            return match result.status {
                kyomi_datasource_server::provider::QueryStatus::Success => {
                    let columns = result.columns.unwrap_or_default();
                    let mut rows = result.rows.unwrap_or_default();
                    rows.truncate(20);
                    let row_count = rows.len();

                    // Build columnar format: { col_name: [values] }
                    let mut col_data: serde_json::Map<String, serde_json::Value> =
                        serde_json::Map::new();
                    let mut col_info = Vec::new();
                    for col in &columns {
                        col_info.push(serde_json::json!({
                            "name": col.name,
                            "type": col.col_type.as_str(),
                        }));
                        col_data.insert(
                            col.name.clone(),
                            serde_json::Value::Array(Vec::new()),
                        );
                    }
                    for row in &rows {
                        for (i, col) in columns.iter().enumerate() {
                            if let Some(arr) =
                                col_data.get_mut(&col.name).and_then(|v| v.as_array_mut())
                            {
                                let val: serde_json::Value =
                                    row.get(i).cloned().unwrap_or(serde_json::Value::Null);
                                arr.push(val);
                            }
                        }
                    }

                    Ok(serde_json::json!({
                        "cols": col_info,
                        "data": col_data,
                        "rows": row_count,
                        "truncated": row_count >= 20,
                        "datasource": "acme-analytics",
                        "type": "clickhouse",
                    })
                    .to_string())
                }
                kyomi_datasource_server::provider::QueryStatus::Error => {
                    Ok(serde_json::json!({
                        "error": result.error.unwrap_or_else(|| "Unknown query error".to_string()),
                        "datasource": "acme-analytics",
                    })
                    .to_string())
                }
            };
        }

        let ds_slug = datasource_slug.ok_or_else(|| {
            kyomi_core::Error::BadRequest("Parameter 'datasource' is required".into())
        })?;
        let ds = kyomi_auth::datasource_service::resolve_datasource(
            &ctx.db,
            ds_slug,
            &ctx.workspace_id,
            false,
        )
        .await?;

        // Create provider (handles both direct and Connect datasources)
        let query_ctx = ctx.query_context();
        let provider =
            super::query_utils::create_provider_for_datasource(&query_ctx, &ds)
                .await
                .map_err(kyomi_core::Error::Internal)?;

        let result = provider.execute_query(sql, Some(20), None, false).await?;
        provider.close().await;

        match result.status {
            kyomi_datasource_server::provider::QueryStatus::Success => {
                let columns = result.columns.unwrap_or_default();
                let mut rows = result.rows.unwrap_or_default();
                // Defensive truncation: cap at 20 rows even if provider
                // returns more (shouldn't happen, but belt-and-suspenders).
                rows.truncate(20);
                let row_count = rows.len();

                // Build columnar format: { col_name: [values] }
                let mut col_data: serde_json::Map<String, serde_json::Value> =
                    serde_json::Map::new();
                let mut col_info = Vec::new();
                for col in &columns {
                    col_info.push(serde_json::json!({
                        "name": col.name,
                        "type": col.col_type.as_str(),
                    }));
                    col_data.insert(
                        col.name.clone(),
                        serde_json::Value::Array(Vec::new()),
                    );
                }
                for row in &rows {
                    for (i, col) in columns.iter().enumerate() {
                        if let Some(arr) =
                            col_data.get_mut(&col.name).and_then(|v| v.as_array_mut())
                        {
                            arr.push(
                                row.get(i).cloned().unwrap_or(serde_json::Value::Null),
                            );
                        }
                    }
                }

                Ok(serde_json::json!({
                    "cols": col_info,
                    "data": col_data,
                    "rows": row_count,
                    "truncated": row_count >= 20,
                    "datasource": ds.slug,
                    "type": ds.datasource_type,
                })
                .to_string())
            }
            kyomi_datasource_server::provider::QueryStatus::Error => {
                Ok(serde_json::json!({
                    "error": result.error.unwrap_or_else(|| "Unknown query error".to_string()),
                    "datasource": ds.slug,
                })
                .to_string())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ValidateSqlTool
// ---------------------------------------------------------------------------

/// Validate SQL query syntax without executing.
pub struct ValidateSqlTool;

#[async_trait]
impl AgentTool for ValidateSqlTool {
    fn name(&self) -> &str {
        "validate_sql"
    }

    fn description(&self) -> &str {
        "Validate SQL query syntax without executing. Returns validation status, \
         error message with line/column if invalid."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "sql": {
                    "type": "string",
                    "description": "SQL query to validate"
                },
                "datasource": {
                    "type": "string",
                    "description": "Datasource slug"
                }
            },
            "required": ["sql", "datasource"]
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
        let sql = args
            .get("sql")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'sql'".into())
            })?;
        let datasource_slug = args
            .get("datasource")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'datasource'".into())
            })?;

        if ctx.is_trial {
            return Ok(serde_json::json!({
                "success": true,
                "message": "SQL syntax appears valid (trial mode)"
            })
            .to_string());
        }

        let ds = kyomi_auth::datasource_service::resolve_datasource(
            &ctx.db,
            datasource_slug,
            &ctx.workspace_id,
            false,
        )
        .await?;

        // Create provider (handles both direct and Connect datasources)
        let query_ctx = ctx.query_context();
        let provider =
            super::query_utils::create_provider_for_datasource(&query_ctx, &ds)
                .await
                .map_err(kyomi_core::Error::Internal)?;

        let result = provider.dry_run(sql).await?;
        provider.close().await;

        let mut response = serde_json::json!({
            "success": result.valid,
            "message": result.message,
        });

        if !result.valid {
            response["error_message"] = serde_json::Value::String(result.message.clone());
            if let Some(line) = result.line {
                response["line"] = serde_json::json!(line);
            }
            if let Some(col) = result.column {
                response["column"] = serde_json::json!(col);
            }
        }

        Ok(response.to_string())
    }
}
