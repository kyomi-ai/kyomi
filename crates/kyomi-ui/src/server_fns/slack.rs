// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the Slack connection section of Profile settings.
//!
//! These replace the REST API calls that ProfileSettings.jsx (lines 160-300)
//! makes to `/api/v1/slack/*` endpoints. Each function calls the same
//! service-layer code (kyomi_core::platform, kyomi_auth) as the existing
//! REST route handlers in `enterprise/kyomi-slack/src/routes.rs`.
//!
//! All functions are gated behind `#[cfg(feature = "slack")]` at the module
//! level (in `mod.rs`).

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Types (shared between server and client via serde)
// ─────────────────────────────────────────────────────────────────────────────

/// Slack integration status for the current user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackStatus {
    /// Whether the Kyomi Slack app is installed in the workspace.
    pub workspace_connected: bool,
    /// Whether the current user has linked their Slack account.
    pub user_connected: bool,
    /// The user's Slack display name (if connected).
    pub slack_username: Option<String>,
    /// The Slack workspace/team name (if installed).
    pub slack_team_name: Option<String>,
}

/// A Slack channel available for watch alerts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackChannel {
    pub channel_id: String,
    pub channel_name: String,
    pub is_private: bool,
}

/// The user's default watch channel setting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchChannel {
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Server functions
// ─────────────────────────────────────────────────────────────────────────────

/// Get the Slack integration status for the current user.
///
/// Checks whether:
/// 1. The workspace has the Slack app installed (workspace_integrations table)
/// 2. The user has linked their Slack account (platform_user_links table)
#[server(prefix = "/leptos-api")]
pub async fn get_slack_status() -> Result<SlackStatus, ServerFnError> {
    use super::{AuthenticatedContext, IntoServerFnError};

    let ac = AuthenticatedContext::extract().await?;

    // Check workspace Slack installation via platform tables
    let ws_config =
        kyomi_core::platform::get_workspace_integration(ac.db(), &ac.ws_id, "slack")
            .await
            .into_sfn()?;

    let installed = ws_config.is_some();
    let slack_team_name = ws_config
        .as_ref()
        .and_then(|cfg| cfg.get("team_name"))
        .and_then(|v| v.as_str())
        .map(String::from);

    // Check if user is connected via platform_user_links
    let mut user_connected = false;
    let mut slack_username: Option<String> = None;

    if installed {
        let link = kyomi_core::platform::get_platform_user_link(
            ac.db(), &ac.ws_id, &ac.auth.user_id, "slack",
        )
        .await
        .into_sfn()?;

        if let Some(link) = link {
            user_connected = true;
            slack_username = link.platform_username;
        }
    }

    Ok(SlackStatus {
        workspace_connected: installed,
        user_connected,
        slack_username,
        slack_team_name,
    })
}

/// Start the Slack OAuth flow for user account linking.
///
/// Generates a CSRF state token, stores it in KV, and returns the
/// Slack OAuth authorization URL. The frontend redirects the user to
/// this URL to complete the OAuth flow.
#[cfg(feature = "slack")]
#[server(prefix = "/leptos-api")]
pub async fn slack_connect() -> Result<String, ServerFnError> {
    use super::{extract_auth, extract_context};

    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    // Verify Slack is configured
    let client_id = ctx
        .config
        .slack_client_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ServerFnError::new("Slack integration not configured"))?;

    // KV store is required for OAuth state
    let kv = ctx
        .kv
        .as_ref()
        .ok_or_else(|| ServerFnError::new("OAuth state store not available"))?;

    // Generate CSRF state token and store in KV
    let oauth_state = kyomi_auth::redis_ops::generate_token();

    kyomi_auth::redis_ops::store_oauth_state(
        kv,
        "slack_user_connect",
        &oauth_state,
        &serde_json::json!({
            "user_id": auth.user_id,
            "created_at": chrono::Utc::now().to_rfc3339(),
        }),
    )
    .await
    // user_message() (KYO-448) — Display would leak the variant tag
    // (a Redis outage surfaces as kyomi_core::Error::Internal).
    .map_err(|e| ServerFnError::new(format!("Failed to store OAuth state: {}", e.user_message())))?;

    // Build redirect URI (same path as the REST route handler)
    let base = ctx.config.frontend_url.trim_end_matches('/');
    let redirect_uri = format!("{base}/api/v1/slack/user/callback");

    let auth_url = format!(
        "{}?client_id={}&user_scope={}&redirect_uri={}&state={}",
        kyomi_slack::client::OAUTH_AUTHORIZE_URL,
        client_id,
        kyomi_slack::client::SLACK_USER_SCOPES,
        redirect_uri,
        oauth_state,
    );

    Ok(auth_url)
}

#[cfg(not(feature = "slack"))]
#[server(prefix = "/leptos-api")]
pub async fn slack_connect() -> Result<String, ServerFnError> {
    Err(ServerFnError::new("Slack integration not available"))
}

/// Disconnect the current user's Slack account.
///
/// Removes the user integration and platform user link from the database.
#[server(prefix = "/leptos-api")]
pub async fn slack_disconnect() -> Result<(), ServerFnError> {
    use super::{AuthenticatedContext, IntoServerFnError};

    let ac = AuthenticatedContext::extract().await?;

    // Verify user has a Slack connection
    let user_integration =
        kyomi_core::platform::get_user_integration(ac.db(), &ac.ws_id, &ac.auth.user_id, "slack")
            .await
            .into_sfn()?;

    if user_integration.is_none() {
        return Err(ServerFnError::new("No Slack connection found"));
    }

    // Remove user integration
    kyomi_core::platform::delete_user_integration(ac.db(), &ac.ws_id, &ac.auth.user_id, "slack")
        .await
        .into_sfn()?;

    // Remove platform user link
    kyomi_core::platform::delete_platform_user_link(ac.db(), &ac.ws_id, &ac.auth.user_id, "slack")
        .await
        .into_sfn()?;

    Ok(())
}

/// List available Slack channels for watch alert configuration.
///
/// Requires:
/// - Workspace has Slack installed (bot token in workspace_integrations)
/// - User has linked their Slack account (platform_user_links)
/// - SlackClient is available in server context
#[cfg(feature = "slack")]
#[server(prefix = "/leptos-api")]
pub async fn get_slack_channels() -> Result<Vec<SlackChannel>, ServerFnError> {
    use super::{AuthenticatedContext, IntoServerFnError};

    let ac = AuthenticatedContext::extract().await?;

    let encryption_key = ac.encryption_key()?;

    let slack_client = ac
        .ctx
        .slack_client
        .as_ref()
        .ok_or_else(|| ServerFnError::new("Slack client not available"))?;

    // Get the decrypted bot token from workspace integration config
    let bot_token = kyomi_slack::routes::get_slack_bot_token(ac.db(), &encryption_key, &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| {
            ServerFnError::new("Kyomi app not installed in your Slack workspace.")
        })?;

    // Verify user is connected to Slack
    let has_link =
        kyomi_core::platform::get_platform_user_link(ac.db(), &ac.ws_id, &ac.auth.user_id, "slack")
            .await
            .into_sfn()?;

    if has_link.is_none() {
        return Err(ServerFnError::new(
            "Connect your Slack account first to see available channels.",
        ));
    }

    // Fetch channels from Slack API via the SlackClient
    let slack_channels = slack_client
        .conversations_list(&bot_token)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to fetch Slack channels: {e}")))?;

    Ok(slack_channels
        .into_iter()
        .map(|ch| SlackChannel {
            channel_id: ch.id,
            channel_name: ch.name,
            is_private: ch.is_private,
        })
        .collect())
}

#[cfg(not(feature = "slack"))]
#[server(prefix = "/leptos-api")]
pub async fn get_slack_channels() -> Result<Vec<SlackChannel>, ServerFnError> {
    Err(ServerFnError::new("Slack integration not available"))
}

/// Get the user's default watch channel setting.
///
/// Reads from the user's Slack integration config in workspace_user_integrations.
#[server(prefix = "/leptos-api")]
pub async fn get_default_watch_channel() -> Result<WatchChannel, ServerFnError> {
    use super::{AuthenticatedContext, IntoServerFnError};

    let ac = AuthenticatedContext::extract().await?;

    let user_config =
        kyomi_core::platform::get_user_integration(ac.db(), &ac.ws_id, &ac.auth.user_id, "slack")
            .await
            .into_sfn()?;

    let (channel_id, channel_name) = match user_config {
        Some(cfg) => {
            let id = cfg
                .get("default_channel_id")
                .and_then(|v| v.as_str())
                .map(String::from);
            let name = cfg
                .get("default_channel_name")
                .and_then(|v| v.as_str())
                .map(String::from);
            (id, name)
        }
        None => (None, None),
    };

    Ok(WatchChannel {
        channel_id,
        channel_name,
    })
}

/// Set the user's default watch channel for new Slack alerts.
///
/// Pass `None` for both fields to clear the default channel.
/// Updates the user's Slack integration config in workspace_user_integrations.
#[server(prefix = "/leptos-api")]
pub async fn set_default_watch_channel(
    channel_id: Option<String>,
    channel_name: Option<String>,
) -> Result<WatchChannel, ServerFnError> {
    use super::{AuthenticatedContext, IntoServerFnError};

    // Validate: both must be Some or both must be None (no partial clear)
    match (&channel_id, &channel_name) {
        (Some(_), None) | (None, Some(_)) => {
            return Err(ServerFnError::new(
                "Both channel_id and channel_name must be provided together, or both omitted to clear.",
            ));
        }
        _ => {}
    }

    let ac = AuthenticatedContext::extract().await?;

    // Get existing user integration config
    let user_config =
        kyomi_core::platform::get_user_integration(ac.db(), &ac.ws_id, &ac.auth.user_id, "slack")
            .await
            .into_sfn()?;

    let mut config = user_config
        .ok_or_else(|| ServerFnError::new("Connect your Slack account first."))?;

    // Merge default channel into the existing config
    if let Some(obj) = config.as_object_mut() {
        match (&channel_id, &channel_name) {
            (Some(id), Some(name)) => {
                obj.insert(
                    "default_channel_id".to_string(),
                    serde_json::Value::String(id.clone()),
                );
                obj.insert(
                    "default_channel_name".to_string(),
                    serde_json::Value::String(name.clone()),
                );
            }
            _ => {
                // Clear the default channel
                obj.insert(
                    "default_channel_id".to_string(),
                    serde_json::Value::Null,
                );
                obj.insert(
                    "default_channel_name".to_string(),
                    serde_json::Value::Null,
                );
            }
        }
    }

    kyomi_core::platform::upsert_user_integration(ac.db(), &ac.ws_id, &ac.auth.user_id, "slack", &config)
        .await
        .into_sfn()?;

    Ok(WatchChannel {
        channel_id,
        channel_name,
    })
}

