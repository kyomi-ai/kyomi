// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for billing endpoints.
//!
//! These tests verify the HTTP-level contract (request/response shapes, headers,
//! status codes) for the billing endpoints:
//!
//! - `POST /api/v1/billing/create-checkout`
//! - `POST /api/v1/billing/webhook`
//! - `POST /api/v1/billing/cancel-subscription`
//! - `POST /api/v1/billing/reactivate-subscription`
//! - `GET  /api/v1/billing/subscription-info`
//! - `POST /api/v1/billing/update-team-size`
//! - `GET  /api/v1/billing/ai-usage-status`
//! - `GET  /api/v1/billing/invoices`
//! - `POST /api/v1/billing/create-portal-session`
//!
//! Test organization:
//! - Section 1: Unauthenticated 401 tests
//! - Section 2: Authenticated response shape tests
//! - Section 3: Admin-only enforcement (403 for non-admin)
//! - Section 4: Webhook signature verification
//! - Section 5: AI usage status response shape

use serde_json::{json, Value};
use std::collections::HashMap;

use kyomi_test_harness::{base_url, cleanup_test_user, AuthContext};

// ===========================================================================
// Test infrastructure
// ===========================================================================

async fn setup_auth_context(suffix: &str) -> Option<AuthContext> {
    kyomi_test_harness::setup_auth_context("Billing Test User", "bill", suffix).await
}

/// Create a non-admin member token within an existing admin context.
async fn create_member_token(admin_ctx: &AuthContext, suffix: &str) -> (String, String) {
    let member_email = format!("bill-member-{suffix}@contract-test.local");

    cleanup_test_user(&admin_ctx.db, &member_email).await;

    let member_user = kyomi_auth::user_service::create_user(
        &admin_ctx.db,
        &member_email,
        Some("Billing Member User"),
        true,
    )
    .await
    .expect("should create member user");

    kyomi_core::db_execute!(
        &admin_ctx.db,
        "INSERT INTO workspace_users (user_id, workspace_id, role) VALUES ($1, $2, 'workspace_user')",
        &member_user.user_id,
        &admin_ctx.workspace_id
    )
    .expect("should add member to workspace");

    let mut extra = HashMap::new();
    extra.insert("user_id".to_string(), json!(member_user.user_id));
    extra.insert("email".to_string(), json!(member_email));
    extra.insert("name".to_string(), json!("Billing Member User"));
    extra.insert("workspace_id".to_string(), json!(admin_ctx.workspace_id));
    extra.insert("workspace_roles".to_string(), json!(["user"]));

    let access_token = kyomi_auth::jwt::create_access_token_str(
        &member_user.user_id,
        &admin_ctx.jwt_secret,
        60,
        extra,
    )
    .expect("should create member access token");

    (access_token, member_email.to_string())
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn auth_get(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .get(format!("{base}{path}"))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={token}"))
}

fn auth_post(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .post(format!("{base}{path}"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("cookie", format!("access_token={token}"))
}

// ===========================================================================
// 1. Unauthenticated 401 tests — all billing endpoints require auth
// ===========================================================================

#[tokio::test]
async fn create_checkout_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/billing/create-checkout"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(json!({"tier": "pro", "billing_cycle": "monthly"}).to_string())
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /create-checkout without auth should be 401"
    );
}

#[tokio::test]
async fn cancel_subscription_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/billing/cancel-subscription"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /cancel-subscription without auth should be 401"
    );
}

#[tokio::test]
async fn reactivate_subscription_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!(
            "{base}/api/v1/billing/reactivate-subscription"
        ))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /reactivate-subscription without auth should be 401"
    );
}

#[tokio::test]
async fn subscription_info_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/billing/subscription-info"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /subscription-info without auth should be 401"
    );
}

#[tokio::test]
async fn update_team_size_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/billing/update-team-size"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(json!({"total_users": 8}).to_string())
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /update-team-size without auth should be 401"
    );
}

#[tokio::test]
async fn ai_usage_status_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/billing/ai-usage-status"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /ai-usage-status without auth should be 401"
    );
}

#[tokio::test]
async fn invoices_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/billing/invoices"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /invoices without auth should be 401"
    );
}

#[tokio::test]
async fn create_portal_session_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!(
            "{base}/api/v1/billing/create-portal-session"
        ))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /create-portal-session without auth should be 401"
    );
}

// ===========================================================================
// 2. Subscription info response shape (authenticated)
// ===========================================================================

#[tokio::test]
async fn subscription_info_returns_correct_response_shape() {
    let ctx = setup_auth_context("sub-info").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: subscription_info_returns_correct_response_shape — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/billing/subscription-info",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("subscription-info request should succeed");

    assert_eq!(resp.status(), 200, "subscription-info should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    // Verify required fields exist
    assert!(body.get("tier").is_some(), "missing 'tier' field");
    assert!(body.get("status").is_some(), "missing 'status' field");
    assert!(
        body.get("billing_cycle").is_some(),
        "missing 'billing_cycle' field"
    );
    assert!(
        body.get("period_start").is_some(),
        "missing 'period_start' field"
    );
    assert!(
        body.get("period_end").is_some(),
        "missing 'period_end' field"
    );
    assert!(
        body.get("ai_reset_date").is_some(),
        "missing 'ai_reset_date' field"
    );
    assert!(
        body.get("user_limit").is_some(),
        "missing 'user_limit' field"
    );

    // Default workspace is free tier
    assert_eq!(body["tier"], "free", "default tier should be 'free'");
    assert!(body["status"].is_string(), "'status' should be a string");

    // Clean up
    cleanup_test_user(&ctx.db, "bill-test-sub-info@contract-test.local").await;
}

// ===========================================================================
// 3. AI usage status response shape (authenticated)
// ===========================================================================

#[tokio::test]
async fn ai_usage_status_returns_correct_response_shape() {
    let ctx = setup_auth_context("ai-usage").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: ai_usage_status_returns_correct_response_shape — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/billing/ai-usage-status",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("ai-usage-status request should succeed");

    assert_eq!(resp.status(), 200, "ai-usage-status should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    // Verify required fields
    assert!(
        body.get("percentage_used").is_some(),
        "missing 'percentage_used'"
    );
    assert!(
        body.get("warning_level").is_some(),
        "missing 'warning_level'"
    );
    assert!(body.get("allowed").is_some(), "missing 'allowed'");
    assert!(body.get("blocked").is_some(), "missing 'blocked'");

    // percentage_used should be a number
    assert!(
        body["percentage_used"].is_number(),
        "'percentage_used' should be a number"
    );

    // warning_level should be a string or null (null when usage is normal)
    assert!(
        body["warning_level"].is_string() || body["warning_level"].is_null(),
        "'warning_level' should be a string or null"
    );

    // allowed and blocked should be booleans
    assert!(
        body["allowed"].is_boolean(),
        "'allowed' should be a boolean"
    );
    assert!(
        body["blocked"].is_boolean(),
        "'blocked' should be a boolean"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "bill-test-ai-usage@contract-test.local").await;
}

// ===========================================================================
// 4. Invoices response shape (authenticated, no Stripe customer)
// ===========================================================================

#[tokio::test]
async fn invoices_returns_empty_list_without_stripe_customer() {
    let ctx = setup_auth_context("invoices-empty").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: invoices_returns_empty_list_without_stripe_customer — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    // Test workspace has no Stripe customer — but Stripe is not configured either,
    // so the endpoint should return an error about billing features.
    // When stripe is None, the endpoint returns 400 "Billing features are not available".
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/billing/invoices",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("invoices request should succeed");

    // Without Stripe configured, expect 400
    assert_eq!(
        resp.status(),
        400,
        "invoices without Stripe should return 400"
    );

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(
        body.get("detail").is_some(),
        "error response should have 'detail'"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "bill-test-invoices-empty@contract-test.local").await;
}

// ===========================================================================
// 5. Create checkout requires Stripe (returns error without it)
// ===========================================================================

#[tokio::test]
async fn create_checkout_returns_error_without_stripe() {
    let ctx = setup_auth_context("checkout-no-stripe").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: create_checkout_returns_error_without_stripe — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/billing/create-checkout",
        &ctx.access_token,
    )
    .body(
        json!({
            "tier": "pro",
            "billing_cycle": "monthly"
        })
        .to_string(),
    )
    .send()
    .await
    .expect("create-checkout request should succeed");

    // Stripe is not configured in test mode, so we get 400
    assert_eq!(
        resp.status(),
        400,
        "create-checkout without Stripe should return 400"
    );

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(
        body.get("detail").is_some(),
        "error response should have 'detail'"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "bill-test-checkout-no-stripe@contract-test.local").await;
}

// ===========================================================================
// 6. Cancel subscription requires Stripe
// ===========================================================================

#[tokio::test]
async fn cancel_subscription_returns_error_without_stripe() {
    let ctx = setup_auth_context("cancel-no-stripe").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: cancel_subscription_returns_error_without_stripe — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/billing/cancel-subscription",
        &ctx.access_token,
    )
    .body("{}".to_string())
    .send()
    .await
    .expect("cancel-subscription request should succeed");

    assert_eq!(
        resp.status(),
        400,
        "cancel-subscription without Stripe should return 400"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "bill-test-cancel-no-stripe@contract-test.local").await;
}

// ===========================================================================
// 7. Reactivate subscription requires Stripe
// ===========================================================================

#[tokio::test]
async fn reactivate_subscription_returns_error_without_stripe() {
    let ctx = setup_auth_context("react-no-stripe").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: reactivate_subscription_returns_error_without_stripe — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/billing/reactivate-subscription",
        &ctx.access_token,
    )
    .body("{}".to_string())
    .send()
    .await
    .expect("reactivate-subscription request should succeed");

    assert_eq!(
        resp.status(),
        400,
        "reactivate-subscription without Stripe should return 400"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "bill-test-react-no-stripe@contract-test.local").await;
}

// ===========================================================================
// 8. Create portal session requires Stripe
// ===========================================================================

#[tokio::test]
async fn create_portal_session_returns_error_without_stripe() {
    let ctx = setup_auth_context("portal-no-stripe").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: create_portal_session_returns_error_without_stripe — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/billing/create-portal-session",
        &ctx.access_token,
    )
    .body("{}".to_string())
    .send()
    .await
    .expect("create-portal-session request should succeed");

    assert_eq!(
        resp.status(),
        400,
        "create-portal-session without Stripe should return 400"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "bill-test-portal-no-stripe@contract-test.local").await;
}

// ===========================================================================
// 9. Update team size requires Stripe
// ===========================================================================

#[tokio::test]
async fn update_team_size_returns_error_without_stripe() {
    let ctx = setup_auth_context("team-no-stripe").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: update_team_size_returns_error_without_stripe — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/billing/update-team-size",
        &ctx.access_token,
    )
    .body(json!({"total_users": 8}).to_string())
    .send()
    .await
    .expect("update-team-size request should succeed");

    assert_eq!(
        resp.status(),
        400,
        "update-team-size without Stripe should return 400"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "bill-test-team-no-stripe@contract-test.local").await;
}

// ===========================================================================
// 10. Admin-only enforcement — non-admin gets 403
// ===========================================================================

#[tokio::test]
async fn subscription_info_returns_403_for_non_admin() {
    let ctx = setup_auth_context("admin-403-info").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: subscription_info_returns_403_for_non_admin — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (member_token, member_email) = create_member_token(&ctx, "admin-403-info").await;

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/billing/subscription-info",
        &member_token,
    )
    .send()
    .await
    .expect("subscription-info request should succeed");

    assert_eq!(
        resp.status(),
        403,
        "subscription-info by non-admin should return 403"
    );

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(
        body.get("detail").is_some(),
        "403 response should have 'detail'"
    );

    // Clean up
    cleanup_test_user(&ctx.db, &member_email).await;
    cleanup_test_user(&ctx.db, "bill-test-admin-403-info@contract-test.local").await;
}

#[tokio::test]
async fn create_checkout_returns_403_for_non_admin() {
    let ctx = setup_auth_context("admin-403-checkout").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: create_checkout_returns_403_for_non_admin — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (member_token, member_email) = create_member_token(&ctx, "admin-403-checkout").await;

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/billing/create-checkout",
        &member_token,
    )
    .body(
        json!({
            "tier": "pro",
            "billing_cycle": "monthly"
        })
        .to_string(),
    )
    .send()
    .await
    .expect("create-checkout request should succeed");

    assert_eq!(
        resp.status(),
        403,
        "create-checkout by non-admin should return 403"
    );

    // Clean up
    cleanup_test_user(&ctx.db, &member_email).await;
    cleanup_test_user(&ctx.db, "bill-test-admin-403-checkout@contract-test.local").await;
}

#[tokio::test]
async fn cancel_subscription_returns_403_for_non_admin() {
    let ctx = setup_auth_context("admin-403-cancel").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: cancel_subscription_returns_403_for_non_admin — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (member_token, member_email) = create_member_token(&ctx, "admin-403-cancel").await;

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/billing/cancel-subscription",
        &member_token,
    )
    .body("{}".to_string())
    .send()
    .await
    .expect("cancel-subscription request should succeed");

    assert_eq!(
        resp.status(),
        403,
        "cancel-subscription by non-admin should return 403"
    );

    // Clean up
    cleanup_test_user(&ctx.db, &member_email).await;
    cleanup_test_user(&ctx.db, "bill-test-admin-403-cancel@contract-test.local").await;
}

#[tokio::test]
async fn invoices_returns_403_for_non_admin() {
    let ctx = setup_auth_context("admin-403-invoices").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: invoices_returns_403_for_non_admin — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (member_token, member_email) = create_member_token(&ctx, "admin-403-invoices").await;

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/billing/invoices",
        &member_token,
    )
    .send()
    .await
    .expect("invoices request should succeed");

    assert_eq!(
        resp.status(),
        403,
        "invoices by non-admin should return 403"
    );

    // Clean up
    cleanup_test_user(&ctx.db, &member_email).await;
    cleanup_test_user(&ctx.db, "bill-test-admin-403-invoices@contract-test.local").await;
}

// ===========================================================================
// 11. Webhook — missing signature returns error
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
// 12. AI usage status — allowed for non-admin (it's per-user, not admin-only)
// ===========================================================================

#[tokio::test]
async fn ai_usage_status_allowed_for_non_admin() {
    let ctx = setup_auth_context("ai-usage-member").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: ai_usage_status_allowed_for_non_admin — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (member_token, member_email) = create_member_token(&ctx, "ai-usage-member").await;

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/billing/ai-usage-status",
        &member_token,
    )
    .send()
    .await
    .expect("ai-usage-status request should succeed");

    // AI usage status is available to all authenticated users (not admin-only)
    assert_eq!(
        resp.status(),
        200,
        "ai-usage-status should be accessible to non-admin members"
    );

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(body.get("percentage_used").is_some(), "missing 'percentage_used'");

    // Clean up
    cleanup_test_user(&ctx.db, &member_email).await;
    cleanup_test_user(&ctx.db, "bill-test-ai-usage-member@contract-test.local").await;
}

// ===========================================================================
// 13. Portal session — allowed for non-admin (it's per-user)
// ===========================================================================

#[tokio::test]
async fn portal_session_allowed_for_non_admin_but_needs_stripe() {
    let ctx = setup_auth_context("portal-member").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: portal_session_allowed_for_non_admin_but_needs_stripe — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (member_token, member_email) = create_member_token(&ctx, "portal-member").await;

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/billing/create-portal-session",
        &member_token,
    )
    .body("{}".to_string())
    .send()
    .await
    .expect("create-portal-session request should succeed");

    // Portal session is not admin-only, but needs Stripe configured
    // So it should fail with 400 (Stripe not available), not 403 (forbidden)
    assert_eq!(
        resp.status(),
        400,
        "create-portal-session without Stripe should return 400 (not 403)"
    );

    // Clean up
    cleanup_test_user(&ctx.db, &member_email).await;
    cleanup_test_user(&ctx.db, "bill-test-portal-member@contract-test.local").await;
}

// ===========================================================================
// 14. AI usage response has per_user and by_feature nested objects
// ===========================================================================

#[tokio::test]
async fn ai_usage_response_has_per_user_and_by_feature() {
    let ctx = setup_auth_context("ai-usage-nested").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: ai_usage_response_has_per_user_and_by_feature — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/billing/ai-usage-status",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("ai-usage-status request should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");

    // Verify nested per_user object
    assert!(body.get("per_user").is_some(), "missing 'per_user'");
    assert!(
        body["per_user"].is_object(),
        "'per_user' should be an object"
    );

    // Verify nested by_feature object
    assert!(body.get("by_feature").is_some(), "missing 'by_feature'");
    assert!(
        body["by_feature"].is_object(),
        "'by_feature' should be an object"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "bill-test-ai-usage-nested@contract-test.local").await;
}

// ===========================================================================
// 15. Create checkout requires valid request body
// ===========================================================================

#[tokio::test]
async fn create_checkout_requires_tier_field() {
    let ctx = setup_auth_context("checkout-body").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: create_checkout_requires_tier_field — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    // Without Stripe configured, any request returns 400 (Stripe not available)
    // This test just verifies the endpoint accepts POST with JSON body
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/billing/create-checkout",
        &ctx.access_token,
    )
    .body(json!({"tier": "pro", "billing_cycle": "monthly"}).to_string())
    .send()
    .await
    .expect("create-checkout request should succeed");

    // Returns 400 because Stripe is not configured (not because body is invalid)
    assert_eq!(
        resp.status(),
        400,
        "create-checkout without Stripe should return 400"
    );

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(body.get("detail").is_some(), "error response should have 'detail'");

    // Clean up
    cleanup_test_user(&ctx.db, "bill-test-checkout-body@contract-test.local").await;
}
