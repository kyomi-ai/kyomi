// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the sidebar — recent chat sessions and user info.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::IntoServerFnErrorCore;

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
    /// Whether the server is running in self-hosted mode.
    pub is_self_hosted: bool,
    /// Subscription status for the workspace: "trialing", "active", "past_due", "cancelled".
    pub subscription_status: String,
    /// Trial expiration ISO 8601 timestamp. Present when status is "trialing".
    pub trial_ends_at: Option<String>,
    /// Whether this workspace must pay before continuing to use the app.
    ///
    /// Computed server-side by `kyomi_core::capability::is_billing_lapsed` —
    /// the one definition of this rule. The client must never re-derive it
    /// (e.g. by string-matching `subscription_status`) — that predicate
    /// already accounts for cases a naive status match gets wrong, such as a
    /// scheduled cancellation (`cancel_at_period_end`) that still has paid-up
    /// time remaining. Always `false` outside SaaS mode (self-hosted and
    /// personal deployments have no billing).
    pub billing_lapsed: bool,
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
    .into_sfn_core()?;

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
        .into_sfn_core()?
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

    // is_billing_lapsed is SaaS-only (see its doc comment) — self-hosted and
    // personal deployments have no billing. `ctx.config.self_hosted` already
    // covers personal mode too: `KyomiMode::self_hosted()` derives `true` for
    // both `SelfHosted` and `Personal`, mirroring the same branch in
    // `get_user_context` (context.rs). The middleware's `WorkspaceContext`
    // doesn't carry `stripe_subscription_id` / `subscription_period_end`, so
    // a SaaS workspace needs its own load, same as `get_user_context` does
    // for capabilities.
    let billing_lapsed = if ctx.config.self_hosted {
        false
    } else if let Some(ws_id) = auth.workspace.workspace_id.as_deref() {
        let workspace = kyomi_auth::workspace_service::get_workspace_full(&ctx.db, ws_id)
            .await
            .into_sfn_core()?
            .ok_or_else(|| ServerFnError::new("Workspace not found"))?;
        kyomi_core::capability::is_billing_lapsed(&workspace, chrono::Utc::now())
    } else {
        // SaaS with no workspace: nothing is billed, so nothing can be lapsed.
        false
    };

    Ok(SidebarUser {
        user_id: auth.user_id.clone(),
        workspace_id: auth.workspace.workspace_id.clone(),
        name: auth.name.clone(),
        email: auth.email.clone(),
        workspace_name: auth.workspace.workspace_name.clone(),
        is_personal_mode: ctx.config.is_personal(),
        is_self_hosted: ctx.config.self_hosted,
        subscription_status: auth.workspace.subscription_status.to_string(),
        trial_ends_at,
        billing_lapsed,
        theme_preference,
    })
}
