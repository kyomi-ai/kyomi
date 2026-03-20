// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for user management endpoints.
//!
//! These tests verify the HTTP-level contract (status codes, auth enforcement)
//! and can run against either backend:
//!
//! - **Rust** (default): spins up the Rust server on a random port
//! - **Python**: set `CONTRACT_TEST_BASE_URL=http://localhost:8002` to test Python

use serde_json::Value;

/// Get the base URL -- either from env (for Python) or start a Rust server.
async fn base_url() -> String {
    if let Ok(url) = std::env::var("CONTRACT_TEST_BASE_URL") {
        return url;
    }

    // Load shared constants (idempotent -- OnceLock ignores second call)
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
        mcp_sessions: kyomi_server::mcp_session_manager::MCPSessionManager::new(kv.clone()),
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

// ---- Admin endpoints: 401 without auth ----

#[tokio::test]
async fn list_users_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/users"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /users without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn create_user_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/users"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "test@example.com", "name": "Test"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /users without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn delete_user_returns_401_without_auth() {
    let base = base_url().await;
    let url = format!("{base}/api/v1/users/user-test123");
    let resp = client()
        .delete(&url)
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "DELETE /users/{{user_id}} without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

// ---- Current user endpoints: 401 without auth ----

#[tokio::test]
async fn update_me_returns_401_without_auth() {
    let base = base_url().await;
    let url = format!("{base}/api/v1/users/me");
    let resp = client()
        .patch(&url)
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"name": "New Name"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "PATCH /users/me without auth should be 401");
}

#[tokio::test]
async fn chartml_config_get_returns_401_without_auth() {
    let base = base_url().await;
    let url = format!("{base}/api/v1/users/me/chartml-config");
    let resp = client()
        .get(&url)
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /users/me/chartml-config without auth should be 401");
}

#[tokio::test]
async fn chartml_config_put_returns_401_without_auth() {
    let base = base_url().await;
    let url = format!("{base}/api/v1/users/me/chartml-config");
    let resp = client()
        .put(&url)
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"config": {}}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "PUT /users/me/chartml-config without auth should be 401");
}

#[tokio::test]
async fn knowledge_get_returns_401_without_auth() {
    let base = base_url().await;
    let url = format!("{base}/api/v1/users/me/knowledge");
    let resp = client()
        .get(&url)
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /users/me/knowledge without auth should be 401");
}

#[tokio::test]
async fn knowledge_put_returns_401_without_auth() {
    let base = base_url().await;
    let url = format!("{base}/api/v1/users/me/knowledge");
    let resp = client()
        .put(&url)
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"knowledge": "test"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "PUT /users/me/knowledge without auth should be 401");
}

#[tokio::test]
async fn preferences_returns_401_without_auth() {
    let base = base_url().await;
    let url = format!("{base}/api/v1/users/me/preferences");
    let resp = client()
        .patch(&url)
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"theme": "dark"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "PATCH /users/me/preferences without auth should be 401");
}

#[tokio::test]
async fn tours_get_returns_401_without_auth() {
    let base = base_url().await;
    let url = format!("{base}/api/v1/users/me/tours");
    let resp = client()
        .get(&url)
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /users/me/tours without auth should be 401");
}

#[tokio::test]
async fn tours_post_returns_401_without_auth() {
    let base = base_url().await;
    let url = format!("{base}/api/v1/users/me/tours/welcome");
    let resp = client()
        .post(&url)
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /users/me/tours/welcome without auth should be 401");
}

// ---- Token endpoints: 401 without auth ----

#[tokio::test]
async fn create_token_returns_401_without_auth() {
    let base = base_url().await;
    let url = format!("{base}/api/v1/users/tokens");
    let resp = client()
        .post(&url)
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"user_email": "test@example.com", "token_name": "test"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /users/tokens without auth should be 401");
}

#[tokio::test]
async fn list_tokens_returns_401_without_auth() {
    let base = base_url().await;
    let url = format!("{base}/api/v1/users/tokens/test@example.com");
    let resp = client()
        .get(&url)
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /users/tokens/email without auth should be 401");
}

#[tokio::test]
async fn revoke_token_returns_401_without_auth() {
    let base = base_url().await;
    let url = format!("{base}/api/v1/users/tokens/tok-test123");
    let resp = client()
        .delete(&url)
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "DELETE /users/tokens/tok-test123 without auth should be 401");
}

// ---- Error response format ----

#[tokio::test]
async fn error_responses_have_detail_field() {
    let base = base_url().await;

    let endpoints = vec![
        ("GET", "/api/v1/users"),
        ("GET", "/api/v1/users/me/chartml-config"),
        ("GET", "/api/v1/users/me/knowledge"),
        ("GET", "/api/v1/users/me/tours"),
    ];

    for (method, path) in endpoints {
        let resp = match method {
            "GET" => client()
                .get(format!("{base}{path}"))
                .header("origin", "http://localhost:5173")
                .send()
                .await
                .unwrap(),
            _ => unreachable!(),
        };

        let status = resp.status();
        assert_eq!(status, 401, "expected 401 for {method} {path}");

        let body: Value = resp.json().await.unwrap();
        assert!(
            body.get("detail").is_some(),
            "{method} {path}: error response must have 'detail' field, got: {body}"
        );
    }
}

// ---- CORS on user endpoints ----

#[tokio::test]
async fn user_endpoints_have_cors_headers() {
    let base = base_url().await;

    // Even 401 responses should have CORS headers
    let resp = client()
        .get(format!("{base}/api/v1/users"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    let acao = resp
        .headers()
        .get("access-control-allow-origin")
        .map(|v| v.to_str().unwrap().to_string());

    assert_eq!(
        acao.as_deref(),
        Some("http://localhost:5173"),
        "CORS should be present on user endpoints"
    );
}

#[tokio::test]
async fn user_endpoints_allow_credentials_cors() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/users"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    let acac = resp
        .headers()
        .get("access-control-allow-credentials")
        .map(|v| v.to_str().unwrap().to_string());

    assert_eq!(
        acac.as_deref(),
        Some("true"),
        "CORS must allow credentials for HTTPOnly cookies"
    );
}
