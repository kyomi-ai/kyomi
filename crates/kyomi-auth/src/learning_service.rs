// SPDX-License-Identifier: AGPL-3.0-or-later

//! Agent learning service — CRUD, semantic search, hybrid search (BM25 + pgvector).
//!
//! Ports Python's `learning_service.py` (1,457 lines).
//!
//! Key design decisions:
//! - Embedding via `kyomi_embed::EmbeddingService` (shared at startup)
//! - Session dedup via Redis (multi-replica safe)
//! - Hybrid search: BM25 (PostgreSQL `websearch_to_tsquery` / SQLite FTS5) + pgvector cosine
//! - Ranking: Reciprocal Rank Fusion (RRF) with k=60
//! - Tiered filtering: high confidence >= 0.5, moderate >= min_similarity (max 3)
//!
//! ## Migration Notice
//!
//! The pgvector-based retrieval functions in this module have been superseded by
//! `kyomi-knowledge` which provides SQL-based knowledge retrieval with BGE-small-en-v1.5
//! embeddings. The legacy pgvector functions remain as fallback during migration.

use chrono::{DateTime, Utc};
use kyomi_core::embedding_compat::{bytes_to_embedding, embedding_to_bytes};
use kyomi_core::sql_compat;
use kyomi_core::{db_execute, DbPool, Result};
use pgvector::Vector;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

// ─── Constants ────────────────────────────────────────────────────────────────

/// RRF constant (standard value from literature).
const RRF_K: f64 = 60.0;

/// Hard cap on total learnings returned to avoid blowing up LLM context.
const HARD_CAP: usize = 10;

/// High-confidence semantic threshold.
const HIGH_CONFIDENCE_THRESHOLD: f64 = 0.5;

/// Max moderate-confidence results.
const MODERATE_CONFIDENCE_LIMIT: usize = 3;

/// Valid learning types.
pub const VALID_LEARNING_TYPES: &[&str] = &["learning", "metric", "preference"];

/// Valid scopes.
pub const VALID_SCOPES: &[&str] = &["workspace", "user"];

// ─── Response types ───────────────────────────────────────────────────────────

/// A learning record returned from queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningRecord {
    pub learning_id: String,
    pub insight: String,
    pub context: Option<String>,
    pub enabled: bool,
    pub scope: String,
    pub learning_type: String,
    pub times_used: i32,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub learned_from_user: Option<String>,
    pub learned_from_session: Option<String>,
    pub datasource_config_id: Option<String>,
    pub reference_queries: Option<serde_json::Value>,
}

/// A learning record with search scores (hybrid search result).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningSearchResult {
    #[serde(flatten)]
    pub learning: LearningRecord,
    pub similarity: f64,
    pub rrf_score: f64,
    pub semantic_score: f64,
    pub keyword_score: f64,
}

// ─── Save learning ────────────────────────────────────────────────────────────

/// Parameters for [`save_learning`].
pub struct SaveLearningParams<'a> {
    pub db: &'a DbPool,
    pub embedding_svc: &'a kyomi_embed::EmbeddingService,
    pub workspace_id: &'a str,
    pub user_id: &'a str,
    pub session_id: &'a str,
    pub insight: &'a str,
    pub context: Option<&'a str>,
    pub scope: &'a str,
    pub datasource_config_id: Option<&'a str>,
    pub learning_type: &'a str,
    pub reference_queries: Option<&'a serde_json::Value>,
    pub structured_metadata: Option<&'a serde_json::Value>,
}

/// Save a new learning with embedding.
///
/// Returns the generated `learning_id`.
pub async fn save_learning(params: SaveLearningParams<'_>) -> Result<String> {
    let SaveLearningParams {
        db,
        embedding_svc,
        workspace_id,
        user_id,
        session_id,
        insight,
        context,
        scope,
        datasource_config_id,
        learning_type,
        reference_queries,
        structured_metadata,
    } = params;
    // Generate embedding from the insight text
    let embedding_vec = embedding_svc.embed_one(insight)?;
    let embedding_bytes = embedding_to_bytes(&embedding_vec);

    let ref_queries_json = reference_queries.map(|rq| serde_json::to_string(rq).unwrap_or_default());

    let _scope_enum = match scope {
        "workspace" => kyomi_core::LearningScope::Workspace,
        "user" => kyomi_core::LearningScope::User,
        _ => return Err(kyomi_core::Error::Internal(format!("invalid scope: {scope}"))),
    };
    let ref_queries_val: Option<serde_json::Value> = ref_queries_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let structured_meta_val: Option<serde_json::Value> = structured_metadata.cloned();

    let learning_id = Uuid::new_v4().to_string();

    match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let vec = Vector::from(bytes_to_embedding(&embedding_bytes));
            sqlx::query(
                r#"
                INSERT INTO agent_learnings
                    (learning_id, workspace_id, insight, context, embedding, enabled, scope,
                     learned_from_session, learned_from_user, created_at, times_used,
                     datasource_config_id, learning_type, reference_queries, structured_metadata)
                VALUES ($1, $2, $3, $4, $5::vector, TRUE, $6::learning_scope, $7, $8, NOW(), 0, $9, $10, $11, $12)
                "#,
            )
            .bind(&learning_id)
            .bind(workspace_id)
            .bind(insight)
            .bind(context)
            .bind(&vec)
            .bind(scope)
            .bind(session_id)
            .bind(user_id)
            .bind(datasource_config_id)
            .bind(learning_type)
            .bind(&ref_queries_val)
            .bind(&structured_meta_val)
            .execute(pg)
            .await
            .map_err(|e| kyomi_core::Error::Internal(format!("failed to save learning: {e}")))?;
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            sqlx::query(
                r#"
                INSERT INTO agent_learnings
                    (learning_id, workspace_id, insight, context, embedding, enabled, scope,
                     learned_from_session, learned_from_user, created_at, times_used,
                     datasource_config_id, learning_type, reference_queries, structured_metadata)
                VALUES ($1, $2, $3, $4, $5, 1, $6, $7, $8, datetime('now'), 0, $9, $10, $11, $12)
                "#,
            )
            .bind(&learning_id)
            .bind(workspace_id)
            .bind(insight)
            .bind(context)
            .bind(&embedding_bytes)
            .bind(scope)
            .bind(session_id)
            .bind(user_id)
            .bind(datasource_config_id)
            .bind(learning_type)
            .bind(ref_queries_val.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default()))
            .bind(structured_meta_val.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default()))
            .execute(sq)
            .await
            .map_err(|e| kyomi_core::Error::Internal(format!("failed to save learning: {e}")))?;
        }
    }

    tracing::info!(learning_id = %learning_id, "Saved new learning");
    Ok(learning_id)
}

// ─── Get all learnings (admin view) ───────────────────────────────────────────

/// Parameters for [`get_all_learnings`].
pub struct GetAllLearningsParams<'a> {
    pub db: &'a DbPool,
    pub workspace_id: &'a str,
    pub offset: i64,
    pub limit: i64,
    pub search: Option<&'a str>,
    pub scope: Option<&'a str>,
    pub datasource_slug: Option<&'a str>,
    pub enabled_only: bool,
}

/// Get learnings with pagination and filtering.
///
/// Returns `(items, total_count)`.
pub async fn get_all_learnings(
    params: GetAllLearningsParams<'_>,
) -> Result<(Vec<LearningRecord>, i64)> {
    let GetAllLearningsParams {
        db,
        workspace_id,
        offset,
        limit,
        search,
        scope,
        datasource_slug,
        enabled_only,
    } = params;
    let is_pg = db.is_postgres();

    // Build dynamic WHERE clause
    let mut conditions = vec!["workspace_id = $1".to_string()];
    let mut param_idx = 2u32;

    // Track which optional params are bound
    let mut bind_search = false;
    let mut bind_scope = false;
    let mut bind_ds_id: Option<String> = None;

    if let Some(s) = search
        && !s.trim().is_empty()
    {
        if is_pg {
            conditions.push(format!(
                "search_vector @@ websearch_to_tsquery('english', ${param_idx})"
            ));
        } else {
            conditions.push(format!(
                "learning_id IN (SELECT learning_id FROM agent_learnings_fts WHERE agent_learnings_fts MATCH ${param_idx})"
            ));
        }
        param_idx += 1;
        bind_search = true;
    }

    if let Some(s) = scope
        && (s == "workspace" || s == "user")
    {
        if is_pg {
            conditions.push(format!("scope = ${param_idx}::learning_scope"));
        } else {
            conditions.push(format!("scope = ${param_idx}"));
        }
        param_idx += 1;
        bind_scope = true;
    }

    if let Some(ds_slug) = datasource_slug {
        if ds_slug == "global" {
            conditions.push("datasource_config_id IS NULL".to_string());
        } else {
            // Resolve slug to ID
            let ds_id: Option<String> = kyomi_core::db_with_pool!(db, |p| {
                sqlx::query_scalar::<_, String>(
                    "SELECT id FROM datasource_configs WHERE workspace_id = $1 AND slug = $2",
                )
                .bind(workspace_id)
                .bind(ds_slug)
                .fetch_optional(p)
                .await
            })
            .map_err(|e| kyomi_core::Error::Internal(format!("failed to resolve datasource: {e}")))?;

            if let Some(id) = ds_id {
                conditions.push(format!("datasource_config_id = ${param_idx}"));
                param_idx += 1;
                bind_ds_id = Some(id);
            } else {
                // Datasource not found — return empty
                return Ok((Vec::new(), 0));
            }
        }
    }

    if enabled_only {
        conditions.push(format!("enabled = {}", sql_compat::bool_true(is_pg)));
    }

    let where_clause = conditions.join(" AND ");

    let learning_id_expr = sql_compat::cast_to_text(is_pg, "learning_id");
    let scope_expr = sql_compat::cast_to_text(is_pg, "scope");

    // Count query
    let count_sql = format!("SELECT COUNT(*) FROM agent_learnings WHERE {where_clause}");

    let total: i64 = kyomi_core::db_with_pool!(db, |p| {
        let mut q = sqlx::query_scalar::<_, i64>(&count_sql).bind(workspace_id);
        if let Some(s) = search.filter(|_| bind_search) { q = q.bind(s.trim()); }
        if let Some(s) = scope.filter(|_| bind_scope) { q = q.bind(s); }
        if let Some(ref ds_id) = bind_ds_id { q = q.bind(ds_id); }
        q.fetch_one(p).await
    })
    .map_err(|e| kyomi_core::Error::Internal(format!("count query failed: {e}")))?;

    // Data query
    let limit_param = param_idx;
    let offset_param = param_idx + 1;
    let data_sql = format!(
        r#"
        SELECT {learning_id_expr} AS learning_id, insight, context, enabled, times_used, last_used_at,
               created_at, learned_from_user, learned_from_session, {scope_expr} as scope,
               learning_type, datasource_config_id, reference_queries
        FROM agent_learnings
        WHERE {where_clause}
        ORDER BY created_at DESC
        LIMIT ${limit_param} OFFSET ${offset_param}
        "#
    );

    let items: Vec<LearningRecord> = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let mut q = sqlx::query(&data_sql).bind(workspace_id);
            if let Some(s) = search.filter(|_| bind_search) { q = q.bind(s.trim()); }
            if let Some(s) = scope.filter(|_| bind_scope) { q = q.bind(s); }
            if let Some(ref ds_id) = bind_ds_id { q = q.bind(ds_id); }
            q = q.bind(limit).bind(offset);
            let rows = q.fetch_all(pg).await
                .map_err(|e| kyomi_core::Error::Internal(format!("data query failed: {e}")))?;
            rows.iter().map(learning_record_from_pg_row).collect()
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let mut q = sqlx::query(&data_sql).bind(workspace_id);
            if let Some(s) = search.filter(|_| bind_search) { q = q.bind(s.trim()); }
            if let Some(s) = scope.filter(|_| bind_scope) { q = q.bind(s); }
            if let Some(ref ds_id) = bind_ds_id { q = q.bind(ds_id); }
            q = q.bind(limit).bind(offset);
            let rows = q.fetch_all(sq).await
                .map_err(|e| kyomi_core::Error::Internal(format!("data query failed: {e}")))?;
            rows.iter().map(learning_record_from_sq_row).collect()
        }
    };

    Ok((items, total))
}

/// Extract a LearningRecord from a Postgres row.
fn learning_record_from_pg_row(row: &sqlx::postgres::PgRow) -> LearningRecord {
    LearningRecord {
        learning_id: row.get("learning_id"),
        insight: row.get("insight"),
        context: row.get("context"),
        enabled: row.get("enabled"),
        times_used: row.get("times_used"),
        last_used_at: row.get("last_used_at"),
        created_at: row.get("created_at"),
        learned_from_user: row.get("learned_from_user"),
        learned_from_session: row.get("learned_from_session"),
        scope: row.get("scope"),
        learning_type: row.get("learning_type"),
        datasource_config_id: row.get("datasource_config_id"),
        reference_queries: row.get("reference_queries"),
    }
}

/// Extract a LearningRecord from a SQLite row.
fn learning_record_from_sq_row(row: &sqlx::sqlite::SqliteRow) -> LearningRecord {
    LearningRecord {
        learning_id: row.get("learning_id"),
        insight: row.get("insight"),
        context: row.get("context"),
        enabled: row.get("enabled"),
        times_used: row.get("times_used"),
        last_used_at: row.get("last_used_at"),
        created_at: row.get("created_at"),
        learned_from_user: row.get("learned_from_user"),
        learned_from_session: row.get("learned_from_session"),
        scope: row.get("scope"),
        learning_type: row.get("learning_type"),
        datasource_config_id: row.get("datasource_config_id"),
        reference_queries: row.get("reference_queries"),
    }
}

// ─── Update learning ──────────────────────────────────────────────────────────

/// Partial update of a learning.
///
/// If `insight` is changed, the embedding is regenerated automatically.
pub async fn update_learning(
    db: &DbPool,
    embedding_svc: &kyomi_embed::EmbeddingService,
    learning_id: &str,
    workspace_id: &str,
    updates: &LearningUpdates,
) -> Result<()> {
    learning_id.parse::<Uuid>()
        .map_err(|e| kyomi_core::Error::Internal(format!("invalid learning_id UUID: {e}")))?;

    // If insight changed, regenerate embedding
    if let Some(ref insight) = updates.insight {
        let embedding_vec = embedding_svc.embed_one(insight)?;
        let embedding_bytes = embedding_to_bytes(&embedding_vec);

        match db {
            kyomi_core::db::DbPool::Postgres(pg) => {
                let vec = Vector::from(bytes_to_embedding(&embedding_bytes));
                sqlx::query(
                    "UPDATE agent_learnings SET insight = $1, embedding = $2::vector WHERE learning_id = $3 AND workspace_id = $4",
                )
                .bind(insight)
                .bind(&vec)
                .bind(learning_id)
                .bind(workspace_id)
                .execute(pg)
                .await
                .map(|_| ())
            }
            kyomi_core::db::DbPool::Sqlite(sq) => {
                sqlx::query(
                    "UPDATE agent_learnings SET insight = $1, embedding = $2 WHERE learning_id = $3 AND workspace_id = $4",
                )
                .bind(insight)
                .bind(&embedding_bytes)
                .bind(learning_id)
                .bind(workspace_id)
                .execute(sq)
                .await
                .map(|_| ())
            }
        }
        .map_err(|e| kyomi_core::Error::Internal(format!("update insight failed: {e}")))?;
    }

    if let Some(ref context) = updates.context {
        db_execute!(
            db,
            "UPDATE agent_learnings SET context = $1 WHERE learning_id = $2 AND workspace_id = $3",
            context,
            learning_id,
            workspace_id
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("update context failed: {e}")))?;
    }

    if let Some(enabled) = updates.enabled {
        db_execute!(
            db,
            "UPDATE agent_learnings SET enabled = $1 WHERE learning_id = $2 AND workspace_id = $3",
            &enabled,
            learning_id,
            workspace_id
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("update enabled failed: {e}")))?;
    }

    if let Some(ref datasource_config_id) = updates.datasource_config_id {
        // Empty string means clear (set to NULL)
        let ds_id: Option<&str> = if datasource_config_id.is_empty() {
            None
        } else {
            Some(datasource_config_id.as_str())
        };
        db_execute!(
            db,
            "UPDATE agent_learnings SET datasource_config_id = $1 WHERE learning_id = $2 AND workspace_id = $3",
            &ds_id,
            learning_id,
            workspace_id
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("update datasource failed: {e}")))?;
    }

    if let Some(ref learning_type) = updates.learning_type {
        db_execute!(
            db,
            "UPDATE agent_learnings SET learning_type = $1 WHERE learning_id = $2 AND workspace_id = $3",
            learning_type,
            learning_id,
            workspace_id
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("update learning_type failed: {e}")))?;
    }

    if let Some(ref reference_queries) = updates.reference_queries {
        let rq_val: Option<serde_json::Value> = if reference_queries.is_null()
            || reference_queries
                .as_array()
                .is_some_and(|a| a.is_empty())
        {
            None
        } else {
            Some(reference_queries.clone())
        };

        match db {
            kyomi_core::db::DbPool::Postgres(pg) => {
                sqlx::query(
                    "UPDATE agent_learnings SET reference_queries = $1 WHERE learning_id = $2 AND workspace_id = $3",
                )
                .bind(&rq_val)
                .bind(learning_id)
                .bind(workspace_id)
                .execute(pg)
                .await
                .map(|_| ())
            }
            kyomi_core::db::DbPool::Sqlite(sq) => {
                let rq_str = rq_val.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
                sqlx::query(
                    "UPDATE agent_learnings SET reference_queries = $1 WHERE learning_id = $2 AND workspace_id = $3",
                )
                .bind(&rq_str)
                .bind(learning_id)
                .bind(workspace_id)
                .execute(sq)
                .await
                .map(|_| ())
            }
        }
        .map_err(|e| kyomi_core::Error::Internal(format!("update reference_queries failed: {e}")))?;
    }

    tracing::info!(learning_id = %learning_id, "Updated learning");
    Ok(())
}

/// Fields that can be updated on a learning.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LearningUpdates {
    pub insight: Option<String>,
    pub context: Option<String>,
    pub enabled: Option<bool>,
    pub datasource_config_id: Option<String>,
    pub learning_type: Option<String>,
    pub reference_queries: Option<serde_json::Value>,
    pub structured_metadata: Option<serde_json::Value>,
}

// ─── Delete learning ──────────────────────────────────────────────────────────

/// Delete a learning permanently.
pub async fn delete_learning(
    db: &DbPool,
    learning_id: &str,
    workspace_id: &str,
) -> Result<()> {
    learning_id.parse::<Uuid>()
        .map_err(|e| kyomi_core::Error::Internal(format!("invalid learning_id UUID: {e}")))?;

    db_execute!(
        db,
        "DELETE FROM agent_learnings WHERE learning_id = $1 AND workspace_id = $2",
        learning_id,
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("delete learning failed: {e}")))?;

    tracing::info!(learning_id = %learning_id, "Deleted learning");
    Ok(())
}

// ─── Supersede learning ───────────────────────────────────────────────────────

/// Mark a learning as superseded by a newer learning.
pub async fn supersede_learning(
    db: &DbPool,
    learning_id: &str,
    superseded_by: &str,
    workspace_id: &str,
) -> Result<()> {
    learning_id.parse::<Uuid>()
        .map_err(|e| kyomi_core::Error::Internal(format!("invalid learning_id UUID: {e}")))?;
    superseded_by.parse::<Uuid>()
        .map_err(|e| kyomi_core::Error::Internal(format!("invalid superseded_by UUID: {e}")))?;

    let is_pg = db.is_postgres();
    let false_val = sql_compat::bool_false(is_pg);
    let sql = format!(
        r#"
        UPDATE agent_learnings
        SET superseded_by = $1, enabled = {false_val}
        WHERE learning_id = $2 AND workspace_id = $3 AND superseded_by IS NULL
        "#
    );

    db_execute!(db, &sql, superseded_by, learning_id, workspace_id)
        .map_err(|e| kyomi_core::Error::Internal(format!("supersede learning failed: {e}")))?;

    tracing::info!(
        learning_id = %learning_id,
        superseded_by = %superseded_by,
        "Superseded learning"
    );
    Ok(())
}

// ─── Increment usage ──────────────────────────────────────────────────────────

/// Increment usage counter when a learning is retrieved for context injection.
pub async fn increment_usage(db: &DbPool, learning_id: &str) -> Result<()> {
    learning_id.parse::<Uuid>()
        .map_err(|e| kyomi_core::Error::Internal(format!("invalid learning_id UUID: {e}")))?;

    let is_pg = db.is_postgres();
    let now_expr = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE agent_learnings SET times_used = times_used + 1, last_used_at = {now_expr} WHERE learning_id = $1"
    );

    db_execute!(db, &sql, learning_id)
        .map_err(|e| kyomi_core::Error::Internal(format!("increment usage failed: {e}")))?;
    Ok(())
}

// ─── Hybrid search (BM25 + semantic, RRF fusion) ─────────────────────────────

/// Parameters for [`get_relevant_learnings_hybrid`].
pub struct GetRelevantLearningsParams<'a> {
    pub db: &'a DbPool,
    pub embedding_svc: &'a kyomi_embed::EmbeddingService,
    pub workspace_id: &'a str,
    pub query: &'a str,
    pub user_id: Option<&'a str>,
    pub limit: usize,
    pub min_similarity: f64,
    pub semantic_weight: f64,
    pub keyword_weight: f64,
}

/// Legacy pgvector-based learning retrieval (superseded by kyomi-knowledge).
///
/// Get relevant learnings using hybrid search (BM25 + pgvector cosine).
///
/// Combines full-text search (keyword matching) with semantic vector search
/// using Reciprocal Rank Fusion (RRF, k=60) for better retrieval accuracy.
///
/// Tiered filtering:
/// - High confidence: semantic >= 0.5 or keyword >= 0.5 (up to HARD_CAP)
/// - Moderate confidence: semantic >= min_similarity (up to 3)
pub async fn get_relevant_learnings_hybrid(
    params: GetRelevantLearningsParams<'_>,
) -> Result<Vec<LearningSearchResult>> {
    let GetRelevantLearningsParams {
        db,
        embedding_svc,
        workspace_id,
        query,
        user_id,
        limit,
        min_similarity,
        semantic_weight,
        keyword_weight,
    } = params;
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    // Generate query embedding (with BGE query prefix for asymmetric retrieval)
    let query_embedding_vec = embedding_svc.embed_query(query)?;

    // Build scope filter (Postgres uses bare column names, SQLite FTS queries
    // join agent_learnings as `al` so need the prefix).
    let (scope_filter, scope_filter_sq) = if user_id.is_some() {
        (
            "(scope = 'workspace' OR (scope = 'user' AND learned_from_user = $3))",
            "(al.scope = 'workspace' OR (al.scope = 'user' AND al.learned_from_user = $3))",
        )
    } else {
        (
            "scope = 'workspace'",
            "al.scope = 'workspace'",
        )
    };

    let fetch_limit = (limit * 2) as i64;

    // ─── Semantic search ─────────────────────────────────────────────────
    let limit_param = if user_id.is_some() { "$4" } else { "$3" };

    let semantic_rows = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let query_embedding = Vector::from(query_embedding_vec.clone());
            let semantic_sql = format!(
                r#"
                SELECT learning_id::text, insight, context, times_used, created_at, scope::text as scope,
                       learned_from_user, learned_from_session, datasource_config_id,
                       reference_queries, learning_type,
                       (1 - (embedding <=> $1::vector))::float8 AS score
                FROM agent_learnings
                WHERE workspace_id = $2
                  AND enabled = TRUE
                  AND superseded_by IS NULL
                  AND {scope_filter}
                ORDER BY embedding <=> $1::vector
                LIMIT {limit_param}
                "#
            );

            let mut q = sqlx::query(&semantic_sql)
                .bind(&query_embedding)
                .bind(workspace_id);
            if let Some(uid) = user_id {
                q = q.bind(uid);
            }
            q = q.bind(fetch_limit);
            q.fetch_all(pg).await
                .map(|rows| extract_scored_rows_pg(&rows))
        }
        kyomi_core::db::DbPool::Sqlite(_sq) => {
            // SQLite: skip vector search (return empty — VectorSearch trait handles this at higher level)
            Ok(Vec::new())
        }
    }
    .map_err(|e| kyomi_core::Error::Internal(format!("semantic search failed: {e}")))?;

    // ─── Keyword search (BM25) ──────────────────────────────────────────
    let fts_query = query.split_whitespace().collect::<Vec<_>>().join(" OR ");

    let keyword_rows = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let kw_sql = if user_id.is_some() {
                format!(
                    r#"
                    SELECT learning_id::text, insight, context, times_used, created_at, scope::text as scope,
                           learned_from_user, learned_from_session, datasource_config_id,
                           reference_queries, learning_type,
                           ts_rank(search_vector, websearch_to_tsquery('english', $1))::float8 AS score
                    FROM agent_learnings
                    WHERE workspace_id = $2
                      AND enabled = TRUE
                      AND {scope_filter}
                      AND search_vector @@ websearch_to_tsquery('english', $1)
                    ORDER BY ts_rank(search_vector, websearch_to_tsquery('english', $1)) DESC
                    LIMIT $4
                    "#
                )
            } else {
                r#"
                SELECT learning_id::text, insight, context, times_used, created_at, scope::text as scope,
                       learned_from_user, learned_from_session, datasource_config_id,
                       reference_queries, learning_type,
                       ts_rank(search_vector, websearch_to_tsquery('english', $1))::float8 AS score
                FROM agent_learnings
                WHERE workspace_id = $2
                  AND enabled = TRUE
                  AND scope = 'workspace'
                  AND search_vector @@ websearch_to_tsquery('english', $1)
                ORDER BY ts_rank(search_vector, websearch_to_tsquery('english', $1)) DESC
                LIMIT $3
                "#.to_string()
            };
            let mut q = sqlx::query(&kw_sql)
                .bind(&fts_query)
                .bind(workspace_id);
            if let Some(uid) = user_id {
                q = q.bind(uid);
            }
            q = q.bind(fetch_limit);
            q.fetch_all(pg).await
                .map(|rows| extract_scored_rows_pg(&rows))
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            // SQLite FTS5 search
            let kw_sql = if user_id.is_some() {
                format!(
                    r#"
                    SELECT al.learning_id, al.insight, al.context, al.times_used, al.created_at, al.scope,
                           al.learned_from_user, al.learned_from_session, al.datasource_config_id,
                           al.reference_queries, al.learning_type,
                           bm25(agent_learnings_fts) AS score
                    FROM agent_learnings_fts
                    JOIN agent_learnings al ON al.learning_id = agent_learnings_fts.learning_id
                    WHERE agent_learnings_fts MATCH $1
                      AND al.workspace_id = $2
                      AND al.enabled = 1
                      AND {scope_filter_sq}
                    ORDER BY score
                    LIMIT $4
                    "#
                )
            } else {
                r#"
                SELECT al.learning_id, al.insight, al.context, al.times_used, al.created_at, al.scope,
                       al.learned_from_user, al.learned_from_session, al.datasource_config_id,
                       al.reference_queries, al.learning_type,
                       bm25(agent_learnings_fts) AS score
                FROM agent_learnings_fts
                JOIN agent_learnings al ON al.learning_id = agent_learnings_fts.learning_id
                WHERE agent_learnings_fts MATCH $1
                  AND al.workspace_id = $2
                  AND al.enabled = 1
                  AND al.scope = 'workspace'
                ORDER BY score
                LIMIT $3
                "#.to_string()
            };
            let mut q = sqlx::query(&kw_sql)
                .bind(&fts_query)
                .bind(workspace_id);
            if let Some(uid) = user_id {
                q = q.bind(uid);
            }
            q = q.bind(fetch_limit);
            q.fetch_all(sq).await
                .map(|rows| extract_scored_rows_sq(&rows))
        }
    }
    .map_err(|e| kyomi_core::Error::Internal(format!("keyword search failed: {e}")))?;

    // 3. RRF fusion
    let mut combined_scores: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();
    let mut learning_data: std::collections::HashMap<String, LearningSearchResult> =
        std::collections::HashMap::new();

    // Semantic results
    for (rank, (lid, record_fields, score)) in semantic_rows.iter().enumerate() {
        let rrf_score = semantic_weight / (RRF_K + (rank + 1) as f64);
        *combined_scores.entry(lid.clone()).or_default() += rrf_score;

        learning_data.entry(lid.clone()).or_insert_with(|| {
            LearningSearchResult {
                learning: record_fields.clone(),
                similarity: 0.0,
                rrf_score: 0.0,
                semantic_score: *score,
                keyword_score: 0.0,
            }
        });
    }

    // Keyword results
    for (rank, (lid, record_fields, score)) in keyword_rows.iter().enumerate() {
        let rrf_score = keyword_weight / (RRF_K + (rank + 1) as f64);
        *combined_scores.entry(lid.clone()).or_default() += rrf_score;

        learning_data.entry(lid.clone()).or_insert_with(|| {
            LearningSearchResult {
                learning: record_fields.clone(),
                similarity: 0.0,
                rrf_score: 0.0,
                semantic_score: 0.0,
                keyword_score: *score,
            }
        });
        // Update keyword score if entry already existed from semantic search
        if let Some(entry) = learning_data.get_mut(lid) {
            entry.keyword_score = *score;
        }
    }

    // Sort by combined RRF score
    let mut sorted: Vec<(String, f64)> = combined_scores.into_iter().collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Tiered filtering
    let mut high_confidence = Vec::new();
    let mut moderate_confidence = Vec::new();

    for (lid, rrf_score) in &sorted {
        if let Some(entry) = learning_data.get_mut(lid) {
            entry.rrf_score = *rrf_score;
            entry.similarity = entry.semantic_score;

            if entry.semantic_score >= HIGH_CONFIDENCE_THRESHOLD || entry.keyword_score >= 0.5 {
                if high_confidence.len() < HARD_CAP {
                    high_confidence.push(entry.clone());
                }
            } else if entry.semantic_score >= min_similarity
                && moderate_confidence.len() < MODERATE_CONFIDENCE_LIMIT
            {
                moderate_confidence.push(entry.clone());
            }
        }
    }

    // Combine: high confidence + moderate (up to hard cap)
    let remaining_slots = HARD_CAP.saturating_sub(high_confidence.len());
    let mut results = high_confidence;
    results.extend(moderate_confidence.into_iter().take(remaining_slots));
    results.truncate(limit);

    tracing::info!(
        count = results.len(),
        semantic_count = semantic_rows.len(),
        keyword_count = keyword_rows.len(),
        "Hybrid search completed"
    );

    Ok(results)
}

/// Extract scored rows from Postgres query results.
fn extract_scored_rows_pg(rows: &[sqlx::postgres::PgRow]) -> Vec<(String, LearningRecord, f64)> {
    rows.iter()
        .map(|row| {
            let lid: String = row.get("learning_id");
            let score: f64 = row.get("score");
            let record = LearningRecord {
                learning_id: row.get("learning_id"),
                insight: row.get("insight"),
                context: row.get("context"),
                enabled: true,
                scope: row.get("scope"),
                learning_type: row.get::<Option<String>, _>("learning_type").unwrap_or_else(|| "navigation".into()),
                times_used: row.get("times_used"),
                last_used_at: None,
                created_at: row.get("created_at"),
                learned_from_user: row.get("learned_from_user"),
                learned_from_session: row.get("learned_from_session"),
                datasource_config_id: row.get("datasource_config_id"),
                reference_queries: row.get("reference_queries"),
            };
            (lid, record, score)
        })
        .collect()
}

/// Extract scored rows from SQLite query results.
fn extract_scored_rows_sq(rows: &[sqlx::sqlite::SqliteRow]) -> Vec<(String, LearningRecord, f64)> {
    rows.iter()
        .map(|row| {
            let lid: String = row.get("learning_id");
            let score: f64 = row.get("score");
            let record = LearningRecord {
                learning_id: row.get("learning_id"),
                insight: row.get("insight"),
                context: row.get("context"),
                enabled: true,
                scope: row.get("scope"),
                learning_type: row.get::<Option<String>, _>("learning_type").unwrap_or_else(|| "navigation".into()),
                times_used: row.get("times_used"),
                last_used_at: None,
                created_at: row.get("created_at"),
                learned_from_user: row.get("learned_from_user"),
                learned_from_session: row.get("learned_from_session"),
                datasource_config_id: row.get("datasource_config_id"),
                reference_queries: row.get("reference_queries"),
            };
            (lid, record, score)
        })
        .collect()
}

// ─── Search learnings (for agent tool / admin) ────────────────────────────────

/// Parameters for [`search_learnings`].
pub struct SearchLearningsParams<'a> {
    pub db: &'a DbPool,
    pub embedding_svc: &'a kyomi_embed::EmbeddingService,
    pub workspace_id: &'a str,
    pub query: &'a str,
    pub user_id: Option<&'a str>,
    pub datasource_config_id: Option<&'a str>,
    pub include_disabled: bool,
    pub limit: usize,
}

/// Search learnings using hybrid search.
///
/// Unlike `get_relevant_learnings_hybrid` (for auto-injection), this method
/// can include disabled learnings and returns more details.
pub async fn search_learnings(
    params: SearchLearningsParams<'_>,
) -> Result<Vec<LearningSearchResult>> {
    let SearchLearningsParams {
        db,
        embedding_svc,
        workspace_id,
        query,
        user_id,
        datasource_config_id,
        include_disabled,
        limit,
    } = params;
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    let query_embedding_vec = embedding_svc.embed_query(query)?;
    let fts_query = query.split_whitespace().collect::<Vec<_>>().join(" OR ");
    let fetch_limit = (limit * 2) as i64;

    // Build filters
    let scope_filter = if user_id.is_some() {
        "AND (scope = 'workspace' OR (scope = 'user' AND learned_from_user = :user_id))"
    } else {
        "AND scope = 'workspace'"
    };

    let ds_filter = if datasource_config_id.is_some() {
        "AND (datasource_config_id IS NULL OR datasource_config_id = :ds_id)"
    } else {
        ""
    };

    let enabled_filter = if include_disabled { "" } else { "AND enabled = TRUE" };

    // ─── Semantic search ─────────────────────────────────────────────────
    let semantic_rows = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let query_embedding = Vector::from(query_embedding_vec.clone());
            let sem_sql = format!(
                r#"
                SELECT learning_id::text, insight, context, times_used, created_at, scope::text as scope,
                       learned_from_user, datasource_config_id, enabled, learning_type,
                       reference_queries,
                       (1 - (embedding <=> $1::vector))::float8 AS score
                FROM agent_learnings
                WHERE workspace_id = $2
                  AND superseded_by IS NULL
                  {scope_filter} {ds_filter} {enabled_filter}
                ORDER BY embedding <=> $1::vector
                LIMIT {fetch_limit}
                "#
            );

            // Replace named placeholders with positional
            let sem_sql_pos = sem_sql
                .replace(":user_id", "$3")
                .replace(":ds_id", if user_id.is_some() { "$4" } else { "$3" });

            let mut q = sqlx::query(&sem_sql_pos)
                .bind(&query_embedding)
                .bind(workspace_id);
            if let Some(uid) = user_id { q = q.bind(uid); }
            if let Some(ds_id) = datasource_config_id { q = q.bind(ds_id); }

            q.fetch_all(pg).await
                .map(|rows| extract_search_rows_pg(&rows))
        }
        kyomi_core::db::DbPool::Sqlite(_sq) => {
            // SQLite: skip vector search
            Ok(Vec::new())
        }
    }
    .map_err(|e| kyomi_core::Error::Internal(format!("search semantic failed: {e}")))?;

    // ─── Keyword search ──────────────────────────────────────────────────
    let enabled_filter_sq = if include_disabled { "" } else { "AND enabled = 1" };

    let keyword_rows = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let kw_sql = format!(
                r#"
                SELECT learning_id::text, insight, context, times_used, created_at, scope::text as scope,
                       learned_from_user, datasource_config_id, enabled, learning_type,
                       reference_queries,
                       ts_rank(search_vector, websearch_to_tsquery('english', $1))::float8 AS score
                FROM agent_learnings
                WHERE workspace_id = $2
                  AND superseded_by IS NULL
                  {scope_filter} {ds_filter} {enabled_filter}
                  AND search_vector @@ websearch_to_tsquery('english', $1)
                ORDER BY ts_rank(search_vector, websearch_to_tsquery('english', $1)) DESC
                LIMIT {fetch_limit}
                "#
            );

            let kw_sql_pos = kw_sql
                .replace(":user_id", "$3")
                .replace(":ds_id", if user_id.is_some() { "$4" } else { "$3" });

            let mut q = sqlx::query(&kw_sql_pos)
                .bind(&fts_query)
                .bind(workspace_id);
            if let Some(uid) = user_id { q = q.bind(uid); }
            if let Some(ds_id) = datasource_config_id { q = q.bind(ds_id); }

            q.fetch_all(pg).await
                .map(|rows| extract_search_rows_pg(&rows))
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let scope_filter_sq = if user_id.is_some() {
                "AND (al.scope = 'workspace' OR (al.scope = 'user' AND al.learned_from_user = :user_id))"
            } else {
                "AND al.scope = 'workspace'"
            };

            let ds_filter_sq = if datasource_config_id.is_some() {
                "AND (al.datasource_config_id IS NULL OR al.datasource_config_id = :ds_id)"
            } else {
                ""
            };

            let kw_sql = format!(
                r#"
                SELECT al.learning_id, al.insight, al.context, al.times_used, al.created_at, al.scope,
                       al.learned_from_user, al.datasource_config_id, al.enabled, al.learning_type,
                       al.reference_queries,
                       bm25(agent_learnings_fts) AS score
                FROM agent_learnings_fts
                JOIN agent_learnings al ON al.learning_id = agent_learnings_fts.learning_id
                WHERE agent_learnings_fts MATCH $1
                  AND al.workspace_id = $2
                  AND al.superseded_by IS NULL
                  {scope_filter_sq} {ds_filter_sq} {enabled_filter_sq}
                ORDER BY score
                LIMIT {fetch_limit}
                "#
            );

            let kw_sql_pos = kw_sql
                .replace(":user_id", "$3")
                .replace(":ds_id", if user_id.is_some() { "$4" } else { "$3" });

            let mut q = sqlx::query(&kw_sql_pos)
                .bind(&fts_query)
                .bind(workspace_id);
            if let Some(uid) = user_id { q = q.bind(uid); }
            if let Some(ds_id) = datasource_config_id { q = q.bind(ds_id); }

            q.fetch_all(sq).await
                .map(|rows| extract_search_rows_sq(&rows))
        }
    }
    .map_err(|e| kyomi_core::Error::Internal(format!("search keyword failed: {e}")))?;

    // RRF fusion
    let mut combined_scores: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();
    let mut learning_data: std::collections::HashMap<String, LearningSearchResult> =
        std::collections::HashMap::new();

    for (rank, (lid, record, score)) in semantic_rows.iter().enumerate() {
        *combined_scores.entry(lid.clone()).or_default() += 0.5 / (RRF_K + (rank + 1) as f64);

        learning_data.entry(lid.clone()).or_insert_with(|| LearningSearchResult {
            learning: record.clone(),
            similarity: 0.0,
            rrf_score: 0.0,
            semantic_score: *score,
            keyword_score: 0.0,
        });
    }

    for (rank, (lid, record, kw_score)) in keyword_rows.iter().enumerate() {
        *combined_scores.entry(lid.clone()).or_default() += 0.5 / (RRF_K + (rank + 1) as f64);

        learning_data
            .entry(lid.clone())
            .and_modify(|e| e.keyword_score = *kw_score)
            .or_insert_with(|| LearningSearchResult {
                learning: record.clone(),
                similarity: 0.0,
                rrf_score: 0.0,
                semantic_score: 0.0,
                keyword_score: *kw_score,
            });
    }

    // Sort and return top N
    let mut sorted: Vec<(String, f64)> = combined_scores.into_iter().collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let results: Vec<LearningSearchResult> = sorted
        .into_iter()
        .take(limit)
        .filter_map(|(lid, rrf)| {
            learning_data.remove(&lid).map(|mut entry| {
                entry.rrf_score = rrf;
                entry.similarity = rrf;
                entry
            })
        })
        .collect();

    Ok(results)
}

/// Extract search rows (includes `enabled` field) from Postgres.
fn extract_search_rows_pg(rows: &[sqlx::postgres::PgRow]) -> Vec<(String, LearningRecord, f64)> {
    rows.iter()
        .map(|row| {
            let lid: String = row.get("learning_id");
            let score: f64 = row.get("score");
            let record = LearningRecord {
                learning_id: row.get("learning_id"),
                insight: row.get("insight"),
                context: row.get("context"),
                enabled: row.get("enabled"),
                scope: row.get("scope"),
                learning_type: row.get::<Option<String>, _>("learning_type").unwrap_or_else(|| "navigation".into()),
                times_used: row.get("times_used"),
                last_used_at: None,
                created_at: row.get("created_at"),
                learned_from_user: row.get("learned_from_user"),
                learned_from_session: None,
                datasource_config_id: row.get("datasource_config_id"),
                reference_queries: row.get("reference_queries"),
            };
            (lid, record, score)
        })
        .collect()
}

/// Extract search rows (includes `enabled` field) from SQLite.
fn extract_search_rows_sq(rows: &[sqlx::sqlite::SqliteRow]) -> Vec<(String, LearningRecord, f64)> {
    rows.iter()
        .map(|row| {
            let lid: String = row.get("learning_id");
            let score: f64 = row.get("score");
            let record = LearningRecord {
                learning_id: row.get("learning_id"),
                insight: row.get("insight"),
                context: row.get("context"),
                enabled: row.get("enabled"),
                scope: row.get("scope"),
                learning_type: row.get::<Option<String>, _>("learning_type").unwrap_or_else(|| "navigation".into()),
                times_used: row.get("times_used"),
                last_used_at: None,
                created_at: row.get("created_at"),
                learned_from_user: row.get("learned_from_user"),
                learned_from_session: None,
                datasource_config_id: row.get("datasource_config_id"),
                reference_queries: row.get("reference_queries"),
            };
            (lid, record, score)
        })
        .collect()
}

// ─── Format learning for LLM context injection ───────────────────────────────

/// Format a learning record (with optional reference queries) for LLM context.
pub fn format_learning_with_queries(
    learning: &LearningRecord,
    ds_id_to_slug: Option<&std::collections::HashMap<String, String>>,
    include_id: bool,
) -> String {
    let mut parts = Vec::new();

    if include_id {
        parts.push(format!("[{}]", learning.learning_id));
    }

    if let (Some(map), Some(ds_id)) = (ds_id_to_slug, &learning.datasource_config_id)
        && let Some(slug) = map.get(ds_id)
    {
        parts.push(format!("(datasource: {slug})"));
    }

    parts.push(learning.insight.clone());
    let mut line = format!("- {}", parts.join(" "));

    // Append reference queries if present
    if let Some(ref rqs) = learning.reference_queries
        && let Some(arr) = rqs.as_array()
    {
        let mut query_lines = Vec::new();
        for rq in arr {
            let comment = rq.get("comment").and_then(|v| v.as_str()).unwrap_or("Reference query");
            let sql = rq.get("sql").and_then(|v| v.as_str()).unwrap_or("");
            let ds = rq.get("datasource").and_then(|v| v.as_str()).unwrap_or("");
            let mut header = format!("  Reference query ({comment})");
            if !ds.is_empty() {
                header.push_str(&format!(" — datasource: {ds}"));
            }
            query_lines.push(format!("{header}:"));
            query_lines.push(format!("  ```sql\n  {sql}\n  ```"));
        }
        if !query_lines.is_empty() {
            line.push('\n');
            line.push_str(&query_lines.join("\n"));
        }
    }

    line
}

// ─── Session dedup via Redis ──────────────────────────────────────────────────
//
// DEPRECATED: These session dedup functions use `learning_dedup:{session_id}` Redis keys.
// They are superseded by `ConversationContext` in kyomi-knowledge, which tracks
// injected entries via `kg:ctx:{session_id}` Redis keys and provides richer
// per-entity-kind tracking (tables, columns, learnings, metrics).
// These functions are still called by the system prompt learning injection path
// and can be removed once that path is migrated to kyomi-knowledge.

/// DEPRECATED: Superseded by `ConversationContext::all_injected()` in kyomi-knowledge.
///
/// Check if a learning was already injected in this session.
pub async fn is_learning_seen_in_session(
    kv: &kyomi_core::KVPool,
    session_id: &str,
    learning_id: &str,
) -> bool {
    let key = format!("learning_dedup:{session_id}");
    let members = kv.smembers(&key).await.unwrap_or_default();
    members.contains(&learning_id.to_string())
}

/// DEPRECATED: Superseded by `ConversationContext::record_injection()` in kyomi-knowledge.
///
/// Mark a learning as seen in this session (with TTL matching session lifetime).
pub async fn mark_learning_seen_in_session(
    kv: &kyomi_core::KVPool,
    session_id: &str,
    learning_id: &str,
    ttl_secs: u64,
) -> Result<()> {
    let key = format!("learning_dedup:{session_id}");

    kv.sadd(&key, learning_id)
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("KVStore SADD failed: {e}")))?;

    kv.expire(&key, ttl_secs)
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("KVStore EXPIRE failed: {e}")))?;

    Ok(())
}

/// Get a learning record by ID (for permission checks).
pub async fn get_learning_by_id(
    db: &DbPool,
    learning_id: &str,
    workspace_id: &str,
) -> Result<Option<LearningRecord>> {
    learning_id.parse::<Uuid>()
        .map_err(|e| kyomi_core::Error::Internal(format!("invalid learning_id UUID: {e}")))?;

    let is_pg = db.is_postgres();
    let learning_id_expr = sql_compat::cast_to_text(is_pg, "learning_id");
    let scope_expr = sql_compat::cast_to_text(is_pg, "scope");

    let sql = format!(
        r#"
        SELECT {learning_id_expr} AS learning_id, insight, context, enabled, times_used, last_used_at,
               created_at, learned_from_user, learned_from_session, {scope_expr} AS scope,
               learning_type, datasource_config_id, reference_queries
        FROM agent_learnings
        WHERE learning_id = $1 AND workspace_id = $2
        "#
    );

    let row: Option<LearningRecord> = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            sqlx::query(&sql)
                .bind(learning_id)
                .bind(workspace_id)
                .fetch_optional(pg)
                .await
                .map(|opt| opt.map(|r| {
                    LearningRecord {
                        learning_id: r.get("learning_id"),
                        insight: r.get("insight"),
                        context: r.get("context"),
                        enabled: r.get::<Option<bool>, _>("enabled").unwrap_or(true),
                        times_used: r.get::<Option<i32>, _>("times_used").unwrap_or(0),
                        last_used_at: r.get("last_used_at"),
                        created_at: r.get::<Option<DateTime<Utc>>, _>("created_at").unwrap_or_else(Utc::now),
                        learned_from_user: r.get("learned_from_user"),
                        learned_from_session: r.get("learned_from_session"),
                        scope: r.get("scope"),
                        learning_type: r.get("learning_type"),
                        datasource_config_id: r.get("datasource_config_id"),
                        reference_queries: r.get("reference_queries"),
                    }
                }))
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            sqlx::query(&sql)
                .bind(learning_id)
                .bind(workspace_id)
                .fetch_optional(sq)
                .await
                .map(|opt| opt.map(|r| {
                    LearningRecord {
                        learning_id: r.get("learning_id"),
                        insight: r.get("insight"),
                        context: r.get("context"),
                        enabled: r.get::<Option<bool>, _>("enabled").unwrap_or(true),
                        times_used: r.get::<Option<i32>, _>("times_used").unwrap_or(0),
                        last_used_at: r.get("last_used_at"),
                        created_at: r.get::<Option<DateTime<Utc>>, _>("created_at").unwrap_or_else(Utc::now),
                        learned_from_user: r.get("learned_from_user"),
                        learned_from_session: r.get("learned_from_session"),
                        scope: r.get("scope"),
                        learning_type: r.get("learning_type"),
                        datasource_config_id: r.get("datasource_config_id"),
                        reference_queries: r.get("reference_queries"),
                    }
                }))
        }
    }
    .map_err(|e| kyomi_core::Error::Internal(format!("get learning failed: {e}")))?;

    Ok(row)
}
