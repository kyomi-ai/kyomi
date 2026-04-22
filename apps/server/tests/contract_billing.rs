// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for the Stripe webhook endpoint.
//!
//! User-facing billing endpoints (checkout, subscription management, invoices,
//! portal) are served by Leptos server_fns and tested there. Only the webhook
//! is tested here because it is the sole REST endpoint remaining in billing.rs.

use serde_json::json;

use kyomi_test_harness::base_url;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
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
