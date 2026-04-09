// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard REST endpoints.
//!
//! Wire-compatible with Python's `routers/dashboards.py`.
//! All business logic is delegated to `kyomi_auth::dashboard_service`.
//! Route handlers are thin wrappers: extract auth, call service, return JSON.
//!
//! ## Endpoints
//!
//! - `POST   /`                                — create dashboard
//! - `GET    /`                                — list/search dashboards
//! - `GET    /{dashboard_id}`                  — get dashboard
//! - `PATCH  /{dashboard_id}`                  — update dashboard
//! - `DELETE /{dashboard_id}`                  — delete dashboard
//! - `GET    /{dashboard_id}/versions`         — list versions
//! - `GET    /{dashboard_id}/versions/diff`    — diff two versions
//! - `GET    /{dashboard_id}/versions/{num}`   — get specific version
//! - `POST   /{dashboard_id}/versions/{num}/restore` — restore version

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use kyomi_agent::tools::chart_palettes;
use kyomi_auth::{
    dashboard_service, middleware::AuthUser, websocket::helpers as ws_helpers, workspace_service,
};
use kyomi_core::capability;

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the `/dashboards` router with all dashboard management endpoints.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_dashboards).post(create_dashboard))
        .route(
            "/{dashboard_id}",
            get(get_dashboard).patch(update_dashboard).delete(delete_dashboard),
        )
        .route("/{dashboard_id}/export/pdf", get(export_pdf))
        // Static path before `/{dashboard_id}/versions/{num}` capture
        .route("/{dashboard_id}/versions/diff", get(diff_versions))
        .route("/{dashboard_id}/versions", get(list_versions))
        .route("/{dashboard_id}/versions/{num}", get(get_version))
        .route(
            "/{dashboard_id}/versions/{num}/restore",
            post(restore_version),
        )
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Extract workspace_id from user, or return 400.
fn get_workspace_id(user: &AuthUser) -> Result<&str, kyomi_core::Error> {
    user.workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("Workspace context required".into()))
}

/// Convert a `Dashboard` model to a `DashboardResponse`.
fn dashboard_to_response(dashboard: &kyomi_core::models::Dashboard) -> DashboardResponse {
    let summary = dashboard_service::extract_summary(&dashboard.content);
    DashboardResponse {
        dashboard_id: dashboard.dashboard_id.clone(),
        user_id: dashboard.user_id.clone(),
        workspace_id: dashboard.workspace_id.clone(),
        title: dashboard.title.clone(),
        content: dashboard.content.clone(),
        summary,
        last_change_summary: dashboard.last_change_summary.clone(),
        created_at: dashboard.created_at.to_rfc3339(),
        updated_at: dashboard.updated_at.to_rfc3339(),
    }
}

/// Convert a `DashboardSearchResult` to a `DashboardListItem`.
fn search_result_to_list_item(
    result: &dashboard_service::DashboardSearchResult,
) -> DashboardListItem {
    DashboardListItem {
        dashboard_id: result.dashboard_id.clone(),
        user_id: result.user_id.clone(),
        workspace_id: result.workspace_id.clone(),
        title: result.title.clone(),
        content: result.content.clone(),
        content_preview: result.content_preview.clone(),
        last_change_summary: result.last_change_summary.clone(),
        created_at: result.created_at.to_rfc3339(),
        updated_at: result.updated_at.to_rfc3339(),
        popularity_score: result.popularity_score,
        view_count: result.view_count,
        recent_views: result.recent_views,
    }
}

/// Convert a `DashboardVersion` model to a `VersionResponse`.
fn version_to_response(version: &kyomi_core::models::DashboardVersion) -> VersionResponse {
    VersionResponse {
        version_id: version.version_id,
        dashboard_id: version.dashboard_id.clone(),
        version_number: version.version_number,
        content: version.content.clone(),
        title: version.title.clone(),
        change_summary: version.change_summary.clone(),
        byte_size: version.byte_size,
        created_at: version.created_at.to_rfc3339(),
        created_by: version.created_by.clone(),
    }
}

/// Convert a `DashboardVersionSummary` to a `VersionSummaryResponse`.
fn version_summary_to_response(
    summary: &dashboard_service::DashboardVersionSummary,
) -> VersionSummaryResponse {
    VersionSummaryResponse {
        version_id: summary.version_id,
        version_number: summary.version_number,
        title: summary.title.clone(),
        change_summary: summary.change_summary.clone(),
        byte_size: summary.byte_size,
        created_at: summary.created_at.to_rfc3339(),
        created_by: CreatedByResponse {
            user_id: summary.created_by.user_id.clone(),
            name: summary.created_by.name.clone(),
            email: Some(summary.created_by.email.clone()),
        },
    }
}

// ===========================================================================
// Request / Response Types
// ===========================================================================

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct CreateDashboardRequest {
    title: String,
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct UpdateDashboardRequest {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    change_summary: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct DashboardResponse {
    dashboard_id: String,
    user_id: String,
    workspace_id: String,
    title: String,
    content: String,
    summary: Option<String>,
    last_change_summary: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct DashboardListItem {
    dashboard_id: String,
    user_id: String,
    workspace_id: String,
    title: String,
    content: String,
    content_preview: Option<String>,
    last_change_summary: Option<String>,
    created_at: String,
    updated_at: String,
    popularity_score: f64,
    view_count: i64,
    recent_views: i64,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct VersionResponse {
    version_id: i32,
    dashboard_id: String,
    version_number: i32,
    content: String,
    title: String,
    change_summary: Option<String>,
    byte_size: Option<i32>,
    created_at: String,
    created_by: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize, Clone))]
struct CreatedByResponse {
    user_id: String,
    name: Option<String>,
    email: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct VersionSummaryResponse {
    version_id: i32,
    version_number: i32,
    title: String,
    change_summary: Option<String>,
    byte_size: Option<i32>,
    created_at: String,
    created_by: CreatedByResponse,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct VersionListResponse {
    versions: Vec<VersionSummaryResponse>,
    current_version: CurrentVersionResponse,
    total_count: i64,
    dashboard_id: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct CurrentVersionResponse {
    version_id: String,
    dashboard_id: String,
    version_number: i32,
    content: String,
    title: String,
    change_summary: String,
    created_at: String,
    created_by: CreatedByResponse,
    is_current: bool,
}

// -- Query params --

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct DashboardListParams {
    #[serde(default)]
    query: Option<String>,
    #[serde(default = "default_sort_by")]
    sort_by: String,
    #[serde(default = "default_limit")]
    limit: i64,
    /// Filter by document type: "dashboard" (default), "knowledge", or "all".
    #[serde(default)]
    doc_type: Option<String>,
}

fn default_sort_by() -> String {
    "recent".to_string()
}

fn default_limit() -> i64 {
    50
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct VersionListParams {
    #[serde(default = "default_version_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_version_limit() -> i64 {
    20
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct DiffParams {
    from_version: i32,
    to_version: i32,
}

// ===========================================================================
// Endpoint Handlers
// ===========================================================================

// ---------------------------------------------------------------------------
// POST / — Create dashboard
// ---------------------------------------------------------------------------

async fn create_dashboard(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<CreateDashboardRequest>,
) -> Result<Json<DashboardResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let dashboard_id = dashboard_service::create_dashboard(
        &state.db,
        &user.user_id,
        workspace_id,
        &request.title,
        &request.content,
        kyomi_core::models::DocType::Dashboard,
    )
    .await?;

    // Fire-and-forget embedding generation
    dashboard_service::spawn_embedding_generation(
        state.db.clone(),
        state.embedding.wait_ready().await?.clone(),
        dashboard_id.clone(),
        workspace_id.to_string(),
        request.title.trim().to_string(),
        request.content.clone(),
    );

    // Fetch the created dashboard to return full response
    let dashboard = dashboard_service::get_dashboard(&state.db, &dashboard_id, workspace_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Internal("Dashboard created but not found on read-back".into())
        })?;

    // Notify workspace members about the new dashboard
    let changed_by_name = user.name.as_deref().unwrap_or(&user.email);
    ws_helpers::send_dashboard_update(
        &state.ws_manager,
        workspace_id,
        &dashboard_id,
        "created",
        &user.user_id,
        changed_by_name,
        Some(&user.user_id),
    )
    .await;

    Ok(Json(dashboard_to_response(&dashboard)))
}

// ---------------------------------------------------------------------------
// GET / — List/search dashboards
// ---------------------------------------------------------------------------

async fn list_dashboards(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<DashboardListParams>,
) -> Result<Json<Vec<DashboardListItem>>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let sort_by = match params.sort_by.as_str() {
        "popularity" => dashboard_service::SearchSort::Popularity,
        "created" => dashboard_service::SearchSort::Created,
        _ => dashboard_service::SearchSort::Recent,
    };

    let limit = params.limit.clamp(1, 100);

    let doc_type_filter = match params.doc_type.as_deref() {
        Some("knowledge") => Some(kyomi_core::models::DocType::Knowledge),
        Some("all") => None,
        _ => Some(kyomi_core::models::DocType::Dashboard), // default: dashboards only
    };

    let results = dashboard_service::search_dashboards(
        &state.db,
        workspace_id,
        params.query.as_deref(),
        doc_type_filter,
        sort_by,
        limit,
    )
    .await?;

    let items: Vec<DashboardListItem> = results.iter().map(search_result_to_list_item).collect();

    Ok(Json(items))
}

// ---------------------------------------------------------------------------
// GET /{dashboard_id} — Get dashboard
// ---------------------------------------------------------------------------

async fn get_dashboard(
    State(state): State<AppState>,
    user: AuthUser,
    Path(dashboard_id): Path<String>,
) -> Result<Json<DashboardResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let dashboard =
        dashboard_service::get_dashboard(&state.db, &dashboard_id, workspace_id).await?;

    let dashboard = dashboard.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!("Dashboard {dashboard_id} not found"))
    })?;

    // Record view for popularity tracking (fire-and-forget)
    let db = state.db.clone();
    let did = dashboard_id.clone();
    let uid = user.user_id.clone();
    let wid = workspace_id.to_string();
    tokio::spawn(async move {
        if let Err(e) = dashboard_service::record_view(&db, &did, &uid, &wid).await {
            tracing::warn!(dashboard_id = %did, error = %e, "Failed to record dashboard view");
        }
    });

    Ok(Json(dashboard_to_response(&dashboard)))
}

// ---------------------------------------------------------------------------
// PATCH /{dashboard_id} — Update dashboard
// ---------------------------------------------------------------------------

async fn update_dashboard(
    State(state): State<AppState>,
    user: AuthUser,
    Path(dashboard_id): Path<String>,
    Json(request): Json<UpdateDashboardRequest>,
) -> Result<Json<DashboardResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Ensure at least one field is provided
    if request.title.is_none() && request.content.is_none() && request.change_summary.is_none() {
        return Err(kyomi_core::Error::BadRequest(
            "No updates provided".into(),
        ));
    }

    dashboard_service::update_dashboard(
        &state.db,
        None, // embed: no rechunking from REST API (yet)
        &dashboard_id,
        workspace_id,
        &user.user_id,
        request.title.as_deref(),
        request.content.as_deref(),
        request.change_summary.as_deref(),
        None, // expected_content_hash: no CAS for dashboard REST
    )
    .await?;

    // Re-embed if content or title changed
    if request.title.is_some() || request.content.is_some() {
        let dashboard =
            dashboard_service::get_dashboard(&state.db, &dashboard_id, workspace_id).await?;
        if let Some(d) = dashboard {
            dashboard_service::spawn_embedding_generation(
                state.db.clone(),
                state.embedding.wait_ready().await?.clone(),
                dashboard_id.clone(),
                workspace_id.to_string(),
                d.title.clone(),
                d.content.clone(),
            );
        }
    }

    // Fetch updated dashboard for response
    let dashboard =
        dashboard_service::get_dashboard(&state.db, &dashboard_id, workspace_id).await?;
    let dashboard = dashboard.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!("Dashboard {dashboard_id} not found"))
    })?;

    // Notify workspace members about the update
    let changed_by_name = user.name.as_deref().unwrap_or(&user.email);
    ws_helpers::send_dashboard_update(
        &state.ws_manager,
        workspace_id,
        &dashboard_id,
        "updated",
        &user.user_id,
        changed_by_name,
        Some(&user.user_id),
    )
    .await;

    Ok(Json(dashboard_to_response(&dashboard)))
}

// ---------------------------------------------------------------------------
// DELETE /{dashboard_id} — Delete dashboard
// ---------------------------------------------------------------------------

async fn delete_dashboard(
    State(state): State<AppState>,
    user: AuthUser,
    Path(dashboard_id): Path<String>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    dashboard_service::delete_dashboard(&state.db, &dashboard_id, workspace_id, &user.user_id)
        .await?;

    // Notify workspace members about the deletion
    let changed_by_name = user.name.as_deref().unwrap_or(&user.email);
    ws_helpers::send_dashboard_update(
        &state.ws_manager,
        workspace_id,
        &dashboard_id,
        "deleted",
        &user.user_id,
        changed_by_name,
        Some(&user.user_id),
    )
    .await;

    Ok(Json(json!({
        "message": "Dashboard deleted",
        "dashboard_id": dashboard_id,
    })))
}

// ---------------------------------------------------------------------------
// GET /{dashboard_id}/versions — List versions
// ---------------------------------------------------------------------------

async fn list_versions(
    State(state): State<AppState>,
    user: AuthUser,
    Path(dashboard_id): Path<String>,
    Query(params): Query<VersionListParams>,
) -> Result<Json<VersionListResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Verify dashboard exists and get its data for current_version
    let dashboard =
        dashboard_service::get_dashboard(&state.db, &dashboard_id, workspace_id).await?;
    let dashboard = dashboard.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!("Dashboard {dashboard_id} not found"))
    })?;

    let limit = params.limit.clamp(1, 100);
    let offset = params.offset.max(0);

    let versions = dashboard_service::list_versions(&state.db, &dashboard_id, limit, offset).await?;
    let total_count = dashboard_service::get_version_count(&state.db, &dashboard_id).await?;

    // Build "current" version representing the live dashboard state
    // (matches Python backend's contract)
    let current_version_number = versions
        .first()
        .map(|v| v.version_number + 1)
        .unwrap_or(1);

    let change_summary = dashboard
        .last_change_summary
        .unwrap_or_else(|| "Current saved version".into());

    let current_version = CurrentVersionResponse {
        version_id: "current".into(),
        dashboard_id: dashboard_id.clone(),
        version_number: current_version_number,
        content: dashboard.content,
        title: dashboard.title,
        change_summary,
        created_at: dashboard.updated_at.to_rfc3339(),
        created_by: CreatedByResponse {
            user_id: dashboard.user_id,
            name: None,
            email: None,
        },
        is_current: true,
    };

    let version_responses: Vec<VersionSummaryResponse> =
        versions.iter().map(version_summary_to_response).collect();

    Ok(Json(VersionListResponse {
        versions: version_responses,
        current_version,
        total_count: total_count + 1, // include current in count
        dashboard_id,
    }))
}

// ---------------------------------------------------------------------------
// GET /{dashboard_id}/versions/diff — Diff two versions
// ---------------------------------------------------------------------------

async fn diff_versions(
    State(state): State<AppState>,
    user: AuthUser,
    Path(dashboard_id): Path<String>,
    Query(params): Query<DiffParams>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Verify dashboard exists
    let dashboard =
        dashboard_service::get_dashboard(&state.db, &dashboard_id, workspace_id).await?;
    if dashboard.is_none() {
        return Err(kyomi_core::Error::NotFound(format!(
            "Dashboard {dashboard_id} not found"
        )));
    }

    let from_version =
        dashboard_service::get_version(&state.db, &dashboard_id, params.from_version).await?;
    let from_version = from_version.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!(
            "Version {} not found",
            params.from_version
        ))
    })?;

    let to_version =
        dashboard_service::get_version(&state.db, &dashboard_id, params.to_version).await?;
    let to_version = to_version.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!(
            "Version {} not found",
            params.to_version
        ))
    })?;

    // Simple line-based diff
    let from_lines: Vec<&str> = from_version.content.lines().collect();
    let to_lines: Vec<&str> = to_version.content.lines().collect();

    let from_set: std::collections::HashSet<&str> = from_lines.iter().copied().collect();
    let to_set: std::collections::HashSet<&str> = to_lines.iter().copied().collect();

    let added: Vec<&str> = to_lines
        .iter()
        .filter(|l| !from_set.contains(**l))
        .copied()
        .collect();
    let removed: Vec<&str> = from_lines
        .iter()
        .filter(|l| !to_set.contains(**l))
        .copied()
        .collect();

    Ok(Json(json!({
        "dashboard_id": dashboard_id,
        "from_version": params.from_version,
        "to_version": params.to_version,
        "from_title": from_version.title,
        "to_title": to_version.title,
        "added_lines": added.len(),
        "removed_lines": removed.len(),
        "added": added,
        "removed": removed,
    })))
}

// ---------------------------------------------------------------------------
// GET /{dashboard_id}/versions/{num} — Get specific version
// ---------------------------------------------------------------------------

async fn get_version(
    State(state): State<AppState>,
    user: AuthUser,
    Path((dashboard_id, num)): Path<(String, i32)>,
) -> Result<Json<VersionResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Verify dashboard exists
    let dashboard =
        dashboard_service::get_dashboard(&state.db, &dashboard_id, workspace_id).await?;
    if dashboard.is_none() {
        return Err(kyomi_core::Error::NotFound(format!(
            "Dashboard {dashboard_id} not found"
        )));
    }

    let version = dashboard_service::get_version(&state.db, &dashboard_id, num).await?;
    let version = version.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!(
            "Version {num} not found for dashboard {dashboard_id}"
        ))
    })?;

    Ok(Json(version_to_response(&version)))
}

// ---------------------------------------------------------------------------
// POST /{dashboard_id}/versions/{num}/restore — Restore version
// ---------------------------------------------------------------------------

async fn restore_version(
    State(state): State<AppState>,
    user: AuthUser,
    Path((dashboard_id, num)): Path<(String, i32)>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let new_version = dashboard_service::restore_version(
        &state.db,
        &dashboard_id,
        workspace_id,
        &user.user_id,
        num,
    )
    .await?;

    // Re-embed after restore
    let dashboard =
        dashboard_service::get_dashboard(&state.db, &dashboard_id, workspace_id).await?;
    if let Some(d) = dashboard {
        dashboard_service::spawn_embedding_generation(
            state.db.clone(),
            state.embedding.wait_ready().await?.clone(),
            dashboard_id.clone(),
            workspace_id.to_string(),
            d.title.clone(),
            d.content.clone(),
        );
    }

    // Notify workspace members about the restore (treated as an update)
    let changed_by_name = user.name.as_deref().unwrap_or(&user.email);
    ws_helpers::send_dashboard_update(
        &state.ws_manager,
        workspace_id,
        &dashboard_id,
        "updated",
        &user.user_id,
        changed_by_name,
        Some(&user.user_id),
    )
    .await;

    Ok(Json(json!({
        "message": format!("Restored to version {num}"),
        "dashboard_id": dashboard_id,
        "new_version": new_version,
    })))
}

// ---------------------------------------------------------------------------
// GET /{dashboard_id}/export/pdf — Export dashboard as PDF
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ExportPdfParams {
    #[serde(default)]
    parameters: Option<String>,
}

async fn export_pdf(
    State(state): State<AppState>,
    user: AuthUser,
    Path(dashboard_id): Path<String>,
    Query(params): Query<ExportPdfParams>,
) -> impl IntoResponse {
    export_pdf_inner(state, user, dashboard_id, params).await.into_response()
}

async fn export_pdf_inner(
    state: AppState,
    user: AuthUser,
    dashboard_id: String,
    params: ExportPdfParams,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // 1. Self-hosted edition gate: PDF export requires Enterprise + chart renderer.
    if state.config.self_hosted {
        if !state.config.is_enterprise() {
            return Ok((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": "feature_not_available",
                    "edition_required": "enterprise",
                    "message": "PDF export requires an Enterprise license. See kyomi.ai/enterprise."
                })),
            )
                .into_response());
        }
        if !state.config.chart_renderer_configured() {
            return Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "service_not_configured",
                    "service": "chart_renderer",
                    "message": "PDF export requires the chart renderer service. Set CHART_RENDERER_URL."
                })),
            )
                .into_response());
        }
    }

    // 2. Capability gate: PDF export requires paid plan
    let workspace = workspace_service::get_workspace_full(&state.db, workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Workspace not found".into()))?;
    let capabilities = if state.config.self_hosted {
        capability::compute_capabilities_self_hosted(false)
    } else {
        capability::compute_capabilities(&workspace, false)
    };
    if !capabilities.pdf_export_enabled {
        return Err(kyomi_core::Error::Forbidden(
            "PDF export requires a Pro or Team plan. Please upgrade to access this feature.".into(),
        ));
    }

    // 2. Fetch dashboard
    let dashboard =
        dashboard_service::get_dashboard(&state.db, &dashboard_id, workspace_id).await?;
    let dashboard = dashboard.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!("Dashboard {dashboard_id} not found"))
    })?;

    // 3. Parse optional parameter values
    let parameter_values: Option<Value> = if let Some(ref params_json) = params.parameters {
        Some(serde_json::from_str(params_json).map_err(|_| {
            kyomi_core::Error::BadRequest("Invalid parameters JSON".into())
        })?)
    } else {
        None
    };

    // 4. Get user palette
    let user_palette =
        chart_palettes::get_user_palette(&state.db, &user.user_id).await;

    // 5. Build QueryContext
    let query_ctx = kyomi_agent::tools::QueryContext {
        db: state.db.clone(),
        user_id: user.user_id.clone(),
        workspace_id: workspace_id.to_string(),
        encryption_key: state.encryption_key.clone(),
        config: state.config.clone(),
        connect_registry: Some(state.connect_registry.clone()),
    };

    // 6. Generate PDF
    let pdf_bytes = kyomi_agent::pdf_export::generate_dashboard_pdf(
        &dashboard.content,
        &dashboard.title,
        &query_ctx,
        &state.config.chart_renderer_url,
        &user_palette,
        parameter_values.as_ref(),
    )
    .await
    .map_err(|e| kyomi_core::Error::Internal(format!("PDF generation failed: {e}")))?;

    // 7. Build filename from title
    let safe_title: String = dashboard
        .title
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join("_");
    let filename = if safe_title.is_empty() {
        "Dashboard.pdf".to_string()
    } else {
        format!("{safe_title}.pdf")
    };

    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        pdf_bytes,
    )
        .into_response())
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // CreateDashboardRequest
    // -----------------------------------------------------------------------

    #[test]
    fn create_dashboard_request_with_all_fields() {
        let json = json!({
            "title": "Sales Dashboard",
            "content": "# Sales\n\nMetrics overview"
        });

        let req: CreateDashboardRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.title, "Sales Dashboard");
        assert_eq!(req.content, "# Sales\n\nMetrics overview");
    }

    #[test]
    fn create_dashboard_request_content_defaults_to_empty() {
        let json = json!({"title": "Empty Dashboard"});

        let req: CreateDashboardRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.title, "Empty Dashboard");
        assert_eq!(req.content, "");
    }

    #[test]
    fn create_dashboard_request_fails_without_title() {
        let json = json!({"content": "some content"});
        assert!(serde_json::from_value::<CreateDashboardRequest>(json).is_err());
    }

    #[test]
    fn create_dashboard_request_round_trip() {
        let json = json!({"title": "Test", "content": "# Test"});
        let req: CreateDashboardRequest = serde_json::from_value(json).unwrap();
        let serialized = serde_json::to_value(&req).unwrap();
        let deserialized: CreateDashboardRequest = serde_json::from_value(serialized).unwrap();
        assert_eq!(deserialized.title, req.title);
        assert_eq!(deserialized.content, req.content);
    }

    // -----------------------------------------------------------------------
    // UpdateDashboardRequest
    // -----------------------------------------------------------------------

    #[test]
    fn update_dashboard_request_all_fields() {
        let json = json!({
            "title": "Updated Title",
            "content": "New content",
            "change_summary": "Changed title and content"
        });

        let req: UpdateDashboardRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.title.as_deref(), Some("Updated Title"));
        assert_eq!(req.content.as_deref(), Some("New content"));
        assert_eq!(req.change_summary.as_deref(), Some("Changed title and content"));
    }

    #[test]
    fn update_dashboard_request_partial_title_only() {
        let json = json!({"title": "New Title"});

        let req: UpdateDashboardRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.title.as_deref(), Some("New Title"));
        assert!(req.content.is_none());
        assert!(req.change_summary.is_none());
    }

    #[test]
    fn update_dashboard_request_empty_object_all_none() {
        let json = json!({});

        let req: UpdateDashboardRequest = serde_json::from_value(json).unwrap();
        assert!(req.title.is_none());
        assert!(req.content.is_none());
        assert!(req.change_summary.is_none());
    }

    #[test]
    fn update_dashboard_request_round_trip() {
        let json = json!({"title": "T", "content": "C"});
        let req: UpdateDashboardRequest = serde_json::from_value(json).unwrap();
        let serialized = serde_json::to_value(&req).unwrap();
        let deserialized: UpdateDashboardRequest = serde_json::from_value(serialized).unwrap();
        assert_eq!(deserialized.title, req.title);
        assert_eq!(deserialized.content, req.content);
    }

    // -----------------------------------------------------------------------
    // DashboardResponse
    // -----------------------------------------------------------------------

    #[test]
    fn dashboard_response_serializes_all_fields() {
        let response = DashboardResponse {
            dashboard_id: "dash-123".into(),
            user_id: "user-abc".into(),
            workspace_id: "ws-xyz".into(),
            title: "Sales Dashboard".into(),
            content: "# Sales".into(),
            summary: Some("Tracks sales metrics".into()),
            last_change_summary: Some("Added chart".into()),
            created_at: "2025-01-15T09:00:00+00:00".into(),
            updated_at: "2025-01-16T10:00:00+00:00".into(),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["dashboard_id"], "dash-123");
        assert_eq!(json["user_id"], "user-abc");
        assert_eq!(json["workspace_id"], "ws-xyz");
        assert_eq!(json["title"], "Sales Dashboard");
        assert_eq!(json["content"], "# Sales");
        assert_eq!(json["summary"], "Tracks sales metrics");
        assert_eq!(json["last_change_summary"], "Added chart");
    }

    #[test]
    fn dashboard_response_null_optional_fields() {
        let response = DashboardResponse {
            dashboard_id: "dash-123".into(),
            user_id: "user-abc".into(),
            workspace_id: "ws-xyz".into(),
            title: "Test".into(),
            content: "".into(),
            summary: None,
            last_change_summary: None,
            created_at: "2025-01-15T09:00:00+00:00".into(),
            updated_at: "2025-01-15T09:00:00+00:00".into(),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(json["summary"].is_null());
        assert!(json["last_change_summary"].is_null());
    }

    #[test]
    fn dashboard_response_round_trip() {
        let response = DashboardResponse {
            dashboard_id: "d1".into(),
            user_id: "u1".into(),
            workspace_id: "w1".into(),
            title: "Title".into(),
            content: "Content".into(),
            summary: None,
            last_change_summary: None,
            created_at: "2025-01-15T09:00:00+00:00".into(),
            updated_at: "2025-01-15T09:00:00+00:00".into(),
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: DashboardResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.dashboard_id, "d1");
        assert_eq!(deserialized.title, "Title");
    }

    // -----------------------------------------------------------------------
    // DashboardListItem
    // -----------------------------------------------------------------------

    #[test]
    fn dashboard_list_item_serializes() {
        let item = DashboardListItem {
            dashboard_id: "dash-1".into(),
            user_id: "user-1".into(),
            workspace_id: "ws-1".into(),
            title: "My Dashboard".into(),
            content: "# Content".into(),
            content_preview: Some("Content preview text".into()),
            last_change_summary: None,
            created_at: "2025-01-15T09:00:00+00:00".into(),
            updated_at: "2025-01-16T10:00:00+00:00".into(),
            popularity_score: 4.5,
            view_count: 10,
            recent_views: 3,
        };

        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["dashboard_id"], "dash-1");
        assert_eq!(json["popularity_score"], 4.5);
        assert_eq!(json["view_count"], 10);
        assert_eq!(json["recent_views"], 3);
        assert_eq!(json["content_preview"], "Content preview text");
    }

    #[test]
    fn dashboard_list_item_round_trip() {
        let item = DashboardListItem {
            dashboard_id: "d1".into(),
            user_id: "u1".into(),
            workspace_id: "w1".into(),
            title: "T".into(),
            content: "C".into(),
            content_preview: None,
            last_change_summary: None,
            created_at: "2025-01-15T09:00:00+00:00".into(),
            updated_at: "2025-01-15T09:00:00+00:00".into(),
            popularity_score: 0.0,
            view_count: 0,
            recent_views: 0,
        };

        let json_str = serde_json::to_string(&item).unwrap();
        let deserialized: DashboardListItem = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.dashboard_id, "d1");
        assert_eq!(deserialized.view_count, 0);
    }

    // -----------------------------------------------------------------------
    // VersionResponse
    // -----------------------------------------------------------------------

    #[test]
    fn version_response_serializes_all_fields() {
        let response = VersionResponse {
            version_id: 1,
            dashboard_id: "dash-123".into(),
            version_number: 3,
            content: "# Dashboard v3".into(),
            title: "Dashboard Title".into(),
            change_summary: Some("Updated charts".into()),
            byte_size: Some(1024),
            created_at: "2025-01-15T09:00:00+00:00".into(),
            created_by: "user-abc".into(),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["version_id"], 1);
        assert_eq!(json["dashboard_id"], "dash-123");
        assert_eq!(json["version_number"], 3);
        assert_eq!(json["content"], "# Dashboard v3");
        assert_eq!(json["title"], "Dashboard Title");
        assert_eq!(json["change_summary"], "Updated charts");
        assert_eq!(json["byte_size"], 1024);
    }

    #[test]
    fn version_response_round_trip() {
        let response = VersionResponse {
            version_id: 1,
            dashboard_id: "d1".into(),
            version_number: 1,
            content: "c".into(),
            title: "t".into(),
            change_summary: None,
            byte_size: None,
            created_at: "2025-01-15T09:00:00+00:00".into(),
            created_by: "u1".into(),
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: VersionResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.version_number, 1);
    }

    // -----------------------------------------------------------------------
    // VersionListResponse
    // -----------------------------------------------------------------------

    #[test]
    fn version_list_response_serializes() {
        let current = CurrentVersionResponse {
            version_id: "v-5".into(),
            dashboard_id: "dash-123".into(),
            version_number: 5,
            content: "# Test".into(),
            title: "Test Dashboard".into(),
            change_summary: "Initial".into(),
            created_at: "2025-01-01T00:00:00+00:00".into(),
            created_by: CreatedByResponse {
                user_id: "u-1".into(),
                name: Some("Test".into()),
                email: Some("test@test.com".into()),
            },
            is_current: true,
        };
        let response = VersionListResponse {
            versions: vec![],
            current_version: current,
            total_count: 5,
            dashboard_id: "dash-123".into(),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(json["current_version"].is_object());
        assert_eq!(json["total_count"], 5);
        assert_eq!(json["dashboard_id"], "dash-123");
        assert!(json["versions"].is_array());
    }

    #[test]
    fn version_list_response_round_trip() {
        let current = CurrentVersionResponse {
            version_id: "v-1".into(),
            dashboard_id: "d1".into(),
            version_number: 1,
            content: "# Test".into(),
            title: "Test".into(),
            change_summary: "Init".into(),
            created_at: "2025-01-01T00:00:00+00:00".into(),
            created_by: CreatedByResponse {
                user_id: "u-1".into(),
                name: None,
                email: None,
            },
            is_current: true,
        };
        let response = VersionListResponse {
            versions: vec![],
            current_version: current,
            total_count: 1,
            dashboard_id: "d1".into(),
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: VersionListResponse = serde_json::from_str(&json_str).unwrap();
        assert!(deserialized.current_version.is_current);
        assert_eq!(deserialized.dashboard_id, "d1");
    }

    // -----------------------------------------------------------------------
    // Query params
    // -----------------------------------------------------------------------

    #[test]
    fn dashboard_list_params_defaults() {
        let json = json!({});
        let params: DashboardListParams = serde_json::from_value(json).unwrap();
        assert!(params.query.is_none());
        assert_eq!(params.sort_by, "recent");
        assert_eq!(params.limit, 50);
    }

    #[test]
    fn dashboard_list_params_custom_values() {
        let json = json!({
            "query": "sales",
            "sort_by": "popularity",
            "limit": 10
        });

        let params: DashboardListParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.query.as_deref(), Some("sales"));
        assert_eq!(params.sort_by, "popularity");
        assert_eq!(params.limit, 10);
    }

    #[test]
    fn version_list_params_defaults() {
        let json = json!({});
        let params: VersionListParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.limit, 20);
        assert_eq!(params.offset, 0);
    }

    #[test]
    fn diff_params_requires_both_versions() {
        let json = json!({"from_version": 1, "to_version": 3});
        let params: DiffParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.from_version, 1);
        assert_eq!(params.to_version, 3);
    }

    #[test]
    fn diff_params_fails_without_required() {
        let json = json!({"from_version": 1});
        assert!(serde_json::from_value::<DiffParams>(json).is_err());
    }

    // -----------------------------------------------------------------------
    // Default function tests
    // -----------------------------------------------------------------------

    #[test]
    fn default_sort_by_is_recent() {
        assert_eq!(default_sort_by(), "recent");
    }

    #[test]
    fn default_limit_is_50() {
        assert_eq!(default_limit(), 50);
    }

    #[test]
    fn default_version_limit_is_20() {
        assert_eq!(default_version_limit(), 20);
    }

    // -----------------------------------------------------------------------
    // CreatedByResponse
    // -----------------------------------------------------------------------

    #[test]
    fn created_by_response_serializes() {
        let response = CreatedByResponse {
            user_id: "user-abc".into(),
            name: Some("John Doe".into()),
            email: Some("john@example.com".into()),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["user_id"], "user-abc");
        assert_eq!(json["name"], "John Doe");
        assert_eq!(json["email"], "john@example.com");
    }

    #[test]
    fn created_by_response_null_name() {
        let response = CreatedByResponse {
            user_id: "user-abc".into(),
            name: None,
            email: Some("john@example.com".into()),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(json["name"].is_null());
    }

    // -----------------------------------------------------------------------
    // VersionSummaryResponse
    // -----------------------------------------------------------------------

    #[test]
    fn version_summary_response_serializes() {
        let response = VersionSummaryResponse {
            version_id: 1,
            version_number: 2,
            title: "Dashboard v2".into(),
            change_summary: Some("Added chart".into()),
            byte_size: Some(512),
            created_at: "2025-01-15T09:00:00+00:00".into(),
            created_by: CreatedByResponse {
                user_id: "user-abc".into(),
                name: Some("John".into()),
                email: Some("john@example.com".into()),
            },
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["version_number"], 2);
        assert_eq!(json["title"], "Dashboard v2");
        assert!(json["created_by"].is_object());
        assert_eq!(json["created_by"]["user_id"], "user-abc");
    }
}
