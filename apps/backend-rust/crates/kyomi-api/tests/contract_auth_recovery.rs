// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for unified account recovery endpoints.
//!
//! Tests the HTTP contract (status codes, response shapes) for:
//! - POST /api/v1/auth/recovery/start
//! - POST /api/v1/auth/recovery/verify
//! - POST /api/v1/auth/recovery/set-password
//!
//! Recovery/start always returns 200 to prevent email enumeration.
//! Recovery/verify and set-password reject invalid tokens/sessions with 400.

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

// ─── POST /api/v1/auth/recovery/start ───────────────────────────────────────

#[tokio::test]
async fn recovery_start_with_unknown_email_returns_200() {
    // Clear rate limit — recovery shares the "register" bucket
    clear_rate_limit("register").await;

    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/recovery/start"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "unknown@example.com"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "recovery start should always return 200 (anti-enumeration)"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body["message"].is_string(),
        "response should have a 'message' field, got: {body}"
    );
}

#[tokio::test]
async fn recovery_start_with_valid_email_format_returns_200() {
    clear_rate_limit("register").await;

    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/recovery/start"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "some-real-looking@company.example.com"}"#)
        .send()
        .await
        .unwrap();

    // Whether user exists or not, should return 200 (anti-enumeration)
    assert_eq!(
        resp.status(),
        200,
        "recovery start always returns 200 regardless of whether user exists"
    );
}

#[tokio::test]
async fn recovery_start_and_unknown_email_return_same_status() {
    // Both known and unknown emails must return 200 to prevent email enumeration.
    // We test with two clearly-unknown emails to verify both return 200.
    clear_rate_limit("register").await;

    let base = base_url().await;

    let resp1 = client()
        .post(format!("{base}/api/v1/auth/recovery/start"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "definitely-not-real-1@test.example"}"#)
        .send()
        .await
        .unwrap();

    clear_rate_limit("register").await;

    let resp2 = client()
        .post(format!("{base}/api/v1/auth/recovery/start"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "definitely-not-real-2@test.example"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp1.status(), 200, "first unknown email should return 200");
    assert_eq!(resp2.status(), 200, "second unknown email should return 200");
}

#[tokio::test]
async fn recovery_start_with_empty_email_returns_200() {
    // Anti-enumeration: the endpoint should return 200 even for empty/invalid
    // email to avoid leaking information. Empty email may alternatively get
    // a 400 if input validation runs before the anti-enumeration logic.
    clear_rate_limit("register").await;

    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/recovery/start"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": ""}"#)
        .send()
        .await
        .unwrap();

    // Accepts 200 (full anti-enumeration) or 400 (validation before anti-enumeration)
    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 400,
        "empty email should return 200 or 400, got: {status}"
    );
}

// ─── POST /api/v1/auth/recovery/verify ──────────────────────────────────────

#[tokio::test]
async fn recovery_verify_with_invalid_token_returns_400() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/recovery/verify"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"token": "invalid_token"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "invalid recovery token should return 400"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "error response must have 'detail' field, got: {body}"
    );
}

#[tokio::test]
async fn recovery_verify_with_random_token_returns_400() {
    let base = base_url().await;
    let fake_token = uuid::Uuid::new_v4().to_string();

    let resp = client()
        .post(format!("{base}/api/v1/auth/recovery/verify"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(serde_json::json!({"token": fake_token}).to_string())
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "random/nonexistent token should return 400"
    );
}

#[tokio::test]
async fn recovery_verify_with_missing_token_returns_validation_error() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/recovery/verify"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{}"#)
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "missing token field should return 400 or 422, got: {status}"
    );
}

// ─── POST /api/v1/auth/recovery/set-password ────────────────────────────────

#[tokio::test]
async fn recovery_set_password_with_invalid_session_returns_400() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/recovery/set-password"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"recovery_session_id": "invalid_session_id", "new_password": "testpass123"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "invalid recovery session ID should return 400"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "error response must have 'detail' field, got: {body}"
    );
}

#[tokio::test]
async fn recovery_set_password_with_short_password_returns_400() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/recovery/set-password"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"recovery_session_id": "fake_session", "new_password": "short"}"#)
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
        "error response must have 'detail' field, got: {body}"
    );
}

#[tokio::test]
async fn recovery_set_password_with_missing_fields_returns_validation_error() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/recovery/set-password"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{}"#)
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "missing fields should return 400 or 422, got: {status}"
    );
}

#[tokio::test]
async fn recovery_set_password_with_random_session_id_returns_400() {
    let base = base_url().await;
    let fake_session = uuid::Uuid::new_v4().to_string();

    let resp = client()
        .post(format!("{base}/api/v1/auth/recovery/set-password"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(serde_json::json!({
            "recovery_session_id": fake_session,
            "new_password": "validpassword123"
        }).to_string())
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "random/nonexistent recovery session ID should return 400"
    );
}
