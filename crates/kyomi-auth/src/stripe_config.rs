// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stripe product and price ID configuration.
//!
//! Price IDs are read from environment variables so they don't appear in
//! source code. Self-hosted deployments that don't use Stripe can ignore
//! these entirely.
//!
//! Environment variables:
//! - `STRIPE_CLOUD_MONTHLY` — Cloud plan price ID ($5/user/month)
//! - `STRIPE_AI_BUNDLE` — AI token bundle price ID (one-time purchase)
//! - `STRIPE_ANALYTICS_BUNDLE` — Analytics event bundle price ID (one-time purchase)
//!
//! Set the price IDs matching your Stripe environment (test or live).
//! The app only runs in one mode at a time — no test/prod split needed.

use std::sync::LazyLock;

// ─── Config loaded from env ────────────────────────────────────────────────

struct StripePrices {
    /// Cloud plan — $5/user/month
    cloud_monthly: Option<String>,
    /// AI token bundle — one-time purchase
    ai_bundle: Option<String>,
    /// Analytics event bundle — one-time purchase
    analytics_bundle: Option<String>,
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

static PRICES: LazyLock<StripePrices> = LazyLock::new(|| StripePrices {
    cloud_monthly: env_opt("STRIPE_CLOUD_MONTHLY"),
    ai_bundle: env_opt("STRIPE_AI_BUNDLE"),
    analytics_bundle: env_opt("STRIPE_ANALYTICS_BUNDLE"),
});

// ─── Public API ─────────────────────────────────────────────────────────────

/// Check whether a Stripe secret key is a test-mode key.
///
/// Used for logging only — price IDs are environment-configured and not
/// split by test/prod mode.
pub fn is_test_mode(secret_key: &str) -> bool {
    secret_key.starts_with("sk_test_")
}

/// Get the Cloud plan price ID.
pub fn get_cloud_price_id() -> Option<&'static str> {
    PRICES.cloud_monthly.as_deref()
}

/// Get the AI token bundle price ID for one-time purchase.
pub fn get_ai_bundle_price_id() -> Option<&'static str> {
    PRICES.ai_bundle.as_deref()
}

/// Get the analytics event bundle price ID.
pub fn get_analytics_bundle_price_id() -> Option<&'static str> {
    PRICES.analytics_bundle.as_deref()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_test_mode() {
        assert!(is_test_mode("sk_test_abc123"));
        assert!(!is_test_mode("sk_live_abc123"));
        assert!(!is_test_mode(""));
    }

    #[test]
    fn test_all_lookups_return_none_without_env() {
        assert!(get_cloud_price_id().is_none());
        assert!(get_ai_bundle_price_id().is_none());
        assert!(get_analytics_bundle_price_id().is_none());
    }
}
