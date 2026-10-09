// SPDX-License-Identifier: AGPL-3.0-or-later

//! Testable decision + write logic for the Stripe billing webhook handlers
//! (KYO-807), extracted from `apps/server/src/routes/billing.rs`.
//!
//! Those handlers take `&AppState`, which is impractical to build in a unit
//! test (it wires up the full server: DB pool, config, every optional
//! service). Every function here instead takes narrow, directly-constructible
//! dependencies — `&DbPool`, `&WebSocketManager`, `&MCPSessionManager`, and
//! either a real Stripe event object (`Subscription`, `Invoice`) or a small
//! facts struct extracted from one by a thin, field-only adapter.
//!
//! Both `Invoice` and `Subscription` turn out to be constructible in a test
//! via `serde_json::from_str` against a realistic fixture — unlike
//! `payment_recovery`'s `CheckoutSession`, whose module doc calls the same
//! approach impractical there. `Subscription` is the deeper of the two
//! (`Subscription.items.data[].plan`/`.price` are required, full nested
//! objects, each 15-20 fields), but every field is still a primitive, an
//! enum, or a small nested struct with no unbounded recursion — see
//! [`tests::subscription_fixture`] for the fixture this file's tests
//! actually exercise.
//!
//! The subscription-event guard (Kyomi-app check + `workspace_id`
//! extraction) is *additionally* tested via [`SubscriptionEventFacts`],
//! extracted from the real `Subscription` by
//! [`extract_subscription_event_facts`] (field access only, no decisions —
//! untested itself, matching `payment_recovery::extract_recovery_session_facts`'s
//! precedent) — this keeps the guard's own unit tests independent of the
//! fixture's exact shape, and is what makes
//! [`apply_subscription_deleted_event_facts`] fully testable without ever
//! touching `Subscription` at all (reverting to free never reads any
//! subscription field beyond metadata).
//!
//! `apps/server/src/routes/billing.rs`'s handlers become thin adapters:
//! deserialize the typed event Stripe's webhook crate already produced, call
//! the matching function here, and log the returned outcome.

use kyomi_core::{DbPool, Error};
use stripe_shared::{Invoice, Subscription};
use stripe_types::Expandable;

use crate::{
    mcp_session_manager::MCPSessionManager,
    stripe_service::StripeService,
    subscription_service::{
        reset_ai_credits_for_new_period, revert_subscription_to_free, set_subscription_status,
        write_subscription_to_workspace, SubscriptionWriteMode,
    },
    websocket::WebSocketManager,
};

// ─── Shared guard: Kyomi-app check + workspace_id extraction ──────────────

/// The subset of a `customer.subscription.*` event's fields the
/// created/updated/deleted guard needs. See the module doc's *Testability*
/// note for why this exists instead of validating the real `Subscription`
/// directly.
#[derive(Debug, Clone, PartialEq)]
pub struct SubscriptionEventFacts {
    pub app: Option<String>,
    pub workspace_id: Option<String>,
}

/// Extract [`SubscriptionEventFacts`] from a real `Subscription`. Field
/// extraction only — no decisions are made here; those all live in
/// [`resolve_subscription_event_workspace`].
fn extract_subscription_event_facts(subscription: &Subscription) -> SubscriptionEventFacts {
    SubscriptionEventFacts {
        app: subscription.metadata.get("app").cloned(),
        workspace_id: subscription.metadata.get("workspace_id").cloned(),
    }
}

/// Why a subscription event was refused before reaching any write. Every
/// variant is a reason to stop, never a partial success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubscriptionEventGuard {
    /// `metadata["app"] != "kyomi"` — a different app sharing this Stripe
    /// account. Prevents cross-app contamination.
    NotKyomiApp,
    /// Kyomi's subscription, but missing/empty `workspace_id` metadata.
    MissingWorkspaceId,
}

/// Resolve the workspace a subscription event is for, or the reason it must
/// be refused. Pure and directly unit-tested — shared by
/// `customer.subscription.created`/`.updated`/`.deleted`, since all three
/// events gate on the exact same two checks.
fn resolve_subscription_event_workspace(
    facts: &SubscriptionEventFacts,
) -> Result<String, SubscriptionEventGuard> {
    if facts.app.as_deref() != Some("kyomi") {
        return Err(SubscriptionEventGuard::NotKyomiApp);
    }
    match &facts.workspace_id {
        Some(id) if !id.is_empty() => Ok(id.clone()),
        _ => Err(SubscriptionEventGuard::MissingWorkspaceId),
    }
}

// ─── customer.subscription.created / .updated ──────────────────────────────

/// Outcome of [`apply_subscription_webhook_event`] — lets the webhook
/// handler log the exact reason nothing was written, without re-deriving the
/// guard logic itself.
#[derive(Debug, Clone, PartialEq)]
pub enum SubscriptionWebhookOutcome {
    /// `subscription.metadata["app"] != "kyomi"` — a different app sharing
    /// this Stripe account.
    NotKyomiApp,
    /// Kyomi's subscription, but missing/empty `workspace_id` metadata.
    MissingWorkspaceId,
    /// The write ran. `changed` mirrors `write_subscription_to_workspace`'s
    /// return — whether the row actually differed from what Stripe
    /// reported (and therefore whether `billing_status_changed` fired).
    Applied {
        workspace_id: String,
        tier: String,
        changed: bool,
    },
}

/// Apply a `customer.subscription.created`/`.updated` webhook event: guard
/// non-Kyomi subscriptions, parse Stripe's subscription data, write it
/// through the shared writer (KYO-806 A5 — which itself fires the KYO-807
/// `billing_status_changed` broadcast iff the write changed the row), and
/// invalidate MCP sessions.
pub async fn apply_subscription_webhook_event(
    db: &DbPool,
    ws: &WebSocketManager,
    mcp_sessions: &MCPSessionManager,
    stripe: &StripeService,
    subscription: &Subscription,
    mode: SubscriptionWriteMode,
) -> Result<SubscriptionWebhookOutcome, Error> {
    let facts = extract_subscription_event_facts(subscription);
    let workspace_id = match resolve_subscription_event_workspace(&facts) {
        Ok(id) => id,
        Err(SubscriptionEventGuard::NotKyomiApp) => {
            return Ok(SubscriptionWebhookOutcome::NotKyomiApp)
        }
        Err(SubscriptionEventGuard::MissingWorkspaceId) => {
            return Ok(SubscriptionWebhookOutcome::MissingWorkspaceId)
        }
    };

    let sub_data = stripe
        .parse_subscription_data(subscription)
        .await
        .map_err(|e| Error::Internal(format!("failed to parse subscription data: {e}")))?;

    let changed = write_subscription_to_workspace(db, ws, &workspace_id, &sub_data, mode).await?;

    mcp_sessions.notify_tools_changed(&workspace_id).await;
    mcp_sessions
        .invalidate_workspace_sessions(&workspace_id)
        .await;

    Ok(SubscriptionWebhookOutcome::Applied {
        workspace_id,
        tier: sub_data.tier,
        changed,
    })
}

// ─── customer.subscription.deleted ─────────────────────────────────────────

/// Outcome of [`apply_subscription_deleted_webhook_event`] /
/// [`apply_subscription_deleted_event_facts`].
#[derive(Debug, Clone, PartialEq)]
pub enum SubscriptionDeletedOutcome {
    NotKyomiApp,
    MissingWorkspaceId,
    Applied { workspace_id: String, changed: bool },
}

/// Apply a `customer.subscription.deleted` webhook event: extract facts from
/// the real `Subscription`, then delegate to
/// [`apply_subscription_deleted_event_facts`] — the testable core, since
/// reverting to free never reads any subscription field beyond metadata.
pub async fn apply_subscription_deleted_webhook_event(
    db: &DbPool,
    ws: &WebSocketManager,
    mcp_sessions: &MCPSessionManager,
    subscription: &Subscription,
) -> Result<SubscriptionDeletedOutcome, Error> {
    apply_subscription_deleted_event_facts(
        db,
        ws,
        mcp_sessions,
        &extract_subscription_event_facts(subscription),
    )
    .await
}

/// Guard, then revert the workspace to the free tier via
/// [`crate::subscription_service::revert_subscription_to_free`] and
/// invalidate MCP sessions. Takes [`SubscriptionEventFacts`] directly (not
/// `&Subscription`) so it's fully testable without constructing the real,
/// impractical-to-build Stripe type — see the module doc's *Testability*
/// note.
async fn apply_subscription_deleted_event_facts(
    db: &DbPool,
    ws: &WebSocketManager,
    mcp_sessions: &MCPSessionManager,
    facts: &SubscriptionEventFacts,
) -> Result<SubscriptionDeletedOutcome, Error> {
    let workspace_id = match resolve_subscription_event_workspace(facts) {
        Ok(id) => id,
        Err(SubscriptionEventGuard::NotKyomiApp) => {
            return Ok(SubscriptionDeletedOutcome::NotKyomiApp)
        }
        Err(SubscriptionEventGuard::MissingWorkspaceId) => {
            return Ok(SubscriptionDeletedOutcome::MissingWorkspaceId)
        }
    };

    let changed = revert_subscription_to_free(db, ws, &workspace_id).await?;

    mcp_sessions.notify_tools_changed(&workspace_id).await;
    mcp_sessions
        .invalidate_workspace_sessions(&workspace_id)
        .await;

    Ok(SubscriptionDeletedOutcome::Applied {
        workspace_id,
        changed,
    })
}

// ─── invoice.payment_failed / invoice.payment_succeeded ────────────────────

/// Outcome of [`apply_invoice_payment_failed_webhook_event`] and
/// [`apply_invoice_payment_succeeded_webhook_event`].
#[derive(Debug, Clone, PartialEq)]
pub enum InvoiceWebhookOutcome {
    /// The invoice carried no `subscription` reference at all.
    NoSubscriptionId,
    /// The invoice's subscription id doesn't match any workspace row — e.g.
    /// a Stripe account shared with another app, or a stale invoice.
    WorkspaceNotFound { subscription_id: String },
    Applied { workspace_id: String, changed: bool },
}

fn invoice_subscription_id(invoice: &Invoice) -> Option<String> {
    match &invoice.subscription {
        Some(Expandable::Id(id)) => Some(id.to_string()),
        Some(Expandable::Object(sub)) => Some(sub.id.to_string()),
        None => None,
    }
}

/// Look up a workspace by `stripe_subscription_id`. Shared by both invoice
/// webhook adapters below — the same lookup
/// `apps/server/src/routes/billing.rs`'s pre-KYO-807
/// `load_workspace_by_subscription` performed inline.
async fn workspace_id_by_subscription(
    db: &DbPool,
    subscription_id: &str,
) -> Result<Option<String>, Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        workspace_id: String,
    }
    let row: Option<Row> = kyomi_core::db_fetch_optional!(
        db,
        Row,
        "SELECT workspace_id FROM workspaces WHERE stripe_subscription_id = $1",
        subscription_id
    )?;
    Ok(row.map(|r| r.workspace_id))
}

/// Apply an `invoice.payment_failed` webhook event: look up the workspace by
/// the invoice's subscription id and mark it `past_due` via
/// [`crate::subscription_service::set_subscription_status`].
pub async fn apply_invoice_payment_failed_webhook_event(
    db: &DbPool,
    ws: &WebSocketManager,
    invoice: &Invoice,
) -> Result<InvoiceWebhookOutcome, Error> {
    let Some(subscription_id) = invoice_subscription_id(invoice) else {
        return Ok(InvoiceWebhookOutcome::NoSubscriptionId);
    };
    let Some(workspace_id) = workspace_id_by_subscription(db, &subscription_id).await? else {
        return Ok(InvoiceWebhookOutcome::WorkspaceNotFound { subscription_id });
    };

    let changed = set_subscription_status(db, ws, &workspace_id, "past_due").await?;

    Ok(InvoiceWebhookOutcome::Applied {
        workspace_id,
        changed,
    })
}

/// Apply an `invoice.payment_succeeded` webhook event: look up the workspace
/// by the invoice's subscription id and reset AI credit usage for the new
/// billing period via
/// [`crate::subscription_service::reset_ai_credits_for_new_period`].
pub async fn apply_invoice_payment_succeeded_webhook_event(
    db: &DbPool,
    ws: &WebSocketManager,
    invoice: &Invoice,
) -> Result<InvoiceWebhookOutcome, Error> {
    let Some(subscription_id) = invoice_subscription_id(invoice) else {
        return Ok(InvoiceWebhookOutcome::NoSubscriptionId);
    };
    let Some(workspace_id) = workspace_id_by_subscription(db, &subscription_id).await? else {
        return Ok(InvoiceWebhookOutcome::WorkspaceNotFound { subscription_id });
    };

    let changed = reset_ai_credits_for_new_period(db, ws, &workspace_id).await?;

    Ok(InvoiceWebhookOutcome::Applied {
        workspace_id,
        changed,
    })
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{seed_membership, seed_user, seed_workspace, sqlite_pool, test_pool};
    use kyomi_core::{MessageType, WebSocketMessage};
    use tokio::sync::mpsc;

    async fn connect_draining_heartbeat(
        manager: &WebSocketManager,
        user_id: &str,
    ) -> mpsc::Receiver<String> {
        let (_conn_id, mut rx) = manager.connect(user_id).expect("connect");
        rx.try_recv()
            .expect("connect() must send an immediate heartbeat");
        rx
    }

    fn expect_billing_status_changed(rx: &mut mpsc::Receiver<String>, workspace_id: &str) {
        let raw = rx
            .try_recv()
            .expect("expected exactly one billing_status_changed message");
        let envelope: WebSocketMessage =
            serde_json::from_str(&raw).expect("valid WebSocketMessage JSON");
        assert_eq!(envelope.message_type, MessageType::BillingStatusChanged);
        let data = envelope
            .data
            .expect("billing_status_changed must carry data");
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

    async fn kv_mcp() -> MCPSessionManager {
        let kv = kyomi_core::kv_store::create_kv_store(None)
            .await
            .expect("in-memory KV store should initialize");
        MCPSessionManager::new(kv)
    }

    fn facts(app: Option<&str>, workspace_id: Option<&str>) -> SubscriptionEventFacts {
        SubscriptionEventFacts {
            app: app.map(str::to_string),
            workspace_id: workspace_id.map(str::to_string),
        }
    }

    // -- resolve_subscription_event_workspace (pure guard) -----------------
    //
    // Real `Subscription`/`Invoice` objects are impractical to construct in
    // a test — see the module doc's *Testability* note — so the guard shared
    // by created/updated/deleted is tested directly against
    // `SubscriptionEventFacts`.

    #[test]
    fn kyomi_app_with_workspace_id_resolves() {
        assert_eq!(
            resolve_subscription_event_workspace(&facts(Some("kyomi"), Some("ws-1"))),
            Ok("ws-1".to_string())
        );
    }

    #[test]
    fn non_kyomi_app_is_refused() {
        assert_eq!(
            resolve_subscription_event_workspace(&facts(Some("some-other-app"), Some("ws-1"))),
            Err(SubscriptionEventGuard::NotKyomiApp)
        );
    }

    #[test]
    fn missing_app_is_refused() {
        assert_eq!(
            resolve_subscription_event_workspace(&facts(None, Some("ws-1"))),
            Err(SubscriptionEventGuard::NotKyomiApp)
        );
    }

    #[test]
    fn missing_workspace_id_is_refused() {
        assert_eq!(
            resolve_subscription_event_workspace(&facts(Some("kyomi"), None)),
            Err(SubscriptionEventGuard::MissingWorkspaceId)
        );
    }

    #[test]
    fn empty_workspace_id_is_refused() {
        assert_eq!(
            resolve_subscription_event_workspace(&facts(Some("kyomi"), Some(""))),
            Err(SubscriptionEventGuard::MissingWorkspaceId)
        );
    }

    // -- apply_subscription_webhook_event (full "Applied" coverage) --------

    fn stripe_service_for_test() -> StripeService {
        StripeService::new("sk_test_fixture", "whsec_test_fixture")
    }

    /// A minimal, realistic `customer.subscription.*` event object — every
    /// field `Subscription`'s `Deserialize` impl requires, with Kyomi's own
    /// `app`/`workspace_id` metadata set. Includes a fully-populated
    /// `Plan`/`Price` on the one subscription item — both are required,
    /// non-`Option` fields — since `StripeService::parse_subscription_data`
    /// reads `items.data.first()` to derive the period dates.
    ///
    /// Built from a raw JSON string (not `serde_json::json!`) — the literal
    /// is large enough that the macro's expansion blows `kyomi-auth`'s
    /// default `serde_json::json!` recursion limit; a plain string avoids
    /// bumping a crate-wide `#![recursion_limit]` for one test fixture.
    ///
    /// This is the same fixture shape `scripts/e2e-regression/billing-live-status.cjs`
    /// (KYO-807) ships as `stripe-fixtures/subscription-updated.json`, kept
    /// in sync by hand — see that script's header comment.
    fn subscription_fixture(subscription_id: &str, workspace_id: &str, app: &str) -> Subscription {
        let json = format!(
            r#"{{
                "id": "{subscription_id}",
                "object": "subscription",
                "application": null,
                "application_fee_percent": null,
                "automatic_tax": {{"enabled": false, "disabled_reason": null, "liability": null}},
                "billing_cycle_anchor": 1735689600,
                "billing_cycle_anchor_config": null,
                "billing_mode": {{"flexible": null, "type": "classic", "updated_at": null}},
                "billing_thresholds": null,
                "cancel_at": null,
                "cancel_at_period_end": false,
                "canceled_at": null,
                "cancellation_details": null,
                "collection_method": "charge_automatically",
                "created": 1735689600,
                "currency": "usd",
                "customer": "cus_test_1",
                "days_until_due": null,
                "default_payment_method": null,
                "default_source": null,
                "default_tax_rates": [],
                "description": null,
                "discount": null,
                "discounts": [],
                "ended_at": null,
                "invoice_settings": {{"account_tax_ids": null, "issuer": {{"type": "self"}}}},
                "items": {{
                    "object": "list",
                    "data": [{{
                        "id": "si_test_1",
                        "object": "subscription_item",
                        "billing_thresholds": null,
                        "created": 1735689600,
                        "current_period_end": 1738368000,
                        "current_period_start": 1735689600,
                        "discounts": [],
                        "metadata": {{}},
                        "plan": {{
                            "id": "plan_test_1",
                            "object": "plan",
                            "active": true,
                            "amount": 500,
                            "amount_decimal": "500",
                            "billing_scheme": "per_unit",
                            "created": 1735689600,
                            "currency": "usd",
                            "interval": "month",
                            "interval_count": 1,
                            "livemode": false,
                            "metadata": {{}},
                            "meter": null,
                            "nickname": null,
                            "product": "prod_test_1",
                            "tiers": null,
                            "tiers_mode": null,
                            "transform_usage": null,
                            "trial_period_days": null,
                            "usage_type": "licensed"
                        }},
                        "price": {{
                            "id": "price_test_1",
                            "object": "price",
                            "active": true,
                            "billing_scheme": "per_unit",
                            "created": 1735689600,
                            "currency": "usd",
                            "currency_options": null,
                            "custom_unit_amount": null,
                            "livemode": false,
                            "lookup_key": null,
                            "metadata": {{}},
                            "nickname": null,
                            "product": "prod_test_1",
                            "recurring": null,
                            "tax_behavior": null,
                            "tiers": null,
                            "tiers_mode": null,
                            "transform_quantity": null,
                            "type": "recurring",
                            "unit_amount": 500,
                            "unit_amount_decimal": "500"
                        }},
                        "quantity": 1,
                        "subscription": "{subscription_id}",
                        "tax_rates": []
                    }}],
                    "has_more": false,
                    "total_count": 1,
                    "url": "/v1/subscription_items"
                }},
                "latest_invoice": null,
                "livemode": false,
                "metadata": {{"app": "{app}", "workspace_id": "{workspace_id}", "billing_cycle": "monthly"}},
                "next_pending_invoice_item_invoice": null,
                "on_behalf_of": null,
                "pause_collection": null,
                "payment_settings": null,
                "pending_invoice_item_interval": null,
                "pending_setup_intent": null,
                "pending_update": null,
                "schedule": null,
                "start_date": 1735689600,
                "status": "active",
                "test_clock": null,
                "transfer_data": null,
                "trial_end": null,
                "trial_settings": null,
                "trial_start": null
            }}"#
        );
        serde_json::from_str(&json).expect("subscription fixture must deserialize")
    }

    #[tokio::test]
    async fn subscription_webhook_event_applies_and_broadcasts_for_kyomi_app() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_membership(sq, "ws-1", "owner", "workspace_admin", true).await;

        let manager = WebSocketManager::new(None, db.clone());
        let mut rx = connect_draining_heartbeat(&manager, "owner").await;
        let mcp = kv_mcp().await;
        let stripe = stripe_service_for_test();
        let subscription = subscription_fixture("sub_test_1", "ws-1", "kyomi");

        let outcome = apply_subscription_webhook_event(
            &db,
            &manager,
            &mcp,
            &stripe,
            &subscription,
            SubscriptionWriteMode::Created,
        )
        .await
        .expect("apply must succeed");

        match outcome {
            SubscriptionWebhookOutcome::Applied {
                workspace_id,
                changed,
                ..
            } => {
                assert_eq!(workspace_id, "ws-1");
                assert!(changed);
            }
            other => panic!("expected Applied, got {other:?}"),
        }
        expect_billing_status_changed(&mut rx, "ws-1");
    }

    #[tokio::test]
    async fn subscription_webhook_event_ignores_non_kyomi_app_and_writes_nothing() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_membership(sq, "ws-1", "owner", "workspace_admin", true).await;

        let manager = WebSocketManager::new(None, db.clone());
        let mut rx = connect_draining_heartbeat(&manager, "owner").await;
        let mcp = kv_mcp().await;
        let stripe = stripe_service_for_test();
        let subscription = subscription_fixture("sub_test_1", "ws-1", "some-other-app");

        let outcome = apply_subscription_webhook_event(
            &db,
            &manager,
            &mcp,
            &stripe,
            &subscription,
            SubscriptionWriteMode::Created,
        )
        .await
        .expect("apply must succeed");

        assert_eq!(outcome, SubscriptionWebhookOutcome::NotKyomiApp);
        assert_no_message(&mut rx, "non-Kyomi subscription event");

        let status: Option<String> = kyomi_core::db_fetch_optional!(
            &db,
            (String,),
            "SELECT subscription_status FROM workspaces WHERE workspace_id = $1",
            "ws-1"
        )
        .expect("query workspace")
        .map(|(s,)| s);
        assert_eq!(
            status.as_deref(),
            Some("active"),
            "non-Kyomi event must not write anything — status stays at the seed default"
        );
    }

    #[tokio::test]
    async fn subscription_webhook_event_missing_workspace_id_writes_nothing() {
        let db = test_pool().await;
        let manager = WebSocketManager::new(None, db.clone());
        let mut rx_dummy = connect_draining_heartbeat(&manager, "nobody").await;
        let mcp = kv_mcp().await;
        let stripe = stripe_service_for_test();
        let subscription = subscription_fixture("sub_test_1", "", "kyomi");

        let outcome = apply_subscription_webhook_event(
            &db,
            &manager,
            &mcp,
            &stripe,
            &subscription,
            SubscriptionWriteMode::Created,
        )
        .await
        .expect("apply must succeed");

        assert_eq!(outcome, SubscriptionWebhookOutcome::MissingWorkspaceId);
        assert_no_message(&mut rx_dummy, "missing workspace_id");
    }

    // -- apply_subscription_deleted_event_facts (fully testable core) -----

    #[tokio::test]
    async fn subscription_deleted_reverts_to_free_and_broadcasts() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_membership(sq, "ws-1", "owner", "workspace_admin", true).await;

        let manager = WebSocketManager::new(None, db.clone());
        let mut rx = connect_draining_heartbeat(&manager, "owner").await;
        let mcp = kv_mcp().await;

        // First move the workspace onto a paid subscription (via the
        // underlying writer directly, since building a `Subscription` to
        // drive `apply_subscription_webhook_event` is impractical — see
        // module doc) so the revert is an actual change.
        write_subscription_to_workspace(
            &db,
            &manager,
            "ws-1",
            &crate::stripe_service::SubscriptionData {
                tier: "cloud".to_string(),
                status: "active".to_string(),
                billing_cycle: Some("monthly".to_string()),
                user_limit: kyomi_core::capability::UNLIMITED_USER_LIMIT,
                period_start: Some(chrono::Utc::now()),
                period_end: Some(chrono::Utc::now() + chrono::Duration::days(30)),
                stripe_subscription_id: "sub_test_1".to_string(),
                stripe_customer_id: "cus_test_1".to_string(),
            },
            SubscriptionWriteMode::Created,
        )
        .await
        .expect("setup write must succeed");
        expect_billing_status_changed(&mut rx, "ws-1");

        let outcome = apply_subscription_deleted_event_facts(
            &db,
            &manager,
            &mcp,
            &facts(Some("kyomi"), Some("ws-1")),
        )
        .await
        .expect("delete apply must succeed");

        match outcome {
            SubscriptionDeletedOutcome::Applied {
                workspace_id,
                changed,
            } => {
                assert_eq!(workspace_id, "ws-1");
                assert!(changed);
            }
            other => panic!("expected Applied, got {other:?}"),
        }
        expect_billing_status_changed(&mut rx, "ws-1");
    }

    #[tokio::test]
    async fn subscription_deleted_no_op_repeat_does_not_broadcast() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_membership(sq, "ws-1", "owner", "workspace_admin", true).await;

        let manager = WebSocketManager::new(None, db.clone());
        let mut rx = connect_draining_heartbeat(&manager, "owner").await;
        let mcp = kv_mcp().await;

        // The seed default is `subscription_status = 'active'`, not
        // `'cancelled'` — the first revert is a real change. Only the
        // *second* revert (already free/cancelled) is the no-op under test.
        let first = apply_subscription_deleted_event_facts(
            &db,
            &manager,
            &mcp,
            &facts(Some("kyomi"), Some("ws-1")),
        )
        .await
        .expect("first delete apply must succeed");
        match first {
            SubscriptionDeletedOutcome::Applied { changed, .. } => assert!(changed),
            other => panic!("expected Applied, got {other:?}"),
        }
        expect_billing_status_changed(&mut rx, "ws-1");

        let second = apply_subscription_deleted_event_facts(
            &db,
            &manager,
            &mcp,
            &facts(Some("kyomi"), Some("ws-1")),
        )
        .await
        .expect("second delete apply must succeed");
        match second {
            SubscriptionDeletedOutcome::Applied { changed, .. } => assert!(!changed),
            other => panic!("expected Applied, got {other:?}"),
        }
        assert_no_message(&mut rx, "no-op revert to free");
    }

    #[tokio::test]
    async fn subscription_deleted_ignores_non_kyomi_app() {
        let db = test_pool().await;
        let manager = WebSocketManager::new(None, db.clone());
        let mcp = kv_mcp().await;

        let outcome = apply_subscription_deleted_event_facts(
            &db,
            &manager,
            &mcp,
            &facts(Some("some-other-app"), Some("ws-1")),
        )
        .await
        .expect("apply must succeed");

        assert_eq!(outcome, SubscriptionDeletedOutcome::NotKyomiApp);
    }

    #[tokio::test]
    async fn subscription_deleted_missing_workspace_id_is_refused() {
        let db = test_pool().await;
        let manager = WebSocketManager::new(None, db.clone());
        let mcp = kv_mcp().await;

        let outcome = apply_subscription_deleted_event_facts(
            &db,
            &manager,
            &mcp,
            &facts(Some("kyomi"), None),
        )
        .await
        .expect("apply must succeed");

        assert_eq!(outcome, SubscriptionDeletedOutcome::MissingWorkspaceId);
    }

    // -- apply_invoice_payment_failed_webhook_event ------------------------

    /// A minimal, realistic `invoice.payment_*` event object. Same rationale
    /// as [`subscription_fixture`] for using a raw JSON string instead of
    /// `serde_json::json!`.
    fn invoice_fixture(invoice_id: &str, subscription_id: Option<&str>) -> Invoice {
        let subscription_json = match subscription_id {
            Some(id) => format!("\"{id}\""),
            None => "null".to_string(),
        };
        let json = format!(
            r#"{{
                "id": "{invoice_id}",
                "object": "invoice",
                "account_country": null,
                "account_name": null,
                "account_tax_ids": null,
                "amount_due": 500,
                "amount_overpaid": 0,
                "amount_paid": 0,
                "amount_remaining": 500,
                "amount_shipping": 0,
                "application": null,
                "attempt_count": 1,
                "attempted": true,
                "auto_advance": false,
                "automatic_tax": {{"enabled": false, "liability": null, "status": null}},
                "billing_reason": "subscription_cycle",
                "collection_method": "charge_automatically",
                "created": 1735689600,
                "currency": "usd",
                "custom_fields": null,
                "customer": "cus_test_1",
                "customer_address": null,
                "customer_email": null,
                "customer_name": null,
                "customer_phone": null,
                "customer_shipping": null,
                "customer_tax_exempt": "none",
                "customer_tax_ids": [],
                "default_payment_method": null,
                "default_source": null,
                "default_tax_rates": [],
                "description": null,
                "discount": null,
                "discounts": [],
                "due_date": null,
                "effective_at": null,
                "ending_balance": 0,
                "footer": null,
                "from_invoice": null,
                "hosted_invoice_url": "https://stripe.example/invoice",
                "invoice_pdf": null,
                "issuer": {{"type": "self"}},
                "last_finalization_error": null,
                "latest_revision": null,
                "lines": {{"object": "list", "data": [], "has_more": false, "total_count": 0, "url": "/v1/invoice_lines"}},
                "livemode": false,
                "metadata": {{}},
                "next_payment_attempt": null,
                "number": null,
                "on_behalf_of": null,
                "paid": false,
                "paid_out_of_band": false,
                "payment_intent": null,
                "payment_settings": {{"default_mandate": null, "payment_method_options": null, "payment_method_types": null}},
                "period_end": 1738368000,
                "period_start": 1735689600,
                "post_payment_credit_notes_amount": 0,
                "pre_payment_credit_notes_amount": 0,
                "quote": null,
                "receipt_number": null,
                "rendering": null,
                "shipping_cost": null,
                "shipping_details": null,
                "starting_balance": 0,
                "statement_descriptor": null,
                "status": "open",
                "status_transitions": {{
                    "finalized_at": null, "marked_uncollectible_at": null,
                    "paid_at": null, "voided_at": null
                }},
                "subscription": {subscription_json},
                "subscription_details": null,
                "subtotal": 500,
                "subtotal_excluding_tax": null,
                "tax": null,
                "test_clock": null,
                "total": 500,
                "total_discount_amounts": [],
                "total_excluding_tax": null,
                "total_tax_amounts": [],
                "transfer_data": null,
                "webhooks_delivered_at": null
            }}"#
        );
        serde_json::from_str(&json).expect("invoice fixture must deserialize")
    }

    #[tokio::test]
    async fn invoice_payment_failed_sets_past_due_and_broadcasts() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_membership(sq, "ws-1", "owner", "workspace_admin", true).await;
        sqlx::query("UPDATE workspaces SET stripe_subscription_id = $1 WHERE workspace_id = $2")
            .bind("sub_test_1")
            .bind("ws-1")
            .execute(sq)
            .await
            .expect("seed stripe_subscription_id");

        let manager = WebSocketManager::new(None, db.clone());
        let mut rx = connect_draining_heartbeat(&manager, "owner").await;

        let outcome = apply_invoice_payment_failed_webhook_event(
            &db,
            &manager,
            &invoice_fixture("in_test_1", Some("sub_test_1")),
        )
        .await
        .expect("apply must succeed");

        match outcome {
            InvoiceWebhookOutcome::Applied {
                workspace_id,
                changed,
            } => {
                assert_eq!(workspace_id, "ws-1");
                assert!(changed);
            }
            other => panic!("expected Applied, got {other:?}"),
        }
        expect_billing_status_changed(&mut rx, "ws-1");
    }

    #[tokio::test]
    async fn invoice_payment_failed_unknown_subscription_writes_nothing() {
        let db = test_pool().await;
        let manager = WebSocketManager::new(None, db.clone());

        let outcome = apply_invoice_payment_failed_webhook_event(
            &db,
            &manager,
            &invoice_fixture("in_test_1", Some("sub_does_not_exist")),
        )
        .await
        .expect("apply must succeed");

        assert_eq!(
            outcome,
            InvoiceWebhookOutcome::WorkspaceNotFound {
                subscription_id: "sub_does_not_exist".to_string()
            }
        );
    }

    #[tokio::test]
    async fn invoice_payment_failed_no_subscription_id_writes_nothing() {
        let db = test_pool().await;
        let manager = WebSocketManager::new(None, db.clone());

        let outcome = apply_invoice_payment_failed_webhook_event(
            &db,
            &manager,
            &invoice_fixture("in_test_1", None),
        )
        .await
        .expect("apply must succeed");

        assert_eq!(outcome, InvoiceWebhookOutcome::NoSubscriptionId);
    }

    // -- apply_invoice_payment_succeeded_webhook_event ---------------------

    #[tokio::test]
    async fn invoice_payment_succeeded_resets_ai_credits_and_broadcasts() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_membership(sq, "ws-1", "owner", "workspace_admin", true).await;
        sqlx::query(
            "UPDATE workspaces SET stripe_subscription_id = $1, ai_credits_used_usd = 4.5 \
             WHERE workspace_id = $2",
        )
        .bind("sub_test_1")
        .bind("ws-1")
        .execute(sq)
        .await
        .expect("seed subscription id + credit usage");

        let manager = WebSocketManager::new(None, db.clone());
        let mut rx = connect_draining_heartbeat(&manager, "owner").await;

        let outcome = apply_invoice_payment_succeeded_webhook_event(
            &db,
            &manager,
            &invoice_fixture("in_test_1", Some("sub_test_1")),
        )
        .await
        .expect("apply must succeed");

        match outcome {
            InvoiceWebhookOutcome::Applied {
                workspace_id,
                changed,
            } => {
                assert_eq!(workspace_id, "ws-1");
                assert!(changed);
            }
            other => panic!("expected Applied, got {other:?}"),
        }
        expect_billing_status_changed(&mut rx, "ws-1");
    }

    #[tokio::test]
    async fn invoice_payment_succeeded_no_op_does_not_broadcast() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_membership(sq, "ws-1", "owner", "workspace_admin", true).await;
        // ai_credits_used_usd defaults to 0.0 already — nothing to reset.
        sqlx::query("UPDATE workspaces SET stripe_subscription_id = $1 WHERE workspace_id = $2")
            .bind("sub_test_1")
            .bind("ws-1")
            .execute(sq)
            .await
            .expect("seed stripe_subscription_id");

        let manager = WebSocketManager::new(None, db.clone());
        let mut rx = connect_draining_heartbeat(&manager, "owner").await;

        let outcome = apply_invoice_payment_succeeded_webhook_event(
            &db,
            &manager,
            &invoice_fixture("in_test_1", Some("sub_test_1")),
        )
        .await
        .expect("apply must succeed");

        match outcome {
            InvoiceWebhookOutcome::Applied { changed, .. } => assert!(!changed),
            other => panic!("expected Applied, got {other:?}"),
        }
        assert_no_message(&mut rx, "no-op AI credit reset");
    }
}
