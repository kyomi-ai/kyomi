// SPDX-License-Identifier: AGPL-3.0-or-later

//! Knowledge search tool — unified search across the workspace knowledge base.
//!
//! Uses pgvector-based semantic search to find tables, learnings, and metrics
//! in a single call.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use kyomi_core::models::DocType;

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
        "Search the workspace's knowledge base for relevant tables, dashboards, \
         knowledge documents, learnings, and metrics using semantic search. \
         Use this to discover what is available before querying. Pass `doc_type` \
         to restrict results to only dashboards or only knowledge documents \
         (omit for everything including tables and metrics)."
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
                "doc_type": {
                    "type": "string",
                    "enum": ["dashboard", "knowledge"],
                    "description": "Optional document type filter. When set, only returns matching dashboards or knowledge documents (tables, metrics, and legacy learnings are excluded). Omit to search everything."
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
        let doc_type_filter: Option<DocType> = args
            .get("doc_type")
            .and_then(|v| v.as_str())
            .map(|s| match s {
                "dashboard" => Ok(DocType::Dashboard),
                "knowledge" => Ok(DocType::Knowledge),
                other => Err(kyomi_core::Error::BadRequest(format!(
                    "Invalid doc_type '{other}' — must be 'dashboard' or 'knowledge'"
                ))),
            })
            .transpose()?;
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
            &ctx.db, &ctx.workspace_id, &query_embedding, limit, doc_type_filter,
        ).await;

        // When doc_type filter is specified, exclude legacy table/metric/learning
        // entries — the caller is asking specifically for documents of that type.
        let legacy_entries: Vec<_> = if doc_type_filter.is_some() {
            Vec::new()
        } else {
            result.entries
        };

        // Format entries as structured JSON
        let mut results: Vec<serde_json::Value> = legacy_entries
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
    dashboard_id: String,
    file_name: String,
    doc_type: String,
    chunk_content: String,
    // SQLite path:   raw f32 LE bytes, cosine similarity computed in Rust.
    // Postgres path: always empty (`''::bytea` placeholder). The field exists
    // only because `sqlx::FromRow` requires every selected column to map to a
    // struct field; the actual scoring on Postgres comes from `score` below.
    embedding: Vec<u8>,
    // Postgres path: `1 - (embedding <=> $2::vector)` — real cosine similarity.
    // SQLite path:   NULL, since we compute from `embedding` bytes in Rust.
    score: Option<f64>,
}

async fn search_knowledge_chunks(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    query_embedding: &[f32],
    limit: usize,
    doc_type_filter: Option<DocType>,
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
            let doc_type_clause = if doc_type_filter.is_some() {
                "AND d.doc_type = $4 "
            } else {
                ""
            };
            let sql = format!(
                "SELECT kc.dashboard_id, d.title AS file_name, \
                        COALESCE(d.doc_type, 'dashboard') AS doc_type, \
                        kc.content AS chunk_content, \
                        ''::bytea AS embedding, \
                        (1.0 - (kc.embedding <=> $2::vector))::float8 AS score \
                 FROM knowledge_chunks kc \
                 JOIN dashboards d ON d.dashboard_id = kc.dashboard_id \
                 WHERE kc.workspace_id = $1 \
                 {doc_type_clause}\
                 ORDER BY kc.embedding <=> $2::vector \
                 LIMIT $3"
            );
            let mut query = sqlx::query_as::<_, KnowledgeChunkRow>(&sql)
                .bind(workspace_id)
                .bind(&vec_str)
                .bind(limit as i64);
            if let Some(dt) = doc_type_filter {
                query = query.bind(dt.as_str());
            }
            query.fetch_all(pg).await.unwrap_or_default()
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            // SQLite path loads all matching chunks and computes similarity in Rust.
            let doc_type_clause = if doc_type_filter.is_some() {
                "AND d.doc_type = $2 "
            } else {
                ""
            };
            let sql = format!(
                "SELECT kc.dashboard_id, d.title AS file_name, \
                        COALESCE(d.doc_type, 'dashboard') AS doc_type, \
                        kc.content AS chunk_content, kc.embedding, \
                        NULL AS score \
                 FROM knowledge_chunks kc \
                 JOIN dashboards d ON d.dashboard_id = kc.dashboard_id \
                 WHERE kc.workspace_id = $1 \
                 {doc_type_clause}"
            );
            let mut query = sqlx::query_as::<_, KnowledgeChunkRow>(&sql).bind(workspace_id);
            if let Some(dt) = doc_type_filter {
                query = query.bind(dt.as_str());
            }
            query.fetch_all(sq).await.unwrap_or_default()
        }
    };
    if rows.is_empty() {
        return Vec::new();
    }

    let mut file_scores: HashMap<String, (f64, String, String, String)> = HashMap::new();
    for row in &rows {
        let score = if let Some(s) = row.score {
            s
        } else {
            let chunk_emb: Vec<f32> = row
                .embedding
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            cosine_similarity(query_embedding, &chunk_emb)
        };
        let entry = file_scores
            .entry(row.dashboard_id.clone())
            .or_insert((0.0, row.file_name.clone(), row.chunk_content.chars().take(200).collect(), row.doc_type.clone()));
        if score > entry.0 {
            entry.0 = score;
            entry.2 = row.chunk_content.chars().take(200).collect();
        }
    }

    let min_score = 0.25;
    let mut results: Vec<serde_json::Value> = file_scores
        .into_iter()
        .filter(|(_, (score, _, _, _))| *score >= min_score)
        .map(|(dashboard_id, (score, name, preview, doc_type))| {
            serde_json::json!({
                "type": "document",
                "source_type": doc_type,
                "id": dashboard_id,
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
// Helper: look up a document by title (exact match via search)
// ---------------------------------------------------------------------------

/// Search for a document by exact title match within the workspace.
/// Returns the dashboard if found.
async fn find_document_by_title(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    title: &str,
) -> kyomi_core::Result<Option<kyomi_core::models::Dashboard>> {
    let results = kyomi_auth::dashboard_service::search_dashboards(
        db,
        workspace_id,
        Some(title),
        None,
        kyomi_auth::dashboard_service::SearchSort::Recent,
        100,
    )
    .await?;

    // Find exact title match (case-sensitive)
    let matched = results.iter().find(|d| d.title == title);

    if let Some(m) = matched {
        kyomi_auth::dashboard_service::get_dashboard(db, &m.dashboard_id, workspace_id).await
    } else {
        Ok(None)
    }
}

/// Resolve a document by path or ID. Supports:
/// - UUID lookup (if input looks like a UUID)
/// - Exact title match
/// - Backward compat: if path contains `/`, strip directory and search by filename
async fn resolve_document(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    path: &str,
) -> kyomi_core::Result<Option<kyomi_core::models::Dashboard>> {
    // If it looks like a UUID, try direct lookup first
    if uuid::Uuid::parse_str(path).is_ok() {
        let doc = kyomi_auth::dashboard_service::get_dashboard(db, path, workspace_id).await?;
        if doc.is_some() {
            return Ok(doc);
        }
    }

    // Try exact title match
    let doc = find_document_by_title(db, workspace_id, path).await?;
    if doc.is_some() {
        return Ok(doc);
    }

    // Backward compat: if path contains `/`, strip directory and search by filename
    if let Some(slash_pos) = path.rfind('/') {
        let filename = &path[slash_pos + 1..];
        if !filename.is_empty() {
            return find_document_by_title(db, workspace_id, filename).await;
        }
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// WriteDocumentTool
// ---------------------------------------------------------------------------

pub struct WriteDocumentTool;

#[async_trait]
impl AgentTool for WriteDocumentTool {
    fn name(&self) -> &str {
        "write_knowledge_file"
    }

    fn description(&self) -> &str {
        "Create a new document or overwrite an existing one. Use for creating new markdown \
         documents in the knowledge base. For updating existing documents, prefer edit_knowledge_file."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Document title or path. If path contains '/', the last segment is used as the title."
                },
                "content": {
                    "type": "string",
                    "description": "Full markdown content for the document"
                },
                "content_hash": {
                    "type": "string",
                    "description": "Hash from a prior read_knowledge_file response. Required when overwriting an existing document."
                },
                "doc_type": {
                    "type": "string",
                    "description": "Document type: 'knowledge' (default) or 'dashboard'",
                    "enum": ["knowledge", "dashboard"],
                    "default": "knowledge"
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
        let doc_type = args
            .get("doc_type")
            .and_then(|v| v.as_str())
            .map(DocType::from_str_or_default)
            .unwrap_or(DocType::Knowledge);

        // Extract title: if path contains '/', use the last segment
        let title = if let Some(slash_pos) = path.rfind('/') {
            &path[slash_pos + 1..]
        } else {
            path
        };

        let embed = ctx.embedding.wait_ready().await?;

        // Look up existing document by title
        let existing = find_document_by_title(&ctx.db, &ctx.workspace_id, title).await?;

        if let Some(doc) = existing {
            // Update existing document
            match kyomi_auth::dashboard_service::update_dashboard(
                kyomi_auth::dashboard_service::UpdateDashboardParams {
                    db: &ctx.db,
                    embed: None, // rechunking handled explicitly below
                    dashboard_id: &doc.dashboard_id,
                    workspace_id: &ctx.workspace_id,
                    user_id: &ctx.user_id,
                    title: None,
                    content: Some(content),
                    change_summary: None,
                    expected_content_hash: content_hash,
                },
            )
            .await
            {
                Ok(updated) => {
                    if !updated {
                        return Ok(serde_json::json!({
                            "success": false,
                            "error": "Document not found or not updated",
                        })
                        .to_string());
                    }

                    // Rechunk after update
                    kyomi_auth::dashboard_service::rechunk_document(
                        &ctx.db,
                        embed,
                        &doc.dashboard_id,
                        content,
                        &ctx.workspace_id,
                    )
                    .await?;

                    let new_hash = kyomi_auth::dashboard_service::hash_content(content);
                    Ok(serde_json::json!({
                        "success": true,
                        "action": "updated",
                        "id": doc.dashboard_id,
                        "path": path,
                        "content_hash": new_hash,
                    })
                    .to_string())
                }
                Err(kyomi_core::Error::Conflict(msg)) => {
                    Ok(serde_json::json!({
                        "success": false,
                        "error": msg,
                    })
                    .to_string())
                }
                Err(e) => Err(e),
            }
        } else {
            // Create new document
            let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
                &ctx.db,
                &ctx.user_id,
                &ctx.workspace_id,
                title,
                content,
                doc_type,
                None, // Agent does explicit sync rechunk below
            )
            .await?;

            // Rechunk the new document
            kyomi_auth::dashboard_service::rechunk_document(
                &ctx.db,
                embed,
                &dashboard_id,
                content,
                &ctx.workspace_id,
            )
            .await?;

            let new_hash = kyomi_auth::dashboard_service::hash_content(content);
            Ok(serde_json::json!({
                "success": true,
                "action": "created",
                "id": dashboard_id,
                "path": path,
                "content_hash": new_hash,
            })
            .to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// ReadDocumentTool
// ---------------------------------------------------------------------------

pub struct ReadDocumentTool;

#[async_trait]
impl AgentTool for ReadDocumentTool {
    fn name(&self) -> &str {
        "read_knowledge_file"
    }

    fn description(&self) -> &str {
        "Read a specific document by title or ID. Returns the full markdown content. \
         Use this when you know which document to look at (from search results)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Document title, ID, or legacy path (e.g. 'Revenue/Metrics.md')"
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

        let doc = resolve_document(&ctx.db, &ctx.workspace_id, path).await?;
        match doc {
            Some(d) => Ok(serde_json::json!({
                "id": d.dashboard_id,
                "path": path,
                "name": d.title,
                "doc_type": d.doc_type().as_str(),
                "content": d.content,
                "content_hash": d.content_hash,
                "updated_at": d.updated_at.to_rfc3339(),
            })
            .to_string()),
            None => Ok(serde_json::json!({
                "error": format!("Document not found: {path}"),
            })
            .to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// ListDocumentsTool
// ---------------------------------------------------------------------------

pub struct ListDocumentsTool;

#[async_trait]
impl AgentTool for ListDocumentsTool {
    fn name(&self) -> &str {
        "list_knowledge_files"
    }

    fn description(&self) -> &str {
        "List all documents in the workspace. Returns titles, types, and metadata. \
         Optionally filter by document type (dashboard or knowledge)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "doc_type": {
                    "type": "string",
                    "description": "Filter by document type: 'dashboard', 'knowledge', or omit for all",
                    "enum": ["dashboard", "knowledge"]
                }
            },
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
        let doc_type_filter = args
            .get("doc_type")
            .and_then(|v| v.as_str())
            .map(DocType::from_str_or_default);

        let results = kyomi_auth::dashboard_service::search_dashboards(
            &ctx.db,
            &ctx.workspace_id,
            None,
            doc_type_filter,
            kyomi_auth::dashboard_service::SearchSort::Recent,
            100,
        )
        .await?;

        let entries: Vec<serde_json::Value> = results
            .iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.dashboard_id,
                    "title": d.title,
                    "doc_type": d.doc_type,
                    "updated_at": d.updated_at.to_rfc3339(),
                    "content_preview": d.content_preview,
                })
            })
            .collect();
        let count = entries.len();
        Ok(serde_json::json!({
            "files": entries,
            "count": count,
        })
        .to_string())
    }
}

// ---------------------------------------------------------------------------
// EditDocumentTool
// ---------------------------------------------------------------------------

pub struct EditDocumentTool;

#[async_trait]
impl AgentTool for EditDocumentTool {
    fn name(&self) -> &str {
        "edit_knowledge_file"
    }

    fn description(&self) -> &str {
        "Make a targeted edit to an existing document using string replacement. \
         Send only the old and new text. Fails if old_text is not found or appears multiple times."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Document title, ID, or legacy path"
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

        let doc = resolve_document(&ctx.db, &ctx.workspace_id, path)
            .await?
            .ok_or_else(|| kyomi_core::Error::NotFound(format!("Document not found: {path}")))?;

        // Verify old_text appears exactly once
        let occurrences = doc.content.matches(old_text).count();
        if occurrences == 0 {
            return Ok(serde_json::json!({
                "success": false,
                "error": "old_text not found in document content",
            })
            .to_string());
        }
        if occurrences > 1 {
            return Ok(serde_json::json!({
                "success": false,
                "error": format!("old_text appears {occurrences} times — must appear exactly once"),
            })
            .to_string());
        }

        // Apply the replacement
        let new_content = doc.content.replacen(old_text, new_text, 1);
        let content_hash = doc.content_hash.as_deref();

        let embed = ctx.embedding.wait_ready().await?;

        // Update via dashboard_service with CAS
        match kyomi_auth::dashboard_service::update_dashboard(
            kyomi_auth::dashboard_service::UpdateDashboardParams {
                db: &ctx.db,
                embed: None, // rechunking handled explicitly below
                dashboard_id: &doc.dashboard_id,
                workspace_id: &ctx.workspace_id,
                user_id: &ctx.user_id,
                title: None,
                content: Some(&new_content),
                change_summary: None,
                expected_content_hash: content_hash,
            },
        )
        .await
        {
            Ok(updated) => {
                if !updated {
                    return Ok(serde_json::json!({
                        "success": false,
                        "error": "Document not found or not updated",
                    })
                    .to_string());
                }

                // Rechunk after edit
                kyomi_auth::dashboard_service::rechunk_document(
                    &ctx.db,
                    embed,
                    &doc.dashboard_id,
                    &new_content,
                    &ctx.workspace_id,
                )
                .await?;

                let new_hash = kyomi_auth::dashboard_service::hash_content(&new_content);
                Ok(serde_json::json!({
                    "success": true,
                    "id": doc.dashboard_id,
                    "path": path,
                    "content_hash": new_hash,
                })
                .to_string())
            }
            Err(kyomi_core::Error::Conflict(msg)) => {
                Ok(serde_json::json!({
                    "success": false,
                    "error": msg,
                })
                .to_string())
            }
            Err(e) => Err(e),
        }
    }
}
