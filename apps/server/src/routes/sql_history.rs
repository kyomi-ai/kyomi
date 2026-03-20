// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL query history endpoints.
//!
//! Wire-compatible with Python's SQL history routes.
//! Provides CRUD for recorded query executions.
//!
//! ## Endpoints
//!
//! - `POST   /api/v1/sql/history`              — create_query_history
//! - `GET    /api/v1/sql/history`              — list_query_history
//! - `GET    /api/v1/sql/history/{query_id}`   — get_query_history
//! - `PATCH  /api/v1/sql/history/{query_id}`   — update_query_history
//! - `DELETE /api/v1/sql/history/{query_id}`   — delete_query_history

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use kyomi_auth::middleware::AuthUser;

use crate::state::AppState;

/// Build the `/sql/history` router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/history",
            get(list_query_history).post(create_query_history),
        )
        .route(
            "/history/{query_id}",
            get(get_query_history)
                .patch(update_query_history)
                .delete(delete_query_history),
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
        .ok_or_else(|| kyomi_core::Error::BadRequest("User not associated with a workspace".into()))
}

// ===========================================================================
// Request / Response Types
// ===========================================================================

#[derive(Deserialize)]
struct CreateQueryHistoryRequest {
    query_text: String,
    #[serde(default)]
    execution_time_ms: Option<i32>,
    #[serde(default)]
    bytes_processed: Option<i64>,
    #[serde(default)]
    row_count: Option<i32>,
    #[serde(default = "default_status")]
    status: String,
    #[serde(default)]
    error_message: Option<String>,
    /// Datasource slug (optional).
    #[serde(default)]
    datasource: Option<String>,
}

fn default_status() -> String {
    "success".to_string()
}

#[derive(Serialize)]
struct QueryHistoryResponse {
    query_id: String,
    workspace_id: String,
    user_id: String,
    query_text: String,
    executed_at: String,
    execution_time_ms: Option<i32>,
    bytes_processed: Option<i64>,
    row_count: Option<i32>,
    status: String,
    error_message: Option<String>,
    is_saved: bool,
    query_name: Option<String>,
    tags: Option<String>,
    datasource_id: Option<String>,
    datasource_slug: Option<String>,
    created_at: String,
    updated_at: String,
}

impl QueryHistoryResponse {
    fn from_model(
        h: kyomi_core::models::SqlQueryHistory,
        slug: Option<String>,
    ) -> Self {
        Self {
            query_id: h.query_id,
            workspace_id: h.workspace_id,
            user_id: h.user_id,
            query_text: h.query_text,
            executed_at: h.executed_at.to_rfc3339(),
            execution_time_ms: h.execution_time_ms,
            bytes_processed: h.bytes_processed,
            row_count: h.row_count,
            status: h.status,
            error_message: h.error_message,
            is_saved: h.is_saved,
            query_name: h.query_name,
            tags: h.tags,
            datasource_id: h.datasource_config_id,
            datasource_slug: slug,
            created_at: h.created_at.to_rfc3339(),
            updated_at: h.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Deserialize)]
struct ListQueryHistoryParams {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    saved_only: bool,
    #[serde(default)]
    search: Option<String>,
}

fn default_limit() -> i64 {
    50
}

#[derive(Deserialize)]
struct UpdateQueryHistoryRequest {
    #[serde(default)]
    is_saved: Option<bool>,
    #[serde(default)]
    query_name: Option<String>,
    #[serde(default)]
    tags: Option<String>,
}

// ===========================================================================
// Endpoint Handlers
// ===========================================================================

// ---------------------------------------------------------------------------
// POST /history — Create query history
// ---------------------------------------------------------------------------

async fn create_query_history(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<CreateQueryHistoryRequest>,
) -> Result<(StatusCode, Json<QueryHistoryResponse>), kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Resolve datasource slug to ID if provided
    let datasource_config_id = if let Some(ref slug) = request.datasource {
        let ds = kyomi_auth::datasource_service::get_datasource_by_slug(
            &state.db,
            slug,
            workspace_id,
        )
        .await?;
        ds.map(|d| d.id)
    } else {
        None
    };

    let record = kyomi_auth::sql_history_service::create_query_history(
        &state.db,
        workspace_id,
        &user.user_id,
        datasource_config_id.as_deref(),
        &request.query_text,
        request.execution_time_ms,
        request.bytes_processed,
        request.row_count,
        &request.status,
        request.error_message.as_deref(),
    )
    .await?;

    // Resolve slug for response
    let slug = request.datasource.clone();

    tracing::info!(
        "Created SQL history {} for user {} in workspace {}",
        record.query_id,
        user.user_id,
        workspace_id
    );

    Ok((StatusCode::CREATED, Json(QueryHistoryResponse::from_model(record, slug))))
}

// ---------------------------------------------------------------------------
// GET /history — List query history
// ---------------------------------------------------------------------------

async fn list_query_history(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ListQueryHistoryParams>,
) -> Result<Json<Vec<QueryHistoryResponse>>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let records = kyomi_auth::sql_history_service::list_query_history(
        &state.db,
        workspace_id,
        &user.user_id,
        params.limit.clamp(1, 1000),
        params.offset.max(0),
        params.saved_only,
        params.search.as_deref(),
    )
    .await?;

    let result: Vec<QueryHistoryResponse> = records
        .into_iter()
        .map(|(h, slug)| QueryHistoryResponse::from_model(h, slug))
        .collect();

    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// GET /history/{query_id} — Get single query history
// ---------------------------------------------------------------------------

async fn get_query_history(
    State(state): State<AppState>,
    user: AuthUser,
    Path(query_id): Path<String>,
) -> Result<Json<QueryHistoryResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let result = kyomi_auth::sql_history_service::get_query_history(
        &state.db,
        &query_id,
        workspace_id,
        &user.user_id,
    )
    .await?;

    match result {
        Some((h, slug)) => Ok(Json(QueryHistoryResponse::from_model(h, slug))),
        None => Err(kyomi_core::Error::NotFound(format!(
            "Query history '{query_id}' not found"
        ))),
    }
}

// ---------------------------------------------------------------------------
// PATCH /history/{query_id} — Update query history
// ---------------------------------------------------------------------------

async fn update_query_history(
    State(state): State<AppState>,
    user: AuthUser,
    Path(query_id): Path<String>,
    Json(request): Json<UpdateQueryHistoryRequest>,
) -> Result<Json<QueryHistoryResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let result = kyomi_auth::sql_history_service::update_query_history(
        &state.db,
        &query_id,
        workspace_id,
        &user.user_id,
        request.is_saved,
        request.query_name.as_deref(),
        request.tags.as_deref(),
    )
    .await?;

    match result {
        Some((h, slug)) => {
            tracing::info!(
                "Updated SQL history {} for user {}",
                query_id,
                user.user_id
            );
            Ok(Json(QueryHistoryResponse::from_model(h, slug)))
        }
        None => Err(kyomi_core::Error::NotFound(format!(
            "Query history '{query_id}' not found"
        ))),
    }
}

// ---------------------------------------------------------------------------
// DELETE /history/{query_id} — Delete query history
// ---------------------------------------------------------------------------

async fn delete_query_history(
    State(state): State<AppState>,
    user: AuthUser,
    Path(query_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let deleted = kyomi_auth::sql_history_service::delete_query_history(
        &state.db,
        &query_id,
        workspace_id,
        &user.user_id,
    )
    .await?;

    if deleted {
        tracing::info!(
            "Deleted SQL history {} for user {}",
            query_id,
            user.user_id
        );
        Ok(Json(json!({"success": true, "message": "Query deleted"})))
    } else {
        Err(kyomi_core::Error::NotFound(format!(
            "Query history '{query_id}' not found"
        )))
    }
}
