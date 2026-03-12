// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for TOTP / 2FA endpoints.
//!
//! Tests the HTTP contract (status codes, response shapes) for:
//! - GET  /api/v1/auth/2fa/status
//! - POST /api/v1/auth/2fa/setup
//! - POST /api/v1/auth/2fa/enable
//! - POST /api/v1/auth/2fa/disable
//!
//! All 2FA endpoints require authentication. These tests verify that the
//! endpoints correctly reject unauthenticated requests with 401.

use serde_json::Value;

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

// ─── GET /api/v1/auth/2fa/status ────────────────────────────────────────────

#[tokio::test]
async fn totp_status_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/auth/2fa/status"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /auth/2fa/status without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "401 response must have 'detail' field, got: {body}"
    );
}

// ─── POST /api/v1/auth/2fa/setup ────────────────────────────────────────────

#[tokio::test]
async fn totp_setup_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/2fa/setup"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /auth/2fa/setup without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "401 response must have 'detail' field, got: {body}"
    );
}

// ─── POST /api/v1/auth/2fa/enable ───────────────────────────────────────────

#[tokio::test]
async fn totp_enable_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/2fa/enable"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"verification_code": "123456"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /auth/2fa/enable without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "401 response must have 'detail' field, got: {body}"
    );
}

// ─── POST /api/v1/auth/2fa/disable ──────────────────────────────────────────

#[tokio::test]
async fn totp_disable_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/2fa/disable"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /auth/2fa/disable without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "401 response must have 'detail' field, got: {body}"
    );
}

// ─── All 2FA endpoints require auth ─────────────────────────────────────────

#[tokio::test]
async fn all_totp_endpoints_require_auth() {
    let base = base_url().await;

    let endpoints: Vec<(&str, &str)> = vec![
        ("GET",  "/api/v1/auth/2fa/status"),
        ("POST", "/api/v1/auth/2fa/setup"),
        ("POST", "/api/v1/auth/2fa/enable"),
        ("POST", "/api/v1/auth/2fa/disable"),
    ];

    for (method, path) in endpoints {
        let resp = match method {
            "GET" => client()
                .get(format!("{base}{path}"))
                .header("origin", "http://localhost:5173")
                .send()
                .await
                .unwrap(),
            "POST" => client()
                .post(format!("{base}{path}"))
                .header("origin", "http://localhost:5173")
                .header("content-type", "application/json")
                .body("{}")
                .send()
                .await
                .unwrap(),
            _ => unreachable!(),
        };

        assert_eq!(
            resp.status(),
            401,
            "{method} {path} should require authentication, got: {}",
            resp.status()
        );
    }
}

// ─── Error response shape ────────────────────────────────────────────────────

#[tokio::test]
async fn totp_endpoints_return_detail_on_401() {
    let base = base_url().await;

    // Verify the detail field is present on all 401 responses
    let resp = client()
        .get(format!("{base}/api/v1/auth/2fa/status"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);

    let body: Value = resp.json().await.unwrap();
    assert!(
        body["detail"].is_string(),
        "detail field must be a string in 401 response, got: {body}"
    );
    assert!(
        !body["detail"].as_str().unwrap().is_empty(),
        "detail message must not be empty"
    );
}
