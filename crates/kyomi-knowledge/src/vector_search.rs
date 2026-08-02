// SPDX-License-Identifier: AGPL-3.0-or-later

//! Vector search abstraction — trait + implementations for Postgres (pgvector) and SQLite (in-memory).
//!
//! The `VectorSearch` trait provides database-agnostic vector similarity search.
//! - `PgVectorSearch`: Uses pgvector's `<=>` cosine distance operator (ORDER BY distance ASC).
//! - `InMemoryVectorSearch`: Loads all embeddings from SQLite BLOBs, computes cosine
//!   similarity in memory, sorts descending (higher = more similar).
//!
//! Both implementations return results ranked by similarity (highest first).

use async_trait::async_trait;
use kyomi_core::db::DbPool;
use sqlx::postgres::PgPool;
use sqlx::sqlite::SqlitePool;

// ---------------------------------------------------------------------------
// Result structs — mirror what retrieval.rs expects from vector search
// ---------------------------------------------------------------------------

/// A table hit from vector search.
#[derive(Debug, Clone)]
pub struct TableSearchResult {
    pub project_id: String,
    pub dataset_id: String,
    pub table_id: String,
    pub table_metadata: Option<serde_json::Value>,
    pub datasource_slug: String,
    pub score: f64,
}

/// A column hit from vector search.
#[derive(Debug, Clone)]
pub struct ColumnSearchResult {
    pub column_name: String,
    pub data_type: String,
    pub project_id: String,
    pub dataset_id: String,
    pub table_id: String,
    pub score: f64,
}

/// A learning hit from vector search.
///
/// Not a duplicate of `kyomi_auth::learning_service::LearningSearchResult`:
/// that flattens a whole `LearningRecord` plus four hybrid-search scores;
/// this is a minimal id/insight/title/score hit.
#[derive(Debug, Clone)]
pub struct LearningSearchResult {
    pub learning_id: String,
    pub insight: String,
    pub title: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub score: f64,
}

/// A metric hit from vector search (same shape as learning but filtered differently).
pub type MetricSearchResult = LearningSearchResult;

/// A query history hit from vector search.
#[derive(Debug, Clone)]
pub struct QueryHistorySearchResult {
    pub query_id: String,
    pub sql_text: String,
    pub score: f64,
}

/// A dashboard hit from vector search.
///
/// Not a duplicate of `kyomi_auth::dashboard_service::DashboardSearchResult`:
/// that is a full dashboard record with popularity/view counts for the
/// dashboards search page; this is a minimal id/title/description/score hit.
#[derive(Debug, Clone)]
pub struct DashboardSearchResult {
    pub dashboard_id: String,
    pub title: String,
    pub description: Option<String>,
    pub score: f64,
}

// ---------------------------------------------------------------------------
// VectorSearch trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait VectorSearch: Send + Sync {
    async fn search_tables(
        &self,
        workspace_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<TableSearchResult>>;

    async fn search_columns(
        &self,
        workspace_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<ColumnSearchResult>>;

    async fn search_learnings(
        &self,
        workspace_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<LearningSearchResult>>;

    async fn search_metrics(
        &self,
        workspace_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<MetricSearchResult>>;

    async fn search_query_history(
        &self,
        workspace_id: &str,
        user_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<QueryHistorySearchResult>>;

    async fn search_dashboards(
        &self,
        workspace_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<DashboardSearchResult>>;
}

// ---------------------------------------------------------------------------
// PgVectorSearch — uses pgvector <=> cosine distance
// ---------------------------------------------------------------------------

/// Postgres implementation using pgvector's cosine distance operator.
pub struct PgVectorSearch {
    pool: PgPool,
}

impl PgVectorSearch {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Row type for table vector searches (pgvector).
#[derive(sqlx::FromRow)]
struct PgTableSearchRow {
    project_id: String,
    dataset_id: String,
    table_id: String,
    table_metadata: Option<serde_json::Value>,
    datasource_slug: String,
    score: f64,
}

/// Row type for column vector searches (pgvector).
#[derive(sqlx::FromRow)]
struct PgColumnSearchRow {
    column_name: String,
    data_type: Option<String>,
    project_id: String,
    dataset_id: String,
    table_id: String,
    score: f64,
}

/// Row type for learning/metric vector searches (pgvector).
#[derive(sqlx::FromRow)]
struct PgLearningSearchRow {
    learning_id: String,
    insight: String,
    title: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    score: f64,
}

#[async_trait]
impl VectorSearch for PgVectorSearch {
    async fn search_tables(
        &self,
        workspace_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<TableSearchResult>> {
        let query_vector = pgvector::Vector::from(query_embedding.to_vec());

        // Search both name_embedding and desc_embedding, union results
        let name_rows = sqlx::query_as::<_, PgTableSearchRow>(
            "SELECT tc.project_id, tc.dataset_id, tc.table_id, \
                    tc.table_metadata, COALESCE(dc.slug, '') as datasource_slug, \
                    1 - (tc.name_embedding <=> $1::vector) AS score \
             FROM datasource_table_cache tc \
             LEFT JOIN datasource_configs dc ON tc.datasource_config_id = dc.id \
             WHERE tc.workspace_id = $2 AND tc.name_embedding IS NOT NULL AND tc.is_archived = false \
             ORDER BY tc.name_embedding <=> $1::vector \
             LIMIT $3",
        )
        .bind(&query_vector)
        .bind(workspace_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await;

        let desc_rows = sqlx::query_as::<_, PgTableSearchRow>(
            "SELECT tc.project_id, tc.dataset_id, tc.table_id, \
                    tc.table_metadata, COALESCE(dc.slug, '') as datasource_slug, \
                    1 - (tc.desc_embedding <=> $1::vector) AS score \
             FROM datasource_table_cache tc \
             LEFT JOIN datasource_configs dc ON tc.datasource_config_id = dc.id \
             WHERE tc.workspace_id = $2 AND tc.desc_embedding IS NOT NULL AND tc.is_archived = false \
             ORDER BY tc.desc_embedding <=> $1::vector \
             LIMIT $3",
        )
        .bind(&query_vector)
        .bind(workspace_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await;

        let mut results = Vec::new();
        for rows in [name_rows, desc_rows] {
            match rows {
                Ok(rows) => {
                    for r in rows {
                        results.push(TableSearchResult {
                            project_id: r.project_id,
                            dataset_id: r.dataset_id,
                            table_id: r.table_id,
                            table_metadata: r.table_metadata,
                            datasource_slug: r.datasource_slug,
                            score: r.score,
                        });
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Vector search: table query failed");
                }
            }
        }

        Ok(results)
    }

    async fn search_columns(
        &self,
        workspace_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<ColumnSearchResult>> {
        let query_vector = pgvector::Vector::from(query_embedding.to_vec());

        // Search both name_embedding and desc_embedding
        let name_rows = sqlx::query_as::<_, PgColumnSearchRow>(
            "SELECT ce.column_name, ce.data_type, \
                    tc.project_id, tc.dataset_id, tc.table_id, \
                    1 - (ce.name_embedding <=> $1::vector) AS score \
             FROM column_embeddings ce \
             JOIN datasource_table_cache tc ON ce.table_cache_id = tc.id \
             WHERE ce.workspace_id = $2 AND ce.name_embedding IS NOT NULL AND tc.is_archived = false \
             ORDER BY ce.name_embedding <=> $1::vector \
             LIMIT $3",
        )
        .bind(&query_vector)
        .bind(workspace_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await;

        let desc_rows = sqlx::query_as::<_, PgColumnSearchRow>(
            "SELECT ce.column_name, ce.data_type, \
                    tc.project_id, tc.dataset_id, tc.table_id, \
                    1 - (ce.desc_embedding <=> $1::vector) AS score \
             FROM column_embeddings ce \
             JOIN datasource_table_cache tc ON ce.table_cache_id = tc.id \
             WHERE ce.workspace_id = $2 AND ce.desc_embedding IS NOT NULL AND tc.is_archived = false \
             ORDER BY ce.desc_embedding <=> $1::vector \
             LIMIT $3",
        )
        .bind(&query_vector)
        .bind(workspace_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await;

        let mut results = Vec::new();
        for rows in [name_rows, desc_rows] {
            match rows {
                Ok(rows) => {
                    for r in rows {
                        results.push(ColumnSearchResult {
                            column_name: r.column_name,
                            data_type: r.data_type.unwrap_or_default(),
                            project_id: r.project_id,
                            dataset_id: r.dataset_id,
                            table_id: r.table_id,
                            score: r.score,
                        });
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Vector search: column query failed");
                }
            }
        }

        Ok(results)
    }

    async fn search_learnings(
        &self,
        workspace_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<LearningSearchResult>> {
        let query_vector = pgvector::Vector::from(query_embedding.to_vec());

        let rows = sqlx::query_as::<_, PgLearningSearchRow>(
            "SELECT learning_id::text, insight, NULL::text AS title, created_at, \
                    1 - (embedding <=> $1::vector) AS score \
             FROM agent_learnings \
             WHERE workspace_id = $2 AND embedding IS NOT NULL \
               AND learning_type != 'metric' AND enabled = true AND superseded_by IS NULL \
             ORDER BY embedding <=> $1::vector \
             LIMIT $3",
        )
        .bind(&query_vector)
        .bind(workspace_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await;

        match rows {
            Ok(rows) => Ok(rows
                .into_iter()
                .map(|r| LearningSearchResult {
                    learning_id: r.learning_id,
                    insight: r.insight,
                    title: r.title,
                    created_at: r.created_at,
                    score: r.score,
                })
                .collect()),
            Err(e) => {
                tracing::error!(error = %e, "Vector search: learning query failed");
                Ok(vec![])
            }
        }
    }

    async fn search_metrics(
        &self,
        workspace_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<MetricSearchResult>> {
        let query_vector = pgvector::Vector::from(query_embedding.to_vec());

        let rows = sqlx::query_as::<_, PgLearningSearchRow>(
            "SELECT learning_id::text, insight, NULL::text AS title, created_at, \
                    1 - (embedding <=> $1::vector) AS score \
             FROM agent_learnings \
             WHERE workspace_id = $2 AND embedding IS NOT NULL \
               AND learning_type = 'metric' AND enabled = true AND superseded_by IS NULL \
             ORDER BY embedding <=> $1::vector \
             LIMIT $3",
        )
        .bind(&query_vector)
        .bind(workspace_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await;

        match rows {
            Ok(rows) => Ok(rows
                .into_iter()
                .map(|r| MetricSearchResult {
                    learning_id: r.learning_id,
                    insight: r.insight,
                    title: r.title,
                    created_at: r.created_at,
                    score: r.score,
                })
                .collect()),
            Err(e) => {
                tracing::error!(error = %e, "Vector search: metric query failed");
                Ok(vec![])
            }
        }
    }

    async fn search_query_history(
        &self,
        _workspace_id: &str,
        _user_id: &str,
        _query_embedding: &[f32],
        _limit: usize,
    ) -> kyomi_core::Result<Vec<QueryHistorySearchResult>> {
        // Query history search not yet implemented for pgvector
        Ok(vec![])
    }

    async fn search_dashboards(
        &self,
        _workspace_id: &str,
        _query_embedding: &[f32],
        _limit: usize,
    ) -> kyomi_core::Result<Vec<DashboardSearchResult>> {
        // Dashboard search not yet implemented for pgvector
        Ok(vec![])
    }
}

// ---------------------------------------------------------------------------
// InMemoryVectorSearch — loads BLOBs from SQLite, computes similarity in Rust
// ---------------------------------------------------------------------------

/// SQLite implementation using in-memory cosine similarity computation.
pub struct InMemoryVectorSearch {
    pool: SqlitePool,
}

impl InMemoryVectorSearch {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

/// Compute cosine similarity between two f32 vectors.
///
/// Returns `dot(a, b) / (norm(a) * norm(b))`.
/// Returns 0.0 if either vector has zero magnitude.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;

    for (x, y) in a.iter().zip(b.iter()) {
        let x = *x as f64;
        let y = *y as f64;
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        return 0.0;
    }

    dot / denom
}

// -- SQLite row types for in-memory vector search ----------------------------

#[derive(sqlx::FromRow)]
struct SqliteTableRow {
    project_id: String,
    dataset_id: String,
    table_id: String,
    table_metadata: Option<String>,
    datasource_slug: String,
    name_embedding: Option<Vec<u8>>,
    desc_embedding: Option<Vec<u8>>,
}

#[derive(sqlx::FromRow)]
struct SqliteColumnRow {
    column_name: String,
    data_type: Option<String>,
    project_id: String,
    dataset_id: String,
    table_id: String,
    name_embedding: Option<Vec<u8>>,
    desc_embedding: Option<Vec<u8>>,
}

#[derive(sqlx::FromRow)]
struct SqliteLearningRow {
    learning_id: String,
    insight: String,
    title: Option<String>,
    created_at: String,
    embedding: Option<Vec<u8>>,
}

/// Score + index pair for sorting.
struct ScoredIndex {
    index: usize,
    score: f64,
}

#[async_trait]
impl VectorSearch for InMemoryVectorSearch {
    async fn search_tables(
        &self,
        workspace_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<TableSearchResult>> {
        let bool_false = kyomi_core::sql_compat::bool_false(false);
        let sql = format!(
            "SELECT tc.project_id, tc.dataset_id, tc.table_id, \
                    tc.table_metadata, COALESCE(dc.slug, '') as datasource_slug, \
                    tc.name_embedding, tc.desc_embedding \
             FROM datasource_table_cache tc \
             LEFT JOIN datasource_configs dc ON tc.datasource_config_id = dc.id \
             WHERE tc.workspace_id = $1 AND tc.is_archived = {bool_false} \
               AND (tc.name_embedding IS NOT NULL OR tc.desc_embedding IS NOT NULL)"
        );
        let rows = match sqlx::query_as::<_, SqliteTableRow>(&sql)
            .bind(workspace_id)
            .fetch_all(&self.pool)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "Vector search: SQLite table query failed");
                return Ok(vec![]);
            }
        };

        let mut scored: Vec<ScoredIndex> = Vec::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            let mut best_score = 0.0f64;
            if let Some(ref emb_bytes) = row.name_embedding {
                let emb = kyomi_core::embedding_compat::bytes_to_embedding(emb_bytes);
                best_score = best_score.max(cosine_similarity(&emb, query_embedding));
            }
            if let Some(ref emb_bytes) = row.desc_embedding {
                let emb = kyomi_core::embedding_compat::bytes_to_embedding(emb_bytes);
                best_score = best_score.max(cosine_similarity(&emb, query_embedding));
            }
            scored.push(ScoredIndex { index: i, score: best_score });
        }

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        Ok(scored
            .into_iter()
            .map(|si| {
                let row = &rows[si.index];
                let metadata = row
                    .table_metadata
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok());
                TableSearchResult {
                    project_id: row.project_id.clone(),
                    dataset_id: row.dataset_id.clone(),
                    table_id: row.table_id.clone(),
                    table_metadata: metadata,
                    datasource_slug: row.datasource_slug.clone(),
                    score: si.score,
                }
            })
            .collect())
    }

    async fn search_columns(
        &self,
        workspace_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<ColumnSearchResult>> {
        let bool_false = kyomi_core::sql_compat::bool_false(false);
        let col_sql = format!(
            "SELECT ce.column_name, ce.data_type, \
                    tc.project_id, tc.dataset_id, tc.table_id, \
                    ce.name_embedding, ce.desc_embedding \
             FROM column_embeddings ce \
             JOIN datasource_table_cache tc ON ce.table_cache_id = tc.id \
             WHERE ce.workspace_id = $1 AND tc.is_archived = {bool_false} \
               AND (ce.name_embedding IS NOT NULL OR ce.desc_embedding IS NOT NULL)"
        );
        let rows = match sqlx::query_as::<_, SqliteColumnRow>(&col_sql)
            .bind(workspace_id)
            .fetch_all(&self.pool)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "Vector search: SQLite column query failed");
                return Ok(vec![]);
            }
        };

        let mut scored: Vec<ScoredIndex> = Vec::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            let mut best_score = 0.0f64;
            if let Some(ref emb_bytes) = row.name_embedding {
                let emb = kyomi_core::embedding_compat::bytes_to_embedding(emb_bytes);
                best_score = best_score.max(cosine_similarity(&emb, query_embedding));
            }
            if let Some(ref emb_bytes) = row.desc_embedding {
                let emb = kyomi_core::embedding_compat::bytes_to_embedding(emb_bytes);
                best_score = best_score.max(cosine_similarity(&emb, query_embedding));
            }
            scored.push(ScoredIndex { index: i, score: best_score });
        }

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        Ok(scored
            .into_iter()
            .map(|si| {
                let row = &rows[si.index];
                ColumnSearchResult {
                    column_name: row.column_name.clone(),
                    data_type: row.data_type.clone().unwrap_or_default(),
                    project_id: row.project_id.clone(),
                    dataset_id: row.dataset_id.clone(),
                    table_id: row.table_id.clone(),
                    score: si.score,
                }
            })
            .collect())
    }

    async fn search_learnings(
        &self,
        workspace_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<LearningSearchResult>> {
        let bool_true = kyomi_core::sql_compat::bool_true(false);
        let sql = format!(
            "SELECT learning_id, insight, NULL AS title, created_at, embedding \
             FROM agent_learnings \
             WHERE workspace_id = $1 AND embedding IS NOT NULL \
               AND learning_type != 'metric' AND enabled = {bool_true} AND superseded_by IS NULL"
        );
        let rows = match sqlx::query_as::<_, SqliteLearningRow>(&sql)
            .bind(workspace_id)
            .fetch_all(&self.pool)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "Vector search: SQLite learning query failed");
                return Ok(vec![]);
            }
        };

        search_learning_rows_in_memory(rows, query_embedding, limit)
    }

    async fn search_metrics(
        &self,
        workspace_id: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> kyomi_core::Result<Vec<MetricSearchResult>> {
        let bool_true = kyomi_core::sql_compat::bool_true(false);
        let sql = format!(
            "SELECT learning_id, insight, NULL AS title, created_at, embedding \
             FROM agent_learnings \
             WHERE workspace_id = $1 AND embedding IS NOT NULL \
               AND learning_type = 'metric' AND enabled = {bool_true} AND superseded_by IS NULL"
        );
        let rows = match sqlx::query_as::<_, SqliteLearningRow>(&sql)
            .bind(workspace_id)
            .fetch_all(&self.pool)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(error = %e, "Vector search: SQLite metric query failed");
                return Ok(vec![]);
            }
        };

        search_learning_rows_in_memory(rows, query_embedding, limit)
    }

    async fn search_query_history(
        &self,
        _workspace_id: &str,
        _user_id: &str,
        _query_embedding: &[f32],
        _limit: usize,
    ) -> kyomi_core::Result<Vec<QueryHistorySearchResult>> {
        // Query history search not yet implemented for SQLite
        Ok(vec![])
    }

    async fn search_dashboards(
        &self,
        _workspace_id: &str,
        _query_embedding: &[f32],
        _limit: usize,
    ) -> kyomi_core::Result<Vec<DashboardSearchResult>> {
        // Dashboard search not yet implemented for SQLite
        Ok(vec![])
    }
}

/// Shared logic for learning/metric in-memory search over SQLite rows.
fn search_learning_rows_in_memory(
    rows: Vec<SqliteLearningRow>,
    query_embedding: &[f32],
    limit: usize,
) -> kyomi_core::Result<Vec<LearningSearchResult>> {
    let mut scored: Vec<ScoredIndex> = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        if let Some(ref emb_bytes) = row.embedding {
            let emb = kyomi_core::embedding_compat::bytes_to_embedding(emb_bytes);
            let score = cosine_similarity(&emb, query_embedding);
            scored.push(ScoredIndex { index: i, score });
        }
    }

    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);

    Ok(scored
        .into_iter()
        .map(|si| {
            let row = &rows[si.index];
            let created_at = chrono::DateTime::parse_from_rfc3339(&row.created_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| {
                    // SQLite may store as "YYYY-MM-DD HH:MM:SS" without timezone
                    chrono::NaiveDateTime::parse_from_str(&row.created_at, "%Y-%m-%d %H:%M:%S")
                        .map(|ndt| ndt.and_utc())
                        .unwrap_or_default()
                });
            LearningSearchResult {
                learning_id: row.learning_id.clone(),
                insight: row.insight.clone(),
                title: row.title.clone(),
                created_at,
                score: si.score,
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Factory function
// ---------------------------------------------------------------------------

/// Create the appropriate vector search implementation based on pool type.
pub fn create_vector_search(pool: &DbPool) -> Box<dyn VectorSearch> {
    match pool {
        DbPool::Postgres(pg) => Box::new(PgVectorSearch::new(pg.clone())),
        DbPool::Sqlite(sq) => Box::new(InMemoryVectorSearch::new(sq.clone())),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_vectors_similarity_near_one() {
        let a = vec![1.0f32, 2.0, 3.0, 4.0];
        let sim = cosine_similarity(&a, &a);
        assert!(
            (sim - 1.0).abs() < 1e-10,
            "Identical vectors should have similarity ~1.0, got {sim}"
        );
    }

    #[test]
    fn orthogonal_vectors_similarity_near_zero() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![0.0f32, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(
            sim.abs() < 1e-10,
            "Orthogonal vectors should have similarity ~0.0, got {sim}"
        );
    }

    #[test]
    fn opposite_vectors_similarity_near_negative_one() {
        let a = vec![1.0f32, 2.0, 3.0];
        let b = vec![-1.0f32, -2.0, -3.0];
        let sim = cosine_similarity(&a, &b);
        assert!(
            (sim + 1.0).abs() < 1e-10,
            "Opposite vectors should have similarity ~-1.0, got {sim}"
        );
    }

    #[test]
    fn similar_vector_ranks_higher() {
        let query = vec![1.0f32, 0.0, 0.0];
        let close = vec![0.9f32, 0.1, 0.0]; // mostly aligned
        let far = vec![0.1f32, 0.9, 0.0]; // mostly orthogonal

        let sim_close = cosine_similarity(&close, &query);
        let sim_far = cosine_similarity(&far, &query);

        assert!(
            sim_close > sim_far,
            "Close vector should rank higher: {sim_close} > {sim_far}"
        );
    }

    #[test]
    fn zero_vector_similarity_is_zero() {
        let a = vec![0.0f32, 0.0, 0.0];
        let b = vec![1.0f32, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert!(
            sim.abs() < 1e-10,
            "Zero vector should have similarity 0.0, got {sim}"
        );
    }

    #[test]
    fn empty_vectors_similarity_is_zero() {
        let sim = cosine_similarity(&[], &[]);
        assert!(sim.abs() < 1e-10);
    }

    #[test]
    fn different_length_vectors_similarity_is_zero() {
        let a = vec![1.0f32, 2.0];
        let b = vec![1.0f32, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-10);
    }

    #[test]
    fn result_ordering_descending_by_similarity() {
        // Simulate the sorting logic used in InMemoryVectorSearch
        let query = vec![1.0f32, 0.0, 0.0];
        let candidates = [
            vec![0.1f32, 0.9, 0.0], // low similarity
            vec![0.9f32, 0.1, 0.0], // high similarity
            vec![0.5f32, 0.5, 0.0], // medium similarity
        ];

        let mut scored: Vec<(usize, f64)> = candidates
            .iter()
            .enumerate()
            .map(|(i, c)| (i, cosine_similarity(c, &query)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        // Highest similarity first
        assert_eq!(scored[0].0, 1, "Highest similarity candidate should be first");
        assert_eq!(scored[1].0, 2, "Medium similarity candidate should be second");
        assert_eq!(scored[2].0, 0, "Lowest similarity candidate should be last");
    }

    #[test]
    fn limit_parameter_respected() {
        let query = vec![1.0f32, 0.0, 0.0];
        let candidates = [
            vec![0.9f32, 0.1, 0.0],
            vec![0.8f32, 0.2, 0.0],
            vec![0.7f32, 0.3, 0.0],
            vec![0.6f32, 0.4, 0.0],
            vec![0.5f32, 0.5, 0.0],
        ];

        let mut scored: Vec<ScoredIndex> = candidates
            .iter()
            .enumerate()
            .map(|(i, c)| ScoredIndex {
                index: i,
                score: cosine_similarity(c, &query),
            })
            .collect();
        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        let limit = 3;
        scored.truncate(limit);

        assert_eq!(scored.len(), limit, "Should only return {limit} results");
        // Verify they are the top 3
        assert_eq!(scored[0].index, 0);
        assert_eq!(scored[1].index, 1);
        assert_eq!(scored[2].index, 2);
    }
}
