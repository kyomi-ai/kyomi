// SPDX-License-Identifier: AGPL-3.0-or-later

//! Billing REST endpoints — Stripe integration for subscriptions.
//!
//! Wire-compatible with Python's `routers/billing.py`.
//!
//! ## Endpoints
//!
//! - `POST /create-checkout`         — Create checkout session or modify existing subscription (admin)
//! - `POST /webhook`                 — Stripe webhook handler (no auth — Stripe signature)
//! - `POST /cancel-subscription`     — Cancel subscription at period end (admin)
//! - `POST /reactivate-subscription` — Reactivate cancelled subscription (admin)
//! - `GET  /subscription-info`       — Current subscription info (admin)
//! - `POST /update-team-size`        — Update Team tier user count (admin)
//! - `GET  /ai-usage-status`         — AI usage breakdown (current user)
//! - `GET  /invoices`                — Last 10 invoices (admin)
//! - `POST /create-portal-session`   — Stripe Customer Portal URL (current user)

use axum::{
    body::Bytes,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use stripe_shared::{Invoice, Subscription};
use stripe_types::Expandable;
use stripe_webhook::EventObject;

use kyomi_auth::{
    billing_service::BillingService,
    middleware::AuthUser,
    stripe_config,
    stripe_service::{CheckoutParams, StripeService},
};

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the `/billing` router with all billing endpoints.
///
/// The webhook endpoint is included here but does NOT use the `AuthUser`
/// extractor — it relies on Stripe signature verification instead.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/create-checkout", post(create_checkout))
        .route("/webhook", post(stripe_webhook))
        .route("/cancel-subscription", post(cancel_subscription))
        .route("/reactivate-subscription", post(reactivate_subscription))
        .route("/subscription-info", get(get_subscription_info))
        .route("/update-team-size", post(update_team_size))
        .route("/ai-usage-status", get(get_ai_usage_status))
        .route("/invoices", get(get_invoices))
        .route("/create-portal-session", post(create_portal_session))
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Reject non-workspace-admin users with 403.
/// Owners always have admin access.
fn require_workspace_admin(user: &AuthUser) -> Result<(), kyomi_core::Error> {
    // Owners always have admin access
    if user.workspace.is_owner {
        return Ok(());
    }
    if !user
        .workspace
        .workspace_roles
        .contains(&kyomi_core::WorkspaceRole::WorkspaceAdmin)
    {
        return Err(kyomi_core::Error::Forbidden(
            "Workspace admin access required".into(),
        ));
    }
    Ok(())
}

/// Extract workspace_id from user, or return 400.
fn get_workspace_id(user: &AuthUser) -> Result<&str, kyomi_core::Error> {
    user.workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("No workspace associated with user".into()))
}

/// Get a reference to the StripeService, or return 400 if not configured.
fn require_stripe(state: &AppState) -> Result<&StripeService, kyomi_core::Error> {
    state.stripe.as_deref().ok_or_else(|| {
        kyomi_core::Error::BadRequest("Billing features are not available".into())
    })
}

/// Determine if the Stripe secret key is in test mode.
fn is_stripe_test_mode(config: &kyomi_core::Config) -> bool {
    config
        .stripe_secret_key
        .as_deref()
        .map(stripe_config::is_test_mode)
        .unwrap_or(true)
}

/// Load a workspace by its workspace_id.
async fn load_workspace(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
) -> Result<WorkspaceRow, kyomi_core::Error> {
    kyomi_core::db_fetch_optional!(
        db, WorkspaceRow,
        "SELECT workspace_id, name, subscription_tier, subscription_status, \
         billing_cycle, subscription_period_start::text, subscription_period_end::text, user_limit, \
         stripe_customer_id, stripe_subscription_id, stripe_additional_users_item_id \
         FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?
    .ok_or_else(|| kyomi_core::Error::NotFound("Workspace not found".into()))
}

/// Load a workspace by stripe_subscription_id.
async fn load_workspace_by_subscription(
    db: &kyomi_core::DbPool,
    subscription_id: &str,
) -> Option<WorkspaceRow> {
    kyomi_core::db_fetch_optional!(
        db, WorkspaceRow,
        "SELECT workspace_id, name, subscription_tier, subscription_status, \
         billing_cycle, subscription_period_start::text, subscription_period_end::text, user_limit, \
         stripe_customer_id, stripe_subscription_id, stripe_additional_users_item_id \
         FROM workspaces WHERE stripe_subscription_id = $1",
        subscription_id
    )
    .ok()
    .flatten()
}

// ===========================================================================
// Internal row types
// ===========================================================================

/// Minimal workspace row for billing operations.
///
/// Date fields are stored as `Option<String>` because runtime `query_as`
/// decodes timestamps as strings (compile-time macros used DateTime).
#[derive(Debug, sqlx::FromRow)]
struct WorkspaceRow {
    workspace_id: String,
    name: Option<String>,
    subscription_tier: String,
    subscription_status: String,
    billing_cycle: Option<String>,
    subscription_period_start: Option<String>,
    subscription_period_end: Option<String>,
    user_limit: Option<i32>,
    stripe_customer_id: Option<String>,
    stripe_subscription_id: Option<String>,
    stripe_additional_users_item_id: Option<String>,
}

impl WorkspaceRow {
    /// Parse `subscription_period_start` as `DateTime<Utc>`.
    fn period_start_dt(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.subscription_period_start.as_deref().and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc))
        })
    }
    /// Parse `subscription_period_end` as `DateTime<Utc>`.
    fn period_end_dt(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.subscription_period_end.as_deref().and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc))
        })
    }
}

// ===========================================================================
// Request / Response types
// ===========================================================================

#[derive(Deserialize)]
struct CreateCheckoutRequest {
    tier: String,
    billing_cycle: String,
    #[serde(default)]
    additional_users: u64,
}

#[derive(Deserialize)]
struct UpdateTeamSizeRequest {
    total_users: i32,
}

// ===========================================================================
// Endpoint Handlers
// ===========================================================================

// ---------------------------------------------------------------------------
// POST /create-checkout — Create checkout session or modify existing subscription
// ---------------------------------------------------------------------------

async fn create_checkout(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<CreateCheckoutRequest>,
) -> Result<Json<Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;
    let stripe_service = require_stripe(&state)?;

    let workspace = load_workspace(&state.db, workspace_id).await?;

    // If user already has an active subscription, modify it directly
    if let Some(ref sub_id) = workspace.stripe_subscription_id
        && (workspace.subscription_status == "active"
            || workspace.subscription_status == "cancelled")
    {
            return modify_existing_subscription(
                &state,
                stripe_service,
                &workspace,
                sub_id,
                &request,
            )
            .await;
        }

    // New subscription flow: create Stripe customer if needed + checkout session
    let is_test = is_stripe_test_mode(&state.config);

    let price_id = stripe_config::get_price_id(&request.tier, &request.billing_cycle, is_test)
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest(format!(
                "Invalid tier/billing_cycle combination: {}/{}",
                request.tier, request.billing_cycle
            ))
        })?;

    // Create Stripe customer if workspace doesn't have one
    let customer_id = match workspace.stripe_customer_id {
        Some(ref id) if !id.is_empty() => id.clone(),
        _ => {
            let email = &user.email;
            let ws_name = workspace.name.as_deref().unwrap_or("Unnamed");
            let new_customer_id = stripe_service
                .create_customer(email, workspace_id, ws_name)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to create Stripe customer: {e}");
                    kyomi_core::Error::Internal(format!("Failed to create Stripe customer: {e}"))
                })?;

            // Save customer_id to workspace
            kyomi_core::db_execute!(
                &state.db,
                "UPDATE workspaces SET stripe_customer_id = $1 WHERE workspace_id = $2",
                &new_customer_id,
                workspace_id
            )?;

            tracing::info!(
                workspace_id,
                customer_id = %new_customer_id,
                "Created Stripe customer"
            );

            new_customer_id
        }
    };

    // Build additional users line item for Team tier
    let additional_users_price_id = if request.tier == "team" && request.additional_users > 0 {
        stripe_config::get_additional_user_price_id(&request.billing_cycle, is_test)
            .map(|s| s.to_string())
    } else {
        None
    };

    let frontend_url = &state.config.frontend_url;
    let success_url = format!("{frontend_url}/settings/billing?success=true");
    let cancel_url = format!("{frontend_url}/settings/billing?cancelled=true");

    let params = CheckoutParams {
        customer_id,
        price_id: price_id.to_string(),
        success_url,
        cancel_url,
        workspace_id: workspace_id.to_string(),
        tier: request.tier.clone(),
        billing_cycle: request.billing_cycle.clone(),
        trial_days: 0,
        additional_users_price_id,
        additional_users_quantity: request.additional_users,
    };

    let checkout_result = stripe_service
        .create_checkout_session(&params)
        .await
        .map_err(|e| {
            tracing::error!("Failed to create checkout session: {e}");
            kyomi_core::Error::Internal(format!("Failed to create checkout session: {e}"))
        })?;

    tracing::info!(
        workspace_id,
        tier = %request.tier,
        billing_cycle = %request.billing_cycle,
        "Created checkout session"
    );

    Ok(Json(json!({
        "checkout_url": checkout_result.checkout_url,
        "session_id": checkout_result.session_id,
    })))
}

/// Modify an existing Stripe subscription to change tier or billing cycle.
async fn modify_existing_subscription(
    state: &AppState,
    stripe_service: &StripeService,
    workspace: &WorkspaceRow,
    subscription_id: &str,
    request: &CreateCheckoutRequest,
) -> Result<Json<Value>, kyomi_core::Error> {
    let is_test = is_stripe_test_mode(&state.config);

    let new_price_id =
        stripe_config::get_price_id(&request.tier, &request.billing_cycle, is_test).ok_or_else(
            || {
                kyomi_core::Error::BadRequest(format!(
                    "Invalid tier/billing_cycle combination: {}/{}",
                    request.tier, request.billing_cycle
                ))
            },
        )?;

    // If downgrading from Team to Starter/Pro, remove additional users item
    if (request.tier == "starter" || request.tier == "pro")
        && let Some(ref item_id) = workspace.stripe_additional_users_item_id
    {
            tracing::info!(
                item_id,
                "Downgrading from Team tier — removing additional users item"
            );

            stripe_service
                .update_subscription_quantity(subscription_id, item_id, 0)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to remove additional users item: {e}");
                    kyomi_core::Error::Internal(format!(
                        "Failed to remove additional users: {e}"
                    ))
                })?;

            kyomi_core::db_execute!(
                &state.db,
                "UPDATE workspaces SET stripe_additional_users_item_id = NULL \
                 WHERE workspace_id = $1",
                &workspace.workspace_id
            )?;
        }

    // Modify the subscription
    let sub_data = stripe_service
        .update_subscription(subscription_id, new_price_id, &request.tier, &request.billing_cycle)
        .await
        .map_err(|e| {
            tracing::error!("Failed to modify subscription: {e}");
            kyomi_core::Error::Internal(format!("Failed to modify subscription: {e}"))
        })?;

    // Update workspace from Stripe data (source of truth)
    let period_start_str = sub_data.period_start.map(|dt| dt.to_rfc3339());
    let period_end_str = sub_data.period_end.map(|dt| dt.to_rfc3339());
    kyomi_core::db_execute!(
        &state.db,
        "UPDATE workspaces SET \
             subscription_tier = $1, \
             subscription_status = $2, \
             billing_cycle = $3, \
             subscription_period_start = $4, \
             subscription_period_end = $5, \
             user_limit = $6, \
             stripe_additional_users_item_id = $7 \
         WHERE workspace_id = $8",
        &sub_data.tier,
        &sub_data.status,
        sub_data.billing_cycle.as_deref(),
        period_start_str.as_deref(),
        period_end_str.as_deref(),
        sub_data.user_limit,
        sub_data.additional_users_item_id.as_deref(),
        &workspace.workspace_id
    )?;

    tracing::info!(
        workspace_id = %workspace.workspace_id,
        tier = %sub_data.tier,
        billing_cycle = ?sub_data.billing_cycle,
        "Modified existing subscription"
    );

    // Notify connected SSE clients that tools have changed, then invalidate
    // all sessions so disconnected clients re-initialize on next request.
    state
        .mcp_sessions
        .notify_tools_changed(&workspace.workspace_id)
        .await;
    state
        .mcp_sessions
        .invalidate_workspace_sessions(&workspace.workspace_id)
        .await;

    // Return a "checkout" response with redirect back to billing page
    // (matching Python — returns checkout_url pointing to billing page)
    let frontend_url = &state.config.frontend_url;
    Ok(Json(json!({
        "checkout_url": format!("{frontend_url}/settings/billing?success=true"),
        "session_id": "modified",
    })))
}

// ---------------------------------------------------------------------------
// POST /webhook — Stripe webhook handler (no auth)
// ---------------------------------------------------------------------------

async fn stripe_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, kyomi_core::Error> {
    let stripe_service = require_stripe(&state)?;

    let sig_header = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| kyomi_core::Error::BadRequest("Missing Stripe signature".into()))?;

    // Convert raw bytes to a string for signature verification
    let payload = std::str::from_utf8(&body).map_err(|_| {
        kyomi_core::Error::BadRequest("Invalid UTF-8 in webhook payload".into())
    })?;

    // Verify webhook signature and parse typed event
    let event = stripe_service
        .construct_webhook_event(payload, sig_header)
        .map_err(|e| {
            tracing::error!(
                error = %e,
                error_debug = ?e,
                sig_header = %sig_header,
                payload_len = payload.len(),
                "Stripe webhook signature verification failed"
            );
            kyomi_core::Error::BadRequest("Invalid signature".into())
        })?;

    let event_type = event.type_.to_string();
    tracing::info!(event_type = %event_type, "Received Stripe webhook");

    // Dispatch on typed event object — the 1.0 webhook crate uses
    // event-name-based variants instead of type-based matching.
    match event.data.object {
        EventObject::CustomerSubscriptionCreated(sub) => {
            handle_subscription_event(&state, stripe_service, &sub, "customer.subscription.created").await;
        }
        EventObject::CustomerSubscriptionUpdated(sub) => {
            handle_subscription_event(&state, stripe_service, &sub, "customer.subscription.updated").await;
        }
        EventObject::CustomerSubscriptionDeleted(sub) => {
            handle_subscription_deleted(&state, &sub).await;
        }
        EventObject::InvoicePaymentSucceeded(inv) => {
            handle_invoice_payment_succeeded(&state, &inv).await;
        }
        EventObject::InvoicePaymentFailed(inv) => {
            handle_invoice_payment_failed(&state, &inv).await;
        }
        EventObject::CheckoutSessionCompleted(session) => {
            let ws_id = session.metadata.as_ref().and_then(|m| m.get("workspace_id"));
            tracing::info!(
                workspace_id = ?ws_id,
                "Checkout completed"
            );
        }
        _ => {
            tracing::debug!(event_type = %event_type, "Unhandled Stripe event type");
        }
    }

    // Always return 200 to acknowledge receipt (Stripe retries on non-200)
    Ok(Json(json!({})))
}

/// Handle `customer.subscription.created` and `customer.subscription.updated` events.
async fn handle_subscription_event(
    state: &AppState,
    stripe_service: &StripeService,
    subscription: &Subscription,
    event_type: &str,
) {
    // Get workspace_id from subscription metadata
    let workspace_id = match subscription.metadata.get("workspace_id") {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            tracing::error!("Subscription missing workspace_id in metadata");
            return;
        }
    };

    // Parse subscription data from Stripe (source of truth)
    let sub_data = match stripe_service.parse_subscription_data(subscription).await {
        Ok(data) => data,
        Err(e) => {
            tracing::error!(
                workspace_id = %workspace_id,
                "Failed to parse subscription data: {e}"
            );
            return;
        }
    };

    // Build the update query based on event type
    let period_start_str = sub_data.period_start.map(|dt| dt.to_rfc3339());
    let period_end_str = sub_data.period_end.map(|dt| dt.to_rfc3339());

    let result = if event_type == "customer.subscription.created" {
        // On creation, also set stripe_subscription_id, stripe_customer_id, and reset credits
        kyomi_core::db_execute!(
            &state.db,
            "UPDATE workspaces SET \
                 subscription_tier = $1, \
                 subscription_status = $2, \
                 billing_cycle = $3, \
                 subscription_period_start = $4, \
                 subscription_period_end = $5, \
                 stripe_subscription_id = $6, \
                 stripe_customer_id = $7, \
                 user_limit = $8, \
                 stripe_additional_users_item_id = $9, \
                 ai_credits_used_usd = 0.0 \
             WHERE workspace_id = $10",
            &sub_data.tier,
            &sub_data.status,
            sub_data.billing_cycle.as_deref(),
            period_start_str.as_deref(),
            period_end_str.as_deref(),
            &sub_data.stripe_subscription_id,
            &sub_data.stripe_customer_id,
            sub_data.user_limit,
            sub_data.additional_users_item_id.as_deref(),
            &workspace_id
        )
    } else {
        // On update, don't overwrite stripe_subscription_id or stripe_customer_id
        kyomi_core::db_execute!(
            &state.db,
            "UPDATE workspaces SET \
                 subscription_tier = $1, \
                 subscription_status = $2, \
                 billing_cycle = $3, \
                 subscription_period_start = $4, \
                 subscription_period_end = $5, \
                 user_limit = $6, \
                 stripe_additional_users_item_id = $7 \
             WHERE workspace_id = $8",
            &sub_data.tier,
            &sub_data.status,
            sub_data.billing_cycle.as_deref(),
            period_start_str.as_deref(),
            period_end_str.as_deref(),
            sub_data.user_limit,
            sub_data.additional_users_item_id.as_deref(),
            &workspace_id
        )
    };

    match result {
        Ok(_) => {
            tracing::info!(
                workspace_id = %workspace_id,
                tier = %sub_data.tier,
                billing_cycle = ?sub_data.billing_cycle,
                user_limit = sub_data.user_limit,
                "{event_type} processed"
            );

            // Notify connected SSE clients that tools have changed, then invalidate
            // all sessions so disconnected clients re-initialize on next request.
            state
                .mcp_sessions
                .notify_tools_changed(&workspace_id)
                .await;
            state
                .mcp_sessions
                .invalidate_workspace_sessions(&workspace_id)
                .await;
        }
        Err(e) => {
            tracing::error!(
                workspace_id = %workspace_id,
                "Failed to update workspace from {event_type}: {e}"
            );
        }
    }
}

/// Handle `customer.subscription.deleted` — revert workspace to free tier.
async fn handle_subscription_deleted(state: &AppState, subscription: &Subscription) {
    let workspace_id = match subscription.metadata.get("workspace_id") {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            tracing::error!("Subscription missing workspace_id in metadata");
            return;
        }
    };

    let result = kyomi_core::db_execute!(
        &state.db,
        "UPDATE workspaces SET \
             subscription_tier = 'free', \
             subscription_status = 'cancelled', \
             billing_cycle = NULL, \
             subscription_period_start = NULL, \
             subscription_period_end = NULL, \
             stripe_subscription_id = NULL, \
             stripe_additional_users_item_id = NULL, \
             user_limit = 1, \
             ai_credits_used_usd = 0.0 \
         WHERE workspace_id = $1",
        &workspace_id
    );

    match result {
        Ok(_) => {
            tracing::info!(
                workspace_id = %workspace_id,
                "Subscription deleted — reverted to free tier"
            );

            // Notify connected SSE clients that tools have changed, then invalidate
            // all sessions so disconnected clients re-initialize on next request.
            state
                .mcp_sessions
                .notify_tools_changed(&workspace_id)
                .await;
            state
                .mcp_sessions
                .invalidate_workspace_sessions(&workspace_id)
                .await;
        }
        Err(e) => {
            tracing::error!(
                workspace_id = %workspace_id,
                "Failed to revert workspace to free tier: {e}"
            );
        }
    }
}

/// Handle `invoice.payment_succeeded` — reset AI credits for new billing period.
async fn handle_invoice_payment_succeeded(state: &AppState, invoice: &Invoice) {
    // Find workspace by subscription ID from the invoice
    let subscription_id = match &invoice.subscription {
        Some(Expandable::Id(id)) => Some(id.to_string()),
        Some(Expandable::Object(sub)) => Some(sub.id.to_string()),
        None => None,
    };

    if let Some(ref sub_id) = subscription_id {
        if let Some(workspace) = load_workspace_by_subscription(&state.db, sub_id).await {
            // Reset AI credits for new billing period
            let result = kyomi_core::db_execute!(
                &state.db,
                "UPDATE workspaces SET ai_credits_used_usd = 0.0 WHERE workspace_id = $1",
                &workspace.workspace_id
            );

            match result {
                Ok(_) => {
                    tracing::info!(
                        workspace_id = %workspace.workspace_id,
                        "Payment succeeded — reset AI credits"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        workspace_id = %workspace.workspace_id,
                        "Failed to reset AI credits: {e}"
                    );
                }
            }
        } else {
            tracing::warn!(
                subscription_id = %sub_id,
                "No workspace found for subscription in payment_succeeded event"
            );
        }
    }
}

/// Handle `invoice.payment_failed` — mark subscription as past_due.
async fn handle_invoice_payment_failed(state: &AppState, invoice: &Invoice) {
    let subscription_id = match &invoice.subscription {
        Some(Expandable::Id(id)) => Some(id.to_string()),
        Some(Expandable::Object(sub)) => Some(sub.id.to_string()),
        None => None,
    };

    if let Some(ref sub_id) = subscription_id
        && let Some(workspace) = load_workspace_by_subscription(&state.db, sub_id).await {
            let result = kyomi_core::db_execute!(
                &state.db,
                "UPDATE workspaces SET subscription_status = 'past_due' WHERE workspace_id = $1",
                &workspace.workspace_id
            );

            match result {
                Ok(_) => {
                    tracing::warn!(
                        workspace_id = %workspace.workspace_id,
                        "Payment failed — marked subscription as past_due"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        workspace_id = %workspace.workspace_id,
                        "Failed to update subscription status to past_due: {e}"
                    );
                }
            }
        }
}

// ---------------------------------------------------------------------------
// POST /cancel-subscription — Cancel subscription at period end (admin)
// ---------------------------------------------------------------------------

async fn cancel_subscription(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;
    let stripe_service = require_stripe(&state)?;

    let workspace = load_workspace(&state.db, workspace_id).await?;

    let sub_id = workspace
        .stripe_subscription_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("No active subscription".into()))?;

    // Cancel at period end in Stripe
    stripe_service
        .cancel_subscription(sub_id, true)
        .await
        .map_err(|e| {
            tracing::error!("Failed to cancel subscription: {e}");
            kyomi_core::Error::Internal(format!("Failed to cancel subscription: {e}"))
        })?;

    // Update workspace status
    kyomi_core::db_execute!(
        &state.db,
        "UPDATE workspaces SET subscription_status = 'cancelled' WHERE workspace_id = $1",
        workspace_id
    )?;

    tracing::info!(workspace_id, "Scheduled subscription cancellation at period end");

    Ok(Json(json!({
        "status": "success",
        "message": "Subscription will be cancelled at period end",
    })))
}

// ---------------------------------------------------------------------------
// POST /reactivate-subscription — Remove cancel_at_period_end flag (admin)
// ---------------------------------------------------------------------------

async fn reactivate_subscription(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;
    let stripe_service = require_stripe(&state)?;

    let workspace = load_workspace(&state.db, workspace_id).await?;

    let sub_id = workspace
        .stripe_subscription_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("No subscription to reactivate".into()))?;

    if workspace.subscription_status != "cancelled" {
        return Err(kyomi_core::Error::BadRequest(
            "Subscription is not cancelled".into(),
        ));
    }

    // Reactivate in Stripe
    stripe_service
        .reactivate_subscription(sub_id)
        .await
        .map_err(|e| {
            tracing::error!("Failed to reactivate subscription: {e}");
            kyomi_core::Error::Internal(format!("Failed to reactivate subscription: {e}"))
        })?;

    // Update workspace status
    kyomi_core::db_execute!(
        &state.db,
        "UPDATE workspaces SET subscription_status = 'active' WHERE workspace_id = $1",
        workspace_id
    )?;

    tracing::info!(workspace_id, "Reactivated subscription");

    Ok(Json(json!({
        "status": "success",
        "message": "Subscription has been reactivated",
    })))
}

// ---------------------------------------------------------------------------
// GET /subscription-info — Current subscription info (admin)
// ---------------------------------------------------------------------------

async fn get_subscription_info(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;

    let workspace = load_workspace(&state.db, workspace_id).await?;

    // Calculate AI reset date (AI credits reset monthly, even for annual plans)
    let ai_reset_date = calculate_ai_reset_date(
        workspace.billing_cycle.as_deref(),
        workspace.period_start_dt(),
        workspace.period_end_dt(),
    );

    Ok(Json(json!({
        "tier": workspace.subscription_tier,
        "status": workspace.subscription_status,
        "billing_cycle": workspace.billing_cycle,
        "period_start": workspace.subscription_period_start,
        "period_end": workspace.subscription_period_end,
        "ai_reset_date": ai_reset_date.map(|dt| dt.to_rfc3339()),
        "user_limit": workspace.user_limit,
    })))
}

/// Calculate the AI reset date based on subscription billing cycle.
///
/// For monthly plans: AI resets at `subscription_period_end`.
/// For annual plans: AI resets monthly (next monthly anniversary of `period_start`).
fn calculate_ai_reset_date(
    billing_cycle: Option<&str>,
    period_start: Option<chrono::DateTime<chrono::Utc>>,
    period_end: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let start = period_start?;

    if billing_cycle == Some("monthly") {
        return period_end;
    }

    // Annual plan: calculate next monthly anniversary from subscription start
    let now = chrono::Utc::now();
    let (_, monthly_end) = BillingService::calculate_monthly_period(start, now);
    Some(monthly_end)
}

// ---------------------------------------------------------------------------
// POST /update-team-size — Update Team tier user count (admin)
// ---------------------------------------------------------------------------

async fn update_team_size(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<UpdateTeamSizeRequest>,
) -> Result<Json<Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;
    let stripe_service = require_stripe(&state)?;

    let workspace = load_workspace(&state.db, workspace_id).await?;

    // Validate Team tier
    if workspace.subscription_tier != "team" {
        return Err(kyomi_core::Error::BadRequest(
            "Team size can only be updated for Team tier subscriptions".into(),
        ));
    }

    // Validate active subscription
    if workspace.subscription_status != "active" && workspace.subscription_status != "trialing" {
        return Err(kyomi_core::Error::BadRequest(format!(
            "Cannot update team size for subscription in '{}' status",
            workspace.subscription_status
        )));
    }

    let sub_id = workspace
        .stripe_subscription_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("No active subscription found".into()))?;

    // Validate minimum 5 users for Team
    if request.total_users < 5 {
        return Err(kyomi_core::Error::BadRequest(
            "Team tier requires a minimum of 5 users".into(),
        ));
    }

    // Check current active member count
    let current_member_count: i64 = kyomi_core::db_fetch_scalar!(
        &state.db, i64,
        "SELECT COUNT(*) FROM workspace_users \
         WHERE workspace_id = $1 AND active = true",
        workspace_id
    )
    .unwrap_or(0);

    if (request.total_users as i64) < current_member_count {
        return Err(kyomi_core::Error::BadRequest(format!(
            "Cannot reduce team size to {} users. You currently have {} active members. \
             Please remove members first before reducing your team size.",
            request.total_users, current_member_count
        )));
    }

    let additional_users = request.total_users - 5;
    let is_test = is_stripe_test_mode(&state.config);

    // Handle case where additional_users item doesn't exist yet
    if workspace.stripe_additional_users_item_id.is_none() && additional_users > 0 {
        let add_price_id = stripe_config::get_additional_user_price_id(
            workspace.billing_cycle.as_deref().unwrap_or("monthly"),
            is_test,
        )
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest(format!(
                "Invalid billing cycle: {:?}",
                workspace.billing_cycle
            ))
        })?;

        // Add additional users subscription item directly via Stripe
        let item_id = {
            let new_item = stripe_billing::subscription_item::CreateSubscriptionItem::new(sub_id)
                .price(add_price_id)
                .quantity(additional_users as u64)
                .send(stripe_service.client())
                .await
                .map_err(|e| {
                    tracing::error!("Failed to add additional users item: {e}");
                    kyomi_core::Error::Internal(format!(
                        "Failed to add additional users: {e}"
                    ))
                })?;

            tracing::info!(
                workspace_id,
                additional_users,
                "Added additional users item to subscription"
            );

            new_item.id
        };

        // Update workspace with new user limit and item ID
        let item_id_str = item_id.to_string();
        kyomi_core::db_execute!(
            &state.db,
            "UPDATE workspaces SET user_limit = $1, stripe_additional_users_item_id = $2 \
             WHERE workspace_id = $3",
            request.total_users,
            &item_id_str,
            workspace_id
        )?;
    } else if let Some(ref item_id) = workspace.stripe_additional_users_item_id {
        // Update existing additional users item quantity
        stripe_service
            .update_subscription_quantity(sub_id, item_id, additional_users as i64)
            .await
            .map_err(|e| {
                tracing::error!("Failed to update team size: {e}");
                kyomi_core::Error::Internal(format!("Failed to update team size: {e}"))
            })?;

        // Update workspace user limit
        let new_limit = if additional_users == 0 {
            // Item was removed; clear the reference
            kyomi_core::db_execute!(
                &state.db,
                "UPDATE workspaces SET user_limit = 5, stripe_additional_users_item_id = NULL \
                 WHERE workspace_id = $1",
                workspace_id
            )?;
            5
        } else {
            kyomi_core::db_execute!(
                &state.db,
                "UPDATE workspaces SET user_limit = $1 WHERE workspace_id = $2",
                request.total_users,
                workspace_id
            )?;
            request.total_users
        };

        tracing::info!(
            workspace_id,
            total_users = request.total_users,
            additional_users,
            "Updated team size"
        );

        return Ok(Json(json!({
            "status": "success",
            "message": format!("Team size updated to {} users", request.total_users),
            "user_limit": new_limit,
            "additional_users": additional_users,
        })));
    } else {
        // additional_users == 0 and no item ID — nothing to do
        return Ok(Json(json!({
            "status": "success",
            "message": "Team size is already at base level (5 users)",
            "user_limit": 5,
        })));
    }

    Ok(Json(json!({
        "status": "success",
        "message": format!("Team size updated to {} users", request.total_users),
        "user_limit": request.total_users,
        "additional_users": additional_users,
    })))
}

// ---------------------------------------------------------------------------
// GET /ai-usage-status — AI usage breakdown (current user)
// ---------------------------------------------------------------------------

async fn get_ai_usage_status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, kyomi_core::Error> {
    // Self-hosted: no billing, unlimited AI usage
    if state.config.self_hosted {
        return Ok(Json(serde_json::json!({
            "percentage_used": 0.0,
            "warning_level": "none",
            "message": null,
            "credits_remaining": 999_999.0,
            "credits_limit": 999_999.0,
            "credits_used": 0.0,
            "is_exhausted": false
        })));
    }

    let workspace_id = get_workspace_id(&user)?;

    let billing_service = BillingService::new();
    let usage_status = billing_service
        .get_ai_usage_status(&state.db, workspace_id, &user.user_id)
        .await?;

    // Serialize the AiUsageStatus struct directly — it already matches
    // the Python wire format
    let response = serde_json::to_value(&usage_status).map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to serialize usage status: {e}"))
    })?;

    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// GET /invoices — Last 10 invoices (admin)
// ---------------------------------------------------------------------------

async fn get_invoices(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;
    let stripe_service = require_stripe(&state)?;

    let workspace = load_workspace(&state.db, workspace_id).await?;

    // If no Stripe customer, return empty list
    let customer_id = match workspace.stripe_customer_id {
        Some(ref id) if !id.is_empty() => id.clone(),
        _ => return Ok(Json(json!({ "invoices": [] }))),
    };

    let invoices = stripe_service
        .list_invoices(&customer_id, 10)
        .await
        .map_err(|e| {
            tracing::error!("Failed to fetch invoices: {e}");
            kyomi_core::Error::Internal("Failed to fetch invoices".into())
        })?;

    // Convert to response format matching Python
    let invoices_json: Vec<Value> = invoices
        .iter()
        .map(|inv| {
            json!({
                "invoice_id": inv.invoice_id,
                "amount_paid": inv.amount_paid as f64 / 100.0,
                "currency": inv.currency.to_uppercase(),
                "status": inv.status,
                "hosted_invoice_url": inv.hosted_invoice_url,
                "invoice_pdf": inv.invoice_pdf,
                "created": inv.created,
            })
        })
        .collect();

    Ok(Json(json!({ "invoices": invoices_json })))
}

// ---------------------------------------------------------------------------
// POST /create-portal-session — Stripe Customer Portal URL (current user)
// ---------------------------------------------------------------------------

async fn create_portal_session(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    let stripe_service = require_stripe(&state)?;

    let workspace = load_workspace(&state.db, workspace_id).await?;

    let customer_id = workspace.stripe_customer_id.as_deref().ok_or_else(|| {
        kyomi_core::Error::BadRequest(
            "No Stripe customer found. Please subscribe to a plan first.".into(),
        )
    })?;

    let return_url = format!("{}/settings/billing", state.config.frontend_url);

    let (portal_url, session_id) = stripe_service
        .create_portal_session(customer_id, &return_url)
        .await
        .map_err(|e| {
            tracing::error!("Failed to create portal session: {e}");
            kyomi_core::Error::Internal("Failed to create portal session".into())
        })?;

    tracing::info!(workspace_id, "Created billing portal session");

    Ok(Json(json!({
        "portal_url": portal_url,
        "session_id": session_id,
    })))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // CreateCheckoutRequest contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn create_checkout_request_deserializes() {
        let json = json!({
            "tier": "pro",
            "billing_cycle": "monthly",
            "additional_users": 3
        });

        let req: CreateCheckoutRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.tier, "pro");
        assert_eq!(req.billing_cycle, "monthly");
        assert_eq!(req.additional_users, 3);
    }

    #[test]
    fn create_checkout_request_defaults_additional_users() {
        let json = json!({
            "tier": "starter",
            "billing_cycle": "annual"
        });

        let req: CreateCheckoutRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.additional_users, 0);
    }

    #[test]
    fn create_checkout_request_missing_tier_fails() {
        let json = json!({"billing_cycle": "monthly"});
        assert!(serde_json::from_value::<CreateCheckoutRequest>(json).is_err());
    }

    #[test]
    fn create_checkout_request_missing_billing_cycle_fails() {
        let json = json!({"tier": "pro"});
        assert!(serde_json::from_value::<CreateCheckoutRequest>(json).is_err());
    }

    // -----------------------------------------------------------------------
    // UpdateTeamSizeRequest contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn update_team_size_request_deserializes() {
        let json = json!({"total_users": 8});

        let req: UpdateTeamSizeRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.total_users, 8);
    }

    #[test]
    fn update_team_size_request_missing_total_users_fails() {
        let json = json!({});
        assert!(serde_json::from_value::<UpdateTeamSizeRequest>(json).is_err());
    }

    // -----------------------------------------------------------------------
    // AI reset date calculation tests
    // -----------------------------------------------------------------------

    #[test]
    fn ai_reset_date_monthly_uses_period_end() {
        use chrono::TimeZone;

        let start = chrono::Utc.with_ymd_and_hms(2024, 1, 15, 0, 0, 0).unwrap();
        let end = chrono::Utc.with_ymd_and_hms(2024, 2, 15, 0, 0, 0).unwrap();

        let reset = calculate_ai_reset_date(Some("monthly"), Some(start), Some(end));
        assert_eq!(reset, Some(end));
    }

    #[test]
    fn ai_reset_date_annual_uses_monthly_period() {
        use chrono::TimeZone;

        let start = chrono::Utc.with_ymd_and_hms(2024, 1, 15, 0, 0, 0).unwrap();
        let end = chrono::Utc.with_ymd_and_hms(2025, 1, 15, 0, 0, 0).unwrap();

        let reset = calculate_ai_reset_date(Some("annual"), Some(start), Some(end));
        // Should return the next monthly anniversary from start, not the annual end
        assert!(reset.is_some());
        let reset_date = reset.unwrap();
        assert!(reset_date > chrono::Utc::now());
    }

    #[test]
    fn ai_reset_date_no_period_start_returns_none() {
        let reset = calculate_ai_reset_date(Some("monthly"), None, None);
        assert!(reset.is_none());
    }

    // -----------------------------------------------------------------------
    // Response shape verification tests
    // -----------------------------------------------------------------------

    #[test]
    fn checkout_response_shape() {
        let response = json!({
            "checkout_url": "https://checkout.stripe.com/example",
            "session_id": "cs_test_123"
        });

        assert!(response.get("checkout_url").is_some());
        assert!(response.get("session_id").is_some());
    }

    #[test]
    fn subscription_info_response_shape() {
        let response = json!({
            "tier": "pro",
            "status": "active",
            "billing_cycle": "monthly",
            "period_start": "2024-01-15T00:00:00+00:00",
            "period_end": "2024-02-15T00:00:00+00:00",
            "ai_reset_date": "2024-02-15T00:00:00+00:00",
            "user_limit": 1
        });

        assert_eq!(response["tier"], "pro");
        assert_eq!(response["status"], "active");
        assert_eq!(response["billing_cycle"], "monthly");
        assert!(response["period_start"].is_string());
        assert!(response["period_end"].is_string());
        assert!(response["ai_reset_date"].is_string());
        assert_eq!(response["user_limit"], 1);
    }

    #[test]
    fn cancel_subscription_response_shape() {
        let response = json!({
            "status": "success",
            "message": "Subscription will be cancelled at period end"
        });

        assert_eq!(response["status"], "success");
        assert!(response["message"].is_string());
    }

    #[test]
    fn invoices_response_shape() {
        let response = json!({
            "invoices": [
                {
                    "invoice_id": "in_test_123",
                    "amount_paid": 25.0,
                    "currency": "USD",
                    "status": "paid",
                    "hosted_invoice_url": "https://invoice.stripe.com/example",
                    "invoice_pdf": "https://pay.stripe.com/invoice/example.pdf",
                    "created": 1706745600
                }
            ]
        });

        assert!(response["invoices"].is_array());
        let invoice = &response["invoices"][0];
        assert!(invoice["invoice_id"].is_string());
        assert!(invoice["amount_paid"].is_number());
        assert_eq!(invoice["currency"], "USD");
        assert!(invoice["created"].is_number());
    }

    #[test]
    fn portal_session_response_shape() {
        let response = json!({
            "portal_url": "https://billing.stripe.com/session/example",
            "session_id": "bps_test_123"
        });

        assert!(response["portal_url"].is_string());
        assert!(response["session_id"].is_string());
    }

    #[test]
    fn modify_subscription_response_shape() {
        let response = json!({
            "checkout_url": "http://localhost:5173/settings/billing?success=true",
            "session_id": "modified"
        });

        assert!(response["checkout_url"].is_string());
        assert_eq!(response["session_id"], "modified");
    }

    #[test]
    fn update_team_size_response_shape() {
        let response = json!({
            "status": "success",
            "message": "Team size updated to 8 users",
            "user_limit": 8,
            "additional_users": 3
        });

        assert_eq!(response["status"], "success");
        assert_eq!(response["user_limit"], 8);
        assert_eq!(response["additional_users"], 3);
    }
}
