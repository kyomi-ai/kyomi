// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for Passkey (WebAuthn) endpoints.
//!
//! Tests error paths and basic flow — full WebAuthn flows require a real
//! authenticator (FIDO2 device), but we can verify error responses, validation,
//! rate limiting, and state management.

use serde_json::Value;

/// Get the base URL — either from env (for Python) or start a Rust server.
async fn base_url() -> String {
    if let Ok(url) = std::env::var("CONTRACT_TEST_BASE_URL") {
        return url;
    }

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

    let rp_origin = url::Url::parse(&config.frontend_url).expect("valid URL");
    let webauthn = kyomi_auth::webauthn::build_webauthn(
        &config.webauthn_rp_id,
        &config.webauthn_rp_name,
        &rp_origin,
    )
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

// ─── POST /auth/passkeys/register/start ────────────────────────────────────

#[tokio::test]
async fn register_start_requires_email() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/register/start"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{}"#)
        .send()
        .await
        .unwrap();

    // Missing required field `email`
    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "missing email should return 400 or 422, got: {status}"
    );
}

#[tokio::test]
async fn register_start_new_user_returns_verification_required() {
    let base = base_url().await;

    // Use a unique email to avoid collision with other tests
    let email = format!("passkey-test-{}@example.com", uuid::Uuid::new_v4());

    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/register/start"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(serde_json::json!({
            "email": email,
            "name": "Test User",
        }).to_string())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "new user should return 200");

    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["status"].as_str().unwrap(),
        "verification_required",
        "new user should need verification"
    );
    assert!(
        body["message"].as_str().unwrap().contains("email"),
        "message should mention email"
    );
}

// ─── POST /auth/passkeys/signup/complete ───────────────────────────────────

#[tokio::test]
async fn signup_complete_rejects_unaccepted_terms() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/signup/complete"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"token": "any_token", "name": "Test", "terms_accepted": false}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "terms_accepted=false should be rejected"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body["detail"].as_str().unwrap().contains("Terms of Service"),
        "error should mention Terms of Service"
    );
}

#[tokio::test]
async fn signup_complete_with_invalid_token_returns_400() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/signup/complete"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"token": "nonexistent_token", "name": "Test", "terms_accepted": true}"#)
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
        body["detail"].as_str().unwrap().contains("Invalid or expired"),
        "should mention invalid/expired"
    );
}

#[tokio::test]
async fn signup_complete_requires_token() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/signup/complete"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"name": "Test", "terms_accepted": true}"#)
        .send()
        .await
        .unwrap();

    // Missing required field
    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "missing token should return 400 or 422, got: {status}"
    );
}

// ─── POST /auth/passkeys/register/complete ─────────────────────────────────

#[tokio::test]
async fn register_complete_with_invalid_challenge_returns_400() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/register/complete"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"challenge_id": "nonexistent_challenge", "credential": {"id": "test", "rawId": "test", "response": {"clientDataJSON": "dGVzdA", "attestationObject": "dGVzdA"}, "type": "public-key"}}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "invalid challenge should return 400"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body["detail"].as_str().unwrap().contains("Invalid or expired challenge"),
        "should mention invalid challenge"
    );
}

// ─── POST /auth/passkeys/login/start ───────────────────────────────────────

#[tokio::test]
async fn login_start_returns_challenge_without_email() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/login/start"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "login start should return 200");

    let body: Value = resp.json().await.unwrap();
    assert!(
        body["challenge_id"].is_string(),
        "should return challenge_id"
    );
    assert!(
        body["options"].is_object(),
        "should return options"
    );
    // webauthn-rs serializes as { publicKey: { challenge: "..." } }
    assert!(
        body["options"]["publicKey"]["challenge"].is_string(),
        "options.publicKey should contain challenge"
    );
}

#[tokio::test]
async fn login_start_with_email_returns_challenge() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/login/start"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "nonexistent@example.com"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "login start with email should return 200");

    let body: Value = resp.json().await.unwrap();
    assert!(
        body["challenge_id"].is_string(),
        "should return challenge_id"
    );
    // No credentials found → discoverable flow with empty allowCredentials
    let allow_creds = body["options"]["publicKey"]["allowCredentials"].as_array();
    assert!(
        allow_creds.is_none() || allow_creds.unwrap().is_empty(),
        "unknown user should have empty or no allowCredentials"
    );
}

// ─── POST /auth/passkeys/login/complete ────────────────────────────────────

#[tokio::test]
async fn login_complete_with_invalid_challenge_returns_400() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/login/complete"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"challenge_id": "nonexistent_challenge", "credential": {"id": "test", "rawId": "test", "response": {"clientDataJSON": "dGVzdA", "authenticatorData": "dGVzdA", "signature": "dGVzdA"}, "type": "public-key"}}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "invalid challenge should return 400"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body["detail"].as_str().unwrap().contains("Invalid or expired challenge"),
        "should mention invalid challenge"
    );
}

#[tokio::test]
async fn login_complete_requires_challenge_id() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/login/complete"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"credential": {"id": "test", "rawId": "test", "response": {"clientDataJSON": "dGVzdA", "authenticatorData": "dGVzdA", "signature": "dGVzdA"}, "type": "public-key"}}"#)
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "missing challenge_id should return 400 or 422, got: {status}"
    );
}

// ─── GET /auth/passkeys/list ────────────────────────────────────────────────

#[tokio::test]
async fn passkeys_list_requires_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/auth/passkeys/list"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "passkeys list should require auth");
}

// ─── POST /auth/passkeys/add/start ──────────────────────────────────────────

#[tokio::test]
async fn add_start_requires_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/add/start"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"device_name": "Test Device"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "add/start should require auth");
}

// ─── POST /auth/passkeys/add/complete ───────────────────────────────────────

#[tokio::test]
async fn add_complete_requires_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/add/complete"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"challenge_id": "nonexistent_challenge", "credential": {"id": "test", "rawId": "test", "response": {"clientDataJSON": "dGVzdA", "attestationObject": "dGVzdA"}, "type": "public-key"}}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "add/complete should require auth"
    );
}

// ─── DELETE /auth/passkeys/{credential_id} ──────────────────────────────────

#[tokio::test]
async fn passkey_delete_requires_auth() {
    let base = base_url().await;
    let resp = client()
        .delete(format!("{base}/api/v1/auth/passkeys/some-cred-id"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "passkey delete should require auth");
}

// ─── PATCH /auth/passkeys/{credential_id} ───────────────────────────────────

#[tokio::test]
async fn passkey_rename_requires_auth() {
    let base = base_url().await;
    let resp = client()
        .patch(format!("{base}/api/v1/auth/passkeys/some-cred-id"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"device_name": "New Name"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "passkey rename should require auth");
}

// ─── POST /auth/passkeys/recovery/request ───────────────────────────────────

#[tokio::test]
async fn recovery_request_requires_email() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/recovery/request"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{}"#)
        .send()
        .await
        .unwrap();

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "missing email should return 400 or 422, got: {status}"
    );
}

#[tokio::test]
async fn recovery_request_returns_200_even_for_unknown_email() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/recovery/request"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "nonexistent-recovery@example.com"}"#)
        .send()
        .await
        .unwrap();

    // Always returns 200 to prevent user enumeration
    assert_eq!(resp.status(), 200, "recovery request should always return 200");

    let body: Value = resp.json().await.unwrap();
    assert!(
        body["message"].as_str().is_some(),
        "should return a message"
    );
}

// ─── POST /auth/passkeys/recovery/verify ────────────────────────────────────

#[tokio::test]
async fn recovery_verify_with_invalid_token_returns_400() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/recovery/verify"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"token": "invalid_recovery_token"}"#)
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
        body["detail"].as_str().unwrap().contains("Invalid or expired"),
        "should mention invalid/expired"
    );
}

// ─── POST /auth/passkeys/recovery/register ──────────────────────────────────

#[tokio::test]
async fn recovery_register_requires_recovery_session() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/passkeys/recovery/register"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"challenge_id": "test", "credential": {"id": "test", "rawId": "test", "response": {"clientDataJSON": "dGVzdA", "attestationObject": "dGVzdA"}, "type": "public-key"}}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "recovery register should require recovery session cookie"
    );
}
