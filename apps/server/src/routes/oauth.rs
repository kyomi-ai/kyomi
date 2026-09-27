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
    Form, Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header::SET_COOKIE},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use kyomi_auth::{jwt, redis_ops, request_meta, token_service, user_service};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use url::Url;

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
        .route(
            "/authorize",
            get(oauth_authorize).post(oauth_consent_decision),
        )
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
        .unwrap_or_else(|| {
            state
                .config
                .base_url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
        });
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
        "code_challenge_methods_supported": ["S256"],
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
    Ok(Json(oauth_metadata(&base_url_from_request(
        &headers, &state,
    ))))
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
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
}

fn default_response_type() -> String {
    "code".into()
}

/// A consent transaction is bound to an independent browser cookie and expires
/// with the shared OAuth state TTL (five minutes). Only the decision POST mints a code.
async fn oauth_authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<AuthorizeParams>,
) -> Result<Response, RouteError> {
    validate_authorize_params(&params)?;
    let client = lookup_active_client(&state, &params.client_id).await?;
    validate_redirect_uri(&client.redirect_uris, &params.redirect_uri)?;

    let cookie_name = &kyomi_core::constants::get().cookies.access_token_name;
    let token = kyomi_auth::cookies::get_cookie_value(&headers, cookie_name);
    if let Some(token) = token
        && let Ok(session) = jwt::validate_token(token, &state.config.jwt_secret)
    {
        return render_consent(&state, &client, &params, &session.claims).await;
    }

    let oauth_state = redis_ops::generate_token();
    let pending = json!({
        "client_id": params.client_id,
        "redirect_uri": params.redirect_uri,
        "state": params.state,
        "scope": params.scope,
        "code_challenge": params.code_challenge,
        "code_challenge_method": params.code_challenge_method,
    });
    redis_ops::store_oauth_state(&state.kv, "oauth_pending", &oauth_state, &pending)
        .await
        .map_err(internal_oauth_error)?;
    let mut login_url = Url::parse(&format!(
        "{}/login",
        state.config.frontend_url.trim_end_matches('/')
    ))
    .map_err(|_| RouteError::from((StatusCode::INTERNAL_SERVER_ERROR, "Invalid login URL")))?;
    login_url
        .query_pairs_mut()
        .append_pair("oauth_continue", &oauth_state);
    Ok(Redirect::to(login_url.as_str()).into_response())
}

fn validate_authorize_params(params: &AuthorizeParams) -> Result<(), RouteError> {
    if params.response_type != "code" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Only response_type=code is supported"})),
        )
            .into());
    }
    if params.scope.as_deref().is_some_and(|s| s != "mcp") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Unsupported scope"})),
        )
            .into());
    }
    let valid_challenge = params.code_challenge.as_deref().is_some_and(|challenge| {
        (43..=128).contains(&challenge.len())
            && challenge
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    });
    if params.code_challenge_method.as_deref() != Some("S256") || !valid_challenge {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "PKCE S256 code_challenge required"})),
        )
            .into());
    }
    Ok(())
}

async fn render_consent(
    state: &AppState,
    client: &kyomi_core::models::OAuthClient,
    params: &AuthorizeParams,
    claims: &jwt::Claims,
) -> Result<Response, RouteError> {
    let transaction = redis_ops::generate_token();
    let csrf = redis_ops::generate_token();
    let browser_nonce = redis_ops::generate_token();
    let workspace_id = claims.extra.get("workspace_id").and_then(|v| v.as_str());
    let data = json!({
        "client_id": params.client_id,
        "redirect_uri": params.redirect_uri,
        "state": params.state,
        "scope": params.scope,
        "code_challenge": params.code_challenge,
        "user_id": claims.sub,
        "workspace_id": workspace_id,
        "browser_nonce_hash": token_service::hash_refresh_token(&browser_nonce),
        "csrf": csrf,
    });
    redis_ops::store_oauth_state(&state.kv, "oauth_consent", &transaction, &data)
        .await
        .map_err(internal_oauth_error)?;
    let callback = Url::parse(&params.redirect_uri)
        .map_err(|_| RouteError::from((StatusCode::BAD_REQUEST, "Invalid redirect_uri")))?;
    let callback_origin = match callback.host_str() {
        Some(host) => format!(
            "{}://{}{}",
            callback.scheme(),
            host,
            callback.port().map(|p| format!(":{p}")).unwrap_or_default()
        ),
        None => format!("{}:", callback.scheme()),
    };
    let account = claims
        .extra
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or(&claims.sub);
    let owner = Owner::new();
    let body = owner.with(|| {
        view! {
            <kyomi_ui::pages::auth::oauth_consent::OAuthConsentPage
                client_name=client.name.clone()
                account=account.to_owned()
                workspace=workspace_id.unwrap_or("No workspace selected").to_owned()
                callback_origin=callback_origin
                transaction=transaction.clone()
                csrf=csrf
            />
        }
        .to_html()
    });
    let page = crate::leptos_frontend::consent_document(&body);
    let mut response = Html(page).into_response();
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    let secure = if state.config.base_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "oauth_consent_{transaction}={browser_nonce}; Max-Age=300; Path=/api/v1/oauth/authorize; SameSite=Lax; HttpOnly{secure}"
    );
    response.headers_mut().append(
        SET_COOKIE,
        HeaderValue::from_str(&cookie)
            .map_err(|_| RouteError::from((StatusCode::INTERNAL_SERVER_ERROR, "Internal error")))?,
    );
    Ok(response)
}

#[derive(Debug, Deserialize)]
struct AuthorizeContinueParams {
    state: String,
}

/// Login continuation presents Kyomi consent; it cannot issue a code.
async fn oauth_authorize_continue(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<AuthorizeContinueParams>,
) -> Result<Response, RouteError> {
    let cookie_name = &kyomi_core::constants::get().cookies.access_token_name;
    let token = kyomi_auth::cookies::get_cookie_value(&headers, cookie_name)
        .ok_or_else(|| RouteError::from((StatusCode::UNAUTHORIZED, "Not logged in")))?;
    let session = jwt::validate_token(token, &state.config.jwt_secret)
        .map_err(|_| RouteError::from((StatusCode::UNAUTHORIZED, "Invalid session")))?;
    let pending = redis_ops::verify_oauth_state(&state.kv, "oauth_pending", &params.state)
        .await
        .map_err(internal_oauth_error)?
        .ok_or_else(|| RouteError::from((StatusCode::BAD_REQUEST, "Authorization expired")))?;
    let params = AuthorizeParams {
        client_id: pending["client_id"].as_str().unwrap_or_default().to_owned(),
        redirect_uri: pending["redirect_uri"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        response_type: default_response_type(),
        state: pending["state"].as_str().map(str::to_owned),
        scope: pending["scope"].as_str().map(str::to_owned),
        code_challenge: pending["code_challenge"].as_str().map(str::to_owned),
        code_challenge_method: pending["code_challenge_method"].as_str().map(str::to_owned),
    };
    validate_authorize_params(&params)?;
    let client = lookup_active_client(&state, &params.client_id).await?;
    validate_redirect_uri(&client.redirect_uris, &params.redirect_uri)?;
    render_consent(&state, &client, &params, &session.claims).await
}

#[derive(Debug, Deserialize)]
struct ConsentDecision {
    transaction: String,
    csrf: String,
    decision: String,
}

async fn oauth_consent_decision(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ConsentDecision>,
) -> Result<Response, RouteError> {
    let cookie_name = &kyomi_core::constants::get().cookies.access_token_name;
    let token = kyomi_auth::cookies::get_cookie_value(&headers, cookie_name)
        .ok_or_else(|| RouteError::from((StatusCode::UNAUTHORIZED, "Not logged in")))?;
    let session = jwt::validate_token(token, &state.config.jwt_secret)
        .map_err(|_| RouteError::from((StatusCode::UNAUTHORIZED, "Invalid session")))?;
    if form.decision != "allow" && form.decision != "deny" {
        return Err((StatusCode::BAD_REQUEST, "Invalid decision").into());
    }
    // Atomic GETDEL prevents replay or simultaneous double approval. Invalid
    // attempts also consume the transaction, requiring a fresh consent page.
    let consent = redis_ops::verify_oauth_state(&state.kv, "oauth_consent", &form.transaction)
        .await
        .map_err(internal_oauth_error)?
        .ok_or_else(|| RouteError::from((StatusCode::BAD_REQUEST, "Consent expired or used")))?;
    let browser_nonce = if form.transaction.len() == 43
        && form
            .transaction
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        kyomi_auth::cookies::get_cookie_value(
            &headers,
            &format!("oauth_consent_{}", form.transaction),
        )
    } else {
        None
    };
    if consent["csrf"].as_str() != Some(&form.csrf)
        || consent["user_id"].as_str() != Some(&session.claims.sub)
        || browser_nonce.is_none_or(|nonce| {
            consent["browser_nonce_hash"].as_str()
                != Some(token_service::hash_refresh_token(nonce).as_str())
        })
    {
        return Err((StatusCode::FORBIDDEN, "Consent session mismatch").into());
    }
    let client_id = consent["client_id"].as_str().unwrap_or_default();
    let redirect_uri = consent["redirect_uri"].as_str().unwrap_or_default();
    let client = lookup_active_client(&state, client_id).await?;
    validate_redirect_uri(&client.redirect_uris, redirect_uri)?;
    let mut callback = Url::parse(redirect_uri)
        .map_err(|_| RouteError::from((StatusCode::BAD_REQUEST, "Invalid redirect_uri")))?;
    if form.decision == "deny" {
        callback
            .query_pairs_mut()
            .append_pair("error", "access_denied");
    } else {
        let auth_code = redis_ops::generate_token();
        let code_data = json!({
            "user_id": session.claims.sub,
            "workspace_id": consent["workspace_id"],
            "client_id": client_id,
            "redirect_uri": redirect_uri,
            "scope": consent["scope"],
            "code_challenge": consent["code_challenge"],
        });
        redis_ops::store_oauth_state(&state.kv, "oauth_code", &auth_code, &code_data)
            .await
            .map_err(internal_oauth_error)?;
        callback.query_pairs_mut().append_pair("code", &auth_code);
    }
    if let Some(original_state) = consent["state"].as_str() {
        callback
            .query_pairs_mut()
            .append_pair("state", original_state);
    }
    Ok(Redirect::to(callback.as_str()).into_response())
}

fn internal_oauth_error(error: kyomi_core::Error) -> RouteError {
    tracing::error!(%error, "OAuth state operation failed");
    (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into()
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
    code_verifier: Option<String>,
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
        "authorization_code" => handle_authorization_code(&state, &headers, &params)
            .await
            .map(Json)
            .map_err(RouteError::from),
        "refresh_token" => handle_refresh_token(&state, &headers, &params)
            .await
            .map(Json)
            .map_err(RouteError::from),
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
    let code = params.code.as_deref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "code required"})),
        )
    })?;
    let redirect_uri = params.redirect_uri.as_deref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "redirect_uri required"})),
        )
    })?;
    let verifier = params.code_verifier.as_deref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "code_verifier required"})),
        )
    })?;
    if !(43..=128).contains(&verifier.len())
        || !verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant: invalid code_verifier"})),
        ));
    }

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

    if code_data.get("redirect_uri").and_then(|v| v.as_str()) != Some(redirect_uri) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant: redirect_uri mismatch"})),
        ));
    }
    let digest = pkce_s256(verifier);
    if code_data.get("code_challenge").and_then(|v| v.as_str()) != Some(digest.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant: PKCE verification failed"})),
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

fn pkce_s256(verifier: &str) -> String {
    let hex_digest = token_service::hash_refresh_token(verifier);
    let digest_bytes: Vec<u8> = hex_digest
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            u8::from_str_radix(
                std::str::from_utf8(pair).expect("SHA-256 digest is ASCII"),
                16,
            )
            .expect("SHA-256 digest is hexadecimal")
        })
        .collect();
    URL_SAFE_NO_PAD.encode(digest_bytes)
}

/// Refresh access token using refresh token.
///
/// MCP/Cursor OAuth flow intentionally does NOT rotate — returns the same refresh token.
async fn handle_refresh_token(
    state: &AppState,
    _headers: &HeaderMap,
    params: &TokenRequest,
) -> Result<TokenResponse, (StatusCode, Json<serde_json::Value>)> {
    let refresh_token = params.refresh_token.as_deref().ok_or_else(|| {
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
    if registration.redirect_uris.len() > 10
        || registration
            .redirect_uris
            .iter()
            .any(|uri| !valid_registration_redirect(uri))
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid redirect_uri"})),
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
    let client_name = registration
        .client_name
        .clone()
        .unwrap_or_else(|| "MCP Client".into());

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
    let is_allowed = valid_registration_redirect(redirect_uri)
        && allowed
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

/// Redirects must be absolute, have no fragment or credentials, and use a
/// secure web origin, a local HTTP loopback, or a native app callback scheme.
fn valid_registration_redirect(uri: &str) -> bool {
    if uri.len() > 2048
        || uri.chars().any(char::is_control)
        || uri.contains(' ')
        || !uri.contains("://")
    {
        return false;
    }
    let Ok(url) = Url::parse(uri) else {
        return false;
    };
    if url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return false;
    }
    match url.scheme() {
        "https" => true,
        "http" => url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(&['[', ']'][..])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }),
        "file" | "ftp" | "javascript" | "data" | "about" => false,
        _ => url.port().is_none(),
    }
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
fn extract_device_info(
    headers: &HeaderMap,
    oauth_client_id: Option<&str>,
) -> token_service::DeviceInfo {
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
    fn pkce_s256_matches_rfc_7636_vector() {
        assert_eq!(
            pkce_s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn redirect_syntax_accepts_native_and_loopback_callbacks() {
        assert!(valid_registration_redirect("cursor://oauth/callback"));
        assert!(valid_registration_redirect(
            "http://127.0.0.1:8000/callback"
        ));
        assert!(valid_registration_redirect("http://[::1]:8000/callback"));
        assert!(!valid_registration_redirect(
            "https://example.com/callback#fragment"
        ));
        assert!(!valid_registration_redirect("http://example.com/callback"));
    }

    #[test]
    fn client_name_is_escaped_in_consent_markup() {
        let owner = Owner::new();
        let html = owner.with(|| {
            view! {
                <kyomi_ui::pages::auth::oauth_consent::OAuthConsentPage
                    client_name="<script>alert(1)</script>".to_owned()
                    account="person@example.com".to_owned()
                    workspace="workspace".to_owned()
                    callback_origin="https://example.com".to_owned()
                    transaction="transaction".to_owned()
                    csrf="csrf".to_owned()
                />
            }
            .to_html()
        });
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
    }

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
