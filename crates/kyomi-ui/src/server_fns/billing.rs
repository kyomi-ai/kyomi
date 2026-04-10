// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for Billing settings.
//!
//! These replace the REST API calls that BillingPanel.jsx makes:
//! - `GET /billing/subscription-info` -> `get_subscription_info()`
//! - `GET /billing/invoices` -> `get_invoices()`
//! - `POST /billing/create-checkout` -> `create_checkout()`
//! - `POST /billing/cancel-subscription` -> `cancel_subscription()`
//! - `POST /billing/reactivate-subscription` -> `reactivate_subscription()`
//! - `POST /billing/update-team-size` -> `update_team_size()`
//! - `POST /billing/create-portal-session` -> `create_portal_session()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/billing.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, workspace_id};

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Subscription information for the current workspace.
///
/// Matches the JSON shape returned by `GET /billing/subscription-info`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubscriptionInfo {
    pub tier: String,
    pub status: String,
    pub billing_cycle: Option<String>,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    pub ai_reset_date: Option<String>,
    pub user_limit: Option<i32>,
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

/// Result of a mutation (cancel, reactivate, update team size).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BillingResult {
    pub message: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// SSR-only helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Reject non-workspace-admin/non-owner users.
#[cfg(feature = "ssr")]
fn require_workspace_admin(
    auth: &kyomi_auth::middleware::AuthUser,
) -> Result<(), ServerFnError> {
    if auth.workspace.is_owner {
        return Ok(());
    }
    if !auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
    {
        return Err(ServerFnError::new("Workspace admin access required"));
    }
    Ok(())
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
    user_limit: Option<i32>,
    stripe_customer_id: Option<String>,
    stripe_subscription_id: Option<String>,
}

#[cfg(feature = "ssr")]
impl WorkspaceRow {
    fn period_start_dt(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.subscription_period_start.as_deref().and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc))
        })
    }
    fn period_end_dt(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.subscription_period_end.as_deref().and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc))
        })
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
         user_limit, \
         stripe_customer_id, stripe_subscription_id \
         FROM workspaces WHERE workspace_id = $1",
        ws_id
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?
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

/// Determine if the Stripe secret key is in test mode.
#[cfg(feature = "ssr")]
fn is_stripe_test_mode(config: &kyomi_core::Config) -> bool {
    config
        .stripe_secret_key
        .as_deref()
        .map(kyomi_auth::stripe_config::is_test_mode)
        .unwrap_or(true)
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

    require_workspace_admin(&auth)?;
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

    Ok(SubscriptionInfo {
        tier: workspace.subscription_tier,
        status: workspace.subscription_status,
        billing_cycle: workspace.billing_cycle,
        period_start: workspace.subscription_period_start,
        period_end: workspace.subscription_period_end,
        ai_reset_date,
        user_limit: workspace.user_limit,
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

    require_workspace_admin(&auth)?;
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

/// Create a Stripe checkout session and return the redirect URL.
///
/// Mirrors `POST /api/v1/billing/create-checkout`.
///
/// Cloud plan — single tier, per-seat pricing. `quantity` is the number of
/// seats (users) to subscribe for.
#[server(prefix = "/leptos-api")]
pub async fn create_checkout(
    quantity: u64,
) -> Result<RedirectUrl, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_admin(&auth)?;
    let ws_id = workspace_id(&auth)?;

    let stripe_service = require_stripe(&ctx.config)?;
    let workspace = load_workspace(&ctx.db, ws_id).await?;

    // If user already has an active subscription, modify it directly
    if let Some(ref sub_id) = workspace.stripe_subscription_id
        && (workspace.subscription_status == "active"
            || workspace.subscription_status == "cancelled")
    {
        return modify_existing_subscription(
            &ctx.db,
            &ctx.config,
            &stripe_service,
            sub_id,
            ws_id,
        )
        .await;
    }

    // New subscription flow
    let is_test = is_stripe_test_mode(&ctx.config);

    // Cloud plan — single price, tier/billing_cycle params ignored by get_price_id
    let price_id = kyomi_auth::stripe_config::get_price_id("cloud", "monthly", is_test)
        .ok_or_else(|| {
            ServerFnError::new("Cloud price ID not configured")
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
            .map_err(|e| ServerFnError::new(e.to_string()))?;

            new_customer_id
        }
    };

    let frontend_url = &ctx.config.frontend_url;
    let success_url = format!("{frontend_url}/settings/billing?checkout=success");
    let cancel_url = format!("{frontend_url}/settings/billing?checkout=cancelled");

    let params = kyomi_auth::stripe_service::CheckoutParams {
        customer_id,
        price_id: price_id.to_string(),
        success_url,
        cancel_url,
        workspace_id: ws_id.to_string(),
        quantity,
        trial_days: 30,
    };

    let checkout_result = stripe_service
        .create_checkout_session(&params)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to create checkout session: {e}")))?;

    Ok(RedirectUrl {
        url: checkout_result.checkout_url,
    })
}

/// Modify an existing Stripe subscription (reactivate cancelled plan).
///
/// Cloud plan — single tier, so the only meaningful modification is
/// reactivating a cancelled subscription on the same price. Tier changes
/// are not applicable since there is only one Cloud tier.
#[cfg(feature = "ssr")]
async fn modify_existing_subscription(
    db: &kyomi_core::DbPool,
    config: &kyomi_core::Config,
    stripe_service: &kyomi_auth::stripe_service::StripeService,
    subscription_id: &str,
    workspace_id: &str,
) -> Result<RedirectUrl, ServerFnError> {
    let is_test = is_stripe_test_mode(config);

    let new_price_id =
        kyomi_auth::stripe_config::get_price_id("cloud", "monthly", is_test).ok_or_else(
            || ServerFnError::new("Cloud price ID not configured"),
        )?;

    // Modify the subscription to the Cloud price
    let sub_data = stripe_service
        .update_subscription(subscription_id, new_price_id, "cloud", "monthly")
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to modify subscription: {e}")))?;

    // Write Stripe state back to DB immediately (don't wait for webhook)
    let period_start_str = sub_data.period_start.map(|dt| dt.to_rfc3339());
    let period_end_str = sub_data.period_end.map(|dt| dt.to_rfc3339());
    kyomi_core::db_execute!(
        db,
        "UPDATE workspaces SET \
             subscription_tier = $1, \
             subscription_status = $2, \
             billing_cycle = $3, \
             subscription_period_start = $4, \
             subscription_period_end = $5, \
             user_limit = $6 \
         WHERE workspace_id = $7",
        &sub_data.tier,
        &sub_data.status,
        sub_data.billing_cycle.as_deref(),
        period_start_str.as_deref(),
        period_end_str.as_deref(),
        sub_data.user_limit,
        workspace_id
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Return redirect back to billing page with success param
    let frontend_url = &config.frontend_url;
    Ok(RedirectUrl {
        url: format!("{frontend_url}/settings/billing?checkout=success"),
    })
}

/// Cancel the current subscription at period end.
///
/// Mirrors `POST /api/v1/billing/cancel-subscription`.
#[server(prefix = "/leptos-api")]
pub async fn cancel_subscription() -> Result<BillingResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_admin(&auth)?;
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
    .map_err(|e| ServerFnError::new(e.to_string()))?;

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
    require_workspace_admin(&auth)?;
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
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(BillingResult {
        message: "Subscription has been reactivated".to_string(),
    })
}

/// Update the user seat count for a Cloud subscription.
///
/// Mirrors `POST /api/v1/billing/update-team-size`.
///
/// Cloud plan — per-seat pricing. Uses `update_subscription` to modify the
/// subscription's primary line item quantity via a price update (same price,
/// new quantity applied by Stripe). Works for all Cloud users (no tier gate).
#[server(prefix = "/leptos-api")]
pub async fn update_team_size(total_users: i32) -> Result<BillingResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_admin(&auth)?;
    let ws_id = workspace_id(&auth)?;

    let stripe_service = require_stripe(&ctx.config)?;
    let workspace = load_workspace(&ctx.db, ws_id).await?;

    if workspace.subscription_status != "active" && workspace.subscription_status != "trialing" {
        return Err(ServerFnError::new(format!(
            "Cannot update team size for subscription in '{}' status",
            workspace.subscription_status
        )));
    }

    let sub_id = workspace
        .stripe_subscription_id
        .as_deref()
        .ok_or_else(|| ServerFnError::new("No active subscription found"))?;

    if total_users < 1 {
        return Err(ServerFnError::new(
            "At least 1 user seat is required",
        ));
    }

    // Check current active member count
    let current_member_count: i64 = kyomi_core::db_fetch_scalar!(
        &ctx.db,
        i64,
        "SELECT COUNT(*) FROM workspace_users \
         WHERE workspace_id = $1 AND active = true",
        ws_id
    )
    .unwrap_or(0);

    if (total_users as i64) < current_member_count {
        return Err(ServerFnError::new(format!(
            "Cannot reduce to {} seats. You currently have {} active members. \
             Please remove members first before reducing your seat count.",
            total_users, current_member_count
        )));
    }

    // Update the seat count on the subscription's main item.
    stripe_service
        .update_seat_count(sub_id, total_users as u64)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to update user count: {e}")))?;

    // Update workspace user_limit
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE workspaces SET user_limit = $1 WHERE workspace_id = $2",
        total_users,
        ws_id
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(BillingResult {
        message: format!("Seat count updated to {} users", total_users),
    })
}

/// Create a Stripe billing portal session and return the redirect URL.
///
/// Mirrors `POST /api/v1/billing/create-portal-session`.
#[server(prefix = "/leptos-api")]
pub async fn create_portal_session() -> Result<RedirectUrl, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let stripe_service = require_stripe(&ctx.config)?;
    let workspace = load_workspace(&ctx.db, ws_id).await?;

    let customer_id = workspace.stripe_customer_id.as_deref().ok_or_else(|| {
        ServerFnError::new("No Stripe customer found. Please subscribe to a plan first.")
    })?;

    let return_url = format!("{}/settings/billing", ctx.config.frontend_url);

    let (portal_url, _session_id) = stripe_service
        .create_portal_session(customer_id, &return_url)
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to create portal session: {e}")))?;

    Ok(RedirectUrl { url: portal_url })
}
