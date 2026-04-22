// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server function for user context — replaces React's AuthContext,
//! CapabilitiesContext, and SystemConfigContext with a single RPC call.
//!
//! Every settings tab uses this for role-based visibility and feature gating.

use std::collections::HashMap;

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context};

/// Combined user, workspace, and capability context.
///
/// Replaces three separate React contexts (AuthContext, CapabilitiesContext,
/// SystemConfigContext) with a single server function call. Every settings
/// tab reads this via Leptos context for role-based visibility decisions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserContext {
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_name: Option<String>,
    /// Workspace roles as strings, e.g. ["workspace_admin", "user"].
    pub workspace_roles: Vec<String>,
    pub is_owner: bool,
    /// Subscription tier slug: "free", "team", "cloud", "enterprise", etc.
    pub subscription_tier: String,
    /// Subscription status: "trialing", "active", "past_due", "cancelled".
    pub subscription_status: String,
    pub is_personal_mode: bool,
    pub is_self_hosted: bool,
    /// Whether billing UI should be shown. Convenience field that mirrors
    /// `capabilities["billing_enabled"]` — duplicated here so callers can
    /// check billing gating without reaching into the capabilities map.
    pub billing_enabled: bool,
    /// Feature flags — boolean capabilities keyed by name.
    pub capabilities: HashMap<String, bool>,
    /// User's chart color palette preference (e.g. "balanced", "vibrant", "accessible").
    pub chart_palette: String,
}

/// Load the authenticated user's full context: identity, workspace, and capabilities.
///
/// This is called once at the settings shell level and provided via Leptos context
/// so all settings tabs can read it without re-fetching.
#[server(prefix = "/leptos-api")]
pub async fn get_user_context() -> Result<UserContext, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let workspace_id = auth.workspace.workspace_id.as_deref();
    let subscription_tier = auth.workspace.subscription_tier;

    // Compute capabilities from the workspace (mirrors apps/server/src/routes/workspaces.rs).
    let capabilities = if ctx.config.self_hosted {
        kyomi_core::capability::compute_capabilities_self_hosted()
    } else if let Some(ws_id) = workspace_id {
        let workspace =
            kyomi_auth::workspace_service::get_workspace_full(&ctx.db, ws_id)
                .await
                .map_err(|e| ServerFnError::new(e.to_string()))?
                .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

        kyomi_core::capability::compute_capabilities(&workspace)
    } else {
        // No workspace in SaaS mode — return minimal free-tier defaults.
        // Do NOT use compute_capabilities_self_hosted here; that grants
        // enterprise permissions to unauthenticated-workspace SaaS users.
        free_tier_capabilities()
    };

    // Flatten Capabilities struct into a HashMap<String, bool> for the frontend.
    let mut caps_map = build_capabilities_map(&capabilities);

    // Runtime checks that aren't part of the static Capabilities struct.
    // These are read by ChatPage to gate the no-datasources empty state
    // and the personal-mode-no-LLM empty state.

    // has_datasources: check if any active datasources exist in this workspace.
    // Matches React's useDatasources() hook which calls GET /api/v1/datasources.
    let has_datasources = if let Some(ws_id) = workspace_id {
        kyomi_auth::datasource_service::list_datasources(&ctx.db, ws_id, false)
            .await
            .map(|ds| !ds.is_empty())
            .unwrap_or(false)
    } else {
        false
    };
    caps_map.insert("has_datasources".into(), has_datasources);

    // llm_configured: whether an LLM API key is configured (personal mode).
    caps_map.insert("llm_configured".into(), ctx.config.llm_configured());

    // billing_enabled: true when Stripe is configured AND not self-hosted.
    // Matches the Capabilities struct's billing_enabled field.
    let billing_enabled = capabilities.billing_enabled;

    // Read user's chart palette preference from chartml_config JSON.
    // Post-KYO-129 Part 2 migration the DB stores the flat shape; legacy
    // rows predating the migration may still be nested, and the shared
    // extractor handles both for defence in depth.
    // See kyomi_auth::user_service::get_user_palette_name.
    let chart_palette = kyomi_auth::user_service::get_user_palette_name(&ctx.db, &auth.user_id).await;

    Ok(UserContext {
        user_id: auth.user_id,
        email: auth.email,
        name: auth.name,
        workspace_id: auth.workspace.workspace_id,
        workspace_name: auth.workspace.workspace_name,
        workspace_roles: auth
            .workspace
            .workspace_roles
            .iter()
            .map(|r| r.to_string())
            .collect(),
        is_owner: auth.workspace.is_owner,
        subscription_tier: subscription_tier.to_string(),
        subscription_status: auth.workspace.subscription_status.to_string(),
        is_personal_mode: ctx.config.is_personal(),
        is_self_hosted: ctx.config.self_hosted,
        billing_enabled,
        capabilities: caps_map,
        chart_palette,
    })
}

/// Minimal free-tier capabilities for SaaS users without a workspace.
///
/// Grants only the baseline feature set — no premium flags, no multi-user,
/// billing enabled (so they can upgrade), and conservative limits.
#[cfg(feature = "ssr")]
fn free_tier_capabilities() -> kyomi_core::capability::Capabilities {
    use kyomi_core::enums::{SubscriptionStatus, SubscriptionTier};

    kyomi_core::capability::Capabilities {
        subscription_tier: SubscriptionTier::Free,
        subscription_status: SubscriptionStatus::Active,
        billing_enabled: true, // SaaS — allow user to see billing/upgrade
        credits_remaining: 0.0,
        credits_limit: 0.0,
        credits_exhausted: true,

        // AI features disabled — no credits
        ai_chat_enabled: false,
        ai_sql_generation_enabled: false,
        ai_autocomplete_enabled: false,
        ai_chart_copilot_enabled: false,

        // BigQuery — keep in sync with compute_capabilities() in capability.rs
        bigquery_access_level: "full".to_string(),

        // Organization features — disabled for free tier
        multi_user_enabled: false,
        user_management_enabled: false,
        dashboard_sharing_enabled: false,

        // Data features — keep in sync with compute_capabilities() in capability.rs
        export_enabled: true,
        api_access_enabled: false,

        // Free-tier limits
        catalog_refresh_limit_per_hour: 1,
        max_dashboards: 5,
        query_history_retention_days: 7,
        user_limit: 1,

        // Premium flags — all disabled
        kyomi_watch_enabled: false,
        slack_integration_enabled: false,
        mcp_access_enabled: true, // MCP is available to all tiers
        pdf_export_enabled: false,
    }
}

/// Convert the Capabilities struct into a HashMap<String, bool> for the frontend.
///
/// Includes all boolean feature flags. Numeric limits and string fields are excluded
/// since settings tabs only need boolean gates for visibility decisions.
#[cfg(feature = "ssr")]
fn build_capabilities_map(caps: &kyomi_core::capability::Capabilities) -> HashMap<String, bool> {
    let mut map = HashMap::new();

    // AI features
    map.insert("ai_chat_enabled".into(), caps.ai_chat_enabled);
    map.insert("ai_sql_generation_enabled".into(), caps.ai_sql_generation_enabled);
    map.insert("ai_autocomplete_enabled".into(), caps.ai_autocomplete_enabled);
    map.insert("ai_chart_copilot_enabled".into(), caps.ai_chart_copilot_enabled);

    // Organization features
    map.insert("multi_user_enabled".into(), caps.multi_user_enabled);
    map.insert("user_management_enabled".into(), caps.user_management_enabled);
    map.insert("dashboard_sharing_enabled".into(), caps.dashboard_sharing_enabled);

    // Data features
    map.insert("export_enabled".into(), caps.export_enabled);
    map.insert("api_access_enabled".into(), caps.api_access_enabled);

    // Premium flags
    map.insert("kyomi_watch_enabled".into(), caps.kyomi_watch_enabled);
    map.insert("slack_integration_enabled".into(), caps.slack_integration_enabled);
    map.insert("mcp_access_enabled".into(), caps.mcp_access_enabled);
    map.insert("pdf_export_enabled".into(), caps.pdf_export_enabled);

    // Billing
    map.insert("billing_enabled".into(), caps.billing_enabled);
    map.insert("credits_exhausted".into(), caps.credits_exhausted);

    map
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::*;

    #[test]
    fn build_capabilities_map_includes_all_boolean_flags() {
        // Use self-hosted capabilities as a convenient fully-populated struct.
        let caps = kyomi_core::capability::compute_capabilities_self_hosted();
        let map = build_capabilities_map(&caps);

        // Verify all expected keys are present.
        let expected_keys = [
            "ai_chat_enabled",
            "ai_sql_generation_enabled",
            "ai_autocomplete_enabled",
            "ai_chart_copilot_enabled",
            "multi_user_enabled",
            "user_management_enabled",
            "dashboard_sharing_enabled",
            "export_enabled",
            "api_access_enabled",
            "kyomi_watch_enabled",
            "slack_integration_enabled",
            "mcp_access_enabled",
            "pdf_export_enabled",
            "billing_enabled",
            "credits_exhausted",
        ];

        for key in &expected_keys {
            assert!(map.contains_key(*key), "Missing key: {key}");
        }

        // Self-hosted should have most features enabled.
        assert_eq!(map["ai_chat_enabled"], true);
        assert_eq!(map["multi_user_enabled"], true);
        assert_eq!(map["billing_enabled"], false); // self-hosted has no billing
        assert_eq!(map["credits_exhausted"], false);
    }

    #[test]
    fn build_capabilities_map_reflects_disabled_features() {
        // Self-hosted should have billing disabled.
        let caps = kyomi_core::capability::compute_capabilities_self_hosted();
        let map = build_capabilities_map(&caps);

        assert_eq!(map["billing_enabled"], false);
        assert_eq!(map["credits_exhausted"], false);
    }

    #[test]
    fn free_tier_capabilities_are_restrictive() {
        let caps = free_tier_capabilities();

        assert_eq!(caps.subscription_tier, kyomi_core::enums::SubscriptionTier::Free);
        assert!(caps.billing_enabled, "SaaS free tier should show billing UI");
        assert!(caps.credits_exhausted, "No credits by default");

        // AI features off
        assert!(!caps.ai_chat_enabled);
        assert!(!caps.ai_sql_generation_enabled);

        // Premium features off
        assert!(!caps.multi_user_enabled);
        assert!(!caps.kyomi_watch_enabled);
        assert!(!caps.slack_integration_enabled);
        assert!(!caps.pdf_export_enabled);

        // Data features
        assert!(caps.export_enabled, "export is included in free tier");

        // MCP available to all tiers
        assert!(caps.mcp_access_enabled);

        // Free-tier limits
        assert_eq!(caps.max_dashboards, 5);
        assert_eq!(caps.user_limit, 1);
        assert_eq!(caps.catalog_refresh_limit_per_hour, 1);
    }
}
