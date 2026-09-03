// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource tools — list, query, and validate SQL against datasources.

use async_trait::async_trait;

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
        let datasources =
            kyomi_auth::datasource_service::list_datasources(&ctx.db, &ctx.workspace_id, false)
                .await?;

        // Count cached (non-archived) tables for every datasource in a
        // single grouped query via the canonical accessor — the same one
        // `browse_catalog`'s `total_tables` derives from (KYO-615), so the
        // two tools can never again report different numbers for the same
        // datasource. The sample datasource is treated like any other — it
        // gets its own per-workspace cache populated on creation by the
        // initial catalog index, so there's no special-case sentinel lookup.
        let ds_ids: Vec<String> = datasources.iter().map(|ds| ds.id.clone()).collect();
        let table_counts =
            match kyomi_auth::datasource_service::fetch_table_counts(&ctx.db, &ds_ids, None).await
            {
                Ok(counts) => counts,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "failed to fetch cached table counts for datasources; defaulting all to 0"
                    );
                    std::collections::HashMap::new()
                }
            };

        let mut results = Vec::new();
        for ds in &datasources {
            let table_count = table_counts.get(&ds.id).copied().unwrap_or(0);

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

        let result = provider.execute_query(sql, Some(20), None, false, None).await?;
        provider.close().await;

        match result.status {
            kyomi_datasource_server::provider::QueryStatus::Success => {
                let columns = result.columns.unwrap_or_default();
                let mut rows = result
                    .record_batch
                    .as_ref()
                    .map(super::query_utils::record_batch_to_rows)
                    .unwrap_or_default();
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
                for row in rows {
                    for (col, value) in columns.iter().zip(row) {
                        if let Some(arr) =
                            col_data.get_mut(&col.name).and_then(|v| v.as_array_mut())
                        {
                            arr.push(value);
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
                let raw_err = result.error.unwrap_or_else(|| "Unknown query error".to_string());
                tracing::warn!(raw_error = %raw_err, datasource = %ds.slug, "datasource query error (sanitized for agent)");
                Ok(serde_json::json!({
                    "error": kyomi_core::sanitize_error(&raw_err),
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

// ---------------------------------------------------------------------------
// Tests — KYO-615: `list_datasources.tables_indexed` must exclude
// soft-deleted (`is_archived`) rows and must batch every datasource's count
// into a single query rather than one `COUNT(*)` per datasource.
//
// Real in-memory SQLite pool with full migrations applied, exercising the
// actual tool `execute()` path end-to-end — mirrors the pattern in
// `tools::catalog::tests`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{build_ctx, seed_user_and_workspace, test_pool};

    /// Insert a plain datasource row in workspace "ws-1".
    async fn seed_datasource(db: &kyomi_core::DbPool, ds_id: &str, slug: &str) {
        let sq = match db {
            kyomi_core::DbPool::Sqlite(sq) => sq,
            _ => unreachable!("test pool is always sqlite"),
        };
        sqlx::query(
            "INSERT INTO datasource_configs \
             (id, workspace_id, name, datasource_type, connection_config, slug) \
             VALUES (?, 'ws-1', ?, 'postgres', '{}', ?)",
        )
        .bind(ds_id)
        .bind(format!("Datasource {slug}"))
        .bind(slug)
        .execute(sq)
        .await
        .expect("insert datasource");
    }

    /// Insert one table cache row for `ds_id`.
    async fn seed_table_row(db: &kyomi_core::DbPool, ds_id: &str, table_id: &str, is_archived: bool) {
        let sq = match db {
            kyomi_core::DbPool::Sqlite(sq) => sq,
            _ => unreachable!("test pool is always sqlite"),
        };
        sqlx::query(
            "INSERT INTO datasource_table_cache \
             (workspace_id, project_id, dataset_id, table_id, table_metadata, \
              datasource_config_id, is_archived) \
             VALUES ('ws-1', 'proj', 'public', ?, '{}', ?, ?)",
        )
        .bind(table_id)
        .bind(ds_id)
        .bind(is_archived)
        .execute(sq)
        .await
        .expect("insert table cache row");
    }

    /// Extract `tables_indexed` for `slug` out of a `list_datasources` JSON
    /// response.
    fn tables_indexed_for(parsed: &serde_json::Value, slug: &str) -> i64 {
        parsed["datasources"]
            .as_array()
            .expect("datasources array present")
            .iter()
            .find(|d| d["slug"] == slug)
            .unwrap_or_else(|| panic!("datasource '{slug}' present in response: {parsed}"))["tables_indexed"]
            .as_i64()
            .expect("tables_indexed is a number")
    }

    #[tokio::test]
    async fn tables_indexed_excludes_archived_rows() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        seed_datasource(&db, "ds-a", "a").await;
        seed_table_row(&db, "ds-a", "t1", false).await;
        seed_table_row(&db, "ds-a", "t2", false).await;
        seed_table_row(&db, "ds-a", "t3", true).await; // archived
        let ctx = build_ctx(db);

        let result = ListDatasourcesTool
            .execute(serde_json::json!({}), &ctx)
            .await
            .expect("list_datasources execute");
        let parsed: serde_json::Value =
            serde_json::from_str(&result).expect("list_datasources result is JSON");

        assert_eq!(
            tables_indexed_for(&parsed, "a"),
            2,
            "the archived row must not be counted: {result}"
        );
    }

    /// Structural proof of single-query batching (KYO-615): `execute()`
    /// above calls `datasource_service::fetch_table_counts` exactly once,
    /// with every datasource id, outside any per-datasource loop — the
    /// per-datasource `COUNT(*)` loop this ticket replaces no longer exists
    /// in the source at all. This crate has no query-count instrumentation
    /// to assert "exactly one SQL statement executed" at runtime, so this
    /// test instead proves *correctness* of that single batched call across
    /// a zero-table datasource, a one-table datasource, and a two-table
    /// datasource together in one response — which an N+1 implementation
    /// would also get right, but which corroborates the single call reads
    /// every id's count correctly rather than, say, only the first.
    #[tokio::test]
    async fn tables_indexed_is_correct_for_every_datasource_in_one_response() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        seed_datasource(&db, "ds-a", "a").await;
        seed_datasource(&db, "ds-b", "b").await;
        seed_datasource(&db, "ds-c", "c").await; // zero cached tables
        seed_table_row(&db, "ds-a", "t1", false).await;
        seed_table_row(&db, "ds-b", "t1", false).await;
        seed_table_row(&db, "ds-b", "t2", false).await;
        let ctx = build_ctx(db);

        let result = ListDatasourcesTool
            .execute(serde_json::json!({}), &ctx)
            .await
            .expect("list_datasources execute");
        let parsed: serde_json::Value =
            serde_json::from_str(&result).expect("list_datasources result is JSON");

        assert_eq!(tables_indexed_for(&parsed, "a"), 1);
        assert_eq!(tables_indexed_for(&parsed, "b"), 2);
        assert_eq!(tables_indexed_for(&parsed, "c"), 0, "a datasource with no cached tables must read 0, not be omitted");
    }
}
