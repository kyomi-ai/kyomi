// SPDX-License-Identifier: AGPL-3.0-or-later

//! GET /api/v1/system/config — public endpoint for frontend feature detection.
//!
//! Returns the server edition and which features are available. No auth required —
//! the frontend needs this before login to hide/show auth options and feature gates.

use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;

use kyomi_core::config::KyomiMode;

use crate::state::AppState;

#[derive(Serialize)]
pub struct SystemConfig {
    self_hosted: bool,
    /// "community", "enterprise", or "saas"
    edition: &'static str,
    /// "personal", "self_hosted", or "saas"
    mode: &'static str,
    /// Whether an LLM provider is configured (ANTHROPIC_API_KEY or LLM_API_KEY)
    llm_configured: bool,
    features: FeatureFlags,
}

#[derive(Serialize)]
struct FeatureFlags {
    ai_enabled: bool,
    smtp_configured: bool,
    slack_configured: bool,
    /// PDF export: Enterprise only
    pdf_export: bool,
    /// Watch email alerts: Enterprise + SMTP configured
    watch_email_alerts: bool,
    /// Watch Slack alerts: Enterprise + Slack configured
    watch_slack_alerts: bool,
    /// Slack workspace integration: Enterprise + Slack configured
    slack_integration: bool,
    /// Website analytics: always false for self-hosted
    website_analytics: bool,
}

/// Map the `KyomiMode` enum to the JSON string returned by the API.
fn mode_str(mode: &KyomiMode) -> &'static str {
    match mode {
        KyomiMode::Personal => "personal",
        KyomiMode::SelfHosted => "self_hosted",
        KyomiMode::Saas => "saas",
    }
}

pub async fn get_system_config(State(state): State<AppState>) -> Json<SystemConfig> {
    let config = &state.config;
    let self_hosted = config.self_hosted;

    let edition: &'static str = if !self_hosted {
        "saas"
    } else if config.is_enterprise() {
        "enterprise"
    } else {
        "community"
    };

    let mode = mode_str(&config.mode);
    let llm_configured = config.llm_configured();

    let smtp_configured = config.smtp_configured();
    let slack_configured = config.slack_configured();
    let ai_enabled = config.llm_configured();

    let features = if !self_hosted {
        // SaaS: all features available at the platform level.
        // Per-user gating is done server-side by the capability service.
        FeatureFlags {
            ai_enabled,
            smtp_configured: true,
            slack_configured: true,
            pdf_export: true,
            watch_email_alerts: true,
            watch_slack_alerts: true,
            slack_integration: true,
            website_analytics: true,
        }
    } else {
        let is_enterprise = config.is_enterprise();
        FeatureFlags {
            ai_enabled,
            smtp_configured,
            slack_configured,
            pdf_export: is_enterprise,
            watch_email_alerts: is_enterprise && smtp_configured,
            watch_slack_alerts: is_enterprise && slack_configured,
            slack_integration: is_enterprise && slack_configured,
            website_analytics: false, // always off for self-hosted
        }
    };

    Json(SystemConfig {
        self_hosted,
        edition,
        mode,
        llm_configured,
        features,
    })
}

/// Build the system config router.
///
/// Mounted under `/api/v1/system` so the full path is:
/// - `GET /api/v1/system/config`
pub fn routes() -> Router<AppState> {
    Router::new().route("/config", get(get_system_config))
}
