// SPDX-License-Identifier: AGPL-3.0-or-later

//! AI credit budget configuration.
//!
//! Budget values are read from environment variables at startup. These only
//! matter for SaaS deployments — self-hosted mode bypasses budgets entirely
//! via `compute_capabilities_self_hosted()`.
//!
//! Environment variables:
//! - `AI_BUDGET_PER_USER` — monthly USD budget per active workspace user (default: $5.00)

use std::sync::LazyLock;

/// Parsed budget configuration, loaded once from environment.
pub struct AiBudgetConfig {
    /// Per-user monthly budget in USD. Total workspace budget = per_user × active_user_count.
    /// If 0, only BYOK or purchased bundles provide credits.
    pub per_user: f64,
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Global budget configuration. Loaded once from env vars.
///
/// Defaults to `5.0` ($5/user/month). Override with `AI_BUDGET_PER_USER`.
/// Self-hosted mode never reads these values.
pub static CONFIG: LazyLock<AiBudgetConfig> = LazyLock::new(|| AiBudgetConfig {
    per_user: env_f64("AI_BUDGET_PER_USER", 5.0),
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_f64_default() {
        // Non-existent env var returns default
        assert!((env_f64("__NONEXISTENT_TEST_VAR__", 42.0) - 42.0).abs() < f64::EPSILON);
    }
}
