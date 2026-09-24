// SPDX-License-Identifier: AGPL-3.0-or-later

//! Past-due payment recovery (KYO-806 A3) — the shared service both
//! `start_payment_recovery` / `complete_payment_recovery`
//! (`crates/kyomi-ui/src/server_fns/billing.rs`) and the Stripe webhook's
//! `checkout.session.completed` backstop
//! (`apps/server/src/routes/billing.rs`) call.
//!
//! ## Why this exists
//!
//! Adding a card via the Stripe Customer Portal does **not** pay a
//! `past_due` subscription's open invoice — Stripe waits for its own retry
//! schedule (which can be days). A lapsed workspace owner who just typed in
//! a working card expects to be unblocked immediately, not "eventually".
//!
//! The flow: create an embedded Setup-mode Checkout Session for the
//! *existing* Stripe customer (never a new subscription — see
//! `crates/kyomi-ui/src/server_fns/billing.rs`'s `create_checkout` guard,
//! KYO-806 A6, which refuses `past_due` before it can ever reach the
//! new-subscription path). Once the customer completes it, apply the
//! resulting payment method to the customer and the existing subscription,
//! then pay every open invoice on that subscription directly — synchronously,
//! not "wait for Stripe's automatic retry".
//!
//! ## Idempotency
//!
//! The server-fn completion call (`complete_payment_recovery`) and the
//! webhook backstop (for an owner who closes the tab before the client-side
//! call fires) can both run for the same session. Every write here —
//! setting the default payment method, paying each invoice, writing the
//! refreshed subscription state — is safe to repeat: setting the same
//! default payment method twice is a no-op, and [`recover_past_due_payment`]
//! treats an invoice that turns out to already be `paid` (by the other
//! caller winning the race) as success rather than an error.
//!
//! ## Testability
//!
//! The real Stripe types this module reads (`stripe_shared::CheckoutSession`,
//! `Invoice`, `ApiErrors`) have dozens of required fields apiece and no
//! `Default` impl, so constructing one in a unit test is impractical. Every
//! decision function here therefore takes a small, hand-built "facts" struct
//! extracted from the real type by a thin, deliberately-low-branching
//! adapter (`extract_recovery_session_facts`, `extract_pay_invoice_error_facts`,
//! `invoice_facts`) — the adapters do field extraction only, all actual
//! decisions live in the pure, directly-testable functions.

use std::collections::HashMap;

use kyomi_core::{DbPool, Error};
use stripe::StripeError;
use stripe_shared::{
    CheckoutSession, CheckoutSessionMode, CheckoutSessionStatus, Invoice, InvoiceStatus,
    PaymentIntentStatus, SetupIntentStatus,
};
use stripe_types::Expandable;

use crate::{
    mcp_session_manager::MCPSessionManager,
    stripe_service::StripeService,
    subscription_service::{write_subscription_to_workspace, SubscriptionWriteMode},
};

/// The metadata marker (`metadata["purpose"]`) that distinguishes a
/// payment-recovery Setup Checkout Session from any other Setup-mode
/// session Stripe might send a `checkout.session.completed` event for.
/// Set on session creation ([`StripeService::create_recovery_setup_session`])
/// and checked by both the server-fn completion path and the webhook
/// backstop.
pub const RECOVERY_PURPOSE_MARKER: &str = "recover_past_due";

/// The outcome of a payment-recovery attempt — mirrors the ticket's
/// `PaymentRecoveryOutcome` wire DTO (`crates/kyomi-ui/src/server_fns/billing.rs`)
/// but lives here, ssr-only, as the internal service-layer result; the
/// server fn maps it 1:1 into the wire type the WASM client matches on.
#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryOutcome {
    /// Every open invoice on the subscription is now paid; the workspace's
    /// subscription state has been refreshed to `active` and MCP sessions
    /// invalidated.
    Recovered,
    /// The payment attempt needs further customer action (e.g. 3-D Secure)
    /// before it can succeed. `hosted_invoice_url` is Stripe's own page for
    /// completing it, when available.
    NeedsAction { hosted_invoice_url: Option<String> },
    /// The payment attempt failed outright (e.g. the card was declined).
    Declined { message: String },
}

// ─── Checkout session validation (server-fn completion path) ──────────────

/// The subset of a Setup-mode `CheckoutSession`'s fields payment-recovery
/// validation needs — see the module doc's *Testability* note for why this
/// exists instead of validating the real `stripe_shared::CheckoutSession`
/// directly.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoverySessionFacts {
    pub mode: CheckoutSessionMode,
    pub status: Option<CheckoutSessionStatus>,
    pub metadata_purpose: Option<String>,
    pub metadata_workspace_id: Option<String>,
    pub customer_id: Option<String>,
    pub setup_intent_status: Option<SetupIntentStatus>,
    pub payment_method_id: Option<String>,
}

/// Extract [`RecoverySessionFacts`] from a real, `setup_intent`-expanded
/// `CheckoutSession` (see [`StripeService::retrieve_checkout_session_with_setup_intent`]).
/// Field extraction only — no validation decisions are made here; those all
/// live in [`validate_recovery_session_facts`].
pub fn extract_recovery_session_facts(session: &CheckoutSession) -> RecoverySessionFacts {
    let metadata = session.metadata.as_ref();

    let customer_id = session.customer.as_ref().map(|c| match c {
        Expandable::Id(id) => id.to_string(),
        Expandable::Object(obj) => obj.id.to_string(),
    });

    let (setup_intent_status, payment_method_id) = match &session.setup_intent {
        Some(Expandable::Object(si)) => {
            let pm_id = si.payment_method.as_ref().map(|pm| match pm {
                Expandable::Id(id) => id.to_string(),
                Expandable::Object(obj) => obj.id.to_string(),
            });
            (Some(si.status.clone()), pm_id)
        }
        // Not expanded, or no setup_intent at all — validation rejects
        // either case identically (SetupIntentMissing).
        Some(Expandable::Id(_)) | None => (None, None),
    };

    RecoverySessionFacts {
        mode: session.mode.clone(),
        status: session.status.clone(),
        metadata_purpose: metadata.and_then(|m| m.get("purpose").cloned()),
        metadata_workspace_id: metadata.and_then(|m| m.get("workspace_id").cloned()),
        customer_id,
        setup_intent_status,
        payment_method_id,
    }
}

/// Why a checkout session was rejected as a valid, completed payment-recovery
/// session. Every variant is a reason to refuse applying the session, never
/// a partial success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryValidationError {
    NotSetupMode,
    NotComplete,
    PurposeMismatch,
    /// The session's `metadata["workspace_id"]` doesn't match the caller's
    /// own workspace — the security check the ticket calls out explicitly:
    /// a caller must not be able to apply someone else's session.
    WorkspaceMismatch,
    /// The session's Stripe customer doesn't match the workspace's
    /// `stripe_customer_id` — the second half of the same security check.
    CustomerMismatch,
    SetupIntentMissing,
    SetupIntentNotSucceeded(String),
    PaymentMethodMissing,
}

impl std::fmt::Display for RecoveryValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSetupMode => write!(f, "checkout session is not a setup-mode session"),
            Self::NotComplete => write!(f, "checkout session is not complete"),
            Self::PurposeMismatch => {
                write!(f, "checkout session is not a payment-recovery session")
            }
            Self::WorkspaceMismatch => {
                write!(f, "checkout session does not belong to this workspace")
            }
            Self::CustomerMismatch => write!(
                f,
                "checkout session's customer does not match this workspace's billing customer"
            ),
            Self::SetupIntentMissing => write!(f, "checkout session has no completed setup intent"),
            Self::SetupIntentNotSucceeded(status) => {
                write!(f, "setup intent has not succeeded (status: {status})")
            }
            Self::PaymentMethodMissing => write!(f, "setup intent captured no payment method"),
        }
    }
}

/// Validate that `facts` describes a genuinely completed payment-recovery
/// session belonging to `expected_workspace_id` / `expected_customer_id`,
/// returning the payment method id it collected on success.
///
/// Pure and exhaustively unit-tested — every rejection reason is checked in
/// isolation. Order matters only for which single reason is reported when
/// several are true at once; every check still runs against real input
/// regardless of order.
pub fn validate_recovery_session_facts(
    facts: &RecoverySessionFacts,
    expected_workspace_id: &str,
    expected_customer_id: &str,
) -> Result<String, RecoveryValidationError> {
    if facts.mode != CheckoutSessionMode::Setup {
        return Err(RecoveryValidationError::NotSetupMode);
    }
    if facts.status != Some(CheckoutSessionStatus::Complete) {
        return Err(RecoveryValidationError::NotComplete);
    }
    if facts.metadata_purpose.as_deref() != Some(RECOVERY_PURPOSE_MARKER) {
        return Err(RecoveryValidationError::PurposeMismatch);
    }
    if facts.metadata_workspace_id.as_deref() != Some(expected_workspace_id) {
        return Err(RecoveryValidationError::WorkspaceMismatch);
    }
    if facts.customer_id.as_deref() != Some(expected_customer_id) {
        return Err(RecoveryValidationError::CustomerMismatch);
    }
    match &facts.setup_intent_status {
        Some(SetupIntentStatus::Succeeded) => {}
        Some(other) => {
            return Err(RecoveryValidationError::SetupIntentNotSucceeded(
                other.as_str().to_string(),
            ))
        }
        None => return Err(RecoveryValidationError::SetupIntentMissing),
    }

    facts
        .payment_method_id
        .clone()
        .ok_or(RecoveryValidationError::PaymentMethodMissing)
}

// ─── Invoice payment outcome classification ────────────────────────────────

/// The subset of an `Invoice`'s fields outcome classification needs.
#[derive(Debug, Clone, PartialEq)]
pub struct InvoiceFacts {
    pub status: Option<InvoiceStatus>,
    pub hosted_invoice_url: Option<String>,
}

fn invoice_facts(invoice: &Invoice) -> InvoiceFacts {
    InvoiceFacts {
        status: invoice.status.clone(),
        hosted_invoice_url: invoice.hosted_invoice_url.clone(),
    }
}

/// Classify an invoice's state after a **successful** `PayInvoice` call
/// (i.e. Stripe didn't return an error). `Paid` is the only success case;
/// anything else — most plausibly `Open` if Stripe accepted the request but
/// the underlying charge is still settling — is reported conservatively
/// rather than assumed to have worked.
pub fn classify_invoice_facts(facts: &InvoiceFacts) -> RecoveryOutcome {
    match &facts.status {
        Some(InvoiceStatus::Paid) => RecoveryOutcome::Recovered,
        Some(InvoiceStatus::Open) => RecoveryOutcome::NeedsAction {
            hosted_invoice_url: facts.hosted_invoice_url.clone(),
        },
        _ => RecoveryOutcome::Declined {
            message: "invoice could not be confirmed as paid".to_string(),
        },
    }
}

/// The subset of a failed `PayInvoice` call's error we need to distinguish
/// "requires further customer action" (e.g. 3-D Secure) from an outright
/// decline. Stripe embeds the `PaymentIntent` directly in the error body for
/// exactly this reason (`ApiErrors::payment_intent`).
#[derive(Debug, Clone, PartialEq)]
pub struct PayInvoiceErrorFacts {
    pub message: Option<String>,
    pub payment_intent_status: Option<PaymentIntentStatus>,
}

/// Extract [`PayInvoiceErrorFacts`] from a `PayInvoice` error. Returns `None`
/// for any [`StripeError`] variant that isn't an API-level rejection (e.g.
/// `Timeout`, `ClientError`) — those are transport/infra failures, not a
/// business outcome, and the caller propagates them as a hard error instead
/// of guessing at Declined/NeedsAction.
pub fn extract_pay_invoice_error_facts(error: &StripeError) -> Option<PayInvoiceErrorFacts> {
    match error {
        StripeError::Stripe(api_errors, _status) => Some(PayInvoiceErrorFacts {
            message: api_errors.message.clone(),
            payment_intent_status: api_errors.payment_intent.as_ref().map(|pi| pi.status.clone()),
        }),
        StripeError::Timeout | StripeError::ClientError(_) | StripeError::JSONDeserialize(_)
        | StripeError::ConfigError(_) => None,
    }
}

/// Classify a `PayInvoice` error into [`RecoveryOutcome::NeedsAction`] or
/// [`RecoveryOutcome::Declined`]. A payment intent left in
/// `requires_action` or `requires_confirmation` means the charge itself
/// wasn't refused — the customer needs to complete an extra step (most
/// commonly 3-D Secure) — which is a fundamentally different outcome from a
/// card decline and must not be reported as one.
pub fn classify_pay_invoice_error_facts(
    facts: &PayInvoiceErrorFacts,
    hosted_invoice_url: Option<String>,
) -> RecoveryOutcome {
    let requires_action = matches!(
        facts.payment_intent_status,
        Some(PaymentIntentStatus::RequiresAction) | Some(PaymentIntentStatus::RequiresConfirmation)
    );

    if requires_action {
        RecoveryOutcome::NeedsAction { hosted_invoice_url }
    } else {
        RecoveryOutcome::Declined {
            message: facts
                .message
                .clone()
                .unwrap_or_else(|| "Payment failed".to_string()),
        }
    }
}

// ─── Webhook checkout-session classification ───────────────────────────────

/// What kind of completed Checkout Session `handle_checkout_completed`
/// (`apps/server/src/routes/billing.rs`) is looking at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutSessionKind {
    /// `mode = payment` — a one-time bundle purchase (AI credits, analytics
    /// events). Existing handling, unchanged by KYO-806.
    Bundle,
    /// `mode = setup` carrying [`RECOVERY_PURPOSE_MARKER`] — the webhook
    /// backstop for [`recover_past_due_payment`].
    PaymentRecovery,
    /// Anything else: `mode = subscription` (handled entirely by the
    /// `customer.subscription.created`/`.updated` events instead), a
    /// `setup`-mode session with no recognized purpose, or an unknown mode.
    Ignore,
}

/// Classify a checkout session for webhook dispatch. Pure — takes only the
/// two fields the decision needs, both cheap to construct directly in a
/// test (unlike the full `CheckoutSession`; see the module doc).
pub fn classify_webhook_checkout_session(
    mode: &CheckoutSessionMode,
    metadata: Option<&HashMap<String, String>>,
) -> CheckoutSessionKind {
    match mode {
        CheckoutSessionMode::Payment => CheckoutSessionKind::Bundle,
        CheckoutSessionMode::Setup => {
            let purpose = metadata.and_then(|m| m.get("purpose")).map(String::as_str);
            if purpose == Some(RECOVERY_PURPOSE_MARKER) {
                CheckoutSessionKind::PaymentRecovery
            } else {
                CheckoutSessionKind::Ignore
            }
        }
        CheckoutSessionMode::Subscription | CheckoutSessionMode::Unknown(_) => {
            CheckoutSessionKind::Ignore
        }
        _ => CheckoutSessionKind::Ignore,
    }
}

// ─── New-subscription checkout sync (KYO-806 A4) ───────────────────────────

/// The subset of a Subscription-mode `CheckoutSession`'s fields
/// [`sync_new_subscription_checkout`] needs to validate ownership before
/// trusting it. See the module doc's *Testability* note for why this thin
/// facts struct exists instead of validating the real type directly.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSubscriptionSessionFacts {
    pub mode: CheckoutSessionMode,
    pub metadata_workspace_id: Option<String>,
    pub customer_id: Option<String>,
    pub has_subscription: bool,
}

/// Extract [`NewSubscriptionSessionFacts`] from a real,
/// `subscription`-expanded `CheckoutSession` (see
/// [`StripeService::retrieve_checkout_session_with_subscription`]). Field
/// extraction only.
pub fn extract_new_subscription_session_facts(
    session: &CheckoutSession,
) -> NewSubscriptionSessionFacts {
    let customer_id = session.customer.as_ref().map(|c| match c {
        Expandable::Id(id) => id.to_string(),
        Expandable::Object(obj) => obj.id.to_string(),
    });

    NewSubscriptionSessionFacts {
        mode: session.mode.clone(),
        metadata_workspace_id: session
            .metadata
            .as_ref()
            .and_then(|m| m.get("workspace_id").cloned()),
        customer_id,
        has_subscription: session.subscription.is_some(),
    }
}

/// Why a checkout session was rejected as a valid, completed new-subscription
/// session to sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewSubscriptionValidationError {
    NotSubscriptionMode,
    /// Same security check as [`RecoveryValidationError::WorkspaceMismatch`]:
    /// a caller must not be able to sync someone else's session.
    WorkspaceMismatch,
    CustomerMismatch,
    NoSubscription,
}

impl std::fmt::Display for NewSubscriptionValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSubscriptionMode => {
                write!(f, "checkout session is not a subscription-mode session")
            }
            Self::WorkspaceMismatch => {
                write!(f, "checkout session does not belong to this workspace")
            }
            Self::CustomerMismatch => write!(
                f,
                "checkout session's customer does not match this workspace's billing customer"
            ),
            Self::NoSubscription => write!(f, "checkout session has no subscription"),
        }
    }
}

/// Validate that `facts` describes a genuinely completed subscription
/// checkout session belonging to `expected_workspace_id` /
/// `expected_customer_id` — exactly the same ownership check
/// [`validate_recovery_session_facts`] performs for the payment-recovery
/// session, applied here to the new-subscription session instead.
pub fn validate_new_subscription_session_facts(
    facts: &NewSubscriptionSessionFacts,
    expected_workspace_id: &str,
    expected_customer_id: &str,
) -> Result<(), NewSubscriptionValidationError> {
    if facts.mode != CheckoutSessionMode::Subscription {
        return Err(NewSubscriptionValidationError::NotSubscriptionMode);
    }
    if facts.metadata_workspace_id.as_deref() != Some(expected_workspace_id) {
        return Err(NewSubscriptionValidationError::WorkspaceMismatch);
    }
    if facts.customer_id.as_deref() != Some(expected_customer_id) {
        return Err(NewSubscriptionValidationError::CustomerMismatch);
    }
    if !facts.has_subscription {
        return Err(NewSubscriptionValidationError::NoSubscription);
    }
    Ok(())
}

/// Sync a completed new-subscription Checkout Session's resulting
/// subscription into the workspace row immediately.
///
/// Without this, the DB only learns of a new subscription from the
/// `customer.subscription.created` webhook, so a client refetch right
/// after the embedded checkout's `onComplete` would still see the
/// workspace as lapsed. Writes through the same shared writer
/// (KYO-806 A5) the webhook uses, in [`SubscriptionWriteMode::Created`]
/// mode, so this call and that webhook are idempotent with each other —
/// whichever runs second just writes the same state again.
///
/// Called by `sync_checkout_subscription`
/// (`crates/kyomi-ui/src/server_fns/billing.rs`) for the `Subscribe` paywall
/// action (cancelled-with-no-subscription, or an expired no-Stripe trial).
pub async fn sync_new_subscription_checkout(
    db: &DbPool,
    stripe: &StripeService,
    mcp_sessions: &MCPSessionManager,
    workspace_id: &str,
    stripe_customer_id: &str,
    session_id: &str,
) -> Result<(), Error> {
    let session = stripe
        .retrieve_checkout_session_with_subscription(session_id)
        .await
        .map_err(|e| Error::Internal(format!("failed to retrieve checkout session: {e}")))?;

    let facts = extract_new_subscription_session_facts(&session);
    validate_new_subscription_session_facts(&facts, workspace_id, stripe_customer_id)
        .map_err(|e| Error::BadRequest(format!("invalid new-subscription checkout session: {e}")))?;

    let subscription = match &session.subscription {
        Some(Expandable::Object(sub)) => sub,
        _ => {
            return Err(Error::Internal(
                "checkout session has no expanded subscription".to_string(),
            ))
        }
    };

    let sub_data = stripe.parse_subscription_data(subscription).await.map_err(|e| {
        Error::Internal(format!("failed to parse subscription data: {e}"))
    })?;

    write_subscription_to_workspace(db, workspace_id, &sub_data, SubscriptionWriteMode::Created)
        .await?;

    mcp_sessions.notify_tools_changed(workspace_id).await;
    mcp_sessions.invalidate_workspace_sessions(workspace_id).await;

    tracing::info!(workspace_id, "New-subscription checkout synced ahead of webhook");

    Ok(())
}

// ─── Orchestration ──────────────────────────────────────────────────────────

/// Attempt to pay a single open invoice, classifying the result.
///
/// Handles the recovery-specific idempotency case: `PayInvoice` erroring
/// doesn't necessarily mean the invoice is unpaid — the webhook backstop and
/// the server-fn completion call can race, and Stripe itself may reject a
/// second payment attempt on an invoice the *other* caller just paid. Before
/// trusting the error, re-fetch the invoice's actual current state.
///
/// Returns `Err` — not a `Declined` [`RecoveryOutcome`] — when the failure
/// couldn't even be classified as a Stripe-level decision (see
/// [`extract_pay_invoice_error_facts`]'s doc comment): a timeout or
/// malformed response is not "the card was declined", and telling the owner
/// that would be actively misleading.
async fn attempt_pay_invoice(
    stripe: &StripeService,
    invoice: &Invoice,
    payment_method_id: &str,
) -> Result<RecoveryOutcome, Error> {
    let invoice_id = invoice
        .id
        .as_ref()
        .ok_or_else(|| Error::Internal("open invoice has no id".to_string()))?
        .to_string();
    let hosted_invoice_url = invoice.hosted_invoice_url.clone();

    match stripe.pay_invoice(&invoice_id, payment_method_id).await {
        Ok(paid) => Ok(classify_invoice_facts(&invoice_facts(&paid))),
        Err(stripe_err) => match stripe.retrieve_invoice(&invoice_id).await {
            Ok(current) if current.status == Some(InvoiceStatus::Paid) => {
                tracing::info!(
                    invoice_id,
                    "Invoice already paid by a concurrent recovery attempt — treating as success"
                );
                Ok(RecoveryOutcome::Recovered)
            }
            _ => match extract_pay_invoice_error_facts(&stripe_err) {
                Some(facts) => Ok(classify_pay_invoice_error_facts(&facts, hosted_invoice_url)),
                // `extract_pay_invoice_error_facts` returns `None` for
                // `Timeout`/`ClientError`/`JSONDeserialize`/`ConfigError` —
                // transport/infra failures, not a decision Stripe made
                // about the card. Reporting these as `Declined` would tell
                // the workspace owner their card was refused when Stripe
                // was never actually reached (or its response couldn't be
                // parsed) — propagate as a hard error instead, matching
                // that function's own doc comment ("the caller propagates
                // them as a hard error instead of guessing at
                // Declined/NeedsAction").
                None => Err(Error::Internal(format!(
                    "Payment recovery could not confirm the outcome of paying invoice \
                     {invoice_id}: {stripe_err}"
                ))),
            },
        },
    }
}

/// Identifiers [`recover_past_due_payment`] needs, grouped into one struct
/// so the function takes four service/pool parameters (`db`, `stripe`,
/// `mcp_sessions`, plus this) instead of seven independent ones — these four
/// strings are "the ids this one recovery call is about", never varied
/// independently of each other, so a struct documents that relationship
/// instead of clippy's `too_many_arguments` merely being suppressed.
#[derive(Debug, Clone, Copy)]
pub struct RecoveryIds<'a> {
    pub workspace_id: &'a str,
    pub stripe_customer_id: &'a str,
    pub stripe_subscription_id: &'a str,
    /// The completed Setup Checkout Session id — must have been created by
    /// [`StripeService::create_recovery_setup_session`] for `workspace_id` /
    /// `stripe_customer_id`; any other session is rejected by
    /// [`validate_recovery_session_facts`] before any Stripe mutation runs.
    pub session_id: &'a str,
}

/// Recover a `past_due` subscription: apply the payment method a completed
/// Setup Checkout Session collected, pay every open invoice on the
/// subscription, and — only once every invoice is confirmed paid — refresh
/// the workspace's subscription state and invalidate MCP sessions.
///
/// Called by both `complete_payment_recovery`
/// (`crates/kyomi-ui/src/server_fns/billing.rs`) and the webhook backstop
/// (`apps/server/src/routes/billing.rs`); see the module doc for the
/// idempotency guarantees that make calling this twice for the same session
/// safe.
pub async fn recover_past_due_payment(
    db: &DbPool,
    stripe: &StripeService,
    mcp_sessions: &MCPSessionManager,
    ids: RecoveryIds<'_>,
) -> Result<RecoveryOutcome, Error> {
    let RecoveryIds {
        workspace_id,
        stripe_customer_id,
        stripe_subscription_id,
        session_id,
    } = ids;

    let session = stripe
        .retrieve_checkout_session_with_setup_intent(session_id)
        .await
        .map_err(|e| Error::Internal(format!("failed to retrieve checkout session: {e}")))?;

    let facts = extract_recovery_session_facts(&session);
    let payment_method_id =
        validate_recovery_session_facts(&facts, workspace_id, stripe_customer_id)
            .map_err(|e| Error::BadRequest(format!("invalid payment recovery session: {e}")))?;

    stripe
        .set_default_payment_method(stripe_customer_id, stripe_subscription_id, &payment_method_id)
        .await
        .map_err(|e| Error::Internal(format!("failed to set default payment method: {e}")))?;

    let open_invoices = stripe
        .list_open_invoices(stripe_subscription_id)
        .await
        .map_err(|e| Error::Internal(format!("failed to list open invoices: {e}")))?;

    for invoice in &open_invoices {
        let outcome = attempt_pay_invoice(stripe, invoice, &payment_method_id).await?;
        if !matches!(outcome, RecoveryOutcome::Recovered) {
            // Stop at the first invoice that didn't succeed — surface it
            // rather than attempting the remaining invoices with a payment
            // method that (as of this invoice) just failed or needs action.
            tracing::warn!(
                workspace_id,
                subscription_id = stripe_subscription_id,
                "Payment recovery did not fully succeed: {outcome:?}"
            );
            return Ok(outcome);
        }
    }

    // Every open invoice is paid (or there were none). Refresh the
    // subscription state from Stripe — the source of truth — and write it
    // through the one shared writer (KYO-806 A5) so an immediate client
    // refetch sees `active` without waiting for
    // `customer.subscription.updated`.
    let subscription = stripe
        .retrieve_subscription(stripe_subscription_id)
        .await
        .map_err(|e| {
            Error::Internal(format!("failed to retrieve subscription after recovery: {e}"))
        })?;
    let sub_data = stripe.parse_subscription_data(&subscription).await.map_err(|e| {
        Error::Internal(format!("failed to parse subscription data after recovery: {e}"))
    })?;

    write_subscription_to_workspace(db, workspace_id, &sub_data, SubscriptionWriteMode::Updated)
        .await?;

    mcp_sessions.notify_tools_changed(workspace_id).await;
    mcp_sessions.invalidate_workspace_sessions(workspace_id).await;

    tracing::info!(
        workspace_id,
        subscription_id = stripe_subscription_id,
        "Past-due payment recovered — subscription reactivated"
    );

    Ok(RecoveryOutcome::Recovered)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // -- validate_recovery_session_facts --------------------------------

    fn valid_facts() -> RecoverySessionFacts {
        RecoverySessionFacts {
            mode: CheckoutSessionMode::Setup,
            status: Some(CheckoutSessionStatus::Complete),
            metadata_purpose: Some(RECOVERY_PURPOSE_MARKER.to_string()),
            metadata_workspace_id: Some("ws-1".to_string()),
            customer_id: Some("cus_1".to_string()),
            setup_intent_status: Some(SetupIntentStatus::Succeeded),
            payment_method_id: Some("pm_1".to_string()),
        }
    }

    #[test]
    fn valid_session_yields_payment_method_id() {
        let facts = valid_facts();
        assert_eq!(
            validate_recovery_session_facts(&facts, "ws-1", "cus_1"),
            Ok("pm_1".to_string())
        );
    }

    #[test]
    fn rejects_non_setup_mode() {
        let mut facts = valid_facts();
        facts.mode = CheckoutSessionMode::Subscription;
        assert_eq!(
            validate_recovery_session_facts(&facts, "ws-1", "cus_1"),
            Err(RecoveryValidationError::NotSetupMode)
        );
    }

    #[test]
    fn rejects_incomplete_session() {
        let mut facts = valid_facts();
        facts.status = Some(CheckoutSessionStatus::Open);
        assert_eq!(
            validate_recovery_session_facts(&facts, "ws-1", "cus_1"),
            Err(RecoveryValidationError::NotComplete)
        );
    }

    #[test]
    fn rejects_missing_purpose_marker() {
        let mut facts = valid_facts();
        facts.metadata_purpose = None;
        assert_eq!(
            validate_recovery_session_facts(&facts, "ws-1", "cus_1"),
            Err(RecoveryValidationError::PurposeMismatch)
        );
    }

    #[test]
    fn rejects_workspace_mismatch() {
        // The security check the ticket calls out explicitly: a caller must
        // not be able to apply a session belonging to another workspace.
        let facts = valid_facts();
        assert_eq!(
            validate_recovery_session_facts(&facts, "ws-other", "cus_1"),
            Err(RecoveryValidationError::WorkspaceMismatch)
        );
    }

    #[test]
    fn rejects_customer_mismatch() {
        let facts = valid_facts();
        assert_eq!(
            validate_recovery_session_facts(&facts, "ws-1", "cus_other"),
            Err(RecoveryValidationError::CustomerMismatch)
        );
    }

    #[test]
    fn rejects_missing_setup_intent() {
        let mut facts = valid_facts();
        facts.setup_intent_status = None;
        assert_eq!(
            validate_recovery_session_facts(&facts, "ws-1", "cus_1"),
            Err(RecoveryValidationError::SetupIntentMissing)
        );
    }

    #[test]
    fn rejects_unsucceeded_setup_intent() {
        let mut facts = valid_facts();
        facts.setup_intent_status = Some(SetupIntentStatus::RequiresAction);
        assert_eq!(
            validate_recovery_session_facts(&facts, "ws-1", "cus_1"),
            Err(RecoveryValidationError::SetupIntentNotSucceeded(
                "requires_action".to_string()
            ))
        );
    }

    #[test]
    fn rejects_missing_payment_method() {
        let mut facts = valid_facts();
        facts.payment_method_id = None;
        assert_eq!(
            validate_recovery_session_facts(&facts, "ws-1", "cus_1"),
            Err(RecoveryValidationError::PaymentMethodMissing)
        );
    }

    // -- classify_invoice_facts ------------------------------------------

    #[test]
    fn paid_invoice_is_recovered() {
        let facts = InvoiceFacts {
            status: Some(InvoiceStatus::Paid),
            hosted_invoice_url: None,
        };
        assert_eq!(classify_invoice_facts(&facts), RecoveryOutcome::Recovered);
    }

    #[test]
    fn still_open_invoice_needs_action() {
        let facts = InvoiceFacts {
            status: Some(InvoiceStatus::Open),
            hosted_invoice_url: Some("https://stripe.example/invoice".to_string()),
        };
        assert_eq!(
            classify_invoice_facts(&facts),
            RecoveryOutcome::NeedsAction {
                hosted_invoice_url: Some("https://stripe.example/invoice".to_string())
            }
        );
    }

    #[test]
    fn uncollectible_invoice_is_declined() {
        let facts = InvoiceFacts {
            status: Some(InvoiceStatus::Uncollectible),
            hosted_invoice_url: None,
        };
        assert!(matches!(
            classify_invoice_facts(&facts),
            RecoveryOutcome::Declined { .. }
        ));
    }

    // -- classify_pay_invoice_error_facts ---------------------------------
    //
    // Constructed directly against `PayInvoiceErrorFacts` rather than a real
    // `StripeError`/`ApiErrors`/`PaymentIntent` — the real `PaymentIntent`
    // type alone has 40+ required fields with no `Default` impl (see the
    // module doc's *Testability* note), so these tests exercise the actual
    // decision function without paying that construction cost.

    #[test]
    fn requires_action_payment_intent_is_needs_action() {
        let facts = PayInvoiceErrorFacts {
            message: Some("requires action".to_string()),
            payment_intent_status: Some(PaymentIntentStatus::RequiresAction),
        };
        assert_eq!(
            classify_pay_invoice_error_facts(&facts, Some("https://x".to_string())),
            RecoveryOutcome::NeedsAction {
                hosted_invoice_url: Some("https://x".to_string())
            }
        );
    }

    #[test]
    fn requires_confirmation_payment_intent_is_needs_action() {
        let facts = PayInvoiceErrorFacts {
            message: Some("requires confirmation".to_string()),
            payment_intent_status: Some(PaymentIntentStatus::RequiresConfirmation),
        };
        assert_eq!(
            classify_pay_invoice_error_facts(&facts, None),
            RecoveryOutcome::NeedsAction {
                hosted_invoice_url: None
            }
        );
    }

    #[test]
    fn declined_card_with_no_payment_intent_is_declined() {
        let facts = PayInvoiceErrorFacts {
            message: Some("Your card was declined.".to_string()),
            payment_intent_status: None,
        };
        assert_eq!(
            classify_pay_invoice_error_facts(&facts, None),
            RecoveryOutcome::Declined {
                message: "Your card was declined.".to_string()
            }
        );
    }

    #[test]
    fn declined_card_with_requires_payment_method_is_declined_not_needs_action() {
        // requires_payment_method after a failed attempt means the payment
        // method itself was rejected, not "needs one more step" — must be
        // Declined, not NeedsAction.
        let facts = PayInvoiceErrorFacts {
            message: Some("Your card was declined.".to_string()),
            payment_intent_status: Some(PaymentIntentStatus::RequiresPaymentMethod),
        };
        assert!(matches!(
            classify_pay_invoice_error_facts(&facts, None),
            RecoveryOutcome::Declined { .. }
        ));
    }

    #[test]
    fn missing_message_falls_back_to_generic_declined_text() {
        let facts = PayInvoiceErrorFacts {
            message: None,
            payment_intent_status: None,
        };
        assert_eq!(
            classify_pay_invoice_error_facts(&facts, None),
            RecoveryOutcome::Declined {
                message: "Payment failed".to_string()
            }
        );
    }

    // -- extract_pay_invoice_error_facts ----------------------------------

    #[test]
    fn non_api_stripe_error_has_no_facts() {
        assert_eq!(extract_pay_invoice_error_facts(&StripeError::Timeout), None);
    }

    #[test]
    fn client_error_has_no_facts() {
        assert_eq!(
            extract_pay_invoice_error_facts(&StripeError::ClientError("reset".into())),
            None
        );
    }

    #[test]
    fn api_error_without_payment_intent_has_facts_with_no_status() {
        let err = StripeError::Stripe(
            Box::new(stripe_shared::ApiErrors {
                type_: stripe_shared::ApiErrorsType::CardError,
                advice_code: None,
                charge: None,
                code: None,
                decline_code: None,
                doc_url: None,
                message: Some("Your card was declined.".to_string()),
                network_advice_code: None,
                network_decline_code: None,
                param: None,
                payment_intent: None,
                payment_method: None,
                payment_method_type: None,
                request_log_url: None,
                setup_intent: None,
                source: None,
            }),
            402,
        );
        let facts = extract_pay_invoice_error_facts(&err).expect("api error has facts");
        assert_eq!(facts.message.as_deref(), Some("Your card was declined."));
        assert_eq!(facts.payment_intent_status, None);
    }

    // -- classify_webhook_checkout_session --------------------------------

    #[test]
    fn payment_mode_is_bundle() {
        assert_eq!(
            classify_webhook_checkout_session(&CheckoutSessionMode::Payment, None),
            CheckoutSessionKind::Bundle
        );
    }

    #[test]
    fn setup_mode_with_recovery_marker_is_payment_recovery() {
        let mut metadata = HashMap::new();
        metadata.insert("purpose".to_string(), RECOVERY_PURPOSE_MARKER.to_string());
        assert_eq!(
            classify_webhook_checkout_session(&CheckoutSessionMode::Setup, Some(&metadata)),
            CheckoutSessionKind::PaymentRecovery
        );
    }

    #[test]
    fn setup_mode_without_recovery_marker_is_ignored() {
        assert_eq!(
            classify_webhook_checkout_session(&CheckoutSessionMode::Setup, None),
            CheckoutSessionKind::Ignore
        );
    }

    #[test]
    fn subscription_mode_is_ignored_here() {
        // Subscription-mode sessions are handled entirely by the
        // customer.subscription.created/updated events — this classifier
        // must route them to Ignore so handle_checkout_completed's existing
        // bundle-only logic is untouched for that mode.
        assert_eq!(
            classify_webhook_checkout_session(&CheckoutSessionMode::Subscription, None),
            CheckoutSessionKind::Ignore
        );
    }

    // -- validate_new_subscription_session_facts (KYO-806 A4) ------------

    fn valid_new_subscription_facts() -> NewSubscriptionSessionFacts {
        NewSubscriptionSessionFacts {
            mode: CheckoutSessionMode::Subscription,
            metadata_workspace_id: Some("ws-1".to_string()),
            customer_id: Some("cus_1".to_string()),
            has_subscription: true,
        }
    }

    #[test]
    fn valid_new_subscription_session_passes() {
        let facts = valid_new_subscription_facts();
        assert_eq!(
            validate_new_subscription_session_facts(&facts, "ws-1", "cus_1"),
            Ok(())
        );
    }

    #[test]
    fn new_subscription_rejects_non_subscription_mode() {
        let mut facts = valid_new_subscription_facts();
        facts.mode = CheckoutSessionMode::Setup;
        assert_eq!(
            validate_new_subscription_session_facts(&facts, "ws-1", "cus_1"),
            Err(NewSubscriptionValidationError::NotSubscriptionMode)
        );
    }

    #[test]
    fn new_subscription_rejects_workspace_mismatch() {
        // The security check: a caller must not sync another workspace's
        // completed checkout session into their own workspace row.
        let facts = valid_new_subscription_facts();
        assert_eq!(
            validate_new_subscription_session_facts(&facts, "ws-other", "cus_1"),
            Err(NewSubscriptionValidationError::WorkspaceMismatch)
        );
    }

    #[test]
    fn new_subscription_rejects_customer_mismatch() {
        let facts = valid_new_subscription_facts();
        assert_eq!(
            validate_new_subscription_session_facts(&facts, "ws-1", "cus_other"),
            Err(NewSubscriptionValidationError::CustomerMismatch)
        );
    }

    #[test]
    fn new_subscription_rejects_missing_subscription() {
        let mut facts = valid_new_subscription_facts();
        facts.has_subscription = false;
        assert_eq!(
            validate_new_subscription_session_facts(&facts, "ws-1", "cus_1"),
            Err(NewSubscriptionValidationError::NoSubscription)
        );
    }
}
