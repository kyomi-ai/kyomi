// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for feedback and newsletter subscription endpoints.
//!
//! These tests verify the HTTP-level contract (request/response shapes, headers,
//! status codes) for:
//!
//! ## Feedback endpoints (auth required):
//! - `POST /api/v1/feedback` — Submit feedback
//! - `GET  /api/v1/feedback` — List user's feedback
//!
//! ## Newsletter endpoints (public, no auth):
//! - `POST /api/v1/subscribe` — Subscribe to newsletter
//! - `GET  /api/v1/subscribers/count` — Get subscriber count
//! - `POST /api/v1/unsubscribe` — Unsubscribe from newsletter
//!
//! Test organization:
//! - Section 1: Feedback auth tests
//! - Section 2: Feedback submit + response shape
//! - Section 3: Feedback validation
//! - Section 4: Feedback list
//! - Section 5: Newsletter subscribe + response shape
//! - Section 6: Newsletter subscribe validation
//! - Section 7: Newsletter duplicate subscribe (updates existing)
//! - Section 8: Newsletter unsubscribe
//! - Section 9: Subscriber count
//! - Section 10: Subscribe rate limiting

use serde_json::{json, Value};
use std::collections::HashMap;

// ===========================================================================
// Test infrastructure
// ===========================================================================

struct TestServer {
    base_url: String,
    db: Option<kyomi_core::DbPool>,
    kv: Option<kyomi_core::KVPool>,
    jwt_secret: Option<String>,
    encryption_key: Option<std::sync::Arc<[u8; 32]>>,
}

struct AuthContext {
    base_url: String,
    access_token: String,
    user_id: String,
    workspace_id: String,
    db: kyomi_core::DbPool,
    kv: kyomi_core::KVPool,
    encryption_key: std::sync::Arc<[u8; 32]>,
    jwt_secret: String,
}

async fn setup_server() -> TestServer {
    if let Ok(url) = std::env::var("CONTRACT_TEST_BASE_URL") {
        return TestServer {
            base_url: url,
            db: None,
            kv: None,
            jwt_secret: None,
            encryption_key: None,
        };
    }

    if let Ok(path) = kyomi_core::constants::find_constants_file() {
        let _ = kyomi_core::constants::load(&path);
    }

    let config = kyomi_core::Config::test_config();
    let db = kyomi_core::db::create_pool(&config.database_url)
        .await
        .expect("test DB should be running (docker compose up)");
    let kv: kyomi_core::KVPool = kyomi_core::kv_store::create_kv_store(config.redis_url.as_deref())
        .await
        .expect("failed to create KV store");

    let encryption_key = kyomi_auth::encryption::derive_key(&config.encryption_key)
        .expect("test encryption key should be valid base64url");

    let rp_origin =
        url::Url::parse(&config.frontend_url).expect("frontend_url must be a valid URL");
    let webauthn = kyomi_auth::webauthn::build_webauthn(
        &config.webauthn_rp_id,
        &config.webauthn_rp_name,
        &rp_origin,
    )
    .expect("webauthn build");

    let jwt_secret = config.jwt_secret.clone();
    let encryption_key_arc = std::sync::Arc::new(encryption_key);

    let ws_manager = kyomi_auth::websocket::WebSocketManager::new(
        None, db.clone(),
    );

    let state = kyomi_api::state::AppState {
        db: db.clone(),
        kv: kv.clone(),
        redis: None,
        config: std::sync::Arc::new(config.clone()),
        encryption_key: encryption_key_arc.clone(),
        webauthn: std::sync::Arc::new(webauthn),
        embedding: kyomi_embed::LazyEmbedding::loaded(kyomi_embed::EmbeddingService::new().expect("embedding model")),
        ws_manager,
        stripe: None,
        mcp_sessions: kyomi_api::mcp_session_manager::MCPSessionManager::new(kv.clone()),
        cancel_registry: kyomi_api::cancel_registry::CancelRegistry::default(),
        connect_token: None,
        connect_registry: kyomi_api::connect::registry::ConnectRegistry::new_local(),
        platforms: std::sync::Arc::new(kyomi_core::platform::PlatformRegistry::new()),
    };

    // Ensure the email_subscribers table exists in the test database.
    // This table was created via a standalone SQL migration (not Alembic)
    // in the Python backend, so it may be missing from the test DB.
    ensure_email_subscribers_table(&db).await;

    let app = kyomi_api::build_service(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    TestServer {
        base_url: format!("http://{addr}"),
        db: Some(db),
        kv: Some(kv),
        jwt_secret: Some(jwt_secret),
        encryption_key: Some(encryption_key_arc),
    }
}

/// Ensure the `email_subscribers` table exists in the test database.
///
/// This table was created via a standalone SQL migration in the Python backend
/// (not through Alembic), so it's typically missing from the test database.
async fn ensure_email_subscribers_table(db: &kyomi_core::DbPool) {
    let is_pg = db.is_postgres();
    let serial = if is_pg { "SERIAL" } else { "INTEGER" };
    let bool_default_false = if is_pg { "BOOLEAN DEFAULT FALSE" } else { "INTEGER DEFAULT 0" };
    let timestamp_default = if is_pg { "TIMESTAMP DEFAULT CURRENT_TIMESTAMP" } else { "TEXT DEFAULT (datetime('now'))" };
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
    kyomi_core::db_execute!(db, &sql)
        .expect("should create email_subscribers table");
}

async fn base_url() -> String {
    setup_server().await.base_url
}

async fn setup_auth_context(suffix: &str) -> Option<AuthContext> {
    let server = setup_server().await;
    let db = server.db?;
    let kv = server.kv?;
    let jwt_secret = server
        .jwt_secret
        .expect("jwt_secret should be set in Rust mode");
    let encryption_key = server
        .encryption_key
        .expect("encryption_key should be set in Rust mode");

    let email = format!("fb-test-{suffix}@contract-test.local");

    cleanup_test_user(&db, &email).await;

    let user =
        kyomi_auth::user_service::create_user(&db, &email, Some("Feedback Test User"), true)
            .await
            .expect("should create test user");

    let workspace_id = kyomi_auth::user_service::create_workspace_for_user(
        &db,
        &user.user_id,
        Some("Feedback Test User"),
        &email,
    )
    .await
    .expect("should create test workspace");

    let mut extra = HashMap::new();
    extra.insert("user_id".to_string(), json!(user.user_id));
    extra.insert("email".to_string(), json!(email));
    extra.insert("name".to_string(), json!("Feedback Test User"));
    extra.insert("workspace_id".to_string(), json!(workspace_id));
    extra.insert(
        "workspace_roles".to_string(),
        json!(["workspace_admin"]),
    );

    let access_token = kyomi_auth::jwt::create_access_token_str(
        &user.user_id,
        &jwt_secret,
        60,
        extra,
    )
    .expect("should create access token");

    Some(AuthContext {
        base_url: server.base_url,
        access_token,
        user_id: user.user_id,
        workspace_id,
        db,
        kv,
        encryption_key,
        jwt_secret,
    })
}

async fn cleanup_test_user(db: &kyomi_core::DbPool, email: &str) {
    let user_id: Option<String> = match db {
        kyomi_core::db::DbPool::Postgres(pg) =>
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE email = $1")
                .bind(email).fetch_optional(pg).await.unwrap_or(None),
        kyomi_core::db::DbPool::Sqlite(sq) =>
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE email = $1")
                .bind(email).fetch_optional(sq).await.unwrap_or(None),
    };

    if let Some(uid) = user_id {
        // Delete feedback entries
        let _ = kyomi_core::db_execute!(db, "DELETE FROM feedback WHERE user_id = $1", &uid);

        let workspace_ids: Vec<String> = match db {
            kyomi_core::db::DbPool::Postgres(pg) =>
                sqlx::query_scalar::<_, String>("SELECT workspace_id FROM workspaces WHERE owner_user_id = $1")
                    .bind(&uid).fetch_all(pg).await.unwrap_or_default(),
            kyomi_core::db::DbPool::Sqlite(sq) =>
                sqlx::query_scalar::<_, String>("SELECT workspace_id FROM workspaces WHERE owner_user_id = $1")
                    .bind(&uid).fetch_all(sq).await.unwrap_or_default(),
        };

        for ws_id in &workspace_ids {
            let _ = kyomi_core::db_execute!(db, "DELETE FROM workspace_users WHERE workspace_id = $1", ws_id);

            let _ = kyomi_core::db_execute!(db, "DELETE FROM workspaces WHERE workspace_id = $1", ws_id);
        }

        let _ = kyomi_core::db_execute!(db, "DELETE FROM workspace_users WHERE user_id = $1", &uid);

        let _ = kyomi_core::db_execute!(db, "DELETE FROM users WHERE user_id = $1", &uid);
    }
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
// 1. Feedback auth tests
// ===========================================================================

#[tokio::test]
async fn submit_feedback_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/feedback"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(
            json!({
                "type": "bug",
                "description": "Something is broken in the dashboard"
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /feedback without auth should be 401"
    );
}

#[tokio::test]
async fn list_feedback_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/feedback"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /feedback without auth should be 401"
    );
}

// ===========================================================================
// 2. Feedback submit — response shape
// ===========================================================================

#[tokio::test]
async fn submit_feedback_returns_correct_response_shape() {
    let ctx = setup_auth_context("submit-shape").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: submit_feedback_returns_correct_response_shape — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/feedback",
        &ctx.access_token,
    )
    .body(
        json!({
            "type": "bug",
            "description": "Something is broken in the dashboard and needs fixing"
        })
        .to_string(),
    )
    .send()
    .await
    .expect("submit feedback request should succeed");

    assert_eq!(resp.status(), 200, "submit feedback should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    // Verify required response fields
    assert_eq!(body["status"], "received", "status should be 'received'");
    assert!(
        body.get("feedback_id").is_some(),
        "missing 'feedback_id'"
    );
    assert!(
        body["feedback_id"].as_str().unwrap().starts_with("fb-"),
        "feedback_id should start with 'fb-'"
    );
    assert!(body.get("message").is_some(), "missing 'message'");
    assert!(body["message"].is_string(), "'message' should be a string");

    cleanup_test_user(&ctx.db, "fb-test-submit-shape@contract-test.local").await;
}

// ===========================================================================
// 3. Feedback submit — validation (short description)
// ===========================================================================

#[tokio::test]
async fn submit_feedback_rejects_short_description() {
    let ctx = setup_auth_context("submit-short").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: submit_feedback_rejects_short_description — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/feedback",
        &ctx.access_token,
    )
    .body(
        json!({
            "type": "bug",
            "description": "short"
        })
        .to_string(),
    )
    .send()
    .await
    .expect("submit feedback request should succeed");

    assert_eq!(
        resp.status(),
        400,
        "feedback with short description should return 400"
    );

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(
        body.get("detail").is_some(),
        "400 response should have 'detail'"
    );

    cleanup_test_user(&ctx.db, "fb-test-submit-short@contract-test.local").await;
}

// ===========================================================================
// 4. Feedback submit — invalid type rejected
// ===========================================================================

#[tokio::test]
async fn submit_feedback_rejects_invalid_type() {
    let ctx = setup_auth_context("submit-badtype").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: submit_feedback_rejects_invalid_type — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/feedback",
        &ctx.access_token,
    )
    .body(
        json!({
            "type": "invalid_type",
            "description": "This has an invalid feedback type that should be rejected"
        })
        .to_string(),
    )
    .send()
    .await
    .expect("submit feedback request should succeed");

    assert_eq!(
        resp.status(),
        400,
        "feedback with invalid type should return 400"
    );

    cleanup_test_user(&ctx.db, "fb-test-submit-badtype@contract-test.local").await;
}

// ===========================================================================
// 5. Feedback submit — all valid types accepted
// ===========================================================================

#[tokio::test]
async fn submit_feedback_accepts_all_valid_types() {
    let ctx = setup_auth_context("submit-types").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: submit_feedback_accepts_all_valid_types — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    for fb_type in &["bug", "feature", "question"] {
        let resp = auth_post(
            &ctx.base_url,
            "/api/v1/feedback",
            &ctx.access_token,
        )
        .body(
            json!({
                "type": fb_type,
                "description": format!("Test feedback of type {fb_type} with sufficient length")
            })
            .to_string(),
        )
        .send()
        .await
        .expect("submit feedback request should succeed");

        assert_eq!(
            resp.status(),
            200,
            "feedback type '{fb_type}' should be accepted"
        );
    }

    cleanup_test_user(&ctx.db, "fb-test-submit-types@contract-test.local").await;
}

// ===========================================================================
// 6. Feedback list — response shape
// ===========================================================================

#[tokio::test]
async fn list_feedback_returns_correct_response_shape() {
    let ctx = setup_auth_context("list-shape").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: list_feedback_returns_correct_response_shape — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    // Submit one feedback first
    let _submit = auth_post(
        &ctx.base_url,
        "/api/v1/feedback",
        &ctx.access_token,
    )
    .body(
        json!({
            "type": "feature",
            "description": "This is a feature request for testing the list endpoint"
        })
        .to_string(),
    )
    .send()
    .await
    .expect("submit should succeed");

    // Now list feedback
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/feedback",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("list feedback request should succeed");

    assert_eq!(resp.status(), 200, "list feedback should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    // Python API returns a bare list (no wrapper), so the response is a JSON array
    assert!(
        body.is_array(),
        "response should be a JSON array, got: {body}"
    );

    let feedback_list = body.as_array().unwrap();
    assert!(
        !feedback_list.is_empty(),
        "should have at least one feedback entry"
    );

    // Verify each feedback entry has the correct shape
    let entry = &feedback_list[0];
    assert!(entry.get("id").is_some(), "feedback entry missing 'id'");
    assert!(entry.get("type").is_some(), "feedback entry missing 'type'");
    assert!(
        entry.get("description").is_some(),
        "feedback entry missing 'description'"
    );
    assert!(
        entry.get("status").is_some(),
        "feedback entry missing 'status'"
    );
    assert!(
        entry.get("created_at").is_some(),
        "feedback entry missing 'created_at'"
    );

    cleanup_test_user(&ctx.db, "fb-test-list-shape@contract-test.local").await;
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
// 14. Feedback submit with screenshot stored in context
// ===========================================================================

#[tokio::test]
async fn submit_feedback_with_screenshot_stores_in_context() {
    let ctx = setup_auth_context("submit-screenshot").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: submit_feedback_with_screenshot_stores_in_context — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/feedback",
        &ctx.access_token,
    )
    .body(
        json!({
            "type": "bug",
            "description": "This is a bug report with a screenshot attached for testing",
            "screenshot": "aW1hZ2VfZGF0YV9oZXJl",
            "include_context": true
        })
        .to_string(),
    )
    .send()
    .await
    .expect("submit feedback request should succeed");

    assert_eq!(resp.status(), 200, "feedback with screenshot should return 200");

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(
        body["feedback_id"].as_str().unwrap().starts_with("fb-"),
        "feedback_id should start with 'fb-'"
    );

    cleanup_test_user(&ctx.db, "fb-test-submit-screenshot@contract-test.local").await;
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
// 17. Submit feedback — feedback_id format check
// ===========================================================================

#[tokio::test]
async fn submit_feedback_returns_feedback_id_with_correct_prefix() {
    let ctx = setup_auth_context("fb-id-fmt").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: submit_feedback_returns_feedback_id_with_correct_prefix — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(&ctx.base_url, "/api/v1/feedback", &ctx.access_token)
        .body(
            json!({
                "type": "feature",
                "description": "Verify feedback ID format in contract test"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("submit feedback request should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");
    let feedback_id = body["feedback_id"]
        .as_str()
        .expect("feedback_id should be a string");

    // feedback_id format: fb-{uuid4_hex_first_12_chars}
    assert!(
        feedback_id.starts_with("fb-"),
        "feedback_id should start with 'fb-', got: {feedback_id}"
    );
    assert_eq!(
        feedback_id.len(),
        15,
        "feedback_id should be 15 chars (fb- + 12 hex), got: {feedback_id}"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "fb-test-fb-id-fmt@contract-test.local").await;
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
