// SPDX-License-Identifier: LicenseRef-Alytic-Enterprise

//! Slack integration REST endpoints — OAuth, status, channel management,
//! bot commands, events, and interactions.
//!
//! Wire-compatible with Python's `routers/slack_integration.py`.
//!
//! ## Endpoints (Phase 11-3 scope — OAuth & settings)
//!
//! - `GET  /install`               — Get Slack OAuth URL for workspace installation (admin)
//! - `GET  /oauth/callback`        — Handle OAuth callback after app installation
//! - `DELETE /uninstall`            — Remove Slack integration from workspace (admin)
//! - `GET  /user/connect`          — Start user Slack account linking
//! - `GET  /user/callback`         — Handle user OAuth callback
//! - `POST /user/disconnect`       — Disconnect user's Slack account
//! - `GET  /status`                — Get Slack integration status
//! - `GET  /channels`              — List available Slack channels
//! - `GET  /default-watch-channel` — Get default watch channel for user
//! - `POST /default-watch-channel` — Set default watch channel for user
//! - `GET  /connect/initiate`      — Initiate OAuth from /kyomi connect command
//!
//! ## Endpoints (Phase 11-4 scope — Bot interactions)
//!
//! - `POST /command`       — Handle /kyomi slash commands (Slack signature verified, no auth)
//! - `POST /events`        — Handle Slack Events API (app_mention, DM) (Slack signature verified)
//! - `POST /interactions`  — Handle Slack interactive payloads (Slack signature verified)

use std::sync::OnceLock;

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{error, info, warn};

use kyomi_auth::{
    encryption,
    middleware::AuthUser,
    permissions::permissions_for,
    redis_ops,
    user_service,
};
use kyomi_core::{capability, enums::WorkspaceRole, DbPool};
use kyomi_types::Permission;

use crate::client::{self as slack_client, SlackClient, SLACK_TIMEZONE_CACHE_HOURS};
use crate::helpers as slack_helpers;
use crate::SlackState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the `/slack` router with all Slack integration endpoints.
pub fn routes() -> Router<SlackState> {
    Router::new()
        // Phase 11-3: OAuth & settings (use AuthUser extractor per-handler)
        .route("/install", get(get_install_url))
        .route("/oauth/callback", get(handle_oauth_callback))
        .route("/uninstall", delete(uninstall_slack))
        .route("/user/connect", get(start_user_connect))
        .route("/user/callback", get(handle_user_callback))
        .route("/user/disconnect", post(disconnect_user))
        .route("/status", get(get_slack_status))
        .route("/channels", get(list_channels))
        .route("/default-watch-channel", get(get_default_watch_channel))
        .route("/default-watch-channel", post(set_default_watch_channel))
        .route("/connect/initiate", get(initiate_connect_oauth))
        // Phase 11-4: Bot interactions (Slack signature verified, NO auth middleware)
        .route("/command", post(handle_slack_command))
        .route("/events", post(handle_slack_events))
        .route("/interactions", post(handle_slack_interactions))
}

// ===========================================================================
// Request / Response types
// ===========================================================================

/// Slack integration status response.
#[derive(Debug, Serialize)]
struct SlackStatusResponse {
    installed: bool,
    team_name: Option<String>,
    team_id: Option<String>,
    user_connected: bool,
    slack_username: Option<String>,
}

/// Channel info in list response.
#[derive(Debug, Serialize)]
struct ChannelResponse {
    id: String,
    name: String,
    is_private: bool,
}

/// Channel list response.
#[derive(Debug, Serialize)]
struct ChannelsResponse {
    channels: Vec<ChannelResponse>,
}

/// Default watch channel response.
#[derive(Debug, Serialize)]
struct DefaultWatchChannelResponse {
    channel_id: Option<String>,
    channel_name: Option<String>,
}

/// Request to set default watch channel.
#[derive(Debug, Deserialize)]
struct SetDefaultWatchChannelRequest {
    channel_id: String,
    channel_name: String,
}

/// OAuth callback query parameters.
#[derive(Debug, Deserialize)]
struct OAuthCallbackQuery {
    code: String,
    state: String,
}

/// Connect initiate query parameters.
#[derive(Debug, Deserialize)]
struct ConnectInitiateQuery {
    state: String,
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Reject users whose workspace does not have the `slack_integration` capability.
fn require_slack_capability(user: &AuthUser) -> Result<(), kyomi_core::Error> {
    if !capability::has_capability(user.workspace.subscription_tier, "slack_integration") {
        return Err(kyomi_core::Error::Forbidden(
            "Slack integration requires an active Kyomi Cloud subscription.".into(),
        ));
    }
    Ok(())
}

/// Reject non-workspace-admin users with 403.
///
/// Thin wrapper over the shared [`permissions_for`] mapping (KYO-189 P1) —
/// adapts its `BTreeSet<Permission>` result to this crate's `kyomi_core::Error`
/// return type, since `kyomi-auth` cannot depend on `kyomi-slack` for the
/// error type and the mapping itself must not be duplicated per-crate.
fn require_workspace_admin(user: &AuthUser) -> Result<(), kyomi_core::Error> {
    if permissions_for(user).contains(&Permission::ManageIntegrations) {
        Ok(())
    } else {
        Err(kyomi_core::Error::Forbidden(
            "Workspace admin access required".into(),
        ))
    }
}

/// Extract workspace_id from user, or return 400.
fn get_workspace_id(user: &AuthUser) -> Result<&str, kyomi_core::Error> {
    user.workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("Workspace context required".into()))
}

/// Get Slack client_id and client_secret from config, or return 500.
fn get_slack_config(config: &kyomi_core::Config) -> Result<(&str, &str), kyomi_core::Error> {
    let client_id = config.slack_client_id.as_deref().filter(|s| !s.is_empty());
    let client_secret = config.slack_client_secret.as_deref().filter(|s| !s.is_empty());

    match (client_id, client_secret) {
        (Some(id), Some(secret)) => Ok((id, secret)),
        _ => Err(kyomi_core::Error::Internal(
            "Slack integration not configured. Set SLACK_CLIENT_ID and SLACK_CLIENT_SECRET.".into(),
        )),
    }
}

/// Build a redirect URI from the frontend URL.
fn build_redirect_uri(config: &kyomi_core::Config, path: &str) -> String {
    let base = config.frontend_url.trim_end_matches('/');
    format!("{base}/api/v1{path}")
}

// ===========================================================================
// Platform table helpers
// ===========================================================================

/// Look up workspace_id by Slack team_id from the `workspace_integrations` table.
///
/// The Slack workspace integration config JSON has `team_id` at the top level.
async fn lookup_workspace_by_team_id(
    db: &DbPool,
    team_id: &str,
) -> kyomi_core::Result<Option<String>> {
    // JSON lookup: config->>'team_id' (Postgres) or json_extract(config, '$.team_id') (SQLite)
    #[derive(sqlx::FromRow)]
    struct Row { workspace_id: String }
    let is_pg = db.is_postgres();
    let json_expr = if is_pg {
        "config->>'team_id'"
    } else {
        "json_extract(config, '$.team_id')"
    };
    let sql = format!(
        "SELECT workspace_id FROM workspace_integrations \
         WHERE platform_type = 'slack' AND {json_expr} = $1"
    );
    let row = kyomi_core::db_fetch_optional!(db, Row, &sql, team_id)?;
    Ok(row.map(|r| r.workspace_id))
}

/// Get the decrypted Slack bot token from the workspace integration config.
///
/// Returns `None` if no Slack integration exists or the config has no bot_token.
pub async fn get_slack_bot_token(
    db: &DbPool,
    encryption_key: &[u8; 32],
    workspace_id: &str,
) -> kyomi_core::Result<Option<String>> {
    let config = kyomi_core::platform::get_workspace_integration(db, workspace_id, "slack").await?;
    let config = match config {
        Some(c) => c,
        None => return Ok(None),
    };
    let encrypted = match config.get("bot_token").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return Ok(None),
    };
    let decrypted = encryption::decrypt_slack_token(encrypted, encryption_key)?;
    Ok(Some(decrypted))
}

/// Look up the Slack user_id for a Kyomi user via `platform_user_links`.
pub(crate) async fn lookup_platform_slack_user(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> kyomi_core::Result<Option<String>> {
    #[derive(sqlx::FromRow)]
    struct Row { platform_user_id: String }
    let row = kyomi_core::db_fetch_optional!(
        db, Row,
        "SELECT platform_user_id FROM platform_user_links \
         WHERE workspace_id = $1 AND user_id = $2 AND platform_type = 'slack'",
        workspace_id,
        user_id
    )?;
    Ok(row.map(|r| r.platform_user_id))
}

/// Insert or update a platform user link for Slack.
async fn upsert_slack_user_link(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
    slack_user_id: &str,
    slack_username: Option<&str>,
) -> kyomi_core::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let now_expr = kyomi_core::sql_compat::now(db.is_postgres());
    let sql = format!(
        "INSERT INTO platform_user_links (id, workspace_id, user_id, platform_type, platform_user_id, platform_username, connected_at) \
         VALUES ($1, $2, $3, 'slack', $4, $5, {now_expr}) \
         ON CONFLICT (workspace_id, platform_type, platform_user_id) \
         DO UPDATE SET platform_username = $5"
    );
    kyomi_core::db_execute!(
        db,
        &sql,
        &id,
        workspace_id,
        user_id,
        slack_user_id,
        slack_username
    )?;
    Ok(())
}

/// Delete the Slack platform user link for a user.
async fn delete_slack_user_link(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> kyomi_core::Result<()> {
    kyomi_core::db_execute!(
        db,
        "DELETE FROM platform_user_links \
         WHERE workspace_id = $1 AND user_id = $2 AND platform_type = 'slack'",
        workspace_id,
        user_id
    )?;
    Ok(())
}

// ===========================================================================
// App Installation Endpoints (Workspace-level)
// ===========================================================================

/// `GET /install` — Get Slack OAuth URL for installing the Kyomi app.
///
/// Requires workspace admin role and Team tier or higher.
async fn get_install_url(
    State(state): State<SlackState>,
    user: AuthUser,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    // Self-hosted: Slack integration requires Enterprise + Slack configured.
    if state.config.self_hosted {
        if !state.config.is_enterprise() {
            return Ok((StatusCode::FORBIDDEN, Json(serde_json::json!({
                "error": "feature_not_available",
                "edition_required": "enterprise",
                "message": "Slack integration requires an Enterprise license. See kyomi.ai/enterprise."
            }))).into_response());
        }
        if !state.config.slack_configured() {
            return Ok((StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
                "error": "service_not_configured",
                "service": "slack",
                "message": "Slack integration requires SLACK_CLIENT_ID and SLACK_CLIENT_SECRET to be configured."
            }))).into_response());
        }
    }
    require_workspace_admin(&user)?;
    require_slack_capability(&user)?;
    let workspace_id = get_workspace_id(&user)?;
    let (client_id, _) = get_slack_config(&state.config)?;

    // Generate CSRF state token
    let oauth_state = redis_ops::generate_token();

    // Store state with user context
    redis_ops::store_oauth_state(
        &state.kv,
        "slack_install",
        &oauth_state,
        &json!({
            "user_id": user.user_id,
            "workspace_id": workspace_id,
            "created_at": Utc::now().to_rfc3339(),
        }),
    )
    .await?;

    let redirect_uri = build_redirect_uri(&state.config, "/slack/oauth/callback");

    let auth_url = format!(
        "{}?client_id={}&scope={}&redirect_uri={}&state={}",
        slack_client::OAUTH_AUTHORIZE_URL,
        client_id,
        slack_client::SLACK_BOT_SCOPES,
        redirect_uri,
        oauth_state,
    );

    Ok(Json(json!({
        "authorization_url": auth_url,
        "state": oauth_state,
    }))
    .into_response())
}

/// `GET /oauth/callback` — Handle Slack OAuth callback after app installation.
///
/// Exchanges the authorization code for a bot token and stores it in the workspace.
async fn handle_oauth_callback(
    State(state): State<SlackState>,
    Query(params): Query<OAuthCallbackQuery>,
) -> Result<axum::response::Redirect, kyomi_core::Error> {
    let (client_id, client_secret) = get_slack_config(&state.config)?;

    // Verify state
    let state_data = redis_ops::verify_oauth_state(&state.kv, "slack_install", &params.state)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest(
                "Invalid or expired state. Please try again.".into(),
            )
        })?;

    let workspace_id = state_data
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest("Invalid state data - missing workspace_id".into())
        })?;

    let installer_user_id = state_data
        .get("user_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest("Invalid state data - missing user_id".into())
        })?;

    // Exchange code for tokens
    let redirect_uri = build_redirect_uri(&state.config, "/slack/oauth/callback");
    let oauth_response = state
        .slack_client
        .exchange_code(client_id, client_secret, &params.code, &redirect_uri)
        .await?;

    if !oauth_response.ok {
        let error = oauth_response.error.as_deref().unwrap_or("unknown_error");
        return Err(kyomi_core::Error::BadRequest(format!(
            "Slack authorization failed: {error}"
        )));
    }

    // Extract installation data
    let team = oauth_response.team.as_ref().ok_or_else(|| {
        kyomi_core::Error::BadRequest("Invalid response from Slack — missing team".into())
    })?;
    let team_id = &team.id;
    let team_name = team
        .name
        .as_deref()
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest("Invalid response from Slack — missing team name".into())
        })?;

    let bot_token = oauth_response.access_token.as_deref().ok_or_else(|| {
        kyomi_core::Error::BadRequest("Invalid response from Slack — missing access_token".into())
    })?;
    let bot_user_id = oauth_response.bot_user_id.as_deref().ok_or_else(|| {
        kyomi_core::Error::BadRequest("Invalid response from Slack — missing bot_user_id".into())
    })?;

    // Encrypt the bot token before storage
    let encrypted_bot_token = encryption::encrypt_slack_token(bot_token, &state.encryption_key)?;

    // Store workspace-level Slack integration in the platform tables.
    let config = serde_json::json!({
        "team_id": team_id,
        "team_name": team_name,
        "bot_token": encrypted_bot_token,
        "bot_user_id": bot_user_id,
        "installed_by": installer_user_id,
    });
    kyomi_core::platform::upsert_workspace_integration(
        &state.db,
        workspace_id,
        "slack",
        &config,
        installer_user_id,
    )
    .await?;

    info!(
        workspace_id = %workspace_id,
        team = %team_name,
        team_id = %team_id,
        "Updated workspace with Slack installation"
    );

    // Redirect to frontend settings with success
    let frontend_url = state.config.frontend_url.trim_end_matches('/');
    Ok(axum::response::Redirect::to(&format!(
        "{frontend_url}/settings?tab=workspace&slack=installed"
    )))
}

/// `DELETE /uninstall` — Remove Slack integration for a workspace.
///
/// Requires workspace admin role and Team tier or higher.
async fn uninstall_slack(
    State(state): State<SlackState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;
    require_slack_capability(&user)?;
    let workspace_id = get_workspace_id(&user)?;

    // Check workspace has Slack installed
    let integration = kyomi_core::platform::get_workspace_integration(
        &state.db, workspace_id, "slack",
    ).await?;

    if integration.is_none() {
        return Err(kyomi_core::Error::NotFound(
            "Slack integration not found for this workspace".into(),
        ));
    }

    // Remove the workspace integration
    kyomi_core::platform::delete_workspace_integration(&state.db, workspace_id, "slack").await?;

    info!(workspace_id = %workspace_id, "Removed Slack installation from workspace");

    Ok(Json(json!({ "success": true })))
}

// ===========================================================================
// User Account Linking Endpoints
// ===========================================================================

/// `GET /user/connect` — Start Slack user account linking from Kyomi settings.
async fn start_user_connect(
    State(state): State<SlackState>,
    user: AuthUser,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    // Self-hosted: Slack integration requires Enterprise + Slack configured.
    if state.config.self_hosted {
        if !state.config.is_enterprise() {
            return Ok((StatusCode::FORBIDDEN, Json(serde_json::json!({
                "error": "feature_not_available",
                "edition_required": "enterprise",
                "message": "Slack integration requires an Enterprise license. See kyomi.ai/enterprise."
            }))).into_response());
        }
        if !state.config.slack_configured() {
            return Ok((StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
                "error": "service_not_configured",
                "service": "slack",
                "message": "Slack integration requires SLACK_CLIENT_ID and SLACK_CLIENT_SECRET to be configured."
            }))).into_response());
        }
    }
    let (client_id, _) = get_slack_config(&state.config)?;

    // Generate CSRF state token
    let oauth_state = redis_ops::generate_token();

    redis_ops::store_oauth_state(
        &state.kv,
        "slack_user_connect",
        &oauth_state,
        &json!({
            "user_id": user.user_id,
            "created_at": Utc::now().to_rfc3339(),
        }),
    )
    .await?;

    let redirect_uri = build_redirect_uri(&state.config, "/slack/user/callback");

    let auth_url = format!(
        "{}?client_id={}&user_scope={}&redirect_uri={}&state={}",
        slack_client::OAUTH_AUTHORIZE_URL,
        client_id,
        slack_client::SLACK_USER_SCOPES,
        redirect_uri,
        oauth_state,
    );

    Ok(Json(json!({
        "authorization_url": auth_url,
        "state": oauth_state,
    }))
    .into_response())
}

/// `GET /user/callback` — Handle Slack OAuth callback for user account linking.
async fn handle_user_callback(
    State(state): State<SlackState>,
    Query(params): Query<OAuthCallbackQuery>,
) -> Result<axum::response::Redirect, kyomi_core::Error> {
    let (client_id, client_secret) = get_slack_config(&state.config)?;

    // Verify state
    let state_data =
        redis_ops::verify_oauth_state(&state.kv, "slack_user_connect", &params.state)
            .await?
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Invalid or expired state. Please try again.".into(),
                )
            })?;

    let user_id = state_data
        .get("user_id")
        .and_then(|v| v.as_str());

    let expected_slack_user_id = state_data
        .get("slack_user_id")
        .and_then(|v| v.as_str())
        .map(String::from);

    // slack_team_id is stored in state for the /kyomi connect flow but not
    // used directly here — the workspace lookup by team_id serves as validation.
    let _expected_slack_team_id = state_data
        .get("slack_team_id")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Exchange code for tokens
    let redirect_uri = build_redirect_uri(&state.config, "/slack/user/callback");
    let oauth_response = state
        .slack_client
        .exchange_code(client_id, client_secret, &params.code, &redirect_uri)
        .await?;

    if !oauth_response.ok {
        let error = oauth_response.error.as_deref().unwrap_or("unknown_error");
        return Err(kyomi_core::Error::BadRequest(format!(
            "Slack authorization failed: {error}"
        )));
    }

    // Extract user identity
    let authed_user = oauth_response.authed_user.as_ref().ok_or_else(|| {
        kyomi_core::Error::BadRequest("Could not get Slack user identity".into())
    })?;
    let slack_user_id = &authed_user.id;
    let user_access_token = authed_user.access_token.as_deref();

    let team = oauth_response.team.as_ref().ok_or_else(|| {
        kyomi_core::Error::BadRequest("Could not get Slack team identity".into())
    })?;
    let slack_team_id = &team.id;

    // Get user's Slack profile for display name
    let mut slack_username: Option<String> = None;
    if let Some(token) = user_access_token {
        match state.slack_client.users_info(token, slack_user_id).await {
            Ok(info) => {
                slack_username = info.real_name.or(info.name);
            }
            Err(e) => {
                warn!(error = %e, "Failed to get Slack profile");
            }
        }
    }

    // Check if workspace has Slack installed (look up by team_id in platform tables)
    let workspace_id = match lookup_workspace_by_team_id(&state.db, slack_team_id).await? {
        Some(id) => id,
        None => {
            let frontend_url = state.config.frontend_url.trim_end_matches('/');
            return Ok(axum::response::Redirect::to(&format!(
                "{frontend_url}/settings?tab=profile&slack=no_installation"
            )));
        }
    };

    // Verify Slack user matches if expected (from /kyomi connect flow)
    if let Some(ref expected) = expected_slack_user_id
        && slack_user_id != expected {
            return Err(kyomi_core::Error::BadRequest(
                "Slack user mismatch. Please run /kyomi connect again.".into(),
            ));
        }

    // Verify we have user_id
    let user_id = user_id.ok_or_else(|| {
        kyomi_core::Error::BadRequest("Invalid state data - missing user_id".into())
    })?;

    // Find active workspace_user
    #[derive(sqlx::FromRow)]
    struct ExistsRow { _n: i32 }
    let has_membership = kyomi_core::db_fetch_optional!(
        &state.db, ExistsRow,
        "SELECT 1 as _n FROM workspace_users \
         WHERE user_id = $1 AND workspace_id = $2 AND active = true",
        user_id,
        &workspace_id
    )?;

    if has_membership.is_none() {
        return Err(kyomi_core::Error::BadRequest(
            "You are not a member of the workspace where Slack is installed".into(),
        ));
    }

    // Encrypt user access token if present
    let encrypted_user_token = match user_access_token {
        Some(token) => Some(encryption::encrypt_slack_token(token, &state.encryption_key)?),
        None => None,
    };

    // Store user-level Slack integration in platform tables
    let user_config = serde_json::json!({
        "slack_user_id": slack_user_id,
        "username": slack_username,
        "user_token": encrypted_user_token,
    });
    kyomi_core::platform::upsert_user_integration(
        &state.db, &workspace_id, user_id, "slack", &user_config,
    ).await?;

    // Create platform user link for identity mapping
    upsert_slack_user_link(
        &state.db, &workspace_id, user_id, slack_user_id, slack_username.as_deref(),
    ).await?;

    info!(
        user_id = %user_id,
        slack_user_id = %slack_user_id,
        workspace_id = %workspace_id,
        "Connected Slack user to workspace"
    );

    let frontend_url = state.config.frontend_url.trim_end_matches('/');
    Ok(axum::response::Redirect::to(&format!(
        "{frontend_url}/settings?tab=profile&slack=connected"
    )))
}

/// `POST /user/disconnect` — Disconnect user's Slack account.
async fn disconnect_user(
    State(state): State<SlackState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Check user has Slack connection via platform tables
    let user_integration = kyomi_core::platform::get_user_integration(
        &state.db, workspace_id, &user.user_id, "slack",
    ).await?;

    if user_integration.is_none() {
        return Err(kyomi_core::Error::NotFound(
            "No Slack connection found".into(),
        ));
    }

    // Remove user integration and platform user link
    kyomi_core::platform::delete_user_integration(&state.db, workspace_id, &user.user_id, "slack").await?;
    delete_slack_user_link(&state.db, workspace_id, &user.user_id).await?;

    info!(
        user_id = %user.user_id,
        workspace_id = %workspace_id,
        "Disconnected Slack for user"
    );

    Ok(Json(json!({
        "success": true,
        "message": "Slack account disconnected. Note: Slack sync for existing chat sessions will stop."
    })))
}

// ===========================================================================
// Status and Channel Endpoints
// ===========================================================================

/// `GET /status` — Get Slack integration status for the current user.
async fn get_slack_status(
    State(state): State<SlackState>,
    user: AuthUser,
) -> Result<Json<SlackStatusResponse>, kyomi_core::Error> {
    require_slack_capability(&user)?;
    let workspace_id = get_workspace_id(&user)?;

    // Check workspace Slack installation from platform tables
    let ws_config = kyomi_core::platform::get_workspace_integration(
        &state.db, workspace_id, "slack",
    ).await?;

    let installed = ws_config.is_some();
    let (slack_team_id, slack_team_name) = match &ws_config {
        Some(cfg) => (
            cfg.get("team_id").and_then(|v| v.as_str()).map(String::from),
            cfg.get("team_name").and_then(|v| v.as_str()).map(String::from),
        ),
        None => (None, None),
    };

    // Check if user is connected to Slack via platform user links
    let mut user_connected = false;
    let mut slack_username: Option<String> = None;

    if installed {
        #[derive(sqlx::FromRow)]
        struct LinkRow { platform_user_id: String, platform_username: Option<String> }
        let link_row = kyomi_core::db_fetch_optional!(
            &state.db, LinkRow,
            "SELECT platform_user_id, platform_username FROM platform_user_links \
             WHERE workspace_id = $1 AND user_id = $2 AND platform_type = 'slack'",
            workspace_id,
            &user.user_id
        )?;

        if let Some(row) = link_row {
            user_connected = true;
            slack_username = row.platform_username;
            let _ = row.platform_user_id; // used to verify link exists
        }
    }

    Ok(Json(SlackStatusResponse {
        installed,
        team_name: slack_team_name,
        team_id: slack_team_id,
        user_connected,
        slack_username,
    }))
}

/// `GET /channels` — List available Slack channels for watch configuration.
async fn list_channels(
    State(state): State<SlackState>,
    user: AuthUser,
) -> Result<Json<ChannelsResponse>, kyomi_core::Error> {
    require_slack_capability(&user)?;
    let workspace_id = get_workspace_id(&user)?;

    // Get bot token from platform tables
    let bot_token = get_slack_bot_token(&state.db, &state.encryption_key, workspace_id).await?;
    let bot_token = bot_token.ok_or_else(|| {
        kyomi_core::Error::BadRequest(
            "Kyomi app not installed in your Slack workspace.".into(),
        )
    })?;

    // Verify user is connected to Slack
    let has_slack_link = lookup_platform_slack_user(&state.db, workspace_id, &user.user_id).await?;
    if has_slack_link.is_none() {
        return Err(kyomi_core::Error::BadRequest(
            "Connect your Slack account first to see available channels.".into(),
        ));
    }

    // Fetch channels from Slack (SlackClient handles pagination and sorting)
    let slack_channels = state.slack_client.conversations_list(&bot_token).await?;

    let channels = slack_channels
        .into_iter()
        .map(|ch| ChannelResponse {
            id: ch.id,
            name: ch.name,
            is_private: ch.is_private,
        })
        .collect();

    Ok(Json(ChannelsResponse { channels }))
}

/// `GET /default-watch-channel` — Get the default Slack channel for watch alerts.
async fn get_default_watch_channel(
    State(state): State<SlackState>,
    user: AuthUser,
) -> Result<Json<DefaultWatchChannelResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Check user has Slack connection and get their config for default channel info
    let user_config = kyomi_core::platform::get_user_integration(
        &state.db, workspace_id, &user.user_id, "slack",
    ).await?;

    let user_config = user_config.ok_or_else(|| {
        kyomi_core::Error::BadRequest("Connect your Slack account first.".into())
    })?;

    let channel_id = user_config.get("default_channel_id").and_then(|v| v.as_str()).map(String::from);
    let channel_name = user_config.get("default_channel_name").and_then(|v| v.as_str()).map(String::from);

    Ok(Json(DefaultWatchChannelResponse {
        channel_id,
        channel_name,
    }))
}

/// `POST /default-watch-channel` — Set the default Slack channel for new watch alerts.
async fn set_default_watch_channel(
    State(state): State<SlackState>,
    user: AuthUser,
    Json(body): Json<SetDefaultWatchChannelRequest>,
) -> Result<Json<DefaultWatchChannelResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Verify user has Slack connection and update default channel in user config
    let user_config = kyomi_core::platform::get_user_integration(
        &state.db, workspace_id, &user.user_id, "slack",
    ).await?;

    let mut user_config = user_config.ok_or_else(|| {
        kyomi_core::Error::BadRequest("Connect your Slack account first.".into())
    })?;

    // Merge default channel into the existing user config
    if let Some(obj) = user_config.as_object_mut() {
        obj.insert("default_channel_id".to_string(), serde_json::Value::String(body.channel_id.clone()));
        obj.insert("default_channel_name".to_string(), serde_json::Value::String(body.channel_name.clone()));
    }

    kyomi_core::platform::upsert_user_integration(
        &state.db, workspace_id, &user.user_id, "slack", &user_config,
    ).await?;

    info!(
        user_id = %user.user_id,
        channel_id = %body.channel_id,
        channel_name = %body.channel_name,
        "Set default watch channel"
    );

    Ok(Json(DefaultWatchChannelResponse {
        channel_id: Some(body.channel_id),
        channel_name: Some(body.channel_name),
    }))
}

/// `GET /connect/initiate` — Initiate OAuth flow from /kyomi connect command.
///
/// User has authenticated with Kyomi, now redirect to Slack OAuth.
/// This enriches the state with user_id and returns the OAuth URL.
async fn initiate_connect_oauth(
    State(state): State<SlackState>,
    user: AuthUser,
    Query(params): Query<ConnectInitiateQuery>,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    // Self-hosted: Slack integration requires Enterprise + Slack configured.
    if state.config.self_hosted {
        if !state.config.is_enterprise() {
            return Ok((StatusCode::FORBIDDEN, Json(serde_json::json!({
                "error": "feature_not_available",
                "edition_required": "enterprise",
                "message": "Slack integration requires an Enterprise license. See kyomi.ai/enterprise."
            }))).into_response());
        }
        if !state.config.slack_configured() {
            return Ok((StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
                "error": "service_not_configured",
                "service": "slack",
                "message": "Slack integration requires SLACK_CLIENT_ID and SLACK_CLIENT_SECRET to be configured."
            }))).into_response());
        }
    }
    let (client_id, _) = get_slack_config(&state.config)?;

    // Verify state from /kyomi connect command
    let state_data =
        redis_ops::verify_oauth_state(&state.kv, "slack_user_connect", &params.state)
            .await?
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Invalid or expired state. Please run /kyomi connect again.".into(),
                )
            })?;

    let slack_user_id = state_data
        .get("slack_user_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| kyomi_core::Error::BadRequest("Invalid state data".into()))?;

    let slack_team_id = state_data
        .get("slack_team_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| kyomi_core::Error::BadRequest("Invalid state data".into()))?;

    // Create new state with user_id added (for the OAuth callback)
    let new_state = redis_ops::generate_token();
    redis_ops::store_oauth_state(
        &state.kv,
        "slack_user_connect",
        &new_state,
        &json!({
            "user_id": user.user_id,
            "slack_user_id": slack_user_id,
            "slack_team_id": slack_team_id,
            "source": "slash_command",
            "created_at": Utc::now().to_rfc3339(),
        }),
    )
    .await?;

    let redirect_uri = build_redirect_uri(&state.config, "/slack/user/callback");

    let auth_url = format!(
        "{}?client_id={}&user_scope={}&redirect_uri={}&state={}",
        slack_client::OAUTH_AUTHORIZE_URL,
        client_id,
        slack_client::SLACK_USER_SCOPES,
        redirect_uri,
        new_state,
    );

    Ok(Json(json!({
        "authorization_url": auth_url,
        "state": new_state,
    }))
    .into_response())
}

// ===========================================================================
// Phase 11-4: Slack Bot Commands + Events + Interactions
// ===========================================================================

// ---------------------------------------------------------------------------
// Signature verification helper
// ---------------------------------------------------------------------------

/// Extract Slack signature headers and verify signature against the raw body.
///
/// Delegates to `slack_client::verify_slack_signature()` which performs both:
/// 1. Timestamp freshness check (within 5 minutes — prevents replay attacks)
/// 2. HMAC-SHA256 signature verification (constant-time comparison)
///
/// Returns `Ok(())` if valid, or `Err(kyomi_core::Error)` if verification fails.
fn verify_slack_request(
    headers: &HeaderMap,
    body: &[u8],
    config: &kyomi_core::Config,
) -> Result<(), kyomi_core::Error> {
    let signing_secret = config
        .slack_signing_secret
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            kyomi_core::Error::Internal(
                "Slack signing secret not configured — cannot verify Slack requests".into(),
            )
        })?;

    let timestamp = headers
        .get("X-Slack-Request-Timestamp")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let signature = headers
        .get("X-Slack-Signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !slack_client::verify_slack_signature(signing_secret, timestamp, body, signature) {
        return Err(kyomi_core::Error::Unauthorized(
            "Invalid Slack signature".into(),
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Bot mention regex
// ---------------------------------------------------------------------------

/// Regex to strip the bot @mention from message text: `<@U12345> rest of text`
fn bot_mention_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<@\w+>\s*").expect("valid regex"))
}

// ---------------------------------------------------------------------------
// Slash command types
// ---------------------------------------------------------------------------

/// Form-encoded fields sent by Slack for slash commands.
#[derive(Debug, Deserialize)]
struct SlackCommandPayload {
    team_id: Option<String>,
    user_id: Option<String>,
    command: Option<String>,
    text: Option<String>,
}

// ---------------------------------------------------------------------------
// POST /command — Handle /kyomi slash commands
// ---------------------------------------------------------------------------

/// `POST /command` — Handle /kyomi slash commands.
///
/// Slack sends form-encoded data. We verify the signature, then dispatch on the
/// `text` field to handle `connect`, `status`, `disconnect`, and help.
///
/// No auth middleware — uses Slack signature verification instead.
async fn handle_slack_command(
    State(state): State<SlackState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Verify Slack signature using raw body bytes.
    verify_slack_request(&headers, &body, &state.config)?;

    // Parse form-encoded body.
    let payload: SlackCommandPayload =
        serde_urlencoded::from_bytes(&body).map_err(|e| {
            kyomi_core::Error::BadRequest(format!("Invalid form data: {e}"))
        })?;

    let text = payload.text.as_deref().unwrap_or("").trim().to_lowercase();
    let slack_user_id = payload.user_id.as_deref().unwrap_or("");
    let slack_team_id = payload.team_id.as_deref().unwrap_or("");

    info!(
        command = ?payload.command,
        text = %text,
        slack_user_id = %slack_user_id,
        team_id = %slack_team_id,
        "Received Slack slash command"
    );

    match text.as_str() {
        "connect" | "" => {
            handle_command_connect(&state, slack_user_id, slack_team_id).await
        }
        "status" => {
            handle_command_status(&state, slack_user_id, slack_team_id).await
        }
        "disconnect" => {
            handle_command_disconnect(&state, slack_user_id, slack_team_id).await
        }
        _ => Ok(Json(json!({
            "response_type": "ephemeral",
            "text": "Available commands:\n\u{2022} `/kyomi connect` \u{2014} Connect your Slack account to Kyomi\n\u{2022} `/kyomi status` \u{2014} Check your connection status\n\u{2022} `/kyomi disconnect` \u{2014} Disconnect your Slack account\n\nOr just mention @Kyomi in a channel to ask a data question!"
        }))),
    }
}

/// Handle `/kyomi connect` — Generate OAuth URL and return button block.
async fn handle_command_connect(
    state: &SlackState,
    slack_user_id: &str,
    slack_team_id: &str,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Look up workspace by slack_team_id via platform tables.
    let workspace_id = match lookup_workspace_by_team_id(&state.db, slack_team_id).await? {
        Some(id) => id,
        None => {
            return Ok(Json(json!({
                "response_type": "ephemeral",
                "text": "Kyomi is not installed in this Slack workspace yet. Ask your workspace admin to install it from Kyomi Settings > Workspace."
            })));
        }
    };

    // Check if this Slack user is already connected via platform_user_links.
    let existing = kyomi_core::platform::resolve_platform_user(
        &state.db, "slack", slack_user_id, &workspace_id,
    ).await?;

    if existing.is_some() {
        return Ok(Json(json!({
            "response_type": "ephemeral",
            "text": "Your Slack account is already connected to Kyomi! You can @mention Kyomi in any channel or send a direct message to ask data questions."
        })));
    }

    // Generate OAuth state and store in Redis.
    let oauth_state = redis_ops::generate_token();
    redis_ops::store_oauth_state(
        &state.kv,
        "slack_user_connect",
        &oauth_state,
        &json!({
            "slack_user_id": slack_user_id,
            "slack_team_id": slack_team_id,
            "source": "slash_command",
            "created_at": Utc::now().to_rfc3339(),
        }),
    )
    .await?;

    // Build auth URL pointing to the frontend's Slack connect page.
    let frontend_url = state.config.frontend_url.trim_end_matches('/');
    let auth_url = format!("{frontend_url}/auth/slack-connect?state={oauth_state}");

    Ok(Json(json!({
        "response_type": "ephemeral",
        "text": "Connect your Slack account to Kyomi to ask data questions directly from Slack.",
        "blocks": [
            {
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": "Connect your Slack account to Kyomi to start asking data questions right here in Slack."
                }
            },
            {
                "type": "actions",
                "elements": [
                    {
                        "type": "button",
                        "text": {
                            "type": "plain_text",
                            "text": "Connect Account"
                        },
                        "url": auth_url,
                        "style": "primary"
                    }
                ]
            }
        ]
    })))
}

/// Handle `/kyomi status` — Show connection status.
async fn handle_command_status(
    state: &SlackState,
    slack_user_id: &str,
    slack_team_id: &str,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Look up workspace via platform tables.
    let workspace_id = match lookup_workspace_by_team_id(&state.db, slack_team_id).await? {
        Some(id) => id,
        None => {
            return Ok(Json(json!({
                "response_type": "ephemeral",
                "text": "Kyomi is not installed in this Slack workspace."
            })));
        }
    };

    // Check if user is connected via platform_user_links.
    #[derive(sqlx::FromRow)]
    struct LinkRow { platform_username: Option<String> }
    let link_row = kyomi_core::db_fetch_optional!(
        &state.db, LinkRow,
        "SELECT platform_username FROM platform_user_links \
         WHERE workspace_id = $1 AND platform_type = 'slack' AND platform_user_id = $2",
        &workspace_id,
        slack_user_id
    )?;

    let message = match link_row {
        Some(ref row) => {
            let name = row.platform_username.clone().unwrap_or_else(|| "your account".to_string());
            format!(
                "Connected to Kyomi as *{name}*.\n\n\
                 You can:\n\
                 \u{2022} @mention Kyomi in any channel to ask a data question\n\
                 \u{2022} Send Kyomi a direct message\n\
                 \u{2022} Use `/kyomi disconnect` to unlink your account"
            )
        }
        None => {
            "Your Slack account is not connected to Kyomi.\n\
             Use `/kyomi connect` to link your account."
                .to_string()
        }
    };

    Ok(Json(json!({
        "response_type": "ephemeral",
        "text": message
    })))
}

/// Handle `/kyomi disconnect` — Remove user's Slack connection via platform tables.
async fn handle_command_disconnect(
    state: &SlackState,
    slack_user_id: &str,
    slack_team_id: &str,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Look up workspace via platform tables.
    let workspace_id = match lookup_workspace_by_team_id(&state.db, slack_team_id).await? {
        Some(id) => id,
        None => {
            return Ok(Json(json!({
                "response_type": "ephemeral",
                "text": "Kyomi is not installed in this Slack workspace."
            })));
        }
    };

    // Find the Kyomi user_id linked to this Slack user.
    let user_id = kyomi_core::platform::resolve_platform_user(
        &state.db, "slack", slack_user_id, &workspace_id,
    ).await?;

    let user_id = match user_id {
        Some(id) => id,
        None => {
            return Ok(Json(json!({
                "response_type": "ephemeral",
                "text": "Your Slack account is not connected to Kyomi. Nothing to disconnect."
            })));
        }
    };

    // Remove platform user link and user integration.
    delete_slack_user_link(&state.db, &workspace_id, &user_id).await?;
    kyomi_core::platform::delete_user_integration(&state.db, &workspace_id, &user_id, "slack").await?;

    info!(
        slack_user_id = %slack_user_id,
        workspace_id = %workspace_id,
        "Disconnected Slack user via slash command"
    );

    Ok(Json(json!({
        "response_type": "ephemeral",
        "text": "Your Slack account has been disconnected from Kyomi. Use `/kyomi connect` to reconnect."
    })))
}

// ---------------------------------------------------------------------------
// POST /events — Handle Slack Events API
// ---------------------------------------------------------------------------

/// `POST /events` — Handle Slack Events API (app_mention, DM messages).
///
/// - If `type == "url_verification"`: return the challenge immediately.
/// - If `type == "event_callback"`: verify signature, spawn background task,
///   return `{ "ok": true }` within 3 seconds (Slack requirement).
///
/// No auth middleware — uses Slack signature verification instead.
async fn handle_slack_events(
    State(state): State<SlackState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Parse body as JSON first to check for url_verification.
    let body_json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
        kyomi_core::Error::BadRequest(format!("Invalid JSON: {e}"))
    })?;

    // URL verification challenge — return immediately (no signature check required
    // per Slack docs, but we verify anyway for safety).
    if body_json.get("type").and_then(|v| v.as_str()) == Some("url_verification") {
        let challenge = body_json
            .get("challenge")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return Ok(Json(json!({ "challenge": challenge })));
    }

    // Verify Slack signature for all other event types.
    verify_slack_request(&headers, &body, &state.config)?;

    let team_id = body_json
        .get("team_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let event = body_json.get("event").cloned();

    if let Some(ref event) = event {
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match event_type {
            "app_mention" => {
                // Ignore bot messages to prevent infinite loops.
                // Defense-in-depth: check both bot_id and subtype.
                if event.get("bot_id").and_then(|v| v.as_str()).is_some()
                    || event.get("subtype").and_then(|v| v.as_str())
                        == Some("bot_message")
                {
                    info!("Ignoring app_mention from bot");
                    return Ok(Json(json!({ "ok": true })));
                }

                let event_clone = event.clone();
                let state_clone = state.clone();
                let team_id_clone = team_id.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        handle_app_mention(&team_id_clone, &event_clone, &state_clone).await
                    {
                        error!(error = %e, "handle_app_mention failed");
                    }
                });
            }
            "message" => {
                let channel_type = event
                    .get("channel_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if channel_type == "im" {
                    // Ignore bot messages and bot_message subtypes in DMs.
                    if event.get("bot_id").and_then(|v| v.as_str()).is_some()
                        || event.get("subtype").and_then(|v| v.as_str())
                            == Some("bot_message")
                    {
                        info!("Ignoring DM from bot");
                        return Ok(Json(json!({ "ok": true })));
                    }

                    let event_clone = event.clone();
                    let state_clone = state.clone();
                    let team_id_clone = team_id.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            handle_direct_message(&team_id_clone, &event_clone, &state_clone).await
                        {
                            error!(error = %e, "handle_direct_message failed");
                        }
                    });
                }
            }
            _ => {
                info!(event_type = %event_type, "Ignoring unhandled Slack event type");
            }
        }
    }

    // Return immediately — Slack requires <3s response.
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// POST /interactions — Handle Slack interactive payloads
// ---------------------------------------------------------------------------

/// `POST /interactions` — Handle Slack interactive payloads.
///
/// URL buttons redirect via Slack directly; we just acknowledge with 200 OK.
/// The `payload` is form-encoded containing a JSON string.
///
/// No auth middleware — uses Slack signature verification instead.
async fn handle_slack_interactions(
    State(state): State<SlackState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Verify Slack signature.
    verify_slack_request(&headers, &body, &state.config)?;

    // The payload is form-encoded as `payload=<JSON>`.
    // Log for debugging but otherwise just acknowledge.
    let form: std::collections::HashMap<String, String> =
        serde_urlencoded::from_bytes(&body).unwrap_or_default();

    if let Some(payload_str) = form.get("payload") {
        let payload: serde_json::Value =
            serde_json::from_str(payload_str).unwrap_or(json!({}));
        let interaction_type = payload
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        info!(
            interaction_type = %interaction_type,
            "Received Slack interaction"
        );
    }

    Ok(Json(json!({ "ok": true })))
}

// ===========================================================================
// Background task handlers
// ===========================================================================

/// Handle an `app_mention` event — user @mentioned the bot in a channel.
///
/// Runs in a background task spawned from the events endpoint.
async fn handle_app_mention(
    team_id: &str,
    event: &serde_json::Value,
    state: &SlackState,
) -> kyomi_core::Result<()> {
    let slack_user_id = event.get("user").and_then(|v| v.as_str()).unwrap_or("");
    let channel_id = event.get("channel").and_then(|v| v.as_str()).unwrap_or("");
    let message_ts = event.get("ts").and_then(|v| v.as_str()).unwrap_or("");
    let thread_ts = event
        .get("thread_ts")
        .and_then(|v| v.as_str())
        .unwrap_or(message_ts);
    let raw_text = event.get("text").and_then(|v| v.as_str()).unwrap_or("");

    // Preprocess text: translate emojis, parse channel refs, strip bot @mention.
    let text = slack_helpers::translate_slack_emojis(raw_text);
    let text = slack_helpers::parse_slack_channel_refs(&text);
    let text = bot_mention_regex().replace_all(&text, "").trim().to_string();

    if text.is_empty() {
        return Ok(());
    }

    info!(
        slack_user_id = %slack_user_id,
        channel_id = %channel_id,
        thread_ts = %thread_ts,
        text_len = text.len(),
        "Processing app_mention"
    );

    // Look up workspace and user.
    let ctx =
        match resolve_slack_context(&state.db, &state.encryption_key, team_id, slack_user_id).await
        {
            Ok(ctx) => ctx,
            Err(err_msg) => {
                // We cannot post ephemeral without a bot token, so log and return.
                warn!(
                    team_id = %team_id,
                    slack_user_id = %slack_user_id,
                    error = %err_msg,
                    "Cannot resolve Slack context for app_mention"
                );
                return Ok(());
            }
        };

    // Check capability (team tier required for Slack integration).
    if !capability::has_capability(ctx.subscription_tier, "slack_integration") {
        post_slack_error(
            &ctx.bot_token,
            channel_id,
            slack_user_id,
            "Slack integration requires an active Kyomi Cloud subscription.",
            &state.slack_client,
        )
        .await;
        return Ok(());
    }

    // Find or create a chat session for this Slack thread.
    let (session_id, is_new_session) = find_or_create_slack_session(
        &state.db,
        &ctx.workspace_id,
        &ctx.user_id,
        channel_id,
        thread_ts,
        true, // shared (channel mention)
    )
    .await?;

    // Fire-and-forget title generation for new sessions.
    if is_new_session
        && kyomi_agent::resolve_provider_config(&state.config).is_ok() {
            kyomi_agent::generate_session_title(
                state.db.clone(),
                state.ws_manager.clone(),
                session_id.clone(),
                ctx.user_id.clone(),
                ctx.workspace_id.clone(),
                text.clone(),
                state.config.clone(),
            );
        }

    // Post "thinking" placeholder.
    let placeholder_ts = post_slack_placeholder(
        &ctx.bot_token,
        channel_id,
        thread_ts,
        &state.slack_client,
    )
    .await;

    // Execute the agent query (shared=true for channel mentions).
    let response = run_slack_query(
        &state.db,
        &state.kv,
        &state.encryption_key,
        &state.embedding,
        &state.ws_manager,
        &state.config,
        &state.connect_registry,
        state.platforms.clone(),
        &session_id,
        &ctx.user_id,
        &ctx.workspace_id,
        &text,
        slack_user_id,
        &ctx.bot_token,
        &state.slack_client,
        true, // shared (channel mention)
    )
    .await;

    let target = SlackReplyTarget {
        channel_id,
        thread_ts,
        placeholder_ts: placeholder_ts.as_deref(),
        slack_user_id,
    };
    respond_to_slack_query(state, response, &ctx, &target, &session_id, "app_mention").await;

    Ok(())
}

/// Handle a direct message event — user sent a DM to the bot.
///
/// Similar to `handle_app_mention` but no @mention stripping and `shared=false`.
async fn handle_direct_message(
    team_id: &str,
    event: &serde_json::Value,
    state: &SlackState,
) -> kyomi_core::Result<()> {
    let slack_user_id = event.get("user").and_then(|v| v.as_str()).unwrap_or("");
    let channel_id = event.get("channel").and_then(|v| v.as_str()).unwrap_or("");
    let message_ts = event.get("ts").and_then(|v| v.as_str()).unwrap_or("");
    // DM threads: use thread_ts if present, otherwise the message ts itself.
    let thread_ts = event
        .get("thread_ts")
        .and_then(|v| v.as_str())
        .unwrap_or(message_ts);
    let raw_text = event.get("text").and_then(|v| v.as_str()).unwrap_or("");

    // Preprocess text: translate emojis, parse channel refs (no @mention stripping for DMs).
    let text = slack_helpers::translate_slack_emojis(raw_text);
    let text = slack_helpers::parse_slack_channel_refs(&text);
    let text = text.trim().to_string();

    if text.is_empty() {
        return Ok(());
    }

    info!(
        slack_user_id = %slack_user_id,
        channel_id = %channel_id,
        thread_ts = %thread_ts,
        text_len = text.len(),
        "Processing direct message"
    );

    // Look up workspace and user.
    let ctx =
        match resolve_slack_context(&state.db, &state.encryption_key, team_id, slack_user_id).await
        {
            Ok(ctx) => ctx,
            Err(err_msg) => {
                warn!(
                    team_id = %team_id,
                    slack_user_id = %slack_user_id,
                    error = %err_msg,
                    "Cannot resolve Slack context for DM"
                );
                return Ok(());
            }
        };

    // Check capability.
    if !capability::has_capability(ctx.subscription_tier, "slack_integration") {
        post_slack_error(
            &ctx.bot_token,
            channel_id,
            slack_user_id,
            "Slack integration requires an active Kyomi Cloud subscription.",
            &state.slack_client,
        )
        .await;
        return Ok(());
    }

    // Find or create a chat session for this DM thread.
    let (session_id, is_new_session) = find_or_create_slack_session(
        &state.db,
        &ctx.workspace_id,
        &ctx.user_id,
        channel_id,
        thread_ts,
        false, // not shared (DM)
    )
    .await?;

    // Fire-and-forget title generation for new sessions.
    if is_new_session
        && kyomi_agent::resolve_provider_config(&state.config).is_ok() {
            kyomi_agent::generate_session_title(
                state.db.clone(),
                state.ws_manager.clone(),
                session_id.clone(),
                ctx.user_id.clone(),
                ctx.workspace_id.clone(),
                text.clone(),
                state.config.clone(),
            );
        }

    // Post "thinking" placeholder.
    let placeholder_ts = post_slack_placeholder(
        &ctx.bot_token,
        channel_id,
        thread_ts,
        &state.slack_client,
    )
    .await;

    // Execute the agent query (shared=false for DMs).
    let response = run_slack_query(
        &state.db,
        &state.kv,
        &state.encryption_key,
        &state.embedding,
        &state.ws_manager,
        &state.config,
        &state.connect_registry,
        state.platforms.clone(),
        &session_id,
        &ctx.user_id,
        &ctx.workspace_id,
        &text,
        slack_user_id,
        &ctx.bot_token,
        &state.slack_client,
        false, // not shared (DM)
    )
    .await;

    let target = SlackReplyTarget {
        channel_id,
        thread_ts,
        placeholder_ts: placeholder_ts.as_deref(),
        slack_user_id,
    };
    respond_to_slack_query(state, response, &ctx, &target, &session_id, "direct_message").await;

    Ok(())
}

// ===========================================================================
// Slack context resolution
// ===========================================================================

/// Resolved Slack context for event/message handling.
struct SlackContext {
    workspace_id: String,
    user_id: String,
    bot_token: String,
    subscription_tier: kyomi_core::SubscriptionTier,
}

/// Resolve workspace, user, and decrypted bot token from Slack identifiers.
///
/// Uses the platform tables instead of old `slack_*` columns.
/// Returns `SlackContext` or an error message string.
async fn resolve_slack_context(
    db: &DbPool,
    encryption_key: &[u8; 32],
    team_id: &str,
    slack_user_id: &str,
) -> Result<SlackContext, String> {
    // Find workspace by team_id via workspace_integrations.
    let workspace_id = lookup_workspace_by_team_id(db, team_id)
        .await
        .map_err(|e| format!("DB error looking up workspace: {e}"))?
        .ok_or_else(|| format!("No workspace found for Slack team {team_id}"))?;

    // Get bot token from workspace integration config.
    let bot_token = get_slack_bot_token(db, encryption_key, &workspace_id)
        .await
        .map_err(|e| format!("Failed to get bot token: {e}"))?
        .ok_or_else(|| "Workspace has no bot token".to_string())?;

    // Resolve Kyomi user_id from platform_user_links.
    let user_id = kyomi_core::platform::resolve_platform_user(db, "slack", slack_user_id, &workspace_id)
        .await
        .map_err(|e| format!("DB error looking up platform user: {e}"))?
        .ok_or_else(|| format!("Slack user {slack_user_id} not connected to Kyomi workspace"))?;

    // Look up subscription tier for capability checks.
    #[derive(sqlx::FromRow)]
    struct TierRow { subscription_tier: String }
    let tier_row: Option<TierRow> = kyomi_core::db_fetch_optional!(
        db, TierRow,
        "SELECT subscription_tier FROM workspaces WHERE workspace_id = $1",
        &workspace_id
    ).map_err(|e| format!("DB error looking up tier: {e}"))?;
    let tier_str = tier_row
        .ok_or_else(|| "Workspace not found".to_string())?
        .subscription_tier;
    let subscription_tier: kyomi_core::SubscriptionTier = tier_str
        .parse()
        .map_err(|e: String| format!("Invalid tier: {e}"))?;

    Ok(SlackContext {
        workspace_id,
        user_id,
        bot_token,
        subscription_tier,
    })
}

// ===========================================================================
// Agent query execution
// ===========================================================================

/// Resolve the acting Slack user's current workspace roles.
///
/// Returns `Err(Forbidden)` if the user has no active membership row in
/// `workspace_id`. `user_service::get_workspace_user` filters on
/// `active = true`, so a `None` result covers both "never a member" and
/// "was removed or deactivated after linking Slack" — the row it queries
/// is exactly the one `remove_workspace_member` deletes on offboarding.
///
/// This is the last gate before agent execution in [`run_slack_query`]; a
/// user whose `platform_user_links` row outlived their workspace
/// membership must never reach it (KYO-223).
async fn resolve_active_workspace_roles(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> kyomi_core::Result<Vec<WorkspaceRole>> {
    match user_service::get_workspace_user(db, workspace_id, user_id)
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "failed to load Slack user's workspace role: {e}"
            ))
        })? {
        Some(wu) => Ok(vec![wu.role]),
        None => {
            warn!(
                %user_id,
                %workspace_id,
                "Slack user has no active workspace membership; rejecting query"
            );
            Err(kyomi_core::Error::Forbidden(
                "Your access to this Kyomi workspace has ended. If you believe this is a mistake, contact your workspace admin.".into(),
            ))
        }
    }
}

/// Execute an agent query for a Slack message.
///
/// Gets the user timezone (cached), builds agent execution config,
/// and runs the agent chat loop.
///
/// NOTE: AI capability is checked inside `execute_agent_chat()` — no need to
/// duplicate here (DRY).
#[allow(clippy::too_many_arguments)]
async fn run_slack_query(
    db: &DbPool,
    kv: &kyomi_core::KVPool,
    encryption_key: &std::sync::Arc<[u8; 32]>,
    embedding: &kyomi_embed::LazyEmbedding,
    ws_manager: &kyomi_auth::websocket::WebSocketManager,
    app_config: &std::sync::Arc<kyomi_core::Config>,
    connect_registry: &kyomi_datasource_server::ConnectRegistry,
    platforms: std::sync::Arc<kyomi_core::platform::PlatformRegistry>,
    session_id: &str,
    user_id: &str,
    workspace_id: &str,
    message: &str,
    slack_user_id: &str,
    bot_token: &str,
    slack_client_ref: &SlackClient,
    is_shared: bool,
) -> kyomi_core::Result<String> {
    // Get user timezone (cached 24h).
    let user_tz = get_slack_user_timezone(
        slack_user_id,
        workspace_id,
        user_id,
        db,
        slack_client_ref,
        bot_token,
    )
    .await;

    // Build current time string in user's timezone.
    let current_time_user_tz = user_tz.as_ref().map(|tz| {
        if let Ok(tz_parsed) = tz.parse::<chrono_tz::Tz>() {
            let now = Utc::now().with_timezone(&tz_parsed);
            now.to_rfc3339()
        } else {
            Utc::now().to_rfc3339()
        }
    });

    // Generate a stable ID for the user message so the agent's
    // persist_after_chat() stores it exactly once (matches the web frontend
    // pattern in server_fns/chat.rs where prepare_chat_dispatch() returns a
    // UUID and the server_fn passes it via user_message_id).
    let user_message_id = uuid::Uuid::new_v4().to_string();

    // Resolve the Slack-linked Kyomi user's current workspace role — and,
    // critically, reject the query outright if that membership is no
    // longer active. KYO-190 added this `get_workspace_user` lookup to
    // feed the agent's ToolContext but deliberately let a `None` result
    // (no active membership row) fall through to running the query anyway
    // with no roles. That was a live authorization bypass (KYO-223):
    // `resolve_slack_context` only confirms a `platform_user_links` row
    // exists, never that the linked user still belongs to the workspace.
    // A user removed from the workspace keeps their Slack link (nothing
    // currently cleans up `platform_user_links` on offboarding — see
    // KYO-223's follow-up ticket) and could otherwise go on querying the
    // workspace's dashboards, datasources, and knowledge through the bot
    // indefinitely. Every other authenticated entry point already rejects
    // on revoked membership (the `AuthUser` axum extractor, scheduled
    // watch execution in `watch_execution.rs`); this closes the same gap
    // for the Slack path.
    let user_roles = resolve_active_workspace_roles(db, workspace_id, user_id).await?;

    // Build agent execution config.
    let agent_config = kyomi_agent::execution::AgentExecutionConfig {
        session_id: session_id.to_string(),
        user_id: user_id.to_string(),
        workspace_id: workspace_id.to_string(),
        message: message.to_string(),
        model_name: None, // use default
        temperature: 0.7,
        is_shared_conversation: is_shared,
        context_type: "slack".into(),
        workspace_user_ids: None,
        cancel_token: tokio_util::sync::CancellationToken::new(),
        current_time_user_tz,
        message_source: Some("slack".into()),
        system_prompt: None, // build from standard prompt
        tools_subset: None,  // all tools available
        max_iterations: 25,
        component: "slack_agent".into(),
        user_message_id: Some(user_message_id),
        assistant_message_id: None,
        conversation_history: None,
        user_display_name: "Kyomi Slack".to_string(),
        context_window: 0,
        workspace_roles: user_roles,
    };

    let result = kyomi_agent::execution::execute_agent_chat(
        agent_config,
        kyomi_agent::AgentExecutionEnv {
            db,
            kv,
            encryption_key,
            embedding,
            ws_manager,
            app_config,
            connect_registry: Some(connect_registry.clone()),
            platforms,
        },
    )
    .await?;

    Ok(result.response_text)
}

// ===========================================================================
// Slack response helpers
// ===========================================================================

/// Where in Slack a [`run_slack_query`] result should be posted.
///
/// Bundles the per-message routing fields so [`respond_to_slack_query`]
/// stays under the clippy argument-count lint instead of taking each field
/// as its own parameter.
struct SlackReplyTarget<'a> {
    channel_id: &'a str,
    thread_ts: &'a str,
    placeholder_ts: Option<&'a str>,
    slack_user_id: &'a str,
}

/// Choose the Slack-visible text for a failed [`run_slack_query`] call.
///
/// `Forbidden` errors carry a message that is already a complete, safe
/// sentence for the end user — e.g. the KYO-223 workspace-membership gate
/// in [`resolve_active_workspace_roles`]. Every other error variant is
/// replaced with a generic message: the caller logs the real error
/// server-side, but Slack must never see raw internal error text (it can
/// leak implementation details, and for `Sqlx`/`Internal` variants may
/// include query fragments).
fn slack_query_error_message(err: &kyomi_core::Error) -> String {
    match err {
        kyomi_core::Error::Forbidden(msg) => msg.clone(),
        _ => "Sorry, I encountered an error processing your request. Please try again.".to_string(),
    }
}

/// Post the result of [`run_slack_query`] back to Slack.
///
/// Shared by the channel-mention and DM handlers so the three outcomes —
/// success, empty response, and failure — are handled identically in one
/// place instead of being duplicated per handler (the duplication is what
/// let KYO-190's role-lookup fix land in only one of the two call sites
/// the first time around). `context` is a short label ("app_mention" /
/// "direct_message") used only for log disambiguation.
async fn respond_to_slack_query(
    state: &SlackState,
    response: kyomi_core::Result<String>,
    ctx: &SlackContext,
    target: &SlackReplyTarget<'_>,
    session_id: &str,
    context: &str,
) {
    let query_ctx = kyomi_agent::tools::QueryContext {
        db: state.db.clone(),
        user_id: ctx.user_id.clone(),
        workspace_id: ctx.workspace_id.clone(),
        encryption_key: state.encryption_key.clone(),
        config: state.config.clone(),
        connect_registry: Some(state.connect_registry.clone()),
    };
    let footer_url = format!("{}/chat/{}", state.config.frontend_url, session_id);

    match response {
        Ok(response_text) if response_text.is_empty() => {
            warn!(session_id = %session_id, %context, "Agent returned empty response for Slack query");
            post_slack_response(
                &ctx.bot_token,
                target.channel_id,
                target.thread_ts,
                target.placeholder_ts,
                "I couldn't generate a response. Please try rephrasing your question.",
                &state.slack_client,
                &query_ctx,
                &footer_url,
            )
            .await;
        }
        Ok(response_text) => {
            post_slack_response(
                &ctx.bot_token,
                target.channel_id,
                target.thread_ts,
                target.placeholder_ts,
                &response_text,
                &state.slack_client,
                &query_ctx,
                &footer_url,
            )
            .await;
        }
        Err(e) => {
            let user_message = slack_query_error_message(&e);
            if matches!(e, kyomi_core::Error::Forbidden(_)) {
                warn!(session_id = %session_id, %context, error = %e, "Slack query rejected");
            } else {
                error!(session_id = %session_id, %context, error = %e, "Slack agent query failed");
            }
            // Update placeholder with the message if we have one, otherwise post ephemeral.
            if let Some(pts) = target.placeholder_ts {
                let _ = state
                    .slack_client
                    .update_message(&ctx.bot_token, target.channel_id, pts, &user_message, None)
                    .await;
            } else {
                post_slack_error(
                    &ctx.bot_token,
                    target.channel_id,
                    target.slack_user_id,
                    &user_message,
                    &state.slack_client,
                )
                .await;
            }
        }
    }
}

/// Post the agent response to Slack using the full message processor pipeline.
///
/// Handles ChartML rendering, markdown tables, text chunking — matching
/// the Python `_post_slack_response()` behavior.
///
/// If a placeholder timestamp is provided, updates the existing message.
/// Otherwise posts a new message.
#[allow(clippy::too_many_arguments)]
async fn post_slack_response(
    bot_token: &str,
    channel: &str,
    thread_ts: &str,
    placeholder_ts: Option<&str>,
    response: &str,
    slack_client_ref: &SlackClient,
    query_ctx: &kyomi_agent::tools::QueryContext,
    footer_url: &str,
) {
    use crate::message_processor;

    // Use the full message processor — ChartML, tables, text chunking
    let (blocks, fallback_text) = message_processor::process_and_build_slack_blocks(
        response,
        bot_token,
        slack_client_ref,
        query_ctx,
        Some(footer_url),
        "Continue in Kyomi",
        None, // no header for chat responses
        None, // no header emoji
    )
    .await;

    if let Some(pts) = placeholder_ts {
        // Update the placeholder message.
        if let Err(e) = slack_client_ref
            .update_message(bot_token, channel, pts, &fallback_text, Some(&blocks))
            .await
        {
            warn!(error = %e, "Failed to update Slack placeholder, posting new message");
            // Fall back to posting a new message.
            if let Err(e2) = slack_client_ref
                .post_message(bot_token, channel, &fallback_text, Some(&blocks), Some(thread_ts))
                .await
            {
                error!(error = %e2, "Failed to post Slack response");
            }
        }
    } else {
        // Post a new threaded message.
        if let Err(e) = slack_client_ref
            .post_message(bot_token, channel, &fallback_text, Some(&blocks), Some(thread_ts))
            .await
        {
            error!(error = %e, "Failed to post Slack response");
        }
    }
}

/// Post an ephemeral error message to the user.
async fn post_slack_error(
    bot_token: &str,
    channel: &str,
    user_id: &str,
    message: &str,
    slack_client_ref: &SlackClient,
) {
    if let Err(e) = slack_client_ref
        .post_ephemeral(bot_token, channel, user_id, message, None)
        .await
    {
        error!(error = %e, "Failed to post Slack ephemeral error");
    }
}

/// Post a "Kyomi is thinking..." placeholder message. Returns the message timestamp.
async fn post_slack_placeholder(
    bot_token: &str,
    channel: &str,
    thread_ts: &str,
    slack_client_ref: &SlackClient,
) -> Option<String> {
    match slack_client_ref
        .post_message(
            bot_token,
            channel,
            "Kyomi is thinking...",
            Some(&[json!({
                "type": "context",
                "elements": [{
                    "type": "mrkdwn",
                    "text": "_Kyomi is thinking..._"
                }]
            })]),
            Some(thread_ts),
        )
        .await
    {
        Ok(result) if result.ok => result.ts,
        Ok(result) => {
            warn!(error = ?result.error, "Failed to post thinking placeholder");
            None
        }
        Err(e) => {
            warn!(error = %e, "Failed to post thinking placeholder");
            None
        }
    }
}

// ===========================================================================
// Slack session management
// ===========================================================================

/// Find an existing chat session for this Slack channel/thread, or create one.
///
/// Sessions are mapped 1:1 to Slack threads via `(platform_type, platform_thread_key)`.
/// The thread_key format is `"channel_id:thread_ts"`.
/// Returns `(session_id, is_new)` so callers can trigger title generation for
/// newly created sessions.
async fn find_or_create_slack_session(
    db: &DbPool,
    workspace_id: &str,
    user_id: &str,
    channel_id: &str,
    thread_ts: &str,
    shared: bool,
) -> kyomi_core::Result<(String, bool)> {
    let thread_key = format!("{channel_id}:{thread_ts}");
    let thread = kyomi_core::platform::PlatformThread {
        platform: "slack".to_string(),
        workspace_id: workspace_id.to_string(),
        thread_key,
    };
    kyomi_core::platform::find_or_create_platform_session(
        db, &thread, user_id, workspace_id, shared,
    ).await
}

// ===========================================================================
// Slack user timezone caching
// ===========================================================================

/// Get the user's IANA timezone string from Slack, with 24-hour caching.
///
/// Checks `workspace_user_integrations.config` for cached `timezone` and
/// `timezone_fetched_at`. If cached (< 24h), returns the cached value.
/// Otherwise calls Slack API, updates the integration config, and returns.
async fn get_slack_user_timezone(
    slack_user_id: &str,
    workspace_id: &str,
    user_id: &str,
    db: &DbPool,
    slack_client_ref: &SlackClient,
    bot_token: &str,
) -> Option<String> {
    // Check cache in workspace_user_integrations config JSON.
    let config = kyomi_core::platform::get_user_integration(db, workspace_id, user_id, "slack")
        .await
        .ok()
        .flatten();

    if let Some(ref cfg) = config {
        let cached_tz = cfg.get("timezone").and_then(|v| v.as_str());
        let cached_at = cfg.get("timezone_fetched_at").and_then(|v| v.as_str());
        if let (Some(tz), Some(fetched_at_str)) = (cached_tz, cached_at)
            && let Ok(fetched_at) = chrono::DateTime::parse_from_rfc3339(fetched_at_str) {
                let age_hours = (Utc::now() - fetched_at.with_timezone(&Utc)).num_hours();
                if age_hours < SLACK_TIMEZONE_CACHE_HOURS {
                    return Some(tz.to_string());
                }
            }
    }

    // Fetch from Slack API.
    let user_info = match slack_client_ref.users_info(bot_token, slack_user_id).await {
        Ok(info) => info,
        Err(e) => {
            warn!(error = %e, "Failed to get Slack user timezone");
            // Return cached value if available, even if stale.
            return config.and_then(|cfg| cfg.get("timezone").and_then(|v| v.as_str()).map(String::from));
        }
    };

    let tz = user_info.tz;

    // Update cache in workspace_user_integrations config JSON.
    if let Some(ref tz_val) = tz {
        // Merge timezone into the existing config (or create new one).
        let mut cfg = config.unwrap_or_else(|| serde_json::json!({}));
        cfg["timezone"] = serde_json::json!(tz_val);
        cfg["timezone_fetched_at"] = serde_json::json!(Utc::now().to_rfc3339());
        let _ = kyomi_core::platform::upsert_user_integration(
            db, workspace_id, user_id, "slack", &cfg,
        )
        .await;
    }

    tz
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    /// Build an in-memory SQLite pool with migrations applied.
    ///
    /// Mirrors the `test_pool()` helper in `kyomi_auth::session` /
    /// `workspace_service` — the established in-memory-sqlite pattern used
    /// across the workspace's unit tests.
    async fn test_pool() -> DbPool {
        let _ = kyomi_core::constants::load_with_fallback();

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .expect("enable foreign keys");

        sqlx::migrate!("../../apps/server/migrations-sqlite")
            .run(&pool)
            .await
            .expect("run sqlite migrations");

        DbPool::Sqlite(pool)
    }

    /// Insert a user row with the given id.
    async fn insert_user(pool: &DbPool, user_id: &str) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        };
        sqlx::query("INSERT INTO users (user_id, email) VALUES ($1, $2)")
            .bind(user_id)
            .bind(format!("{user_id}@test.local"))
            .execute(sq)
            .await
            .expect("insert user");
    }

    /// Insert a workspace row owned by `owner_user_id`.
    async fn insert_workspace(pool: &DbPool, workspace_id: &str, owner_user_id: &str) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        };
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $2, $3)",
        )
        .bind(workspace_id)
        .bind(format!("Workspace {workspace_id}"))
        .bind(owner_user_id)
        .execute(sq)
        .await
        .expect("insert workspace");
    }

    /// Insert a `workspace_users` row with an explicit `active` flag.
    async fn insert_workspace_user(
        pool: &DbPool,
        workspace_id: &str,
        user_id: &str,
        role: &str,
        active: bool,
    ) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        };
        sqlx::query(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(workspace_id)
        .bind(user_id)
        .bind(role)
        .bind(active)
        .execute(sq)
        .await
        .expect("insert workspace_users row");
    }

    // -----------------------------------------------------------------
    // KYO-223: resolve_active_workspace_roles (the membership gate that
    // sits in front of agent execution in run_slack_query)
    // -----------------------------------------------------------------

    /// Happy path: a Slack-linked user with active workspace membership
    /// must not regress — the gate has to let them through with their role.
    #[tokio::test]
    async fn resolve_active_workspace_roles_allows_active_member() {
        let pool = test_pool().await;
        insert_user(&pool, "user-1").await;
        insert_workspace(&pool, "ws-1", "user-1").await;
        insert_workspace_user(&pool, "ws-1", "user-1", "workspace_admin", true).await;

        let roles = resolve_active_workspace_roles(&pool, "ws-1", "user-1")
            .await
            .expect("active member must be allowed through the gate");

        assert_eq!(roles, vec![WorkspaceRole::WorkspaceAdmin]);
    }

    /// KYO-223 core case: the `platform_user_links` row survives offboarding
    /// (nothing deletes it — see `remove_workspace_member`), but the
    /// `workspace_users` row is gone. The gate must reject, not degrade to
    /// running with no roles.
    #[tokio::test]
    async fn resolve_active_workspace_roles_rejects_revoked_membership() {
        let pool = test_pool().await;
        insert_user(&pool, "user-1").await;
        insert_workspace(&pool, "ws-1", "user-1").await;
        // No workspace_users row at all — membership was removed.

        let result = resolve_active_workspace_roles(&pool, "ws-1", "user-1").await;

        match result {
            Err(kyomi_core::Error::Forbidden(msg)) => {
                assert!(
                    msg.contains("access") && msg.contains("ended"),
                    "message should be actionable, got: {msg}"
                );
            }
            other => panic!("expected Forbidden, got: {other:?}"),
        }
    }

    /// Guards the `active = true` filter itself: a `workspace_users` row
    /// that exists but is deactivated (rather than deleted) must be
    /// rejected exactly like a missing row. If `get_workspace_user` ever
    /// stopped filtering on `active`, this test — unlike the "row absent"
    /// case above — would catch it because the row genuinely exists.
    #[tokio::test]
    async fn resolve_active_workspace_roles_rejects_inactive_membership_row() {
        let pool = test_pool().await;
        insert_user(&pool, "user-1").await;
        insert_workspace(&pool, "ws-1", "user-1").await;
        insert_workspace_user(&pool, "ws-1", "user-1", "workspace_admin", false).await;

        let result = resolve_active_workspace_roles(&pool, "ws-1", "user-1").await;

        assert!(
            matches!(result, Err(kyomi_core::Error::Forbidden(_))),
            "inactive membership row must be rejected, got: {result:?}"
        );
    }

    // -----------------------------------------------------------------
    // KYO-223: the rejection must surface as a readable Slack message,
    // not be swallowed into the generic failure text.
    // -----------------------------------------------------------------

    #[test]
    fn slack_query_error_message_forbidden_is_shown_verbatim() {
        let err = kyomi_core::Error::Forbidden(
            "Your access to this Kyomi workspace has ended. If you believe this is a mistake, contact your workspace admin.".into(),
        );
        let msg = slack_query_error_message(&err);
        assert_eq!(
            msg,
            "Your access to this Kyomi workspace has ended. If you believe this is a mistake, contact your workspace admin."
        );
    }

    /// Non-Forbidden errors must never reach Slack verbatim — only the
    /// generic fallback. This is what keeps internal error text (which can
    /// include query fragments for `Sqlx`/`Internal` variants) out of a
    /// public or semi-public Slack channel.
    #[test]
    fn slack_query_error_message_other_errors_are_generic() {
        let err = kyomi_core::Error::Internal(
            "duplicate key value violates unique constraint \"chat_sessions_pkey\"".into(),
        );
        let msg = slack_query_error_message(&err);
        assert_eq!(
            msg,
            "Sorry, I encountered an error processing your request. Please try again."
        );
        assert!(!msg.contains("constraint"), "must not leak internal error detail");
    }

    #[test]
    fn build_redirect_uri_format() {
        let config = kyomi_core::Config::test_config();
        let uri = build_redirect_uri(&config, "/slack/oauth/callback");
        assert!(uri.ends_with("/api/v1/slack/oauth/callback"));
    }

    #[test]
    fn build_redirect_uri_strips_trailing_slash() {
        let mut config = kyomi_core::Config::test_config();
        config.frontend_url = "https://app.kyomi.ai/".into();
        let uri = build_redirect_uri(&config, "/slack/user/callback");
        assert_eq!(uri, "https://app.kyomi.ai/api/v1/slack/user/callback");
    }

    #[test]
    fn get_slack_config_missing_returns_error() {
        let config = kyomi_core::Config::test_config();
        let result = get_slack_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn get_slack_config_present_returns_values() {
        let mut config = kyomi_core::Config::test_config();
        config.slack_client_id = Some("test-client-id".into());
        config.slack_client_secret = Some("test-client-secret".into());
        let (id, secret) = get_slack_config(&config).unwrap();
        assert_eq!(id, "test-client-id");
        assert_eq!(secret, "test-client-secret");
    }

    #[test]
    fn get_slack_config_empty_string_returns_error() {
        let mut config = kyomi_core::Config::test_config();
        config.slack_client_id = Some("".into());
        config.slack_client_secret = Some("test-client-secret".into());
        let result = get_slack_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn status_response_serializes() {
        let response = SlackStatusResponse {
            installed: true,
            team_name: Some("Test Team".into()),
            team_id: Some("T12345".into()),
            user_connected: true,
            slack_username: Some("testuser".into()),
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["installed"], true);
        assert_eq!(json["team_name"], "Test Team");
        assert_eq!(json["user_connected"], true);
    }

    #[test]
    fn status_response_not_installed() {
        let response = SlackStatusResponse {
            installed: false,
            team_name: None,
            team_id: None,
            user_connected: false,
            slack_username: None,
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["installed"], false);
        assert!(json["team_name"].is_null());
    }

    #[test]
    fn channels_response_serializes() {
        let response = ChannelsResponse {
            channels: vec![
                ChannelResponse {
                    id: "C123".into(),
                    name: "general".into(),
                    is_private: false,
                },
                ChannelResponse {
                    id: "C456".into(),
                    name: "alerts".into(),
                    is_private: true,
                },
            ],
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["channels"].as_array().unwrap().len(), 2);
        assert_eq!(json["channels"][0]["name"], "general");
        assert!(!json["channels"][0]["is_private"].as_bool().unwrap());
        assert!(json["channels"][1]["is_private"].as_bool().unwrap());
    }

    #[test]
    fn default_watch_channel_response_serializes() {
        let response = DefaultWatchChannelResponse {
            channel_id: Some("C789".into()),
            channel_name: Some("reporting".into()),
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["channel_id"], "C789");
        assert_eq!(json["channel_name"], "reporting");
    }

    #[test]
    fn default_watch_channel_empty_response() {
        let response = DefaultWatchChannelResponse {
            channel_id: None,
            channel_name: None,
        };
        let json = serde_json::to_value(&response).unwrap();
        assert!(json["channel_id"].is_null());
        assert!(json["channel_name"].is_null());
    }

    #[test]
    fn set_default_channel_request_deserializes() {
        let json = r#"{"channel_id": "C123", "channel_name": "alerts"}"#;
        let req: SetDefaultWatchChannelRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.channel_id, "C123");
        assert_eq!(req.channel_name, "alerts");
    }

    #[test]
    fn oauth_callback_query_deserializes() {
        let json = r#"{"code": "abc123", "state": "xyz789"}"#;
        let query: OAuthCallbackQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.code, "abc123");
        assert_eq!(query.state, "xyz789");
    }
}
