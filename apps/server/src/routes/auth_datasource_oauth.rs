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
    rate_limiter,
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
    crate::helpers::extract_client_ip(headers, None)
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
    let provider = parse_provider(&provider_str)?;

    // Rate limit
    let ip = extract_client_ip(&headers);
    let rate_result =
        rate_limiter::check_rate_limit(&state.kv, &ip, "login", None).await?;
    if !rate_result.allowed {
        tracing::warn!(ip = %ip, provider = provider.as_str(), "OAuth callback rate limited");
        return Err(kyomi_core::Error::TooManyRequests(
            format!(
                "Rate limited. Try again in {} seconds",
                rate_result.retry_after_secs
            ),
            rate_result.retry_after_secs,
        ));
    }

    // Validate state
    let csrf_state = data.state.as_deref().ok_or_else(|| {
        kyomi_core::Error::BadRequest("Missing state parameter".into())
    })?;

    let state_data = redis_ops::verify_oauth_state(
        &state.kv,
        &format!("datasource_{}", provider.as_str()),
        csrf_state,
    )
    .await?
    .ok_or_else(|| {
        kyomi_core::Error::BadRequest(format!(
            "Invalid or expired state parameter for {} account linking",
            provider.as_str()
        ))
    })?;

    // Verify this is a linking action
    let action = state_data["action"].as_str().unwrap_or("");
    if action != "link_account" {
        return Err(kyomi_core::Error::BadRequest(
            "Invalid linking state".into(),
        ));
    }

    let user_id = state_data["user_id"]
        .as_str()
        .ok_or_else(|| kyomi_core::Error::BadRequest("Missing user_id in state".into()))?;

    let workspace_id = state_data["workspace_id"]
        .as_str()
        .ok_or_else(|| kyomi_core::Error::BadRequest("Missing workspace_id in state".into()))?;

    let datasource_slug = state_data["datasource_slug"]
        .as_str()
        .ok_or_else(|| kyomi_core::Error::BadRequest("Missing datasource_slug in state".into()))?;

    // Load datasource connection_config
    let ds = load_datasource(&state.db, datasource_slug, workspace_id).await?;

    // Extract provider config
    let provider_config = ProviderConfig::from_connection_config(provider, &ds.connection_config)?;

    // Build redirect URI (must match what was used in /connect)
    let redirect_uri = format!(
        "{}/auth/oauth/{}/callback",
        state.config.frontend_url.trim_end_matches('/'),
        provider.as_str()
    );

    // Exchange code for tokens
    let code_verifier = state_data["code_verifier"].as_str();
    let token_data = datasource_oauth::exchange_code_for_tokens(
        &provider_config,
        &data.code,
        &redirect_uri,
        code_verifier,
    )
    .await?;

    // Get user info from provider
    let user_info = datasource_oauth::get_user_info(
        provider,
        &token_data.access_token,
        &provider_config.account_or_host,
    )
    .await?;

    // Calculate token expiry
    let expires_in = token_data.expires_in.unwrap_or(3600);
    let expires_at =
        (chrono::Utc::now() + chrono::Duration::seconds(expires_in)).to_rfc3339();

    // Build OAuth credential JSON
    let oauth_credentials = serde_json::json!({
        "auth_type": "oauth",
        "oauth_access_token": token_data.access_token,
        "oauth_refresh_token": token_data.refresh_token,
        "oauth_token_expiry": expires_at,
        "oauth_scope": token_data.scope,
        "oauth_username": user_info.username.as_deref().or(user_info.email.as_deref()),
        "oauth_email": user_info.email,
    });

    // Encrypt credentials
    let encrypted = encryption::encrypt_json(
        &oauth_credentials,
        &state.encryption_key,
    )?;

    // Upsert user_datasource_credentials
    let now = chrono::Utc::now();
    let now_str = now.to_rfc3339();
    kyomi_core::db_execute!(
        &state.db,
        "INSERT INTO user_datasource_credentials (user_id, datasource_config_id, workspace_id, credentials, enabled, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, true, $5, $5) \
         ON CONFLICT (user_id, datasource_config_id) \
         DO UPDATE SET credentials = $4, updated_at = $5",
        user_id,
        &ds.id,
        workspace_id,
        &encrypted,
        &now_str
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("DB error saving credentials: {e}")))?;

    tracing::info!(
        provider = provider.as_str(),
        datasource_slug = datasource_slug,
        user_id = user_id,
        "Saved OAuth credentials"
    );

    // Send WebSocket credential_status_changed notification
    let ws_manager = state.ws_manager.clone();
    let ws_user_id = user_id.to_string();
    let ws_workspace_id = workspace_id.to_string();
    let ws_ds_slug = datasource_slug.to_string();
    let ws_ds_type = ds.datasource_type.to_string();
    tokio::spawn(async move {
        ws_helpers::send_credential_status_changed(
            &ws_manager,
            &ws_user_id,
            &ws_workspace_id,
            &ws_ds_slug,
            "connected",
            &ws_ds_type,
        )
        .await;
    });

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("{} account linked successfully", provider.as_str()),
        "provider": provider.as_str(),
        "provider_email": user_info.email,
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
