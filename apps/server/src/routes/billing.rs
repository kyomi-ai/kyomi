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

/// Load the Stripe ids the payment-recovery webhook backstop
/// (`handle_payment_recovery_checkout_completed`, KYO-806) needs to call
/// `recover_past_due_payment`.
async fn load_recovery_workspace_row(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
) -> Option<RecoveryWorkspaceRow> {
    kyomi_core::db_fetch_optional!(
        db, RecoveryWorkspaceRow,
        "SELECT stripe_customer_id, stripe_subscription_id FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )
    .ok()
    .flatten()
}

// ===========================================================================
// Internal row types
// ===========================================================================

/// Minimal workspace row used by the payment-recovery webhook backstop.
#[derive(Debug, sqlx::FromRow)]
struct RecoveryWorkspaceRow {
    stripe_customer_id: Option<String>,
    stripe_subscription_id: Option<String>,
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
///
/// Thin adapter (KYO-807): the guard, parse, write, and MCP-invalidation
/// logic all live in `kyomi_auth::billing_webhook::apply_subscription_webhook_event`,
/// which is unit-tested directly against a `Subscription` fixture without
/// needing `AppState`. This function's only job is to pick the write mode
/// and log the outcome.
async fn handle_subscription_event(
    state: &AppState,
    stripe_service: &StripeService,
    subscription: &Subscription,
    event_type: &str,
) {
    let write_mode = if event_type == "customer.subscription.created" {
        kyomi_auth::subscription_service::SubscriptionWriteMode::Created
    } else {
        kyomi_auth::subscription_service::SubscriptionWriteMode::Updated
    };

    let result = kyomi_auth::billing_webhook::apply_subscription_webhook_event(
        &state.db,
        &state.ws_manager,
        &state.mcp_sessions,
        stripe_service,
        subscription,
        write_mode,
    )
    .await;

    use kyomi_auth::billing_webhook::SubscriptionWebhookOutcome;
    match result {
        Ok(SubscriptionWebhookOutcome::NotKyomiApp) => {
            tracing::debug!(
                subscription_id = %subscription.id,
                app = ?subscription.metadata.get("app"),
                "Ignoring subscription event for non-Kyomi app"
            );
        }
        Ok(SubscriptionWebhookOutcome::MissingWorkspaceId) => {
            tracing::error!("Subscription missing workspace_id in metadata");
        }
        Ok(SubscriptionWebhookOutcome::Applied {
            workspace_id,
            tier,
            changed,
        }) => {
            tracing::info!(
                workspace_id = %workspace_id,
                tier = %tier,
                changed,
                "{event_type} processed"
            );
        }
        Err(e) => {
            tracing::error!("Failed to update workspace from {event_type}: {e}");
        }
    }
}

/// Handle `customer.subscription.deleted` — revert workspace to free tier.
///
/// Thin adapter (KYO-807) over
/// `kyomi_auth::billing_webhook::apply_subscription_deleted_webhook_event`.
async fn handle_subscription_deleted(state: &AppState, subscription: &Subscription) {
    use kyomi_auth::billing_webhook::SubscriptionDeletedOutcome;
    let result = kyomi_auth::billing_webhook::apply_subscription_deleted_webhook_event(
        &state.db,
        &state.ws_manager,
        &state.mcp_sessions,
        subscription,
    )
    .await;

    match result {
        Ok(SubscriptionDeletedOutcome::NotKyomiApp) => {
            tracing::debug!(
                subscription_id = %subscription.id,
                app = ?subscription.metadata.get("app"),
                "Ignoring subscription deletion for non-Kyomi app"
            );
        }
        Ok(SubscriptionDeletedOutcome::MissingWorkspaceId) => {
            tracing::error!("Subscription missing workspace_id in metadata");
        }
        Ok(SubscriptionDeletedOutcome::Applied {
            workspace_id,
            changed,
        }) => {
            tracing::info!(
                workspace_id = %workspace_id,
                changed,
                "Subscription deleted — reverted to free tier"
            );
        }
        Err(e) => {
            tracing::error!("Failed to revert workspace to free tier: {e}");
        }
    }
}

/// Handle `invoice.payment_succeeded` — reset AI credits for new billing period.
///
/// Thin adapter (KYO-807) over
/// `kyomi_auth::billing_webhook::apply_invoice_payment_succeeded_webhook_event`.
async fn handle_invoice_payment_succeeded(state: &AppState, invoice: &Invoice) {
    use kyomi_auth::billing_webhook::InvoiceWebhookOutcome;
    let result = kyomi_auth::billing_webhook::apply_invoice_payment_succeeded_webhook_event(
        &state.db,
        &state.ws_manager,
        invoice,
    )
    .await;

    match result {
        Ok(InvoiceWebhookOutcome::NoSubscriptionId) => {
            tracing::debug!("invoice.payment_succeeded: no subscription id on invoice");
        }
        Ok(InvoiceWebhookOutcome::WorkspaceNotFound { subscription_id }) => {
            tracing::warn!(
                subscription_id = %subscription_id,
                "No workspace found for subscription in payment_succeeded event"
            );
        }
        Ok(InvoiceWebhookOutcome::Applied {
            workspace_id,
            changed,
        }) => {
            tracing::info!(
                workspace_id = %workspace_id,
                changed,
                "Payment succeeded — reset AI credits"
            );
        }
        Err(e) => {
            tracing::error!("Failed to reset AI credits: {e}");
        }
    }
}

/// Handle `invoice.payment_failed` — mark subscription as past_due.
///
/// Thin adapter (KYO-807) over
/// `kyomi_auth::billing_webhook::apply_invoice_payment_failed_webhook_event`.
async fn handle_invoice_payment_failed(state: &AppState, invoice: &Invoice) {
    use kyomi_auth::billing_webhook::InvoiceWebhookOutcome;
    let result = kyomi_auth::billing_webhook::apply_invoice_payment_failed_webhook_event(
        &state.db,
        &state.ws_manager,
        invoice,
    )
    .await;

    match result {
        Ok(InvoiceWebhookOutcome::NoSubscriptionId) => {
            tracing::debug!("invoice.payment_failed: no subscription id on invoice");
        }
        Ok(InvoiceWebhookOutcome::WorkspaceNotFound { subscription_id }) => {
            tracing::warn!(
                subscription_id = %subscription_id,
                "No workspace found for subscription in payment_failed event"
            );
        }
        Ok(InvoiceWebhookOutcome::Applied {
            workspace_id,
            changed,
        }) => {
            tracing::warn!(
                workspace_id = %workspace_id,
                changed,
                "Payment failed — marked subscription as past_due"
            );
        }
        Err(e) => {
            tracing::error!("Failed to update subscription status to past_due: {e}");
        }
    }
}

/// Handle `checkout.session.completed`.
///
/// Dispatches on [`kyomi_auth::payment_recovery::classify_webhook_checkout_session`]:
/// - `Bundle` (`mode = payment`) — fulfil a one-time bundle purchase (AI
///   credits, analytics events). Unchanged by KYO-806.
/// - `PaymentRecovery` (`mode = setup` carrying the recovery purpose
///   marker) — the backstop for [`kyomi_auth::payment_recovery::recover_past_due_payment`]
///   (KYO-806 A3), covering an owner who closes the tab before the
///   client-side completion call fires.
/// - `Ignore` — most commonly `mode = subscription`, handled entirely by
///   the `customer.subscription.created`/`.updated` events instead.
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

    match kyomi_auth::payment_recovery::classify_webhook_checkout_session(
        &session.mode,
        Some(metadata),
    ) {
        kyomi_auth::payment_recovery::CheckoutSessionKind::Bundle => {
            handle_bundle_checkout_completed(state, &workspace_id, metadata).await;
        }
        kyomi_auth::payment_recovery::CheckoutSessionKind::PaymentRecovery => {
            handle_payment_recovery_checkout_completed(state, &workspace_id, session).await;
        }
        kyomi_auth::payment_recovery::CheckoutSessionKind::Ignore => {
            tracing::info!(
                workspace_id = %workspace_id,
                mode = ?session.mode,
                "Checkout session completed — not a bundle or payment-recovery session"
            );
        }
    }
}

/// The payment-recovery webhook backstop (KYO-806 A3 step 4) — runs the same
/// shared service the `complete_payment_recovery` server fn does, so an
/// owner who closes the tab before that call fires still gets their
/// subscription recovered once Stripe's webhook arrives.
async fn handle_payment_recovery_checkout_completed(
    state: &AppState,
    workspace_id: &str,
    session: &stripe_shared::CheckoutSession,
) {
    let stripe_service = match require_stripe(state) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(
                workspace_id,
                "Payment recovery webhook backstop: Stripe not configured: {e}"
            );
            return;
        }
    };

    let row = match load_recovery_workspace_row(&state.db, workspace_id).await {
        Some(row) => row,
        None => {
            tracing::error!(
                workspace_id,
                "Payment recovery webhook backstop: workspace not found"
            );
            return;
        }
    };

    let (Some(customer_id), Some(subscription_id)) =
        (row.stripe_customer_id, row.stripe_subscription_id)
    else {
        tracing::error!(
            workspace_id,
            "Payment recovery webhook backstop: workspace is missing stripe_customer_id or \
             stripe_subscription_id — cannot recover"
        );
        return;
    };

    match kyomi_auth::payment_recovery::recover_past_due_payment(
        &state.db,
        stripe_service,
        &state.ws_manager,
        &state.mcp_sessions,
        kyomi_auth::payment_recovery::RecoveryIds {
            workspace_id,
            stripe_customer_id: &customer_id,
            stripe_subscription_id: &subscription_id,
            session_id: session.id.as_ref(),
        },
    )
    .await
    {
        Ok(outcome) => {
            tracing::info!(
                workspace_id,
                ?outcome,
                "Payment recovery webhook backstop processed"
            );
        }
        Err(e) => {
            tracing::error!(workspace_id, "Payment recovery webhook backstop failed: {e}");
        }
    }
}

/// Fulfil a one-time bundle purchase (`mode = payment`) — AI credits or
/// analytics events. Existing behaviour, unchanged by KYO-806; only
/// extracted into its own function so [`handle_checkout_completed`] could
/// add the payment-recovery branch alongside it.
async fn handle_bundle_checkout_completed(
    state: &AppState,
    workspace_id: &str,
    metadata: &std::collections::HashMap<String, String>,
) {
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
                workspace_id
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
                workspace_id
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
