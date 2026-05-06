// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server function for the home page landing redirect.
//!
//! Fetches all data needed to determine where the user should be
//! redirected in a single round-trip: landing page preference,
//! default dashboard IDs (user and workspace), and system config flags.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// All data needed to resolve the user's landing redirect target.
///
/// Consolidated into a single struct to avoid multiple round-trips
/// on the home page, which is latency-critical.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LandingConfig {
    /// User's preferred landing page: "chat", "watches", "sql_editor", or "dashboards".
    pub landing_page: String,
    /// User's personal default dashboard (from `extra_metadata.default_dashboard_id`).
    pub user_default_dashboard_id: Option<String>,
    /// Workspace-level default dashboard (from `workspaces.settings.default_dashboard_id`).
    pub workspace_default_dashboard_id: Option<String>,
    /// Whether the system is running in personal/self-hosted mode.
    pub is_personal_mode: bool,
    /// Whether an LLM API key is configured (Anthropic or generic).
    pub llm_configured: bool,
}

/// Fetch the landing configuration for the authenticated user.
///
/// Reads the user's landing page preference from `extra_metadata`,
/// both user-level and workspace-level default dashboard IDs, and
/// system configuration flags needed for the redirect decision.
///
/// Mirrors the data consumed by `apps/frontend/src/components/LandingRedirect.jsx`.
#[server(prefix = "/leptos-api")]
pub async fn get_landing_config() -> Result<LandingConfig, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    // ── User preferences ────────────────────────────────────────────────
    let user = kyomi_auth::user_service::get_user_by_id(&ctx.db, &auth.user_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let metadata = user.extra_metadata.as_ref().and_then(|v| v.as_object());

    let landing_page = metadata
        .and_then(|m| m.get("landing_page"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let user_default_dashboard_id = metadata
        .and_then(|m| m.get("default_dashboard_id"))
        .and_then(|v| v.as_str())
        .map(String::from);

    // ── Workspace default dashboard ─────────────────────────────────────
    let workspace_default_dashboard_id = if let Some(ws_id) = auth.workspace.workspace_id.as_deref()
    {
        let workspace = kyomi_auth::workspace_service::get_workspace_full(&ctx.db, ws_id)
            .await
            .into_sfn()?;

        workspace
            .and_then(|ws| ws.settings)
            .and_then(|s| s.get("default_dashboard_id").cloned())
            .and_then(|v| v.as_str().map(String::from))
    } else {
        None
    };

    // ── System config flags ─────────────────────────────────────────────
    let is_personal_mode = ctx.config.is_personal();
    let llm_configured = ctx.config.llm_configured();

    Ok(LandingConfig {
        landing_page,
        user_default_dashboard_id,
        workspace_default_dashboard_id,
        is_personal_mode,
        llm_configured,
    })
}

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, IntoServerFnError};
