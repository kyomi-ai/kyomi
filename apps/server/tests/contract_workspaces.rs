// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for workspace management endpoints.
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

// ---- 401 without auth tests ----

#[tokio::test]
async fn current_workspace_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/current"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /workspaces/current without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn my_workspaces_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/my-workspaces"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /workspaces/my-workspaces without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn settings_get_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/settings"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /workspaces/settings without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn settings_patch_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .patch(format!("{base}/api/v1/workspaces/settings"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"name": "Test"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "PATCH /workspaces/settings without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn default_dashboard_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/default-dashboard"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /workspaces/default-dashboard without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn billing_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/billing"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /workspaces/billing without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn model_settings_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/workspaces/model-settings"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"default_model": "claude-sonnet-4-5-20250929"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /workspaces/model-settings without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn catalog_status_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/catalog/status"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /workspaces/catalog/status without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn onboarding_complete_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/workspaces/onboarding/catalog/complete"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"project_ids": []}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /workspaces/onboarding/catalog/complete without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn workspace_knowledge_get_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/ws-test123/knowledge"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /workspaces/{{workspace_id}}/knowledge without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn workspace_knowledge_put_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .put(format!("{base}/api/v1/workspaces/ws-test123/knowledge"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"knowledge": "test content"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "PUT /workspaces/{{workspace_id}}/knowledge without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn chartml_config_get_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/chartml-config"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /workspaces/chartml-config without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn chartml_config_put_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .put(format!("{base}/api/v1/workspaces/chartml-config"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"config": {}}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "PUT /workspaces/chartml-config without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn microsoft_oauth_get_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/settings/microsoft-oauth"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /workspaces/settings/microsoft-oauth without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn microsoft_oauth_put_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .put(format!("{base}/api/v1/workspaces/settings/microsoft-oauth"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"enabled": false}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "PUT /workspaces/settings/microsoft-oauth without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

// ---- Error response format ----

#[tokio::test]
async fn error_responses_have_detail_field() {
    let base = base_url().await;

    let endpoints = vec![
        ("GET", "/api/v1/workspaces/current"),
        ("GET", "/api/v1/workspaces/my-workspaces"),
        ("GET", "/api/v1/workspaces/settings"),
        ("GET", "/api/v1/workspaces/billing"),
        ("GET", "/api/v1/workspaces/catalog/status"),
        ("GET", "/api/v1/workspaces/ws-test123/knowledge"),
        ("GET", "/api/v1/workspaces/chartml-config"),
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

// ---- CORS on workspace endpoints ----

#[tokio::test]
async fn workspace_endpoints_have_cors_headers() {
    let base = base_url().await;

    // Even 401 responses should have CORS headers
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/current"))
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
        "CORS should be present on workspace endpoints"
    );
}

#[tokio::test]
async fn workspace_endpoints_allow_credentials_cors() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/current"))
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
