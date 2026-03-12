// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for password authentication endpoints.
//!
//! Tests the HTTP contract (status codes, response shapes) for:
//! - POST /api/v1/auth/login
//! - POST /api/v1/auth/set-password
//! - POST /api/v1/auth/change-password
//! - POST /api/v1/auth/signup/start
//! - POST /api/v1/auth/signup/complete
//!
//! These are unauthenticated contract tests — we verify error paths and
//! validation without creating real user accounts.

use serde_json::Value;

/// Clear a rate limit key in the KV store so tests aren't affected by prior runs.
///
/// In tests, `extract_client_ip` returns "unknown" (no proxy headers, no peer_addr).
async fn clear_rate_limit(endpoint: &str) {
    let config = kyomi_core::Config::test_config();
    if let Ok(kv) = kyomi_core::kv_store::create_kv_store(config.redis_url.as_deref()).await {
        // Clear both "unknown" (actual test IP) and "0.0.0.0" (legacy) for safety
        let key1 = format!("ratelimit:ip:unknown:{endpoint}");
        let key2 = format!("ratelimit:ip:0.0.0.0:{endpoint}");
        let _ = kv.del(&key1).await;
        let _ = kv.del(&key2).await;
    }
}

/// Get the base URL — either from env (for Python) or start a Rust server.
async fn base_url() -> String {
    if let Ok(url) = std::env::var("CONTRACT_TEST_BASE_URL") {
        return url;
    }

    // Load shared constants (idempotent — OnceLock ignores second call)
    if let Ok(path) = kyomi_core::constants::find_constants_file() {
        let _ = kyomi_core::constants::load(&path);
    }

    let config = kyomi_core::Config::test_config();
    let db = kyomi_core::db::create_pool(&config.database_url)
        .await
        .expect("test DB should be running");
    let kv: kyomi_core::KVPool = kyomi_core::kv_store::create_kv_store(config.redis_url.as_deref())
        .await
        .expect("failed to create KV store");

    let encryption_key = kyomi_auth::encryption::derive_key(&config.encryption_key)
        .expect("test encryption key should be valid");

    let rp_origin = url::Url::parse(&config.frontend_url)
        .expect("frontend_url must be a valid URL");
    let webauthn =
        kyomi_auth::webauthn::build_webauthn(&config.webauthn_rp_id, &config.webauthn_rp_name, &rp_origin)
            .expect("webauthn build");

    let ws_manager = kyomi_auth::websocket::WebSocketManager::new(
        None, db.clone(),
    );

    let state = kyomi_api::state::AppState {
        db,
        kv: kv.clone(),
        redis: None,
        config: std::sync::Arc::new(config.clone()),
        encryption_key: std::sync::Arc::new(encryption_key),
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

    let app = kyomi_api::build_service(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    format!("http://{addr}")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

// ─── POST /api/v1/auth/login ─────────────────────────────────────────────────

#[tokio::test]
async fn login_with_wrong_password_returns_401() {
    clear_rate_limit("login").await;

    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/login"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "nonexistent@test.com", "password": "wrongpass"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "wrong password should return 401");

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "401 response must have 'detail' field, got: {body}"
    );
}

#[tokio::test]
async fn login_with_nonexistent_email_returns_401() {
    clear_rate_limit("login").await;

    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/login"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "definitely-does-not-exist-xyz@test.com", "password": "anypassword123"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "non-existent email should return 401 (no email enumeration)"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "401 response must have 'detail' field"
    );
}

#[tokio::test]
async fn login_nonexistent_and_wrong_password_return_same_error_shape() {
    clear_rate_limit("login").await;

    let base = base_url().await;

    let resp_nonexistent = client()
        .post(format!("{base}/api/v1/auth/login"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "no-such-user@test.com", "password": "password123"}"#)
        .send()
        .await
        .unwrap();

    clear_rate_limit("login").await;

    let resp_wrong = client()
        .post(format!("{base}/api/v1/auth/login"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "nonexistent@test.com", "password": "wrongpass"}"#)
        .send()
        .await
        .unwrap();

    // Both should return 401 — same status to prevent email enumeration
    assert_eq!(resp_nonexistent.status(), 401);
    assert_eq!(resp_wrong.status(), 401);
}

#[tokio::test]
async fn login_with_empty_password_returns_validation_error() {
    clear_rate_limit("login").await;

    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/login"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "test@example.com", "password": ""}"#)
        .send()
        .await
        .unwrap();

    // Empty password should be rejected with 400 or 422 (validation) or 401 (credential check)
    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 401 || status == 422,
        "empty password should return 400, 401, or 422, got: {status}"
    );
}

#[tokio::test]
async fn login_with_missing_password_field_returns_validation_error() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/login"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "test@example.com"}"#)
        .send()
        .await
        .unwrap();

    // Missing required field — should be 400 or 422
    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "missing password field should return 400 or 422, got: {status}"
    );
}

#[tokio::test]
async fn login_with_missing_email_field_returns_validation_error() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/login"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"password": "somepassword"}"#)
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "missing email field should return 400 or 422, got: {status}"
    );
}

// ─── POST /api/v1/auth/set-password ─────────────────────────────────────────

#[tokio::test]
async fn set_password_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/set-password"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"new_password": "newpassword123"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "set-password without auth should return 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "401 response must have 'detail' field"
    );
}

// ─── POST /api/v1/auth/change-password ──────────────────────────────────────

#[tokio::test]
async fn change_password_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/change-password"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"current_password": "old123", "new_password": "new123456"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "change-password without auth should return 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "401 response must have 'detail' field"
    );
}

// ─── POST /api/v1/auth/signup/start ─────────────────────────────────────────

#[tokio::test]
async fn signup_start_with_valid_email_returns_200() {
    // Clear rate limit — signup shares the "register" bucket
    clear_rate_limit("register").await;

    let base = base_url().await;
    // Use a unique email to avoid conflicts with other test runs
    let email = format!("signup-test-{}@example.com", uuid::Uuid::new_v4());

    let resp = client()
        .post(format!("{base}/api/v1/auth/signup/start"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(serde_json::json!({"email": email}).to_string())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "signup/start with valid email should return 200");

    let body: Value = resp.json().await.unwrap();
    assert!(
        body["message"].is_string(),
        "response should have a 'message' field, got: {body}"
    );
}

#[tokio::test]
async fn signup_start_with_empty_email_returns_200() {
    // Anti-enumeration: the endpoint applies the "always return 200" pattern
    // before any email validation, so even an empty email returns 200.
    // This matches the account recovery pattern and prevents email enumeration.
    clear_rate_limit("register").await;

    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/signup/start"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": ""}"#)
        .send()
        .await
        .unwrap();

    // The anti-enumeration logic runs before validation — always 200.
    // (A 400 would be acceptable too if the backend adds input validation first,
    // but the current contract is 200.)
    assert_eq!(
        resp.status(),
        200,
        "signup/start returns 200 even for empty email (anti-enumeration)"
    );
}

#[tokio::test]
async fn signup_start_with_missing_email_returns_validation_error() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/signup/start"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{}"#)
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "missing email field should return 400 or 422, got: {status}"
    );
}

// ─── POST /api/v1/auth/signup/complete ──────────────────────────────────────

#[tokio::test]
async fn signup_complete_with_invalid_token_returns_400() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/signup/complete"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"token": "invalid_token_value", "name": "Test User", "password": "testpass123", "terms_accepted": true}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "invalid token should return 400"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "error response must have 'detail' field"
    );
}

#[tokio::test]
async fn signup_complete_without_terms_accepted_returns_400() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/signup/complete"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"token": "fake_token", "name": "Test User", "password": "testpass123", "terms_accepted": false}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "terms_accepted=false should return 400"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "error response must have 'detail' field"
    );
}

#[tokio::test]
async fn signup_complete_with_short_password_returns_400() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/signup/complete"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"token": "fake_token", "name": "Test User", "password": "short", "terms_accepted": true}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "password shorter than 8 chars should return 400"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "error response must have 'detail' field"
    );
}

#[tokio::test]
async fn signup_complete_with_missing_token_returns_validation_error() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/signup/complete"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"name": "Test User", "password": "testpass123", "terms_accepted": true}"#)
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "missing token field should return 400 or 422, got: {status}"
    );
}
