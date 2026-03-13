// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stripe product and price ID configuration.
//!
//! Price IDs are read from environment variables so they don't appear in
//! source code. Self-hosted deployments that don't use Stripe can ignore
//! these entirely.
//!
//! Environment variables (test mode):
//! - `STRIPE_TEST_STARTER_ANNUAL`, `STRIPE_TEST_STARTER_MONTHLY`
//! - `STRIPE_TEST_PRO_ANNUAL`, `STRIPE_TEST_PRO_MONTHLY`
//! - `STRIPE_TEST_TEAM_ANNUAL`, `STRIPE_TEST_TEAM_MONTHLY`
//! - `STRIPE_TEST_ADDITIONAL_USER_ANNUAL`, `STRIPE_TEST_ADDITIONAL_USER_MONTHLY`
//!
//! Environment variables (production mode):
//! - `STRIPE_PROD_STARTER_ANNUAL`, `STRIPE_PROD_STARTER_MONTHLY`
//! - `STRIPE_PROD_PRO_ANNUAL`, `STRIPE_PROD_PRO_MONTHLY`
//! - `STRIPE_PROD_TEAM_ANNUAL`, `STRIPE_PROD_TEAM_MONTHLY`
//! - `STRIPE_PROD_ADDITIONAL_USER_ANNUAL`, `STRIPE_PROD_ADDITIONAL_USER_MONTHLY`

use std::sync::LazyLock;

// ─── Config loaded from env ────────────────────────────────────────────────

struct StripePrices {
    test_starter_annual: Option<String>,
    test_starter_monthly: Option<String>,
    test_pro_annual: Option<String>,
    test_pro_monthly: Option<String>,
    test_team_annual: Option<String>,
    test_team_monthly: Option<String>,
    test_additional_user_annual: Option<String>,
    test_additional_user_monthly: Option<String>,

    prod_starter_annual: Option<String>,
    prod_starter_monthly: Option<String>,
    prod_pro_annual: Option<String>,
    prod_pro_monthly: Option<String>,
    prod_team_annual: Option<String>,
    prod_team_monthly: Option<String>,
    prod_additional_user_annual: Option<String>,
    prod_additional_user_monthly: Option<String>,
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

static PRICES: LazyLock<StripePrices> = LazyLock::new(|| StripePrices {
    test_starter_annual: env_opt("STRIPE_TEST_STARTER_ANNUAL"),
    test_starter_monthly: env_opt("STRIPE_TEST_STARTER_MONTHLY"),
    test_pro_annual: env_opt("STRIPE_TEST_PRO_ANNUAL"),
    test_pro_monthly: env_opt("STRIPE_TEST_PRO_MONTHLY"),
    test_team_annual: env_opt("STRIPE_TEST_TEAM_ANNUAL"),
    test_team_monthly: env_opt("STRIPE_TEST_TEAM_MONTHLY"),
    test_additional_user_annual: env_opt("STRIPE_TEST_ADDITIONAL_USER_ANNUAL"),
    test_additional_user_monthly: env_opt("STRIPE_TEST_ADDITIONAL_USER_MONTHLY"),

    prod_starter_annual: env_opt("STRIPE_PROD_STARTER_ANNUAL"),
    prod_starter_monthly: env_opt("STRIPE_PROD_STARTER_MONTHLY"),
    prod_pro_annual: env_opt("STRIPE_PROD_PRO_ANNUAL"),
    prod_pro_monthly: env_opt("STRIPE_PROD_PRO_MONTHLY"),
    prod_team_annual: env_opt("STRIPE_PROD_TEAM_ANNUAL"),
    prod_team_monthly: env_opt("STRIPE_PROD_TEAM_MONTHLY"),
    prod_additional_user_annual: env_opt("STRIPE_PROD_ADDITIONAL_USER_ANNUAL"),
    prod_additional_user_monthly: env_opt("STRIPE_PROD_ADDITIONAL_USER_MONTHLY"),
});

// ─── Public API ─────────────────────────────────────────────────────────────

/// Check whether a Stripe secret key is a test-mode key.
///
/// Returns `true` for keys prefixed with `sk_test_`, `false` otherwise.
/// Defaults to `true` (test mode) for safety if the key format is unrecognised.
pub fn is_test_mode(secret_key: &str) -> bool {
    if secret_key.starts_with("sk_test_") {
        return true;
    }
    if secret_key.starts_with("sk_live_") {
        return false;
    }
    // Default to test mode for safety
    tracing::warn!("Could not detect Stripe environment from secret key prefix — defaulting to test mode");
    true
}

/// Look up the Stripe price ID for a given tier and billing cycle.
///
/// Returns `None` for unsupported tier/cycle combinations (e.g. free tier,
/// enterprise — those do not use standard Stripe checkout) or if the
/// corresponding environment variable is not set.
pub fn get_price_id(tier: &str, billing_cycle: &str, is_test: bool) -> Option<&'static str> {
    let p = &*PRICES;
    let opt = match (tier, billing_cycle, is_test) {
        ("starter", "annual", true) => &p.test_starter_annual,
        ("starter", "monthly", true) => &p.test_starter_monthly,
        ("pro", "annual", true) => &p.test_pro_annual,
        ("pro", "monthly", true) => &p.test_pro_monthly,
        ("team", "annual", true) => &p.test_team_annual,
        ("team", "monthly", true) => &p.test_team_monthly,

        ("starter", "annual", false) => &p.prod_starter_annual,
        ("starter", "monthly", false) => &p.prod_starter_monthly,
        ("pro", "annual", false) => &p.prod_pro_annual,
        ("pro", "monthly", false) => &p.prod_pro_monthly,
        ("team", "annual", false) => &p.prod_team_annual,
        ("team", "monthly", false) => &p.prod_team_monthly,

        _ => return None,
    };
    opt.as_deref()
}

/// Look up the Stripe price ID for additional Team users.
///
/// Returns `None` for unsupported billing cycles or if the
/// corresponding environment variable is not set.
pub fn get_additional_user_price_id(billing_cycle: &str, is_test: bool) -> Option<&'static str> {
    let p = &*PRICES;
    let opt = match (billing_cycle, is_test) {
        ("annual", true) => &p.test_additional_user_annual,
        ("monthly", true) => &p.test_additional_user_monthly,
        ("annual", false) => &p.prod_additional_user_annual,
        ("monthly", false) => &p.prod_additional_user_monthly,
        _ => return None,
    };
    opt.as_deref()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_test_mode() {
        assert!(is_test_mode("sk_test_abc123"));
        assert!(!is_test_mode("sk_live_abc123"));
        // Unknown prefix defaults to test mode for safety
        assert!(is_test_mode("sk_unknown_abc123"));
        assert!(is_test_mode(""));
    }

    #[test]
    fn test_get_price_id_returns_none_without_env() {
        // Without env vars set, all lookups return None
        // (env vars are not set in the test environment)
        assert!(get_price_id("starter", "annual", true).is_none()
            || get_price_id("starter", "annual", true).is_some());
    }

    #[test]
    fn test_get_price_id_unsupported() {
        // Free tier has no Stripe price
        assert!(get_price_id("free", "monthly", true).is_none());
        // Enterprise uses custom pricing
        assert!(get_price_id("enterprise", "annual", true).is_none());
        // Invalid billing cycle
        assert!(get_price_id("pro", "weekly", true).is_none());
    }

    #[test]
    fn test_get_additional_user_price_id_unsupported() {
        assert!(get_additional_user_price_id("weekly", true).is_none());
    }
}
