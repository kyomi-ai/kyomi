// SPDX-License-Identifier: AGPL-3.0-or-later

//! HTTP handler for `GET /api/v1/connect/info`.
//!
//! Returns metadata about the datasource associated with a Connect JWT token.
//! Used by the Connect setup wizard to display the datasource name, type, and
//! workspace before the operator starts the Connect binary.
//!
//! Authentication follows the same pattern as [`super::handler`]: extract a
//! Bearer token from the `Authorization` header, verify with the
//! `ConnectTokenService`, load the datasource config, and verify
//! `connection_type` + `jti`.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;

use kyomi_core::datasource_registry;

use super::extract_bearer_token;
use crate::state::AppState;

/// Response body for `GET /api/v1/connect/info`.
#[derive(Serialize)]
pub struct ConnectInfoResponse {
    pub datasource_name: String,
    pub datasource_type: String,
    pub datasource_type_label: String,
    pub workspace_name: String,
    pub default_port: Option<u16>,
}

/// Axum handler for `GET /api/v1/connect/info`.
///
/// Authenticates with a Connect JWT (same verification as the WebSocket handler),
/// then returns metadata about the datasource and workspace.
pub async fn connect_info(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // -----------------------------------------------------------------------
    // 1. Extract Bearer token from Authorization header
    // -----------------------------------------------------------------------
    let token = match extract_bearer_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "Authorization header required" })),
            )
                .into_response();
        }
    };

    // -----------------------------------------------------------------------
    // 2. Verify JWT via ConnectTokenService
    // -----------------------------------------------------------------------
    let connect_token_service = match &state.connect_token {
        Some(svc) => svc.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "Connect not configured" })),
            )
                .into_response();
        }
    };

    let claims = match connect_token_service.verify(&token) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "Connect info JWT verification failed");
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "Invalid token" })),
            )
                .into_response();
        }
    };

    let datasource_config_id = &claims.dsid;
    let workspace_id = &claims.wid;

    // -----------------------------------------------------------------------
    // 3. Load datasource config and verify connection_type + jti
    // -----------------------------------------------------------------------
    let ds_config = match kyomi_auth::datasource_service::get_datasource(
        &state.db,
        datasource_config_id,
        workspace_id,
    )
    .await
    {
        Ok(Some(ds)) => ds,
        Ok(None) => {
            tracing::warn!(
                datasource_config_id,
                workspace_id,
                "Connect info rejected: datasource not found"
            );
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "Datasource not found" })),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!(
                datasource_config_id,
                error = %e,
                "Connect info: database error loading datasource"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal error" })),
            )
                .into_response();
        }
    };

    // Must be a "connect" type datasource
    if ds_config.connection_type != "connect" {
        tracing::warn!(
            datasource_config_id,
            connection_type = %ds_config.connection_type,
            "Connect info rejected: datasource is not a Connect type"
        );
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "Datasource is not Connect type" })),
        )
            .into_response();
    }

    // Verify the token's jti matches the stored jti (revocation check)
    match &ds_config.connect_token_jti {
        Some(stored_jti) if stored_jti == &claims.jti => {
            // Token is current -- proceed
        }
        Some(_) => {
            tracing::warn!(
                datasource_config_id,
                "Connect info rejected: token has been revoked (jti mismatch)"
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "Token revoked" })),
            )
                .into_response();
        }
        None => {
            tracing::warn!(
                datasource_config_id,
                "Connect info rejected: no token jti stored"
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "Token not recognized" })),
            )
                .into_response();
        }
    }

    // -----------------------------------------------------------------------
    // 4. Load workspace name
    // -----------------------------------------------------------------------
    #[derive(sqlx::FromRow)]
    struct WorkspaceName { name: Option<String> }

    let workspace_name = match kyomi_core::db_fetch_optional!(
        &state.db, WorkspaceName,
        "SELECT name FROM workspaces WHERE workspace_id = $1",
        workspace_id
    ) {
        Ok(Some(row)) => row.name.unwrap_or_default(),
        Ok(None) => {
            tracing::warn!(workspace_id, "Connect info: workspace not found");
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "Workspace not found" })),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!(
                workspace_id,
                error = %e,
                "Connect info: database error loading workspace"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Internal error" })),
            )
                .into_response();
        }
    };

    // -----------------------------------------------------------------------
    // 5. Build response with datasource metadata
    // -----------------------------------------------------------------------
    let ds_type: datasource_registry::DatasourceType = ds_config.datasource_type.into();

    (
        StatusCode::OK,
        Json(ConnectInfoResponse {
            datasource_name: ds_config.name,
            datasource_type: ds_type.as_str().to_string(),
            datasource_type_label: ds_type.display_name().to_string(),
            workspace_name,
            default_port: ds_type.default_port(),
        }),
    )
        .into_response()
}

