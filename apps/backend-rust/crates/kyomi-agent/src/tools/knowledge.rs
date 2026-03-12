// SPDX-License-Identifier: AGPL-3.0-or-later

//! Knowledge search tool — unified search across the workspace knowledge base.
//!
//! Uses pgvector-based semantic search to find tables, learnings, and metrics
//! in a single call.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;

use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct TableSlugRow {
    full_name: Option<String>,
    slug: String,
}

#[derive(sqlx::FromRow)]
struct TableFullNameRow {
    full_name: Option<String>,
}

// ---------------------------------------------------------------------------
// SearchKnowledgeTool
// ---------------------------------------------------------------------------

/// Unified search across the workspace knowledge base.
///
/// Searches tables, learnings, and metrics using pgvector-based semantic
/// search in PostgreSQL.
pub struct SearchKnowledgeTool;

#[async_trait]
impl AgentTool for SearchKnowledgeTool {
    fn name(&self) -> &str {
        "search_knowledge"
    }

    fn description(&self) -> &str {
        "Search the workspace's knowledge base for relevant tables, learnings, \
         and metrics using semantic search. Use this to discover what data is \
         available before querying."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language search query"
                },
                "datasource": {
                    "type": "string",
                    "description": "Optional datasource slug to filter"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results (default: 10)",
                    "default": 10
                }
            },
            "required": ["query"]
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
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'query'".into())
            })?;
        let datasource_slug = args.get("datasource").and_then(|v| v.as_str());
        let limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(10) as usize;

        let embed = ctx.embedding.wait_ready().await?;

        // Empty already_injected = return all matches.
        // Large token budget = don't artificially limit tool results.
        let vsearch = kyomi_knowledge::create_vector_search(&ctx.db);
        let mut result = kyomi_knowledge::retrieval::retrieve(
            vsearch.as_ref(),
            embed,
            &ctx.workspace_id,
            query,
            &HashSet::new(),
            Some(4096),
        )
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Knowledge retrieval failed: {e}")))?;

        // Also search the public dataset workspace if any BigQuery datasource has include_public_datasets enabled.
        let is_pg = ctx.db.is_postgres();
        let json_field = kyomi_core::sql_compat::json_extract_text(is_pg, "connection_config", "include_public_datasets");
        let bool_true = kyomi_core::sql_compat::bool_true(is_pg);
        let include_public_sql = format!(
            "SELECT COUNT(*) FROM datasource_configs \
             WHERE workspace_id = $1 \
               AND datasource_type = 'bigquery' \
               AND active = {bool_true} \
               AND COALESCE({json_field}, 'true') = 'true'"
        );
        let include_public: bool = kyomi_core::db_fetch_scalar!(
            ctx.db, i64,
            &include_public_sql,
            &ctx.workspace_id
        )
        .unwrap_or(0) > 0;

        if include_public {
            if let Ok(public_result) = kyomi_knowledge::retrieval::retrieve(
                vsearch.as_ref(),
                embed,
                kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID,
                query,
                &HashSet::new(),
                Some(2048),
            )
            .await
            {
                result.entries.extend(public_result.entries);
                // Re-sort by score descending after merging
                result
                    .entries
                    .sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
            }
        }

        // Format entries as structured JSON
        let mut results: Vec<serde_json::Value> = result
            .entries
            .into_iter()
            .map(|entry| {
                let entry_type = match entry.kind {
                    kyomi_knowledge::ContextEntryKind::Table => "table",
                    kyomi_knowledge::ContextEntryKind::Learning => "learning",
                    kyomi_knowledge::ContextEntryKind::Metric => "metric",
                };

                let mut obj = serde_json::json!({
                    "type": entry_type,
                    "id": entry.id,
                    "text": entry.text,
                    "score": format!("{:.2}", entry.score),
                });

                if !entry.matched_columns.is_empty() {
                    let cols: Vec<serde_json::Value> = entry
                        .matched_columns
                        .iter()
                        .map(|c| {
                            serde_json::json!({
                                "name": c.name,
                                "data_type": c.data_type,
                                "score": format!("{:.2}", c.score),
                            })
                        })
                        .collect();
                    obj["matched_columns"] = serde_json::json!(cols);
                }

                obj
            })
            .collect();

        // Resolve datasource slugs for table entries so the agent knows
        // which datasource each table belongs to.
        let has_tables = results.iter().any(|e| e["type"].as_str() == Some("table"));
        let bool_false = kyomi_core::sql_compat::bool_false(is_pg);
        let tc_full_name = kyomi_core::sql_compat::full_table_name_expr_prefixed(is_pg, "tc");
        if has_tables {
            let slug_sql = format!(
                "SELECT \
                    {tc_full_name} AS full_name, \
                    dc.slug \
                 FROM datasource_table_cache tc \
                 JOIN datasource_configs dc ON dc.id = tc.datasource_config_id \
                 WHERE dc.workspace_id = $1 AND tc.is_archived = {bool_false}"
            );
            let rows: Vec<TableSlugRow> = kyomi_core::db_fetch_all!(
                ctx.db, TableSlugRow,
                &slug_sql,
                &ctx.workspace_id
            )
            .unwrap_or_default();

            let name_to_slug: HashMap<String, String> = rows
                .into_iter()
                .filter_map(|r| {
                    let full_name = r.full_name?;
                    Some((full_name, r.slug))
                })
                .collect();

            for entry in results.iter_mut() {
                if entry["type"].as_str() == Some("table") {
                    if let Some(id) = entry["id"].as_str() {
                        if let Some(slug) = name_to_slug.get(id) {
                            entry["datasource"] = serde_json::json!(slug);
                        }
                    }
                }
            }
        }

        // Post-filter by datasource slug if specified
        if let Some(slug) = datasource_slug {
            // For table entries, the id is the full_name which doesn't contain the slug.
            // Resolve the slug and check if table entries belong to it via the cache.
            let ds = kyomi_auth::datasource_service::resolve_datasource(
                &ctx.db,
                slug,
                &ctx.workspace_id,
                false,
            )
            .await?;

            // Get all table full_names for this datasource from the cache
            let bare_full_name = kyomi_core::sql_compat::full_table_name_expr(is_pg);
            let ds_tables_sql = format!(
                "SELECT {bare_full_name} AS full_name \
                 FROM datasource_table_cache \
                 WHERE datasource_config_id = $1 AND is_archived = {bool_false}"
            );
            let ds_table_rows: Vec<TableFullNameRow> = kyomi_core::db_fetch_all!(
                ctx.db, TableFullNameRow,
                &ds_tables_sql,
                &ds.id
            )
            .unwrap_or_default();
            let ds_tables: HashSet<String> = ds_table_rows
                .into_iter()
                .filter_map(|r| r.full_name)
                .collect();

            results.retain(|entry| {
                let entry_type = entry["type"].as_str().unwrap_or("");
                if entry_type == "table" {
                    let id = entry["id"].as_str().unwrap_or("");
                    ds_tables.contains(id)
                } else {
                    true // keep learnings and metrics regardless
                }
            });
        }

        // Apply limit
        results.truncate(limit);
        let found = results.len();

        Ok(serde_json::json!({
            "results": results,
            "found": found,
        })
        .to_string())
    }
}
