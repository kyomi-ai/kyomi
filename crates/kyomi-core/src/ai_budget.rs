// SPDX-License-Identifier: AGPL-3.0-or-later

//! AI credit budget configuration.
//!
//! Budget values are read from environment variables at startup. These only
//! matter for SaaS deployments — self-hosted mode bypasses budgets entirely
//! via `compute_capabilities_self_hosted()`.
//!
//! Environment variables:
//! - `AI_BUDGET_FREE` — monthly USD budget for Free tier
//! - `AI_BUDGET_STARTER` — monthly USD budget for Starter/Basic tier
//! - `AI_BUDGET_PRO` — monthly USD budget for Pro tier
//! - `AI_BUDGET_TEAM_BASE` — base USD budget for Team tier
//! - `AI_BUDGET_TEAM_PER_USER` — additional USD per user beyond base count
//! - `AI_BUDGET_TEAM_BASE_USERS` — number of users included in base budget
//! - `AI_BUDGET_ENTERPRISE` — monthly USD budget for Enterprise tier

use std::sync::LazyLock;

/// Parsed budget configuration, loaded once from environment.
pub struct AiBudgetConfig {
    pub free: f64,
    pub starter: f64,
    pub pro: f64,
    pub team_base: f64,
    pub team_per_user: f64,
    pub team_base_users: i32,
    pub enterprise: f64,
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_i32(key: &str, default: i32) -> i32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Global budget configuration. Loaded once from env vars.
///
/// Defaults to `0.0` for all tiers — SaaS deployments MUST set the env vars.
/// Self-hosted mode never reads these values.
pub static CONFIG: LazyLock<AiBudgetConfig> = LazyLock::new(|| AiBudgetConfig {
    free: env_f64("AI_BUDGET_FREE", 0.0),
    starter: env_f64("AI_BUDGET_STARTER", 0.0),
    pro: env_f64("AI_BUDGET_PRO", 0.0),
    team_base: env_f64("AI_BUDGET_TEAM_BASE", 0.0),
    team_per_user: env_f64("AI_BUDGET_TEAM_PER_USER", 0.0),
    team_base_users: env_i32("AI_BUDGET_TEAM_BASE_USERS", 5),
    enterprise: env_f64("AI_BUDGET_ENTERPRISE", 0.0),
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_f64_default() {
        // Non-existent env var returns default
        assert!((env_f64("__NONEXISTENT_TEST_VAR__", 42.0) - 42.0).abs() < f64::EPSILON);
    }

    #[test]
    fn env_i32_default() {
        assert_eq!(env_i32("__NONEXISTENT_TEST_VAR__", 7), 7);
    }
}
