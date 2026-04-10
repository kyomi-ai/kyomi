// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for Usage settings.
//!
//! These replace the REST API call for AI usage status:
//! - `GET /billing/ai-usage-status` -> `get_ai_usage_status()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/billing.rs::get_ai_usage_status`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, workspace_id};

/// Workspace bundle balances row (SSR only).
#[cfg(feature = "ssr")]
#[derive(Debug, sqlx::FromRow)]
struct BundleRow {
    ai_bundle_balance_usd: f64,
    analytics_bundle_events: i64,
}

/// Per-user fair-share usage info (mirrors `kyomi_auth::billing_service::PerUserUsage`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PerUserUsage {
    pub percentage_used: f64,
    pub fair_share_percentage: f64,
}

/// Full AI usage status for a workspace/user.
///
/// Matches the JSON shape returned by `GET /billing/ai-usage-status`,
/// extended with bundle balance and analytics event data.
/// See `kyomi_auth::billing_service::AiUsageStatus`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageData {
    pub percentage_used: f64,
    pub warning_level: Option<String>,
    pub allowed: bool,
    pub blocked: bool,
    pub ai_reset_date: Option<String>,
    pub trial_ends_at: Option<String>,
    pub per_user: PerUserUsage,
    pub by_feature: HashMap<String, f64>,

    // ── Bundle & analytics fields ──────────────────────────────────
    /// Remaining purchased AI token bundle balance in USD.
    pub ai_bundle_balance_usd: f64,
    /// Analytics events used this month (from analytics quota tracking).
    pub analytics_events_used: u64,
    /// Analytics events included in the Cloud plan (100K).
    pub analytics_events_included: u64,
    /// Purchased analytics event bundle balance (non-expiring).
    pub analytics_bundle_events: i64,
}

/// Fetch the AI usage status for the current user's workspace.
///
/// Self-hosted mode returns unlimited usage (no billing).
#[server(prefix = "/leptos-api")]
pub async fn get_ai_usage_status() -> Result<UsageData, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    // Self-hosted: no billing, unlimited AI usage
    if ctx.config.self_hosted {
        return Ok(UsageData {
            percentage_used: 0.0,
            warning_level: None,
            allowed: true,
            blocked: false,
            ai_reset_date: None,
            trial_ends_at: None,
            per_user: PerUserUsage {
                percentage_used: 0.0,
                fair_share_percentage: 100.0,
            },
            by_feature: HashMap::from([
                ("chat".to_string(), 0.0),
                ("dashboard_copilot".to_string(), 0.0),
                ("chart_builder_copilot".to_string(), 0.0),
                ("kyomi_watch".to_string(), 0.0),
            ]),
            ai_bundle_balance_usd: 0.0,
            analytics_events_used: 0,
            analytics_events_included: kyomi_core::capability::ANALYTICS_EVENTS_INCLUDED,
            analytics_bundle_events: 0,
        });
    }

    let ws_id = workspace_id(&auth)?;

    let billing_service = kyomi_auth::billing_service::BillingService::new();
    let status = billing_service
        .get_ai_usage_status(&ctx.db, ws_id, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Fetch bundle balances from the workspace row.
    let bundles = kyomi_core::db_fetch_optional!(
        &ctx.db,
        BundleRow,
        "SELECT COALESCE(ai_bundle_balance_usd, 0) AS ai_bundle_balance_usd, \
         COALESCE(analytics_bundle_events, 0) AS analytics_bundle_events \
         FROM workspaces WHERE workspace_id = $1",
        ws_id
    )
    .map_err(|e| ServerFnError::new(format!("failed to fetch bundle balances: {e}")))?;

    let (ai_bundle_balance_usd, analytics_bundle_events) = bundles
        .map(|b| (b.ai_bundle_balance_usd, b.analytics_bundle_events))
        .unwrap_or((0.0, 0));

    // Get analytics events used this month from Redis.
    let analytics_events_used: u64 = if let Some(ref redis_url) = ctx.config.redis_url {
        match kyomi_core::redis::create_pool(redis_url).await {
            Ok(mut conn) => {
                kyomi_auth::analytics_quota::get_usage_count(&mut conn, ws_id)
                    .await
                    .unwrap_or(0)
            }
            Err(_) => 0,
        }
    } else {
        0
    };
    let analytics_events_included: u64 = kyomi_core::capability::ANALYTICS_EVENTS_INCLUDED;

    Ok(UsageData {
        percentage_used: status.percentage_used,
        warning_level: status.warning_level,
        allowed: status.allowed,
        blocked: status.blocked,
        ai_reset_date: status.ai_reset_date.map(|dt| dt.to_rfc3339()),
        trial_ends_at: status.trial_ends_at.map(|dt| dt.to_rfc3339()),
        per_user: PerUserUsage {
            percentage_used: status.per_user.percentage_used,
            fair_share_percentage: status.per_user.fair_share_percentage,
        },
        by_feature: status.by_feature,
        ai_bundle_balance_usd,
        analytics_events_used,
        analytics_events_included,
        analytics_bundle_events,
    })
}
