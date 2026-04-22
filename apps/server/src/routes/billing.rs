// SPDX-License-Identifier: AGPL-3.0-or-later

//! Billing REST endpoints — Stripe webhook only.
//!
//! This module contains only the Stripe webhook handler. All user-facing
//! billing operations (checkout, subscription management, invoices, portal)
//! are served by Leptos server_fns in `crates/kyomi-ui/src/server_fns/billing.rs`.
//!
//! The webhook endpoint is exempt from the server_fn migration because Stripe
//! POSTs to it directly — it is not initiated by an authenticated browser session.

use axum::{
    body::Bytes,
    extract::State,
    http::HeaderMap,
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use stripe_shared::{Invoice, Subscription};
use stripe_types::Expandable;
use stripe_webhook::EventObject;

use kyomi_auth::stripe_service::StripeService;

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the `/billing` router — webhook endpoint only.
///
/// The webhook endpoint does NOT use the `AuthUser` extractor — it relies on
/// Stripe signature verification instead.
pub fn routes() -> Router<AppState> {
    Router::new().route("/webhook", post(stripe_webhook))
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Get a reference to the StripeService, or return 400 if not configured.
fn require_stripe(state: &AppState) -> Result<&StripeService, kyomi_core::Error> {
    state.stripe.as_deref().ok_or_else(|| {
        kyomi_core::Error::BadRequest("Billing features are not available".into())
    })
}

/// Load a workspace by stripe_subscription_id.
async fn load_workspace_by_subscription(
    db: &kyomi_core::DbPool,
    subscription_id: &str,
) -> Option<WorkspaceRow> {
    kyomi_core::db_fetch_optional!(
        db, WorkspaceRow,
        "SELECT workspace_id FROM workspaces WHERE stripe_subscription_id = $1",
        subscription_id
    )
    .ok()
    .flatten()
}

// ===========================================================================
// Internal row types
// ===========================================================================

/// Minimal workspace row used by webhook event handlers.
///
/// Only `workspace_id` is accessed in code; the struct exists to satisfy
/// `sqlx::FromRow` for the subscription-lookup query.
#[derive(Debug, sqlx::FromRow)]
struct WorkspaceRow {
    workspace_id: String,
}

// ===========================================================================
// Webhook handler
// ===========================================================================

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
            handle_checkout_completed(&state, &session).await;
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

    // Build the update query based on event type.
    //
    // `sub_data.period_start` / `period_end` are already `Option<DateTime<Utc>>`.
    // Bind them directly — do NOT convert to RFC3339 strings. Postgres's
    // `timestamp with time zone` column rejects `text` binds with
    // `column … is of type timestamp with time zone but expression is of type text`
    // (the KYO-106 production bug). sqlx's chrono integration maps
    // `Option<DateTime<Utc>>` to `TIMESTAMPTZ` natively.
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
                 ai_credits_used_usd = 0.0 \
             WHERE workspace_id = $9",
            &sub_data.tier,
            &sub_data.status,
            sub_data.billing_cycle.as_deref(),
            sub_data.period_start,
            sub_data.period_end,
            &sub_data.stripe_subscription_id,
            &sub_data.stripe_customer_id,
            sub_data.user_limit,
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
                 user_limit = $6 \
             WHERE workspace_id = $7",
            &sub_data.tier,
            &sub_data.status,
            sub_data.billing_cycle.as_deref(),
            sub_data.period_start,
            sub_data.period_end,
            sub_data.user_limit,
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

/// Handle `checkout.session.completed` — fulfill one-time bundle purchases.
///
/// Subscription checkouts are handled by `customer.subscription.created` instead.
/// This handler only processes one-time payment sessions (bundle purchases).
async fn handle_checkout_completed(state: &AppState, session: &stripe_shared::CheckoutSession) {
    let metadata = match &session.metadata {
        Some(m) => m,
        None => return,
    };

    let workspace_id = match metadata.get("workspace_id") {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            tracing::debug!("Checkout session has no workspace_id in metadata — skipping");
            return;
        }
    };

    // Only process one-time payments (bundle purchases). Subscription checkouts
    // are handled by the customer.subscription.created event.
    if session.mode != stripe_shared::CheckoutSessionMode::Payment {
        tracing::info!(
            workspace_id = %workspace_id,
            "Subscription checkout completed"
        );
        return;
    }

    let purchase_type = match metadata.get("purchase_type") {
        Some(pt) => pt.clone(),
        None => {
            tracing::warn!(
                workspace_id = %workspace_id,
                "Payment checkout completed but no purchase_type in metadata"
            );
            return;
        }
    };

    // Read bundle quantity from metadata (set during checkout creation).
    // Defaults to 1 for backward compatibility with sessions created before
    // quantity support was added.
    let quantity: u64 = metadata
        .get("bundle_quantity")
        .and_then(|q| q.parse().ok())
        .unwrap_or(1)
        .max(1);

    match purchase_type.as_str() {
        "ai_bundle" => {
            // Credit AI bundle balance. Configurable via AI_BUNDLE_CREDIT_USD env var.
            let credit_per_unit: f64 = std::env::var("AI_BUNDLE_CREDIT_USD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10.0);
            let total_credit = credit_per_unit * quantity as f64;
            let result = kyomi_core::db_execute!(
                &state.db,
                "UPDATE workspaces SET ai_bundle_balance_usd = ai_bundle_balance_usd + $1 \
                 WHERE workspace_id = $2",
                total_credit,
                &workspace_id
            );
            match result {
                Ok(_) => {
                    tracing::info!(
                        workspace_id = %workspace_id,
                        quantity,
                        credit_per_unit,
                        total_credit,
                        "AI bundle purchased — credited balance"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        workspace_id = %workspace_id,
                        "Failed to credit AI bundle balance: {e}"
                    );
                }
            }
        }
        "analytics_bundle" => {
            // Credit analytics event bundle. Configurable via ANALYTICS_BUNDLE_CREDIT_EVENTS env var.
            let events_per_unit: i64 = std::env::var("ANALYTICS_BUNDLE_CREDIT_EVENTS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1_000_000);
            let total_events = events_per_unit * quantity as i64;
            let result = kyomi_core::db_execute!(
                &state.db,
                "UPDATE workspaces SET analytics_bundle_events = analytics_bundle_events + $1 \
                 WHERE workspace_id = $2",
                total_events,
                &workspace_id
            );
            match result {
                Ok(_) => {
                    tracing::info!(
                        workspace_id = %workspace_id,
                        quantity,
                        events_per_unit,
                        total_events,
                        "Analytics bundle purchased — credited events"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        workspace_id = %workspace_id,
                        "Failed to credit analytics bundle events: {e}"
                    );
                }
            }
        }
        other => {
            tracing::warn!(
                workspace_id = %workspace_id,
                purchase_type = %other,
                "Unknown purchase_type in checkout session metadata"
            );
        }
    }
}
