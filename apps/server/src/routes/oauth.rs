// SPDX-License-Identifier: AGPL-3.0-or-later

//! OAuth 2.0 endpoints for MCP client authentication.
//!
//! Implements:
//! - RFC 8414: OAuth 2.0 Authorization Server Metadata (`.well-known/oauth-authorization-server`)
//! - RFC 9728: OAuth 2.0 Protected Resource Metadata (`.well-known/oauth-protected-resource`)
//! - OpenID Connect Discovery (`.well-known/openid-configuration`)
//! - RFC 7591: Dynamic Client Registration (`/api/v1/oauth/register`)
//! - OAuth 2.0 Authorization Code Flow (`/api/v1/oauth/authorize`, `/api/v1/oauth/token`)
//!
//! All token creation reuses `kyomi_auth::jwt`, `kyomi_auth::token_service`, and
//! `kyomi_auth::redis_ops` — no credential logic is duplicated.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Json, Router,
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use kyomi_auth::{jwt, redis_ops, request_meta, token_service, user_service};

use super::route_error::RouteError;
use crate::state::AppState;

// ===========================================================================
// Well-known discovery routes (mounted at root level, no /api/v1 prefix)
// ===========================================================================

/// Build the well-known discovery router (mounted at root, not under /api/v1).
pub fn well_known_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_authorization_server_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth_protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/{*path}",
            get(oauth_protected_resource_metadata_with_path),
        )
        .route(
            "/.well-known/openid-configuration",
            get(openid_configuration),
        )
}

/// Build the OAuth action router (mounted under /api/v1/oauth).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/authorize", get(oauth_authorize))
        .route("/authorize/continue", get(oauth_authorize_continue))
        .route("/token", post(oauth_token))
        .route("/register", post(register_client))
}

// ===========================================================================
// Shared helpers
// ===========================================================================

/// Build the base URL from config (e.g., "https://dev.kyomi.ai").
fn base_url(state: &AppState) -> String {
    state.config.base_url.trim_end_matches('/').to_string()
}

/// Build the base URL from request headers (X-Forwarded-Proto + Host).
///
/// When behind nginx/proxy, the client's actual URL may differ from `config.base_url`.
/// MCP clients (Cursor) validate that the protected resource URL matches
/// the URL they connected to, so we must return the URL as seen by the client.
fn base_url_from_request(headers: &HeaderMap, state: &AppState) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| state.config.base_url.trim_start_matches("https://").trim_start_matches("http://"));
    format!("{scheme}://{host}")
}

/// Standard OAuth metadata shared by multiple discovery endpoints.
fn oauth_metadata(base: &str) -> serde_json::Value {
    json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/api/v1/oauth/authorize"),
        "token_endpoint": format!("{base}/api/v1/oauth/token"),
        "registration_endpoint": format!("{base}/api/v1/oauth/register"),
        "scopes_supported": ["mcp"],
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none"],
    })
}

// ===========================================================================
// Discovery endpoints
// ===========================================================================

/// `GET /.well-known/oauth-authorization-server` — RFC 8414.
///
/// Returns 404 in personal mode — no OAuth needed for single-user desktop app.
/// MCP clients interpret missing discovery as "no auth required" and connect directly.
async fn oauth_authorization_server_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if state.config.is_personal() {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(oauth_metadata(&base_url_from_request(&headers, &state))))
}

/// `GET /.well-known/oauth-protected-resource` — RFC 9728.
///
/// Returns 404 in personal mode — no OAuth needed for single-user desktop app.
async fn oauth_protected_resource_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if state.config.is_personal() {
        return Err(StatusCode::NOT_FOUND);
    }
    let base = base_url_from_request(&headers, &state);
    Ok(Json(json!({
        "resource": base,
        "authorization_servers": [base],
        "scopes_supported": ["mcp"],
        "bearer_methods_supported": ["header"],
    })))
}

/// `GET /.well-known/oauth-protected-resource/{*path}` — RFC 9728 with resource path.
///
/// Returns 404 in personal mode — no OAuth needed for single-user desktop app.
async fn oauth_protected_resource_metadata_with_path(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if state.config.is_personal() {
        return Err(StatusCode::NOT_FOUND);
    }
    let base = base_url_from_request(&headers, &state);
    Ok(Json(json!({
        "resource": format!("{base}/{path}"),
        "authorization_servers": [base],
        "scopes_supported": ["mcp"],
        "bearer_methods_supported": ["header"],
    })))
}

/// `GET /.well-known/openid-configuration` — OpenID Connect Discovery.
///
/// Returns 404 in personal mode — no OAuth needed for single-user desktop app.
async fn openid_configuration(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if state.config.is_personal() {
        return Err(StatusCode::NOT_FOUND);
    }
    let mut meta = oauth_metadata(&base_url_from_request(&headers, &state));
    if let Some(obj) = meta.as_object_mut() {
        obj.insert("subject_types_supported".into(), json!(["public"]));
    }
    Ok(Json(meta))
}

// ===========================================================================
// MCP-relative discovery (mounted inside the /mcp router)
// ===========================================================================

/// `GET /mcp/.well-known/openid-configuration` — OAuth discovery relative to MCP URL.
///
/// Some MCP clients look for discovery relative to the server URL.
/// Returns 404 in personal mode — no OAuth needed for single-user desktop app.
pub async fn mcp_openid_configuration(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if state.config.is_personal() {
        return Err(StatusCode::NOT_FOUND);
    }
    let mut meta = oauth_metadata(&base_url_from_request(&headers, &state));
    if let Some(obj) = meta.as_object_mut() {
        obj.insert("subject_types_supported".into(), json!(["public"]));
    }
    Ok(Json(meta))
}

// ===========================================================================
// OAuth Authorization Code Flow
// ===========================================================================

#[derive(Debug, Deserialize)]
struct AuthorizeParams {
    client_id: String,
    redirect_uri: String,
    #[serde(default = "default_response_type")]
    response_type: String,
    state: Option<String>,
    scope: Option<String>,
}

fn default_response_type() -> String {
    "code".into()
}

/// `GET /api/v1/oauth/authorize` — OAuth 2.0 Authorization Endpoint.
///
/// 1. Validate client_id and redirect_uri
/// 2. If user not logged in → redirect to Kyomi login with return URL
/// 3. If user logged in → generate auth code, redirect to client
async fn oauth_authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<AuthorizeParams>,
) -> Result<Response, RouteError> {
    if params.response_type != "code" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Only response_type=code is supported"})),
        )
            .into());
    }

    // Validate client
    let client = lookup_active_client(&state, &params.client_id).await?;

    // Validate redirect_uri
    validate_redirect_uri(&client.redirect_uris, &params.redirect_uri)?;

    // Check if user is logged in (via cookie)
    let cookie_name = &kyomi_core::constants::get().cookies.access_token_name;
    let access_token = kyomi_auth::cookies::get_cookie_value(&headers, cookie_name);

    let mut user_id = None;
    let mut workspace_id = None;

    if let Some(token) = access_token
        && let Ok(decoded) = jwt::validate_token(token, &state.config.jwt_secret) {
            user_id = Some(decoded.claims.sub.clone());
            workspace_id = decoded
                .claims
                .extra
                .get("workspace_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }

    // If not logged in, redirect to login
    let Some(user_id) = user_id else {
        let oauth_state = redis_ops::generate_token();
        let state_data = json!({
            "client_id": params.client_id,
            "redirect_uri": params.redirect_uri,
            "state": params.state,
            "scope": params.scope,
            "created_at": Utc::now().to_rfc3339(),
        });
        redis_ops::store_oauth_state(&state.kv, "oauth_pending", &oauth_state, &state_data)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to store OAuth pending state");
                RouteError::from((StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))
            })?;

        let login_url = format!(
            "{}/login?oauth_continue={}",
            state.config.frontend_url.trim_end_matches('/'),
            oauth_state
        );
        return Ok(Redirect::to(&login_url).into_response());
    };

    // User is logged in — generate auth code
    let auth_code = redis_ops::generate_token();
    let code_data = json!({
        "user_id": user_id,
        "workspace_id": workspace_id,
        "client_id": params.client_id,
        "redirect_uri": params.redirect_uri,
        "scope": params.scope,
        "created_at": Utc::now().to_rfc3339(),
    });
    redis_ops::store_oauth_state(&state.kv, "oauth_code", &auth_code, &code_data)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to store OAuth auth code");
            RouteError::from((StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))
        })?;

    let mut redirect_url = format!("{}?code={}", params.redirect_uri, auth_code);
    if let Some(st) = &params.state {
        redirect_url.push_str(&format!("&state={st}"));
    }

    tracing::info!(
        user_id = %user_id,
        client_id = %params.client_id,
        "OAuth authorize: redirecting to client"
    );

    Ok(Redirect::to(&redirect_url).into_response())
}

/// `GET /api/v1/oauth/authorize/continue` — Post-login OAuth continuation.
#[derive(Debug, Deserialize)]
struct AuthorizeContinueParams {
    state: String,
}

async fn oauth_authorize_continue(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<AuthorizeContinueParams>,
) -> Result<Response, RouteError> {
    // Check authentication FIRST, before consuming the state.
    let cookie_name = &kyomi_core::constants::get().cookies.access_token_name;
    let access_token = kyomi_auth::cookies::get_cookie_value(&headers, cookie_name)
        .ok_or_else(|| RouteError::from((StatusCode::UNAUTHORIZED, "Not logged in")))?;

    let decoded = jwt::validate_token(access_token, &state.config.jwt_secret).map_err(|_| {
        RouteError::from((StatusCode::UNAUTHORIZED, "Invalid session"))
    })?;

    let user_id = decoded.claims.sub.clone();
    let workspace_id = decoded
        .claims
        .extra
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Now that we've verified auth, consume the state
    let oauth_params =
        redis_ops::verify_oauth_state(&state.kv, "oauth_pending", &params.state)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to verify OAuth pending state");
                RouteError::from((StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))
            })?;

    let Some(oauth_params) = oauth_params else {
        // State was likely already consumed — show friendly message
        tracing::info!("OAuth authorize/continue: state not found (likely already consumed)");
        let redirect = format!(
            "{}/oauth-complete",
            state.config.frontend_url.trim_end_matches('/')
        );
        return Ok(Redirect::to(&redirect).into_response());
    };

    // Validate client still exists
    let client_id = oauth_params["client_id"].as_str().unwrap_or("");
    let redirect_uri = oauth_params["redirect_uri"].as_str().unwrap_or("");
    let original_state = oauth_params["state"].as_str();
    let scope = oauth_params["scope"].as_str();

    let client = lookup_active_client(&state, client_id).await?;

    validate_redirect_uri(&client.redirect_uris, redirect_uri)?;

    // Generate auth code
    let auth_code = redis_ops::generate_token();
    let code_data = json!({
        "user_id": user_id,
        "workspace_id": workspace_id,
        "client_id": client_id,
        "redirect_uri": redirect_uri,
        "scope": scope,
        "created_at": Utc::now().to_rfc3339(),
    });
    redis_ops::store_oauth_state(&state.kv, "oauth_code", &auth_code, &code_data)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to store OAuth auth code");
            RouteError::from((StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))
        })?;

    let mut redirect_url = format!("{redirect_uri}?code={auth_code}");
    if let Some(st) = original_state {
        redirect_url.push_str(&format!("&state={st}"));
    }

    tracing::info!(
        user_id = %user_id,
        client_id = %client_id,
        "OAuth continue: redirecting to client"
    );

    Ok(Redirect::to(&redirect_url).into_response())
}

// ===========================================================================
// Token endpoint
// ===========================================================================

#[derive(Debug, Deserialize)]
struct TokenRequest {
    grant_type: String,
    code: Option<String>,
    refresh_token: Option<String>,
    client_id: String,
    redirect_uri: Option<String>,
}

#[derive(Debug, Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
}

/// `POST /api/v1/oauth/token` — OAuth 2.0 Token Endpoint.
async fn oauth_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(params): Form<TokenRequest>,
) -> Result<Json<TokenResponse>, RouteError> {
    tracing::info!(
        grant_type = %params.grant_type,
        client_id = %&params.client_id[..std::cmp::min(20, params.client_id.len())],
        "OAuth token request"
    );

    // Validate client
    let _client = lookup_active_client(&state, &params.client_id).await?;

    match params.grant_type.as_str() {
        "authorization_code" => {
            handle_authorization_code(&state, &headers, &params)
                .await
                .map(Json)
                .map_err(RouteError::from)
        }
        "refresh_token" => {
            handle_refresh_token(&state, &headers, &params)
                .await
                .map(Json)
                .map_err(RouteError::from)
        }
        other => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("Unsupported grant_type: {other}")})),
        )
            .into()),
    }
}

/// Exchange authorization code for tokens.
async fn handle_authorization_code(
    state: &AppState,
    headers: &HeaderMap,
    params: &TokenRequest,
) -> Result<TokenResponse, (StatusCode, Json<serde_json::Value>)> {
    let code = params
        .code
        .as_deref()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, Json(json!({"error": "code required"}))))?;

    // Verify and consume auth code
    let code_data = redis_ops::verify_oauth_state(&state.kv, "oauth_code", code)
        .await
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant: code expired or invalid"})),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant: code expired or invalid"})),
            )
        })?;

    // Verify client_id matches
    if code_data.get("client_id").and_then(|v| v.as_str()) != Some(&params.client_id) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant: client_id mismatch"})),
        ));
    }

    // Verify redirect_uri matches (if provided)
    if let Some(redirect_uri) = &params.redirect_uri
        && code_data.get("redirect_uri").and_then(|v| v.as_str()) != Some(redirect_uri) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant: redirect_uri mismatch"})),
            ));
        }

    let user_id = code_data["user_id"].as_str().unwrap_or("");
    let workspace_id = code_data["workspace_id"].as_str();

    // Verify user still exists and is active
    let user = user_service::get_user_by_id(&state.db, user_id)
        .await
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant: user not found"})),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant: user not found"})),
            )
        })?;

    if !user.active {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant: user not found"})),
        ));
    }

    // Build JWT claims with workspace context
    let jwt_config = &kyomi_core::constants::get().jwt;
    let mut extra = std::collections::HashMap::new();
    extra.insert("user_id".into(), json!(&user.user_id));
    extra.insert("email".into(), json!(&user.email));
    extra.insert("name".into(), json!(&user.name));

    if let Some(ws_id) = workspace_id {
        extra.insert("workspace_id".into(), json!(ws_id));
    }

    let access_token = jwt::create_access_token_str(
        &user.user_id,
        &state.config.jwt_secret,
        jwt_config.access_token_expire_minutes,
        extra,
    )
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create access token");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "internal_error"})),
        )
    })?;

    // Create refresh token with a new family
    let raw_refresh = jwt::create_refresh_token();
    let token_hash = token_service::hash_refresh_token(&raw_refresh);
    let expires_at = Utc::now() + Duration::days(jwt_config.refresh_token_expire_days);
    let device_info = extract_device_info(headers, Some(&params.client_id));
    let family_id = token_service::generate_family_id();

    token_service::store_refresh_token(
        &state.db,
        &user.user_id,
        &token_hash,
        expires_at,
        &device_info,
        &family_id,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to store refresh token");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "internal_error"})),
        )
    })?;

    tracing::info!(
        user_id = %user.user_id,
        client_id = %params.client_id,
        "OAuth token issued"
    );

    Ok(TokenResponse {
        access_token,
        token_type: "Bearer".into(),
        expires_in: jwt_config.access_token_expire_minutes * 60,
        refresh_token: Some(raw_refresh),
        scope: Some("mcp".into()),
    })
}

/// Refresh access token using refresh token.
///
/// MCP/Cursor OAuth flow intentionally does NOT rotate — returns the same refresh token.
async fn handle_refresh_token(
    state: &AppState,
    _headers: &HeaderMap,
    params: &TokenRequest,
) -> Result<TokenResponse, (StatusCode, Json<serde_json::Value>)> {
    let refresh_token = params
        .refresh_token
        .as_deref()
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "refresh_token required"})),
            )
        })?;

    // Verify refresh token via DB (handles rotation state)
    let verify_result = token_service::verify_refresh_token(&state.db, refresh_token)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "OAuth refresh token verification failed");
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant: refresh token invalid or expired"})),
            )
        })?;

    // Accept Valid or GracePeriod (both mean the token is usable).
    // TheftDetected and Invalid are rejected.
    let user_data = match verify_result {
        token_service::RefreshTokenVerifyResult::Valid(data)
        | token_service::RefreshTokenVerifyResult::GracePeriod(data) => data,
        token_service::RefreshTokenVerifyResult::TheftDetected { .. } => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant: refresh token revoked"})),
            ));
        }
        token_service::RefreshTokenVerifyResult::Invalid => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant: refresh token invalid or expired"})),
            ));
        }
    };

    // Verify user still exists and is active
    let user = user_service::get_user_by_id(&state.db, &user_data.user_id)
        .await
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant: user not found"})),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant: user not found"})),
            )
        })?;

    if !user.active {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant: user not found"})),
        ));
    }

    // Get workspace context
    let workspace_id = user_service::get_user_workspace_context(&state.db, &user.user_id)
        .await
        .ok()
        .flatten()
        .map(|(ws, _)| ws.workspace_id);

    let Some(workspace_id) = workspace_id else {
        tracing::warn!(user_id = %user.user_id, "OAuth refresh: no workspace found");
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant: no workspace access"})),
        ));
    };

    // Issue new access token
    let jwt_config = &kyomi_core::constants::get().jwt;
    let mut extra = std::collections::HashMap::new();
    extra.insert("user_id".into(), json!(&user.user_id));
    extra.insert("email".into(), json!(&user.email));
    extra.insert("name".into(), json!(&user.name));
    extra.insert("workspace_id".into(), json!(&workspace_id));

    let access_token = jwt::create_access_token_str(
        &user.user_id,
        &state.config.jwt_secret,
        jwt_config.access_token_expire_minutes,
        extra,
    )
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create access token");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "internal_error"})),
        )
    })?;

    tracing::info!(
        user_id = %user.user_id,
        client_id = %params.client_id,
        "OAuth token refreshed"
    );

    // Return same refresh_token (no rotation) — required by Cursor
    Ok(TokenResponse {
        access_token,
        token_type: "Bearer".into(),
        expires_in: jwt_config.access_token_expire_minutes * 60,
        refresh_token: Some(refresh_token.to_string()),
        scope: Some("mcp".into()),
    })
}

// ===========================================================================
// Dynamic Client Registration (RFC 7591)
// ===========================================================================

#[derive(Debug, Deserialize)]
struct ClientRegistrationRequest {
    redirect_uris: Vec<String>,
    client_name: Option<String>,
    logo_uri: Option<String>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct ClientRegistrationResponse {
    client_id: String,
    client_id_issued_at: i64,
    redirect_uris: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logo_uri: Option<String>,
    grant_types: Vec<String>,
    response_types: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
}

/// `POST /api/v1/oauth/register` — Dynamic Client Registration (RFC 7591).
async fn register_client(
    State(state): State<AppState>,
    Json(registration): Json<ClientRegistrationRequest>,
) -> Result<Json<ClientRegistrationResponse>, RouteError> {
    if registration.redirect_uris.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "redirect_uris is required and must not be empty"})),
        )
            .into());
    }

    // Generate unique client_id
    let client_id = format!("mcp-{}", &redis_ops::generate_token()[..22]);

    let grant_types = registration
        .grant_types
        .unwrap_or_else(|| vec!["authorization_code".into(), "refresh_token".into()]);
    let response_types = registration
        .response_types
        .unwrap_or_else(|| vec!["code".into()]);
    let client_name = registration.client_name.clone().unwrap_or_else(|| "MCP Client".into());

    let redirect_uris_json = json!(registration.redirect_uris);
    let scopes_json = json!(["mcp"]);
    let new_id = uuid::Uuid::new_v4().to_string();

    // Insert into database
    let is_pg = state.db.is_postgres();
    let bool_true = kyomi_core::sql_compat::bool_true(is_pg);
    let insert_sql = format!(
        "INSERT INTO oauth_clients (id, client_id, name, redirect_uris, scopes, client_type, active) \
         VALUES ($1, $2, $3, $4, $5, 'public', {bool_true})"
    );
    kyomi_core::db_execute!(
        &state.db,
        &insert_sql,
        &new_id,
        &client_id,
        &client_name,
        &redirect_uris_json,
        &scopes_json
    )
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to register OAuth client");
        RouteError::from((StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))
    })?;

    tracing::info!(client_id = %client_id, name = %client_name, "Registered new OAuth client");

    // Build logo URI
    let logo_uri = registration.logo_uri.or_else(|| {
        let base = base_url(&state);
        Some(format!("{base}/kyomi_oauth_logo.png"))
    });

    Ok(Json(ClientRegistrationResponse {
        client_id,
        client_id_issued_at: Utc::now().timestamp(),
        redirect_uris: registration.redirect_uris,
        client_name: Some(client_name),
        logo_uri,
        grant_types,
        response_types,
        scope: Some("mcp".into()),
    }))
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Look up an active OAuth client by client_id.
async fn lookup_active_client(
    state: &AppState,
    client_id: &str,
) -> Result<kyomi_core::models::OAuthClient, (StatusCode, Json<serde_json::Value>)> {
    let is_pg = state.db.is_postgres();
    let bool_true = kyomi_core::sql_compat::bool_true(is_pg);
    let select_sql = format!(
        "SELECT id, client_id, client_secret_hash, name, redirect_uris, scopes, \
                client_type, active, created_at \
         FROM oauth_clients \
         WHERE client_id = $1 AND active = {bool_true}"
    );
    kyomi_core::db_fetch_optional!(
        &state.db,
        kyomi_core::models::OAuthClient,
        &select_sql,
        client_id
    )
    .map_err(|e| {
        tracing::error!(error = %e, "OAuth client lookup failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "internal_error"})),
        )
    })?
    .ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("Unknown client_id: {client_id}")})),
        )
    })
}

/// Validate that redirect_uri is in the client's allowed list.
fn validate_redirect_uri(
    allowed: &serde_json::Value,
    redirect_uri: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let is_allowed = allowed
        .as_array()
        .map(|uris| uris.iter().any(|u| u.as_str() == Some(redirect_uri)))
        .unwrap_or(false);

    if !is_allowed {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid redirect_uri"})),
        ));
    }

    Ok(())
}

/// Extract device info from request headers (for refresh token storage).
///
/// Wraps the canonical `kyomi_auth::request_meta::extract_device_info` and
/// attaches the registered OAuth `client_id` this refresh token was issued
/// to — MCP client token storage needs to record which client requested the
/// token, which the generic extractor has no way to know. It now only adds
/// the one field the canonical version doesn't have.
///
/// # Behaviour changed here by KYO-194 — three ways, deliberately
///
/// This used to be a full standalone reimplementation. Consolidating onto
/// the canonical extractor changed what gets recorded for MCP refresh
/// tokens. None of these were incidental:
///
/// 1. **Header precedence flipped.** The old code read `X-Forwarded-For`
///    first and fell back to `X-Real-IP`. The canonical order is the
///    reverse. This is a **security fix**, not a regression: nginx
///    *overwrites* `X-Real-IP` from `$remote_addr`, so a client cannot
///    forge it, whereas it only *appends* to `X-Forwarded-For`, so a client
///    can inject a fake first entry. The old order preferred the spoofable
///    header, meaning an attacker could choose the IP recorded against
///    their own refresh token.
/// 2. **Malformed values are now rejected.** The old code stored whatever
///    string the header held; the canonical version validates with
///    `IpAddr::parse()` and skips anything that isn't a valid IPv4/IPv6
///    address.
/// 3. **`ip_address` is never `None` now.** The old code produced
///    `Option<String>` and left it `None` when neither header was present;
///    the canonical version returns the sentinel `Some("unknown")`. Note
///    this sentinel was *already* reachable for ordinary login/signup
///    sessions before KYO-194 — `helpers::extract_device_info` has always
///    wrapped `extract_client_ip`'s `"unknown"` fallback in `Some` — so
///    this aligns the MCP path with existing behaviour rather than
///    introducing a new shape. See KYO-276 for the display consequence.
fn extract_device_info(headers: &HeaderMap, oauth_client_id: Option<&str>) -> token_service::DeviceInfo {
    let mut device_info = request_meta::extract_device_info(headers);
    device_info.oauth_client_id = oauth_client_id.map(|s| s.to_string());
    device_info
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_metadata_shape() {
        let meta = oauth_metadata("https://dev.kyomi.ai");
        assert_eq!(meta["issuer"], "https://dev.kyomi.ai");
        assert_eq!(
            meta["authorization_endpoint"],
            "https://dev.kyomi.ai/api/v1/oauth/authorize"
        );
        assert_eq!(
            meta["token_endpoint"],
            "https://dev.kyomi.ai/api/v1/oauth/token"
        );
        assert_eq!(
            meta["registration_endpoint"],
            "https://dev.kyomi.ai/api/v1/oauth/register"
        );
    }

    #[test]
    fn validate_redirect_uri_accepts_valid() {
        let allowed = json!(["https://example.com/callback", "cursor://oauth/callback"]);
        assert!(validate_redirect_uri(&allowed, "cursor://oauth/callback").is_ok());
    }

    #[test]
    fn validate_redirect_uri_rejects_invalid() {
        let allowed = json!(["https://example.com/callback"]);
        assert!(validate_redirect_uri(&allowed, "https://evil.com/steal").is_err());
    }

    #[test]
    fn validate_redirect_uri_handles_empty_array() {
        let allowed = json!([]);
        assert!(validate_redirect_uri(&allowed, "anything").is_err());
    }
}
