// SPDX-License-Identifier: AGPL-3.0-or-later

//! Knowledge search tool — unified search across the workspace knowledge base.
//!
//! Uses pgvector-based semantic search to find tables and metrics
//! in a single call.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use kyomi_auth::websocket::helpers as ws_helpers;
use kyomi_core::models::DocType;

use crate::tools::document::{
    apply_update, find_document_by_title, ApplyUpdateOutcome, ApplyUpdateParams, DocumentEditTool,
    DocumentReadTool,
};
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

/// Whether any active BigQuery datasource in `workspace_id` has "Include
/// public datasets" enabled — gates the second, public-workspace knowledge
/// search below.
///
/// Extracted out of [`SearchKnowledgeTool::execute`] so this predicate can be
/// exercised directly against a real (in-memory) database, without also
/// fixturing the embedding pipeline the rest of that method depends on.
///
/// The `COALESCE(..., 'false')` here must stay in step with
/// `kyomi_core::json_utils::bigquery_include_public` — that Rust function is
/// the single source of truth for what an *absent* key means (disabled);
/// this SQL predicate can't call it directly, so the `'false'` default is
/// hand-mirrored (KYO-446). A DB error collapses to "no public datasources
/// enabled" rather than propagating, matching the pre-KYO-446 behavior at
/// this call site.
///
/// Known gap (pre-existing, not introduced by KYO-446, tracked as KYO-451):
/// this predicate compares the extracted value against the text literal
/// `'true'`. That's correct on every backend for the JSON *string* `"true"`
/// (what the settings UI's save path actually persists — see KYO-21) and
/// correct on Postgres for a genuine JSON *boolean* `true` too (`->>` always
/// yields text), but not on SQLite: `json_extract()` there returns SQLite's
/// native integer `1`/`0` for JSON booleans, which never equals the text
/// literal `'true'`.
async fn workspace_wants_bigquery_public_datasets(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
) -> bool {
    let is_pg = db.is_postgres();
    let json_field =
        kyomi_core::sql_compat::json_extract_text(is_pg, "connection_config", "include_public_datasets");
    let bool_true = kyomi_core::sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT COUNT(*) FROM datasource_configs \
         WHERE workspace_id = $1 \
           AND datasource_type = 'bigquery' \
           AND active = {bool_true} \
           AND COALESCE({json_field}, 'false') = 'true'"
    );
    kyomi_core::db_fetch_scalar!(db, i64, &sql, workspace_id).unwrap_or(0) > 0
}

// ---------------------------------------------------------------------------
// SearchKnowledgeTool
// ---------------------------------------------------------------------------

/// Unified search across the workspace knowledge base.
///
/// Searches tables and metrics using pgvector-based semantic
/// search in PostgreSQL.
pub struct SearchKnowledgeTool;

#[async_trait]
impl AgentTool for SearchKnowledgeTool {
    fn name(&self) -> &str {
        "search_knowledge"
    }

    fn description(&self) -> &str {
        "Search the workspace's knowledge base for relevant tables, dashboards, \
         knowledge documents, and metrics using semantic search. \
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
                    "description": "Optional document type filter. When set, only returns matching dashboards or knowledge documents (tables and metrics are excluded). Omit to search everything."
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
        .await?;

        // Also search the public dataset workspace if any BigQuery datasource has include_public_datasets enabled.
        let include_public = workspace_wants_bigquery_public_datasets(&ctx.db, &ctx.workspace_id).await;

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
            &ctx.db, &ctx.workspace_id, &ctx.user_id, &query_embedding, limit, doc_type_filter,
        ).await;

        // When doc_type filter is specified, exclude legacy table/metric entries —
        // the caller is asking specifically for documents of that type.
        // Learning entries are always excluded from tool results.
        let legacy_entries: Vec<_> = if doc_type_filter.is_some() {
            Vec::new()
        } else {
            result
                .entries
                .into_iter()
                .filter(|e| e.kind != kyomi_knowledge::ContextEntryKind::Learning)
                .collect()
        };

        // Format entries as structured JSON
        let mut results: Vec<serde_json::Value> = legacy_entries
            .into_iter()
            .filter_map(|entry| {
                let entry_type = match entry.kind {
                    kyomi_knowledge::ContextEntryKind::Table => "table",
                    kyomi_knowledge::ContextEntryKind::Metric => "metric",
                    kyomi_knowledge::ContextEntryKind::Learning => return None,
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

                Some(obj)
            })
            .collect();

        // Resolve datasource slugs for table entries so the agent knows
        // which datasource each table belongs to.
        let has_tables = results.iter().any(|e| e["type"].as_str() == Some("table"));
        let is_pg = ctx.db.is_postgres();
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
                    // Keep metrics regardless of datasource; Learning entries
                    // are already excluded before this point.
                    entry_type == "metric"
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
    user_id: &str,
    query_embedding: &[f32],
    limit: usize,
    doc_type_filter: Option<DocType>,
) -> Vec<serde_json::Value> {
    // For SQLite: load all chunks, compute cosine similarity in memory
    // For Postgres: use pgvector ORDER BY embedding <=> $2::vector
    //
    // Left dual-armed (not routed through sql_compat::embedding_placeholder):
    // SQLite has no `<=>` operator, so these are two genuinely different
    // queries, not the same SQL differing only by a cast. The Postgres arm
    // orders and scores in SQL; the SQLite arm selects the raw embedding
    // column unordered and both order and score are computed afterward in
    // Rust (see the score-vs-embedding field split on `KnowledgeChunkRow`).
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
            // $1=workspace_id, $2=embedding, $3=limit, $4=user_id, $5=doc_type (optional)
            let vis_pred = kyomi_auth::dashboard_service::visibility_predicate(4, true);
            let doc_type_clause = if doc_type_filter.is_some() {
                "AND d.doc_type = $5 "
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
                 {vis_pred} \
                 {doc_type_clause}\
                 ORDER BY kc.embedding <=> $2::vector \
                 LIMIT $3"
            );
            let mut query = sqlx::query_as::<_, KnowledgeChunkRow>(&sql)
                .bind(workspace_id)
                .bind(&vec_str)
                .bind(limit as i64)
                .bind(user_id);
            if let Some(dt) = doc_type_filter {
                query = query.bind(dt.as_str());
            }
            query.fetch_all(pg).await.unwrap_or_default()
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            // SQLite path loads all matching chunks and computes similarity in Rust.
            // $1=workspace_id, $2=user_id, $3=doc_type (optional)
            let vis_pred = kyomi_auth::dashboard_service::visibility_predicate(2, false);
            let doc_type_clause = if doc_type_filter.is_some() {
                "AND d.doc_type = $3 "
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
                 {vis_pred} \
                 {doc_type_clause}"
            );
            let mut query = sqlx::query_as::<_, KnowledgeChunkRow>(&sql)
                .bind(workspace_id)
                .bind(user_id);
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
            let chunk_emb = kyomi_core::embedding_compat::bytes_to_embedding(&row.embedding);
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
//
// KYO-538: `find_document_by_title` and `resolve_document` moved unchanged
// to `tools::document` (the shared document-operations core) — both are
// doc_type-agnostic and are now also called from `tools::document`'s own
// `DocumentReadTool` / `DocumentEditTool`. Imported above so this file's
// production code and its pre-existing test module (`use super::*;` below)
// keep resolving the same bare names.

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
        let existing = find_document_by_title(&ctx.db, &ctx.workspace_id, &ctx.user_id, title).await?;

        if let Some(doc) = existing {
            // Update existing document
            let outcome = apply_update(ApplyUpdateParams {
                db: &ctx.db,
                dashboard_id: &doc.dashboard_id,
                workspace_id: &ctx.workspace_id,
                user_id: &ctx.user_id,
                title: None,
                content: Some(content),
                change_summary: None,
                expected_content_hash: content_hash,
            })
            .await?;

            match outcome {
                ApplyUpdateOutcome::Updated => {
                    // Rechunk after update
                    kyomi_auth::dashboard_service::rechunk_document(
                        &ctx.db,
                        embed,
                        &doc.dashboard_id,
                        content,
                        &ctx.workspace_id,
                    )
                    .await?;

                    ws_helpers::broadcast_dashboard_sync(
                        &ctx.db, &ctx.ws_manager, &doc.dashboard_id, &ctx.workspace_id,
                        kyomi_types::sync::SyncActionType::Update,
                        &ctx.user_id,
                    )
                    .await;

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
                ApplyUpdateOutcome::NotFound => Ok(serde_json::json!({
                    "success": false,
                    "error": "Document not found or not updated",
                })
                .to_string()),
                ApplyUpdateOutcome::Conflict(msg) => Ok(serde_json::json!({
                    "success": false,
                    "error": msg,
                })
                .to_string()),
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

            ws_helpers::broadcast_dashboard_sync(
                &ctx.db, &ctx.ws_manager, &dashboard_id, &ctx.workspace_id,
                kyomi_types::sync::SyncActionType::Insert,
                &ctx.user_id,
            )
            .await;

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
//
// KYO-538: the actual logic moved to `tools::document::DocumentReadTool`,
// which this unit struct delegates to entirely for `DocType::Knowledge` (see
// `read_document` there for the one real behavioural fork against the
// dashboard side). This struct exists only so the pre-existing test module
// below (`use super::*;`) keeps resolving `ReadDocumentTool` as a bare unit
// value, unchanged. `create_default_registry` constructs
// `document::DocumentReadTool::new(DocType::Knowledge)` directly, not this.
pub struct ReadDocumentTool;

#[async_trait]
impl AgentTool for ReadDocumentTool {
    fn name(&self) -> &str {
        DocumentReadTool::name_for(DocType::Knowledge)
    }

    fn description(&self) -> &str {
        DocumentReadTool::description_for(DocType::Knowledge)
    }

    fn parameters_schema(&self) -> serde_json::Value {
        DocumentReadTool::schema_for(DocType::Knowledge)
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
        DocumentReadTool::execute_for(DocType::Knowledge, args, ctx).await
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
            &ctx.user_id,
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
//
// KYO-538: the actual logic moved to `tools::document::DocumentEditTool`,
// which this unit struct delegates to entirely for `DocType::Knowledge`
// (including the CAS-via-`update_dashboard`, synchronous rechunk, and
// single `broadcast_dashboard_sync` call this tool always performed). This
// struct exists only so the pre-existing test module below
// (`use super::*;`) keeps resolving `EditDocumentTool` as a bare unit
// value, unchanged. `create_default_registry` constructs
// `document::DocumentEditTool::new(DocType::Knowledge)` directly, not this.
pub struct EditDocumentTool;

#[async_trait]
impl AgentTool for EditDocumentTool {
    fn name(&self) -> &str {
        DocumentEditTool::name_for(DocType::Knowledge)
    }

    fn description(&self) -> &str {
        DocumentEditTool::description_for(DocType::Knowledge)
    }

    fn parameters_schema(&self) -> serde_json::Value {
        DocumentEditTool::schema_for(DocType::Knowledge)
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
        DocumentEditTool::execute_for(DocType::Knowledge, args, ctx).await
    }
}

// ---------------------------------------------------------------------------
// Tests — KYO-446: `workspace_wants_bigquery_public_datasets` must honor an
// absent, `false`, and `true` `include_public_datasets` key identically
// (absent and `false` both mean "no public search"; only `true` triggers it).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{build_ctx, loaded_embedding, seed_user_and_workspace, test_pool};

    /// Seed workspace "ws-1" with one active BigQuery datasource whose
    /// `connection_config` is exactly `connection_config_json`.
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
    }

    #[tokio::test]
    async fn absent_key_defaults_to_disabled() {
        let db = test_pool().await;
        seed_bigquery_datasource(&db, "{}").await;

        let enabled = workspace_wants_bigquery_public_datasets(&db, "ws-1").await;

        assert!(
            !enabled,
            "an absent include_public_datasets key must default to disabled, not enabled"
        );
    }

    #[tokio::test]
    async fn false_stays_disabled() {
        let db = test_pool().await;
        seed_bigquery_datasource(&db, r#"{"include_public_datasets": false}"#).await;

        let enabled = workspace_wants_bigquery_public_datasets(&db, "ws-1").await;

        assert!(!enabled);
    }

    #[tokio::test]
    async fn true_is_enabled() {
        let db = test_pool().await;
        // Seeded as the JSON *string* "true", not a JSON boolean: this is
        // what the settings UI's Leptos URL-encoded save path actually
        // persists (see KYO-21's verification notes), and it's also the
        // shape this SQL predicate's text-literal comparison
        // (`json_extract_text(...) = 'true'`) is built to match on every
        // backend. A genuine JSON *boolean* `true` round-trips correctly
        // through this same comparison on Postgres, but not on SQLite —
        // `json_extract()` there returns SQLite's native integer 1/0 for
        // JSON booleans rather than text, so `1 = 'true'` is never true.
        // That's a pre-existing, backend-specific gap unrelated to the
        // absent/false/true default this ticket fixes; tracked separately
        // as KYO-451 rather than papered over in this test.
        seed_bigquery_datasource(&db, r#"{"include_public_datasets": "true"}"#).await;

        let enabled = workspace_wants_bigquery_public_datasets(&db, "ws-1").await;

        assert!(enabled);
    }

    #[tokio::test]
    async fn no_bigquery_datasource_is_disabled() {
        let db = test_pool().await;
        // No datasource_configs row at all for this workspace.
        let enabled = workspace_wants_bigquery_public_datasets(&db, "ws-1").await;
        assert!(!enabled);
    }

    // ---------------------------------------------------------------------
    // SearchKnowledgeTool — KYO-537 characterization tests.
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn search_knowledge_missing_query_is_bad_request() {
        let ctx = build_ctx(test_pool().await);
        let err = SearchKnowledgeTool
            .execute(serde_json::json!({}), &ctx)
            .await
            .expect_err("query is required");
        assert!(matches!(err, kyomi_core::Error::BadRequest(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn search_knowledge_invalid_doc_type_is_bad_request() {
        let ctx = build_ctx(test_pool().await);
        let err = SearchKnowledgeTool
            .execute(
                serde_json::json!({"query": "revenue", "doc_type": "bogus"}),
                &ctx,
            )
            .await
            .expect_err("invalid doc_type must be rejected");
        match err {
            kyomi_core::Error::BadRequest(msg) => {
                assert!(msg.contains("Invalid doc_type"), "{msg}");
            }
            other => panic!("expected BadRequest, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn search_knowledge_empty_workspace_returns_empty_results() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let mut ctx = build_ctx(db);
        ctx.embedding = loaded_embedding();

        let result = SearchKnowledgeTool
            .execute(serde_json::json!({"query": "revenue"}), &ctx)
            .await
            .expect("search_knowledge execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");

        assert_eq!(
            parsed,
            serde_json::json!({"results": [], "found": 0}),
            "exact shape of an empty-workspace search: {result}"
        );
    }

    #[tokio::test]
    async fn search_knowledge_unknown_datasource_filter_is_error() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let mut ctx = build_ctx(db);
        ctx.embedding = loaded_embedding();

        let err = SearchKnowledgeTool
            .execute(
                serde_json::json!({"query": "revenue", "datasource": "does-not-exist"}),
                &ctx,
            )
            .await
            .expect_err("an unresolvable datasource slug must error, not silently no-op");
        assert!(matches!(err, kyomi_core::Error::NotFound(_)), "got: {err:?}");
    }

    // ---------------------------------------------------------------------
    // WriteDocumentTool — KYO-537 characterization tests.
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn write_knowledge_file_missing_path_is_bad_request() {
        let ctx = build_ctx(test_pool().await);
        let err = WriteDocumentTool
            .execute(serde_json::json!({"content": "hello"}), &ctx)
            .await
            .expect_err("path is required");
        assert!(matches!(err, kyomi_core::Error::BadRequest(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn write_knowledge_file_missing_content_is_bad_request() {
        let ctx = build_ctx(test_pool().await);
        let err = WriteDocumentTool
            .execute(serde_json::json!({"path": "Doc"}), &ctx)
            .await
            .expect_err("content is required");
        assert!(matches!(err, kyomi_core::Error::BadRequest(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn write_knowledge_file_creates_new_document() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let mut ctx = build_ctx(db);
        ctx.embedding = loaded_embedding();

        let result = WriteDocumentTool
            .execute(
                serde_json::json!({"path": "Runbook", "content": "# Runbook\nSteps."}),
                &ctx,
            )
            .await
            .expect("write_knowledge_file execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");

        assert_eq!(parsed["success"], serde_json::json!(true), "{result}");
        assert_eq!(parsed["action"], serde_json::json!("created"), "{result}");
        assert_eq!(parsed["path"], serde_json::json!("Runbook"), "{result}");
        assert!(parsed["id"].as_str().is_some(), "{result}");
        assert!(parsed["content_hash"].as_str().is_some(), "{result}");

        // No 'doc_type' key was given, so the default per the tool's own
        // parameter schema — 'knowledge', not 'dashboard' — must be what
        // actually landed in the shared dashboards table.
        let doc = find_document_by_title(&ctx.db, "ws-1", "user-a", "Runbook")
            .await
            .expect("lookup")
            .expect("document exists");
        assert_eq!(doc.doc_type(), DocType::Knowledge);
        assert_eq!(doc.content, "# Runbook\nSteps.");
    }

    #[tokio::test]
    async fn write_knowledge_file_updates_existing_document_by_title() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let mut ctx = build_ctx(db);
        ctx.embedding = loaded_embedding();

        let create_result = WriteDocumentTool
            .execute(
                serde_json::json!({"path": "Runbook", "content": "v1"}),
                &ctx,
            )
            .await
            .expect("create");
        let created: serde_json::Value = serde_json::from_str(&create_result).expect("json");
        let hash = created["content_hash"].as_str().expect("content_hash").to_string();

        let update_result = WriteDocumentTool
            .execute(
                serde_json::json!({
                    "path": "Runbook",
                    "content": "v2",
                    "content_hash": hash,
                }),
                &ctx,
            )
            .await
            .expect("update");
        let updated: serde_json::Value = serde_json::from_str(&update_result).expect("json");

        assert_eq!(updated["success"], serde_json::json!(true), "{update_result}");
        assert_eq!(updated["action"], serde_json::json!("updated"), "{update_result}");
        assert_eq!(updated["id"], created["id"], "must update the same document, not create a second one");

        let doc = find_document_by_title(&ctx.db, "ws-1", "user-a", "Runbook")
            .await
            .expect("lookup")
            .expect("document exists");
        assert_eq!(doc.content, "v2");
    }

    #[tokio::test]
    async fn write_knowledge_file_conflict_on_stale_hash() {
        // KYO-539 pin: write_knowledge_file DOES pass expected_content_hash
        // through to update_dashboard (knowledge.rs ~694), so a caller
        // supplying a stale hash gets a CAS conflict rather than silently
        // clobbering a concurrent edit. Contrast with
        // `modify_dashboard_cas_is_never_enforced` in tools/dashboard.rs,
        // which pins the opposite for the dashboard-side tool.
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let mut ctx = build_ctx(db);
        ctx.embedding = loaded_embedding();

        WriteDocumentTool
            .execute(
                serde_json::json!({"path": "Runbook", "content": "v1"}),
                &ctx,
            )
            .await
            .expect("create");

        let result = WriteDocumentTool
            .execute(
                serde_json::json!({
                    "path": "Runbook",
                    "content": "v2",
                    "content_hash": "0000000000000000",
                }),
                &ctx,
            )
            .await
            .expect("stale-hash write still returns Ok(..) with a structured failure");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["success"], serde_json::json!(false), "{result}");
        assert!(
            parsed["error"].as_str().unwrap_or_default().contains("Content hash mismatch"),
            "{result}"
        );

        let doc = find_document_by_title(&ctx.db, "ws-1", "user-a", "Runbook")
            .await
            .expect("lookup")
            .expect("document exists");
        assert_eq!(doc.content, "v1", "a rejected CAS write must not apply");
    }

    // ---------------------------------------------------------------------
    // ReadDocumentTool — KYO-537 characterization tests.
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn read_knowledge_file_missing_path_is_bad_request() {
        let ctx = build_ctx(test_pool().await);
        let err = ReadDocumentTool
            .execute(serde_json::json!({}), &ctx)
            .await
            .expect_err("path is required");
        assert!(matches!(err, kyomi_core::Error::BadRequest(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn read_knowledge_file_not_found_returns_error_json_not_err() {
        let ctx = build_ctx(test_pool().await);
        let result = ReadDocumentTool
            .execute(serde_json::json!({"path": "Nope"}), &ctx)
            .await
            .expect("a missing document is a structured result, not an Err");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");
        assert_eq!(
            parsed,
            serde_json::json!({"error": "Document not found: Nope"}),
            "{result}"
        );
    }

    #[tokio::test]
    async fn read_knowledge_file_returns_full_content() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "Runbook", "full content here", DocType::Knowledge, None,
        )
        .await
        .expect("seed knowledge doc");
        let ctx = build_ctx(db);

        let result = ReadDocumentTool
            .execute(serde_json::json!({"path": "Runbook"}), &ctx)
            .await
            .expect("read_knowledge_file execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["id"], serde_json::json!(dashboard_id), "{result}");
        assert_eq!(parsed["name"], serde_json::json!("Runbook"), "{result}");
        assert_eq!(parsed["doc_type"], serde_json::json!("knowledge"), "{result}");
        assert_eq!(parsed["content"], serde_json::json!("full content here"), "{result}");
        assert!(parsed["content_hash"].as_str().is_some(), "{result}");
        assert!(parsed["updated_at"].as_str().is_some(), "{result}");
    }

    // ---------------------------------------------------------------------
    // ListDocumentsTool — KYO-537 characterization tests.
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn list_knowledge_files_returns_all_doc_types_when_unfiltered() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "A Dashboard", "content", DocType::Dashboard, None,
        )
        .await
        .expect("seed dashboard");
        kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "A Knowledge Doc", "content", DocType::Knowledge, None,
        )
        .await
        .expect("seed knowledge doc");
        let ctx = build_ctx(db);

        let result = ListDocumentsTool
            .execute(serde_json::json!({}), &ctx)
            .await
            .expect("list_knowledge_files execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["count"], serde_json::json!(2), "{result}");
        assert_eq!(parsed["files"].as_array().expect("array").len(), 2, "{result}");
    }

    #[tokio::test]
    async fn list_knowledge_files_filters_by_doc_type() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "A Dashboard", "content", DocType::Dashboard, None,
        )
        .await
        .expect("seed dashboard");
        kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "A Knowledge Doc", "content", DocType::Knowledge, None,
        )
        .await
        .expect("seed knowledge doc");
        let ctx = build_ctx(db);

        let result = ListDocumentsTool
            .execute(serde_json::json!({"doc_type": "knowledge"}), &ctx)
            .await
            .expect("list_knowledge_files execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["count"], serde_json::json!(1), "{result}");
        assert_eq!(
            parsed["files"][0]["title"],
            serde_json::json!("A Knowledge Doc"),
            "{result}"
        );
    }

    // ---------------------------------------------------------------------
    // EditDocumentTool — KYO-537 characterization tests.
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn edit_knowledge_file_missing_path_is_bad_request() {
        let ctx = build_ctx(test_pool().await);
        let err = EditDocumentTool
            .execute(
                serde_json::json!({"old_text": "a", "new_text": "b"}),
                &ctx,
            )
            .await
            .expect_err("path is required");
        assert!(matches!(err, kyomi_core::Error::BadRequest(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn edit_knowledge_file_missing_old_text_is_bad_request() {
        let ctx = build_ctx(test_pool().await);
        let err = EditDocumentTool
            .execute(
                serde_json::json!({"path": "Doc", "new_text": "b"}),
                &ctx,
            )
            .await
            .expect_err("old_text is required");
        assert!(matches!(err, kyomi_core::Error::BadRequest(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn edit_knowledge_file_missing_new_text_is_bad_request() {
        let ctx = build_ctx(test_pool().await);
        let err = EditDocumentTool
            .execute(
                serde_json::json!({"path": "Doc", "old_text": "a"}),
                &ctx,
            )
            .await
            .expect_err("new_text is required");
        assert!(matches!(err, kyomi_core::Error::BadRequest(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn edit_knowledge_file_document_not_found_is_err() {
        let ctx = build_ctx(test_pool().await);
        let err = EditDocumentTool
            .execute(
                serde_json::json!({"path": "Nope", "old_text": "a", "new_text": "b"}),
                &ctx,
            )
            .await
            .expect_err("a document that resolves to nothing must be an Err, unlike read_knowledge_file");
        assert!(matches!(err, kyomi_core::Error::NotFound(_)), "got: {err:?}");
    }

    /// KYO-537 named pin (ticket item 1): `edit_knowledge_file`'s zero-match
    /// guard (`knowledge.rs` ~999) returns a structured failure, not an Err.
    #[tokio::test]
    async fn edit_knowledge_file_zero_matches_returns_error_json() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "Runbook", "alpha beta gamma", DocType::Knowledge, None,
        )
        .await
        .expect("seed doc");
        let ctx = build_ctx(db);

        let result = EditDocumentTool
            .execute(
                serde_json::json!({"path": "Runbook", "old_text": "delta", "new_text": "x"}),
                &ctx,
            )
            .await
            .expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["success"], serde_json::json!(false), "{result}");
        assert_eq!(
            parsed["error"],
            serde_json::json!("old_text not found in document content"),
            "{result}"
        );
    }

    /// KYO-537 named pin (ticket item 1): `edit_knowledge_file`'s
    /// multi-match guard (`knowledge.rs` ~1006) must name the exact
    /// occurrence count in its error message — the model needs that number
    /// to know how to disambiguate its next attempt.
    #[tokio::test]
    async fn edit_knowledge_file_multi_match_names_occurrence_count() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "Runbook", "alpha beta alpha gamma alpha", DocType::Knowledge, None,
        )
        .await
        .expect("seed doc");
        let ctx = build_ctx(db);

        let result = EditDocumentTool
            .execute(
                serde_json::json!({"path": "Runbook", "old_text": "alpha", "new_text": "x"}),
                &ctx,
            )
            .await
            .expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["success"], serde_json::json!(false), "{result}");
        assert_eq!(
            parsed["error"],
            serde_json::json!("old_text appears 3 times — must appear exactly once"),
            "{result}"
        );
    }

    #[tokio::test]
    async fn edit_knowledge_file_applies_replacement_happy_path() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "Runbook", "alpha beta gamma", DocType::Knowledge, None,
        )
        .await
        .expect("seed doc");
        let mut ctx = build_ctx(db);
        ctx.embedding = loaded_embedding();

        let result = EditDocumentTool
            .execute(
                serde_json::json!({"path": "Runbook", "old_text": "beta", "new_text": "BETA"}),
                &ctx,
            )
            .await
            .expect("execute");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(parsed["success"], serde_json::json!(true), "{result}");
        assert!(parsed["content_hash"].as_str().is_some(), "{result}");

        let doc = find_document_by_title(&ctx.db, "ws-1", "user-a", "Runbook")
            .await
            .expect("lookup")
            .expect("document exists");
        assert_eq!(doc.content, "alpha BETA gamma");
    }

    // NOTE (ticket item 5, CAS presence): unlike `write_knowledge_file`,
    // there is no black-box test here forcing `edit_knowledge_file`'s CAS
    // mismatch (`Err(Conflict)`) branch to fire. `write_knowledge_file`
    // exposes `content_hash` as a caller-supplied argument, so a stale
    // value can be passed in directly (see
    // `write_knowledge_file_conflict_on_stale_hash` above) — a genuine,
    // deterministic black-box trigger. `edit_knowledge_file` has no such
    // parameter: its `expected_content_hash` (knowledge.rs ~1031) is always
    // derived internally from the document state `resolve_document` just
    // read a few lines earlier in the same `execute()` call, so it can only
    // ever mismatch given a real concurrent writer racing between that read
    // and this call's own `update_dashboard`. Constructing that
    // deterministically (rather than hoping a sleep wins the race, which
    // `docs/standards/testing/nondeterministic-verdict-is-a-failing-test.md`
    // rules out) would require a synchronization checkpoint inside
    // `execute()` that production code doesn't have — adding one is a
    // production-code change, out of scope for a tests-only ticket. The
    // fact that `edit_knowledge_file` *does* pass a real, non-`None` hash
    // (mirroring `write_knowledge_file`'s CAS wiring, unlike
    // `modify_dashboard`'s permanent `None`) is still exercised on every
    // successful edit: `edit_knowledge_file_applies_replacement_happy_path`
    // above asserts the returned `content_hash` changes to match the new
    // content, which only happens by going through this same
    // `update_dashboard` CAS-aware call.

    /// KYO-537 named pin (ticket item 4 — "doc_type reach"): despite its
    /// name, `edit_knowledge_file` is NOT knowledge-specific. It resolves
    /// its target through `find_document_by_title`, which passes
    /// `doc_type_filter: None` to `search_dashboards` (knowledge.rs ~545) —
    /// so it happily resolves and edits a `Dashboard`-doc_type row too.
    ///
    /// NOTE: this is surprising, current, unfixed behavior, not a
    /// recommendation. Stage 2 (KYO-538) is expected to reckon with it
    /// deliberately; this test exists so that reckoning is a visible,
    /// intentional diff against a known baseline rather than an unnoticed
    /// behavior change.
    #[tokio::test]
    async fn edit_knowledge_file_reaches_across_doc_type_into_dashboards() {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        kyomi_auth::dashboard_service::create_dashboard(
            &db, "user-a", "ws-1", "Q3 Revenue Dashboard", "initial content here", DocType::Dashboard, None,
        )
        .await
        .expect("seed a Dashboard-doc_type row, not Knowledge");
        let mut ctx = build_ctx(db);
        ctx.embedding = loaded_embedding();

        let result = EditDocumentTool
            .execute(
                serde_json::json!({
                    "path": "Q3 Revenue Dashboard",
                    "old_text": "initial",
                    "new_text": "updated",
                }),
                &ctx,
            )
            .await
            .expect("edit_knowledge_file executes against a dashboard-doc_type row without error");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("json");

        assert_eq!(
            parsed["success"],
            serde_json::json!(true),
            "edit_knowledge_file must currently succeed against a dashboard doc, \
             per doc_type_filter: None in find_document_by_title: {result}"
        );

        let doc = find_document_by_title(&ctx.db, "ws-1", "user-a", "Q3 Revenue Dashboard")
            .await
            .expect("lookup")
            .expect("document exists");
        assert_eq!(doc.doc_type(), DocType::Dashboard, "doc_type must be unchanged by the edit");
        assert_eq!(doc.content, "updated content here");
    }
}
