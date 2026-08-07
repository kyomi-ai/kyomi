// SPDX-License-Identifier: AGPL-3.0-or-later

//! Capability service — unified feature gating for workspaces.
//!
//! This is the Rust equivalent of Python's `capability_service.py`.
//! It is the **single source of truth** for determining what features
//! are available to a workspace based on credits and deployment mode
//! (SaaS vs self-hosted). The Cloud-plan billing migration flattened
//! subscription tiers, so tier is no longer a gating input.
//!
//! ## Phase 4 simplifications
//!
//! - No `BillingService` integration — uses `workspace.ai_credits_used_usd`
//!   directly with hardcoded credit budgets.

use serde::Serialize;

use crate::enums::{SubscriptionStatus, SubscriptionTier};
use crate::models::Workspace;

/// Effective "no limit" seat cap. Workspaces are never capped on member count —
/// billing is per active user, so the cap is always unlimited. Used as the
/// default anywhere a workspace's `user_limit` is unset.
pub const UNLIMITED_USER_LIMIT: i32 = 999_999;

/// Included analytics events per month for all Cloud subscribers.
pub const ANALYTICS_EVENTS_INCLUDED: u64 = 100_000;

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

    // AI chat (gated by credits_exhausted)
    pub ai_chat_enabled: bool,

    // BigQuery
    pub bigquery_access_level: String,

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
    pub slack_integration_enabled: bool,
    pub mcp_access_enabled: bool,
    pub pdf_export_enabled: bool,
}

/// Credit usage information for a workspace.
#[derive(Debug, Serialize)]
pub struct CapabilityCredits {
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
        && let Some(tier) = parse_tier(tier_str)
    {
        return tier;
    }

    // Use the DB field directly — it's already a SubscriptionTier enum
    workspace.subscription_tier
}

/// Parse a tier string, mapping legacy names to enum variants.
fn parse_tier(s: &str) -> Option<SubscriptionTier> {
    match s {
        "free" => Some(SubscriptionTier::Free),
        "basic" => Some(SubscriptionTier::Basic),
        "starter" => Some(SubscriptionTier::Starter),
        "pro" | "regular" => Some(SubscriptionTier::Pro),
        "team" => Some(SubscriptionTier::Team),
        "enterprise" => Some(SubscriptionTier::Enterprise),
        "cloud" => Some(SubscriptionTier::Cloud),
        _ => None,
    }
}

/// Get the monthly AI credit budget in USD.
///
/// Cloud plan — budget scales with workspace size: `per_user_rate × user_count`.
/// Defaults to a single-user budget when `user_count` is `None` or zero.
pub fn get_credits_limit(tier: SubscriptionTier, user_count: Option<i32>) -> f64 {
    let _ = tier;
    let cfg = &crate::ai_budget::CONFIG;
    cfg.per_user * user_count.unwrap_or(1).max(1) as f64
}

/// Compute credit usage info from `workspace.ai_credits_used_usd`.
///
/// **Fallback path**: Uses the workspace's cached `ai_credits_used_usd` field
/// directly. For accurate billing-period-aware usage, call
/// `BillingService::calculate_credits_info()` and pass the result to
/// `compute_capabilities_with_credits()`.
pub fn get_credits_info(workspace: &Workspace, tier: SubscriptionTier) -> CapabilityCredits {
    // Fallback: no DB access to query active user count, so default to single-user
    // budget as a conservative lower bound. For the accurate multi-user budget,
    // callers should use BillingService::calculate_credits_info().
    let limit = get_credits_limit(tier, None) + workspace.ai_bundle_balance_usd;
    let used = workspace.ai_credits_used_usd;
    let remaining = (limit - used).max(0.0);
    let exhausted = limit == 0.0 || used >= limit;
    let percentage_used = if limit > 0.0 {
        ((used / limit) * 100.0).min(100.0)
    } else {
        0.0 // No budget and no bundles — show 0% (not 100%)
    };

    CapabilityCredits {
        limit_usd: limit,
        used_usd: used,
        remaining_usd: remaining,
        exhausted,
        percentage_used,
    }
}

/// Get the user limit for a workspace.
///
/// Cloud plan — uses `workspace.user_limit` for all tiers, defaulting to
/// 999_999 (effectively unlimited) when not set.
pub fn get_user_limit(workspace: &Workspace, tier: SubscriptionTier) -> i32 {
    let _ = tier;
    workspace.user_limit.unwrap_or(UNLIMITED_USER_LIMIT)
}

// ─── Main entry point ────────────────────────────────────────────────────────

/// Compute the full capabilities for a workspace.
///
/// This is the main entry point, equivalent to Python's
/// `CapabilityService.compute_capabilities()`.
///
/// Field names and values match the Python JSON output exactly.
pub fn compute_capabilities(workspace: &Workspace) -> Capabilities {
    let tier = get_subscription_tier(workspace);
    let credits = get_credits_info(workspace, tier);

    // AI chat is enabled when credits are not exhausted
    let ai_enabled = !credits.exhausted;

    Capabilities {
        subscription_tier: tier,
        subscription_status: workspace.subscription_status,
        billing_enabled: true, // SaaS mode — Stripe billing available
        credits_remaining: credits.remaining_usd,
        credits_limit: credits.limit_usd,
        credits_exhausted: credits.exhausted,

        // AI chat gated by credits only
        ai_chat_enabled: ai_enabled,

        // BigQuery
        bigquery_access_level: "full".to_string(),

        // Cloud plan — all organization features enabled
        multi_user_enabled: true,
        user_management_enabled: true,
        dashboard_sharing_enabled: true,

        // Cloud plan — all data features enabled
        export_enabled: true,
        api_access_enabled: true,

        // Limits — Cloud plan values are the same for all tiers.
        catalog_refresh_limit_per_hour: 5,
        max_dashboards: 0, // unlimited
        query_history_retention_days: 0, // unlimited
        user_limit: get_user_limit(workspace, tier),

        // Cloud plan — all premium flags enabled
        kyomi_watch_enabled: true,
        slack_integration_enabled: true,
        mcp_access_enabled: true,
        pdf_export_enabled: true,
    }
}

/// Compute the full capabilities for a workspace using externally-provided
/// credits information from `BillingService`.
///
/// This is the **preferred** entry point when a database connection is available.
/// Callers should:
/// 1. Call `BillingService::calculate_credits_info()` to get real usage data
/// 2. Convert the result to `CapabilityCredits`
/// 3. Pass it here
pub fn compute_capabilities_with_credits(
    workspace: &Workspace,
    credits: &CapabilityCredits,
) -> Capabilities {
    let tier = get_subscription_tier(workspace);

    // AI chat is enabled when credits are not exhausted
    let ai_enabled = !credits.exhausted;

    Capabilities {
        subscription_tier: tier,
        subscription_status: workspace.subscription_status,
        billing_enabled: true, // SaaS mode — Stripe billing available
        credits_remaining: credits.remaining_usd,
        credits_limit: credits.limit_usd,
        credits_exhausted: credits.exhausted,

        // AI chat gated by credits only
        ai_chat_enabled: ai_enabled,

        // BigQuery
        bigquery_access_level: "full".to_string(),

        // Cloud plan — all organization features enabled
        multi_user_enabled: true,
        user_management_enabled: true,
        dashboard_sharing_enabled: true,

        // Cloud plan — all data features enabled
        export_enabled: true,
        api_access_enabled: true,

        // Limits — Cloud plan values are the same for all tiers.
        catalog_refresh_limit_per_hour: 5,
        max_dashboards: 0, // unlimited
        query_history_retention_days: 0, // unlimited
        user_limit: get_user_limit(workspace, tier),

        // Cloud plan — all premium flags enabled
        kyomi_watch_enabled: true,
        slack_integration_enabled: true,
        mcp_access_enabled: true,
        pdf_export_enabled: true,
    }
}

/// Compute capabilities for self-hosted mode.
///
/// Returns enterprise-level capabilities with unlimited credits and all features
/// enabled. Self-hosted users pay their own LLM provider directly, so there are
/// no credit budgets or tier restrictions.
pub fn compute_capabilities_self_hosted() -> Capabilities {
    Capabilities {
        subscription_tier: SubscriptionTier::Enterprise,
        subscription_status: SubscriptionStatus::Active,
        billing_enabled: false,
        credits_remaining: 999_999.0,
        credits_limit: 999_999.0,
        credits_exhausted: false,

        // AI chat enabled
        ai_chat_enabled: true,

        // BigQuery
        bigquery_access_level: "full".to_string(),

        // Organization features — all enabled
        multi_user_enabled: true,
        user_management_enabled: true,
        dashboard_sharing_enabled: true,

        // Data features
        export_enabled: true,
        api_access_enabled: true,

        // Limits — unlimited (0 = unlimited by convention). Self-hosted gets
        // unlimited catalog refresh (0); Cloud SaaS uses 5 (see compute_capabilities()).
        catalog_refresh_limit_per_hour: 0,
        max_dashboards: 0,
        query_history_retention_days: 0,
        user_limit: 999_999,

        // Premium flags — all enabled
        kyomi_watch_enabled: true,
        slack_integration_enabled: true,
        mcp_access_enabled: true,
        pdf_export_enabled: true,
    }
}

/// Compute minimal free-tier capabilities for SaaS users without a workspace.
///
/// Grants only the baseline feature set — no premium flags, no multi-user,
/// billing enabled (so they can upgrade), and conservative limits. This is
/// the default for an unauthenticated-workspace SaaS user; callers must
/// never substitute [`compute_capabilities_self_hosted`] here, as that
/// grants enterprise permissions to such users.
///
/// Relocated verbatim from `kyomi-ui/src/server_fns/context.rs` in KYO-225 —
/// this crate's module doc calls it the single source of truth for
/// `Capabilities`, so a fourth constructor living in a UI crate was a defect.
pub fn compute_capabilities_free_tier() -> Capabilities {
    Capabilities {
        subscription_tier: SubscriptionTier::Free,
        subscription_status: SubscriptionStatus::Active,
        billing_enabled: true, // SaaS — allow user to see billing/upgrade
        credits_remaining: 0.0,
        credits_limit: 0.0,
        credits_exhausted: true,

        // AI chat disabled — no credits
        ai_chat_enabled: false,

        // BigQuery
        bigquery_access_level: "full".to_string(),

        // Organization features — disabled for free tier
        multi_user_enabled: false,
        user_management_enabled: false,
        dashboard_sharing_enabled: false,

        // Data features
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
            ai_bundle_balance_usd: 0.0,
            analytics_bundle_events: 0,
            user_limit: None,
            stripe_customer_id: None,
            stripe_subscription_id: None,
            settings: None,
            business_knowledge: None,
            knowledge_updated_at: None,
            last_catalog_refresh: None,
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
    fn test_credits_limit_cloud_plan() {
        use SubscriptionTier::*;
        // None and Some(1) both mean single-user budget — they must be equal.
        let single_user = get_credits_limit(Free, None);
        assert!((get_credits_limit(Free, Some(1)) - single_user).abs() < f64::EPSILON);

        // All tiers with the same user_count return the same budget.
        assert!((get_credits_limit(Basic, None) - single_user).abs() < f64::EPSILON);
        assert!((get_credits_limit(Starter, None) - single_user).abs() < f64::EPSILON);
        assert!((get_credits_limit(Pro, None) - single_user).abs() < f64::EPSILON);
        assert!((get_credits_limit(Team, None) - single_user).abs() < f64::EPSILON);
        assert!((get_credits_limit(Enterprise, None) - single_user).abs() < f64::EPSILON);

        // Budget scales with user count: 5 users = 5× single-user budget.
        let five_users = get_credits_limit(Free, Some(5));
        assert!((five_users - single_user * 5.0).abs() < f64::EPSILON);
        assert!((get_credits_limit(Team, Some(5)) - five_users).abs() < f64::EPSILON);

        // Budget scales with user count: 20 users = 20× single-user budget.
        let twenty_users = get_credits_limit(Free, Some(20));
        assert!((twenty_users - single_user * 20.0).abs() < f64::EPSILON);
        assert!((get_credits_limit(Team, Some(20)) - twenty_users).abs() < f64::EPSILON);
    }

    #[test]
    fn test_credits_info_not_exhausted() {
        // Use compute_capabilities_with_credits to provide explicit budget values,
        // independent of env-var-driven tier budgets.
        let ws = test_workspace(SubscriptionTier::Pro, 3.0);
        let credits = CapabilityCredits {
            limit_usd: 9.0,
            used_usd: 3.0,
            remaining_usd: 6.0,
            exhausted: false,
            percentage_used: 33.333333333333336,
        };
        let caps = compute_capabilities_with_credits(&ws, &credits);
        assert!(!caps.credits_exhausted);
        assert!(caps.ai_chat_enabled);
        assert!((caps.credits_remaining - 6.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_credits_info_exhausted() {
        // Single-user workspace budget = per_user_rate × 1. Spend more than
        // the budget to verify exhaustion. Use a value well above $5 so this
        // test stays valid across reasonable per_user_rate changes.
        let per_user_rate = crate::ai_budget::CONFIG.per_user;
        let over_budget = per_user_rate * 2.0; // 2× single-user rate — definitely exhausted
        let ws = test_workspace(SubscriptionTier::Free, over_budget);
        let info = get_credits_info(&ws, SubscriptionTier::Free);
        assert!(info.exhausted);
        assert!((info.remaining_usd - 0.0).abs() < f64::EPSILON);
        assert!((info.percentage_used - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_user_limit_default_unlimited() {
        // Cloud plan — no user_limit set defaults to 999_999
        let ws = test_workspace(SubscriptionTier::Free, 0.0);
        assert_eq!(get_user_limit(&ws, SubscriptionTier::Free), 999_999);
        assert_eq!(get_user_limit(&ws, SubscriptionTier::Basic), 999_999);
        assert_eq!(get_user_limit(&ws, SubscriptionTier::Pro), 999_999);
        assert_eq!(get_user_limit(&ws, SubscriptionTier::Enterprise), 999_999);
    }

    #[test]
    fn test_user_limit_custom() {
        // Cloud plan — workspace.user_limit is respected for all tiers
        let mut ws = test_workspace(SubscriptionTier::Free, 0.0);
        ws.user_limit = Some(10);
        assert_eq!(get_user_limit(&ws, SubscriptionTier::Free), 10);
        assert_eq!(get_user_limit(&ws, SubscriptionTier::Team), 10);
        assert_eq!(get_user_limit(&ws, SubscriptionTier::Enterprise), 10);
    }

    #[test]
    fn test_compute_capabilities_free_tier_cloud() {
        let ws = test_workspace(SubscriptionTier::Free, 0.0);
        let credits = CapabilityCredits {
            limit_usd: 1.0,
            used_usd: 0.0,
            remaining_usd: 1.0,
            exhausted: false,
            percentage_used: 0.0,
        };
        let caps = compute_capabilities_with_credits(&ws, &credits);

        assert_eq!(caps.subscription_tier, SubscriptionTier::Free);
        assert!(caps.ai_chat_enabled); // Not exhausted
        // Cloud plan — all features enabled regardless of tier
        assert!(caps.multi_user_enabled);
        assert!(caps.kyomi_watch_enabled);
        assert!(caps.slack_integration_enabled);
        assert!(caps.mcp_access_enabled);
        assert_eq!(caps.max_dashboards, 0); // unlimited
        assert_eq!(caps.catalog_refresh_limit_per_hour, 5);
        assert_eq!(caps.query_history_retention_days, 0); // unlimited
        assert_eq!(caps.user_limit, 999_999); // default unlimited
    }

    #[test]
    fn test_compute_capabilities_pro_tier() {
        let ws = test_workspace(SubscriptionTier::Pro, 0.0);
        let credits = CapabilityCredits {
            limit_usd: 9.0,
            used_usd: 0.0,
            remaining_usd: 9.0,
            exhausted: false,
            percentage_used: 0.0,
        };
        let caps = compute_capabilities_with_credits(&ws, &credits);

        assert_eq!(caps.subscription_tier, SubscriptionTier::Pro);
        assert!(caps.ai_chat_enabled);
        assert!(caps.kyomi_watch_enabled);
        assert!(caps.api_access_enabled);
        assert!(caps.mcp_access_enabled);
        // Cloud plan — all features enabled
        assert!(caps.multi_user_enabled);
        assert!(caps.slack_integration_enabled);
        assert_eq!(caps.max_dashboards, 0); // unlimited
        assert_eq!(caps.query_history_retention_days, 0); // unlimited
    }

    #[test]
    fn test_compute_capabilities_team_tier() {
        let mut ws = test_workspace(SubscriptionTier::Team, 0.0);
        ws.user_limit = Some(5);
        let caps = compute_capabilities(&ws);

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
        let caps = compute_capabilities(&ws);

        assert!(caps.credits_exhausted);
        assert!(!caps.ai_chat_enabled);
    }

    #[test]
    fn test_capabilities_json_field_names_match_python() {
        let ws = test_workspace(SubscriptionTier::Free, 0.0);
        let caps = compute_capabilities(&ws);
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
            "bigquery_access_level",
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
        let credits = CapabilityCredits {
            limit_usd: 9.0,
            used_usd: 4.5,
            remaining_usd: 4.5,
            exhausted: false,
            percentage_used: 50.0,
        };
        let caps = compute_capabilities_with_credits(&ws, &credits);

        assert_eq!(caps.subscription_tier, SubscriptionTier::Pro);
        assert!(caps.ai_chat_enabled);
        assert!(!caps.credits_exhausted);
        assert!((caps.credits_remaining - 4.5).abs() < f64::EPSILON);
        assert!((caps.credits_limit - 9.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_capabilities_with_credits_exhausted() {
        let ws = test_workspace(SubscriptionTier::Pro, 0.0);
        let credits = CapabilityCredits {
            limit_usd: 9.0,
            used_usd: 9.5,
            remaining_usd: 0.0,
            exhausted: true,
            percentage_used: 100.0,
        };
        let caps = compute_capabilities_with_credits(&ws, &credits);

        assert!(caps.credits_exhausted);
        assert!(!caps.ai_chat_enabled);
    }

    #[test]
    fn test_compute_capabilities_self_hosted() {
        let caps = compute_capabilities_self_hosted();
        assert_eq!(caps.subscription_tier, SubscriptionTier::Enterprise);
        assert_eq!(caps.subscription_status, SubscriptionStatus::Active);
        assert!(!caps.billing_enabled);
        assert!(!caps.credits_exhausted);

        // AI chat enabled
        assert!(caps.ai_chat_enabled);

        // BigQuery
        assert_eq!(caps.bigquery_access_level, "full");

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
        assert!(caps.slack_integration_enabled);
        assert!(caps.mcp_access_enabled);
        assert!(caps.pdf_export_enabled);

        // Credits — effectively unlimited
        assert!((caps.credits_remaining - 999_999.0).abs() < f64::EPSILON);
        assert!((caps.credits_limit - 999_999.0).abs() < f64::EPSILON);
    }

    #[test]
    fn free_tier_capabilities_are_restrictive() {
        let caps = compute_capabilities_free_tier();

        assert_eq!(caps.subscription_tier, SubscriptionTier::Free);
        assert!(caps.billing_enabled, "SaaS free tier should show billing UI");
        assert!(caps.credits_exhausted, "No credits by default");

        // AI chat off
        assert!(!caps.ai_chat_enabled);

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

    /// The invariant the `context.rs` call site's warning exists to protect:
    /// an unauthenticated-workspace SaaS user (free tier) must never receive
    /// a capability that self-hosted mode grants — that would be the exact
    /// "enterprise permissions to unauthenticated-workspace SaaS users" leak
    /// the warning calls out.
    ///
    /// `billing_enabled` and `credits_exhausted` are deliberately excluded:
    /// `billing_enabled` is a UI-visibility toggle (free tier shows a billing
    /// upsell; self-hosted has no billing at all), and `credits_exhausted`
    /// uses inverted true-means-restricted semantics. Neither is a capability
    /// grant, so "permissive" doesn't apply to them the same way as the
    /// feature-gating flags below.
    ///
    /// Flags are enumerated individually rather than looped so that adding a
    /// new boolean to `Capabilities` does not silently start passing this
    /// test without a human deciding which side of the invariant it belongs
    /// on.
    #[test]
    fn free_tier_never_grants_more_than_self_hosted() {
        let free = compute_capabilities_free_tier();
        let self_hosted = compute_capabilities_self_hosted();

        // `bool` is `Ord` with `false < true`, so `<=` is exactly "free tier
        // does not grant more than self-hosted" for each flag.
        assert!(free.ai_chat_enabled <= self_hosted.ai_chat_enabled);
        assert!(free.multi_user_enabled <= self_hosted.multi_user_enabled);
        assert!(free.user_management_enabled <= self_hosted.user_management_enabled);
        assert!(free.dashboard_sharing_enabled <= self_hosted.dashboard_sharing_enabled);
        assert!(free.export_enabled <= self_hosted.export_enabled);
        assert!(free.api_access_enabled <= self_hosted.api_access_enabled);
        assert!(free.kyomi_watch_enabled <= self_hosted.kyomi_watch_enabled);
        assert!(free.slack_integration_enabled <= self_hosted.slack_integration_enabled);
        assert!(free.mcp_access_enabled <= self_hosted.mcp_access_enabled);
        assert!(free.pdf_export_enabled <= self_hosted.pdf_export_enabled);

        // And strictly less overall, not merely "not more": self-hosted
        // grants capabilities free tier has none of, so the two sets are not
        // accidentally identical.
        assert!(!free.ai_chat_enabled && self_hosted.ai_chat_enabled);
        assert!(!free.multi_user_enabled && self_hosted.multi_user_enabled);
        assert!(!free.user_management_enabled && self_hosted.user_management_enabled);
        assert!(!free.dashboard_sharing_enabled && self_hosted.dashboard_sharing_enabled);
        assert!(!free.api_access_enabled && self_hosted.api_access_enabled);
        assert!(!free.kyomi_watch_enabled && self_hosted.kyomi_watch_enabled);
        assert!(!free.slack_integration_enabled && self_hosted.slack_integration_enabled);
        assert!(!free.pdf_export_enabled && self_hosted.pdf_export_enabled);
    }

    /// The four limits that deliberately differ from `compute_capabilities()`
    /// / `compute_capabilities_self_hosted()`'s unlimited (`0`) convention.
    /// If a future "simplification" collapses free tier toward the unlimited
    /// default, this fails loudly instead of silently granting unlimited
    /// dashboards/history/refreshes/seats to unauthenticated-workspace users.
    #[test]
    fn free_tier_limits_differ_from_unlimited_defaults() {
        let caps = compute_capabilities_free_tier();

        assert_eq!(caps.max_dashboards, 5);
        assert_eq!(caps.query_history_retention_days, 7);
        assert_eq!(caps.catalog_refresh_limit_per_hour, 1);
        assert_eq!(caps.user_limit, 1);
    }

    /// Every one of the 21 `Capabilities` field values, pinned against the
    /// literal that lived at `kyomi-ui/src/server_fns/context.rs:168` before
    /// KYO-225 relocated it here (verified via `git show
    /// main:crates/kyomi-ui/src/server_fns/context.rs`). Proves the
    /// relocation changed nothing.
    #[test]
    fn free_tier_capabilities_match_pre_relocation_literal() {
        let caps = compute_capabilities_free_tier();

        assert_eq!(caps.subscription_tier, SubscriptionTier::Free);
        assert_eq!(caps.subscription_status, SubscriptionStatus::Active);
        assert!(caps.billing_enabled);
        assert_eq!(caps.credits_remaining, 0.0);
        assert_eq!(caps.credits_limit, 0.0);
        assert!(caps.credits_exhausted);

        assert!(!caps.ai_chat_enabled);

        assert_eq!(caps.bigquery_access_level, "full");

        assert!(!caps.multi_user_enabled);
        assert!(!caps.user_management_enabled);
        assert!(!caps.dashboard_sharing_enabled);

        assert!(caps.export_enabled);
        assert!(!caps.api_access_enabled);

        assert_eq!(caps.catalog_refresh_limit_per_hour, 1);
        assert_eq!(caps.max_dashboards, 5);
        assert_eq!(caps.query_history_retention_days, 7);
        assert_eq!(caps.user_limit, 1);

        assert!(!caps.kyomi_watch_enabled);
        assert!(!caps.slack_integration_enabled);
        assert!(caps.mcp_access_enabled);
        assert!(!caps.pdf_export_enabled);
    }
}
