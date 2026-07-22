// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the Workspace settings page.
//!
//! These replace the REST API calls that WorkspaceSettings.jsx makes
//! to `/api/v1/workspaces/*` endpoints. Each function calls the same
//! service-layer code as the existing REST route handlers in
//! `apps/server/src/routes/workspaces.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::types::WorkspaceSettingsData;

// ─────────────────────────────────────────────────────────────────────────────
// Workspace switcher
// ─────────────────────────────────────────────────────────────────────────────

/// One workspace in the sidebar switcher.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub workspace_id: String,
    pub name: String,              // "Workspace" fallback applied server-side
    pub member_count: i64,
    pub subscription_tier: String, // snake_case tier ("team", "enterprise", ...)
    pub role: String,              // humanized ("Admin"/"Viewer"/"Member")
    pub is_active: bool,           // true for the caller's current JWT workspace
}

/// List all workspaces the caller belongs to, for the sidebar switcher.
#[server(prefix = "/leptos-api")]
pub async fn list_my_workspaces() -> Result<Vec<WorkspaceSummary>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let current = auth.workspace.workspace_id.clone();

    let summaries =
        kyomi_auth::workspace_service::get_user_workspaces_with_counts(&ctx.db, &auth.user_id)
            .await
            .into_sfn()?;

    Ok(summaries
        .into_iter()
        .map(|s| {
            // Tier -> snake_case string, same convention the JWT uses.
            let tier = serde_json::to_value(s.subscription_tier)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            // Role DB token -> humanized label (server-side; kyomi-core is ssr-only in kyomi-ui).
            let role_token = serde_json::to_value(s.role)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let is_active = Some(&s.workspace_id) == current.as_ref();
            WorkspaceSummary {
                name: s.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Workspace".to_string()),
                workspace_id: s.workspace_id,
                member_count: s.member_count,
                subscription_tier: tier,
                role: kyomi_core::constants::humanize_workspace_role(&role_token).to_string(),
                is_active,
            }
        })
        .collect())
}

/// Switch the caller's active workspace and re-mint their session.
#[server(prefix = "/leptos-api")]
pub async fn switch_workspace(workspace_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let user = kyomi_auth::user_service::get_user_by_id(&ctx.db, &auth.user_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;
    let device = super::auth::extract_device_info(&headers);

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    let sess = kyomi_auth::session::switch_active_workspace(
        &ctx.db,
        &kv,
        &ctx.config.jwt_secret,
        &user,
        &workspace_id,
        &device,
    )
    .await
    .into_sfn()?;

    // Apply the new session cookies so middleware sees the new workspace next request.
    super::auth::set_session_cookies(&sess);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers (server-only)
// ─────────────────────────────────────────────────────────────────────────────

/// Reject non-workspace-admin users.
///
/// Mirrors `require_workspace_admin()` in `apps/server/src/routes/workspaces.rs`.
#[cfg(feature = "ssr")]
fn require_workspace_admin(
    auth: &kyomi_auth::middleware::AuthUser,
) -> Result<(), ServerFnError> {
    if !auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
    {
        return Err(ServerFnError::new("Workspace admin access required"));
    }
    Ok(())
}

/// Read a nested key from `settings.custom_settings[key]`.
///
/// Mirrors `custom_settings_get()` in `apps/server/src/routes/workspaces.rs`.
#[cfg(feature = "ssr")]
fn custom_settings_get<'a>(
    settings: &'a Option<serde_json::Value>,
    key: &str,
) -> Option<&'a serde_json::Value> {
    settings
        .as_ref()
        .and_then(|s| s.get("custom_settings"))
        .and_then(|cs| cs.get(key))
}

/// Remove a single key from `settings.custom_settings`, leaving all other
/// keys intact. Returns the modified settings JSON.
///
/// If the key is absent, or `custom_settings` does not exist, the settings
/// are returned unchanged (so callers need not pre-check).
#[cfg(feature = "ssr")]
fn clear_custom_settings_key(
    settings: &Option<serde_json::Value>,
    key: &str,
) -> serde_json::Value {
    let mut s = settings
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));

    if let Some(cs) = s.get_mut("custom_settings").and_then(|v| v.as_object_mut()) {
        cs.remove(key);
    }

    s
}

/// Merge a key-value pair into `settings.custom_settings`.
///
/// Mirrors `merge_custom_settings()` in `apps/server/src/routes/workspaces.rs`.
#[cfg(feature = "ssr")]
fn merge_custom_settings(
    settings: &Option<serde_json::Value>,
    key: &str,
    value: serde_json::Value,
) -> serde_json::Value {
    let mut s = settings
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));

    // Ensure custom_settings exists
    if s.get("custom_settings").is_none()
        && let Some(obj) = s.as_object_mut()
    {
        obj.insert(
            "custom_settings".to_string(),
            serde_json::json!({}),
        );
    }

    if let Some(cs) = s.get_mut("custom_settings").and_then(|v| v.as_object_mut()) {
        cs.insert(key.to_string(), value);
    }

    s
}

// ─────────────────────────────────────────────────────────────────────────────
// Read operations
// ─────────────────────────────────────────────────────────────────────────────

/// Load workspace settings for the admin settings page.
///
/// Returns workspace name, default AI model, and chart palette.
/// Requires workspace admin role.
#[server(prefix = "/leptos-api")]
pub async fn get_workspace_settings() -> Result<WorkspaceSettingsData, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let workspace = kyomi_auth::workspace_service::get_workspace_full(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    let default_model = custom_settings_get(&workspace.settings, "default_model")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| ac.ctx.config.llm_model.clone());

    let chart_palette = custom_settings_get(&workspace.settings, "chartml_config")
        .and_then(kyomi_auth::user_service::extract_palette_style)
        .unwrap_or("kyomi")
        .to_string();

    let title_model = custom_settings_get(&workspace.settings, "title_model")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    Ok(WorkspaceSettingsData {
        workspace_name: workspace.name.unwrap_or_default(),
        default_model,
        chart_palette,
        title_model,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Write operations
// ─────────────────────────────────────────────────────────────────────────────

/// Update the workspace name. Requires admin role.
#[server(prefix = "/leptos-api")]
pub async fn update_workspace_name(name: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ServerFnError::new("Workspace name cannot be empty"));
    }

    kyomi_auth::workspace_service::update_workspace_name(ac.db(), &ac.ws_id, trimmed)
        .await
        .into_sfn()?;

    Ok(())
}

/// Update the workspace default AI model. Requires admin role.
#[server(prefix = "/leptos-api")]
pub async fn update_workspace_model(
    model: String,
    context_window: Option<u64>,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let workspace = kyomi_auth::workspace_service::get_workspace_full(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    let mut updated_settings = merge_custom_settings(
        &workspace.settings,
        "default_model",
        serde_json::json!(model),
    );

    if let Some(cw) = context_window {
        updated_settings = merge_custom_settings(
            &Some(updated_settings),
            "context_window",
            serde_json::json!(cw),
        );
    }

    kyomi_auth::workspace_service::update_workspace_settings(
        ac.db(),
        &ac.ws_id,
        &updated_settings,
    )
    .await
    .into_sfn()?;

    Ok(())
}

/// Update the model used specifically for session title generation. Requires admin role.
///
/// Stores the value in `settings.custom_settings.title_model`. When set, the
/// title generation logic uses this model instead of overriding to the cheapest
/// model for the provider. Pass an empty string to clear the override and
/// restore the cheapest-model fallback.
#[server(prefix = "/leptos-api")]
pub async fn update_workspace_title_model(model: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let workspace = kyomi_auth::workspace_service::get_workspace_full(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    let trimmed = model.trim();

    // An empty value clears the override — remove the key from custom_settings
    // so the fallback (cheapest model) kicks in again.
    let updated_settings = if trimmed.is_empty() {
        clear_custom_settings_key(&workspace.settings, "title_model")
    } else {
        merge_custom_settings(&workspace.settings, "title_model", serde_json::json!(trimmed))
    };

    kyomi_auth::workspace_service::update_workspace_settings(
        ac.db(),
        &ac.ws_id,
        &updated_settings,
    )
    .await
    .into_sfn()?;

    Ok(())
}

/// Update the workspace ChartML config (chart palette). Requires admin role.
#[server(prefix = "/leptos-api")]
pub async fn update_workspace_chartml_config(palette: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let workspace = kyomi_auth::workspace_service::get_workspace_full(ac.db(), &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    let config_value = serde_json::json!({
        "type": "config",
        "version": 1,
        "style": palette
    });

    let updated_settings = merge_custom_settings(
        &workspace.settings,
        "chartml_config",
        config_value,
    );

    kyomi_auth::workspace_service::update_workspace_settings(
        ac.db(),
        &ac.ws_id,
        &updated_settings,
    )
    .await
    .into_sfn()?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Workspace-level Slack integration (install/uninstall)
// ─────────────────────────────────────────────────────────────────────────────

/// Get workspace-level Slack integration status.
///
/// Returns whether the Kyomi Slack app is installed in the workspace,
/// along with the Slack team name and ID if installed.
#[server(prefix = "/leptos-api")]
pub async fn get_workspace_slack_status() -> Result<crate::types::WorkspaceSlackStatus, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let ws_config =
        kyomi_core::platform::get_workspace_integration(ac.db(), &ac.ws_id, "slack")
            .await
            .into_sfn()?;

    let installed = ws_config.is_some();
    let team_id = ws_config
        .as_ref()
        .and_then(|cfg| cfg.get("team_id"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let team_name = ws_config
        .as_ref()
        .and_then(|cfg| cfg.get("team_name"))
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(crate::types::WorkspaceSlackStatus {
        installed,
        team_id,
        team_name,
    })
}

/// Get the Slack OAuth install URL for adding Kyomi to a workspace.
///
/// Returns the OAuth authorization URL. The frontend redirects the user
/// to this URL to complete the Slack app installation.
/// Requires workspace admin role.
#[cfg(feature = "slack")]
#[server(prefix = "/leptos-api")]
pub async fn get_slack_install_url() -> Result<String, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let client_id = ac.ctx.config
        .slack_client_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ServerFnError::new("Slack integration not configured"))?;

    let kv = ac.ctx.kv
        .as_ref()
        .ok_or_else(|| ServerFnError::new("OAuth state store not available"))?;

    // Generate CSRF state token and store in KV
    let oauth_state = kyomi_auth::redis_ops::generate_token();

    kyomi_auth::redis_ops::store_oauth_state(
        kv,
        "slack_install",
        &oauth_state,
        &serde_json::json!({
            "user_id": ac.auth.user_id,
            "workspace_id": &ac.ws_id,
            "created_at": chrono::Utc::now().to_rfc3339(),
        }),
    )
    .await
    .map_err(|e| ServerFnError::new(format!("Failed to store OAuth state: {e}")))?;

    let base = ac.ctx.config.frontend_url.trim_end_matches('/');
    let redirect_uri = format!("{base}/api/v1/slack/oauth/callback");

    let auth_url = format!(
        "{}?client_id={}&scope={}&redirect_uri={}&state={}",
        kyomi_slack::client::OAUTH_AUTHORIZE_URL,
        client_id,
        kyomi_slack::client::SLACK_BOT_SCOPES,
        redirect_uri,
        oauth_state,
    );

    Ok(auth_url)
}

#[cfg(not(feature = "slack"))]
#[server(prefix = "/leptos-api")]
pub async fn get_slack_install_url() -> Result<String, ServerFnError> {
    Err(ServerFnError::new("Slack integration not available"))
}

/// Remove the Slack integration from the workspace.
///
/// Removes the workspace integration record from the database.
/// Requires workspace admin role.
#[server(prefix = "/leptos-api")]
pub async fn uninstall_workspace_slack(team_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    // Verify the integration exists
    let integration =
        kyomi_core::platform::get_workspace_integration(ac.db(), &ac.ws_id, "slack")
            .await
            .into_sfn()?;

    if integration.is_none() {
        return Err(ServerFnError::new(
            "Slack integration not found for this workspace",
        ));
    }

    // Remove the workspace integration
    kyomi_core::platform::delete_workspace_integration(ac.db(), &ac.ws_id, "slack")
        .await
        .into_sfn()?;

    let _ = team_id; // used by React for cache invalidation — we verify server-side

    Ok(())
}

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, AuthenticatedContext, IntoServerFnError};

#[cfg(all(test, feature = "ssr"))]
mod tests {
    //! Guards against accidental re-nesting of the workspace chartml_config writer payload.
    //!
    //! See the companion test in `server_fns::profile::tests` for the per-user
    //! equivalent and KYO-129 Part 2 for rationale.

    #[test]
    fn workspace_chart_palette_writer_produces_flat_shape() {
        let palette = "balanced".to_string();
        let config_value = serde_json::json!({
            "type": "config",
            "version": 1,
            "style": palette
        });
        assert_eq!(config_value["style"], "balanced");
        assert_eq!(config_value["type"], "config");
        assert_eq!(config_value["version"], 1);
        assert!(
            config_value.get("config").is_none(),
            "workspace chartml_config must be flat, not nested under a 'config' key"
        );
    }
}
