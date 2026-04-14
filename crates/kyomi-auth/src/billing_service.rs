// SPDX-License-Identifier: AGPL-3.0-or-later

//! Billing service — subscription-aware AI usage aggregation.
//!
//! Calculates billing metrics (spent vs budget, percentage used, exhausted
//! status) for subscription tiers. Uses dollar-based budgets (not tokens)
//! for flexibility across different AI models.
//!
//! This is the Rust equivalent of Python's `billing_service.py`.
//!
//! Separation of concerns:
//! - `api_usage_logger` (agent crate): Logs individual LLM calls to database
//! - `billing_service` (this module): Subscription/billing-specific metrics
//! - `capability` (core crate): Feature gating that consumes billing data

use chrono::{DateTime, Datelike, Timelike, Utc};
use kyomi_core::DbPool;
use serde::{Deserialize, Serialize};

// ─── Budget config ─────────────────────────────────────────────────────────
//
// Budget values come from environment variables via `kyomi_core::ai_budget::CONFIG`.
// See that module for the env var names and defaults.

// ─── Public types ───────────────────────────────────────────────────────────

/// Aggregated usage statistics for a billing period.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageStats {
    pub total_cost_usd: f64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_calls: i64,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
}

/// Credit usage information for a workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditsInfo {
    pub limit_usd: f64,
    pub used_usd: f64,
    pub remaining_usd: f64,
    pub percentage_used: f64,
    pub exhausted: bool,
    pub total_calls: i64,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
}

/// Whether AI usage is allowed and any warning level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageAllowed {
    pub allowed: bool,
    pub blocked: bool,
    pub warning_level: Option<String>,
    pub percentage_used: f64,
    pub message: Option<String>,
}

/// Per-user fair-share usage info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerUserUsage {
    pub percentage_used: f64,
    pub fair_share_percentage: f64,
}

/// Full AI usage status for a workspace/user, matching the Python
/// `/billing/ai-usage-status` response shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiUsageStatus {
    pub percentage_used: f64,
    pub warning_level: Option<String>,
    pub allowed: bool,
    pub blocked: bool,
    pub ai_reset_date: Option<DateTime<Utc>>,
    pub trial_ends_at: Option<DateTime<Utc>>,
    pub per_user: PerUserUsage,
    pub by_feature: std::collections::HashMap<String, f64>,
}

// ─── Internal row types ─────────────────────────────────────────────────────

/// Row returned by the aggregate usage query.
#[derive(Debug, sqlx::FromRow)]
struct UsageRow {
    total_cost_usd: Option<f64>,
    total_input_tokens: Option<i64>,
    total_output_tokens: Option<i64>,
    total_calls: Option<i64>,
}

/// Row returned by the per-feature usage query.
#[derive(Debug, sqlx::FromRow)]
struct FeatureUsageRow {
    component: Option<String>,
    cost_usd: Option<f64>,
}

/// Minimal workspace row for billing period lookups.
#[derive(Debug, sqlx::FromRow)]
struct WorkspaceBilling {
    billing_cycle: Option<String>,
    subscription_period_start: Option<DateTime<Utc>>,
    subscription_period_end: Option<DateTime<Utc>>,
    trial_ends_at: Option<DateTime<Utc>>,
    user_limit: Option<i32>,
    subscription_tier: kyomi_core::SubscriptionTier,
    created_at: DateTime<Utc>,
    ai_bundle_balance_usd: f64,
}

// ─── Service ────────────────────────────────────────────────────────────────

/// Stateless billing service — all state lives in the database.
///
/// Takes a `&DbPool` as a parameter to each method rather than holding
/// a reference, matching the Rust service pattern used elsewhere.
pub struct BillingService;

impl BillingService {
    /// Create a new `BillingService`.
    pub fn new() -> Self {
        Self
    }

    /// Get the monthly AI budget in USD for a given tier and user limit.
    ///
    /// Delegates to `kyomi_core::capability::get_credits_limit()` — single
    /// source of truth for budget values (read from env vars).
    pub fn get_ai_budget_for_tier(tier: kyomi_core::SubscriptionTier, user_limit: Option<i32>) -> f64 {
        kyomi_core::capability::get_credits_limit(tier, user_limit)
    }

    /// Calculate which monthly billing period we are currently in.
    ///
    /// For annual plans, AI credits reset **monthly** even though the
    /// subscription is annual. This function steps forward month-by-month
    /// from the subscription start until we find the current period.
    ///
    /// Returns `(period_start, period_end)`.
    pub fn calculate_monthly_period(
        subscription_start: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> (DateTime<Utc>, DateTime<Utc>) {
        let mut current = subscription_start;

        loop {
            let next = add_one_month(current);
            if next > now {
                return (current, next);
            }
            current = next;
        }
    }

    /// Get aggregated AI usage statistics for a workspace within a billing period.
    ///
    /// If `period_start`/`period_end` are `None`, the correct period is
    /// computed from the workspace's subscription data (with monthly reset
    /// for annual plans).
    pub async fn get_ai_usage_for_period(
        &self,
        db: &DbPool,
        workspace_id: &str,
        period_start: Option<DateTime<Utc>>,
        period_end: Option<DateTime<Utc>>,
    ) -> kyomi_core::Result<UsageStats> {
        let now = Utc::now();

        let (start, end) = match (period_start, period_end) {
            (Some(s), Some(e)) => (s, e),
            _ => {
                // Look up workspace billing info to determine the period
                let ws = kyomi_core::db_fetch_optional!(
                    db,
                    WorkspaceBilling,
                    r#"SELECT billing_cycle, subscription_period_start,
                     subscription_period_end, trial_ends_at,
                     user_limit,
                     subscription_tier,
                     created_at,
                     COALESCE(ai_bundle_balance_usd, 0) AS ai_bundle_balance_usd
                     FROM workspaces WHERE workspace_id = $1"#,
                    workspace_id
                )
                .map_err(|e| kyomi_core::Error::Internal(format!("failed to fetch workspace billing: {e}")))?;

                match ws {
                    Some(ws) => {
                        let end = period_end.unwrap_or(now);

                        let start = if period_start.is_some() {
                            period_start.unwrap_or(now)
                        } else if ws.billing_cycle.as_deref() == Some("annual") {
                            if let Some(sub_start) = ws.subscription_period_start {
                                // Annual plan: monthly reset from subscription start
                                let (monthly_start, _) =
                                    Self::calculate_monthly_period(sub_start, end);
                                monthly_start
                            } else {
                                ws.subscription_period_start
                                    .unwrap_or(ws.created_at)
                            }
                        } else {
                            // Monthly plan or no subscription yet: use period start,
                            // falling back to calendar month via created_at.
                            ws.subscription_period_start.unwrap_or(ws.created_at)
                        };

                        (start, end)
                    }
                    None => {
                        tracing::warn!(
                            workspace_id,
                            "Workspace not found for usage query, using defaults"
                        );
                        (period_start.unwrap_or(now), period_end.unwrap_or(now))
                    }
                }
            }
        };

        // Query api_usage_log for aggregated stats
        let row = kyomi_core::db_fetch_one!(
            db,
            UsageRow,
            "SELECT \
                 COALESCE(SUM(cost_estimate), 0) AS total_cost_usd, \
                 COALESCE(SUM(input_tokens), 0) AS total_input_tokens, \
                 COALESCE(SUM(output_tokens), 0) AS total_output_tokens, \
                 COUNT(*) AS total_calls \
             FROM api_usage_log \
             WHERE workspace_id = $1 \
               AND timestamp >= $2 \
               AND timestamp <= $3",
            workspace_id,
            start,
            end
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to fetch usage stats: {e}")))?;

        Ok(UsageStats {
            total_cost_usd: row.total_cost_usd.unwrap_or(0.0),
            total_input_tokens: row.total_input_tokens.unwrap_or(0),
            total_output_tokens: row.total_output_tokens.unwrap_or(0),
            total_calls: row.total_calls.unwrap_or(0),
            period_start: start,
            period_end: end,
        })
    }

    /// Calculate AI credits information for a workspace.
    ///
    /// Returns dollar-based credits with percentage for display.
    /// Handles the unified expiration logic:
    /// - Exhausted if budget >= 100% used
    /// - Exhausted if current time > subscription_period_end
    pub async fn calculate_credits_info(
        &self,
        db: &DbPool,
        workspace_id: &str,
    ) -> kyomi_core::Result<CreditsInfo> {
        let now = Utc::now();

        // Load workspace billing data
        let ws = kyomi_core::db_fetch_optional!(
            db,
            WorkspaceBilling,
            r#"SELECT billing_cycle, subscription_period_start,
             subscription_period_end, trial_ends_at,
             user_limit,
             subscription_tier,
             created_at,
             COALESCE(ai_bundle_balance_usd, 0) AS ai_bundle_balance_usd
             FROM workspaces WHERE workspace_id = $1"#,
            workspace_id
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to fetch workspace billing: {e}")))?;

        let ws = ws.ok_or_else(|| {
            kyomi_core::Error::NotFound(format!("Workspace {workspace_id} not found"))
        })?;

        // Get budget for this tier, plus any purchased bundle balance
        let budget_usd = Self::get_ai_budget_for_tier(ws.subscription_tier, ws.user_limit)
            + ws.ai_bundle_balance_usd;

        // Get actual usage for current billing period
        let usage = self
            .get_ai_usage_for_period(db, workspace_id, None, None)
            .await?;
        let used_usd = usage.total_cost_usd;

        // Calculate remaining and percentage
        let remaining_usd = (budget_usd - used_usd).max(0.0);
        let percentage_used = if budget_usd > 0.0 {
            ((used_usd / budget_usd) * 100.0).min(100.0)
        } else {
            100.0
        };
        let mut exhausted = percentage_used >= 100.0 || budget_usd == 0.0;

        // Expiration check: all Cloud users have subscription periods.
        if let Some(period_end) = ws.subscription_period_end {
            if now > period_end {
                tracing::warn!(
                    workspace_id,
                    %period_end,
                    tier = %ws.subscription_tier,
                    "Subscription period expired"
                );
                exhausted = true;
            }
        } else {
            // Free/new workspaces have no subscription period — this is expected.
            tracing::debug!(
                workspace_id,
                "Workspace has no subscription_period_end"
            );
        }

        Ok(CreditsInfo {
            limit_usd: budget_usd,
            used_usd,
            remaining_usd,
            percentage_used: if exhausted { 100.0 } else { percentage_used },
            exhausted,
            total_calls: usage.total_calls,
            period_start: usage.period_start,
            period_end: usage.period_end,
        })
    }

    /// Check if AI usage is allowed for a workspace.
    ///
    /// Returns warning levels based on usage percentage:
    /// - 0-79%:  OK (no warning)
    /// - 80-89%: Warning
    /// - 90-99%: Critical warning
    /// - 100%+:  Blocked
    pub async fn check_ai_usage_allowed(
        &self,
        db: &DbPool,
        workspace_id: &str,
    ) -> kyomi_core::Result<UsageAllowed> {
        // Verify workspace exists by fetching its tier as a scalar
        let _tier: String = kyomi_core::db_fetch_scalar!(
            db,
            String,
            "SELECT subscription_tier FROM workspaces WHERE workspace_id = $1",
            workspace_id
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to fetch workspace tier: {e}")))?;

        let credits_info = self.calculate_credits_info(db, workspace_id).await?;
        let percentage = credits_info.percentage_used;

        if credits_info.exhausted {
            Ok(UsageAllowed {
                allowed: false,
                blocked: true,
                warning_level: Some("blocked".to_string()),
                percentage_used: percentage,
                message: Some(format!(
                    "AI budget exhausted ({percentage:.1}% used). \
                     Add an AI token bundle or connect your own API key."
                )),
            })
        } else if percentage >= 90.0 {
            Ok(UsageAllowed {
                allowed: true,
                blocked: false,
                warning_level: Some("critical".to_string()),
                percentage_used: percentage,
                message: Some(format!(
                    "AI budget critically low ({percentage:.1}% used). \
                     Consider purchasing an AI token bundle to avoid interruption."
                )),
            })
        } else if percentage >= 80.0 {
            Ok(UsageAllowed {
                allowed: true,
                blocked: false,
                warning_level: Some("warning".to_string()),
                percentage_used: percentage,
                message: Some(format!(
                    "AI budget at {percentage:.1}%. \
                     You may want to purchase additional AI credits soon."
                )),
            })
        } else {
            Ok(UsageAllowed {
                allowed: true,
                blocked: false,
                warning_level: None,
                percentage_used: percentage,
                message: None,
            })
        }
    }

    /// Get the full AI usage status for a workspace/user.
    ///
    /// Returns overall usage, per-user fair share, and per-feature breakdown.
    /// Matches the Python `/billing/ai-usage-status` response shape.
    pub async fn get_ai_usage_status(
        &self,
        db: &DbPool,
        workspace_id: &str,
        _user_id: &str,
    ) -> kyomi_core::Result<AiUsageStatus> {
        let credits_info = self.calculate_credits_info(db, workspace_id).await?;
        let usage_allowed = self.check_ai_usage_allowed(db, workspace_id).await?;

        // Load workspace for trial/reset date info
        let ws = kyomi_core::db_fetch_optional!(
            db,
            WorkspaceBilling,
            r#"SELECT billing_cycle, subscription_period_start,
             subscription_period_end, trial_ends_at,
             user_limit,
             subscription_tier,
             created_at,
             COALESCE(ai_bundle_balance_usd, 0) AS ai_bundle_balance_usd
             FROM workspaces WHERE workspace_id = $1"#,
            workspace_id
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to fetch workspace billing: {e}")))?;

        let (ai_reset_date, trial_ends_at, user_limit) = match &ws {
            Some(ws) => {
                let now = Utc::now();
                // AI reset date = end of current monthly billing period
                let reset = if ws.billing_cycle.as_deref() == Some("annual") {
                    if let Some(sub_start) = ws.subscription_period_start {
                        let (_, period_end) =
                            Self::calculate_monthly_period(sub_start, now);
                        Some(period_end)
                    } else {
                        ws.subscription_period_end
                    }
                } else {
                    ws.subscription_period_end
                };

                (reset, ws.trial_ends_at, ws.user_limit.unwrap_or(999_999))
            }
            None => (None, None, 999_999),
        };

        // Per-user fair share (simple: user gets 100% of their share)
        let fair_share_pct = if user_limit > 0 {
            100.0 / f64::from(user_limit)
        } else {
            100.0
        };

        // Per-feature breakdown
        let feature_rows = kyomi_core::db_fetch_all!(
            db,
            FeatureUsageRow,
            "SELECT component, \
                    COALESCE(SUM(cost_estimate), 0) AS cost_usd \
             FROM api_usage_log \
             WHERE workspace_id = $1 \
               AND timestamp >= $2 \
               AND timestamp <= $3 \
             GROUP BY component",
            workspace_id,
            credits_info.period_start,
            credits_info.period_end
        )
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to fetch feature usage: {e}")))?;

        // Aggregate cost per canonical feature name.
        let mut cost_by_feature: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        for row in feature_rows {
            let component = row.component.unwrap_or_else(|| "unknown".to_string());
            let feature_key = match component.as_str() {
                "custom_agent" | "chat_agent" | "chat" | "chat_title_generation" => "chat",
                "dashboard_copilot" => "dashboard_copilot",
                "chart_builder_copilot" | "chart_copilot" => "chart_builder_copilot",
                "kyomi_watch" | "watch" => "kyomi_watch",
                other => other,
            };
            *cost_by_feature.entry(feature_key.to_string()).or_default() +=
                row.cost_usd.unwrap_or(0.0);
        }

        // Convert costs to percentages of total workspace usage (matches Python format).
        let total_cost = credits_info.used_usd;
        let by_feature: std::collections::HashMap<String, f64> = if total_cost > 0.0 {
            cost_by_feature
                .into_iter()
                .map(|(k, v)| (k, v / total_cost * 100.0))
                .collect()
        } else {
            // No usage — return all zeros for the known features.
            [
                ("chat", 0.0),
                ("dashboard_copilot", 0.0),
                ("chart_builder_copilot", 0.0),
                ("kyomi_watch", 0.0),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
        };

        Ok(AiUsageStatus {
            percentage_used: credits_info.percentage_used,
            warning_level: usage_allowed.warning_level,
            allowed: usage_allowed.allowed,
            blocked: usage_allowed.blocked,
            ai_reset_date,
            trial_ends_at,
            per_user: PerUserUsage {
                percentage_used: credits_info.percentage_used,
                fair_share_percentage: fair_share_pct,
            },
            by_feature,
        })
    }
}

impl Default for BillingService {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Date helpers ───────────────────────────────────────────────────────────

/// Add one calendar month to a `DateTime<Utc>`.
///
/// Handles month-end overflow by clamping to the last day of the target
/// month (e.g. Jan 31 + 1 month = Feb 28/29).
fn add_one_month(dt: DateTime<Utc>) -> DateTime<Utc> {
    let year = dt.year();
    let month = dt.month();

    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };

    // Clamp day to the max day of the target month
    let max_day = days_in_month(next_year, next_month);
    let day = dt.day().min(max_day);

    // Build the target date from scratch to avoid issues where `with_month`
    // fails when the current day is out of range for the new month (e.g.
    // Jan 31 -> with_month(2) fails because Feb 31 doesn't exist).
    chrono::NaiveDate::from_ymd_opt(next_year, next_month, day)
        .and_then(|d| d.and_hms_opt(dt.hour(), dt.minute(), dt.second()))
        .map(|naive| DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
        .unwrap_or(dt)
}

/// Get the number of days in a given month.
fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Check if a year is a leap year.
fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

// ─── Seat billing sync ─────────────────────────────────────────────────────

/// Sync the Stripe subscription quantity to match the actual active member count.
///
/// Called after invite accept (`increment = 1`) or member removal (`increment = -1`).
/// Checks that `user_count == stripe_quantity + increment` — if not, logs a
/// warning indicating drift between Stripe and the DB.
pub async fn update_billing_users(
    db: &kyomi_core::DbPool,
    stripe_service: &crate::stripe_service::StripeService,
    workspace_id: &str,
    user_count: i64,
    increment: i64,
) -> kyomi_core::Result<()> {
    let sub_id: Option<String> = kyomi_core::db_fetch_optional!(
        db,
        SubIdRow,
        "SELECT stripe_subscription_id FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?
    .and_then(|row| row.stripe_subscription_id);

    let Some(sub_id) = sub_id else {
        // No subscription — ensure user_limit is at least user_count
        // SQLite doesn't have GREATEST(); MAX(a,b) works on both.
        kyomi_core::db_execute!(
            db,
            "UPDATE workspaces SET user_limit = MAX(user_limit, $1) WHERE workspace_id = $2",
            user_count as i32,
            workspace_id
        )?;
        return Ok(());
    };

    let stripe_quantity = stripe_service
        .get_subscription_quantity(&sub_id)
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to get Stripe quantity: {e}")))?;

    let expected = stripe_quantity as i64 + increment;
    if user_count != expected {
        tracing::warn!(
            workspace_id = %workspace_id,
            user_count = user_count,
            stripe_quantity = stripe_quantity,
            increment = increment,
            expected = expected,
            "Seat count drift detected — DB member count does not match Stripe quantity + increment"
        );
    }

    if user_count as u64 != stripe_quantity {
        stripe_service
            .update_seat_count(&sub_id, user_count as u64)
            .await
            .map_err(|e| kyomi_core::Error::Internal(format!("Failed to update Stripe seats: {e}")))?;

        tracing::info!(
            workspace_id = %workspace_id,
            previous = stripe_quantity,
            new = user_count,
            "Updated Stripe seat count"
        );
    }

    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct SubIdRow {
    stripe_subscription_id: Option<String>,
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use kyomi_core::SubscriptionTier;

    #[test]
    fn test_get_ai_budget_delegates_to_capability() {
        // All tiers now use the same Cloud budget. Verify BillingService delegates
        // correctly and all tiers return the same value.
        use kyomi_core::capability::get_credits_limit;
        let expected = get_credits_limit(SubscriptionTier::Free, None);

        for tier in [
            SubscriptionTier::Free,
            SubscriptionTier::Starter,
            SubscriptionTier::Basic,
            SubscriptionTier::Pro,
            SubscriptionTier::Team,
            SubscriptionTier::Enterprise,
            SubscriptionTier::Cloud,
        ] {
            let budget = BillingService::get_ai_budget_for_tier(tier, None);
            assert!(
                (budget - expected).abs() < f64::EPSILON,
                "All tiers should return the same budget, but {tier:?} returned {budget} (expected {expected})"
            );
        }
        // user_limit should not affect the budget in single-tier model
        let with_limit = BillingService::get_ai_budget_for_tier(SubscriptionTier::Team, Some(8));
        assert!(
            (with_limit - expected).abs() < f64::EPSILON,
            "user_limit should not affect budget"
        );
    }

    #[test]
    fn test_calculate_monthly_period_simple() {
        // Subscription starts Jan 15, we're on Feb 20
        let start = Utc.with_ymd_and_hms(2024, 1, 15, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2024, 2, 20, 0, 0, 0).unwrap();

        let (period_start, period_end) =
            BillingService::calculate_monthly_period(start, now);

        assert_eq!(period_start.month(), 2);
        assert_eq!(period_start.day(), 15);
        assert_eq!(period_end.month(), 3);
        assert_eq!(period_end.day(), 15);
    }

    #[test]
    fn test_calculate_monthly_period_first_month() {
        // Subscription starts Jan 15, we're still in January
        let start = Utc.with_ymd_and_hms(2024, 1, 15, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2024, 1, 20, 0, 0, 0).unwrap();

        let (period_start, period_end) =
            BillingService::calculate_monthly_period(start, now);

        assert_eq!(period_start.month(), 1);
        assert_eq!(period_start.day(), 15);
        assert_eq!(period_end.month(), 2);
        assert_eq!(period_end.day(), 15);
    }

    #[test]
    fn test_calculate_monthly_period_month_end_clamping() {
        // Subscription starts Jan 31, check period in February
        let start = Utc.with_ymd_and_hms(2024, 1, 31, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2024, 3, 5, 0, 0, 0).unwrap();

        let (period_start, _period_end) =
            BillingService::calculate_monthly_period(start, now);

        // Feb doesn't have 31 days, so it should clamp
        assert_eq!(period_start.month(), 2);
        assert!(period_start.day() <= 29); // 2024 is a leap year
    }

    #[test]
    fn test_add_one_month() {
        let jan = Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap();
        let feb = add_one_month(jan);
        assert_eq!(feb.month(), 2);
        assert_eq!(feb.day(), 15);

        let dec = Utc.with_ymd_and_hms(2024, 12, 15, 12, 0, 0).unwrap();
        let next_jan = add_one_month(dec);
        assert_eq!(next_jan.year(), 2025);
        assert_eq!(next_jan.month(), 1);
    }

    #[test]
    fn test_add_one_month_end_of_month() {
        // Jan 31 -> Feb 29 (2024 is leap year)
        let jan31 = Utc.with_ymd_and_hms(2024, 1, 31, 0, 0, 0).unwrap();
        let feb = add_one_month(jan31);
        assert_eq!(feb.month(), 2);
        assert_eq!(feb.day(), 29);

        // Jan 31 -> Feb 28 (2023 is not a leap year)
        let jan31_2023 = Utc.with_ymd_and_hms(2023, 1, 31, 0, 0, 0).unwrap();
        let feb_2023 = add_one_month(jan31_2023);
        assert_eq!(feb_2023.month(), 2);
        assert_eq!(feb_2023.day(), 28);
    }

    #[test]
    fn test_days_in_month() {
        assert_eq!(days_in_month(2024, 1), 31);
        assert_eq!(days_in_month(2024, 2), 29); // leap year
        assert_eq!(days_in_month(2023, 2), 28); // not leap year
        assert_eq!(days_in_month(2024, 4), 30);
        assert_eq!(days_in_month(2024, 12), 31);
    }

    #[test]
    fn test_is_leap_year() {
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2023));
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(1900));
    }
}
