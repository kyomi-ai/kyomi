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
    websocket::WebSocketManager,
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
/// handlers (`crate::billing_webhook`), by past-due payment recovery
/// (`crate::payment_recovery`), and by the new-subscription checkout sync
/// path (`kyomi_ui::server_fns::billing::sync_checkout_subscription`), so
/// the three call sites cannot drift from each other the way the
/// webhook-only version previously could.
///
/// `sub_data.period_start` / `period_end` are bound as `Option<DateTime<Utc>>`
/// directly — **never** convert them to RFC3339 strings first. Postgres's
/// `timestamp with time zone` column rejects `text` binds with `column …
/// is of type timestamp with time zone but expression is of type text`
/// (the KYO-106 production bug); sqlx's chrono integration maps
/// `Option<DateTime<Utc>>` to `TIMESTAMPTZ` natively.
///
/// The `UPDATE` carries a change-detecting `WHERE` predicate (`IS DISTINCT
/// FROM` — supported by both Postgres and this workspace's SQLite 3.46) so a
/// write that would leave every touched column unchanged affects zero rows.
/// [`WebSocketManager::broadcast_to_workspace`] fires the KYO-807
/// `billing_status_changed` event iff at least one row was actually
/// affected, so a caller that runs this with data identical to what's
/// already on the row (a duplicate/retried webhook, an idempotent recovery
/// re-run) never sends a spurious event. Returns whether the row changed, so
/// callers can log it.
///
/// Does not touch MCP session invalidation — callers that need it (every
/// current caller does) run that themselves after this returns `Ok`, since
/// the invalidation step differs slightly by caller (webhook uses
/// `state.mcp_sessions` directly; server fns go through `ac.ctx.mcp_sessions`).
pub async fn write_subscription_to_workspace(
    db: &DbPool,
    ws: &WebSocketManager,
    workspace_id: &str,
    sub_data: &crate::stripe_service::SubscriptionData,
    mode: SubscriptionWriteMode,
) -> Result<bool, Error> {
    let result = match mode {
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
                 WHERE workspace_id = $9 \
                   AND ( \
                     subscription_tier IS DISTINCT FROM $10 \
                     OR subscription_status IS DISTINCT FROM $11 \
                     OR billing_cycle IS DISTINCT FROM $12 \
                     OR subscription_period_start IS DISTINCT FROM $13 \
                     OR subscription_period_end IS DISTINCT FROM $14 \
                     OR stripe_subscription_id IS DISTINCT FROM $15 \
                     OR stripe_customer_id IS DISTINCT FROM $16 \
                     OR user_limit IS DISTINCT FROM $17 \
                     OR ai_credits_used_usd IS DISTINCT FROM 0.0 \
                   )",
                &sub_data.tier,
                &sub_data.status,
                sub_data.billing_cycle.as_deref(),
                sub_data.period_start,
                sub_data.period_end,
                &sub_data.stripe_subscription_id,
                &sub_data.stripe_customer_id,
                sub_data.user_limit,
                workspace_id,
                &sub_data.tier,
                &sub_data.status,
                sub_data.billing_cycle.as_deref(),
                sub_data.period_start,
                sub_data.period_end,
                &sub_data.stripe_subscription_id,
                &sub_data.stripe_customer_id,
                sub_data.user_limit
            )?
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
                 WHERE workspace_id = $7 \
                   AND ( \
                     subscription_tier IS DISTINCT FROM $8 \
                     OR subscription_status IS DISTINCT FROM $9 \
                     OR billing_cycle IS DISTINCT FROM $10 \
                     OR subscription_period_start IS DISTINCT FROM $11 \
                     OR subscription_period_end IS DISTINCT FROM $12 \
                     OR user_limit IS DISTINCT FROM $13 \
                   )",
                &sub_data.tier,
                &sub_data.status,
                sub_data.billing_cycle.as_deref(),
                sub_data.period_start,
                sub_data.period_end,
                sub_data.user_limit,
                workspace_id,
                &sub_data.tier,
                &sub_data.status,
                sub_data.billing_cycle.as_deref(),
                sub_data.period_start,
                sub_data.period_end,
                sub_data.user_limit
            )?
        }
    };

    let changed = result.rows_affected() > 0;
    if changed {
        crate::websocket::helpers::broadcast_billing_status_changed(ws, workspace_id).await;
    }
    Ok(changed)
}

// ─── Billing-state writers with change detection + broadcast (KYO-807) ─────
//
// Siblings of `write_subscription_to_workspace` above, for the billing-state
// writes that don't go through Stripe's `SubscriptionData` shape: reverting
// to free on `customer.subscription.deleted`, setting `subscription_status`
// directly (past_due, cancel, reactivate), and resetting AI credit usage for
// a new billing period. Each follows the identical change-detect-then-
// broadcast-iff-changed contract `write_subscription_to_workspace` documents
// above, so every billing-state mutation in this codebase behaves the same
// way from the client's perspective regardless of which writer touched the
// row.

/// Revert a workspace to the free tier — the `customer.subscription.deleted`
/// webhook's write. Called by `crate::billing_webhook::apply_subscription_deleted_webhook_event`.
///
/// `999999` (`kyomi_core::capability::UNLIMITED_USER_LIMIT`) matches the
/// sentinel `kyomi_core::capability::billing_gate_blocks` and the invite
/// flow already treat as "no seat cap".
pub async fn revert_subscription_to_free(
    db: &DbPool,
    ws: &WebSocketManager,
    workspace_id: &str,
) -> Result<bool, Error> {
    let unlimited = kyomi_core::capability::UNLIMITED_USER_LIMIT;
    let result = kyomi_core::db_execute!(
        db,
        "UPDATE workspaces SET \
             subscription_tier = 'free', \
             subscription_status = 'cancelled', \
             billing_cycle = NULL, \
             subscription_period_start = NULL, \
             subscription_period_end = NULL, \
             stripe_subscription_id = NULL, \
             user_limit = $1, \
             ai_credits_used_usd = 0.0 \
         WHERE workspace_id = $2 \
           AND ( \
             subscription_tier IS DISTINCT FROM 'free' \
             OR subscription_status IS DISTINCT FROM 'cancelled' \
             OR billing_cycle IS NOT NULL \
             OR subscription_period_start IS NOT NULL \
             OR subscription_period_end IS NOT NULL \
             OR stripe_subscription_id IS NOT NULL \
             OR user_limit IS DISTINCT FROM $1 \
             OR ai_credits_used_usd IS DISTINCT FROM 0.0 \
           )",
        unlimited,
        workspace_id
    )?;

    let changed = result.rows_affected() > 0;
    if changed {
        crate::websocket::helpers::broadcast_billing_status_changed(ws, workspace_id).await;
    }
    Ok(changed)
}

/// Set `subscription_status` directly — the shared writer for the three
/// callers that only ever change this one column: the
/// `invoice.payment_failed` webhook (→ `"past_due"`), and the
/// `cancel_subscription`/`reactivate_subscription` server fns
/// (`crates/kyomi-ui/src/server_fns/billing.rs`, → `"cancelled"`/`"active"`).
pub async fn set_subscription_status(
    db: &DbPool,
    ws: &WebSocketManager,
    workspace_id: &str,
    status: &str,
) -> Result<bool, Error> {
    let result = kyomi_core::db_execute!(
        db,
        "UPDATE workspaces SET subscription_status = $1 \
         WHERE workspace_id = $2 AND subscription_status IS DISTINCT FROM $3",
        status,
        workspace_id,
        status
    )?;

    let changed = result.rows_affected() > 0;
    if changed {
        crate::websocket::helpers::broadcast_billing_status_changed(ws, workspace_id).await;
    }
    Ok(changed)
}

/// Reset `ai_credits_used_usd` to `0.0` for a new billing period — the
/// `invoice.payment_succeeded` webhook's write. AI credit usage is billing
/// state shown on the billing settings page, so a reset broadcasts the same
/// as any other writer here.
pub async fn reset_ai_credits_for_new_period(
    db: &DbPool,
    ws: &WebSocketManager,
    workspace_id: &str,
) -> Result<bool, Error> {
    let result = kyomi_core::db_execute!(
        db,
        "UPDATE workspaces SET ai_credits_used_usd = 0.0 \
         WHERE workspace_id = $1 AND ai_credits_used_usd IS DISTINCT FROM 0.0",
        workspace_id
    )?;

    let changed = result.rows_affected() > 0;
    if changed {
        crate::websocket::helpers::broadcast_billing_status_changed(ws, workspace_id).await;
    }
    Ok(changed)
}

/// Modify an existing Stripe subscription to the current Cloud price,
/// persist the new state to the DB, and immediately invalidate MCP
/// sessions so connected clients pick up the new tool capabilities
/// without waiting for the Stripe webhook round-trip.
///
/// Steps:
/// 1. Call Stripe to update the subscription to the current Cloud price.
/// 2. Write the resulting Stripe state back to the workspaces DB row via
///    the one shared writer ([`write_subscription_to_workspace`], KYO-806
///    A5), which also fires the KYO-807 `billing_status_changed` broadcast
///    when the write actually changes the row. This function used to run
///    its own inline `UPDATE` here, binding `period_start`/`period_end` as
///    RFC3339 **strings** — the exact KYO-106 production bug
///    [`write_subscription_to_workspace`]'s doc comment warns about
///    (Postgres's `timestamptz` column rejects a `text` bind). Routing
///    through the shared writer, which binds `Option<DateTime<Utc>>`
///    directly, fixes that.
/// 3. Push `notifications/tools/list_changed` to SSE clients on this
///    replica, then invalidate workspace sessions in the KV store so
///    clients connected to other replicas re-initialize on their next
///    request.
pub async fn modify_existing_subscription(
    db: &DbPool,
    stripe: &StripeService,
    ws: &WebSocketManager,
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

    // Update workspace from Stripe data (source of truth) — see the doc
    // comment above for why this goes through the shared writer rather than
    // a second, hand-rolled `UPDATE`.
    write_subscription_to_workspace(db, ws, workspace_id, &sub_data, SubscriptionWriteMode::Updated)
        .await?;

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

    // -- billing-state writers: change detection + broadcast (KYO-807) ----
    //
    // Real integration tests against an in-memory SQLite pool and a real
    // `WebSocketManager` in single-instance mode (no Redis) — the same style
    // `websocket::helpers`'s own test module uses. `manager.connect(user_id)`
    // sends an immediate Heartbeat before anything else, so every test below
    // drains that first.

    use crate::test_support::{seed_membership, seed_user, seed_workspace, sqlite_pool, test_pool};
    use crate::websocket::WebSocketManager;
    use kyomi_core::{MessageType, WebSocketMessage};
    use tokio::sync::mpsc;

    fn sample_sub_data(status: &str) -> crate::stripe_service::SubscriptionData {
        crate::stripe_service::SubscriptionData {
            tier: "cloud".to_string(),
            status: status.to_string(),
            billing_cycle: Some("monthly".to_string()),
            user_limit: kyomi_core::capability::UNLIMITED_USER_LIMIT,
            period_start: Some(chrono::Utc::now()),
            period_end: Some(chrono::Utc::now() + chrono::Duration::days(30)),
            stripe_subscription_id: "sub_test_1".to_string(),
            stripe_customer_id: "cus_test_1".to_string(),
        }
    }

    /// Connect a member and drain the immediate `connect()` Heartbeat so
    /// tests can assert on exactly the messages a writer under test sends.
    async fn connect_draining_heartbeat(
        manager: &WebSocketManager,
        user_id: &str,
    ) -> mpsc::Receiver<String> {
        let (_conn_id, mut rx) = manager.connect(user_id).expect("connect");
        rx.try_recv().expect("connect() must send an immediate heartbeat");
        rx
    }

    fn expect_billing_status_changed(rx: &mut mpsc::Receiver<String>, workspace_id: &str) {
        let raw = rx
            .try_recv()
            .expect("expected exactly one billing_status_changed message");
        let envelope: WebSocketMessage =
            serde_json::from_str(&raw).expect("valid WebSocketMessage JSON");
        assert_eq!(envelope.message_type, MessageType::BillingStatusChanged);
        let data = envelope.data.expect("billing_status_changed must carry data");
        assert_eq!(
            data.get("workspace_id").and_then(|v| v.as_str()),
            Some(workspace_id)
        );
        assert!(
            rx.try_recv().is_err(),
            "expected exactly one billing_status_changed message, got more"
        );
    }

    fn assert_no_message(rx: &mut mpsc::Receiver<String>, context: &str) {
        assert!(
            rx.try_recv().is_err(),
            "{context}: expected no message, got one"
        );
    }

    #[tokio::test]
    async fn write_subscription_to_workspace_created_broadcasts_once_on_real_change() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_membership(sq, "ws-1", "owner", "workspace_admin", true).await;

        let manager = WebSocketManager::new(None, db.clone());
        let mut rx = connect_draining_heartbeat(&manager, "owner").await;

        let changed = write_subscription_to_workspace(
            &db,
            &manager,
            "ws-1",
            &sample_sub_data("active"),
            SubscriptionWriteMode::Created,
        )
        .await
        .expect("write must succeed");

        assert!(changed);
        expect_billing_status_changed(&mut rx, "ws-1");
    }

    #[tokio::test]
    async fn write_subscription_to_workspace_no_op_repeat_does_not_broadcast() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_membership(sq, "ws-1", "owner", "workspace_admin", true).await;

        let manager = WebSocketManager::new(None, db.clone());
        let mut rx = connect_draining_heartbeat(&manager, "owner").await;
        let sub_data = sample_sub_data("active");

        let first = write_subscription_to_workspace(
            &db,
            &manager,
            "ws-1",
            &sub_data,
            SubscriptionWriteMode::Created,
        )
        .await
        .expect("first write must succeed");
        assert!(first);
        expect_billing_status_changed(&mut rx, "ws-1");

        // Identical data, written again (e.g. a duplicate/retried webhook).
        let second = write_subscription_to_workspace(
            &db,
            &manager,
            "ws-1",
            &sub_data,
            SubscriptionWriteMode::Created,
        )
        .await
        .expect("second write must succeed");
        assert!(!second, "identical data must not count as a change");
        assert_no_message(&mut rx, "no-op repeat write");
    }

    #[tokio::test]
    async fn write_subscription_to_workspace_nonexistent_workspace_does_not_broadcast() {
        let db = test_pool().await;
        let manager = WebSocketManager::new(None, db.clone());

        let changed = write_subscription_to_workspace(
            &db,
            &manager,
            "ws-does-not-exist",
            &sample_sub_data("active"),
            SubscriptionWriteMode::Updated,
        )
        .await
        .expect("write against a missing workspace must not error");

        assert!(!changed);
    }

    #[tokio::test]
    async fn write_subscription_to_workspace_only_reaches_its_own_workspace() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_user(sq, "other-owner", "other-owner@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_workspace(sq, "ws-2", "other-owner").await;
        seed_membership(sq, "ws-1", "owner", "workspace_admin", true).await;
        seed_membership(sq, "ws-2", "other-owner", "workspace_admin", true).await;

        let manager = WebSocketManager::new(None, db.clone());
        let mut rx_ws1 = connect_draining_heartbeat(&manager, "owner").await;
        let mut rx_ws2 = connect_draining_heartbeat(&manager, "other-owner").await;

        write_subscription_to_workspace(
            &db,
            &manager,
            "ws-1",
            &sample_sub_data("active"),
            SubscriptionWriteMode::Created,
        )
        .await
        .expect("write must succeed");

        expect_billing_status_changed(&mut rx_ws1, "ws-1");
        assert_no_message(&mut rx_ws2, "a different workspace's member");
    }

    #[tokio::test]
    async fn revert_subscription_to_free_broadcasts_once_and_is_idempotent() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_membership(sq, "ws-1", "owner", "workspace_admin", true).await;

        let manager = WebSocketManager::new(None, db.clone());
        let mut rx = connect_draining_heartbeat(&manager, "owner").await;

        // First move the workspace onto a paid subscription so reverting is
        // an actual change.
        write_subscription_to_workspace(
            &db,
            &manager,
            "ws-1",
            &sample_sub_data("active"),
            SubscriptionWriteMode::Created,
        )
        .await
        .expect("setup write must succeed");
        expect_billing_status_changed(&mut rx, "ws-1");

        let changed = revert_subscription_to_free(&db, &manager, "ws-1")
            .await
            .expect("revert must succeed");
        assert!(changed);
        expect_billing_status_changed(&mut rx, "ws-1");

        // Already free/cancelled — must not broadcast again.
        let changed_again = revert_subscription_to_free(&db, &manager, "ws-1")
            .await
            .expect("second revert must succeed");
        assert!(!changed_again);
        assert_no_message(&mut rx, "repeat revert to free");
    }

    #[tokio::test]
    async fn revert_subscription_to_free_nonexistent_workspace_does_not_broadcast() {
        let db = test_pool().await;
        let manager = WebSocketManager::new(None, db.clone());

        let changed = revert_subscription_to_free(&db, &manager, "ws-does-not-exist")
            .await
            .expect("revert against a missing workspace must not error");
        assert!(!changed);
    }

    #[tokio::test]
    async fn set_subscription_status_broadcasts_once_and_is_idempotent() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_membership(sq, "ws-1", "owner", "workspace_admin", true).await;

        let manager = WebSocketManager::new(None, db.clone());
        let mut rx = connect_draining_heartbeat(&manager, "owner").await;

        // Default subscription_status on a freshly seeded workspace is
        // 'active' (see apps/server/migrations-sqlite/00001_baseline.sql).
        let changed = set_subscription_status(&db, &manager, "ws-1", "past_due")
            .await
            .expect("status write must succeed");
        assert!(changed);
        expect_billing_status_changed(&mut rx, "ws-1");

        let changed_again = set_subscription_status(&db, &manager, "ws-1", "past_due")
            .await
            .expect("repeat status write must succeed");
        assert!(!changed_again, "setting the same status again must not count as a change");
        assert_no_message(&mut rx, "repeat set_subscription_status");
    }

    #[tokio::test]
    async fn set_subscription_status_nonexistent_workspace_does_not_broadcast() {
        let db = test_pool().await;
        let manager = WebSocketManager::new(None, db.clone());

        let changed = set_subscription_status(&db, &manager, "ws-does-not-exist", "past_due")
            .await
            .expect("status write against a missing workspace must not error");
        assert!(!changed);
    }

    #[tokio::test]
    async fn reset_ai_credits_for_new_period_broadcasts_once_and_is_idempotent() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_membership(sq, "ws-1", "owner", "workspace_admin", true).await;
        sqlx::query("UPDATE workspaces SET ai_credits_used_usd = 12.5 WHERE workspace_id = $1")
            .bind("ws-1")
            .execute(sq)
            .await
            .expect("seed ai_credits_used_usd");

        let manager = WebSocketManager::new(None, db.clone());
        let mut rx = connect_draining_heartbeat(&manager, "owner").await;

        let changed = reset_ai_credits_for_new_period(&db, &manager, "ws-1")
            .await
            .expect("reset must succeed");
        assert!(changed);
        expect_billing_status_changed(&mut rx, "ws-1");

        // Already reset — must not broadcast again.
        let changed_again = reset_ai_credits_for_new_period(&db, &manager, "ws-1")
            .await
            .expect("second reset must succeed");
        assert!(!changed_again);
        assert_no_message(&mut rx, "repeat AI-credit reset");
    }

    #[tokio::test]
    async fn reset_ai_credits_for_new_period_nonexistent_workspace_does_not_broadcast() {
        let db = test_pool().await;
        let manager = WebSocketManager::new(None, db.clone());

        let changed = reset_ai_credits_for_new_period(&db, &manager, "ws-does-not-exist")
            .await
            .expect("reset against a missing workspace must not error");
        assert!(!changed);
    }
}
