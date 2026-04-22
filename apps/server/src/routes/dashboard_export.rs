// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard PDF export endpoint.
//!
//! This route returns binary PDF content and cannot be a Leptos server_fn,
//! so it is kept as a dedicated REST endpoint.
//!
//! ## Endpoint
//!
//! - `GET /{dashboard_id}/export/pdf` — Export dashboard as PDF

use axum::{
    extract::{Path, Query, State},
    http::header,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;

use kyomi_agent::tools::chart_palettes;
use kyomi_auth::{dashboard_service, middleware::AuthUser, workspace_service};
use kyomi_core::capability;

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the `/dashboards` router with the PDF export endpoint.
pub fn routes() -> Router<AppState> {
    Router::new().route("/{dashboard_id}/export/pdf", get(export_pdf))
}

// ===========================================================================
// Request types
// ===========================================================================

#[derive(Deserialize)]
struct ExportPdfParams {
    #[serde(default)]
    parameters: Option<String>,
}

// ===========================================================================
// Handler
// ===========================================================================

// ---------------------------------------------------------------------------
// GET /{dashboard_id}/export/pdf — Export dashboard as PDF
// ---------------------------------------------------------------------------

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
    let workspace_id = user
        .workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("Workspace context required".into()))?;

    // 1. Self-hosted edition gate: PDF export requires Enterprise + chart renderer.
    if state.config.self_hosted {
        if !state.config.is_enterprise() {
            return Ok((
                axum::http::StatusCode::FORBIDDEN,
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
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
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
        capability::compute_capabilities_self_hosted()
    } else {
        capability::compute_capabilities(&workspace)
    };
    if !capabilities.pdf_export_enabled {
        return Err(kyomi_core::Error::Forbidden(
            "PDF export requires a Pro or Team plan. Please upgrade to access this feature.".into(),
        ));
    }

    // 3. Fetch dashboard
    let dashboard =
        dashboard_service::get_dashboard(&state.db, &dashboard_id, workspace_id).await?;
    let dashboard = dashboard.ok_or_else(|| {
        kyomi_core::Error::NotFound(format!("Dashboard {dashboard_id} not found"))
    })?;

    // 4. Parse optional parameter values
    let parameter_values: Option<Value> = if let Some(ref params_json) = params.parameters {
        Some(serde_json::from_str(params_json).map_err(|_| {
            kyomi_core::Error::BadRequest("Invalid parameters JSON".into())
        })?)
    } else {
        None
    };

    // 5. Get user palette
    let user_palette =
        chart_palettes::get_user_palette(&state.db, &user.user_id).await;

    // 6. Build QueryContext
    let query_ctx = kyomi_agent::tools::QueryContext {
        db: state.db.clone(),
        user_id: user.user_id.clone(),
        workspace_id: workspace_id.to_string(),
        encryption_key: state.encryption_key.clone(),
        config: state.config.clone(),
        connect_registry: Some(state.connect_registry.clone()),
    };

    // 7. Generate PDF
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

    // 8. Build filename from title
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
