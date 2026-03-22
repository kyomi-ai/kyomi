// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for dashboard CRUD operations.
//!
//! These replace the REST API calls for listing, viewing, creating,
//! updating, and deleting dashboards. Each function calls the same
//! service-layer code as the existing REST routes in
//! `apps/server/src/routes/dashboards.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// A dashboard in a list/search result.
///
/// Maps 1:1 from `dashboard_service::DashboardSearchResult`, with timestamps
/// converted to RFC 3339 strings and summary extracted from content.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DashboardListItem {
    pub dashboard_id: String,
    pub user_id: String,
    pub workspace_id: String,
    pub title: String,
    pub content: String,
    pub content_preview: Option<String>,
    pub summary: Option<String>,
    pub last_change_summary: Option<String>,
    pub popularity_score: f64,
    pub view_count: i64,
    pub recent_views: i64,
    pub updated_at: String,
    pub created_at: String,
}

/// Full dashboard detail for viewing/editing.
///
/// Returned by `get_dashboard`, includes full content and extracted summary.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DashboardDetail {
    pub dashboard_id: String,
    pub user_id: String,
    pub workspace_id: String,
    pub title: String,
    pub content: String,
    pub summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_change_summary: Option<String>,
}

/// Lightweight version info for the history list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VersionSummary {
    pub version_number: i32,
    pub title: String,
    pub change_summary: Option<String>,
    pub byte_size: Option<i32>,
    pub created_at: String,
    pub created_by_name: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Read operations
// ─────────────────────────────────────────────────────────────────────────────

/// List or search dashboards in the current workspace.
///
/// When `query` is provided, searches by title/content. Otherwise lists all.
/// `sort_by` accepts "popularity", "recent" (default), or "created".
/// `limit` defaults to 50, clamped to [1, 100].
///
/// Mirrors `GET /dashboards/` in `apps/server/src/routes/dashboards.rs`.
#[server(prefix = "/leptos-api")]
pub async fn list_dashboards(
    query: Option<String>,
    sort_by: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<DashboardListItem>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let sort = match sort_by.as_deref() {
        Some("popularity") => kyomi_auth::dashboard_service::SearchSort::Popularity,
        Some("created") => kyomi_auth::dashboard_service::SearchSort::Created,
        _ => kyomi_auth::dashboard_service::SearchSort::Recent,
    };

    let limit = limit.unwrap_or(50).clamp(1, 100);

    let results = kyomi_auth::dashboard_service::search_dashboards(
        &ctx.db,
        ws_id,
        query.as_deref(),
        sort,
        limit,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(results
        .into_iter()
        .map(|r| {
            let summary = kyomi_auth::dashboard_service::extract_summary(&r.content);
            DashboardListItem {
                dashboard_id: r.dashboard_id,
                user_id: r.user_id,
                workspace_id: r.workspace_id,
                title: r.title,
                content: r.content,
                content_preview: r.content_preview,
                summary,
                last_change_summary: r.last_change_summary,
                popularity_score: r.popularity_score,
                view_count: r.view_count,
                recent_views: r.recent_views,
                updated_at: r.updated_at.to_rfc3339(),
                created_at: r.created_at.to_rfc3339(),
            }
        })
        .collect())
}

/// Get a single dashboard by ID, including full content.
///
/// Records a view for popularity tracking (fire-and-forget).
/// Returns a 404-equivalent error if the dashboard is not found.
///
/// Mirrors `GET /dashboards/{dashboard_id}` in `apps/server/src/routes/dashboards.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_dashboard(dashboard_id: String) -> Result<DashboardDetail, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let dashboard =
        kyomi_auth::dashboard_service::get_dashboard(&ctx.db, &dashboard_id, ws_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .ok_or_else(|| ServerFnError::new(format!("Dashboard {dashboard_id} not found")))?;

    // Record view for popularity tracking (fire-and-forget)
    let db = ctx.db.clone();
    let did = dashboard_id.clone();
    let uid = auth.user_id.clone();
    let wid = ws_id.to_string();
    tokio::spawn(async move {
        if let Err(e) = kyomi_auth::dashboard_service::record_view(&db, &did, &uid, &wid).await {
            tracing::warn!(dashboard_id = %did, error = %e, "Failed to record dashboard view");
        }
    });

    let summary = kyomi_auth::dashboard_service::extract_summary(&dashboard.content);

    Ok(DashboardDetail {
        dashboard_id: dashboard.dashboard_id,
        user_id: dashboard.user_id,
        workspace_id: dashboard.workspace_id,
        title: dashboard.title,
        content: dashboard.content,
        summary,
        created_at: dashboard.created_at.to_rfc3339(),
        updated_at: dashboard.updated_at.to_rfc3339(),
        last_change_summary: dashboard.last_change_summary,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Write operations
// ─────────────────────────────────────────────────────────────────────────────

/// Create a new dashboard. Returns the new dashboard_id.
///
/// The service layer enforces free-tier dashboard limits (5 per workspace).
/// After creation, fires off background embedding generation.
///
/// Mirrors `POST /dashboards/` in `apps/server/src/routes/dashboards.rs`.
#[server(prefix = "/leptos-api")]
pub async fn create_dashboard(
    title: String,
    content: Option<String>,
) -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let content = content.unwrap_or_default();

    let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
        &ctx.db,
        &auth.user_id,
        ws_id,
        &title,
        &content,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Fire-and-forget embedding generation (matches REST handler — propagates error)
    let embedding_svc = ctx.embedding.wait_ready().await
        .map_err(|e| ServerFnError::new(format!("Embedding service unavailable: {e}")))?;
    kyomi_auth::dashboard_service::spawn_embedding_generation(
        ctx.db.clone(),
        embedding_svc.clone(),
        dashboard_id.clone(),
        ws_id.to_string(),
        title.trim().to_string(),
        content.clone(),
    );

    Ok(dashboard_id)
}

/// Update an existing dashboard's title, content, and/or change summary.
///
/// Rejects no-op updates where all fields are `None`.
/// Re-embeds the dashboard if title or content changed.
///
/// Mirrors `PATCH /dashboards/{dashboard_id}` in `apps/server/src/routes/dashboards.rs`.
#[server(prefix = "/leptos-api")]
pub async fn update_dashboard(
    dashboard_id: String,
    title: Option<String>,
    content: Option<String>,
    change_summary: Option<String>,
) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Reject no-op updates (matches REST handler validation)
    if title.is_none() && content.is_none() && change_summary.is_none() {
        return Err(ServerFnError::new("No updates provided"));
    }

    kyomi_auth::dashboard_service::update_dashboard(
        &ctx.db,
        &dashboard_id,
        ws_id,
        &auth.user_id,
        title.as_deref(),
        content.as_deref(),
        change_summary.as_deref(),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Re-embed if content or title changed (matches REST handler — propagates error)
    if title.is_some() || content.is_some() {
        let embedding_svc = ctx.embedding.wait_ready().await
            .map_err(|e| ServerFnError::new(format!("Embedding service unavailable: {e}")))?;
        if let Ok(Some(d)) =
            kyomi_auth::dashboard_service::get_dashboard(&ctx.db, &dashboard_id, ws_id).await
        {
            kyomi_auth::dashboard_service::spawn_embedding_generation(
                ctx.db.clone(),
                embedding_svc.clone(),
                dashboard_id,
                ws_id.to_string(),
                d.title,
                d.content,
            );
        }
    }

    Ok(())
}

/// Delete a dashboard by ID.
///
/// Mirrors `DELETE /dashboards/{dashboard_id}` in `apps/server/src/routes/dashboards.rs`.
#[server(prefix = "/leptos-api")]
pub async fn delete_dashboard(dashboard_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    kyomi_auth::dashboard_service::delete_dashboard(
        &ctx.db,
        &dashboard_id,
        ws_id,
        &auth.user_id,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Version operations
// ─────────────────────────────────────────────────────────────────────────────

/// List version history for a dashboard (most recent first).
#[server(prefix = "/leptos-api")]
pub async fn list_versions(dashboard_id: String) -> Result<Vec<VersionSummary>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Verify the dashboard belongs to this workspace
    kyomi_auth::dashboard_service::get_dashboard(&ctx.db, &dashboard_id, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new(format!("Dashboard {dashboard_id} not found")))?;

    let versions =
        kyomi_auth::dashboard_service::list_versions(&ctx.db, &dashboard_id, 50, 0)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(versions
        .into_iter()
        .map(|v| VersionSummary {
            version_number: v.version_number,
            title: v.title,
            change_summary: v.change_summary,
            byte_size: v.byte_size,
            created_at: v.created_at.to_rfc3339(),
            created_by_name: v.created_by.name,
        })
        .collect())
}

/// Restore a dashboard to a previous version.
///
/// Creates a snapshot of the current state, then replaces the dashboard
/// content with the specified version's content.
#[server(prefix = "/leptos-api")]
pub async fn restore_version(
    dashboard_id: String,
    version_number: i32,
) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    kyomi_auth::dashboard_service::restore_version(
        &ctx.db,
        &dashboard_id,
        ws_id,
        &auth.user_id,
        version_number,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Re-embed after restore (matches REST handler — propagates error)
    let embedding_svc = ctx.embedding.wait_ready().await
        .map_err(|e| ServerFnError::new(format!("Embedding service unavailable: {e}")))?;
    if let Ok(Some(d)) =
        kyomi_auth::dashboard_service::get_dashboard(&ctx.db, &dashboard_id, ws_id).await
    {
        kyomi_auth::dashboard_service::spawn_embedding_generation(
            ctx.db.clone(),
            embedding_svc.clone(),
            dashboard_id,
            ws_id.to_string(),
            d.title,
            d.content,
        );
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers — delegate to shared extractors in parent module
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, workspace_id};
