// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stripe product and price ID configuration.
//!
//! Price IDs are read from environment variables so they don't appear in
//! source code. Self-hosted deployments that don't use Stripe can ignore
//! these entirely.
//!
//! Environment variables (test mode):
//! - `STRIPE_TEST_CLOUD_MONTHLY`
//! - `STRIPE_TEST_AI_BUNDLE`
//! - `STRIPE_TEST_ANALYTICS_BUNDLE`
//!
//! Environment variables (production mode):
//! - `STRIPE_PROD_CLOUD_MONTHLY`
//! - `STRIPE_PROD_AI_BUNDLE`
//! - `STRIPE_PROD_ANALYTICS_BUNDLE`

use std::sync::LazyLock;

// ─── Config loaded from env ────────────────────────────────────────────────

struct StripePrices {
    /// Cloud plan — $5/user/month
    test_cloud_monthly: Option<String>,
    prod_cloud_monthly: Option<String>,
    /// AI token bundle — one-time purchase
    test_ai_bundle: Option<String>,
    prod_ai_bundle: Option<String>,
    /// Analytics event bundle — recurring add-on
    test_analytics_bundle: Option<String>,
    prod_analytics_bundle: Option<String>,
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

static PRICES: LazyLock<StripePrices> = LazyLock::new(|| StripePrices {
    test_cloud_monthly: env_opt("STRIPE_TEST_CLOUD_MONTHLY"),
    prod_cloud_monthly: env_opt("STRIPE_PROD_CLOUD_MONTHLY"),
    test_ai_bundle: env_opt("STRIPE_TEST_AI_BUNDLE"),
    prod_ai_bundle: env_opt("STRIPE_PROD_AI_BUNDLE"),
    test_analytics_bundle: env_opt("STRIPE_TEST_ANALYTICS_BUNDLE"),
    prod_analytics_bundle: env_opt("STRIPE_PROD_ANALYTICS_BUNDLE"),
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

/// Get the Cloud plan price ID.
/// `tier` and `billing_cycle` params are kept for backward compatibility
/// but ignored — there's only one price now.
pub fn get_price_id(tier: &str, billing_cycle: &str, is_test: bool) -> Option<&'static str> {
    let _ = (tier, billing_cycle);
    let p = &*PRICES;
    let opt = if is_test { &p.test_cloud_monthly } else { &p.prod_cloud_monthly };
    opt.as_deref()
}

/// Get the AI token bundle price ID for one-time purchase.
pub fn get_ai_bundle_price_id(is_test: bool) -> Option<&'static str> {
    let p = &*PRICES;
    let opt = if is_test { &p.test_ai_bundle } else { &p.prod_ai_bundle };
    opt.as_deref()
}

/// Get the analytics event bundle price ID.
pub fn get_analytics_bundle_price_id(is_test: bool) -> Option<&'static str> {
    let p = &*PRICES;
    let opt = if is_test { &p.test_analytics_bundle } else { &p.prod_analytics_bundle };
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
    fn test_get_price_id_returns_cloud_regardless_of_tier() {
        // All tier/cycle combinations resolve to the same Cloud price.
        // Without env vars set, that's None.
        let free = get_price_id("free", "monthly", true);
        let enterprise = get_price_id("enterprise", "annual", true);
        let starter = get_price_id("starter", "monthly", true);
        assert_eq!(free, enterprise);
        assert_eq!(free, starter);
    }

    #[test]
    fn test_get_price_id_ignores_tier_and_cycle() {
        // All combinations should return the same value (the Cloud price)
        let a = get_price_id("starter", "annual", true);
        let b = get_price_id("pro", "monthly", true);
        let c = get_price_id("team", "annual", true);
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn test_ai_bundle_price_id_without_env() {
        // Without env vars, returns None
        assert!(get_ai_bundle_price_id(true).is_none());
        assert!(get_ai_bundle_price_id(false).is_none());
    }

    #[test]
    fn test_analytics_bundle_price_id_without_env() {
        // Without env vars, returns None
        assert!(get_analytics_bundle_price_id(true).is_none());
        assert!(get_analytics_bundle_price_id(false).is_none());
    }
}
