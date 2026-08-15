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
use kyomi_core::embedding_compat::{bytes_to_pg_vector, embedding_to_bytes};
use kyomi_core::sql_compat;
use kyomi_core::{db_execute, db_fetch_optional, db_fetch_scalar};
use kyomi_core::models::DocType;
use kyomi_core::{DbPool, Result};
use kyomi_embed::EmbeddingService;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::sync::LazyLock;

use crate::sync_log_service;
use kyomi_types::sync::{SyncActionType, entity_types};
use kyomi_types::CreatedBy;

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
    Regex::new(r"^<!-- dashboard-summary: (.+?) -->\n?").expect("hardcoded regex is valid")
});

/// Regex for extracting ChartML fenced code blocks.
static CHARTML_BLOCK_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)```chartml\n(.*?)```").expect("hardcoded regex is valid")
});

/// Target chunk size in characters (~500 tokens).
const CHUNK_SIZE: usize = 2000;
/// Overlap between adjacent chunks in characters (~100 tokens).
const CHUNK_OVERLAP: usize = 400;

/// Compute a short SHA-256 hash of content (first 16 hex chars).
pub fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..8])
}

/// Build a SQL fragment that restricts dashboard rows to those the requesting
/// user is allowed to see: either they own the dashboard, or it belongs to at
/// least one public collection.
///
/// The fragment starts with ` AND` so it can be appended directly to an
/// existing `WHERE` clause. Callers must alias the dashboards table as `d`.
///
/// `user_id_bind` is the positional bind-variable index ($1, $2, …) for the
/// user_id parameter in the parent query.
///
/// `is_pg` controls boolean literal syntax: Postgres uses `TRUE`, SQLite
/// uses `1` (via [`kyomi_core::sql_compat::bool_true`]).
pub fn visibility_predicate(user_id_bind: u32, is_pg: bool) -> String {
    let bool_val = sql_compat::bool_true(is_pg);
    format!(
        r#" AND (
            d.user_id = ${uid}
            OR EXISTS (
                SELECT 1 FROM collection_dashboards cd
                JOIN collections c ON cd.collection_id = c.id
                WHERE cd.dashboard_id = d.dashboard_id
                AND c.is_public = {bool_val}
            )
        )"#,
        uid = user_id_bind,
        bool_val = bool_val
    )
}

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
///
/// Not a duplicate of `kyomi_knowledge::vector_search::DashboardSearchResult`:
/// that is a minimal id/title/description/score vector-search hit; this is
/// the full dashboard record with popularity and view counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSearchResult {
    pub dashboard_id: String,
    pub user_id: String,
    pub workspace_id: String,
    pub title: String,
    pub content: String,
    pub doc_type: String,
    pub last_change_summary: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub popularity_score: f64,
    pub content_preview: Option<String>,
    pub view_count: i64,
    pub recent_views: i64,
    /// `true` if the dashboard belongs to at least one public collection.
    pub is_publicly_shared: bool,
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

// ─── Sync snapshot helper ────────────────────────────────────────────────────

/// Fetch a dashboard/knowledge row and build a JSON snapshot for the sync log.
///
/// Returns `Ok(None)` if the row genuinely doesn't exist (or isn't visible to
/// `user_id`) and `Err` if the query itself failed. Callers must not collapse
/// these into the same case — a DB error is not the same fact as "not found",
/// and treating them identically is what let `broadcast_dashboard_sync` emit
/// a null-payload Upsert on a transient DB failure (KYO-245).
pub(crate) async fn fetch_dashboard_snapshot(
    db: &DbPool,
    dashboard_id: &str,
    user_id: &str,
) -> Result<Option<serde_json::Value>> {
    #[derive(sqlx::FromRow)]
    struct SnapshotRow {
        dashboard_id: String,
        user_id: String,
        workspace_id: String,
        title: String,
        content: String,
        doc_type: String,
        last_change_summary: Option<String>,
        updated_at: String,
        created_at: String,
        view_count: i64,
        recent_views: i64,
    }

    let is_pg = db.is_postgres();
    let vis = visibility_predicate(3, is_pg);
    let recent_cutoff = Utc::now() - Duration::days(30);

    let sql = format!(
        r#"SELECT d.dashboard_id, d.user_id, d.workspace_id, d.title, d.content,
                  d.doc_type, d.last_change_summary,
                  CAST(d.updated_at AS TEXT) AS updated_at,
                  CAST(d.created_at AS TEXT) AS created_at,
                  COALESCE(v.view_count, 0) AS view_count,
                  COALESCE(v.recent_views, 0) AS recent_views
           FROM dashboards d
           LEFT JOIN (
               SELECT dashboard_id,
                      COUNT(*) AS view_count,
                      SUM(CASE WHEN viewed_at >= $2 THEN 1 ELSE 0 END) AS recent_views
               FROM dashboard_views
               WHERE dashboard_id = $1
               GROUP BY dashboard_id
           ) v ON d.dashboard_id = v.dashboard_id
           WHERE d.dashboard_id = $1{vis}"#
    );

    let row = db_fetch_optional!(
        db,
        SnapshotRow,
        &sql,
        dashboard_id,
        recent_cutoff,
        user_id
    )
    .map_err(|e| {
        tracing::error!(
            dashboard_id,
            error = %e,
            "fetch_dashboard_snapshot: query failed"
        );
        kyomi_core::Error::from(e)
    })?;

    let Some(row) = row else {
        tracing::warn!(
            dashboard_id,
            "fetch_dashboard_snapshot: dashboard not found or not visible to user"
        );
        return Ok(None);
    };

    let summary = extract_summary(&row.content);
    let content_preview = if row.content.is_empty() {
        None
    } else {
        Some(row.content.chars().take(200).collect::<String>())
    };
    Ok(Some(serde_json::json!({
        "dashboard_id": row.dashboard_id,
        "user_id": row.user_id,
        "workspace_id": row.workspace_id,
        "title": row.title,
        "content": row.content,
        "content_preview": content_preview,
        "summary": summary,
        "last_change_summary": row.last_change_summary,
        "doc_type": row.doc_type,
        "updated_at": row.updated_at,
        "created_at": row.created_at,
        "view_count": row.view_count,
        "recent_views": row.recent_views,
    })))
}

// ─── Create dashboard ────────────────────────────────────────────────────────

/// Create a new dashboard or knowledge document.
///
/// Validates title length, validates ChartML content (dashboards only),
/// checks free tier limit, and INSERTs into the `dashboards` table.
///
/// - Knowledge documents skip ChartML validation and compute a `content_hash`.
pub async fn create_dashboard(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
    title: &str,
    content: &str,
    doc_type: DocType,
    embed: Option<&EmbeddingService>,
) -> Result<String> {
    validate_title(title)?;

    // Dashboards: validate ChartML and enforce free tier limit
    if doc_type.is_dashboard() {
        validate_dashboard_content(content)?;

        #[derive(sqlx::FromRow)]
        struct TierRow { subscription_tier: String }
        let tier_row = db_fetch_optional!(
            db,
            TierRow,
            "SELECT subscription_tier FROM workspaces WHERE workspace_id = $1",
            workspace_id
        )?
        .ok_or_else(|| kyomi_core::Error::NotFound("Workspace not found".into()))?;

        if tier_row.subscription_tier == "free" {
            let count = get_dashboard_count(db, workspace_id, Some(user_id)).await?;
            if count >= FREE_TIER_DASHBOARD_LIMIT {
                return Err(kyomi_core::Error::Forbidden(
                    "Free tier is limited to 5 dashboards. Please upgrade to create more dashboards."
                        .into(),
                ));
            }
        }
    }

    let is_pg = db.is_postgres();
    let now_expr = sql_compat::now(is_pg);
    let dashboard_id = format!("{}", uuid::Uuid::new_v4());
    let content_hash = Some(hash_content(content));
    let doc_type_str = doc_type.as_str();

    let sql = format!(
        r#"
        INSERT INTO dashboards
            (dashboard_id, user_id, workspace_id, title, content,
             doc_type, content_hash, created_by, updated_by,
             created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8, {now_expr}, {now_expr})
        "#
    );

    db_execute!(
        db, &sql, &dashboard_id, user_id, workspace_id, title.trim(), content,
        doc_type_str, &content_hash as &Option<String>, user_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to create dashboard: {e}")))?;

    tracing::info!(dashboard_id = %dashboard_id, doc_type = doc_type_str, "Created dashboard");

    // Sync log — best-effort: log a warning and continue on failure. Never
    // write an Insert-type row with `data: None` (KYO-245, mirroring the
    // live-broadcast fix): if the snapshot can't be fetched right after the
    // row was created, skip the write entirely rather than persist a
    // null-payload row — a missing sync_log entry means "not converged
    // yet", recoverable on the next mutation or a full bootstrap, whereas a
    // null-payload Insert row would replay to every future delta consumer.
    {
        let entity_type = if doc_type.is_dashboard() {
            entity_types::DASHBOARD
        } else {
            entity_types::KNOWLEDGE
        };
        match fetch_dashboard_snapshot(db, &dashboard_id, user_id).await {
            Ok(Some(snapshot)) => {
                // Same KYO-245 rule applied to the visibility read (KYO-354):
                // the row was already written above, so returning `Err` here
                // would report failure for a create that actually succeeded.
                // On a failed read, skip the sync_log write entirely rather
                // than guess a visibility value — a missing row means "not
                // converged yet" and is recoverable on the next mutation or a
                // bootstrap, whereas a wrong `Delete`-shaped guess never
                // self-heals for other entity types and a wrong `Insert`
                // guess here would still misroute this one.
                match is_doc_publicly_visible(db, &dashboard_id).await {
                    Ok(is_visible) => {
                        if let Err(e) = sync_log_service::write_sync_entry(
                            db,
                            sync_log_service::SyncEntryParams {
                                entity_type,
                                entity_id: &dashboard_id,
                                workspace_id,
                                action: SyncActionType::Insert,
                                data: Some(snapshot),
                                owner_user_id: Some(user_id),
                                is_workspace_visible: is_visible,
                            },
                        )
                        .await
                        {
                            tracing::warn!(error = %e, dashboard_id = %dashboard_id, "Failed to write sync log entry");
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            dashboard_id = %dashboard_id,
                            error = %e,
                            "sync log: failed to read visibility after create; skipping write"
                        );
                    }
                }
            }
            Ok(None) => {
                tracing::warn!(
                    dashboard_id = %dashboard_id,
                    "sync log: snapshot unavailable immediately after create; skipping write"
                );
            }
            Err(e) => {
                tracing::error!(
                    dashboard_id = %dashboard_id,
                    error = %e,
                    "sync log: failed to fetch snapshot after create; skipping write"
                );
            }
        }
    }

    // Rechunk newly created document in background (if content is non-trivial)
    if let Some(embed_svc) = embed
        && !content.trim().is_empty()
    {
            spawn_rechunk_document(
                db.clone(),
                embed_svc.clone(),
                dashboard_id.clone(),
                content.to_string(),
                workspace_id.to_string(),
            );
    }

    Ok(dashboard_id)
}

// ─── Get dashboard ───────────────────────────────────────────────────────────

/// Fetch a dashboard by ID within a workspace, bypassing visibility filtering.
///
/// Use this only for write operations (`update_dashboard`, `delete_dashboard`,
/// `restore_version`) that need to check ownership regardless of collection
/// visibility.  Read paths must use [`get_dashboard`] instead.
pub(crate) async fn get_dashboard_unchecked(
    db: &DbPool,
    dashboard_id: &str,
    workspace_id: &str,
) -> Result<Option<kyomi_core::models::Dashboard>> {
    let row = db_fetch_optional!(
        db,
        kyomi_core::models::Dashboard,
        r#"
        SELECT dashboard_id, user_id, workspace_id, title, content,
               doc_type, content_hash, last_change_summary,
               created_by, updated_by,
               created_at, updated_at
        FROM dashboards d
        WHERE d.dashboard_id = $1 AND d.workspace_id = $2
        "#,
        dashboard_id,
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get dashboard: {e}")))?;

    Ok(row)
}

/// Fetch a dashboard by ID within a workspace, applying visibility filtering.
///
/// Visibility rules (owner OR in a public collection) are enforced via
/// [`visibility_predicate`].  Write operations that need ownership checks
/// without visibility filtering should call [`get_dashboard_unchecked`].
pub async fn get_dashboard(
    db: &DbPool,
    dashboard_id: &str,
    workspace_id: &str,
    user_id: &str,
) -> Result<Option<kyomi_core::models::Dashboard>> {
    let is_pg = db.is_postgres();
    let vis = visibility_predicate(3, is_pg);
    let sql = format!(
        r#"
        SELECT dashboard_id, user_id, workspace_id, title, content,
               doc_type, content_hash, last_change_summary,
               created_by, updated_by,
               created_at, updated_at
        FROM dashboards d
        WHERE d.dashboard_id = $1 AND d.workspace_id = $2{vis}
        "#
    );
    let row = db_fetch_optional!(
        db,
        kyomi_core::models::Dashboard,
        &sql,
        dashboard_id,
        workspace_id,
        user_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get dashboard: {e}")))?;

    Ok(row)
}

// ─── Update dashboard ────────────────────────────────────────────────────────

/// Parameters for [`update_dashboard`].
pub struct UpdateDashboardParams<'a> {
    pub db: &'a DbPool,
    pub embed: Option<&'a EmbeddingService>,
    pub dashboard_id: &'a str,
    pub workspace_id: &'a str,
    pub user_id: &'a str,
    pub title: Option<&'a str>,
    pub content: Option<&'a str>,
    pub change_summary: Option<&'a str>,
    pub expected_content_hash: Option<&'a str>,
}

/// Update a dashboard (title, content, change_summary).
///
/// Ownership check: only the dashboard owner can update.
/// Before updating, creates a version snapshot of the old state.
/// Auto-generates a change summary if not provided.
///
/// For all document types:
/// - Supports CAS via `expected_content_hash` — returns `Err(Conflict)` on mismatch.
/// - Updates `content_hash` and `updated_by` when content changes.
/// - Triggers rechunking if `embed` is `Some`.
pub async fn update_dashboard(
    params: UpdateDashboardParams<'_>,
) -> Result<bool> {
    let UpdateDashboardParams {
        db,
        embed,
        dashboard_id,
        workspace_id,
        user_id,
        title,
        content,
        change_summary,
        expected_content_hash,
    } = params;
    // Fetch current dashboard for ownership check and version creation
    let current = get_dashboard_unchecked(db, dashboard_id, workspace_id).await?;
    let current = current.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!("Dashboard {dashboard_id} not found"))
    })?;

    if current.user_id != user_id {
        return Err(kyomi_core::Error::Forbidden(
            "Only the dashboard owner can update it".into(),
        ));
    }

    // CAS check for knowledge documents: verify content_hash matches expected
    if let Some(expected) = expected_content_hash {
        let current_hash = current.content_hash.as_deref().unwrap_or("");
        if current_hash != expected {
            return Err(kyomi_core::Error::Conflict(format!(
                "Content hash mismatch: expected {expected}, got {current_hash}. \
                 The document was modified concurrently."
            )));
        }
    }

    // Validate new values
    if let Some(t) = title {
        validate_title(t)?;
    }
    let current_doc_type = current.doc_type();
    // Only validate ChartML for dashboards — knowledge docs are free-form markdown
    if let Some(c) = content
        && current_doc_type.is_dashboard()
    {
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

    // Compute new content_hash for all document types
    let new_content_hash = content.map(hash_content);

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

    // Always set change summary, updated_by, and updated_at
    set_parts.push(format!("last_change_summary = ${param_idx}"));
    param_idx += 1;
    set_parts.push(format!("updated_by = ${param_idx}"));
    param_idx += 1;

    // Set content_hash for knowledge documents
    if new_content_hash.is_some() {
        set_parts.push(format!("content_hash = ${param_idx}"));
        param_idx += 1;
    }

    set_parts.push(format!("updated_at = ${param_idx}"));

    let sql = format!(
        "UPDATE dashboards SET {} WHERE dashboard_id = $1 AND workspace_id = $2",
        set_parts.join(", ")
    );

    let now = Utc::now();
    // Dynamic SQL with variable bind count — identical logic for both backends.
    let rows_affected = kyomi_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query(&sql).bind(dashboard_id).bind(workspace_id);
        if let Some(t) = title { query = query.bind(t.trim()); }
        if let Some(c) = content { query = query.bind(c); }
        query = query.bind(&auto_summary);
        query = query.bind(user_id);
        if let Some(ref hash) = new_content_hash { query = query.bind(hash); }
        query = query.bind(now);
        query.execute(p).await.map(|r| r.rows_affected())
    })
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to update dashboard: {e}"))
    })?;

    tracing::info!(dashboard_id = %dashboard_id, "Updated dashboard");

    // Rechunk documents after content update (all doc types) — fire-and-forget
    if rows_affected > 0
        && let Some(c) = content
    {
        if let Some(embed_svc) = embed {
            spawn_rechunk_document(
                db.clone(),
                embed_svc.clone(),
                dashboard_id.to_string(),
                c.to_string(),
                workspace_id.to_string(),
            );
        } else {
            tracing::warn!(
                dashboard_id = %dashboard_id,
                "Knowledge document updated without EmbeddingService — chunks are now stale"
            );
        }
    }

    // Sync log — best-effort: log a warning and continue on failure. Never
    // write an Update-type row with `data: None` (KYO-245, mirroring the
    // live-broadcast fix): if the snapshot can't be fetched right after the
    // update lands, skip the write entirely rather than persist a
    // null-payload row — see the matching comment in `create_dashboard`.
    if rows_affected > 0 {
        let entity_type = if current_doc_type.is_dashboard() {
            entity_types::DASHBOARD
        } else {
            entity_types::KNOWLEDGE
        };
        match fetch_dashboard_snapshot(db, dashboard_id, user_id).await {
            Ok(Some(snapshot)) => {
                // Same KYO-245 rule applied to the visibility read (KYO-354):
                // the row was already written above, so returning `Err` here
                // would report failure for an update that actually
                // succeeded. On a failed read, skip the sync_log write
                // entirely rather than guess a visibility value — a missing
                // row means "not converged yet" and is recoverable on the
                // next mutation or a bootstrap.
                match is_doc_publicly_visible(db, dashboard_id).await {
                    Ok(is_visible) => {
                        if let Err(e) = sync_log_service::write_sync_entry(
                            db,
                            sync_log_service::SyncEntryParams {
                                entity_type,
                                entity_id: dashboard_id,
                                workspace_id,
                                action: SyncActionType::Update,
                                data: Some(snapshot),
                                owner_user_id: Some(user_id),
                                is_workspace_visible: is_visible,
                            },
                        )
                        .await
                        {
                            tracing::warn!(error = %e, dashboard_id = %dashboard_id, "Failed to write sync log entry");
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            dashboard_id = %dashboard_id,
                            error = %e,
                            "sync log: failed to read visibility after update; skipping write"
                        );
                    }
                }
            }
            Ok(None) => {
                tracing::warn!(
                    dashboard_id = %dashboard_id,
                    "sync log: snapshot unavailable immediately after update; skipping write"
                );
            }
            Err(e) => {
                tracing::error!(
                    dashboard_id = %dashboard_id,
                    error = %e,
                    "sync log: failed to fetch snapshot after update; skipping write"
                );
            }
        }
    }

    Ok(rows_affected > 0)
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
    let current = get_dashboard_unchecked(db, dashboard_id, workspace_id).await?;
    let current = current.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!("Dashboard {dashboard_id} not found"))
    })?;

    if current.user_id != user_id {
        return Err(kyomi_core::Error::Forbidden(
            "Only the dashboard owner can delete it".into(),
        ));
    }

    // Capture visibility before the DELETE — this ordering is load-bearing
    // (KYO-313). `collection_dashboards` CASCADE-deletes with the dashboard,
    // so afterwards no join can recompute the value: the `Delete` row written
    // below is the only surviving record of whether non-owners could see this
    // document, and a deleted entity is never mutated again, so a wrong value
    // never self-heals. Moving this read after the DELETE would silently strip
    // every deletion from non-owners' deltas, stranding the document in their
    // caches forever. Guarded by
    // `delete_of_public_dashboard_reaches_non_owners`.
    //
    // Propagate on error rather than defaulting to a guessed visibility
    // (KYO-354): the read runs before the DELETE, so an `Err` here aborts
    // the operation with nothing destructive having happened yet, and the
    // caller can safely retry.
    let was_visible = is_doc_publicly_visible(db, dashboard_id).await?;

    let result = db_execute!(
        db,
        "DELETE FROM dashboards WHERE dashboard_id = $1 AND workspace_id = $2",
        dashboard_id,
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete dashboard: {e}")))?;

    tracing::info!(dashboard_id = %dashboard_id, "Deleted dashboard");

    // Sync log — best-effort: log a warning and continue on failure.
    if result.rows_affected() > 0 {
        let entity_type = if current.doc_type().is_dashboard() {
            entity_types::DASHBOARD
        } else {
            entity_types::KNOWLEDGE
        };
        if let Err(e) = sync_log_service::write_sync_entry(
            db,
            sync_log_service::SyncEntryParams {
                entity_type,
                entity_id: dashboard_id,
                workspace_id,
                action: SyncActionType::Delete,
                data: None,
                owner_user_id: Some(user_id),
                is_workspace_visible: was_visible,
            },
        )
        .await
        {
            tracing::warn!(error = %e, dashboard_id = %dashboard_id, "Failed to write sync log entry");
        }
    }

    Ok(result.rows_affected() > 0)
}

// ─── Search dashboards ──────────────────────────────────────────────────────

/// Search dashboards by text query with sorting and popularity scoring.
///
/// Uses ILIKE/LIKE text search on title and content. For full hybrid search
/// (BM25 + semantic), the REST endpoint layer can compose this with
/// embedding-based search.
///
/// - `doc_type_filter`: `None` searches all types, `Some(DocType::Dashboard)` or
///   `Some(DocType::Knowledge)` filters by document type.
///
/// Popularity is computed via time-weighted view counts:
/// - Last 7 days: 1.0 weight
/// - Last 30 days: 0.5
/// - Last 90 days: 0.25
/// - Older: 0.1
pub async fn search_dashboards(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
    query: Option<&str>,
    doc_type_filter: Option<DocType>,
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
    let doc_type_str: Option<&str> = doc_type_filter.map(|dt: DocType| dt.as_str());

    // Build the text filter — $5 is query text, $6 is doc_type filter, $7 is user_id (visibility)
    let text_filter = if is_pg {
        "AND ($5::text IS NULL OR d.title ILIKE '%' || $5 || '%' OR d.content ILIKE '%' || $5 || '%')"
    } else {
        "AND ($5 IS NULL OR d.title LIKE '%' || $5 || '%' OR d.content LIKE '%' || $5 || '%')"
    };

    let doc_type_clause = if is_pg {
        "AND ($6::text IS NULL OR d.doc_type = $6)"
    } else {
        "AND ($6 IS NULL OR d.doc_type = $6)"
    };

    let vis = visibility_predicate(7, is_pg);

    // Postgres SUM returns NUMERIC; cast to FLOAT8 so sqlx can decode as f64.
    // SQLite doesn't need the cast (SUM already returns REAL).
    let float_cast = if is_pg { "::FLOAT8" } else { "" };

    let bool_true = sql_compat::bool_true(is_pg);

    // Popularity sub-query with CASE expressions
    // Postgres uses FILTER (WHERE ...) but we use CASE for cross-db compat
    let popularity_sql = format!(
        r#"
        SELECT
            d.dashboard_id, d.user_id, d.workspace_id, d.title, d.content,
            d.doc_type, d.last_change_summary, d.created_at, d.updated_at,
            COALESCE(v.popularity_score, 0.0) AS popularity_score,
            COALESCE(v.view_count, 0) AS view_count,
            COALESCE(v.recent_views, 0) AS recent_views,
            CASE WHEN EXISTS (
                SELECT 1 FROM collection_dashboards cd
                JOIN collections c ON cd.collection_id = c.id
                WHERE cd.dashboard_id = d.dashboard_id
                AND c.is_public = {bool_true}
            ) THEN 1 ELSE 0 END AS is_publicly_shared
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
                ){float_cast} AS popularity_score
            FROM dashboard_views
            WHERE workspace_id = $1
            GROUP BY dashboard_id
        ) v ON d.dashboard_id = v.dashboard_id
        WHERE d.workspace_id = $1
        {text_filter}
        {doc_type_clause}
        {vis}
        "#
    );

    let rows: Vec<DashboardSearchResult> = kyomi_core::db_with_pool!(db, |p| {
        let raw_rows = sqlx::query(&popularity_sql)
            .bind(workspace_id)
            .bind(recent_cutoff)
            .bind(medium_cutoff)
            .bind(old_cutoff)
            .bind(query_param)
            .bind(doc_type_str)
            .bind(user_id)
            .fetch_all(p)
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
                doc_type: row.get("doc_type"),
                last_change_summary: row.get("last_change_summary"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
                popularity_score: row.get::<Option<f64>, _>("popularity_score").unwrap_or(0.0),
                content_preview: preview,
                view_count: row.get::<Option<i64>, _>("view_count").unwrap_or(0),
                recent_views: row.get::<Option<i64>, _>("recent_views").unwrap_or(0),
                is_publicly_shared: row.get::<i32, _>("is_publicly_shared") != 0,
            }
        }).collect()
    });

    let mut results = rows;

    // Sort in Rust to avoid 3 duplicate SQL queries for each ORDER BY variant
    match sort_by {
        SearchSort::Popularity => results.sort_by(|a, b| {
            b.popularity_score
                .partial_cmp(&a.popularity_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.updated_at.cmp(&a.updated_at))
        }),
        SearchSort::Recent => results.sort_by_key(|b| std::cmp::Reverse(b.updated_at)),
        SearchSort::Created => results.sort_by_key(|b| std::cmp::Reverse(b.created_at)),
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
            "SELECT COUNT(*) FROM dashboards WHERE workspace_id = $1 AND user_id = $2 AND doc_type = 'dashboard'",
            workspace_id,
            uid
        )
    } else {
        db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM dashboards WHERE workspace_id = $1 AND doc_type = 'dashboard'",
            workspace_id
        )
    }
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to count dashboards: {e}")))?;

    Ok(count)
}

/// Count documents in a workspace, visibility-scoped to `user_id` and
/// optionally filtered by `doc_type`.
///
/// The count only includes rows `user_id` may actually see — the same
/// `visibility_predicate` used by [`search_dashboards`] (owned by `user_id`,
/// or a member of a public collection). This exists because the caller
/// (the `search_dashboards` agent tool) surfaces the number directly to the
/// requesting user as `total_workspace_documents`, both in chat and over
/// MCP; an unfiltered workspace-wide count would let a member infer the
/// existence and volume of other members' private dashboards and knowledge
/// docs (KYO-181), and would also be inconsistent with the result set the
/// caller can actually see, since that result set is itself
/// visibility-filtered.
///
/// Unlike `get_dashboard_count` (which always counts `doc_type='dashboard'`,
/// unfiltered by visibility, across the whole workspace), this counts
/// documents of any type — or only the specified type when `doc_type_filter`
/// is `Some`. `get_dashboard_count`'s unfiltered count is intentional: its
/// sole caller uses it for a free-tier *limit* check (`count >=
/// FREE_TIER_DASHBOARD_LIMIT`), which must count every dashboard in the
/// workspace regardless of who is asking — the number itself is never
/// rendered back to a user, only a pass/fail decision. That is a different
/// contract from this function, whose return value is displayed as-is.
///
/// Never widen this function to drop the visibility filter — see KYO-181
/// and the "workspace_id is not an authorization boundary for dashboards
/// reads" rule in `docs/CODING_STANDARDS.md`.
pub async fn get_document_count(
    db: &DbPool,
    workspace_id: &str,
    doc_type_filter: Option<DocType>,
    user_id: &str,
) -> Result<i64> {
    let is_pg = db.is_postgres();

    let count: i64 = if let Some(dt) = doc_type_filter {
        // $1 = workspace_id, $2 = doc_type, so the visibility predicate's
        // user_id bind is $3.
        let vis = visibility_predicate(3, is_pg);
        let sql = format!(
            "SELECT COUNT(*) FROM dashboards d WHERE d.workspace_id = $1 AND d.doc_type = $2{vis}"
        );
        db_fetch_scalar!(db, i64, &sql, workspace_id, dt.as_str(), user_id)
    } else {
        // $1 = workspace_id only, so the visibility predicate's user_id
        // bind is $2.
        let vis = visibility_predicate(2, is_pg);
        let sql = format!("SELECT COUNT(*) FROM dashboards d WHERE d.workspace_id = $1{vis}");
        db_fetch_scalar!(db, i64, &sql, workspace_id, user_id)
    }
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to count documents: {e}")))?;

    Ok(count)
}

// ─── Rechunking (knowledge documents) ───────────────────────────────────────

/// Rechunk a knowledge document: delete old chunks, split content into
/// fixed-size chunks with overlap, embed each chunk, and insert into
/// `knowledge_chunks`. Also extracts and stores table references in
/// `knowledge_file_tables`.
///
/// The `knowledge_chunks` and `knowledge_file_tables` tables use `dashboard_id`
/// as the foreign key (renamed from `file_id` during the knowledge unification migration).
pub async fn rechunk_document(
    db: &DbPool,
    embed: &EmbeddingService,
    dashboard_id: &str,
    content: &str,
    workspace_id: &str,
) -> Result<()> {
    use kyomi_knowledge::knowledge_files::{extract_table_references, split_into_chunks};

    if content.trim().is_empty() {
        // No content — just delete old chunks and table refs
        db_execute!(
            db,
            "DELETE FROM knowledge_chunks WHERE dashboard_id = $1",
            dashboard_id
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete chunks: {e}")))?;
        db_execute!(
            db,
            "DELETE FROM knowledge_file_tables WHERE dashboard_id = $1",
            dashboard_id
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete table refs: {e}")))?;
        return Ok(());
    }

    // Split into chunks
    let chunks = split_into_chunks(content, CHUNK_SIZE, CHUNK_OVERLAP);

    if chunks.is_empty() {
        return Ok(());
    }

    // Embed BEFORE deleting old chunks — if embedding fails, old chunks remain intact
    let chunk_refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
    let embeddings = embed
        .embed_passages(&chunk_refs)
        .map_err(|e| kyomi_core::Error::Internal(format!("embedding failed: {e}")))?;

    if embeddings.len() != chunks.len() {
        return Err(kyomi_core::Error::Internal(format!(
            "BUG: embedding count {} != chunk count {}",
            embeddings.len(),
            chunks.len()
        )));
    }

    // Extract table references before the transaction
    let table_refs = extract_table_references(content);

    // The chunk INSERT text is identical across backends except the embedding
    // placeholder's `::vector` cast — build it once and bind the embedding
    // value differently per backend below (pgvector::Vector only implements
    // sqlx's Encode for Postgres, see embedding_compat::bytes_to_pg_vector).
    let is_pg = db.is_postgres();
    let chunk_insert_sql = format!(
        "INSERT INTO knowledge_chunks \
            (id, dashboard_id, workspace_id, content, chunk_index, embedding) \
         VALUES ($1, $2, $3, $4, $5, {embedding})",
        embedding = sql_compat::embedding_placeholder(is_pg, "$6"),
    );

    // Wrap delete + insert in a transaction for atomicity — if any insert
    // fails, the old chunks remain intact rather than leaving partial state.
    match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let mut tx = pg.begin().await.map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to begin transaction: {e}"))
            })?;

            sqlx::query("DELETE FROM knowledge_chunks WHERE dashboard_id = $1")
                .bind(dashboard_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete chunks: {e}")))?;
            sqlx::query("DELETE FROM knowledge_file_tables WHERE dashboard_id = $1")
                .bind(dashboard_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete table refs: {e}")))?;

            for (i, (chunk_text, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
                let chunk_id = uuid::Uuid::new_v4().to_string();
                let vector = pgvector::Vector::from(embedding.clone());
                sqlx::query(&chunk_insert_sql)
                    .bind(&chunk_id)
                    .bind(dashboard_id)
                    .bind(workspace_id)
                    .bind(chunk_text)
                    .bind(i as i32)
                    .bind(&vector)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| kyomi_core::Error::Internal(format!("failed to insert chunk: {e}")))?;
            }

            for table_ref in &table_refs {
                sqlx::query(
                    "INSERT INTO knowledge_file_tables (dashboard_id, workspace_id, table_full_name) \
                     VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
                )
                .bind(dashboard_id)
                .bind(workspace_id)
                .bind(table_ref)
                .execute(&mut *tx)
                .await
                .map_err(|e| kyomi_core::Error::Internal(format!("failed to insert table ref: {e}")))?;
            }

            tx.commit().await.map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to commit rechunk transaction: {e}"))
            })?;
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let mut tx = sq.begin().await.map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to begin transaction: {e}"))
            })?;

            sqlx::query("DELETE FROM knowledge_chunks WHERE dashboard_id = $1")
                .bind(dashboard_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete chunks: {e}")))?;
            sqlx::query("DELETE FROM knowledge_file_tables WHERE dashboard_id = $1")
                .bind(dashboard_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete table refs: {e}")))?;

            for (i, (chunk_text, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
                let chunk_id = uuid::Uuid::new_v4().to_string();
                let emb_bytes = embedding_to_bytes(embedding);
                sqlx::query(&chunk_insert_sql)
                    .bind(&chunk_id)
                    .bind(dashboard_id)
                    .bind(workspace_id)
                    .bind(chunk_text)
                    .bind(i as i32)
                    .bind(&emb_bytes)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| kyomi_core::Error::Internal(format!("failed to insert chunk: {e}")))?;
            }

            for table_ref in &table_refs {
                sqlx::query(
                    "INSERT INTO knowledge_file_tables (dashboard_id, workspace_id, table_full_name) \
                     VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
                )
                .bind(dashboard_id)
                .bind(workspace_id)
                .bind(table_ref)
                .execute(&mut *tx)
                .await
                .map_err(|e| kyomi_core::Error::Internal(format!("failed to insert table ref: {e}")))?;
            }

            tx.commit().await.map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to commit rechunk transaction: {e}"))
            })?;
        }
    }

    tracing::debug!(
        dashboard_id,
        chunks = chunks.len(),
        table_refs = table_refs.len(),
        "Rechunked knowledge document"
    );

    Ok(())
}

// ─── Record view ─────────────────────────────────────────────────────────────

/// Record a dashboard view for popularity tracking.
///
/// Silently skips recording if the user does not have visibility access to the
/// dashboard — reuses the same predicate as all other read paths so no orphan
/// view rows are inserted for dashboards the user cannot actually see.
pub async fn record_view(
    db: &DbPool,
    dashboard_id: &str,
    user_id: &str,
    workspace_id: &str,
) -> Result<()> {
    // Skip recording if the user can't see this dashboard.
    let visible = get_dashboard(db, dashboard_id, workspace_id, user_id).await?;
    if visible.is_none() {
        return Ok(());
    }

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
    // MAX() is an aggregate that always returns exactly one row (NULL when no rows exist).
    // db_fetch_scalar! uses fetch_one, which is correct here — SELECT MAX(...) never
    // returns zero rows, so fetch_one returns Ok(None) rather than Err(RowNotFound).
    let max_version: Option<i32> = db_fetch_scalar!(
        db,
        Option<i32>,
        "SELECT MAX(version_number) FROM dashboard_versions WHERE dashboard_id = $1",
        dashboard_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get max version: {e}")))?;

    let max_version = max_version.unwrap_or(0);

    // SHA-256 dedup
    let content_hash = format!("{:x}", Sha256::digest(content.as_bytes()));

    if max_version > 0 {
        let latest_hash: Option<String> = kyomi_core::db_with_pool!(db, |p| {
            sqlx::query_scalar::<_, String>(
                "SELECT content_hash FROM dashboard_versions WHERE dashboard_id = $1 AND version_number = $2",
            )
            .bind(dashboard_id)
            .bind(max_version)
            .fetch_optional(p)
            .await
        })
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

    let versions: Vec<DashboardVersionSummary> = kyomi_core::db_with_pool!(db, |p| {
        let rows = sqlx::query(sql)
            .bind(dashboard_id)
            .bind(limit)
            .bind(offset)
            .fetch_all(p)
            .await
            .map_err(|e| kyomi_core::Error::Internal(format!("failed to list versions: {e}")))?;
        rows.iter().map(version_summary_from_row).collect()
    });

    Ok(versions)
}

/// Extract a [`DashboardVersionSummary`] from any sqlx row type.
fn version_summary_from_row<'r, R>(row: &'r R) -> DashboardVersionSummary
where
    R: sqlx::Row,
    &'r str: sqlx::ColumnIndex<R>,
    String: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    Option<String>: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    i32: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    Option<i32>: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    DateTime<Utc>: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
{
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
            email: Some(
                row.get::<Option<String>, _>("email")
                    .unwrap_or_else(|| "(deleted user)".into()),
            ),
            ..Default::default()
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

// ---------------------------------------------------------------------------
// Version diff (KYO-124)
// ---------------------------------------------------------------------------

/// A single line of a dashboard version diff.
///
/// `line_type` is one of `"add"`, `"delete"`, or `"context"` (unchanged).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardDiffLine {
    pub line_type: String,
    pub content: String,
}

/// The full result of diffing two versions of a dashboard.
///
/// Returned by [`diff_versions`]. Both the Leptos server_fn and the REST
/// handler map this into their own response shapes — the service function
/// owns the underlying algorithm and the "current version" short-circuit.
#[derive(Debug, Clone)]
pub struct DashboardVersionDiff {
    pub dashboard_id: String,
    pub from_version: i32,
    pub to_version: i32,
    pub from_title: String,
    pub to_title: String,
    pub additions: i32,
    pub deletions: i32,
    pub diff_lines: Vec<DashboardDiffLine>,
}

/// Compute a line-based diff between two strings using the Myers algorithm.
///
/// Returns `(additions, deletions, diff_lines)`. Trailing newlines on each
/// line are stripped so callers don't double up on line-breaks when
/// rendering. Pure function — no I/O, safe to unit-test in isolation.
fn compute_line_diff(
    from_content: &str,
    to_content: &str,
) -> (i32, i32, Vec<DashboardDiffLine>) {
    let diff = similar::TextDiff::from_lines(from_content, to_content);
    let mut additions = 0i32;
    let mut deletions = 0i32;
    let mut diff_lines = Vec::new();

    for change in diff.iter_all_changes() {
        let line_type = match change.tag() {
            similar::ChangeTag::Insert => {
                additions += 1;
                "add"
            }
            similar::ChangeTag::Delete => {
                deletions += 1;
                "delete"
            }
            similar::ChangeTag::Equal => "context",
        };
        diff_lines.push(DashboardDiffLine {
            line_type: line_type.to_string(),
            content: change.value().trim_end_matches('\n').to_string(),
        });
    }

    (additions, deletions, diff_lines)
}

/// Diff two versions of a dashboard.
///
/// Fetches the live dashboard (also verifies workspace ownership) and, for
/// each side, pulls the matching version's content — or the live content
/// when the version number is `max_version + 1` (the "current" sentinel).
/// Runs a Myers line diff and returns the aggregated result.
///
/// Returns `Error::NotFound` if the dashboard doesn't exist or belongs to
/// another workspace, or if either version number isn't on disk (and isn't
/// the current sentinel).
///
/// This is the single source of truth for the dashboard version-diff
/// orchestration. The Leptos `server_fn` at
/// `crates/kyomi-ui/src/server_fns/dashboards.rs::diff_versions` delegates
/// here; the REST handler that used to share this logic was deleted
/// wholesale in the React→Leptos migration (KYO-73, #182).
pub async fn diff_versions(
    pool: &DbPool,
    dashboard_id: &str,
    workspace_id: &str,
    user_id: &str,
    from_version: i32,
    to_version: i32,
) -> Result<DashboardVersionDiff> {
    let dashboard = get_dashboard(pool, dashboard_id, workspace_id, user_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::NotFound(format!("Dashboard {dashboard_id} not found"))
        })?;

    let version_count = get_version_count(pool, dashboard_id).await? as i32;
    let current_version_number = version_count + 1;

    let (from_content, from_title) = if from_version == current_version_number {
        (dashboard.content.clone(), dashboard.title.clone())
    } else {
        let v = get_version(pool, dashboard_id, from_version)
            .await?
            .ok_or_else(|| {
                kyomi_core::Error::NotFound(format!("Version {from_version} not found"))
            })?;
        (v.content, v.title)
    };

    let (to_content, to_title) = if to_version == current_version_number {
        (dashboard.content.clone(), dashboard.title.clone())
    } else {
        let v = get_version(pool, dashboard_id, to_version)
            .await?
            .ok_or_else(|| {
                kyomi_core::Error::NotFound(format!("Version {to_version} not found"))
            })?;
        (v.content, v.title)
    };

    let (additions, deletions, diff_lines) = compute_line_diff(&from_content, &to_content);

    Ok(DashboardVersionDiff {
        dashboard_id: dashboard_id.to_string(),
        from_version,
        to_version,
        from_title,
        to_title,
        additions,
        deletions,
        diff_lines,
    })
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
    let current = get_dashboard_unchecked(db, dashboard_id, workspace_id).await?;
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

/// Generate an embedding from `"{title}\n{content}"` and store it on the
/// dashboard row identified by `dashboard_id`/`workspace_id`.
///
/// This is the awaitable core of [`spawn_embedding_generation`], extracted so
/// tests can `.await` the real bind path directly instead of racing a
/// detached `tokio::spawn`. It owns all three production log outcomes
/// (embed success/store, DB write failure, embed failure) itself, so the
/// `tokio::spawn` wrapper doesn't need to inspect the returned `Result` to
/// reproduce today's log output — it only needs to drive the future.
async fn store_dashboard_embedding(
    db: &DbPool,
    embedding_svc: &EmbeddingService,
    dashboard_id: &str,
    workspace_id: &str,
    title: &str,
    content: &str,
) -> Result<()> {
    let text = format!("{title}\n{content}");
    let vec = match embedding_svc.embed_one(&text) {
        Ok(vec) => vec,
        Err(e) => {
            tracing::error!(dashboard_id = %dashboard_id, error = %e, "Failed to generate dashboard embedding");
            return Err(kyomi_core::Error::Internal(format!(
                "failed to generate dashboard embedding: {e}"
            )));
        }
    };

    let embedding_bytes = embedding_to_bytes(&vec);
    let is_pg = db.is_postgres();
    let sql = format!(
        "UPDATE dashboards SET embedding = {embedding} WHERE dashboard_id = $2 AND workspace_id = $3",
        embedding = sql_compat::embedding_placeholder(is_pg, "$1"),
    );
    let result = match db {
        kyomi_core::db::DbPool::Postgres(pg) => sqlx::query(&sql)
            .bind(bytes_to_pg_vector(&embedding_bytes))
            .bind(dashboard_id)
            .bind(workspace_id)
            .execute(pg)
            .await
            .map(|_| ()),
        kyomi_core::db::DbPool::Sqlite(sq) => sqlx::query(&sql)
            .bind(&embedding_bytes)
            .bind(dashboard_id)
            .bind(workspace_id)
            .execute(sq)
            .await
            .map(|_| ()),
    };

    match result {
        Ok(()) => {
            tracing::info!(dashboard_id = %dashboard_id, "Stored dashboard embedding");
            Ok(())
        }
        Err(e) => {
            tracing::error!(dashboard_id = %dashboard_id, error = %e, "Failed to store dashboard embedding");
            Err(kyomi_core::Error::Internal(format!(
                "failed to store dashboard embedding: {e}"
            )))
        }
    }
}

/// Spawn background embedding generation for a dashboard.
///
/// Generates an embedding from `"{title}\n{content}"` and stores it on the
/// dashboard row. Fire-and-forget — errors are logged (by
/// [`store_dashboard_embedding`]) but don't propagate.
pub fn spawn_embedding_generation(
    db: DbPool,
    embedding_svc: kyomi_embed::EmbeddingService,
    dashboard_id: String,
    workspace_id: String,
    title: String,
    content: String,
) {
    tokio::spawn(async move {
        let _ = store_dashboard_embedding(
            &db,
            &embedding_svc,
            &dashboard_id,
            &workspace_id,
            &title,
            &content,
        )
        .await;
    });
}

// ─── Background rechunking ───────────────────────────────────────────────────

/// Spawn background rechunking for a document.
///
/// Fire-and-forget — errors are logged but don't propagate. Parameters are
/// owned because they're moved into the spawned future.
pub fn spawn_rechunk_document(
    db: DbPool,
    embed: EmbeddingService,
    dashboard_id: String,
    content: String,
    workspace_id: String,
) {
    tokio::spawn(async move {
        if let Err(e) = rechunk_document(&db, &embed, &dashboard_id, &content, &workspace_id).await {
            tracing::error!(dashboard_id = %dashboard_id, "Background rechunking failed: {e}");
        }
    });
}

// ─── Sync helpers ─────────────────────────────────────────────────────────────

/// List all dashboards (doc_type = 'dashboard') for a workspace that the user
/// can see, returning list-level metadata as JSON values for the sync bootstrap
/// protocol. Only docs owned by the user or in a public collection are included.
pub async fn list_dashboards_for_sync(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> Result<Vec<serde_json::Value>> {
    list_docs_for_sync(db, workspace_id, user_id, "dashboard").await
}

/// List all knowledge documents (doc_type = 'knowledge') for a workspace that
/// the user can see, returning list-level metadata as JSON values for the sync
/// bootstrap protocol. Only docs owned by the user or in a public collection are
/// included.
pub async fn list_knowledge_for_sync(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> Result<Vec<serde_json::Value>> {
    list_docs_for_sync(db, workspace_id, user_id, "knowledge").await
}

/// Shared implementation for list_dashboards_for_sync / list_knowledge_for_sync.
async fn list_docs_for_sync(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
    doc_type: &str,
) -> Result<Vec<serde_json::Value>> {
    #[derive(sqlx::FromRow)]
    struct DocSyncRow {
        dashboard_id: String,
        user_id: String,
        workspace_id: String,
        title: String,
        content: String,
        last_change_summary: Option<String>,
        updated_at: String,
        created_at: String,
        doc_type: String,
        view_count: i64,
        recent_views: i64,
    }

    let recent_cutoff = Utc::now() - Duration::days(30);
    let is_pg = db.is_postgres();

    // Build SQL with LEFT JOIN on dashboard_views to include view metrics.
    // Parameter numbering: $1 = workspace_id, $2 = recent_cutoff, $3 = doc_type,
    // $4 = user_id (injected via visibility_predicate).
    // SUM(CASE WHEN ...) avoids FILTER (WHERE ...) which is Postgres-only.
    let vis = visibility_predicate(4, is_pg);
    let sql = format!(r#"
        SELECT d.dashboard_id, d.user_id, d.workspace_id, d.title, d.content,
               d.last_change_summary,
               CAST(d.updated_at AS TEXT) AS updated_at,
               CAST(d.created_at AS TEXT) AS created_at,
               d.doc_type,
               COALESCE(v.view_count, 0) AS view_count,
               COALESCE(v.recent_views, 0) AS recent_views
        FROM dashboards d
        LEFT JOIN (
            SELECT
                dashboard_id,
                COUNT(*) AS view_count,
                SUM(CASE WHEN viewed_at >= $2 THEN 1 ELSE 0 END) AS recent_views
            FROM dashboard_views
            WHERE workspace_id = $1
            GROUP BY dashboard_id
        ) v ON d.dashboard_id = v.dashboard_id
        WHERE d.workspace_id = $1 AND d.doc_type = $3{vis}
        ORDER BY d.updated_at DESC
    "#);

    let rows: Vec<DocSyncRow> = kyomi_core::db_fetch_all!(
        db,
        DocSyncRow,
        &sql,
        workspace_id,
        recent_cutoff,
        doc_type,
        user_id
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to list {doc_type} docs for sync: {e}"))
    })?;

    let values = rows
        .into_iter()
        .map(|row| {
            let summary = extract_summary(&row.content);
            let content_preview = if row.content.is_empty() {
                None
            } else {
                Some(row.content.chars().take(200).collect::<String>())
            };
            serde_json::json!({
                "dashboard_id": row.dashboard_id,
                "user_id": row.user_id,
                "workspace_id": row.workspace_id,
                "title": row.title,
                "content": row.content,
                "content_preview": content_preview,
                "summary": summary,
                "last_change_summary": row.last_change_summary,
                "updated_at": row.updated_at,
                "created_at": row.created_at,
                "doc_type": row.doc_type,
                "view_count": row.view_count,
                "recent_views": row.recent_views,
            })
        })
        .collect();

    Ok(values)
}

/// Check whether a dashboard is in any public collection.
///
/// Used by the live-sync broadcast path and every `sync_log` write site to
/// decide whether a doc-mutation event goes to all workspace members
/// (public) or only to the document owner (private). A query error is not a
/// visibility answer — collapsing it into `false` silently writes an
/// incorrect "private" row that, for a `Delete` action, can never be
/// corrected afterward (the entity is gone, so nothing will ever mutate it
/// again). Callers must handle the error explicitly rather than receive a
/// guessed boolean.
pub(crate) async fn is_doc_publicly_visible(
    db: &DbPool,
    dashboard_id: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = db.is_postgres();
    let bool_val = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT 1 AS n FROM collection_dashboards cd \
         JOIN collections c ON cd.collection_id = c.id \
         WHERE cd.dashboard_id = $1 AND c.is_public = {bool_val} \
         LIMIT 1"
    );

    let row = kyomi_core::db_fetch_optional!(db, (i32,), &sql, dashboard_id).map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to read document visibility: {e}"))
    })?;

    Ok(row.is_some())
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

    // ── hash_content ────────────────────────────────────────────────────

    #[test]
    fn test_hash_content_deterministic() {
        let h1 = hash_content("test content");
        let h2 = hash_content("test content");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_content_different_inputs() {
        let h1 = hash_content("content a");
        let h2 = hash_content("content b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hash_content_is_16_hex_chars() {
        let h = hash_content("hello");
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── compute_line_diff (KYO-124) ──────────────────────────────────────

    #[test]
    fn test_compute_line_diff_identical_content() {
        // Identical inputs yield zero additions, zero deletions, and every
        // emitted line is tagged as "context".
        let content = "line one\nline two\nline three\n";
        let (additions, deletions, lines) = compute_line_diff(content, content);

        assert_eq!(additions, 0, "identical content should report 0 additions");
        assert_eq!(deletions, 0, "identical content should report 0 deletions");
        assert!(
            !lines.is_empty(),
            "diff should still emit context lines for identical content"
        );
        assert!(
            lines.iter().all(|l| l.line_type == "context"),
            "every line should be `context` when content is identical"
        );
    }

    #[test]
    fn test_compute_line_diff_disjoint_content() {
        // Completely disjoint inputs — every line on the left side is a
        // delete, every line on the right side is an add, and nothing is
        // shared as context.
        let from = "old line 1\nold line 2\nold line 3\n";
        let to = "new line A\nnew line B\n";
        let (additions, deletions, lines) = compute_line_diff(from, to);

        assert_eq!(additions, 2, "expected 2 new-side lines to be tagged add");
        assert_eq!(deletions, 3, "expected 3 old-side lines to be tagged delete");
        assert!(
            lines.iter().all(|l| l.line_type == "add" || l.line_type == "delete"),
            "disjoint content should produce no context lines, got: {lines:?}"
        );
        // Sanity check: content is trimmed of trailing newlines.
        assert!(
            lines.iter().all(|l| !l.content.ends_with('\n')),
            "diff line content must not retain trailing newline"
        );
    }

    #[test]
    fn test_compute_line_diff_mixed_changes() {
        // A shared prefix, one modified line, one added line on the new side.
        // Confirms the Myers output interleaves context / delete / add tags
        // and that counts match the number of tagged lines.
        let from = "keep 1\nchange me\nkeep 2\n";
        let to = "keep 1\nchanged!\nkeep 2\nbrand new\n";
        let (additions, deletions, lines) = compute_line_diff(from, to);

        // Tags must tally with the returned counts exactly.
        let add_count = lines.iter().filter(|l| l.line_type == "add").count() as i32;
        let del_count = lines.iter().filter(|l| l.line_type == "delete").count() as i32;
        let ctx_count = lines.iter().filter(|l| l.line_type == "context").count();

        assert_eq!(additions, add_count, "additions must match tagged `add` lines");
        assert_eq!(deletions, del_count, "deletions must match tagged `delete` lines");
        assert!(
            ctx_count >= 2,
            "shared `keep 1` and `keep 2` lines should be emitted as context (got {ctx_count})"
        );
        assert!(additions >= 2, "expected at least 2 additions (modified + appended)");
        assert!(deletions >= 1, "expected at least 1 deletion (the modified line)");
    }

    // ─── Postgres coverage (KYO-334) ────────────────────────────────────
    //
    // Every test above runs against `sqlite::memory:`, which only ever
    // exercises the `Vec<u8>` BLOB bind arm for `knowledge_chunks.embedding`.
    // `rechunk_document`'s Postgres arm binds `pgvector::Vector::from(embedding.clone())`
    // directly (never routing through `embedding_compat::bytes_to_pg_vector`
    // at all), a structurally different bind site from `learning_service.rs`'s
    // helper-mediated one — and `Vector` only implements sqlx's `Encode` for
    // Postgres, so a wrong bind here type-checks, does not error at query
    // time, and silently corrupts the stored vector. This test writes
    // through the real `rechunk_document` production function against a
    // real Postgres+pgvector database and asserts cosine self-similarity
    // between the stored embedding and the same embedding recomputed from
    // the same input text, with a non-vacuity control asserting an
    // unrelated embedding scores far from 1.0 — see `crate::test_pg` module
    // docs for the harness.

    /// Delete everything the rechunk-bind Postgres test inserted, scoped by
    /// `workspace_id`/`dashboard_id`. `knowledge_chunks`/`knowledge_file_tables`
    /// cascade off `dashboards.dashboard_id`, but are deleted explicitly
    /// first for the same reason every other `cleanup_pg` in this crate
    /// does. `dashboards.workspace_id`/`user_id` have no `ON DELETE CASCADE`,
    /// so the dashboard row must go before `cleanup_workspace_and_users_pg`
    /// deletes the workspace and owner.
    async fn cleanup_rechunk_pg(pg: &sqlx::PgPool, workspace_id: &str, owner_user_id: &str, dashboard_id: &str) {
        sqlx::query("DELETE FROM knowledge_chunks WHERE dashboard_id = $1")
            .bind(dashboard_id)
            .execute(pg)
            .await
            .expect("cleanup knowledge_chunks (postgres)");
        sqlx::query("DELETE FROM knowledge_file_tables WHERE dashboard_id = $1")
            .bind(dashboard_id)
            .execute(pg)
            .await
            .expect("cleanup knowledge_file_tables (postgres)");
        sqlx::query("DELETE FROM dashboards WHERE dashboard_id = $1")
            .bind(dashboard_id)
            .execute(pg)
            .await
            .expect("cleanup dashboards (postgres)");
        sqlx::query("DELETE FROM sync_log WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(pg)
            .await
            .expect("cleanup sync_log (postgres)");
        crate::test_pg::cleanup_workspace_and_users_pg(pg, workspace_id, &[owner_user_id]).await;
    }

    #[tokio::test]
    async fn postgres_rechunk_document_embedding_roundtrips_through_pgvector_bind() {
        let test_name = "postgres_rechunk_document_embedding_roundtrips_through_pgvector_bind";
        let Some(db) = crate::test_pg::postgres_test_pool_or_skip(test_name).await else {
            return;
        };
        let pg = crate::test_pg::postgres_pool(&db);

        let workspace_id = crate::test_pg::unique_test_id("ws");
        let owner_id = crate::test_pg::unique_test_id("owner");
        crate::test_pg::seed_user_pg(pg, &owner_id, &format!("{owner_id}@test.local")).await;
        crate::test_pg::seed_workspace_pg(pg, &workspace_id, &owner_id).await;

        let embed = EmbeddingService::new().expect("load embedding model");
        let content = "Our nightly ETL job aggregates click-through rates from the \
            marketing_events table and writes hourly rollups into the \
            campaign_performance table. Analysts pull this data into ChartML \
            dashboards to track conversion funnel drop-off by campaign_id.";

        // embed=None so create_dashboard doesn't also spawn a background
        // rechunk racing the explicit one below.
        let dashboard_id = create_dashboard(
            &db,
            &owner_id,
            &workspace_id,
            "Bind coverage doc",
            content,
            DocType::Knowledge,
            None,
        )
        .await
        .expect("create_dashboard (postgres)");

        rechunk_document(&db, &embed, &dashboard_id, content, &workspace_id)
            .await
            .expect("rechunk_document (postgres)");

        let chunk_id: String = sqlx::query_scalar(
            "SELECT id FROM knowledge_chunks WHERE dashboard_id = $1 ORDER BY chunk_index LIMIT 1",
        )
        .bind(&dashboard_id)
        .fetch_one(pg)
        .await
        .expect("fetch chunk id");

        // Same production embedding call `rechunk_document` made internally
        // (`embed_passages`) for this single-chunk content — deterministic
        // for identical input text, so this is "the same vector" the ticket
        // requires, not a re-derivation of the bind logic under test.
        let expected = embed
            .embed_passages(&[content])
            .expect("embed_passages (expected)")
            .into_iter()
            .next()
            .expect("one embedding for one chunk");

        let similarity =
            crate::test_pg::cosine_similarity_pg(pg, "knowledge_chunks", "id", &chunk_id, expected).await;
        assert!(
            (similarity - 1.0).abs() < 1e-4,
            "self-similarity should be ~1.0 for the same embedding, got {similarity}"
        );

        // Non-vacuity control: an unrelated chunk's embedding must not
        // score near 1.0 against the stored vector — without this, the
        // assertion above would pass equally well on a table of zeroes.
        let unrelated = embed
            .embed_passages(&[
                "The Pacific Northwest rainforest receives over 100 inches of \
                 rainfall annually, supporting old-growth Douglas fir and western \
                 hemlock stands more than 500 years old. Coastal fog drip \
                 contributes significantly to summer moisture during the dry season.",
            ])
            .expect("embed_passages (control)")
            .into_iter()
            .next()
            .expect("one embedding for one control chunk");
        let control_similarity =
            crate::test_pg::cosine_similarity_pg(pg, "knowledge_chunks", "id", &chunk_id, unrelated).await;
        assert!(
            control_similarity < 0.5,
            "unrelated embedding must not score near 1.0, got {control_similarity}"
        );

        cleanup_rechunk_pg(pg, &workspace_id, &owner_id, &dashboard_id).await;
    }

    // ─── store_dashboard_embedding Postgres coverage (KYO-371) ─────────────
    //
    // `store_dashboard_embedding` (the extracted, awaitable core of
    // `spawn_embedding_generation`) is the fourth production `pgvector::Vector`
    // bind site and the only one KYO-334 left uncovered — see that function's
    // doc comment. It binds `bytes_to_pg_vector(&embedding_bytes)`, the same
    // helper-mediated bind shape `learning_service.rs` uses, making this the
    // third production site of that shape (after `save_learning` and
    // `update_learning` in `learning_service.rs`) — distinct only from
    // `rechunk_document`'s direct `pgvector::Vector::from(embedding.clone())`
    // above — so a wrong bind here type-checks, does not error at query time,
    // and silently degrades dashboard semantic search. This test writes through
    // the real `store_dashboard_embedding` production function against a real
    // Postgres+pgvector database and asserts cosine self-similarity between the
    // stored embedding and the same embedding recomputed from the same input
    // text, with a non-vacuity control asserting an unrelated embedding scores
    // far from 1.0 — see `crate::test_pg` module docs for the harness.

    /// Delete everything the `store_dashboard_embedding` Postgres test
    /// inserted, scoped by `workspace_id`/`dashboard_id`. `dashboards`
    /// has no `ON DELETE CASCADE` from `workspaces`/`users`, so it must go
    /// before `cleanup_workspace_and_users_pg` deletes those. `sync_log` is
    /// deleted too because `create_dashboard` writes one on every insert —
    /// mirrors `cleanup_rechunk_pg`'s tail above.
    async fn cleanup_dashboard_embedding_pg(
        pg: &sqlx::PgPool,
        workspace_id: &str,
        owner_user_id: &str,
        dashboard_id: &str,
    ) {
        sqlx::query("DELETE FROM dashboards WHERE dashboard_id = $1")
            .bind(dashboard_id)
            .execute(pg)
            .await
            .expect("cleanup dashboards (postgres)");
        sqlx::query("DELETE FROM sync_log WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(pg)
            .await
            .expect("cleanup sync_log (postgres)");
        crate::test_pg::cleanup_workspace_and_users_pg(pg, workspace_id, &[owner_user_id]).await;
    }

    #[tokio::test]
    async fn postgres_store_dashboard_embedding_roundtrips_through_pgvector_bind() {
        let test_name = "postgres_store_dashboard_embedding_roundtrips_through_pgvector_bind";
        let Some(db) = crate::test_pg::postgres_test_pool_or_skip(test_name).await else {
            return;
        };
        let pg = crate::test_pg::postgres_pool(&db);

        let workspace_id = crate::test_pg::unique_test_id("ws");
        let owner_id = crate::test_pg::unique_test_id("owner");
        crate::test_pg::seed_user_pg(pg, &owner_id, &format!("{owner_id}@test.local")).await;
        crate::test_pg::seed_workspace_pg(pg, &workspace_id, &owner_id).await;

        let embed = EmbeddingService::new().expect("load embedding model");
        let title = "Campaign performance rollups";
        let content = "Our nightly ETL job aggregates click-through rates from the \
            marketing_events table and writes hourly rollups into the \
            campaign_performance table for the growth team's weekly review.";

        // embed=None: create_dashboard's own `embed` param spawns a
        // background `rechunk_document` call (writes `knowledge_chunks`),
        // not the embedding generation under test here — but passing None
        // keeps this test's only background work the explicit
        // `store_dashboard_embedding` call below, rather than leaving an
        // unrelated fire-and-forget task outliving the test's cleanup.
        let dashboard_id = create_dashboard(
            &db,
            &owner_id,
            &workspace_id,
            title,
            content,
            DocType::Knowledge,
            None,
        )
        .await
        .expect("create_dashboard (postgres)");

        store_dashboard_embedding(&db, &embed, &dashboard_id, &workspace_id, title, content)
            .await
            .expect("store_dashboard_embedding (postgres)");

        // Same production embedding call `store_dashboard_embedding` makes
        // internally (`embed_one` over "{title}\n{content}") — deterministic
        // for identical input text, so this is "the same vector" the ticket
        // requires, not a re-derivation of the bind logic under test.
        let expected = embed
            .embed_one(&format!("{title}\n{content}"))
            .expect("embed_one (expected)");

        let similarity =
            crate::test_pg::cosine_similarity_pg(pg, "dashboards", "dashboard_id", &dashboard_id, expected)
                .await;
        assert!(
            (similarity - 1.0).abs() < 1e-4,
            "self-similarity should be ~1.0 for the same embedding, got {similarity}"
        );

        // Non-vacuity control: an unrelated embedding must not score near
        // 1.0 against the stored vector — without this, the assertion above
        // would pass equally well on a table of zeroes.
        let unrelated = embed
            .embed_one(
                "The community garden's tomato harvest this year was the largest \
                 on record thanks to an unusually warm and rainy June.",
            )
            .expect("embed_one (control)");
        let control_similarity =
            crate::test_pg::cosine_similarity_pg(pg, "dashboards", "dashboard_id", &dashboard_id, unrelated)
                .await;
        assert!(
            control_similarity < 0.5,
            "unrelated embedding must not score near 1.0, got {control_similarity}"
        );

        cleanup_dashboard_embedding_pg(pg, &workspace_id, &owner_id, &dashboard_id).await;
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

    // ── visibility_predicate ─────────────────────────────────────────────

    #[test]
    fn visibility_predicate_postgres() {
        let sql = visibility_predicate(3, true);
        assert!(sql.contains("d.user_id = $3"));
        assert!(sql.contains("c.is_public = TRUE"));
        assert!(sql.contains("EXISTS"));
        assert!(sql.contains("collection_dashboards cd"));
    }

    #[test]
    fn visibility_predicate_sqlite() {
        let sql = visibility_predicate(2, false);
        assert!(sql.contains("d.user_id = $2"));
        assert!(sql.contains("c.is_public = 1"));
    }

    // ── get_document_count visibility (KYO-181) ──────────────────────────
    //
    // Integration tests (async, in-memory SQLite). `search_dashboards`
    // already applies `visibility_predicate` to its result rows; these
    // tests lock in that the *count* fed alongside those results
    // (`total_workspace_documents` in the `search_dashboards` agent tool)
    // is scoped the same way, rather than counting every row in the
    // workspace regardless of ownership or collection membership.

    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_pool() -> DbPool {
        let _ = kyomi_core::constants::load_with_fallback();

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .expect("enable foreign keys");

        sqlx::migrate!("../../apps/server/migrations-sqlite")
            .run(&pool)
            .await
            .expect("run sqlite migrations");

        DbPool::Sqlite(pool)
    }

    fn sqlite_pool(db: &DbPool) -> &sqlx::SqlitePool {
        match db {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        }
    }

    async fn seed_user(sq: &sqlx::SqlitePool, user_id: &str, email: &str) {
        sqlx::query("INSERT INTO users (user_id, email) VALUES ($1, $2)")
            .bind(user_id)
            .bind(email)
            .execute(sq)
            .await
            .expect("insert user");
    }

    async fn seed_workspace(sq: &sqlx::SqlitePool, workspace_id: &str, owner_user_id: &str) {
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $2, $3)",
        )
        .bind(workspace_id)
        .bind(format!("Workspace {workspace_id}"))
        .bind(owner_user_id)
        .execute(sq)
        .await
        .expect("insert workspace");
    }

    /// Seeds `ws-1` with two members: `user-a` (workspace owner) and
    /// `user-b`, a member who owns nothing shared with them by default.
    async fn seed_two_member_workspace(db: &DbPool) {
        let sq = sqlite_pool(db);
        seed_user(sq, "user-a", "a@test.local").await;
        seed_user(sq, "user-b", "b@test.local").await;
        seed_workspace(sq, "ws-1", "user-a").await;
    }

    /// Makes `doc_id` workspace-visible by putting it in a fresh public
    /// collection — the only mechanism by which a non-owner can see another
    /// member's document (`visibility_predicate`).
    async fn share_via_public_collection(
        db: &DbPool,
        doc_id: &str,
        doc_type: &str,
        collection_name: &str,
    ) {
        let collection = crate::collection_service::create_collection(
            crate::collection_service::NewCollectionParams {
                db,
                workspace_id: "ws-1",
                name: collection_name,
                description: None,
                color: None,
                is_public: true,
                doc_type,
                created_by: "user-a",
            },
        )
        .await
        .expect("create public collection");

        crate::collection_service::add_dashboard(
            db, &collection.id, doc_id, "ws-1", "user-a", None,
        )
        .await
        .expect("add doc to public collection");
    }

    /// Seeds the KYO-181 scenario: user-a owns 3 private docs plus 1 doc
    /// that is a member of a public collection; user-b owns 1 private doc.
    /// Five rows total. Split across `doc_type` (2 dashboard, 3 knowledge)
    /// so a `doc_type_filter` test exercises the filtered SQL branch (and
    /// its different visibility-predicate bind index) too, not just the
    /// unfiltered one.
    async fn seed_kyo_181_scenario(db: &DbPool) -> String {
        seed_two_member_workspace(db).await;

        // user-a: 3 private docs (2 dashboard, 1 knowledge).
        create_dashboard(
            db, "user-a", "ws-1", "A Private Dash 1", "# content",
            DocType::Dashboard, None,
        )
        .await
        .expect("create a-private-dash-1");
        create_dashboard(
            db, "user-a", "ws-1", "A Private Dash 2", "# content",
            DocType::Dashboard, None,
        )
        .await
        .expect("create a-private-dash-2");
        create_dashboard(
            db, "user-a", "ws-1", "A Private Knowledge", "private notes",
            DocType::Knowledge, None,
        )
        .await
        .expect("create a-private-knowledge");

        // user-a: 1 more knowledge doc, made visible workspace-wide via a
        // public collection.
        let public_doc_id = create_dashboard(
            db, "user-a", "ws-1", "A Public Knowledge", "public notes",
            DocType::Knowledge, None,
        )
        .await
        .expect("create a-public-knowledge");

        share_via_public_collection(db, &public_doc_id, "knowledge", "Public Folder").await;

        // user-b: 1 private dashboard doc.
        create_dashboard(
            db, "user-b", "ws-1", "B Private Dash", "# content",
            DocType::Dashboard, None,
        )
        .await
        .expect("create b-private-dash");

        public_doc_id
    }

    #[tokio::test]
    async fn get_document_count_scopes_to_visibility_not_raw_workspace_total() {
        let db = test_pool().await;
        seed_kyo_181_scenario(&db).await;

        // user-b sees only their own doc plus the one visible through the
        // public collection — not all 5 rows in the workspace.
        let count_b = get_document_count(&db, "ws-1", None, "user-b")
            .await
            .expect("count for user-b");
        assert_eq!(
            count_b, 2,
            "user-b should see their own doc plus the public one, not all 5 workspace rows"
        );

        // user-a owns all 4 of their own docs, so they see all of them.
        let count_a = get_document_count(&db, "ws-1", None, "user-a")
            .await
            .expect("count for user-a");
        assert_eq!(count_a, 4, "user-a should see all 4 of their own docs");
    }

    #[tokio::test]
    async fn get_document_count_with_doc_type_filter_scopes_to_visibility() {
        let db = test_pool().await;
        seed_kyo_181_scenario(&db).await;

        // Exercises the `Some(doc_type_filter)` SQL branch, whose
        // visibility-predicate bind index ($3) differs from the unfiltered
        // branch's ($2) — a wrong index here would error or silently
        // filter on the wrong column.
        let knowledge_b = get_document_count(&db, "ws-1", Some(DocType::Knowledge), "user-b")
            .await
            .expect("knowledge count for user-b");
        assert_eq!(
            knowledge_b, 1,
            "user-b should see only the public knowledge doc, not user-a's private one"
        );

        let knowledge_a = get_document_count(&db, "ws-1", Some(DocType::Knowledge), "user-a")
            .await
            .expect("knowledge count for user-a");
        assert_eq!(knowledge_a, 2, "user-a should see both of their own knowledge docs");

        let dashboard_b = get_document_count(&db, "ws-1", Some(DocType::Dashboard), "user-b")
            .await
            .expect("dashboard count for user-b");
        assert_eq!(
            dashboard_b, 1,
            "user-b should see only their own dashboard-type doc"
        );
    }

    // ── Delete-row visibility (KYO-313) ──────────────────────────────────
    //
    // `delete_dashboard` reads `is_doc_publicly_visible` *before* the
    // `DELETE`, because `collection_dashboards` CASCADE-deletes with the
    // dashboard: after the delete there is no join left to recompute the
    // value from, and a deleted entity is never mutated again, so a wrong
    // `is_workspace_visible` on the `Delete` row is permanent. These tests
    // drive the real `delete_dashboard` and assert through the real
    // `get_entries_since` filter, so moving that read after the `DELETE`
    // (or hardcoding the flag) fails them.

    /// The sync delta `user_id` would receive from a cold start.
    async fn delta_for(db: &DbPool, user_id: &str) -> Vec<kyomi_types::sync::SyncAction> {
        crate::sync_log_service::get_entries_since(db, "ws-1", 0, user_id, 100)
            .await
            .expect("read sync delta")
    }

    fn delete_rows_for<'a>(
        delta: &'a [kyomi_types::sync::SyncAction],
        entity_id: &str,
    ) -> Vec<&'a kyomi_types::sync::SyncAction> {
        delta
            .iter()
            .filter(|a| a.entity_id == entity_id && matches!(a.action, SyncActionType::Delete))
            .collect()
    }

    #[tokio::test]
    async fn delete_of_public_dashboard_reaches_non_owners() {
        let db = test_pool().await;
        seed_two_member_workspace(&db).await;

        let doc_id = create_dashboard(
            &db, "user-a", "ws-1", "Shared Dash", "# content",
            DocType::Dashboard, None,
        )
        .await
        .expect("create shared dashboard");
        share_via_public_collection(&db, &doc_id, "dashboard", "Public Dashboards").await;

        assert!(
            delete_dashboard(&db, &doc_id, "ws-1", "user-a")
                .await
                .expect("delete dashboard"),
            "the seeded dashboard should have been deleted"
        );

        // user-b could see the doc through the public collection, so their
        // delta must carry the Delete that evicts it from their cache.
        let delta_b = delta_for(&db, "user-b").await;
        let deletes = delete_rows_for(&delta_b, &doc_id);
        assert_eq!(
            deletes.len(),
            1,
            "non-owner must receive exactly one Delete row for a publicly \
             visible dashboard, got delta: {delta_b:?}"
        );
        assert_eq!(deletes[0].entity_type, entity_types::DASHBOARD);
    }

    #[tokio::test]
    async fn delete_of_public_knowledge_doc_reaches_non_owners_as_knowledge() {
        let db = test_pool().await;
        seed_two_member_workspace(&db).await;

        let doc_id = create_dashboard(
            &db, "user-a", "ws-1", "Shared Notes", "public notes",
            DocType::Knowledge, None,
        )
        .await
        .expect("create shared knowledge doc");
        share_via_public_collection(&db, &doc_id, "knowledge", "Public Notes").await;

        assert!(
            delete_dashboard(&db, &doc_id, "ws-1", "user-a")
                .await
                .expect("delete knowledge doc"),
            "the seeded knowledge doc should have been deleted"
        );

        // Exercises the `!current.doc_type().is_dashboard()` branch: the
        // client keys its cache by entity_type, so a `dashboard` row here
        // would never evict the knowledge entry.
        let delta_b = delta_for(&db, "user-b").await;
        let deletes = delete_rows_for(&delta_b, &doc_id);
        assert_eq!(
            deletes.len(),
            1,
            "non-owner must receive exactly one Delete row for a publicly \
             visible knowledge doc, got delta: {delta_b:?}"
        );
        assert_eq!(deletes[0].entity_type, entity_types::KNOWLEDGE);
    }

    #[tokio::test]
    async fn delete_of_private_dashboard_stays_hidden_from_non_owners() {
        let db = test_pool().await;
        seed_two_member_workspace(&db).await;

        let private_id = create_dashboard(
            &db, "user-a", "ws-1", "Private Dash", "# content",
            DocType::Dashboard, None,
        )
        .await
        .expect("create private dashboard");

        // A second, publicly visible doc that is also deleted. Its Delete row
        // is what makes user-b's delta non-empty, so the "no rows for
        // private_id" assertion below is a real filter result rather than an
        // empty query.
        let public_id = create_dashboard(
            &db, "user-a", "ws-1", "Public Dash", "# content",
            DocType::Dashboard, None,
        )
        .await
        .expect("create public dashboard");
        share_via_public_collection(&db, &public_id, "dashboard", "Public Dashboards").await;

        for id in [&private_id, &public_id] {
            assert!(
                delete_dashboard(&db, id, "ws-1", "user-a")
                    .await
                    .expect("delete dashboard"),
                "the seeded dashboard should have been deleted"
            );
        }

        let delta_b = delta_for(&db, "user-b").await;
        assert_eq!(
            delete_rows_for(&delta_b, &public_id).len(),
            1,
            "control: the publicly visible doc's Delete must reach user-b, \
             otherwise the assertion below proves nothing"
        );
        assert!(
            delta_b.iter().all(|a| a.entity_id != private_id),
            "a member who could never see the document must not learn of its \
             existence from its deletion; got delta: {delta_b:?}"
        );

        // The owner still needs the Delete to evict their own cached copy.
        let delta_a = delta_for(&db, "user-a").await;
        let owner_deletes = delete_rows_for(&delta_a, &private_id);
        assert_eq!(
            owner_deletes.len(),
            1,
            "owner must receive the Delete row for their own private doc, \
             got delta: {delta_a:?}"
        );
        assert_eq!(owner_deletes[0].entity_type, entity_types::DASHBOARD);
    }

    // ── Visibility-read failure must not write a wrong Delete row (KYO-354) ─
    //
    // Before this fix, `is_doc_publicly_visible` swallowed its own query
    // error and returned `false`. For `delete_dashboard`, that meant a
    // transient DB error on the visibility read collapsed into "private" and
    // let the DELETE proceed, permanently stranding the document in
    // non-owners' caches (a deleted entity is never mutated again, so the
    // wrong value could never self-heal). This test breaks only the
    // visibility read (by dropping the table it joins against) and asserts
    // three things: the call returns `Err`, the dashboard row still exists,
    // and no `Delete` row reaches either user's delta — proving the DELETE
    // itself never ran, not just that the return value looked like failure.

    #[tokio::test]
    async fn delete_dashboard_propagates_visibility_read_failure_instead_of_writing_wrong_delete()
    {
        let db = test_pool().await;
        seed_two_member_workspace(&db).await;

        let doc_id = create_dashboard(
            &db, "user-a", "ws-1", "Shared Dash", "# content",
            DocType::Dashboard, None,
        )
        .await
        .expect("create shared dashboard");
        share_via_public_collection(&db, &doc_id, "dashboard", "Public Dashboards").await;

        // Break only the visibility read: `is_doc_publicly_visible` joins
        // `collection_dashboards` to `collections`. Dropping the join table
        // leaves the ownership check (`get_dashboard_unchecked`, a plain
        // `SELECT ... FROM dashboards`) and the DELETE itself untouched, so
        // any success would come from the fix under test, not a broader
        // outage.
        sqlx::query("DROP TABLE collection_dashboards")
            .execute(sqlite_pool(&db))
            .await
            .expect("drop collection_dashboards");

        let result = delete_dashboard(&db, &doc_id, "ws-1", "user-a").await;
        assert!(
            result.is_err(),
            "a failed visibility read must propagate as an error, not \
             collapse into a successful delete: {result:?}"
        );

        let still_exists: Option<(String,)> = sqlx::query_as(
            "SELECT dashboard_id FROM dashboards WHERE dashboard_id = $1",
        )
        .bind(&doc_id)
        .fetch_optional(sqlite_pool(&db))
        .await
        .expect("query dashboards");
        assert!(
            still_exists.is_some(),
            "the dashboard row must survive a failed visibility read -- the \
             DELETE must not run when the read ahead of it fails"
        );

        let delta_a = delta_for(&db, "user-a").await;
        let delta_b = delta_for(&db, "user-b").await;
        assert!(
            delete_rows_for(&delta_a, &doc_id).is_empty(),
            "owner delta must carry no Delete row for a delete that never \
             happened, got delta: {delta_a:?}"
        );
        assert!(
            delete_rows_for(&delta_b, &doc_id).is_empty(),
            "non-owner delta must carry no Delete row for a delete that \
             never happened, got delta: {delta_b:?}"
        );
    }
}
