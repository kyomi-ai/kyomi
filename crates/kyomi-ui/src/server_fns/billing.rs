// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for Billing settings.
//!
//! These replace the REST API calls that BillingPanel.jsx makes:
//! - `GET /billing/subscription-info` -> `get_subscription_info()`
//! - `GET /billing/invoices` -> `get_invoices()`
//! - `POST /billing/create-checkout` -> `create_checkout()`
//! - `POST /billing/cancel-subscription` -> `cancel_subscription()`
//! - `POST /billing/reactivate-subscription` -> `reactivate_subscription()`
//! - `POST /billing/create-portal-session` -> `create_portal_session()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/billing.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, workspace_id, IntoServerFnError};

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Subscription information for the current workspace.
///
/// Cloud plan — single tier at $5/user/month. The `billing_cycle` field is
/// retained for backward compatibility but is always "monthly" for new
/// subscriptions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubscriptionInfo {
    pub tier: String,
    pub status: String,
    pub billing_cycle: Option<String>,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    pub ai_reset_date: Option<String>,
    /// Trial expiration timestamp (ISO 8601). Present when status is "trialing".
    pub trial_ends_at: Option<String>,
    pub user_limit: Option<i32>,
    /// AI token bundle balance in cents (e.g. 1500 = $15.00). Non-expiring.
    pub ai_token_balance_cents: Option<i64>,
    /// Number of analytics events consumed this month.
    pub analytics_events_used: Option<i64>,
    /// Remaining purchased analytics event bundle balance (non-expiring).
    pub analytics_bundle_balance: Option<i64>,
    /// Number of active members in the workspace (for seat billing display).
    pub active_members: i32,
}

/// A single invoice record.
///
/// Matches the JSON shape returned by `GET /billing/invoices`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvoiceRecord {
    pub invoice_id: String,
    pub amount_paid: f64,
    pub currency: String,
    pub status: Option<String>,
    pub hosted_invoice_url: Option<String>,
    pub invoice_pdf: Option<String>,
    pub created: Option<i64>,
    pub description: Option<String>,
}

/// Result of a checkout or portal session creation — a URL to redirect to.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RedirectUrl {
    pub url: String,
}

/// Result of creating an embedded checkout session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmbeddedCheckoutSession {
    pub client_secret: String,
    pub session_id: String,
}

/// Result of creating a subscription checkout — either embedded checkout
/// (for new subscriptions) or an immediate modification result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CheckoutOutcome {
    /// New subscription — mount embedded checkout with this client_secret.
    Embedded(EmbeddedCheckoutSession),
    /// Existing subscription modified — no checkout needed.
    Modified(String),
}

/// Status of a checkout session (for verifying completion from onComplete callback).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckoutStatus {
    pub status: String,
    pub payment_status: String,
}

/// Result of a mutation (cancel, reactivate, update team size).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BillingResult {
    pub message: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// SSR-only helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Reject anyone who isn't the workspace owner.
///
/// Billing is owner-only because the owner is the single spending authority
/// for the workspace. Admins can invite users (consuming seats), but only the
/// owner can change subscription plan, buy bundles, or adjust the seat cap.
///
/// Sets the HTTP response status to 403 Forbidden via `ResponseOptions` on
/// the reject path so tower_http and the browser don't classify owner-only
/// rejection as a 5xx server error. Mirrors the 401 pattern in
/// `extract_auth` — permission failures are client errors, not server
/// errors.
#[cfg(feature = "ssr")]
fn require_workspace_owner(
    auth: &kyomi_auth::middleware::AuthUser,
) -> Result<(), ServerFnError> {
    if auth.workspace.is_owner {
        Ok(())
    } else {
        leptos::prelude::expect_context::<leptos_axum::ResponseOptions>()
            .set_status(axum::http::StatusCode::FORBIDDEN);
        Err(ServerFnError::new("Workspace owner access required"))
    }
}

/// Minimal workspace row for billing operations.
#[cfg(feature = "ssr")]
#[derive(Debug, sqlx::FromRow)]
struct WorkspaceRow {
    name: Option<String>,
    subscription_tier: String,
    subscription_status: String,
    billing_cycle: Option<String>,
    subscription_period_start: Option<String>,
    subscription_period_end: Option<String>,
    trial_ends_at: Option<String>,
    user_limit: Option<i32>,
    stripe_customer_id: Option<String>,
    stripe_subscription_id: Option<String>,
    #[sqlx(default)]
    analytics_bundle_events: Option<i64>,
}

/// Parse a timestamp produced by `CAST(timestamptz AS TEXT)` in Postgres
/// (`YYYY-MM-DD HH:MM:SS[.fff]+00`) or a standard RFC3339 string. Returns
/// `None` on failure.
///
/// Postgres's `timestamptz::text` cast uses a space instead of `T` between
/// the date and time, and a short `+00` offset rather than `+00:00`, which
/// `chrono::DateTime::parse_from_rfc3339` rejects. We try RFC3339 first and
/// fall back to the Postgres text format.
#[cfg(feature = "ssr")]
fn parse_pg_or_rfc3339(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    // Postgres text cast: 2026-05-12 07:39:04.698546+00
    for fmt in ["%Y-%m-%d %H:%M:%S%.f%#z", "%Y-%m-%d %H:%M:%S%#z"] {
        if let Ok(dt) = chrono::DateTime::parse_from_str(s, fmt) {
            return Some(dt.with_timezone(&chrono::Utc));
        }
    }
    None
}

#[cfg(feature = "ssr")]
impl WorkspaceRow {
    fn period_start_dt(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.subscription_period_start
            .as_deref()
            .and_then(parse_pg_or_rfc3339)
    }
    fn period_end_dt(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.subscription_period_end
            .as_deref()
            .and_then(parse_pg_or_rfc3339)
    }
    fn trial_ends_at_dt(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.trial_ends_at.as_deref().and_then(parse_pg_or_rfc3339)
    }
}

/// Load a workspace by workspace_id.
#[cfg(feature = "ssr")]
async fn load_workspace(
    db: &kyomi_core::DbPool,
    ws_id: &str,
) -> Result<WorkspaceRow, ServerFnError> {
    kyomi_core::db_fetch_optional!(
        db, WorkspaceRow,
        "SELECT name, subscription_tier, subscription_status, \
         billing_cycle, \
         CAST(subscription_period_start AS TEXT) AS subscription_period_start, \
         CAST(subscription_period_end AS TEXT) AS subscription_period_end, \
         CAST(trial_ends_at AS TEXT) AS trial_ends_at, \
         user_limit, \
         stripe_customer_id, stripe_subscription_id, \
         COALESCE(analytics_bundle_events, 0) AS analytics_bundle_events \
         FROM workspaces WHERE workspace_id = $1",
        ws_id
    )
    .into_sfn()?
    .ok_or_else(|| ServerFnError::new("Workspace not found"))
}

/// Get the StripeService from config, or error.
#[cfg(feature = "ssr")]
fn require_stripe(
    config: &kyomi_core::Config,
) -> Result<kyomi_auth::stripe_service::StripeService, ServerFnError> {
    let secret_key = config
        .stripe_secret_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Billing features are not available"))?;
    let webhook_secret = config
        .stripe_webhook_secret
        .as_deref()
        .unwrap_or_default();
    Ok(kyomi_auth::stripe_service::StripeService::new(
        secret_key,
        webhook_secret,
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// Server functions
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch subscription info for the current workspace.
///
/// Mirrors `GET /api/v1/billing/subscription-info`.
///
/// Note: subscription data (tier, status, period) comes from the database and
/// is always available regardless of Stripe configuration. Only checkout and
/// portal operations require Stripe.
#[server(prefix = "/leptos-api")]
pub async fn get_subscription_info() -> Result<SubscriptionInfo, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    require_workspace_owner(&auth)?;
    let ws_id = workspace_id(&auth)?;

    let workspace = load_workspace(&ctx.db, ws_id).await?;

    // Calculate AI reset date
    let ai_reset_date = {
        let billing_cycle = workspace.billing_cycle.as_deref();
        let period_start = workspace.period_start_dt();
        let period_end = workspace.period_end_dt();

        if let Some(start) = period_start {
            if billing_cycle == Some("monthly") {
                period_end.map(|dt| dt.to_rfc3339())
            } else {
                let now = chrono::Utc::now();
                let (_, monthly_end) =
                    kyomi_auth::billing_service::BillingService::calculate_monthly_period(
                        start, now,
                    );
                Some(monthly_end.to_rfc3339())
            }
        } else {
            None
        }
    };

    // AI bundle balance remaining comes from the billing service, which
    // computes it from authoritative live usage records. Using the stale
    // `ai_credits_used_usd` cache column here produced a stuck "$X.XX
    // remaining" display that never decreased as users actually consumed
    // credits — see the Billing Service comment for the full story.
    let ai_remaining_usd = kyomi_auth::billing_service::BillingService::new()
        .get_bundle_remaining_usd(&ctx.db, ws_id)
        .await
        .into_sfn()?;
    let ai_token_balance_cents = (ai_remaining_usd * 100.0) as i64;

    // Analytics events this month from Redis (same pattern as usage.rs).
    // Falls back to 0 if Redis is unavailable.
    let analytics_events_used: i64 = if let Some(ref redis_url) = ctx.config.redis_url {
        match kyomi_core::redis::create_pool(redis_url).await {
            Ok(mut conn) => {
                kyomi_auth::analytics_quota::get_usage_count(&mut conn, ws_id)
                    .await
                    .unwrap_or(0) as i64
            }
            Err(_) => 0,
        }
    } else {
        0
    };

    // Analytics bundle balance from the workspace row (already loaded)
    let analytics_bundle_balance = workspace.analytics_bundle_events.unwrap_or(0);

    // Count active workspace members for seat billing display
    let bt = kyomi_core::sql_compat::bool_true(ctx.db.is_postgres());
    let count_sql = format!(
        "SELECT COUNT(*) FROM workspace_users WHERE workspace_id = $1 AND active = {bt}"
    );
    let active_members: i32 = kyomi_core::db_fetch_scalar!(
        &ctx.db, i64, &count_sql, ws_id
    ).map_err(|e| ServerFnError::new(format!("Failed to count workspace members: {e}")))? as i32;

    // Normalize all timestamps to RFC3339 so the frontend's date formatter
    // (which expects RFC3339) renders them correctly. Postgres's
    // `timestamptz::text` cast uses a space separator that trips the parser.
    let period_start_rfc = workspace.period_start_dt().map(|dt| dt.to_rfc3339());
    let period_end_rfc = workspace.period_end_dt().map(|dt| dt.to_rfc3339());
    let trial_ends_at_rfc = workspace.trial_ends_at_dt().map(|dt| dt.to_rfc3339());

    Ok(SubscriptionInfo {
        tier: workspace.subscription_tier,
        status: workspace.subscription_status,
        billing_cycle: workspace.billing_cycle,
        period_start: period_start_rfc,
        period_end: period_end_rfc,
        ai_reset_date,
        trial_ends_at: trial_ends_at_rfc,
        user_limit: workspace.user_limit,
        ai_token_balance_cents: Some(ai_token_balance_cents),
        analytics_events_used: Some(analytics_events_used),
        analytics_bundle_balance: Some(analytics_bundle_balance),
        active_members,
    })
}

/// Fetch recent invoices for the current workspace.
///
/// Mirrors `GET /api/v1/billing/invoices`.
#[server(prefix = "/leptos-api")]
pub async fn get_invoices() -> Result<Vec<InvoiceRecord>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    // Stripe not configured — no invoices to show.
    if ctx.config.stripe_secret_key.is_none() {
        return Ok(vec![]);
    }

    require_workspace_owner(&auth)?;
    let ws_id = workspace_id(&auth)?;

    let workspace = load_workspace(&ctx.db, ws_id).await?;

    // If no Stripe customer, return empty list
    let customer_id = match workspace.stripe_customer_id {
        Some(ref id) if !id.is_empty() => id.clone(),
        _ => return Ok(vec![]),
    };

    let stripe_service = require_stripe(&ctx.config)?;
    let invoices = stripe_service
        .list_invoices(&customer_id, 10)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to fetch invoices: {e}")))?;

    Ok(invoices
        .into_iter()
        .map(|inv| InvoiceRecord {
            invoice_id: inv.invoice_id,
            amount_paid: inv.amount_paid as f64 / 100.0,
            currency: inv.currency.to_uppercase(),
            status: inv.status,
            hosted_invoice_url: inv.hosted_invoice_url,
            invoice_pdf: inv.invoice_pdf,
            created: inv.created,
            description: None,
        })
        .collect())
}

/// Create a Stripe checkout session for subscription.
///
/// Returns `CheckoutOutcome::Embedded` with a client_secret for new
/// subscriptions (mount via Stripe.js embedded checkout), or
/// `CheckoutOutcome::Modified` when an existing subscription was
/// reactivated directly (no checkout needed).
#[server(prefix = "/leptos-api")]
pub async fn create_checkout(
    quantity: u64,
) -> Result<CheckoutOutcome, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_owner(&auth)?;
    let ws_id = workspace_id(&auth)?;

    let stripe_service = require_stripe(&ctx.config)?;
    let workspace = load_workspace(&ctx.db, ws_id).await?;

    // If user already has an active subscription, modify it directly.
    //
    // Delegates to the shared service so this path performs the same
    // Stripe + DB + MCP invalidation sequence as the REST route. Without
    // this, MCP clients would only see updated tool capabilities after
    // the Stripe webhook round-trip.
    if let Some(ref sub_id) = workspace.stripe_subscription_id
        && (workspace.subscription_status == "active"
            || workspace.subscription_status == "cancelled")
    {
        kyomi_auth::subscription_service::modify_existing_subscription(
            &ctx.db,
            &stripe_service,
            ctx.mcp_sessions
                .as_ref()
                .ok_or_else(|| ServerFnError::new("MCP session manager unavailable"))?,
            ws_id,
            sub_id,
        )
        .await
        .into_sfn()?;

        return Ok(CheckoutOutcome::Modified(
            "Subscription reactivated successfully".to_string(),
        ));
    }

    // New subscription flow — single Cloud price from env
    let price_id = kyomi_auth::stripe_config::get_cloud_price_id()
        .ok_or_else(|| {
            ServerFnError::new("STRIPE_CLOUD_MONTHLY not configured")
        })?;

    // Create Stripe customer if workspace doesn't have one
    let customer_id = match workspace.stripe_customer_id {
        Some(ref id) if !id.is_empty() => id.clone(),
        _ => {
            let email = &auth.email;
            let ws_name = workspace.name.as_deref().unwrap_or("Unnamed");
            let new_customer_id = stripe_service
                .create_customer(email, ws_id, ws_name)
                .await
                .map_err(|e| ServerFnError::new(format!("Failed to create Stripe customer: {e}")))?;

            kyomi_core::db_execute!(
                &ctx.db,
                "UPDATE workspaces SET stripe_customer_id = $1 WHERE workspace_id = $2",
                &new_customer_id,
                ws_id
            )
            .into_sfn()?;

            new_customer_id
        }
    };

    let params = kyomi_auth::stripe_service::EmbeddedCheckoutParams {
        customer_id,
        price_id: price_id.to_string(),
        workspace_id: ws_id.to_string(),
        quantity,
        trial_days: 30,
    };

    let result = stripe_service
        .create_embedded_checkout_session(&params)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to create checkout session: {e}")))?;

    Ok(CheckoutOutcome::Embedded(EmbeddedCheckoutSession {
        client_secret: result.client_secret,
        session_id: result.session_id,
    }))
}

/// Cancel the current subscription at period end.
///
/// Mirrors `POST /api/v1/billing/cancel-subscription`.
#[server(prefix = "/leptos-api")]
pub async fn cancel_subscription() -> Result<BillingResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_owner(&auth)?;
    let ws_id = workspace_id(&auth)?;

    let stripe_service = require_stripe(&ctx.config)?;
    let workspace = load_workspace(&ctx.db, ws_id).await?;

    let sub_id = workspace
        .stripe_subscription_id
        .as_deref()
        .ok_or_else(|| ServerFnError::new("No active subscription"))?;

    stripe_service
        .cancel_subscription(sub_id, true)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to cancel subscription: {e}")))?;

    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE workspaces SET subscription_status = 'cancelled' WHERE workspace_id = $1",
        ws_id
    )
    .into_sfn()?;

    Ok(BillingResult {
        message: "Subscription will be cancelled at the end of your billing period".to_string(),
    })
}

/// Reactivate a cancelled subscription.
///
/// Mirrors `POST /api/v1/billing/reactivate-subscription`.
#[server(prefix = "/leptos-api")]
pub async fn reactivate_subscription() -> Result<BillingResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_owner(&auth)?;
    let ws_id = workspace_id(&auth)?;

    let stripe_service = require_stripe(&ctx.config)?;
    let workspace = load_workspace(&ctx.db, ws_id).await?;

    let sub_id = workspace
        .stripe_subscription_id
        .as_deref()
        .ok_or_else(|| ServerFnError::new("No subscription to reactivate"))?;

    if workspace.subscription_status != "cancelled" {
        return Err(ServerFnError::new("Subscription is not cancelled"));
    }

    stripe_service
        .reactivate_subscription(sub_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to reactivate subscription: {e}")))?;

    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE workspaces SET subscription_status = 'active' WHERE workspace_id = $1",
        ws_id
    )
    .into_sfn()?;

    Ok(BillingResult {
        message: "Subscription has been reactivated".to_string(),
    })
}

/// The DB sentinel for "no seat cap". Treated as unlimited by the invite flow.
pub const UNLIMITED_SEAT_CAP: i32 = 999_999;

/// Update the workspace seat cap — the owner's spending ceiling.
///
/// Workspace admins can invite users up to this cap. Owner-only because
/// raising the cap increases monthly Stripe charges.
///
/// Validates that the new cap is at least as high as current active members —
/// lowering below that would require removing users first.
#[server(prefix = "/leptos-api")]
pub async fn update_user_limit(limit: i32) -> Result<i32, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_owner(&auth)?;
    let ws_id = workspace_id(&auth)?;

    if limit < 1 {
        return Err(ServerFnError::new("Seat cap must be at least 1"));
    }

    // Count active workspace members
    let bt = kyomi_core::sql_compat::bool_true(ctx.db.is_postgres());
    let count_sql = format!(
        "SELECT COUNT(*) FROM workspace_users WHERE workspace_id = $1 AND active = {bt}"
    );
    let active: i64 = kyomi_core::db_fetch_scalar!(&ctx.db, i64, &count_sql, ws_id)
        .map_err(|e| ServerFnError::new(format!("Failed to count members: {e}")))?;

    if (limit as i64) < active {
        return Err(ServerFnError::new(format!(
            "Cannot set seat cap below current active members ({active}). Remove users first."
        )));
    }

    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE workspaces SET user_limit = $1 WHERE workspace_id = $2",
        limit,
        ws_id
    )
    .into_sfn()?;

    tracing::info!(workspace_id = %ws_id, limit, "Updated workspace seat cap");

    Ok(limit)
}

/// Create a Stripe billing portal session and return the redirect URL.
///
/// Mirrors `POST /api/v1/billing/create-portal-session`.
#[server(prefix = "/leptos-api")]
pub async fn create_portal_session() -> Result<RedirectUrl, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_owner(&auth)?;
    let ws_id = workspace_id(&auth)?;

    let stripe_service = require_stripe(&ctx.config)?;
    let workspace = load_workspace(&ctx.db, ws_id).await?;

    let customer_id = workspace.stripe_customer_id.as_deref().ok_or_else(|| {
        ServerFnError::new("No Stripe customer found. Please subscribe to a plan first.")
    })?;

    // Portal returns via cross-site redirect from Stripe — use the
    // intermediate bounce page so SameSite=Strict cookies work.
    let return_url = format!("{}/billing/return", ctx.config.frontend_url);

    let (portal_url, _session_id) = stripe_service
        .create_portal_session(customer_id, &return_url)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to create portal session: {e}")))?;

    Ok(RedirectUrl { url: portal_url })
}

/// Purchase an AI token bundle via embedded Stripe checkout.
///
/// Returns an `EmbeddedCheckoutSession` with the client_secret to mount
/// the Stripe form inline. The webhook handler credits the workspace's
/// `ai_bundle_balance_usd` upon successful payment.
#[server(prefix = "/leptos-api")]
pub async fn purchase_ai_bundle(quantity: u32) -> Result<EmbeddedCheckoutSession, ServerFnError> {
    if quantity < 1 {
        return Err(ServerFnError::new("Quantity must be at least 1"));
    }

    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_owner(&auth)?;
    let ws_id = workspace_id(&auth)?;

    let stripe_service = require_stripe(&ctx.config)?;
    let workspace = load_workspace(&ctx.db, ws_id).await?;

    let customer_id = workspace.stripe_customer_id.as_deref().ok_or_else(|| {
        ServerFnError::new("No Stripe customer found. Please subscribe to a plan first.")
    })?;

    let price_id =
        kyomi_auth::stripe_config::get_ai_bundle_price_id().ok_or_else(|| {
            ServerFnError::new("STRIPE_AI_BUNDLE not configured")
        })?;

    let params = kyomi_auth::stripe_service::EmbeddedPaymentCheckoutParams {
        customer_id: customer_id.to_string(),
        price_id: price_id.to_string(),
        workspace_id: ws_id.to_string(),
        purchase_type: "ai_bundle".to_string(),
        quantity: u64::from(quantity),
    };

    let result = stripe_service
        .create_embedded_payment_checkout_session(&params)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to create AI bundle checkout: {e}")))?;

    Ok(EmbeddedCheckoutSession {
        client_secret: result.client_secret,
        session_id: result.session_id,
    })
}

/// Purchase an analytics event bundle via embedded Stripe checkout.
///
/// Returns an `EmbeddedCheckoutSession` with the client_secret to mount
/// the Stripe form inline. The webhook handler credits the workspace's
/// `analytics_bundle_events` upon successful payment.
#[server(prefix = "/leptos-api")]
pub async fn purchase_analytics_bundle(quantity: u32) -> Result<EmbeddedCheckoutSession, ServerFnError> {
    if quantity < 1 {
        return Err(ServerFnError::new("Quantity must be at least 1"));
    }

    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_owner(&auth)?;
    let ws_id = workspace_id(&auth)?;

    let stripe_service = require_stripe(&ctx.config)?;
    let workspace = load_workspace(&ctx.db, ws_id).await?;

    let customer_id = workspace.stripe_customer_id.as_deref().ok_or_else(|| {
        ServerFnError::new("No Stripe customer found. Please subscribe to a plan first.")
    })?;

    let price_id = kyomi_auth::stripe_config::get_analytics_bundle_price_id()
        .ok_or_else(|| {
            ServerFnError::new("STRIPE_ANALYTICS_BUNDLE not configured")
        })?;

    let params = kyomi_auth::stripe_service::EmbeddedPaymentCheckoutParams {
        customer_id: customer_id.to_string(),
        price_id: price_id.to_string(),
        workspace_id: ws_id.to_string(),
        purchase_type: "analytics_bundle".to_string(),
        quantity: u64::from(quantity),
    };

    let result = stripe_service
        .create_embedded_payment_checkout_session(&params)
        .await
        .map_err(|e| {
            ServerFnError::new(format!(
                "Failed to create analytics bundle checkout: {e}"
            ))
        })?;

    Ok(EmbeddedCheckoutSession {
        client_secret: result.client_secret,
        session_id: result.session_id,
    })
}

/// Get the Stripe publishable key (needed for embedded checkout on the frontend).
///
/// Publishable keys are designed to be public — this is not a secret.
#[server(prefix = "/leptos-api")]
pub async fn get_stripe_publishable_key() -> Result<Option<String>, ServerFnError> {
    let _auth = extract_auth().await?;
    let ctx = extract_context()?;
    Ok(ctx.config.stripe_publishable_key.clone())
}

/// Check the status of a checkout session (for verifying completion).
///
/// Called by the embedded checkout `onComplete` callback to confirm
/// the session actually completed before showing success UI.
#[server(prefix = "/leptos-api")]
pub async fn get_checkout_session_status(
    session_id: String,
) -> Result<CheckoutStatus, ServerFnError> {
    let _auth = extract_auth().await?;
    let ctx = extract_context()?;
    let stripe_service = require_stripe(&ctx.config)?;

    let status = stripe_service
        .retrieve_checkout_session_status(&session_id)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to retrieve session status: {e}")))?;

    Ok(CheckoutStatus {
        status: status.status,
        payment_status: status.payment_status,
    })
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::*;

    #[test]
    fn parse_pg_or_rfc3339_accepts_postgres_text_cast() {
        let parsed = parse_pg_or_rfc3339("2026-05-12 07:39:06+00").expect("parse");
        assert_eq!(parsed.to_rfc3339(), "2026-05-12T07:39:06+00:00");
    }

    #[test]
    fn parse_pg_or_rfc3339_accepts_postgres_text_with_fractional_seconds() {
        let parsed = parse_pg_or_rfc3339("2026-05-12 07:39:04.698546+00").expect("parse");
        assert_eq!(parsed.to_rfc3339(), "2026-05-12T07:39:04.698546+00:00");
    }

    #[test]
    fn parse_pg_or_rfc3339_accepts_rfc3339() {
        let parsed = parse_pg_or_rfc3339("2026-05-12T07:39:06+00:00").expect("parse");
        assert_eq!(parsed.to_rfc3339(), "2026-05-12T07:39:06+00:00");
    }

    #[test]
    fn parse_pg_or_rfc3339_accepts_postgres_text_with_colon_offset() {
        // Some Postgres client configurations emit `+00:00` instead of `+00`;
        // `%#z` accepts both variants.
        let parsed =
            parse_pg_or_rfc3339("2026-05-12 07:39:04.698546+00:00").expect("parse");
        assert_eq!(parsed.to_rfc3339(), "2026-05-12T07:39:04.698546+00:00");
    }

    #[test]
    fn parse_pg_or_rfc3339_rejects_garbage() {
        assert!(parse_pg_or_rfc3339("not a date").is_none());
    }
}
