// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for Google OAuth endpoints.
//!
//! Tests error paths — we can't test the full flow without a real Google account,
//! but we can verify the error responses, rate limiting, and state management.

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
    // KYO-242: connects to (and provisions/self-heals) this worktree's
    // private test database rather than the shared `kyomi_test` database.
    let db = kyomi_core::test_db::connect_test_pool()
        .await
        .expect("test DB should be reachable and migratable — see the error for the remedy");
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

    let state = kyomi_server::state::AppState {
        db,
        kv: kv.clone(),
        redis: None,
        config: std::sync::Arc::new(config.clone()),
        encryption_key: std::sync::Arc::new(encryption_key),
        webauthn: std::sync::Arc::new(webauthn),
        embedding: kyomi_embed::LazyEmbedding::loaded(kyomi_embed::EmbeddingService::new().expect("embedding model")),
        ws_manager,
        stripe: None,
        mcp_sessions: kyomi_auth::mcp_session_manager::MCPSessionManager::new(kv.clone()),
        cancel_registry: kyomi_server::cancel_registry::CancelRegistry::default(),
        connect_token: None,
        connect_registry: kyomi_server::connect::registry::ConnectRegistry::new_local(),
        platforms: std::sync::Arc::new(kyomi_core::platform::PlatformRegistry::new()),
    };

    let app = kyomi_server::build_service(state);
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

// ─── GET /auth/google/login ────────────────────────────────────────────────

#[tokio::test]
async fn google_login_returns_authorization_url() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/auth/google/login"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "google login should return 200");

    let body: Value = resp.json().await.unwrap();
    let auth_url = body["authorization_url"].as_str().unwrap();
    let state = body["state"].as_str().unwrap();

    assert!(
        auth_url.starts_with("https://accounts.google.com/o/oauth2/auth"),
        "authorization_url should start with Google auth endpoint, got: {auth_url}"
    );
    assert!(
        auth_url.contains("client_id=test-google-client-id"),
        "authorization_url should contain client_id"
    );
    assert!(
        auth_url.contains("userinfo.email"),
        "authorization_url should contain email scope"
    );
    assert!(!state.is_empty(), "state should not be empty");
}

#[tokio::test]
async fn google_login_with_oauth_continue() {
    let base = base_url().await;
    let resp = client()
        .get(format!(
            "{base}/api/v1/auth/google/login?oauth_continue=test_continue_token"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert!(body["authorization_url"].is_string());
    assert!(body["state"].is_string());
}

// ─── POST /auth/google/callback ────────────────────────────────────────────

#[tokio::test]
async fn google_callback_with_invalid_code_returns_400() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/google/callback"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"code": "invalid_code_from_google"}"#)
        .send()
        .await
        .unwrap();

    // Should fail when trying to exchange the invalid code with Google
    assert_eq!(
        resp.status(),
        400,
        "invalid code should return 400"
    );
}

#[tokio::test]
async fn google_callback_requires_code() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/google/callback"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{}"#)
        .send()
        .await
        .unwrap();

    // Should return 422 (missing required field) or 400
    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "missing code should return 400 or 422, got: {status}"
    );
}

// ─── POST /auth/accept-terms ───────────────────────────────────────────────

#[tokio::test]
async fn accept_terms_rejects_unaccepted() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/accept-terms"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"temp_token": "any_token", "accepted": false}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "accepted=false should be rejected"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body["detail"].as_str().unwrap().contains("accept the terms"),
        "error should mention terms acceptance"
    );
}

#[tokio::test]
async fn accept_terms_with_invalid_token_returns_400() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/accept-terms"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"temp_token": "nonexistent_token", "accepted": true}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "invalid temp_token should return 400"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .contains("Invalid or expired"),
        "should mention invalid/expired token"
    );
}

#[tokio::test]
async fn accept_terms_requires_temp_token() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/accept-terms"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"accepted": true}"#)
        .send()
        .await
        .unwrap();

    // Missing required field
    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 422,
        "missing temp_token should return 400 or 422, got: {status}"
    );
}

// ─── Phase 3C: BigQuery endpoints (authenticated) ──────────────────────────

#[tokio::test]
async fn google_oauth_connect_requires_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/auth/google-oauth/connect"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "connect should require auth");
}

#[tokio::test]
async fn google_oauth_disconnect_requires_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/google-oauth/disconnect"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "disconnect should require auth");
}

#[tokio::test]
async fn google_oauth_status_requires_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/auth/google-oauth/status"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "status should require auth");
}

#[tokio::test]
async fn google_oauth_projects_requires_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/auth/google-oauth/projects"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "projects should require auth");
}

#[tokio::test]
async fn google_link_callback_with_invalid_state_returns_400() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/google/link-callback"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"code": "test_code", "state": "invalid_state"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "invalid state should return 400"
    );
}
