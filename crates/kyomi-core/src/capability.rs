// SPDX-License-Identifier: AGPL-3.0-or-later

//! Capability service — unified feature gating for workspaces.
//!
//! This is the Rust equivalent of Python's `capability_service.py`.
//! It is the **single source of truth** for determining what features
//! are available to a workspace based on subscription tier and credits.
//!
//! ## Phase 4 simplifications
//!
//! - No `BillingService` integration — uses `workspace.ai_credits_used_usd`
//!   directly with hardcoded credit budgets.
//! - BigQuery Arrow streaming derived from `bq_arrow_enabled` parameter.
//! - No async DB queries for `has_capability` — operates on a `&Workspace`.

use serde::Serialize;

use crate::enums::{SubscriptionStatus, SubscriptionTier};
use crate::models::Workspace;

// ─── Public types ────────────────────────────────────────────────────────────

/// Full capabilities response matching the Python JSON shape exactly.
///
/// Field names use `#[serde(rename)]` where the Rust field name differs
/// from the Python JSON key for wire compatibility.
#[derive(Debug, Serialize)]
pub struct Capabilities {
    // Subscription info
    pub subscription_tier: SubscriptionTier,
    pub subscription_status: SubscriptionStatus,
    /// Whether Stripe billing is available (SaaS mode only, false for self-hosted).
    pub billing_enabled: bool,
    pub credits_remaining: f64,
    pub credits_limit: f64,
    pub credits_exhausted: bool,

    // AI features (gated by credits_exhausted)
    pub ai_chat_enabled: bool,
    pub ai_sql_generation_enabled: bool,
    pub ai_autocomplete_enabled: bool,
    pub ai_chart_copilot_enabled: bool,

    // BigQuery
    pub bigquery_access_level: String,
    pub bigquery_retrieval_mode: String,

    // Organization features
    pub multi_user_enabled: bool,
    pub user_management_enabled: bool,
    pub dashboard_sharing_enabled: bool,

    // Data features
    pub export_enabled: bool,
    pub api_access_enabled: bool,

    // Limits
    pub catalog_refresh_limit_per_hour: i32,
    pub max_dashboards: i32,
    pub query_history_retention_days: i32,
    pub user_limit: i32,

    // Premium flags
    pub kyomi_watch_enabled: bool,
    pub arrow_streaming_enabled: bool,
    pub slack_integration_enabled: bool,
    pub mcp_access_enabled: bool,
    pub pdf_export_enabled: bool,
}

/// Credit usage information for a workspace.
#[derive(Debug, Serialize)]
pub struct CreditsInfo {
    pub limit_usd: f64,
    pub used_usd: f64,
    pub remaining_usd: f64,
    pub exhausted: bool,
    pub percentage_used: f64,
}

// ─── Tier helpers ────────────────────────────────────────────────────────────

/// Get the effective subscription tier for a workspace.
///
/// - Checks for a developer override in `workspace.settings.custom_settings.subscription_tier`.
/// - Maps legacy `"regular"` to `Pro` for backward compatibility.
pub fn get_subscription_tier(workspace: &Workspace) -> SubscriptionTier {
    // Check for explicit tier override in settings.custom_settings
    if let Some(ref settings) = workspace.settings
        && let Some(custom) = settings.get("custom_settings")
        && let Some(tier_val) = custom.get("subscription_tier")
        && let Some(tier_str) = tier_val.as_str()
    {
        if let Some(tier) = parse_tier(tier_str) {
            return tier;
        }
    }

    // Use the DB field directly — it's already a SubscriptionTier enum
    workspace.subscription_tier
}

/// Parse a tier string, mapping legacy `"regular"` to `Pro`.
fn parse_tier(s: &str) -> Option<SubscriptionTier> {
    match s {
        "free" => Some(SubscriptionTier::Free),
        "basic" => Some(SubscriptionTier::Basic),
        "starter" => Some(SubscriptionTier::Starter),
        "pro" | "regular" => Some(SubscriptionTier::Pro),
        "team" => Some(SubscriptionTier::Team),
        "enterprise" => Some(SubscriptionTier::Enterprise),
        _ => None,
    }
}

/// Get the monthly AI credit budget in USD for a given tier.
///
/// Budget values are read from environment variables via `ai_budget::CONFIG`.
/// For Team tier, the budget scales with `user_limit`:
/// `base + per_user * max(0, user_limit - base_users)` for additional users.
/// Pass `user_limit` from `workspace.user_limit` for accurate Team budgets.
pub fn get_credits_limit(tier: SubscriptionTier, user_limit: Option<i32>) -> f64 {
    let cfg = &crate::ai_budget::CONFIG;
    match tier {
        SubscriptionTier::Free => cfg.free,
        SubscriptionTier::Basic | SubscriptionTier::Starter => cfg.starter,
        SubscriptionTier::Pro => cfg.pro,
        SubscriptionTier::Team => {
            let effective_limit = user_limit
                .filter(|&u| u > 0)
                .unwrap_or(cfg.team_base_users);
            let additional_users = (effective_limit - cfg.team_base_users).max(0);
            cfg.team_base + (cfg.team_per_user * f64::from(additional_users))
        }
        SubscriptionTier::Enterprise => cfg.enterprise,
    }
}

/// Compute credit usage info from `workspace.ai_credits_used_usd`.
///
/// **Fallback path**: Uses the workspace's cached `ai_credits_used_usd` field
/// directly. For accurate billing-period-aware usage, call
/// `BillingService::calculate_credits_info()` and pass the result to
/// `compute_capabilities_with_credits()`.
pub fn get_credits_info(workspace: &Workspace, tier: SubscriptionTier) -> CreditsInfo {
    let limit = get_credits_limit(tier, workspace.user_limit);
    let used = workspace.ai_credits_used_usd;
    let remaining = (limit - used).max(0.0);
    let exhausted = used >= limit;
    let percentage_used = if limit > 0.0 {
        ((used / limit) * 100.0).min(100.0)
    } else {
        100.0 // No budget = fully exhausted
    };

    CreditsInfo {
        limit_usd: limit,
        used_usd: used,
        remaining_usd: remaining,
        exhausted,
        percentage_used,
    }
}

/// Get the user limit for a subscription tier.
///
/// - `free`, `basic`, `starter`, `pro`: 1 user
/// - `team`: uses `workspace.user_limit` (default 1, matching DB `server_default=text("1")`)
///   The Stripe webhook handler sets `user_limit` explicitly when creating Team subscriptions.
/// - `enterprise`: effectively unlimited (999999)
pub fn get_user_limit(workspace: &Workspace, tier: SubscriptionTier) -> i32 {
    match tier {
        SubscriptionTier::Enterprise => 999_999,
        SubscriptionTier::Team => workspace.user_limit.unwrap_or(1),
        _ => 1, // free, basic, starter, pro
    }
}

/// Check if a tier has a specific premium capability.
///
/// Returns `true` for capabilities available to all tiers if the
/// capability name is not in the premium lookup table.
pub fn has_capability(tier: SubscriptionTier, capability: &str) -> bool {
    use SubscriptionTier::*;

    let allowed_tiers: &[SubscriptionTier] = match capability {
        "kyomi_watch" => &[Pro, Team, Enterprise],
        "arrow_streaming" => &[Pro, Team, Enterprise],
        "slack_integration" => &[Team, Enterprise],
        "multi_user" => &[Team, Enterprise],
        "dashboard_sharing" => &[Team, Enterprise],
        "api_access" => &[Pro, Team, Enterprise],
        "mcp_access" => &[Free, Starter, Basic, Pro, Team, Enterprise],
        "pdf_export" => &[Pro, Team, Enterprise],
        _ => return true, // Not a premium capability — available to all
    };

    allowed_tiers.contains(&tier)
}

// ─── Limit helpers ───────────────────────────────────────────────────────────

/// Catalog refresh limit per hour by tier.
///
/// Python: free=1, all paid tiers=5.
fn get_catalog_refresh_limit(tier: SubscriptionTier) -> i32 {
    match tier {
        SubscriptionTier::Free => 1,
        _ => 5, // All paid tiers get the same limit
    }
}

/// Maximum dashboards by tier. `0` means unlimited (Python convention).
fn get_dashboard_limit(tier: SubscriptionTier) -> i32 {
    match tier {
        SubscriptionTier::Free => 5,
        _ => 0, // basic, pro, team, enterprise: unlimited
    }
}

/// Query history retention in days. `0` means unlimited (Python convention).
fn get_query_history_retention(tier: SubscriptionTier) -> i32 {
    match tier {
        SubscriptionTier::Free => 7,
        SubscriptionTier::Basic | SubscriptionTier::Starter => 30,
        _ => 0, // pro, team, enterprise: unlimited
    }
}

// ─── Main entry point ────────────────────────────────────────────────────────

/// Compute the full capabilities for a workspace.
///
/// This is the main entry point, equivalent to Python's
/// `CapabilityService.compute_capabilities()`.
///
/// `bq_arrow_enabled` should be `true` when at least one BigQuery datasource
/// in the workspace has `connection_config.enable_arrow_streaming = true`.
/// The route handler queries `datasource_configs` for this flag.
///
/// Field names and values match the Python JSON output exactly.
pub fn compute_capabilities(workspace: &Workspace, bq_arrow_enabled: bool) -> Capabilities {
    let tier = get_subscription_tier(workspace);
    let credits = get_credits_info(workspace, tier);

    // AI features are enabled when credits are not exhausted
    let ai_enabled = !credits.exhausted;

    Capabilities {
        subscription_tier: tier,
        subscription_status: workspace.subscription_status,
        billing_enabled: true, // SaaS mode — Stripe billing available
        credits_remaining: credits.remaining_usd,
        credits_limit: credits.limit_usd,
        credits_exhausted: credits.exhausted,

        // AI features
        ai_chat_enabled: ai_enabled,
        ai_sql_generation_enabled: ai_enabled,
        ai_autocomplete_enabled: ai_enabled,
        ai_chart_copilot_enabled: ai_enabled,

        // BigQuery — uses arrow_streaming when enabled + tier supports it
        bigquery_access_level: "full".to_string(),
        bigquery_retrieval_mode: if bq_arrow_enabled && has_capability(tier, "arrow_streaming") {
            "arrow_streaming".to_string()
        } else {
            "direct_api".to_string()
        },

        // Organization features
        multi_user_enabled: has_capability(tier, "multi_user"),
        user_management_enabled: has_capability(tier, "multi_user"), // same gate
        dashboard_sharing_enabled: has_capability(tier, "dashboard_sharing"),

        // Data features — Python reads from workspace.settings, default true
        export_enabled: true,
        api_access_enabled: has_capability(tier, "api_access"),

        // Limits
        catalog_refresh_limit_per_hour: get_catalog_refresh_limit(tier),
        max_dashboards: get_dashboard_limit(tier),
        query_history_retention_days: get_query_history_retention(tier),
        user_limit: get_user_limit(workspace, tier),

        // Premium flags
        kyomi_watch_enabled: has_capability(tier, "kyomi_watch"),
        arrow_streaming_enabled: has_capability(tier, "arrow_streaming"),
        slack_integration_enabled: has_capability(tier, "slack_integration"),
        mcp_access_enabled: has_capability(tier, "mcp_access"),
        pdf_export_enabled: has_capability(tier, "pdf_export"),
    }
}

/// Compute the full capabilities for a workspace using externally-provided
/// credits information from `BillingService`.
///
/// This is the **preferred** entry point when a database connection is available.
/// Callers should:
/// 1. Call `BillingService::calculate_credits_info()` to get real usage data
/// 2. Convert the result to `CreditsInfo`
/// 3. Pass it here
///
/// This preserves all existing tier-based feature gating logic — only the
/// credits/budget **source** changes.
pub fn compute_capabilities_with_credits(
    workspace: &Workspace,
    bq_arrow_enabled: bool,
    credits: &CreditsInfo,
) -> Capabilities {
    let tier = get_subscription_tier(workspace);

    // AI features are enabled when credits are not exhausted
    let ai_enabled = !credits.exhausted;

    Capabilities {
        subscription_tier: tier,
        subscription_status: workspace.subscription_status,
        billing_enabled: true, // SaaS mode — Stripe billing available
        credits_remaining: credits.remaining_usd,
        credits_limit: credits.limit_usd,
        credits_exhausted: credits.exhausted,

        // AI features
        ai_chat_enabled: ai_enabled,
        ai_sql_generation_enabled: ai_enabled,
        ai_autocomplete_enabled: ai_enabled,
        ai_chart_copilot_enabled: ai_enabled,

        // BigQuery — uses arrow_streaming when enabled + tier supports it
        bigquery_access_level: "full".to_string(),
        bigquery_retrieval_mode: if bq_arrow_enabled && has_capability(tier, "arrow_streaming") {
            "arrow_streaming".to_string()
        } else {
            "direct_api".to_string()
        },

        // Organization features
        multi_user_enabled: has_capability(tier, "multi_user"),
        user_management_enabled: has_capability(tier, "multi_user"), // same gate
        dashboard_sharing_enabled: has_capability(tier, "dashboard_sharing"),

        // Data features — Python reads from workspace.settings, default true
        export_enabled: true,
        api_access_enabled: has_capability(tier, "api_access"),

        // Limits
        catalog_refresh_limit_per_hour: get_catalog_refresh_limit(tier),
        max_dashboards: get_dashboard_limit(tier),
        query_history_retention_days: get_query_history_retention(tier),
        user_limit: get_user_limit(workspace, tier),

        // Premium flags
        kyomi_watch_enabled: has_capability(tier, "kyomi_watch"),
        arrow_streaming_enabled: has_capability(tier, "arrow_streaming"),
        slack_integration_enabled: has_capability(tier, "slack_integration"),
        mcp_access_enabled: has_capability(tier, "mcp_access"),
        pdf_export_enabled: has_capability(tier, "pdf_export"),
    }
}

/// Compute capabilities for self-hosted mode.
///
/// Returns enterprise-level capabilities with unlimited credits and all features
/// enabled. Self-hosted users pay their own LLM provider directly, so there are
/// no credit budgets or tier restrictions.
///
/// `bq_arrow_enabled` should be `true` when at least one BigQuery datasource
/// in the workspace has `connection_config.enable_arrow_streaming = true`.
pub fn compute_capabilities_self_hosted(bq_arrow_enabled: bool) -> Capabilities {
    Capabilities {
        subscription_tier: SubscriptionTier::Enterprise,
        subscription_status: SubscriptionStatus::Active,
        billing_enabled: false,
        credits_remaining: 999_999.0,
        credits_limit: 999_999.0,
        credits_exhausted: false,

        // All AI features enabled
        ai_chat_enabled: true,
        ai_sql_generation_enabled: true,
        ai_autocomplete_enabled: true,
        ai_chart_copilot_enabled: true,

        // BigQuery — arrow streaming derived from datasource config
        bigquery_access_level: "full".to_string(),
        bigquery_retrieval_mode: if bq_arrow_enabled {
            "arrow_streaming".to_string()
        } else {
            "direct_api".to_string()
        },

        // Organization features — all enabled
        multi_user_enabled: true,
        user_management_enabled: true,
        dashboard_sharing_enabled: true,

        // Data features
        export_enabled: true,
        api_access_enabled: true,

        // Limits — unlimited (0 = unlimited by convention, except user_limit
        // which uses 999_999 to match Enterprise tier convention from get_user_limit())
        catalog_refresh_limit_per_hour: 0,
        max_dashboards: 0,
        query_history_retention_days: 0,
        user_limit: 999_999,

        // Premium flags — all enabled
        kyomi_watch_enabled: true,
        arrow_streaming_enabled: bq_arrow_enabled,
        slack_integration_enabled: true,
        mcp_access_enabled: true,
        pdf_export_enabled: true,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enums::WorkspaceStatus;
    use chrono::Utc;

    /// Helper to create a minimal Workspace for testing.
    fn test_workspace(tier: SubscriptionTier, credits_used: f64) -> Workspace {
        Workspace {
            workspace_id: "ws-test".to_string(),
            name: Some("Test Workspace".to_string()),
            domain: None,
            status: WorkspaceStatus::Active,
            admin_email: Some("admin@test.com".to_string()),
            owner_user_id: "user-test".to_string(),
            subscription_tier: tier,
            subscription_status: SubscriptionStatus::Active,
            billing_cycle: None,
            subscription_period_start: None,
            subscription_period_end: None,
            trial_ends_at: None,
            ai_credits_used_usd: credits_used,
            user_limit: None,
            stripe_customer_id: None,
            stripe_subscription_id: None,
            stripe_additional_users_item_id: None,
            settings: None,
            business_knowledge: None,
            knowledge_updated_at: None,
            last_catalog_refresh: None,
            catalog_refresh_status: None,
            catalog_refresh_progress: None,
            catalog_onboarding_completed: false,
            catalog_indexed_projects: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_get_subscription_tier_basic() {
        let ws = test_workspace(SubscriptionTier::Pro, 0.0);
        assert_eq!(get_subscription_tier(&ws), SubscriptionTier::Pro);
    }

    #[test]
    fn test_get_subscription_tier_override() {
        let mut ws = test_workspace(SubscriptionTier::Free, 0.0);
        ws.settings = Some(serde_json::json!({
            "custom_settings": {
                "subscription_tier": "enterprise"
            }
        }));
        assert_eq!(get_subscription_tier(&ws), SubscriptionTier::Enterprise);
    }

    #[test]
    fn test_get_subscription_tier_override_regular() {
        let mut ws = test_workspace(SubscriptionTier::Free, 0.0);
        ws.settings = Some(serde_json::json!({
            "custom_settings": {
                "subscription_tier": "regular"
            }
        }));
        assert_eq!(get_subscription_tier(&ws), SubscriptionTier::Pro);
    }

    #[test]
    fn test_credits_limit_by_tier() {
        use SubscriptionTier::*;
        let cfg = &crate::ai_budget::CONFIG;
        assert!((get_credits_limit(Free, None) - cfg.free).abs() < f64::EPSILON);
        assert!((get_credits_limit(Basic, None) - cfg.starter).abs() < f64::EPSILON);
        assert!((get_credits_limit(Starter, None) - cfg.starter).abs() < f64::EPSILON);
        assert!((get_credits_limit(Pro, None) - cfg.pro).abs() < f64::EPSILON);
        // Team with base users = base budget
        assert!(
            (get_credits_limit(Team, Some(cfg.team_base_users)) - cfg.team_base)
                .abs()
                < f64::EPSILON
        );
        // Team with 3 additional users
        let extra = 3;
        let expected_team = cfg.team_base + (f64::from(extra) * cfg.team_per_user);
        assert!(
            (get_credits_limit(Team, Some(cfg.team_base_users + extra)) - expected_team).abs()
                < f64::EPSILON
        );
        // Team with None defaults to base users
        assert!(
            (get_credits_limit(Team, None) - cfg.team_base).abs() < f64::EPSILON
        );
        assert!((get_credits_limit(Enterprise, None) - cfg.enterprise).abs() < f64::EPSILON);
    }

    #[test]
    fn test_credits_info_not_exhausted() {
        // Use compute_capabilities_with_credits to provide explicit budget values,
        // independent of env-var-driven tier budgets.
        let ws = test_workspace(SubscriptionTier::Pro, 3.0);
        let credits = CreditsInfo {
            limit_usd: 9.0,
            used_usd: 3.0,
            remaining_usd: 6.0,
            exhausted: false,
            percentage_used: 33.333333333333336,
        };
        let caps = compute_capabilities_with_credits(&ws, false, &credits);
        assert!(!caps.credits_exhausted);
        assert!(caps.ai_chat_enabled);
        assert!((caps.credits_remaining - 6.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_credits_info_exhausted() {
        let ws = test_workspace(SubscriptionTier::Free, 1.0);
        let info = get_credits_info(&ws, SubscriptionTier::Free);
        assert!(info.exhausted);
        assert!((info.remaining_usd - 0.0).abs() < f64::EPSILON);
        assert!((info.percentage_used - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_user_limit_single_user_tiers() {
        let ws = test_workspace(SubscriptionTier::Free, 0.0);
        assert_eq!(get_user_limit(&ws, SubscriptionTier::Free), 1);
        assert_eq!(get_user_limit(&ws, SubscriptionTier::Basic), 1);
        assert_eq!(get_user_limit(&ws, SubscriptionTier::Pro), 1);
    }

    #[test]
    fn test_user_limit_team_default() {
        let ws = test_workspace(SubscriptionTier::Team, 0.0);
        assert_eq!(get_user_limit(&ws, SubscriptionTier::Team), 1);
    }

    #[test]
    fn test_user_limit_team_custom() {
        let mut ws = test_workspace(SubscriptionTier::Team, 0.0);
        ws.user_limit = Some(10);
        assert_eq!(get_user_limit(&ws, SubscriptionTier::Team), 10);
    }

    #[test]
    fn test_user_limit_enterprise() {
        let ws = test_workspace(SubscriptionTier::Enterprise, 0.0);
        assert_eq!(get_user_limit(&ws, SubscriptionTier::Enterprise), 999_999);
    }

    #[test]
    fn test_has_capability_premium() {
        use SubscriptionTier::*;
        assert!(!has_capability(Free, "kyomi_watch"));
        assert!(has_capability(Pro, "kyomi_watch"));
        assert!(has_capability(Team, "slack_integration"));
        assert!(!has_capability(Pro, "slack_integration"));
        assert!(has_capability(Basic, "mcp_access"));
        assert!(has_capability(Free, "mcp_access"));
    }

    #[test]
    fn test_has_capability_non_premium() {
        // Unknown capabilities are available to all tiers
        assert!(has_capability(SubscriptionTier::Free, "some_random_thing"));
    }

    #[test]
    fn test_compute_capabilities_free_tier() {
        let ws = test_workspace(SubscriptionTier::Free, 0.0);
        let credits = CreditsInfo {
            limit_usd: 1.0,
            used_usd: 0.0,
            remaining_usd: 1.0,
            exhausted: false,
            percentage_used: 0.0,
        };
        let caps = compute_capabilities_with_credits(&ws, false, &credits);

        assert_eq!(caps.subscription_tier, SubscriptionTier::Free);
        assert!(caps.ai_chat_enabled); // Not exhausted
        assert!(!caps.multi_user_enabled);
        assert!(!caps.kyomi_watch_enabled);
        assert!(!caps.slack_integration_enabled);
        assert!(caps.mcp_access_enabled);
        assert_eq!(caps.max_dashboards, 5);
        assert_eq!(caps.catalog_refresh_limit_per_hour, 1);
        assert_eq!(caps.query_history_retention_days, 7);
        assert_eq!(caps.user_limit, 1);
        assert_eq!(caps.bigquery_retrieval_mode, "direct_api");
    }

    #[test]
    fn test_compute_capabilities_pro_tier() {
        let ws = test_workspace(SubscriptionTier::Pro, 0.0);
        let credits = CreditsInfo {
            limit_usd: 9.0,
            used_usd: 0.0,
            remaining_usd: 9.0,
            exhausted: false,
            percentage_used: 0.0,
        };
        let caps = compute_capabilities_with_credits(&ws, false, &credits);

        assert_eq!(caps.subscription_tier, SubscriptionTier::Pro);
        assert!(caps.ai_chat_enabled);
        assert!(caps.kyomi_watch_enabled);
        assert!(caps.arrow_streaming_enabled);
        assert!(caps.api_access_enabled);
        assert!(caps.mcp_access_enabled);
        assert!(!caps.multi_user_enabled);
        assert!(!caps.slack_integration_enabled);
        assert_eq!(caps.max_dashboards, 0); // unlimited
        assert_eq!(caps.query_history_retention_days, 0); // unlimited
        assert_eq!(caps.bigquery_retrieval_mode, "direct_api");
    }

    #[test]
    fn test_compute_capabilities_pro_tier_with_arrow() {
        let ws = test_workspace(SubscriptionTier::Pro, 0.0);
        let caps = compute_capabilities(&ws, true);
        assert_eq!(caps.bigquery_retrieval_mode, "arrow_streaming");
    }

    #[test]
    fn test_compute_capabilities_free_tier_with_arrow() {
        let ws = test_workspace(SubscriptionTier::Free, 0.0);
        let caps = compute_capabilities(&ws, true);
        assert_eq!(caps.bigquery_retrieval_mode, "direct_api");
    }

    #[test]
    fn test_compute_capabilities_team_tier() {
        let mut ws = test_workspace(SubscriptionTier::Team, 0.0);
        ws.user_limit = Some(5);
        let caps = compute_capabilities(&ws, false);

        assert_eq!(caps.subscription_tier, SubscriptionTier::Team);
        assert!(caps.multi_user_enabled);
        assert!(caps.user_management_enabled);
        assert!(caps.dashboard_sharing_enabled);
        assert!(caps.slack_integration_enabled);
        assert!(caps.kyomi_watch_enabled);
        assert_eq!(caps.user_limit, 5);
        assert_eq!(caps.catalog_refresh_limit_per_hour, 5);
    }

    #[test]
    fn test_compute_capabilities_credits_exhausted() {
        let ws = test_workspace(SubscriptionTier::Pro, 10.0);
        let caps = compute_capabilities(&ws, false);

        assert!(caps.credits_exhausted);
        assert!(!caps.ai_chat_enabled);
        assert!(!caps.ai_sql_generation_enabled);
        assert!(!caps.ai_autocomplete_enabled);
        assert!(!caps.ai_chart_copilot_enabled);
    }

    #[test]
    fn test_capabilities_json_field_names_match_python() {
        let ws = test_workspace(SubscriptionTier::Free, 0.0);
        let caps = compute_capabilities(&ws, false);
        let json = serde_json::to_value(&caps).unwrap();

        // Verify every JSON key matches the Python compute_capabilities() return dict
        let expected_keys = [
            "subscription_tier",
            "subscription_status",
            "billing_enabled",
            "credits_remaining",
            "credits_limit",
            "credits_exhausted",
            "ai_chat_enabled",
            "ai_sql_generation_enabled",
            "ai_autocomplete_enabled",
            "ai_chart_copilot_enabled",
            "bigquery_access_level",
            "bigquery_retrieval_mode",
            "multi_user_enabled",
            "user_management_enabled",
            "dashboard_sharing_enabled",
            "export_enabled",
            "api_access_enabled",
            "catalog_refresh_limit_per_hour",
            "max_dashboards",
            "query_history_retention_days",
            "user_limit",
            "kyomi_watch_enabled",
            "arrow_streaming_enabled",
            "slack_integration_enabled",
            "mcp_access_enabled",
            "pdf_export_enabled",
        ];
        for key in &expected_keys {
            assert!(json.get(key).is_some(), "Missing JSON key: {key}");
        }

        // Verify no extra keys beyond what Python returns
        let obj = json.as_object().unwrap();
        for key in obj.keys() {
            assert!(
                expected_keys.contains(&key.as_str()),
                "Unexpected JSON key: {key}"
            );
        }
    }

    #[test]
    fn test_compute_capabilities_with_credits() {
        let ws = test_workspace(SubscriptionTier::Pro, 0.0);
        let credits = CreditsInfo {
            limit_usd: 9.0,
            used_usd: 4.5,
            remaining_usd: 4.5,
            exhausted: false,
            percentage_used: 50.0,
        };
        let caps = compute_capabilities_with_credits(&ws, false, &credits);

        assert_eq!(caps.subscription_tier, SubscriptionTier::Pro);
        assert!(caps.ai_chat_enabled);
        assert!(!caps.credits_exhausted);
        assert!((caps.credits_remaining - 4.5).abs() < f64::EPSILON);
        assert!((caps.credits_limit - 9.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_capabilities_with_credits_exhausted() {
        let ws = test_workspace(SubscriptionTier::Pro, 0.0);
        let credits = CreditsInfo {
            limit_usd: 9.0,
            used_usd: 9.5,
            remaining_usd: 0.0,
            exhausted: true,
            percentage_used: 100.0,
        };
        let caps = compute_capabilities_with_credits(&ws, false, &credits);

        assert!(caps.credits_exhausted);
        assert!(!caps.ai_chat_enabled);
        assert!(!caps.ai_sql_generation_enabled);
    }

    #[test]
    fn test_compute_capabilities_self_hosted() {
        let caps = compute_capabilities_self_hosted(false);
        assert_eq!(caps.subscription_tier, SubscriptionTier::Enterprise);
        assert_eq!(caps.subscription_status, SubscriptionStatus::Active);
        assert!(!caps.billing_enabled);
        assert!(!caps.credits_exhausted);

        // All AI features enabled
        assert!(caps.ai_chat_enabled);
        assert!(caps.ai_sql_generation_enabled);
        assert!(caps.ai_autocomplete_enabled);
        assert!(caps.ai_chart_copilot_enabled);

        // BigQuery — no arrow when not configured
        assert_eq!(caps.bigquery_access_level, "full");
        assert_eq!(caps.bigquery_retrieval_mode, "direct_api");

        // Organization features
        assert!(caps.multi_user_enabled);
        assert!(caps.user_management_enabled);
        assert!(caps.dashboard_sharing_enabled);

        // Data features
        assert!(caps.export_enabled);
        assert!(caps.api_access_enabled);

        // Limits — unlimited
        assert_eq!(caps.catalog_refresh_limit_per_hour, 0);
        assert_eq!(caps.max_dashboards, 0);
        assert_eq!(caps.query_history_retention_days, 0);
        assert_eq!(caps.user_limit, 999_999);

        // Premium flags
        assert!(caps.kyomi_watch_enabled);
        assert!(!caps.arrow_streaming_enabled); // false when bq_arrow_enabled=false
        assert!(caps.slack_integration_enabled);
        assert!(caps.mcp_access_enabled);
        assert!(caps.pdf_export_enabled);

        // Credits — effectively unlimited
        assert!((caps.credits_remaining - 999_999.0).abs() < f64::EPSILON);
        assert!((caps.credits_limit - 999_999.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_capabilities_self_hosted_with_arrow() {
        let caps = compute_capabilities_self_hosted(true);
        assert_eq!(caps.bigquery_retrieval_mode, "arrow_streaming");
        assert!(caps.arrow_streaming_enabled);
    }
}
