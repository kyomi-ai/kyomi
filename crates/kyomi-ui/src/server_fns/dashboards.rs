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
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub workspace_id: String,
    pub title: String,
    #[serde(default)]
    pub content: String,
    pub content_preview: Option<String>,
    pub summary: Option<String>,
    pub last_change_summary: Option<String>,
    #[serde(default)]
    pub popularity_score: f64,
    #[serde(default)]
    pub view_count: i64,
    #[serde(default)]
    pub recent_views: i64,
    pub updated_at: String,
    pub created_at: String,
    #[serde(default)]
    pub is_publicly_shared: bool,
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

/// Full version detail including content.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VersionDetail {
    pub version_number: i32,
    pub title: String,
    pub content: String,
    pub change_summary: Option<String>,
    pub byte_size: Option<i32>,
    pub created_at: String,
    pub created_by_name: Option<String>,
}

/// Diff between two versions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VersionDiff {
    pub from_version: i32,
    pub to_version: i32,
    pub additions: i32,
    pub deletions: i32,
    pub diff_lines: Vec<DiffLine>,
}

/// A single line in a version diff.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffLine {
    /// "add", "delete", or "context"
    pub line_type: String,
    pub content: String,
}

/// Result from list_versions — includes the current live version and historical snapshots.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VersionListResult {
    /// The current live dashboard content (version_number = max + 1).
    pub current_version: CurrentVersion,
    /// Historical version snapshots, newest first.
    pub versions: Vec<VersionSummary>,
}

/// The current live dashboard state — not yet snapshotted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CurrentVersion {
    pub version_number: i32,
    pub title: String,
    pub content: String,
    pub change_summary: Option<String>,
    pub created_at: String,
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
    let ac = AuthenticatedContext::extract().await?;

    let sort = match sort_by.as_deref() {
        Some("popularity") => kyomi_auth::dashboard_service::SearchSort::Popularity,
        Some("created") => kyomi_auth::dashboard_service::SearchSort::Created,
        _ => kyomi_auth::dashboard_service::SearchSort::Recent,
    };

    let limit = limit.unwrap_or(50).clamp(1, 100);

    let results = kyomi_auth::dashboard_service::search_dashboards(
        ac.db(),
        &ac.ws_id,
        &ac.auth.user_id,
        query.as_deref(),
        Some(kyomi_core::models::DocType::Dashboard), // dashboard page only shows dashboards
        sort,
        limit,
    )
    .await
    .into_sfn()?;

    Ok(results
        .into_iter()
        .map(map_search_result_to_list_item)
        .collect())
}

/// Convert a `DashboardSearchResult` to a `DashboardListItem`.
///
/// Shared by both `list_dashboards` and `list_knowledge_docs` to avoid
/// duplicating the 13-field mapping.
#[cfg(feature = "ssr")]
pub(crate) fn map_search_result_to_list_item(
    r: kyomi_auth::dashboard_service::DashboardSearchResult,
) -> DashboardListItem {
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
        is_publicly_shared: r.is_publicly_shared,
    }
}

/// Get a single dashboard by ID, including full content.
///
/// Records a view for popularity tracking (fire-and-forget).
/// Returns a 404-equivalent error if the dashboard is not found.
///
/// Mirrors `GET /dashboards/{dashboard_id}` in `apps/server/src/routes/dashboards.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_dashboard(dashboard_id: String) -> Result<DashboardDetail, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let dashboard =
        kyomi_auth::dashboard_service::get_dashboard(ac.db(), &dashboard_id, &ac.ws_id, &ac.auth.user_id)
            .await
            .into_sfn()?
            .ok_or_else(|| ServerFnError::new(format!("Dashboard {dashboard_id} not found")))?;

    // Record view for popularity tracking (fire-and-forget)
    let db = ac.ctx.db.clone();
    let did = dashboard_id.clone();
    let uid = ac.auth.user_id.clone();
    let wid = ac.ws_id.clone();
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
    let ac = AuthenticatedContext::extract().await?;

    let content = content.unwrap_or_default();

    // Get embedding service for both embedding generation and rechunking
    let embedding_svc = ac.ctx.embedding.wait_ready().await
        .map_err(|e| ServerFnError::new(format!("Embedding service unavailable: {e}")))?;

    let dashboard_id = kyomi_auth::dashboard_service::create_dashboard(
        ac.db(),
        &ac.auth.user_id,
        &ac.ws_id,
        &title,
        &content,
        kyomi_core::models::DocType::Dashboard,
        Some(embedding_svc),
    )
    .await
    .into_sfn()?;
    kyomi_auth::dashboard_service::spawn_embedding_generation(
        ac.ctx.db.clone(),
        embedding_svc.clone(),
        dashboard_id.clone(),
        ac.ws_id.clone(),
        title.trim().to_string(),
        content.clone(),
    );

    if let Some(ws_manager) = &ac.ctx.ws_manager {
        kyomi_auth::websocket::helpers::broadcast_dashboard_sync(
            ac.db(), ws_manager, &dashboard_id, &ac.ws_id,
            kyomi_types::sync::SyncActionType::Insert,
            &ac.auth.user_id,
        ).await;
    }

    Ok(dashboard_id)
}

/// Update an existing dashboard's title, content, and/or change summary.
///
/// Rejects no-op updates where all fields are `None`.
/// Re-embeds the dashboard if title or content changed.
///
/// Mirrors `PATCH /dashboards/{dashboard_id}` in `apps/server/src/routes/dashboards.rs`.
#[server(prefix = "/leptos-api", input = server_fn::codec::Json)]
pub async fn update_dashboard(
    dashboard_id: String,
    title: Option<String>,
    content: Option<String>,
    change_summary: Option<String>,
) -> Result<(), ServerFnError> {
    // lint-allow: server-fn-callouts=sync broadcast is a separate cross-cutting concern alongside mutation + embedding + re-fetch
    let ac = AuthenticatedContext::extract().await?;

    // Reject no-op updates (matches REST handler validation)
    if title.is_none() && content.is_none() && change_summary.is_none() {
        return Err(ServerFnError::new("No updates provided"));
    }

    kyomi_auth::dashboard_service::update_dashboard(
        kyomi_auth::dashboard_service::UpdateDashboardParams {
            db: ac.db(),
            embed: None, // no rechunking from dashboard UI (yet)
            dashboard_id: &dashboard_id,
            workspace_id: &ac.ws_id,
            user_id: &ac.auth.user_id,
            title: title.as_deref(),
            content: content.as_deref(),
            change_summary: change_summary.as_deref(),
            expected_content_hash: None, // no CAS for dashboard UI
        },
    )
    .await
    .into_sfn()?;

    // Re-embed if content or title changed (matches REST handler — propagates error)
    if title.is_some() || content.is_some() {
        let embedding_svc = ac.ctx.embedding.wait_ready().await
            .map_err(|e| ServerFnError::new(format!("Embedding service unavailable: {e}")))?;
        if let Ok(Some(d)) =
            kyomi_auth::dashboard_service::get_dashboard(ac.db(), &dashboard_id, &ac.ws_id, &ac.auth.user_id).await
        {
            kyomi_auth::dashboard_service::spawn_embedding_generation(
                ac.ctx.db.clone(),
                embedding_svc.clone(),
                dashboard_id.clone(),
                ac.ws_id.clone(),
                d.title,
                d.content,
            );
        }
    }

    // Generate dashboard summary if content changed and none exists.
    tracing::info!(
        dashboard_id = %dashboard_id,
        has_content = content.is_some(),
        content_empty = content.as_ref().map(|c| c.is_empty()).unwrap_or(true),
        has_summary = content.as_ref().map(|c| kyomi_auth::dashboard_service::extract_summary(c).is_some()).unwrap_or(false),
        "Dashboard summary check"
    );
    if let Some(ref c) = content
        && !c.is_empty()
        && kyomi_auth::dashboard_service::extract_summary(c).is_none()
        && let Some(ws_manager) = ac.ctx.ws_manager.clone()
    {
            let fetched = kyomi_auth::dashboard_service::get_dashboard(
                ac.db(), &dashboard_id, &ac.ws_id, &ac.auth.user_id,
            )
            .await
            .map_err(|e| tracing::warn!(dashboard_id = %dashboard_id, error = %e, "failed to fetch dashboard for summary"))
            .ok()
            .flatten();

            let title_for_summary = title
                .clone()
                .or_else(|| fetched.as_ref().map(|d| d.title.clone()))
                .unwrap_or_default();
            let doc_type_for_summary = fetched
                .map(|d| d.doc_type)
                .unwrap_or_else(|| "dashboard".to_string());

            kyomi_agent::generate_dashboard_summary(
                kyomi_agent::DashboardSummaryParams {
                    db: ac.ctx.db.clone(),
                    ws_manager,
                    dashboard_id: dashboard_id.clone(),
                    user_id: ac.auth.user_id.clone(),
                    workspace_id: ac.ws_id.clone(),
                    title: title_for_summary,
                    content: c.clone(),
                    app_config: ac.ctx.config.clone(),
                    doc_type: doc_type_for_summary,
                },
            );
    }

    if let Some(ws_manager) = &ac.ctx.ws_manager {
        kyomi_auth::websocket::helpers::broadcast_dashboard_sync(
            ac.db(), ws_manager, &dashboard_id, &ac.ws_id,
            kyomi_types::sync::SyncActionType::Update,
            &ac.auth.user_id,
        ).await;
    }

    Ok(())
}

/// Delete a dashboard by ID.
///
/// Mirrors `DELETE /dashboards/{dashboard_id}` in `apps/server/src/routes/dashboards.rs`.
#[server(prefix = "/leptos-api")]
pub async fn delete_dashboard(dashboard_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    kyomi_auth::dashboard_service::delete_dashboard(
        ac.db(),
        &dashboard_id,
        &ac.ws_id,
        &ac.auth.user_id,
    )
    .await
    .into_sfn()?;

    if let Some(ws_manager) = &ac.ctx.ws_manager {
        kyomi_auth::websocket::helpers::broadcast_entity_delete(
            ws_manager, kyomi_types::sync::entity_types::DASHBOARD,
            &dashboard_id, &ac.ws_id,
        ).await;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Version operations
// ─────────────────────────────────────────────────────────────────────────────

/// List version history for a dashboard (most recent first).
///
/// Returns the current live dashboard content as `current_version`
/// (with `version_number = max + 1`) alongside historical snapshots.
/// Matches the Python/REST API contract.
#[server(prefix = "/leptos-api")]
pub async fn list_versions(dashboard_id: String) -> Result<VersionListResult, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Fetch the live dashboard (also verifies workspace ownership)
    let dashboard = kyomi_auth::dashboard_service::get_dashboard(ac.db(), &dashboard_id, &ac.ws_id, &ac.auth.user_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new(format!("Dashboard {dashboard_id} not found")))?;

    let versions =
        kyomi_auth::dashboard_service::list_versions(ac.db(), &dashboard_id, 50, 0)
            .await
            .into_sfn()?;

    // Current version = max + 1 (represents the live dashboard content)
    let current_version_number = versions
        .first()
        .map(|v| v.version_number + 1)
        .unwrap_or(1);

    let current_version = CurrentVersion {
        version_number: current_version_number,
        title: dashboard.title,
        content: dashboard.content,
        change_summary: dashboard.last_change_summary,
        created_at: dashboard.updated_at.to_rfc3339(),
    };

    let version_summaries = versions
        .into_iter()
        .map(|v| VersionSummary {
            version_number: v.version_number,
            title: v.title,
            change_summary: v.change_summary,
            byte_size: v.byte_size,
            created_at: v.created_at.to_rfc3339(),
            created_by_name: v.created_by.name,
        })
        .collect();

    Ok(VersionListResult {
        current_version,
        versions: version_summaries,
    })
}

/// Get a specific version's full content.
#[server(prefix = "/leptos-api")]
pub async fn get_version(
    dashboard_id: String,
    version_number: i32,
) -> Result<VersionDetail, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Verify dashboard belongs to workspace and is visible to user
    kyomi_auth::dashboard_service::get_dashboard(ac.db(), &dashboard_id, &ac.ws_id, &ac.auth.user_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new(format!("Dashboard {dashboard_id} not found")))?;

    let version =
        kyomi_auth::dashboard_service::get_version(ac.db(), &dashboard_id, version_number)
            .await
            .into_sfn()?
            .ok_or_else(|| {
                ServerFnError::new(format!(
                    "Version {version_number} not found for dashboard {dashboard_id}"
                ))
            })?;

    Ok(VersionDetail {
        version_number: version.version_number,
        title: version.title,
        content: version.content,
        change_summary: version.change_summary,
        byte_size: version.byte_size,
        created_at: version.created_at.to_rfc3339(),
        created_by_name: None, // get_version returns user_id only; name resolved by list_versions
    })
}

/// Diff two versions of a dashboard.
///
/// Returns added/removed line counts and the actual diff lines.
/// Handles the "current version" case: if either version number equals
/// `max_version + 1`, reads content from the live `dashboards` table
/// instead of `dashboard_versions`. Matches the Python/REST API contract.
#[server(prefix = "/leptos-api")]
pub async fn diff_versions(
    dashboard_id: String,
    from_version: i32,
    to_version: i32,
) -> Result<VersionDiff, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let result = kyomi_auth::dashboard_service::diff_versions(
        ac.db(),
        &dashboard_id,
        &ac.ws_id,
        &ac.auth.user_id,
        from_version,
        to_version,
    )
    .await
    .into_sfn()?;

    Ok(VersionDiff {
        from_version: result.from_version,
        to_version: result.to_version,
        additions: result.additions,
        deletions: result.deletions,
        diff_lines: result
            .diff_lines
            .into_iter()
            .map(|l| DiffLine {
                line_type: l.line_type,
                content: l.content,
            })
            .collect(),
    })
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
    let ac = AuthenticatedContext::extract().await?;

    kyomi_auth::dashboard_service::restore_version(
        ac.db(),
        &dashboard_id,
        &ac.ws_id,
        &ac.auth.user_id,
        version_number,
    )
    .await
    .into_sfn()?;

    // Re-embed after restore (matches REST handler — propagates error)
    let embedding_svc = ac.ctx.embedding.wait_ready().await
        .map_err(|e| ServerFnError::new(format!("Embedding service unavailable: {e}")))?;
    if let Ok(Some(d)) =
        kyomi_auth::dashboard_service::get_dashboard(ac.db(), &dashboard_id, &ac.ws_id, &ac.auth.user_id).await
    {
        kyomi_auth::dashboard_service::spawn_embedding_generation(
            ac.ctx.db.clone(),
            embedding_svc.clone(),
            dashboard_id,
            ac.ws_id.clone(),
            d.title,
            d.content,
        );
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Default dashboard operations
// ─────────────────────────────────────────────────────────────────────────────

/// Get the user's personal default dashboard ID.
///
/// Reads from `users.extra_metadata.default_dashboard_id`.
/// Returns `None` if no default is set.
///
/// Mirrors `PATCH /users/me/preferences` read path in `apps/server/src/routes/users.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_user_default_dashboard() -> Result<Option<String>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let user = kyomi_auth::user_service::get_user_by_id(&ctx.db, &auth.user_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let default_id = user
        .extra_metadata
        .as_ref()
        .and_then(|m| m.get("default_dashboard_id"))
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(default_id)
}

/// Set or clear the user's personal default dashboard.
///
/// Writes to `users.extra_metadata.default_dashboard_id`.
/// Pass `None` or empty string to clear the default.
///
/// Mirrors `PATCH /users/me/preferences` write path in `apps/server/src/routes/users.rs`.
#[server(prefix = "/leptos-api")]
pub async fn set_user_default_dashboard(dashboard_id: Option<String>) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let value = match dashboard_id {
        Some(id) if !id.is_empty() => serde_json::json!({ "default_dashboard_id": id }),
        _ => serde_json::json!({ "default_dashboard_id": null }),
    };

    kyomi_auth::user_service::update_extra_metadata(&ctx.db, &auth.user_id, &value)
        .await
        .into_sfn()?;

    Ok(())
}

/// Get the workspace default dashboard ID (any authenticated user can read).
///
/// Reads from `workspaces.settings.default_dashboard_id` (top-level, not custom_settings).
/// Returns `None` if no default is set.
///
/// Mirrors `GET /workspaces/default-dashboard` in `apps/server/src/routes/workspaces.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_workspace_default_dashboard() -> Result<Option<String>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let workspace = kyomi_auth::workspace_service::get_workspace_full(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    let default_id = workspace
        .settings
        .as_ref()
        .and_then(|s| s.get("default_dashboard_id"))
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(default_id)
}

/// Set or clear the workspace default dashboard (admin only).
///
/// Writes to `workspaces.settings.default_dashboard_id` (top-level).
/// Pass `None` or empty string to clear the default.
///
/// Mirrors `PATCH /workspaces/settings` write path in `apps/server/src/routes/workspaces.rs`.
#[server(prefix = "/leptos-api")]
pub async fn set_workspace_default_dashboard(
    dashboard_id: Option<String>,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    ac.require(Permission::SetWorkspaceDefaults, "Workspace admin access required")?;

    let workspace = kyomi_auth::workspace_service::get_workspace_full(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    let mut current_settings = workspace.settings.clone().unwrap_or(serde_json::json!({}));

    let dashboard_value = match dashboard_id {
        Some(id) if !id.is_empty() => serde_json::json!(id),
        _ => serde_json::Value::Null,
    };

    if let Some(obj) = current_settings.as_object_mut() {
        obj.insert("default_dashboard_id".to_string(), dashboard_value);
    }

    kyomi_auth::workspace_service::update_workspace_settings(ac.db(), &ac.ws_id, &current_settings)
        .await
        .into_sfn()?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers — delegate to shared extractors in parent module
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, AuthenticatedContext, IntoServerFnError};
#[cfg(feature = "ssr")]
use kyomi_types::Permission;
