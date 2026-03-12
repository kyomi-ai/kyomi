// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard service — CRUD, search, versioning, and embedding generation.
//!
//! Ports Python's dashboard router + version service + summary service into a
//! shared service layer. Used by both agent tools (Phase 9B-3) and REST
//! endpoints (Phase 12) — zero business logic duplication.
//!
//! Key design decisions:
//! - Free-function pattern (`&DbPool` first arg) matching `chat_service.rs`
//! - ChartML validation via YAML parsing (no LLM calls)
//! - Popularity scoring via time-weighted view counts (7d/30d/90d/older)
//! - Background embedding generation via `tokio::spawn`

use chrono::{DateTime, Duration, Utc};
use kyomi_core::embedding_compat::{bytes_to_embedding, embedding_to_bytes};
use kyomi_core::sql_compat;
use kyomi_core::{db_execute, db_fetch_optional, db_fetch_scalar};
use kyomi_core::{DbPool, Result};
use pgvector::Vector;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::sync::LazyLock;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Free tier dashboard limit.
const FREE_TIER_DASHBOARD_LIMIT: i64 = 5;

/// Popularity scoring weights (recency-weighted view counts).
const POPULARITY_RECENT_DAYS: i64 = 7;
const POPULARITY_MEDIUM_DAYS: i64 = 30;
const POPULARITY_OLD_DAYS: i64 = 90;
// Popularity weights are hardcoded in the SQL query (1.0, 0.5, 0.25, 0.1)
// to allow compile-time query validation via sqlx::query_as!().

/// Regex for extracting `<!-- dashboard-summary: ... -->` HTML comments.
static SUMMARY_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^<!-- dashboard-summary: (.+?) -->\n?").unwrap()
});

/// Regex for extracting ChartML fenced code blocks.
static CHARTML_BLOCK_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)```chartml\n(.*?)```").unwrap()
});

// ─── Response types ──────────────────────────────────────────────────────────

/// Sort order for dashboard search/listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchSort {
    Popularity,
    Recent,
    Created,
}

/// A dashboard search result with computed fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSearchResult {
    pub dashboard_id: String,
    pub user_id: String,
    pub workspace_id: String,
    pub title: String,
    pub content: String,
    pub last_change_summary: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub popularity_score: f64,
    pub content_preview: Option<String>,
    pub view_count: i64,
    pub recent_views: i64,
}

/// A version summary for listing (without full content).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardVersionSummary {
    pub version_id: i32,
    pub version_number: i32,
    pub title: String,
    pub change_summary: Option<String>,
    pub created_by: CreatedBy,
    pub created_at: DateTime<Utc>,
    pub byte_size: Option<i32>,
}

/// User attribution in version summaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedBy {
    pub user_id: String,
    pub name: Option<String>,
    pub email: String,
}

// ─── Title validation ────────────────────────────────────────────────────────

/// Validate dashboard title length (3-255 characters).
fn validate_title(title: &str) -> Result<()> {
    let trimmed = title.trim();
    if trimmed.len() < 3 {
        return Err(kyomi_core::Error::BadRequest(
            "Dashboard title must be at least 3 characters".into(),
        ));
    }
    if trimmed.len() > 255 {
        return Err(kyomi_core::Error::BadRequest(
            "Dashboard title must be at most 255 characters".into(),
        ));
    }
    Ok(())
}

// ─── ChartML validation ─────────────────────────────────────────────────────

/// Validate dashboard content by extracting and parsing ChartML fenced blocks.
///
/// - Content with no ChartML blocks is valid (pure markdown).
/// - Each `chartml` fenced block must be valid YAML with `data` and `visualize` keys.
pub fn validate_dashboard_content(content: &str) -> Result<()> {
    for cap in CHARTML_BLOCK_PATTERN.captures_iter(content) {
        let yaml_str = &cap[1];
        let parsed: serde_yaml::Value = serde_yaml::from_str(yaml_str).map_err(|e| {
            kyomi_core::Error::BadRequest(format!("Invalid ChartML YAML: {e}"))
        })?;

        // A chartml block can be a single chart (mapping) or multiple charts (sequence)
        let charts: Vec<&serde_yaml::Mapping> = if let Some(mapping) = parsed.as_mapping() {
            vec![mapping]
        } else if let Some(sequence) = parsed.as_sequence() {
            sequence
                .iter()
                .map(|item| {
                    item.as_mapping().ok_or_else(|| {
                        kyomi_core::Error::BadRequest(
                            "Each chart in a ChartML block must be a YAML mapping".into(),
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            return Err(kyomi_core::Error::BadRequest(
                "ChartML block must be a YAML mapping or a list of mappings".into(),
            ));
        };

        for mapping in charts {
            let has_data = mapping.contains_key(serde_yaml::Value::String("data".into()));
            let has_visualize =
                mapping.contains_key(serde_yaml::Value::String("visualize".into()));

            if !has_data {
                return Err(kyomi_core::Error::BadRequest(
                    "ChartML block missing required 'data' key".into(),
                ));
            }
            if !has_visualize {
                return Err(kyomi_core::Error::BadRequest(
                    "ChartML block missing required 'visualize' key".into(),
                ));
            }
        }
    }
    Ok(())
}

// ─── Summary extraction ──────────────────────────────────────────────────────

/// Extract the dashboard summary from the `<!-- dashboard-summary: ... -->`
/// HTML comment at the start of content.
pub fn extract_summary(content: &str) -> Option<String> {
    SUMMARY_PATTERN
        .captures(content)
        .map(|cap| cap[1].trim().to_string())
}

// ─── Change summary generation ───────────────────────────────────────────────

/// Generate a human-readable change summary by comparing old and new content.
///
/// Ports Python's `DashboardVersionService.generate_change_summary`.
pub fn generate_change_summary(old_content: &str, new_content: &str) -> String {
    if old_content.is_empty() {
        return "Initial version".into();
    }

    let old_lines: std::collections::HashSet<&str> = old_content.lines().collect();
    let new_lines: std::collections::HashSet<&str> = new_content.lines().collect();

    let added = new_lines.difference(&old_lines).count();
    let removed = old_lines.difference(&new_lines).count();

    let old_charts = old_content.matches("```chartml").count();
    let new_charts = new_content.matches("```chartml").count();

    if new_charts > old_charts {
        format!("Added {} chart(s)", new_charts - old_charts)
    } else if new_charts < old_charts {
        format!("Removed {} chart(s)", old_charts - new_charts)
    } else if added > 0 && removed == 0 {
        "Added content".into()
    } else if removed > 0 && added == 0 {
        "Removed content".into()
    } else if added > removed {
        format!("Updated dashboard (+{} lines)", added - removed)
    } else if removed > added {
        format!("Updated dashboard (-{} lines)", removed - added)
    } else {
        "Updated dashboard".into()
    }
}

// ─── Create dashboard ────────────────────────────────────────────────────────

/// Create a new dashboard.
///
/// Validates title length, validates ChartML content, checks free tier limit,
/// and INSERTs into the `dashboards` table.
pub async fn create_dashboard(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
    title: &str,
    content: &str,
) -> Result<String> {
    validate_title(title)?;
    validate_dashboard_content(content)?;

    // Check free tier limit (5 dashboards per workspace) — paid tiers are unlimited
    #[derive(sqlx::FromRow)]
    struct TierRow { subscription_tier: String }
    let tier_row = db_fetch_optional!(
        db,
        TierRow,
        "SELECT subscription_tier FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?
    .ok_or_else(|| kyomi_core::Error::NotFound("Workspace not found".into()))?;
    let tier_str = tier_row.subscription_tier;

    if tier_str == "free" {
        let count = get_dashboard_count(db, workspace_id, Some(user_id)).await?;
        if count >= FREE_TIER_DASHBOARD_LIMIT {
            return Err(kyomi_core::Error::Forbidden(
                "Free tier is limited to 5 dashboards. Please upgrade to create more dashboards."
                    .into(),
            ));
        }
    }

    let is_pg = db.is_postgres();
    let now_expr = sql_compat::now(is_pg);
    let dashboard_id = format!("{}", uuid::Uuid::new_v4());

    let sql = format!(
        r#"
        INSERT INTO dashboards (dashboard_id, user_id, workspace_id, title, content, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, {now_expr}, {now_expr})
        "#
    );

    db_execute!(db, &sql, &dashboard_id, user_id, workspace_id, title.trim(), content)
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to create dashboard: {e}")))?;

    tracing::info!(dashboard_id = %dashboard_id, "Created dashboard");
    Ok(dashboard_id)
}

// ─── Get dashboard ───────────────────────────────────────────────────────────

/// Fetch a dashboard by ID within a workspace.
pub async fn get_dashboard(
    db: &DbPool,
    dashboard_id: &str,
    workspace_id: &str,
) -> Result<Option<kyomi_core::models::Dashboard>> {
    let row = db_fetch_optional!(
        db,
        kyomi_core::models::Dashboard,
        r#"
        SELECT dashboard_id, user_id, workspace_id, title, content,
               last_change_summary, embedding,
               created_at, updated_at
        FROM dashboards
        WHERE dashboard_id = $1 AND workspace_id = $2
        "#,
        dashboard_id,
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get dashboard: {e}")))?;

    Ok(row)
}

// ─── Update dashboard ────────────────────────────────────────────────────────

/// Update a dashboard (title, content, change_summary).
///
/// Ownership check: only the dashboard owner can update.
/// Before updating, creates a version snapshot of the old state.
/// Auto-generates a change summary if not provided.
pub async fn update_dashboard(
    db: &DbPool,
    dashboard_id: &str,
    workspace_id: &str,
    user_id: &str,
    title: Option<&str>,
    content: Option<&str>,
    change_summary: Option<&str>,
) -> Result<bool> {
    // Fetch current dashboard for ownership check and version creation
    let current = get_dashboard(db, dashboard_id, workspace_id).await?;
    let current = current.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!("Dashboard {dashboard_id} not found"))
    })?;

    if current.user_id != user_id {
        return Err(kyomi_core::Error::Forbidden(
            "Only the dashboard owner can update it".into(),
        ));
    }

    // Validate new values
    if let Some(t) = title {
        validate_title(t)?;
    }
    if let Some(c) = content {
        validate_dashboard_content(c)?;
    }

    // Create version of old state before updating
    let auto_summary = match change_summary {
        Some(s) => s.to_string(),
        None => {
            let new_content = content.unwrap_or(&current.content);
            generate_change_summary(&current.content, new_content)
        }
    };

    create_version(
        db,
        dashboard_id,
        &current.content,
        &current.title,
        user_id,
        Some(&auto_summary),
    )
    .await?;

    // Dynamic UPDATE
    let mut set_parts: Vec<String> = Vec::new();
    let mut param_idx = 3u32; // $1 = dashboard_id, $2 = workspace_id

    if title.is_some() {
        set_parts.push(format!("title = ${param_idx}"));
        param_idx += 1;
    }
    if content.is_some() {
        set_parts.push(format!("content = ${param_idx}"));
        param_idx += 1;
    }

    // Always set change summary and updated_at
    set_parts.push(format!("last_change_summary = ${param_idx}"));
    param_idx += 1;
    set_parts.push(format!("updated_at = ${param_idx}"));

    let sql = format!(
        "UPDATE dashboards SET {} WHERE dashboard_id = $1 AND workspace_id = $2",
        set_parts.join(", ")
    );

    let now = Utc::now();
    // Dynamic SQL with variable bind count — use match pool directly
    let result = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let mut query = sqlx::query(&sql).bind(dashboard_id).bind(workspace_id);
            if let Some(t) = title { query = query.bind(t.trim()); }
            if let Some(c) = content { query = query.bind(c); }
            query = query.bind(&auto_summary);
            query = query.bind(now);
            query.execute(pg).await.map(kyomi_core::db::DbQueryResult::from_pg)
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let mut query = sqlx::query(&sql).bind(dashboard_id).bind(workspace_id);
            if let Some(t) = title { query = query.bind(t.trim()); }
            if let Some(c) = content { query = query.bind(c); }
            query = query.bind(&auto_summary);
            query = query.bind(now);
            query.execute(sq).await.map(kyomi_core::db::DbQueryResult::from_sqlite)
        }
    }
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to update dashboard: {e}"))
    })?;

    tracing::info!(dashboard_id = %dashboard_id, "Updated dashboard");
    Ok(result.rows_affected() > 0)
}

// ─── Delete dashboard ────────────────────────────────────────────────────────

/// Delete a dashboard (ownership check required).
///
/// CASCADE handles views and versions.
pub async fn delete_dashboard(
    db: &DbPool,
    dashboard_id: &str,
    workspace_id: &str,
    user_id: &str,
) -> Result<bool> {
    // Ownership check
    let current = get_dashboard(db, dashboard_id, workspace_id).await?;
    let current = current.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!("Dashboard {dashboard_id} not found"))
    })?;

    if current.user_id != user_id {
        return Err(kyomi_core::Error::Forbidden(
            "Only the dashboard owner can delete it".into(),
        ));
    }

    let result = db_execute!(
        db,
        "DELETE FROM dashboards WHERE dashboard_id = $1 AND workspace_id = $2",
        dashboard_id,
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete dashboard: {e}")))?;

    tracing::info!(dashboard_id = %dashboard_id, "Deleted dashboard");
    Ok(result.rows_affected() > 0)
}

// ─── Search dashboards ──────────────────────────────────────────────────────

/// Search dashboards by text query with sorting and popularity scoring.
///
/// Uses ILIKE/LIKE text search on title and content. For full hybrid search
/// (BM25 + semantic), the REST endpoint layer can compose this with
/// embedding-based search.
///
/// Popularity is computed via time-weighted view counts:
/// - Last 7 days: 1.0 weight
/// - Last 30 days: 0.5
/// - Last 90 days: 0.25
/// - Older: 0.1
pub async fn search_dashboards(
    db: &DbPool,
    workspace_id: &str,
    query: Option<&str>,
    sort_by: SearchSort,
    limit: i64,
) -> Result<Vec<DashboardSearchResult>> {
    let is_pg = db.is_postgres();
    let now = Utc::now();
    let recent_cutoff = now - Duration::days(POPULARITY_RECENT_DAYS);
    let medium_cutoff = now - Duration::days(POPULARITY_MEDIUM_DAYS);
    let old_cutoff = now - Duration::days(POPULARITY_OLD_DAYS);

    let query_param: Option<&str> = query
        .filter(|q| !q.trim().is_empty())
        .map(|q| q.trim());

    // Build the text filter
    let text_filter = if is_pg {
        "AND ($5::text IS NULL OR d.title ILIKE '%' || $5 || '%' OR d.content ILIKE '%' || $5 || '%')"
    } else {
        "AND ($5 IS NULL OR d.title LIKE '%' || $5 || '%' OR d.content LIKE '%' || $5 || '%')"
    };

    // Popularity sub-query with CASE expressions
    // Postgres uses FILTER (WHERE ...) but we use CASE for cross-db compat
    let popularity_sql = format!(
        r#"
        SELECT
            d.dashboard_id, d.user_id, d.workspace_id, d.title, d.content,
            d.last_change_summary, d.created_at, d.updated_at,
            COALESCE(v.popularity_score, 0.0) AS popularity_score,
            COALESCE(v.view_count, 0) AS view_count,
            COALESCE(v.recent_views, 0) AS recent_views
        FROM dashboards d
        LEFT JOIN (
            SELECT
                dashboard_id,
                COUNT(*) AS view_count,
                SUM(CASE WHEN viewed_at >= $2 THEN 1 ELSE 0 END) AS recent_views,
                SUM(
                    CASE
                        WHEN viewed_at >= $2 THEN 1.0
                        WHEN viewed_at >= $3 THEN 0.5
                        WHEN viewed_at >= $4 THEN 0.25
                        ELSE 0.1
                    END
                ) AS popularity_score
            FROM dashboard_views
            WHERE workspace_id = $1
            GROUP BY dashboard_id
        ) v ON d.dashboard_id = v.dashboard_id
        WHERE d.workspace_id = $1
        {text_filter}
        "#
    );

    // NOTE: The row-mapping closures for Postgres and SQLite are intentionally
    // duplicated because `sqlx::PgRow` and `sqlx::SqliteRow` are distinct types
    // that both implement `sqlx::Row` but cannot be unified without trait objects
    // or a generic helper.  The mapping logic is identical; if you change one arm,
    // update the other to match.
    let rows: Vec<DashboardSearchResult> = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let raw_rows = sqlx::query(&popularity_sql)
                .bind(workspace_id)
                .bind(recent_cutoff)
                .bind(medium_cutoff)
                .bind(old_cutoff)
                .bind(&query_param)
                .fetch_all(pg)
                .await
                .map_err(|e| kyomi_core::Error::Internal(format!("failed to search dashboards: {e}")))?;

            raw_rows.iter().map(|row| {
                let content: String = row.get("content");
                let preview = extract_summary(&content).or_else(|| {
                    let clean = CHARTML_BLOCK_PATTERN.replace_all(&content, "");
                    let clean = SUMMARY_PATTERN.replace(&clean, "");
                    let clean = clean.trim();
                    if clean.is_empty() { None } else { Some(clean.chars().take(200).collect()) }
                });
                DashboardSearchResult {
                    dashboard_id: row.get("dashboard_id"),
                    user_id: row.get("user_id"),
                    workspace_id: row.get("workspace_id"),
                    title: row.get("title"),
                    content,
                    last_change_summary: row.get("last_change_summary"),
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                    popularity_score: row.get::<Option<f64>, _>("popularity_score").unwrap_or(0.0),
                    content_preview: preview,
                    view_count: row.get::<Option<i64>, _>("view_count").unwrap_or(0),
                    recent_views: row.get::<Option<i64>, _>("recent_views").unwrap_or(0),
                }
            }).collect()
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let raw_rows = sqlx::query(&popularity_sql)
                .bind(workspace_id)
                .bind(recent_cutoff)
                .bind(medium_cutoff)
                .bind(old_cutoff)
                .bind(&query_param)
                .fetch_all(sq)
                .await
                .map_err(|e| kyomi_core::Error::Internal(format!("failed to search dashboards: {e}")))?;

            raw_rows.iter().map(|row| {
                let content: String = row.get("content");
                let preview = extract_summary(&content).or_else(|| {
                    let clean = CHARTML_BLOCK_PATTERN.replace_all(&content, "");
                    let clean = SUMMARY_PATTERN.replace(&clean, "");
                    let clean = clean.trim();
                    if clean.is_empty() { None } else { Some(clean.chars().take(200).collect()) }
                });
                DashboardSearchResult {
                    dashboard_id: row.get("dashboard_id"),
                    user_id: row.get("user_id"),
                    workspace_id: row.get("workspace_id"),
                    title: row.get("title"),
                    content,
                    last_change_summary: row.get("last_change_summary"),
                    created_at: row.get("created_at"),
                    updated_at: row.get("updated_at"),
                    popularity_score: row.get::<Option<f64>, _>("popularity_score").unwrap_or(0.0),
                    content_preview: preview,
                    view_count: row.get::<Option<i64>, _>("view_count").unwrap_or(0),
                    recent_views: row.get::<Option<i64>, _>("recent_views").unwrap_or(0),
                }
            }).collect()
        }
    };

    let mut results = rows;

    // Sort in Rust to avoid 3 duplicate SQL queries for each ORDER BY variant
    match sort_by {
        SearchSort::Popularity => results.sort_by(|a, b| {
            b.popularity_score
                .partial_cmp(&a.popularity_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.updated_at.cmp(&a.updated_at))
        }),
        SearchSort::Recent => results.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
        SearchSort::Created => results.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
    }
    results.truncate(limit as usize);

    Ok(results)
}

// ─── Dashboard count ─────────────────────────────────────────────────────────

/// Get the number of dashboards in a workspace, optionally filtered by user.
///
/// When `user_id` is `Some`, counts only that user's dashboards (for free tier checks).
/// When `None`, counts all dashboards in the workspace.
pub async fn get_dashboard_count(
    db: &DbPool,
    workspace_id: &str,
    user_id: Option<&str>,
) -> Result<i64> {
    let count: i64 = if let Some(uid) = user_id {
        db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM dashboards WHERE workspace_id = $1 AND user_id = $2",
            workspace_id,
            uid
        )
    } else {
        db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM dashboards WHERE workspace_id = $1",
            workspace_id
        )
    }
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to count dashboards: {e}")))?;

    Ok(count)
}

// ─── Record view ─────────────────────────────────────────────────────────────

/// Record a dashboard view for popularity tracking.
pub async fn record_view(
    db: &DbPool,
    dashboard_id: &str,
    user_id: &str,
    workspace_id: &str,
) -> Result<()> {
    let is_pg = db.is_postgres();
    let now_expr = sql_compat::now(is_pg);
    let view_id = format!("view-{}", &uuid::Uuid::new_v4().to_string()[..20]);

    let sql = format!(
        r#"
        INSERT INTO dashboard_views (view_id, dashboard_id, user_id, workspace_id, viewed_at)
        VALUES ($1, $2, $3, $4, {now_expr})
        "#
    );

    db_execute!(db, &sql, &view_id, dashboard_id, user_id, workspace_id)
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to record view: {e}")))?;

    Ok(())
}

// ─── Version management ──────────────────────────────────────────────────────

/// Create a new version snapshot for a dashboard.
///
/// Determines the next version number, computes a SHA-256 content hash for
/// dedup (skips if content unchanged), and auto-generates a change summary.
pub async fn create_version(
    db: &DbPool,
    dashboard_id: &str,
    content: &str,
    title: &str,
    user_id: &str,
    change_summary: Option<&str>,
) -> Result<i32> {
    // Get next version number
    let max_version: Option<i32> = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            sqlx::query_scalar::<_, i32>(
                "SELECT MAX(version_number) FROM dashboard_versions WHERE dashboard_id = $1",
            )
            .bind(dashboard_id)
            .fetch_optional(pg)
            .await
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            sqlx::query_scalar::<_, i32>(
                "SELECT MAX(version_number) FROM dashboard_versions WHERE dashboard_id = $1",
            )
            .bind(dashboard_id)
            .fetch_optional(sq)
            .await
        }
    }
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get max version: {e}")))?;

    let max_version = max_version.unwrap_or(0);

    // SHA-256 dedup
    let content_hash = format!("{:x}", Sha256::digest(content.as_bytes()));

    if max_version > 0 {
        let latest_hash: Option<String> = match db {
            kyomi_core::db::DbPool::Postgres(pg) => {
                sqlx::query_scalar::<_, String>(
                    "SELECT content_hash FROM dashboard_versions WHERE dashboard_id = $1 AND version_number = $2",
                )
                .bind(dashboard_id)
                .bind(max_version)
                .fetch_optional(pg)
                .await
            }
            kyomi_core::db::DbPool::Sqlite(sq) => {
                sqlx::query_scalar::<_, String>(
                    "SELECT content_hash FROM dashboard_versions WHERE dashboard_id = $1 AND version_number = $2",
                )
                .bind(dashboard_id)
                .bind(max_version)
                .fetch_optional(sq)
                .await
            }
        }
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to check latest hash: {e}"))
        })?;

        if latest_hash.as_deref() == Some(content_hash.as_str()) {
            tracing::debug!(
                dashboard_id = %dashboard_id,
                "Skipping version creation — content unchanged"
            );
            return Ok(max_version);
        }
    }

    let summary = change_summary
        .map(String::from)
        .unwrap_or_else(|| "Updated dashboard".into());
    let byte_size = content.len() as i32;
    let next_version = max_version + 1;

    let is_pg = db.is_postgres();
    let now_expr = sql_compat::now(is_pg);
    let sql = format!(
        r#"
        INSERT INTO dashboard_versions
            (dashboard_id, version_number, content, title, change_summary,
             created_by, content_hash, byte_size, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, {now_expr})
        "#
    );

    db_execute!(
        db,
        &sql,
        dashboard_id,
        &next_version,
        content,
        title,
        &summary,
        user_id,
        &content_hash,
        &byte_size
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to create version: {e}")))?;

    tracing::info!(
        dashboard_id = %dashboard_id,
        version = next_version,
        "Created dashboard version"
    );
    Ok(next_version)
}

/// List version summaries for a dashboard (newest first, paginated).
pub async fn list_versions(
    db: &DbPool,
    dashboard_id: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<DashboardVersionSummary>> {
    let sql = r#"
        SELECT dv.version_id, dv.version_number, dv.title, dv.change_summary,
               dv.created_at, dv.byte_size, dv.created_by,
               u.user_id, u.name, u.email
        FROM dashboard_versions dv
        LEFT JOIN users u ON dv.created_by = u.user_id
        WHERE dv.dashboard_id = $1
        ORDER BY dv.version_number DESC
        LIMIT $2 OFFSET $3
    "#;

    let versions = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let rows = sqlx::query(sql)
                .bind(dashboard_id)
                .bind(limit)
                .bind(offset)
                .fetch_all(pg)
                .await
                .map_err(|e| kyomi_core::Error::Internal(format!("failed to list versions: {e}")))?;

            rows.iter().map(|row| version_summary_from_pg_row(row)).collect()
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let rows = sqlx::query(sql)
                .bind(dashboard_id)
                .bind(limit)
                .bind(offset)
                .fetch_all(sq)
                .await
                .map_err(|e| kyomi_core::Error::Internal(format!("failed to list versions: {e}")))?;

            rows.iter().map(|row| version_summary_from_sq_row(row)).collect()
        }
    };

    Ok(versions)
}

/// Extract a DashboardVersionSummary from a Postgres row.
fn version_summary_from_pg_row(row: &sqlx::postgres::PgRow) -> DashboardVersionSummary {
    let user_id: Option<String> = row.get("user_id");
    let created_by_id: String = row.get("created_by");
    DashboardVersionSummary {
        version_id: row.get("version_id"),
        version_number: row.get("version_number"),
        title: row.get("title"),
        change_summary: row.get("change_summary"),
        created_by: CreatedBy {
            user_id: user_id.unwrap_or(created_by_id),
            name: row.get("name"),
            email: row
                .get::<Option<String>, _>("email")
                .unwrap_or_else(|| "(deleted user)".into()),
        },
        created_at: row.get("created_at"),
        byte_size: row.get("byte_size"),
    }
}

/// Extract a DashboardVersionSummary from a SQLite row.
fn version_summary_from_sq_row(row: &sqlx::sqlite::SqliteRow) -> DashboardVersionSummary {
    let user_id: Option<String> = row.get("user_id");
    let created_by_id: String = row.get("created_by");
    DashboardVersionSummary {
        version_id: row.get("version_id"),
        version_number: row.get("version_number"),
        title: row.get("title"),
        change_summary: row.get("change_summary"),
        created_by: CreatedBy {
            user_id: user_id.unwrap_or(created_by_id),
            name: row.get("name"),
            email: row
                .get::<Option<String>, _>("email")
                .unwrap_or_else(|| "(deleted user)".into()),
        },
        created_at: row.get("created_at"),
        byte_size: row.get("byte_size"),
    }
}

/// Get a specific version with full content.
pub async fn get_version(
    db: &DbPool,
    dashboard_id: &str,
    version_number: i32,
) -> Result<Option<kyomi_core::models::DashboardVersion>> {
    let version = db_fetch_optional!(
        db,
        kyomi_core::models::DashboardVersion,
        r#"
        SELECT version_id, dashboard_id, created_by, version_number, content,
               title, change_summary, content_hash, byte_size, created_at
        FROM dashboard_versions
        WHERE dashboard_id = $1 AND version_number = $2
        "#,
        dashboard_id,
        &version_number
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get version: {e}")))?;

    Ok(version)
}

/// Get the total number of versions for a dashboard (for pagination).
pub async fn get_version_count(db: &DbPool, dashboard_id: &str) -> Result<i64> {
    let count: i64 = db_fetch_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM dashboard_versions WHERE dashboard_id = $1",
        dashboard_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to count versions: {e}")))?;

    Ok(count)
}

/// Restore a dashboard to a previous version.
///
/// Creates a version of the current state before restoring, then updates
/// the dashboard with the old content and creates a new version for the restore.
/// Returns the new version number.
pub async fn restore_version(
    db: &DbPool,
    dashboard_id: &str,
    workspace_id: &str,
    user_id: &str,
    version_number: i32,
) -> Result<i32> {
    // Fetch the version to restore
    let old_version = get_version(db, dashboard_id, version_number).await?;
    let old_version = old_version.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!(
            "Version {version_number} not found for dashboard {dashboard_id}"
        ))
    })?;

    // Fetch current dashboard for ownership check and version creation
    let current = get_dashboard(db, dashboard_id, workspace_id).await?;
    let current = current.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!("Dashboard {dashboard_id} not found"))
    })?;

    if current.user_id != user_id {
        return Err(kyomi_core::Error::Forbidden(
            "Only the dashboard owner can restore versions".into(),
        ));
    }

    // Create version of current state before restoring
    create_version(
        db,
        dashboard_id,
        &current.content,
        &current.title,
        user_id,
        Some(&format!(
            "Auto-saved before restoring to version {version_number}"
        )),
    )
    .await?;

    // Update dashboard with old content
    let is_pg = db.is_postgres();
    let now_expr = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE dashboards SET content = $1, title = $2, updated_at = {now_expr} WHERE dashboard_id = $3 AND workspace_id = $4"
    );

    db_execute!(db, &sql, &old_version.content, &old_version.title, dashboard_id, workspace_id)
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to restore dashboard: {e}")))?;

    // Create new version for the restore
    let new_version = create_version(
        db,
        dashboard_id,
        &old_version.content,
        &old_version.title,
        user_id,
        Some(&format!("Restored from version {version_number}")),
    )
    .await?;

    tracing::info!(
        dashboard_id = %dashboard_id,
        restored_from = version_number,
        new_version = new_version,
        "Restored dashboard version"
    );
    Ok(new_version)
}

// ─── Background embedding generation ─────────────────────────────────────────

/// Spawn background embedding generation for a dashboard.
///
/// Generates an embedding from `"{title}\n{content}"` and stores it on the
/// dashboard row. Fire-and-forget — errors are logged but don't propagate.
pub fn spawn_embedding_generation(
    db: DbPool,
    embedding_svc: kyomi_embed::EmbeddingService,
    dashboard_id: String,
    workspace_id: String,
    title: String,
    content: String,
) {
    tokio::spawn(async move {
        let text = format!("{title}\n{content}");
        match embedding_svc.embed_one(&text) {
            Ok(vec) => {
                let embedding_bytes = embedding_to_bytes(&vec);
                let result = match &db {
                    kyomi_core::db::DbPool::Postgres(pg) => {
                        let pg_vec = Vector::from(bytes_to_embedding(&embedding_bytes));
                        sqlx::query(
                            "UPDATE dashboards SET embedding = $1::vector WHERE dashboard_id = $2 AND workspace_id = $3",
                        )
                        .bind(&pg_vec)
                        .bind(&dashboard_id)
                        .bind(&workspace_id)
                        .execute(pg)
                        .await
                        .map(|_| ())
                    }
                    kyomi_core::db::DbPool::Sqlite(sq) => {
                        sqlx::query(
                            "UPDATE dashboards SET embedding = $1 WHERE dashboard_id = $2 AND workspace_id = $3",
                        )
                        .bind(&embedding_bytes)
                        .bind(&dashboard_id)
                        .bind(&workspace_id)
                        .execute(sq)
                        .await
                        .map(|_| ())
                    }
                };

                match result {
                    Ok(_) => tracing::info!(dashboard_id = %dashboard_id, "Stored dashboard embedding"),
                    Err(e) => tracing::error!(dashboard_id = %dashboard_id, error = %e, "Failed to store dashboard embedding"),
                }
            }
            Err(e) => {
                tracing::error!(dashboard_id = %dashboard_id, error = %e, "Failed to generate dashboard embedding");
            }
        }
    });
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_dashboard_content ───────────────────────────────────────

    #[test]
    fn test_validate_no_chartml_passes() {
        // Pure markdown with no ChartML blocks is valid
        let content = "# My Dashboard\n\nSome text content.";
        assert!(validate_dashboard_content(content).is_ok());
    }

    #[test]
    fn test_validate_valid_chartml_passes() {
        let content = r#"# Dashboard

```chartml
data:
  datasource: my-db
  query: "SELECT 1"
visualize:
  type: bar
  columns: x
  rows: y
```
"#;
        assert!(validate_dashboard_content(content).is_ok());
    }

    #[test]
    fn test_validate_invalid_yaml_fails() {
        let content = "```chartml\n: invalid: yaml: [[\n```";
        let result = validate_dashboard_content(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid ChartML YAML"), "got: {err}");
    }

    #[test]
    fn test_validate_missing_visualize_fails() {
        let content = r#"```chartml
data:
  datasource: my-db
  query: "SELECT 1"
```"#;
        let result = validate_dashboard_content(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("visualize"), "got: {err}");
    }

    #[test]
    fn test_validate_missing_data_fails() {
        let content = r#"```chartml
visualize:
  type: bar
```"#;
        let result = validate_dashboard_content(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("data"), "got: {err}");
    }

    // ── generate_change_summary ──────────────────────────────────────────

    #[test]
    fn test_change_summary_initial_version() {
        assert_eq!(generate_change_summary("", "new content"), "Initial version");
    }

    #[test]
    fn test_change_summary_chart_added() {
        let old = "# Dashboard\nSome text";
        let new = "# Dashboard\nSome text\n```chartml\ndata:\n  x: 1\nvisualize:\n  type: bar\n```";
        assert_eq!(generate_change_summary(old, new), "Added 1 chart(s)");
    }

    #[test]
    fn test_change_summary_chart_removed() {
        let old = "# Dashboard\n```chartml\ndata:\n  x: 1\nvisualize:\n  type: bar\n```";
        let new = "# Dashboard\nText only now";
        assert_eq!(generate_change_summary(old, new), "Removed 1 chart(s)");
    }

    #[test]
    fn test_change_summary_added_content() {
        let old = "line1\nline2\nline3";
        let new = "line1\nline2\nline3\nline4\nline5";
        assert_eq!(generate_change_summary(old, new), "Added content");
    }

    #[test]
    fn test_change_summary_net_lines_added() {
        // Both additions and removals, but net positive
        let old = "line1\nline2\nline3";
        let new = "line1\nline2_modified\nline3\nline4\nline5";
        assert_eq!(
            generate_change_summary(old, new),
            "Updated dashboard (+2 lines)"
        );
    }

    #[test]
    fn test_change_summary_removed_content() {
        let old = "line1\nline2\nline3";
        let new = "line1";
        assert_eq!(generate_change_summary(old, new), "Removed content");
    }

    #[test]
    fn test_change_summary_same_content() {
        let content = "line1\nline2\nline3";
        assert_eq!(
            generate_change_summary(content, content),
            "Updated dashboard"
        );
    }

    // ── extract_summary ──────────────────────────────────────────────────

    #[test]
    fn test_extract_summary_present() {
        let content =
            "<!-- dashboard-summary: Tracks monthly sales performance -->\n# Sales Dashboard";
        assert_eq!(
            extract_summary(content),
            Some("Tracks monthly sales performance".into())
        );
    }

    #[test]
    fn test_extract_summary_absent() {
        let content = "# Sales Dashboard\nNo summary here.";
        assert_eq!(extract_summary(content), None);
    }

    // ── title validation ────────────────────────────────────────────────

    #[test]
    fn test_title_too_short() {
        assert!(validate_title("ab").is_err());
    }

    #[test]
    fn test_title_too_long() {
        let long_title = "x".repeat(256);
        assert!(validate_title(&long_title).is_err());
    }

    #[test]
    fn test_title_valid() {
        assert!(validate_title("My Dashboard").is_ok());
    }

    #[test]
    fn test_title_minimum_valid() {
        assert!(validate_title("abc").is_ok());
    }

    #[test]
    fn test_title_maximum_valid() {
        let title = "x".repeat(255);
        assert!(validate_title(&title).is_ok());
    }
}

// ─── Contract tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod contract_tests {
    use super::*;

    // ── ChartML validation edge cases ────────────────────────────────────

    #[test]
    fn validate_multiple_chartml_blocks_all_valid() {
        let content = r#"# Dashboard

```chartml
data:
  datasource: db1
  query: "SELECT 1"
visualize:
  type: bar
  columns: x
  rows: y
```

Some text between charts.

```chartml
data:
  datasource: db2
  query: "SELECT 2"
visualize:
  type: line
  columns: date
  rows: revenue
```
"#;
        assert!(validate_dashboard_content(content).is_ok());
    }

    #[test]
    fn validate_multiple_chartml_blocks_one_invalid() {
        let content = r#"# Dashboard

```chartml
data:
  datasource: db1
  query: "SELECT 1"
visualize:
  type: bar
  columns: x
  rows: y
```

```chartml
data:
  datasource: db2
  query: "SELECT 2"
```
"#;
        // Second block missing 'visualize' key
        let result = validate_dashboard_content(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("visualize"), "got: {err}");
    }

    #[test]
    fn validate_chartml_with_nested_yaml() {
        let content = r#"```chartml
data:
  datasource: my-db
  query: |
    SELECT region, SUM(revenue) as revenue
    FROM sales
    GROUP BY region
visualize:
  type: bar
  columns: region
  rows: revenue
  axes:
    rows:
      label: "Revenue ($)"
      format: "$,.0f"
  style:
    title: "Revenue by Region"
```"#;
        assert!(validate_dashboard_content(content).is_ok());
    }

    #[test]
    fn validate_empty_chartml_block_is_invalid_yaml() {
        let content = "```chartml\n\n```";
        // Empty block cannot be a valid YAML mapping with data + visualize
        // serde_yaml will parse empty string as Null, not a mapping
        let result = validate_dashboard_content(content);
        // Empty YAML parses as Null, which is not a mapping — should fail
        assert!(result.is_err());
    }

    #[test]
    fn validate_whitespace_only_chartml_block() {
        let content = "```chartml\n   \n   \n```";
        let result = validate_dashboard_content(content);
        // Whitespace-only YAML parses as Null, not a mapping
        assert!(result.is_err());
    }

    #[test]
    fn validate_content_with_no_chartml_blocks_is_always_valid() {
        assert!(validate_dashboard_content("").is_ok());
        assert!(validate_dashboard_content("# Title\n\nSome paragraph.").is_ok());
        assert!(validate_dashboard_content("```python\nprint('hello')\n```").is_ok());
    }

    #[test]
    fn validate_multiple_blocks_both_invalid_reports_error() {
        let content = "```chartml\ndata: {}\n```\n\n```chartml\nvisualize: bar\n```";
        let result = validate_dashboard_content(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        // Error should reference what's wrong (missing visualize or missing data)
        assert!(
            err.contains("visualize") || err.contains("data"),
            "Error should reference the missing key, got: {err}"
        );
    }

    #[test]
    fn validate_chartml_yaml_array_of_charts() {
        // ChartML blocks can contain a YAML array of multiple charts
        let content = r#"```chartml
- type: chart
  version: 1
  title: Metric A
  data:
    datasource: my-db
    query: "SELECT COUNT(*) as total FROM users"
  visualize:
    type: metric
    value: total
    label: Total Users
- type: chart
  version: 1
  title: Chart B
  data:
    datasource: my-db
    query: "SELECT date, revenue FROM sales"
  visualize:
    type: line
    columns: date
    rows: revenue
```"#;
        assert!(validate_dashboard_content(content).is_ok());
    }

    #[test]
    fn validate_chartml_yaml_array_missing_data_fails() {
        let content = r#"```chartml
- type: chart
  data:
    datasource: my-db
    query: "SELECT 1"
  visualize:
    type: bar
    columns: x
    rows: y
- type: chart
  visualize:
    type: metric
    value: total
```"#;
        let result = validate_dashboard_content(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("data"), "got: {err}");
    }

    // ── Change summary edge cases ───────────────────────────────────────

    #[test]
    fn change_summary_large_diff() {
        let old: String = (0..100).map(|i| format!("old line {i}")).collect::<Vec<_>>().join("\n");
        let new: String = (0..200).map(|i| format!("new line {i}")).collect::<Vec<_>>().join("\n");
        let summary = generate_change_summary(&old, &new);
        // 200 new lines vs 100 old lines — net addition should be reflected
        assert!(!summary.is_empty());
        assert!(
            summary.contains("Added") || summary.contains("Updated"),
            "Summary should indicate content was added/updated, got: {summary}"
        );
    }

    #[test]
    fn change_summary_only_charts_changed() {
        let old = "# Dashboard\nSome text\n```chartml\nold chart\n```";
        let new = "# Dashboard\nSome text\n```chartml\nnew chart\n```\n```chartml\nextra chart\n```";
        let summary = generate_change_summary(old, new);
        assert_eq!(summary, "Added 1 chart(s)");
    }

    #[test]
    fn change_summary_markdown_formatting_preserved() {
        let old = "# Title\n\n- item 1\n- item 2";
        let new = "# Title\n\n- item 1\n- item 2\n- item 3";
        let summary = generate_change_summary(old, new);
        assert_eq!(summary, "Added content");
    }

    #[test]
    fn change_summary_equal_additions_and_removals() {
        // Replace lines so added == removed
        let old = "line1\nold_line\nline3";
        let new = "line1\nnew_line\nline3";
        let summary = generate_change_summary(old, new);
        assert_eq!(summary, "Updated dashboard");
    }

    // ── Title validation edge cases ─────────────────────────────────────

    #[test]
    fn title_with_only_spaces_fails() {
        // "   " (3 spaces) trimmed is empty, which is < 3 chars
        assert!(validate_title("   ").is_err());
    }

    #[test]
    fn title_with_spaces_around_valid_text_passes() {
        // "  abc  " trimmed is "abc" (3 chars) — valid
        assert!(validate_title("  abc  ").is_ok());
    }

    #[test]
    fn title_unicode_passes() {
        assert!(validate_title("Dashboard").is_ok());
        assert!(validate_title("Tableau de bord").is_ok());
    }

    #[test]
    fn title_at_exactly_3_chars() {
        assert!(validate_title("abc").is_ok());
    }

    #[test]
    fn title_at_exactly_255_chars() {
        let title = "x".repeat(255);
        assert!(validate_title(&title).is_ok());
    }

    #[test]
    fn title_at_2_chars_fails() {
        assert!(validate_title("ab").is_err());
    }

    #[test]
    fn title_at_256_chars_fails() {
        let title = "x".repeat(256);
        assert!(validate_title(&title).is_err());
    }

    // ── Extract summary edge cases ──────────────────────────────────────

    #[test]
    fn extract_summary_not_at_start_returns_none() {
        let content = "# Title\n<!-- dashboard-summary: should not match -->";
        assert_eq!(extract_summary(content), None);
    }

    #[test]
    fn extract_summary_with_special_characters() {
        let content = "<!-- dashboard-summary: Revenue > $1M & growing -->\n# Dashboard";
        assert_eq!(
            extract_summary(content),
            Some("Revenue > $1M & growing".into())
        );
    }
}
