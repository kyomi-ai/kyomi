// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stripe product and price ID configuration.
//!
//! Automatically detects test vs production mode based on the
//! `STRIPE_SECRET_KEY` prefix (`sk_test_` vs `sk_live_`).
//!
//! See pricing page for current tier details.

// ─── Test/Development Stripe prices ─────────────────────────────────────────

/// Price IDs for Stripe test mode (sk_test_ keys).
const TEST_STARTER_ANNUAL: &str = "price_1SOFKB2tFy6p684MXnUHqq8k";
const TEST_STARTER_MONTHLY: &str = "price_1SOFLN2tFy6p684Mad32u8Ce";
const TEST_PRO_ANNUAL: &str = "price_1SOFLn2tFy6p684MQFJRIgYN";
const TEST_PRO_MONTHLY: &str = "price_1SOFMe2tFy6p684M9GMRQSTz";
const TEST_TEAM_ANNUAL: &str = "price_1SOFN02tFy6p684MTbX2tY2z";
const TEST_TEAM_MONTHLY: &str = "price_1SOFNh2tFy6p684MAuvNA3Rn";
const TEST_ADDITIONAL_USER_ANNUAL: &str = "price_1SOFZ82tFy6p684Musawmz45";
const TEST_ADDITIONAL_USER_MONTHLY: &str = "price_1SOFay2tFy6p684MgLnRcO8l";

// ─── Production Stripe prices ───────────────────────────────────────────────

/// Price IDs for Stripe production mode (sk_live_ keys).
const PROD_STARTER_ANNUAL: &str = "price_1SO9Aa2xIs7Ty6uWgppqAjTb";
const PROD_STARTER_MONTHLY: &str = "price_1SO9BL2xIs7Ty6uWVL6eE9g6";
const PROD_PRO_ANNUAL: &str = "price_1SO9D82xIs7Ty6uWZU0rNzMA";
const PROD_PRO_MONTHLY: &str = "price_1SO9Dw2xIs7Ty6uWIDp1eIcJ";
const PROD_TEAM_ANNUAL: &str = "price_1SO9EQ2xIs7Ty6uW0KsJd4g3";
const PROD_TEAM_MONTHLY: &str = "price_1SO9FF2xIs7Ty6uWKEdhLdHW";
const PROD_ADDITIONAL_USER_ANNUAL: &str = "price_1SUNGH2xIs7Ty6uW5Oiib1u1";
const PROD_ADDITIONAL_USER_MONTHLY: &str = "price_1SUNFV2xIs7Ty6uWwUC7bvrk";

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
/// enterprise — those do not use standard Stripe checkout).
pub fn get_price_id(tier: &str, billing_cycle: &str, is_test: bool) -> Option<&'static str> {
    match (tier, billing_cycle, is_test) {
        // Test mode
        ("starter", "annual", true) => Some(TEST_STARTER_ANNUAL),
        ("starter", "monthly", true) => Some(TEST_STARTER_MONTHLY),
        ("pro", "annual", true) => Some(TEST_PRO_ANNUAL),
        ("pro", "monthly", true) => Some(TEST_PRO_MONTHLY),
        ("team", "annual", true) => Some(TEST_TEAM_ANNUAL),
        ("team", "monthly", true) => Some(TEST_TEAM_MONTHLY),

        // Production mode
        ("starter", "annual", false) => Some(PROD_STARTER_ANNUAL),
        ("starter", "monthly", false) => Some(PROD_STARTER_MONTHLY),
        ("pro", "annual", false) => Some(PROD_PRO_ANNUAL),
        ("pro", "monthly", false) => Some(PROD_PRO_MONTHLY),
        ("team", "annual", false) => Some(PROD_TEAM_ANNUAL),
        ("team", "monthly", false) => Some(PROD_TEAM_MONTHLY),

        _ => None,
    }
}

/// Look up the Stripe price ID for additional Team users.
///
/// Returns `None` for unsupported billing cycles.
pub fn get_additional_user_price_id(billing_cycle: &str, is_test: bool) -> Option<&'static str> {
    match (billing_cycle, is_test) {
        ("annual", true) => Some(TEST_ADDITIONAL_USER_ANNUAL),
        ("monthly", true) => Some(TEST_ADDITIONAL_USER_MONTHLY),
        ("annual", false) => Some(PROD_ADDITIONAL_USER_ANNUAL),
        ("monthly", false) => Some(PROD_ADDITIONAL_USER_MONTHLY),
        _ => None,
    }
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
    fn test_get_price_id_test_mode() {
        assert_eq!(
            get_price_id("starter", "annual", true),
            Some(TEST_STARTER_ANNUAL)
        );
        assert_eq!(
            get_price_id("pro", "monthly", true),
            Some(TEST_PRO_MONTHLY)
        );
        assert_eq!(
            get_price_id("team", "annual", true),
            Some(TEST_TEAM_ANNUAL)
        );
    }

    #[test]
    fn test_get_price_id_prod_mode() {
        assert_eq!(
            get_price_id("starter", "annual", false),
            Some(PROD_STARTER_ANNUAL)
        );
        assert_eq!(
            get_price_id("pro", "monthly", false),
            Some(PROD_PRO_MONTHLY)
        );
        assert_eq!(
            get_price_id("team", "monthly", false),
            Some(PROD_TEAM_MONTHLY)
        );
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
    fn test_get_additional_user_price_id() {
        assert_eq!(
            get_additional_user_price_id("annual", true),
            Some(TEST_ADDITIONAL_USER_ANNUAL)
        );
        assert_eq!(
            get_additional_user_price_id("monthly", false),
            Some(PROD_ADDITIONAL_USER_MONTHLY)
        );
        assert!(get_additional_user_price_id("weekly", true).is_none());
    }
}
