// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-datasource OAuth endpoints.
//!
//! Wire-compatible with Python's `routers/auth_oauth.py`.
//!
//! Provides parameterized OAuth endpoints for Snowflake, Databricks,
//! BigQuery Enterprise, and Microsoft Enterprise datasource connections.
//!
//! Endpoints (all under `/api/v1/auth/oauth`):
//!   - GET  /providers                  — list registered providers
//!   - GET  /{provider}/connect         — start OAuth flow (302 redirect)
//!   - POST /{provider}/callback        — exchange code for tokens
//!   - POST /{provider}/disconnect      — remove OAuth tokens
//!   - GET  /{provider}/status          — check connection status

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use kyomi_auth::{
    datasource_oauth::{self, OAuthProvider, ProviderConfig},
    encryption,
    middleware::AuthUser,
    redis_ops,
    websocket::helpers as ws_helpers,
};
use kyomi_core::models::datasource::{DatasourceConfig, UserDatasourceCredential};

use crate::state::AppState;

/// Build the per-datasource OAuth router.
///
/// Mounted at `/api/v1/auth/oauth`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/providers", get(list_providers))
        .route("/{provider}/connect", get(start_connect))
        .route("/{provider}/callback", post(handle_callback))
        .route("/{provider}/disconnect", post(disconnect))
        .route("/{provider}/status", get(status))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse and validate the provider path parameter.
fn parse_provider(provider: &str) -> Result<OAuthProvider, kyomi_core::Error> {
    OAuthProvider::parse(provider).ok_or_else(|| {
        let available: Vec<&str> = OAuthProvider::all().iter().map(|p| p.as_str()).collect();
        kyomi_core::Error::NotFound(format!(
            "Unknown OAuth provider: {provider}. Available providers: {available:?}"
        ))
    })
}

fn extract_client_ip(headers: &HeaderMap) -> String {
    kyomi_auth::request_meta::extract_client_ip(headers, None)
}

/// Load a datasource config by slug and workspace.
async fn load_datasource(
    db: &kyomi_core::DbPool,
    slug: &str,
    workspace_id: &str,
) -> kyomi_core::Result<DatasourceConfig> {
    kyomi_core::db_fetch_optional!(
        db,
        DatasourceConfig,
        "SELECT id, workspace_id, name, slug, \
         datasource_type, connection_config, \
         active, connection_type, connect_token_jti, \
         created_at, updated_at, \
         last_catalog_refresh, last_index_started_at, auto_refresh_allowed \
         FROM datasource_configs \
         WHERE slug = $1 AND workspace_id = $2 AND active = true",
        slug,
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("DB error: {e}")))?
    .ok_or_else(|| kyomi_core::Error::NotFound(format!("Datasource not found: {slug}")))
}

/// Load just the datasource config ID by slug and workspace.
async fn load_datasource_id(
    db: &kyomi_core::DbPool,
    slug: &str,
    workspace_id: &str,
) -> kyomi_core::Result<String> {
    #[derive(sqlx::FromRow)]
    struct IdRow { id: String }

    kyomi_core::db_fetch_optional!(
        db, IdRow,
        "SELECT id FROM datasource_configs WHERE slug = $1 AND workspace_id = $2 AND active = true",
        slug,
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("DB error: {e}")))?
    .map(|r| r.id)
    .ok_or_else(|| kyomi_core::Error::NotFound(format!("Datasource not found: {slug}")))
}

// ---------------------------------------------------------------------------
// Request/Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ConnectQuery {
    datasource_slug: Option<String>,
}

#[derive(Deserialize)]
struct CallbackRequest {
    code: String,
    state: Option<String>,
}

#[derive(Deserialize)]
struct DisconnectQuery {
    datasource_slug: Option<String>,
}

#[derive(Deserialize)]
struct StatusQuery {
    datasource_slug: Option<String>,
}

// ---------------------------------------------------------------------------
// GET /providers
// ---------------------------------------------------------------------------

async fn list_providers() -> Json<serde_json::Value> {
    let providers: Vec<&str> = OAuthProvider::all().iter().map(|p| p.as_str()).collect();
    Json(serde_json::json!({
        "providers": providers,
        "count": providers.len(),
    }))
}

// ---------------------------------------------------------------------------
// GET /{provider}/connect (requires auth)
// ---------------------------------------------------------------------------

async fn start_connect(
    State(state): State<AppState>,
    user: AuthUser,
    Path(provider_str): Path<String>,
    Query(query): Query<ConnectQuery>,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    let provider = parse_provider(&provider_str)?;

    let datasource_slug = query.datasource_slug.as_deref().ok_or_else(|| {
        kyomi_core::Error::BadRequest(format!(
            "{} OAuth requires a datasource_slug parameter",
            provider.as_str()
        ))
    })?;

    let workspace_id = user.workspace.workspace_id.as_deref().ok_or_else(|| {
        kyomi_core::Error::BadRequest("No workspace context".into())
    })?;

    // Load datasource connection_config from DB
    let ds = load_datasource(&state.db, datasource_slug, workspace_id).await?;

    // Validate BigQuery enterprise auth_mode
    if provider == OAuthProvider::BigqueryEnterprise {
        let auth_mode = ds
            .connection_config
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("kyomi_oauth");
        if auth_mode != "enterprise_oauth" {
            return Err(kyomi_core::Error::BadRequest(format!(
                "Datasource is configured for {auth_mode}, not enterprise_oauth"
            )));
        }
    }

    // Extract provider config from connection_config
    let provider_config = ProviderConfig::from_connection_config(provider, &ds.connection_config)?;

    // Generate CSRF state
    let csrf_state = redis_ops::generate_token();

    // Build redirect URI
    let redirect_uri = format!(
        "{}/auth/oauth/{}/callback",
        state.config.frontend_url.trim_end_matches('/'),
        provider.as_str()
    );

    // Build authorization URL (may include PKCE)
    let auth_result =
        datasource_oauth::build_authorization_url(&provider_config, &redirect_uri, &csrf_state);

    // Store state in Redis with all context needed for the callback
    let mut state_data = serde_json::json!({
        "user_id": user.user_id,
        "workspace_id": workspace_id,
        "action": "link_account",
        "provider": provider.as_str(),
        "datasource_slug": datasource_slug,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(ref verifier) = auth_result.code_verifier {
        state_data["code_verifier"] = serde_json::json!(verifier);
    }

    redis_ops::store_oauth_state(
        &state.kv,
        &format!("datasource_{}", provider.as_str()),
        &csrf_state,
        &state_data,
    )
    .await?;

    tracing::info!(
        provider = provider.as_str(),
        datasource_slug = datasource_slug,
        user_email = %user.email,
        "Starting per-datasource OAuth connect"
    );

    // 302 Found — matches Python's RedirectResponse(status_code=302)
    Ok((axum::http::StatusCode::FOUND, [(axum::http::header::LOCATION, auth_result.url)]))
}

// ---------------------------------------------------------------------------
// POST /{provider}/callback (no auth — user returns from external OAuth)
// ---------------------------------------------------------------------------

async fn handle_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(provider_str): Path<String>,
    Json(data): Json<CallbackRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    use kyomi_auth::auth_service::{
        datasource_oauth_callback_service, DatasourceOAuthCallbackParams,
    };

    // Validate the provider string before delegating
    parse_provider(&provider_str)?;

    let ip = extract_client_ip(&headers);

    let result = datasource_oauth_callback_service(DatasourceOAuthCallbackParams {
        db: &state.db,
        kv: &state.kv,
        encryption_key: &state.encryption_key,
        code: &data.code,
        state: data.state.as_deref(),
        provider: &provider_str,
        frontend_url: &state.config.frontend_url,
        ip: &ip,
    })
    .await?;

    // Send WebSocket credential_status_changed notification
    let ws_manager = state.ws_manager.clone();
    tokio::spawn(async move {
        ws_helpers::send_credential_status_changed(
            &ws_manager,
            &result.user_id,
            &result.workspace_id,
            &result.datasource_slug,
            "connected",
            &result.datasource_type,
        )
        .await;
    });

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("{} account linked successfully", result.provider),
        "provider": result.provider,
        "provider_email": result.provider_email,
        "linked_at": chrono::Utc::now().to_rfc3339(),
    })))
}

// ---------------------------------------------------------------------------
// POST /{provider}/disconnect (requires auth)
// ---------------------------------------------------------------------------

async fn disconnect(
    State(state): State<AppState>,
    user: AuthUser,
    Path(provider_str): Path<String>,
    Query(query): Query<DisconnectQuery>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let provider = parse_provider(&provider_str)?;

    let datasource_slug = query.datasource_slug.as_deref().ok_or_else(|| {
        kyomi_core::Error::BadRequest(format!(
            "{} OAuth disconnect requires a datasource_slug parameter",
            provider.as_str()
        ))
    })?;

    let workspace_id = user.workspace.workspace_id.as_deref().ok_or_else(|| {
        kyomi_core::Error::BadRequest("No workspace context".into())
    })?;

    // Find the datasource
    let ds_id = load_datasource_id(&state.db, datasource_slug, workspace_id).await?;

    // Delete the user's credentials for this datasource
    let result = kyomi_core::db_execute!(
        &state.db,
        "DELETE FROM user_datasource_credentials WHERE user_id = $1 AND datasource_config_id = $2",
        &user.user_id,
        &ds_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("DB error: {e}")))?;

    if result.rows_affected() == 0 {
        return Ok(Json(serde_json::json!({
            "message": format!("No {} account connected", provider.as_str()),
            "already_disconnected": true,
        })));
    }

    tracing::info!(
        provider = provider.as_str(),
        datasource_slug = datasource_slug,
        user_id = %user.user_id,
        "Disconnected OAuth credentials"
    );

    Ok(Json(serde_json::json!({
        "message": format!("{} account disconnected successfully", provider.as_str()),
        "provider": provider.as_str(),
        "disconnected_at": chrono::Utc::now().to_rfc3339(),
    })))
}

// ---------------------------------------------------------------------------
// GET /{provider}/status (requires auth)
// ---------------------------------------------------------------------------

async fn status(
    State(state): State<AppState>,
    user: AuthUser,
    Path(provider_str): Path<String>,
    Query(query): Query<StatusQuery>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let provider = parse_provider(&provider_str)?;

    let datasource_slug = query.datasource_slug.as_deref().ok_or_else(|| {
        kyomi_core::Error::BadRequest(format!(
            "{} OAuth status requires a datasource_slug parameter",
            provider.as_str()
        ))
    })?;

    let workspace_id = user.workspace.workspace_id.as_deref().ok_or_else(|| {
        kyomi_core::Error::BadRequest("No workspace context".into())
    })?;

    // Find the datasource
    let ds_id = load_datasource_id(&state.db, datasource_slug, workspace_id).await?;

    // Look up user credentials
    let cred = kyomi_core::db_fetch_optional!(
        &state.db,
        UserDatasourceCredential,
        "SELECT id, user_id, datasource_config_id, workspace_id, credentials, \
         enabled, created_at, updated_at \
         FROM user_datasource_credentials \
         WHERE user_id = $1 AND datasource_config_id = $2",
        &user.user_id,
        &ds_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("DB error: {e}")))?;

    let Some(cred) = cred else {
        return Ok(Json(serde_json::json!({
            "connected": false,
            "provider": provider.as_str(),
            "provider_email": serde_json::Value::Null,
            "needs_connect": true,
            "connect_url": format!("/api/v1/auth/oauth/{}/connect", provider.as_str()),
        })));
    };

    // Decrypt credentials to check token status
    let credentials = encryption::decrypt_json(
        &cred.credentials,
        &state.encryption_key,
    )?;

    let access_token = credentials
        .get("oauth_access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    let refresh_token = credentials
        .get("oauth_refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    let has_refresh = refresh_token.is_some();

    let expires_at = credentials
        .get("oauth_token_expiry")
        .and_then(|v| v.as_str());

    let token_expired = expires_at
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|exp| exp.with_timezone(&chrono::Utc) < chrono::Utc::now())
        .unwrap_or(false);

    // Only consider expired if no refresh token to auto-refresh
    let needs_reconnect = token_expired && !has_refresh;

    let provider_email = credentials
        .get("oauth_email")
        .and_then(|v| v.as_str());

    let provider_username = credentials
        .get("oauth_username")
        .and_then(|v| v.as_str());

    Ok(Json(serde_json::json!({
        "connected": access_token.is_some(),
        "provider": provider.as_str(),
        "provider_email": provider_email,
        "provider_username": provider_username,
        "token_expired": token_expired,
        "has_refresh_token": has_refresh,
        "needs_reconnect": needs_reconnect,
        "connect_url": format!("/api/v1/auth/oauth/{}/connect", provider.as_str()),
        "disconnect_url": format!("/api/v1/auth/oauth/{}/disconnect", provider.as_str()),
    })))
}
