// SPDX-License-Identifier: AGPL-3.0-or-later

//! AI credit budget configuration.
//!
//! Budget values are read from environment variables at startup. These only
//! matter for SaaS deployments — self-hosted mode bypasses budgets entirely
//! via `compute_capabilities_self_hosted()`.
//!
//! Environment variables:
//! - `AI_BUDGET_CLOUD` — monthly USD budget for the Cloud plan

use std::sync::LazyLock;

/// Parsed budget configuration, loaded once from environment.
pub struct AiBudgetConfig {
    /// Single Cloud plan budget in USD. If 0, only BYOK or purchased bundles provide credits.
    pub cloud: f64,
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Global budget configuration. Loaded once from env vars.
///
/// Defaults to `0.0` — SaaS deployments MUST set `AI_BUDGET_CLOUD`.
/// Self-hosted mode never reads these values.
pub static CONFIG: LazyLock<AiBudgetConfig> = LazyLock::new(|| AiBudgetConfig {
    cloud: env_f64("AI_BUDGET_CLOUD", 0.0),
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
