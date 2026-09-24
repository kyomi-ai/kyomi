// SPDX-License-Identifier: AGPL-3.0-or-later

//! Subscription modification service — shared by the REST route
//! (`apps/server/src/routes/billing.rs`) and the Leptos server_fn
//! (`crates/kyomi-ui/src/server_fns/billing.rs`).
//!
//! Owns the full "modify an existing subscription" flow so both entry
//! points invalidate MCP sessions immediately. Before consolidation, the
//! Leptos server_fn only updated Stripe + DB and relied on the Stripe
//! webhook to invalidate MCP sessions, leaving MCP clients with stale
//! tool capabilities for the duration of the webhook round-trip.

use kyomi_core::{DbPool, Error};

use crate::{
    mcp_session_manager::MCPSessionManager, stripe_config, stripe_service::StripeService,
};

/// Result of a successful subscription modification.
///
/// Callers format this into their response shape (Json for REST,
/// `CheckoutOutcome::Modified` for the Leptos server_fn). The underlying
/// Stripe + DB + MCP invalidation sequence is identical either way.
#[derive(Debug, Clone)]
pub struct ModifySubscriptionResult {
    pub tier: String,
    pub status: String,
    pub billing_cycle: Option<String>,
    pub user_limit: i32,
}

// ─── Checkout path guard (KYO-806 A6) ───────────────────────────────────────

/// Which flow `create_checkout` (`crates/kyomi-ui/src/server_fns/billing.rs`)
/// should run for the caller's current workspace subscription state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutPath {
    /// `active` or `cancelled` with a live `stripe_subscription_id` — modify
    /// the existing subscription in place (`modify_existing_subscription`).
    ModifyExisting,
    /// No existing Stripe subscription to act on: a `cancelled` workspace
    /// with `stripe_subscription_id = NULL` (Stripe already deleted it), or
    /// the app-managed no-Stripe trial that expired. Start a brand-new
    /// subscription.
    NewSubscription,
    /// `past_due` **with** a live `stripe_subscription_id` — there is
    /// exactly one subscription and it owes money. `create_checkout` must
    /// refuse this path entirely rather than route it anywhere that calls
    /// Stripe: routing it to `NewSubscription` creates a second,
    /// competing subscription (the bug this variant exists to make
    /// unrepresentable); routing it to `ModifyExisting` would silently
    /// paper over billing state `modify_existing_subscription` was never
    /// designed to reconcile. Recovering a `past_due` subscription is
    /// `crate::payment_recovery::recover_past_due_payment`'s job, reached
    /// through the separate `start_payment_recovery` /
    /// `complete_payment_recovery` server fns, not `create_checkout`.
    RecoverPastDue,
}

/// Decide [`CheckoutPath`] from a workspace's `subscription_status` and
/// `stripe_subscription_id` — pure, and the single place this routing
/// decision is made (KYO-806 A6). `status` matches the DB column's string
/// values (`"active"`, `"cancelled"`, `"past_due"`, `"trialing"`), the same
/// representation `create_checkout`'s `WorkspaceRow` already carries.
///
/// Structured so [`CheckoutPath::NewSubscription`] is reachable **only**
/// when `stripe_subscription_id` is `None` — not merely as a convention
/// this function's author kept in mind, but because every `Some(_)` arm
/// below yields [`CheckoutPath::RecoverPastDue`] or
/// [`CheckoutPath::ModifyExisting`] and nothing else. This makes the
/// double-subscription defect class structurally unreachable rather than
/// merely absent for the status strings this function currently
/// recognises: an unrecognised or future `status` value paired with an
/// existing subscription id still can't fall through to
/// `NewSubscription`, because there is no code path from `Some(_)` to that
/// variant at all — see
/// `docs/standards/code-organization/close-the-class-by-making-the-wrong-call-uncallable.md`.
/// A workspace whose status isn't literally `"past_due"` but does have a
/// live subscription id (e.g. `"active"`, `"cancelled"` in its grace
/// period, or an unrecognised value) safely routes to `ModifyExisting`:
/// modifying an existing subscription in place is never the unsafe
/// operation here, only creating a second one is.
pub fn checkout_path(status: &str, stripe_subscription_id: Option<&str>) -> CheckoutPath {
    match stripe_subscription_id {
        None => CheckoutPath::NewSubscription,
        Some(_) if status == "past_due" => CheckoutPath::RecoverPastDue,
        Some(_) => CheckoutPath::ModifyExisting,
    }
}

// ─── Shared workspace-subscription writer (KYO-806 A5) ─────────────────────

/// Which UPDATE shape [`write_subscription_to_workspace`] should run.
///
/// Mirrors the `customer.subscription.created` vs. `customer.subscription.updated`
/// split that used to live only in `apps/server/src/routes/billing.rs`'s
/// `handle_subscription_event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionWriteMode {
    /// A brand-new Stripe subscription. Also sets `stripe_subscription_id`
    /// and `stripe_customer_id` (which don't exist on the row yet) and
    /// resets `ai_credits_used_usd` to 0.0 for the new billing period.
    Created,
    /// An existing Stripe subscription changed state. Never overwrites
    /// `stripe_subscription_id` / `stripe_customer_id` and never resets
    /// credit usage — only the created path does either of those.
    Updated,
}

/// Write Stripe-parsed subscription data to a workspace row.
///
/// The ONE writer of this shape (KYO-806 A5) — used by the webhook's
/// `customer.subscription.created` / `customer.subscription.updated`
/// handlers (`apps/server/src/routes/billing.rs`), by past-due payment
/// recovery (`crate::payment_recovery`), and by the new-subscription
/// checkout sync path (`kyomi_ui::server_fns::billing::sync_checkout_subscription`),
/// so the three call sites cannot drift from each other the way the
/// webhook-only version previously could.
///
/// `sub_data.period_start` / `period_end` are bound as `Option<DateTime<Utc>>`
/// directly — **never** convert them to RFC3339 strings first. Postgres's
/// `timestamp with time zone` column rejects `text` binds with `column …
/// is of type timestamp with time zone but expression is of type text`
/// (the KYO-106 production bug); sqlx's chrono integration maps
/// `Option<DateTime<Utc>>` to `TIMESTAMPTZ` natively.
///
/// Does not touch MCP session invalidation — callers that need it (every
/// current caller does) run that themselves after this returns `Ok`, since
/// the invalidation step differs slightly by caller (webhook uses
/// `state.mcp_sessions` directly; server fns go through `ac.ctx.mcp_sessions`).
pub async fn write_subscription_to_workspace(
    db: &DbPool,
    workspace_id: &str,
    sub_data: &crate::stripe_service::SubscriptionData,
    mode: SubscriptionWriteMode,
) -> Result<(), Error> {
    match mode {
        SubscriptionWriteMode::Created => {
            kyomi_core::db_execute!(
                db,
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
                workspace_id
            )?;
        }
        SubscriptionWriteMode::Updated => {
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
                sub_data.period_start,
                sub_data.period_end,
                sub_data.user_limit,
                workspace_id
            )?;
        }
    }

    Ok(())
}

/// Modify an existing Stripe subscription to the current Cloud price,
/// persist the new state to the DB, and immediately invalidate MCP
/// sessions so connected clients pick up the new tool capabilities
/// without waiting for the Stripe webhook round-trip.
///
/// Steps:
/// 1. Call Stripe to update the subscription to the current Cloud price.
/// 2. Write the resulting Stripe state back to the workspaces DB row.
/// 3. Push `notifications/tools/list_changed` to SSE clients on this
///    replica, then invalidate workspace sessions in the KV store so
///    clients connected to other replicas re-initialize on their next
///    request.
pub async fn modify_existing_subscription(
    db: &DbPool,
    stripe: &StripeService,
    mcp_sessions: &MCPSessionManager,
    workspace_id: &str,
    subscription_id: &str,
) -> Result<ModifySubscriptionResult, Error> {
    let new_price_id = stripe_config::get_cloud_price_id().ok_or_else(|| {
        Error::BadRequest("STRIPE_CLOUD_MONTHLY not configured".to_string())
    })?;

    // Modify the subscription to the Cloud price
    let sub_data = stripe
        .update_subscription(subscription_id, new_price_id, "cloud", "monthly")
        .await
        .map_err(|e| {
            tracing::error!("Failed to modify subscription: {e}");
            Error::Internal(format!("Failed to modify subscription: {e}"))
        })?;

    // Update workspace from Stripe data (source of truth)
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
    )?;

    tracing::info!(
        workspace_id = %workspace_id,
        tier = %sub_data.tier,
        "Modified existing subscription"
    );

    // Notify connected SSE clients that tools have changed, then invalidate
    // all sessions so disconnected clients re-initialize on next request.
    mcp_sessions.notify_tools_changed(workspace_id).await;
    mcp_sessions
        .invalidate_workspace_sessions(workspace_id)
        .await;

    Ok(ModifySubscriptionResult {
        tier: sub_data.tier,
        status: sub_data.status,
        billing_cycle: sub_data.billing_cycle,
        user_limit: sub_data.user_limit,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- checkout_path (KYO-806 A6) ---------------------------------------
    //
    // The exact regression this guard exists to prevent: past_due with a
    // live subscription id must never yield NewSubscription.

    #[test]
    fn past_due_with_subscription_never_yields_new_subscription() {
        assert_eq!(checkout_path("past_due", Some("sub_123")), CheckoutPath::RecoverPastDue);
        assert_ne!(checkout_path("past_due", Some("sub_123")), CheckoutPath::NewSubscription);
    }

    #[test]
    fn past_due_without_subscription_id_is_new_subscription() {
        // No existing subscription to recover — nothing to double up.
        assert_eq!(checkout_path("past_due", None), CheckoutPath::NewSubscription);
    }

    #[test]
    fn active_with_subscription_modifies_existing() {
        assert_eq!(checkout_path("active", Some("sub_123")), CheckoutPath::ModifyExisting);
    }

    #[test]
    fn cancelled_with_subscription_modifies_existing() {
        assert_eq!(checkout_path("cancelled", Some("sub_123")), CheckoutPath::ModifyExisting);
    }

    #[test]
    fn cancelled_without_subscription_is_new_subscription() {
        // Stripe already deleted the subscription (stripe_subscription_id
        // cleared to NULL) — nothing to modify, start fresh.
        assert_eq!(checkout_path("cancelled", None), CheckoutPath::NewSubscription);
    }

    #[test]
    fn trialing_without_subscription_is_new_subscription() {
        // The expired no-Stripe trial case.
        assert_eq!(checkout_path("trialing", None), CheckoutPath::NewSubscription);
    }

    #[test]
    fn trialing_with_subscription_modifies_existing_not_new() {
        // A status this function doesn't treat specially (trialing) paired
        // with a live subscription id — proves the structural guarantee,
        // not just the four statuses this function names: Some(_) never
        // reaches NewSubscription, regardless of which status string it is.
        assert_eq!(checkout_path("trialing", Some("sub_123")), CheckoutPath::ModifyExisting);
    }

    #[test]
    fn unrecognized_status_with_subscription_never_yields_new_subscription() {
        // A future/unrecognised status string, still paired with a live
        // subscription id, must never create a second subscription either
        // — the same invariant `past_due_with_subscription_never_yields_new_subscription`
        // checks for the known dangerous case, here for an input nobody
        // has classified yet.
        assert_ne!(
            checkout_path("some_future_status", Some("sub_123")),
            CheckoutPath::NewSubscription
        );
    }

    /// Verify that MCP invalidation runs against an in-memory KV store.
    ///
    /// The Stripe call itself is not mocked (there's no Stripe mocking
    /// infrastructure in the crate). This test exercises the invalidation
    /// path directly — the service's other behaviour (Stripe update + DB
    /// write) is covered by the REST route's existing contract tests.
    #[tokio::test]
    async fn mcp_invalidation_clears_workspace_sessions() {
        let kv = kyomi_core::kv_store::create_kv_store(None)
            .await
            .expect("in-memory KV store should initialize");
        let mcp = MCPSessionManager::new(kv);

        // Seed two sessions for the target workspace and one for a different workspace.
        let s1 = mcp.create_session("ws-subscription-test-1").await;
        let s2 = mcp.create_session("ws-subscription-test-1").await;
        let other = mcp.create_session("ws-subscription-test-other").await;

        // Perform the invalidation portion of the service flow directly
        // against the same manager. This matches what
        // `modify_existing_subscription` does after the Stripe + DB steps.
        mcp.notify_tools_changed("ws-subscription-test-1").await;
        mcp.invalidate_workspace_sessions("ws-subscription-test-1")
            .await;

        assert!(mcp.validate_session(&s1).await.is_none());
        assert!(mcp.validate_session(&s2).await.is_none());
        // Untargeted workspace must survive.
        assert_eq!(
            mcp.validate_session(&other).await,
            Some("ws-subscription-test-other".to_string())
        );
    }
}
