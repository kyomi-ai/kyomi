// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for the Stripe webhook endpoint, plus the KYO-806 A6
//! `create_checkout` past_due guard.
//!
//! User-facing billing endpoints (checkout, subscription management, invoices,
//! portal) are served by Leptos server_fns; most are tested at the unit level
//! in `crates/kyomi-ui/src/server_fns/billing.rs` / `mod.rs`. The webhook is
//! tested here because it is the sole REST endpoint remaining in billing.rs.
//! `create_checkout`'s past_due guard is also tested here (over HTTP, via the
//! shared harness) because it runs — and must be provable to run — entirely
//! before `require_stripe`, so it's exercisable without Stripe credentials in
//! this test environment.

use serde_json::json;

use kyomi_test_harness::{base_url, cleanup_test_user, setup_auth_context};

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn server_fn_url<T: leptos::server_fn::ServerFn>(base: &str) -> String {
    format!("{base}{}", T::PATH)
}

// ===========================================================================
// Webhook — missing signature returns error
// ===========================================================================

#[tokio::test]
async fn webhook_missing_signature_returns_400() {
    let base = base_url().await;

    // POST to webhook without Stripe-Signature header
    let resp = client()
        .post(format!("{base}/api/v1/billing/webhook"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(json!({"type": "invoice.payment_succeeded"}).to_string())
        .send()
        .await
        .unwrap();

    // Without Stripe configured, require_stripe returns 400 "Billing features are not available"
    assert_eq!(
        resp.status(),
        400,
        "webhook without Stripe configured should return 400"
    );
}

// ===========================================================================
// create_checkout — KYO-806 A6 past_due guard
// ===========================================================================

#[tokio::test]
async fn create_checkout_refuses_past_due_with_live_subscription_before_any_stripe_call() {
    let ctx = setup_auth_context(
        "Billing Checkout Guard Test User",
        "checkoutguard",
        "past-due",
    )
    .await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: create_checkout_refuses_past_due_with_live_subscription_before_any_stripe_call \
             — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    // past_due WITH a live stripe_subscription_id — the exact
    // double-subscription regression KYO-806 A6 exists to prevent.
    // `setup_auth_context` makes the caller the workspace owner
    // (`create_workspace_for_user`), which is what create_checkout's
    // `require_workspace_owner` gate needs.
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE workspaces SET subscription_status = 'past_due', \
         stripe_subscription_id = 'sub_test_kyo806_guard' WHERE workspace_id = $1",
        &ctx.workspace_id
    )
    .expect("should set past_due with a live subscription id");

    let url = server_fn_url::<kyomi_ui::server_fns::billing::CreateCheckout>(&ctx.base_url);
    let resp = client()
        .post(url)
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .body("quantity=1")
        .send()
        .await
        .expect("create_checkout request should succeed at the transport level");

    // This test environment has no Stripe credentials configured, so a 200
    // (or any response Stripe would have to produce) would mean the guard
    // did not run before require_stripe. The guard sets 409 Conflict
    // explicitly on the refusal path.
    assert_eq!(
        resp.status(),
        409,
        "past_due with a live subscription must be refused by the KYO-806 A6 guard \
         (409 Conflict), before create_checkout ever reaches require_stripe"
    );

    cleanup_test_user(&ctx.db, "checkoutguard-test-past-due@contract-test.local").await;
}
