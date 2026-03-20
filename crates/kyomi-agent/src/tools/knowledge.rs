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

        if include_public
            && let Ok(public_result) = kyomi_knowledge::retrieval::retrieve(
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

        // Search knowledge file chunks (new knowledge system)
        let query_embedding = embed.embed_passage(query)?;
        let knowledge_file_results = search_knowledge_chunks(
            &ctx.db, &ctx.workspace_id, &query_embedding, limit,
        ).await;

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
                if entry["type"].as_str() == Some("table")
                    && let Some(id) = entry["id"].as_str()
                        && let Some(slug) = name_to_slug.get(id) {
                            entry["datasource"] = serde_json::json!(slug);
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

        // Merge knowledge file results
        for kf in knowledge_file_results {
            results.push(kf);
        }
        // Re-sort all results by score descending
        results.sort_by(|a, b| {
            let sa: f64 = a["score"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let sb: f64 = b["score"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });

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

// ---------------------------------------------------------------------------
// Knowledge chunk search helper
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct KnowledgeChunkRow {
    file_id: String,
    file_name: String,
    #[allow(dead_code)]
    file_path: Option<String>,
    chunk_content: String,
    embedding: Vec<u8>,
}

async fn search_knowledge_chunks(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    query_embedding: &[f32],
    limit: usize,
) -> Vec<serde_json::Value> {
    // For SQLite: load all chunks, compute cosine similarity in memory
    // For Postgres: use pgvector ORDER BY embedding <=> $2::vector
    let rows = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let vec_str = format!(
                "[{}]",
                query_embedding
                    .iter()
                    .map(|f| f.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            sqlx::query_as::<_, KnowledgeChunkRow>(
                "SELECT kc.file_id, kf.name AS file_name, NULL AS file_path, \
                        kc.content AS chunk_content, ''::bytea AS embedding \
                 FROM knowledge_chunks kc \
                 JOIN knowledge_files kf ON kf.id = kc.file_id \
                 WHERE kc.workspace_id = $1 \
                 ORDER BY kc.embedding <=> $2::vector \
                 LIMIT $3",
            )
            .bind(workspace_id)
            .bind(&vec_str)
            .bind(limit as i64)
            .fetch_all(pg)
            .await
            .unwrap_or_default()
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            sqlx::query_as::<_, KnowledgeChunkRow>(
                "SELECT kc.file_id, kf.name AS file_name, NULL AS file_path, \
                        kc.content AS chunk_content, kc.embedding \
                 FROM knowledge_chunks kc \
                 JOIN knowledge_files kf ON kf.id = kc.file_id \
                 WHERE kc.workspace_id = $1",
            )
            .bind(workspace_id)
            .fetch_all(sq)
            .await
            .unwrap_or_default()
        }
    };
    if rows.is_empty() {
        return Vec::new();
    }

    let mut file_scores: HashMap<String, (f64, String, String)> = HashMap::new();
    for row in &rows {
        let score = if row.embedding.is_empty() {
            0.6 // Postgres path -- approximate score
        } else {
            let chunk_emb: Vec<f32> = row
                .embedding
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            cosine_similarity(query_embedding, &chunk_emb)
        };
        let entry = file_scores
            .entry(row.file_id.clone())
            .or_insert((0.0, row.file_name.clone(), row.chunk_content.chars().take(200).collect()));
        if score > entry.0 {
            entry.0 = score;
            entry.2 = row.chunk_content.chars().take(200).collect();
        }
    }

    let min_score = 0.25;
    let mut results: Vec<serde_json::Value> = file_scores
        .into_iter()
        .filter(|(_, (score, _, _))| *score >= min_score)
        .map(|(file_id, (score, name, preview))| {
            serde_json::json!({
                "type": "knowledge_file",
                "id": file_id,
                "text": format!("{name}: {preview}"),
                "score": format!("{:.2}", score),
                "file_name": name,
            })
        })
        .collect();
    results.sort_by(|a, b| {
        let sa: f64 = a["score"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let sb: f64 = b["score"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);
    results
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b.iter()) {
        let (x, y) = (*x as f64, *y as f64);
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let d = na.sqrt() * nb.sqrt();
    if d == 0.0 {
        0.0
    } else {
        dot / d
    }
}

// ---------------------------------------------------------------------------
// WriteKnowledgeFileTool
// ---------------------------------------------------------------------------

pub struct WriteKnowledgeFileTool;

#[async_trait]
impl AgentTool for WriteKnowledgeFileTool {
    fn name(&self) -> &str {
        "write_knowledge_file"
    }

    fn description(&self) -> &str {
        "Create a new knowledge file or overwrite an existing one. Use for creating new markdown \
         documents in the knowledge base. For updating existing files, prefer edit_knowledge_file. \
         Parent folders are created automatically if they don't exist."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (e.g. 'Revenue/Metrics.md'). Parent folders created automatically."
                },
                "content": {
                    "type": "string",
                    "description": "Full markdown content for the file"
                },
                "content_hash": {
                    "type": "string",
                    "description": "Hash from a prior read_knowledge_file response. Required when overwriting an existing file."
                }
            },
            "required": ["path", "content"]
        })
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            read_only_hint: Some(false),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| kyomi_core::Error::BadRequest("path is required".into()))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| kyomi_core::Error::BadRequest("content is required".into()))?;
        let content_hash = args["content_hash"].as_str();

        let existing = kyomi_knowledge::knowledge_files::get_file_by_path(
            &ctx.db,
            &ctx.workspace_id,
            path,
        )
        .await
        .map_err(|e| kyomi_core::Error::Internal(e.to_string()))?;

        if let Some(file) = existing {
            let result = kyomi_knowledge::knowledge_files::update_file_content(
                &ctx.db,
                ctx.embedding.wait_ready().await?,
                &ctx.workspace_id,
                &file.id,
                content,
                &ctx.user_id,
                content_hash,
            )
            .await
            .map_err(|e| kyomi_core::Error::Internal(e.to_string()))?;
            match result {
                Some(f) => Ok(serde_json::json!({
                    "success": true,
                    "action": "updated",
                    "id": f.id,
                    "path": path,
                    "content_hash": f.content_hash,
                })
                .to_string()),
                None => Ok(serde_json::json!({
                    "success": false,
                    "error": "File was modified since you last read it. Read it again to get the current content_hash.",
                })
                .to_string()),
            }
        } else {
            let (parent_id, file_name) = if let Some(slash_pos) = path.rfind('/') {
                let folder_path = &path[..slash_pos];
                let file_name = &path[slash_pos + 1..];
                let parent = kyomi_knowledge::knowledge_files::ensure_parent_folders(
                    &ctx.db,
                    &ctx.workspace_id,
                    folder_path,
                    &ctx.user_id,
                )
                .await
                .map_err(|e| kyomi_core::Error::Internal(e.to_string()))?;
                (parent, file_name)
            } else {
                (None, path)
            };
            let file = kyomi_knowledge::knowledge_files::create_file(
                &ctx.db,
                ctx.embedding.wait_ready().await?,
                &ctx.workspace_id,
                parent_id.as_deref(),
                file_name,
                Some(content),
                false,
                &ctx.user_id,
            )
            .await
            .map_err(|e| kyomi_core::Error::Internal(e.to_string()))?;
            Ok(serde_json::json!({
                "success": true,
                "action": "created",
                "id": file.id,
                "path": path,
                "content_hash": file.content_hash,
            })
            .to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// ReadKnowledgeFileTool
// ---------------------------------------------------------------------------

pub struct ReadKnowledgeFileTool;

#[async_trait]
impl AgentTool for ReadKnowledgeFileTool {
    fn name(&self) -> &str {
        "read_knowledge_file"
    }

    fn description(&self) -> &str {
        "Read a specific knowledge file by path. Returns the full markdown content. \
         Use this when you know which file to look at (from the knowledge tree or search results)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (e.g. 'Revenue/Metrics.md')"
                }
            },
            "required": ["path"]
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
        let path = args["path"]
            .as_str()
            .ok_or_else(|| kyomi_core::Error::BadRequest("path is required".into()))?;
        let file = kyomi_knowledge::knowledge_files::get_file_by_path(
            &ctx.db,
            &ctx.workspace_id,
            path,
        )
        .await
        .map_err(|e| kyomi_core::Error::Internal(e.to_string()))?;
        match file {
            Some(f) => Ok(serde_json::json!({
                "id": f.id,
                "path": path,
                "name": f.name,
                "content": f.content,
                "content_hash": f.content_hash,
                "updated_at": f.updated_at.to_rfc3339(),
            })
            .to_string()),
            None => Ok(serde_json::json!({
                "error": format!("File not found: {path}"),
            })
            .to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// ListKnowledgeFilesTool
// ---------------------------------------------------------------------------

pub struct ListKnowledgeFilesTool;

#[async_trait]
impl AgentTool for ListKnowledgeFilesTool {
    fn name(&self) -> &str {
        "list_knowledge_files"
    }

    fn description(&self) -> &str {
        "List all knowledge files and folders in the workspace. Returns the file tree \
         structure with names, types (file/folder), and hierarchy."
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
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let _ = args;
        let tree = kyomi_knowledge::knowledge_files::list_tree(
            &ctx.db,
            &ctx.workspace_id,
        )
        .await
        .map_err(|e| kyomi_core::Error::Internal(e.to_string()))?;
        let entries: Vec<serde_json::Value> = tree
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "parent_id": e.parent_id,
                    "name": e.name,
                    "is_folder": e.is_folder,
                    "updated_at": e.updated_at.to_rfc3339(),
                })
            })
            .collect();
        Ok(serde_json::json!({
            "files": entries,
            "count": entries.len(),
        })
        .to_string())
    }
}

// ---------------------------------------------------------------------------
// EditKnowledgeFileTool
// ---------------------------------------------------------------------------

pub struct EditKnowledgeFileTool;

#[async_trait]
impl AgentTool for EditKnowledgeFileTool {
    fn name(&self) -> &str {
        "edit_knowledge_file"
    }

    fn description(&self) -> &str {
        "Make a targeted edit to an existing knowledge file using string replacement. \
         Send only the old and new text. Fails if old_text is not found or appears multiple times."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (e.g. 'Revenue/Metrics.md')"
                },
                "old_text": {
                    "type": "string",
                    "description": "Exact string to find"
                },
                "new_text": {
                    "type": "string",
                    "description": "Replacement string"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            read_only_hint: Some(false),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| kyomi_core::Error::BadRequest("path is required".into()))?;
        let old_text = args["old_text"]
            .as_str()
            .ok_or_else(|| kyomi_core::Error::BadRequest("old_text is required".into()))?;
        let new_text = args["new_text"]
            .as_str()
            .ok_or_else(|| kyomi_core::Error::BadRequest("new_text is required".into()))?;
        let file = kyomi_knowledge::knowledge_files::get_file_by_path(
            &ctx.db,
            &ctx.workspace_id,
            path,
        )
        .await
        .map_err(|e| kyomi_core::Error::Internal(e.to_string()))?
        .ok_or_else(|| kyomi_core::Error::NotFound(format!("File not found: {path}")))?;
        let updated = kyomi_knowledge::knowledge_files::edit_file_content(
            &ctx.db,
            ctx.embedding.wait_ready().await?,
            &ctx.workspace_id,
            &file.id,
            old_text,
            new_text,
            &ctx.user_id,
        )
        .await
        .map_err(|e| kyomi_core::Error::Internal(e.to_string()))?;
        Ok(serde_json::json!({
            "success": true,
            "id": updated.id,
            "path": path,
            "content_hash": updated.content_hash,
        })
        .to_string())
    }
}
