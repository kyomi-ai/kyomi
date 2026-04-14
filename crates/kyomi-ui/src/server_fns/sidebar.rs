// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the sidebar — recent chat sessions and user info.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// Minimal chat session info for the sidebar list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SidebarSession {
    pub session_id: String,
    pub title: String,
}

/// User info for the sidebar user menu.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SidebarUser {
    pub user_id: String,
    pub workspace_id: Option<String>,
    pub name: Option<String>,
    pub email: String,
    pub workspace_name: Option<String>,
    pub is_personal_mode: bool,
    /// Subscription status for the workspace: "trialing", "active", "past_due", "cancelled".
    pub subscription_status: String,
    /// Trial expiration ISO 8601 timestamp. Present when status is "trialing".
    pub trial_ends_at: Option<String>,
    /// User's theme preference: "light", "dark", or "system".
    pub theme_preference: String,
}

/// Load recent chat sessions for the sidebar.
#[server(prefix = "/leptos-api")]
pub async fn get_recent_sessions() -> Result<Vec<SidebarSession>, ServerFnError> {
    let auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;

    let workspace_id = auth
        .workspace
        .workspace_id
        .as_deref()
        .unwrap_or("");

    let sessions = kyomi_auth::chat_service::get_user_sessions(
        &ctx.db,
        &auth.user_id,
        workspace_id,
        20,    // limit
        0,     // offset
        false, // pinned_only
        "chat",
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(sessions
        .into_iter()
        .map(|s| SidebarSession {
            session_id: s.session_id,
            title: s.title.unwrap_or_else(|| "New Chat".to_string()),
        })
        .collect())
}

/// Load current user info for the sidebar user menu.
#[server(prefix = "/leptos-api")]
pub async fn get_sidebar_user() -> Result<SidebarUser, ServerFnError> {
    let auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;

    // Read theme preference from user's extra_metadata (same source as profile.rs)
    let user = kyomi_auth::user_service::get_user_by_id(&ctx.db, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let theme_preference = user
        .extra_metadata
        .as_ref()
        .and_then(|v| v.get("theme"))
        .and_then(|v| v.as_str())
        .unwrap_or("system")
        .to_string();

    // trial_ends_at is already on the middleware's WorkspaceContext — no extra DB query needed.
    let trial_ends_at = auth.workspace.trial_ends_at.map(|dt| dt.to_rfc3339());

    Ok(SidebarUser {
        user_id: auth.user_id.clone(),
        workspace_id: auth.workspace.workspace_id.clone(),
        name: auth.name.clone(),
        email: auth.email.clone(),
        workspace_name: auth.workspace.workspace_name.clone(),
        is_personal_mode: ctx.config.is_personal(),
        subscription_status: auth.workspace.subscription_status.to_string(),
        trial_ends_at,
        theme_preference,
    })
}
