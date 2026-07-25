// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for newsletter subscription endpoints.
//!
//! These tests verify the HTTP-level contract (request/response shapes, headers,
//! status codes) for:
//!
//! ## Newsletter endpoints (public, no auth):
//! - `POST /api/v1/subscribe` — Subscribe to newsletter
//! - `GET  /api/v1/subscribers/count` — Get subscriber count
//! - `POST /api/v1/unsubscribe` — Unsubscribe from newsletter
//!
//! The `POST`/`GET /api/v1/feedback` REST routes this file used to also cover
//! were deleted in KYO-73 (migrated to Leptos server_fns). Their dead
//! contract tests were removed accordingly — the newsletter endpoints above
//! are a different, still-live feature and are unaffected. The section
//! numbers below are non-contiguous as a result; they were left as-is rather
//! than renumbered to keep this diff reviewable.

use serde_json::{json, Value};

use kyomi_test_harness::{cleanup_test_user, AuthContext, TestServer};

// ===========================================================================
// Test infrastructure
// ===========================================================================

/// Wraps the shared harness to also ensure the `email_subscribers` table
/// exists (it's created via a standalone SQL migration, not Alembic, so the
/// test DB often lacks it).
async fn setup_server() -> TestServer {
    let ts = kyomi_test_harness::setup_server().await;
    if let Some(db) = &ts.db {
        ensure_email_subscribers_table(db).await;
    }
    ts
}

async fn base_url() -> String {
    setup_server().await.base_url
}

async fn setup_auth_context(suffix: &str) -> Option<AuthContext> {
    let ctx =
        kyomi_test_harness::setup_auth_context("Feedback Test User", "fb", suffix).await?;
    ensure_email_subscribers_table(&ctx.db).await;
    Some(ctx)
}

async fn ensure_email_subscribers_table(db: &kyomi_core::DbPool) {
    let is_pg = db.is_postgres();
    let serial = if is_pg { "SERIAL" } else { "INTEGER" };
    let bool_default_false = if is_pg {
        "BOOLEAN DEFAULT FALSE"
    } else {
        "INTEGER DEFAULT 0"
    };
    let timestamp_default = if is_pg {
        "TIMESTAMP DEFAULT CURRENT_TIMESTAMP"
    } else {
        "TEXT DEFAULT (datetime('now'))"
    };
    let timestamp_col = if is_pg { "TIMESTAMP" } else { "TEXT" };
    let sql = format!(
        "CREATE TABLE IF NOT EXISTS email_subscribers (
            id {serial} PRIMARY KEY,
            email VARCHAR(255) UNIQUE NOT NULL,
            company_name VARCHAR(255),
            company_size VARCHAR(50),
            use_case VARCHAR(100),
            marketing_consent {bool_default_false},
            created_at {timestamp_default},
            updated_at {timestamp_default},
            source VARCHAR(50) DEFAULT 'web',
            notified {bool_default_false},
            notified_at {timestamp_col}
        )"
    );
    kyomi_core::db_execute!(db, &sql).expect("should create email_subscribers table");
}

/// Clean up test email subscribers created during tests.
async fn cleanup_test_subscriber(db: &kyomi_core::DbPool, email: &str) {
    let _ = kyomi_core::db_execute!(db, "DELETE FROM email_subscribers WHERE email = $1", email);
}

/// Clean up KV store rate-limit key for a given IP.
async fn cleanup_rate_limit_key(kv: &kyomi_core::KVPool, ip: &str) {
    let key = format!("subscribe:rate:{ip}");
    let _ = kv.del(&key).await;
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

// ===========================================================================
// 7. Newsletter subscribe — response shape
// ===========================================================================

#[tokio::test]
async fn subscribe_returns_correct_response_shape() {
    let ctx = setup_auth_context("sub-shape").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: subscribe_returns_correct_response_shape — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let test_email = "fb-subscribe-shape-test@contract-test.local";

    // Clean up first (subscriber + rate limit key from previous runs)
    cleanup_test_subscriber(&ctx.db, test_email).await;
    cleanup_rate_limit_key(&ctx.kv,"10.0.0.100").await;

    let resp = client()
        .post(format!("{}/api/v1/subscribe", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "10.0.0.100")
        .body(
            json!({
                "email": test_email,
                "marketing_consent": true,
                "source": "test"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("subscribe request should succeed");

    assert_eq!(resp.status(), 200, "subscribe should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    assert!(body.get("message").is_some(), "missing 'message'");
    assert!(body["message"].is_string(), "'message' should be a string");
    assert!(body.get("email").is_some(), "missing 'email'");
    assert_eq!(body["email"], test_email, "email should match");

    // Clean up
    cleanup_test_subscriber(&ctx.db, test_email).await;
    cleanup_rate_limit_key(&ctx.kv,"10.0.0.100").await;
    cleanup_test_user(&ctx.db, "fb-test-sub-shape@contract-test.local").await;
}

// ===========================================================================
// 8. Newsletter subscribe — invalid email rejected
// ===========================================================================

#[tokio::test]
async fn subscribe_rejects_invalid_email() {
    let ctx = setup_server().await;
    let test_ip = "10.0.0.101";

    // Clean up any existing rate limit key from previous runs
    if let Some(ref kv) = ctx.kv {
        cleanup_rate_limit_key(kv, test_ip).await;
    }

    let resp = client()
        .post(format!("{}/api/v1/subscribe", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("x-forwarded-for", test_ip)
        .body(
            json!({
                "email": "not-an-email",
                "marketing_consent": false
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "subscribe with invalid email should return 400"
    );

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(
        body.get("detail").is_some(),
        "400 response should have 'detail'"
    );

    // Clean up rate limit key
    if let Some(ref kv) = ctx.kv {
        cleanup_rate_limit_key(kv, test_ip).await;
    }
}

#[tokio::test]
async fn subscribe_rejects_email_without_domain_dot() {
    let ctx = setup_server().await;
    let test_ip = "10.0.0.102";

    // Clean up any existing rate limit key from previous runs
    if let Some(ref kv) = ctx.kv {
        cleanup_rate_limit_key(kv, test_ip).await;
    }

    let resp = client()
        .post(format!("{}/api/v1/subscribe", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("x-forwarded-for", test_ip)
        .body(
            json!({
                "email": "user@nodot",
                "marketing_consent": false
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "subscribe with email without domain dot should return 400"
    );

    // Clean up rate limit key
    if let Some(ref kv) = ctx.kv {
        cleanup_rate_limit_key(kv, test_ip).await;
    }
}

// ===========================================================================
// 9. Newsletter subscribe — duplicate updates existing
// ===========================================================================

#[tokio::test]
async fn subscribe_duplicate_email_updates_existing() {
    let ctx = setup_auth_context("sub-dup").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: subscribe_duplicate_email_updates_existing — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let test_email = "fb-duplicate-test@contract-test.local";
    cleanup_test_subscriber(&ctx.db, test_email).await;
    cleanup_rate_limit_key(&ctx.kv,"10.0.0.103").await;

    // First subscribe
    let resp1 = client()
        .post(format!("{}/api/v1/subscribe", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "10.0.0.103")
        .body(
            json!({
                "email": test_email,
                "company_name": "First Corp",
                "marketing_consent": false
            })
            .to_string(),
        )
        .send()
        .await
        .expect("first subscribe should succeed");

    assert_eq!(resp1.status(), 200, "first subscribe should return 200");

    // Second subscribe with same email, different data
    let resp2 = client()
        .post(format!("{}/api/v1/subscribe", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "10.0.0.103")
        .body(
            json!({
                "email": test_email,
                "company_name": "Updated Corp",
                "marketing_consent": true
            })
            .to_string(),
        )
        .send()
        .await
        .expect("second subscribe should succeed");

    assert_eq!(
        resp2.status(),
        200,
        "duplicate subscribe should return 200 (updates existing)"
    );

    let body: Value = resp2.json().await.expect("should return JSON");
    assert_eq!(body["email"], test_email);

    // Clean up
    cleanup_test_subscriber(&ctx.db, test_email).await;
    cleanup_rate_limit_key(&ctx.kv,"10.0.0.103").await;
    cleanup_test_user(&ctx.db, "fb-test-sub-dup@contract-test.local").await;
}

// ===========================================================================
// 10. Newsletter unsubscribe — response shape
// ===========================================================================

#[tokio::test]
async fn unsubscribe_returns_correct_response_shape() {
    let base = base_url().await;

    // Unsubscribe always returns success (even if email doesn't exist)
    let resp = client()
        .post(format!("{base}/api/v1/unsubscribe"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(
            json!({
                "email": "nonexistent@contract-test.local"
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "unsubscribe should always return 200");

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(body.get("message").is_some(), "missing 'message'");
    assert!(body["message"].is_string(), "'message' should be a string");
}

// ===========================================================================
// 11. Subscriber count — response shape
// ===========================================================================

#[tokio::test]
async fn subscriber_count_returns_correct_shape() {
    let base = base_url().await;

    let resp = client()
        .get(format!("{base}/api/v1/subscribers/count"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "subscribers/count should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    assert!(body.get("total").is_some(), "missing 'total'");
    assert!(
        body.get("with_marketing_consent").is_some(),
        "missing 'with_marketing_consent'"
    );
    assert!(body["total"].is_number(), "'total' should be a number");
    assert!(
        body["with_marketing_consent"].is_number(),
        "'with_marketing_consent' should be a number"
    );
}

// ===========================================================================
// 12. Subscribe rate limiting
// ===========================================================================

#[tokio::test]
async fn subscribe_rate_limit_returns_429_after_limit() {
    let ctx = setup_auth_context("rate-limit").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: subscribe_rate_limit_returns_429_after_limit — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let test_ip = "10.99.99.1";

    // Clean up any existing rate limit key
    cleanup_rate_limit_key(&ctx.kv,test_ip).await;

    // Send 5 requests (the limit)
    for i in 0..5 {
        let test_email = format!("fb-ratelimit-{i}@contract-test.local");
        cleanup_test_subscriber(&ctx.db, &test_email).await;

        let resp = client()
            .post(format!("{}/api/v1/subscribe", ctx.base_url))
            .header("origin", "http://localhost:5173")
            .header("content-type", "application/json")
            .header("x-forwarded-for", test_ip)
            .body(
                json!({
                    "email": test_email,
                    "marketing_consent": false
                })
                .to_string(),
            )
            .send()
            .await
            .expect("subscribe request should succeed");

        assert_eq!(
            resp.status(),
            200,
            "request {i} should succeed (within limit)"
        );
    }

    // 6th request should be rate-limited
    let resp = client()
        .post(format!("{}/api/v1/subscribe", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("x-forwarded-for", test_ip)
        .body(
            json!({
                "email": "fb-ratelimit-blocked@contract-test.local",
                "marketing_consent": false
            })
            .to_string(),
        )
        .send()
        .await
        .expect("subscribe request should succeed");

    assert_eq!(
        resp.status(),
        429,
        "6th subscribe from same IP should return 429"
    );

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(
        body.get("detail").is_some(),
        "429 response should have 'detail'"
    );

    // Clean up
    for i in 0..5 {
        let test_email = format!("fb-ratelimit-{i}@contract-test.local");
        cleanup_test_subscriber(&ctx.db, &test_email).await;
    }
    cleanup_rate_limit_key(&ctx.kv,test_ip).await;
    cleanup_test_user(&ctx.db, "fb-test-rate-limit@contract-test.local").await;
}

// ===========================================================================
// 13. Subscribe — email normalisation (case insensitive)
// ===========================================================================

#[tokio::test]
async fn subscribe_normalizes_email_to_lowercase() {
    let ctx = setup_auth_context("sub-case").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: subscribe_normalizes_email_to_lowercase — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let test_email = "FB-CASE-TEST@contract-test.local";
    let normalized = "fb-case-test@contract-test.local";
    cleanup_test_subscriber(&ctx.db, normalized).await;
    cleanup_rate_limit_key(&ctx.kv,"10.0.0.110").await;

    let resp = client()
        .post(format!("{}/api/v1/subscribe", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "10.0.0.110")
        .body(
            json!({
                "email": test_email,
                "marketing_consent": true
            })
            .to_string(),
        )
        .send()
        .await
        .expect("subscribe request should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(
        body["email"], normalized,
        "email should be normalized to lowercase"
    );

    cleanup_test_subscriber(&ctx.db, normalized).await;
    cleanup_rate_limit_key(&ctx.kv,"10.0.0.110").await;
    cleanup_test_user(&ctx.db, "fb-test-sub-case@contract-test.local").await;
}

// ===========================================================================
// 15. Subscriber count — no auth required
// ===========================================================================

#[tokio::test]
async fn subscriber_count_does_not_require_auth() {
    let base = base_url().await;

    // This endpoint is public (no auth required)
    let resp = client()
        .get(format!("{base}/api/v1/subscribers/count"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "subscribers/count should not require auth"
    );
}

// ===========================================================================
// 16. Unsubscribe — no auth required
// ===========================================================================

#[tokio::test]
async fn unsubscribe_does_not_require_auth() {
    let base = base_url().await;

    let resp = client()
        .post(format!("{base}/api/v1/unsubscribe"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(json!({"email": "anyone@example.com"}).to_string())
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "unsubscribe should not require auth"
    );
}

// ===========================================================================
// 18. Subscribe — optional fields accepted
// ===========================================================================

#[tokio::test]
async fn subscribe_accepts_optional_company_fields() {
    let ctx = setup_server().await;
    let test_email = "fb-company-fields@contract-test.local";

    if let Some(ref db) = ctx.db {
        cleanup_test_subscriber(db, test_email).await;
    }
    if let Some(ref kv) = ctx.kv {
        cleanup_rate_limit_key(kv, "10.0.0.120").await;
    }

    let resp = client()
        .post(format!("{}/api/v1/subscribe", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "10.0.0.120")
        .body(
            json!({
                "email": test_email,
                "company_name": "Test Corp",
                "company_size": "1-10",
                "use_case": "analytics",
                "marketing_consent": true,
                "source": "test"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("subscribe request should succeed");

    assert_eq!(
        resp.status(),
        200,
        "subscribe with company fields should return 200"
    );

    let body: Value = resp.json().await.expect("should return JSON");
    assert_eq!(body["email"], test_email, "email should match");

    // Clean up
    if let Some(ref db) = ctx.db {
        cleanup_test_subscriber(db, test_email).await;
    }
    if let Some(ref kv) = ctx.kv {
        cleanup_rate_limit_key(kv, "10.0.0.120").await;
    }
}
