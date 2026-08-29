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
        .into_sfn_core()?
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
            .into_sfn_core()?;

        workspace
            .and_then(|ws| ws.settings)
            .and_then(|s| s.get("default_dashboard_id").cloned())
            .and_then(|v| v.as_str().map(String::from))
    } else {
        None
    };

    // ── Validate default dashboard IDs ─────────────────────────────────
    let (user_default_dashboard_id, workspace_default_dashboard_id) =
        validate_default_dashboard_ids(
            &ctx.db,
            &auth.user_id,
            user_default_dashboard_id,
            workspace_default_dashboard_id,
            auth.workspace.workspace_id.as_deref(),
        )
        .await;

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
use super::{extract_auth, extract_context, IntoServerFnErrorCore};

/// Check that default dashboard IDs still refer to existing dashboards.
/// Returns `None` for any stale ID so the redirect logic falls through.
#[cfg(feature = "ssr")]
async fn validate_default_dashboard_ids(
    db: &kyomi_core::DbPool,
    auth_user_id: &str,
    user_id: Option<String>,
    workspace_id_val: Option<String>,
    ws_id: Option<&str>,
) -> (Option<String>, Option<String>) {
    let user_id = match (&user_id, ws_id) {
        (Some(id), Some(ws)) => {
            match kyomi_auth::dashboard_service::get_dashboard(db, id, ws, auth_user_id).await {
                Ok(Some(_)) => user_id,
                Ok(None) => None,
                Err(e) => {
                    tracing::warn!(
                        dashboard_id = %id,
                        error = %e,
                        "failed to validate user default dashboard — treating as unset"
                    );
                    None
                }
            }
        }
        _ => user_id,
    };

    let workspace_id_val = match (&workspace_id_val, ws_id) {
        (Some(id), Some(ws)) => {
            match kyomi_auth::dashboard_service::get_dashboard(db, id, ws, auth_user_id).await {
                Ok(Some(_)) => workspace_id_val,
                Ok(None) => None,
                Err(e) => {
                    tracing::warn!(
                        dashboard_id = %id,
                        error = %e,
                        "failed to validate workspace default dashboard — treating as unset"
                    );
                    None
                }
            }
        }
        _ => workspace_id_val,
    };

    (user_id, workspace_id_val)
}
