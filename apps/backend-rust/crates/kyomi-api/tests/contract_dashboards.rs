// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for dashboard REST endpoints.
//!
//! Verifies HTTP-level contract (status codes, auth enforcement, response format)
//! for all endpoints under `/api/v1/dashboards`.

use serde_json::Value;

/// Get the base URL -- either from env (for Python) or start a Rust server.
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

    let rp_origin =
        url::Url::parse(&config.frontend_url).expect("frontend_url must be a valid URL");
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

// ---- Auth enforcement: all dashboard endpoints require authentication ----

#[tokio::test]
async fn create_dashboard_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/dashboards"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"title": "Test", "content": "Hello"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

#[tokio::test]
async fn list_dashboards_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/dashboards"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

#[tokio::test]
async fn get_dashboard_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/dashboards/dash-test123"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

#[tokio::test]
async fn update_dashboard_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .patch(format!("{base}/api/v1/dashboards/dash-test123"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"title": "Updated"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

#[tokio::test]
async fn delete_dashboard_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .delete(format!("{base}/api/v1/dashboards/dash-test123"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

#[tokio::test]
async fn list_versions_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/dashboards/dash-test123/versions"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

#[tokio::test]
async fn get_version_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!(
            "{base}/api/v1/dashboards/dash-test123/versions/1"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

#[tokio::test]
async fn diff_versions_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!(
            "{base}/api/v1/dashboards/dash-test123/versions/diff?from_version=1&to_version=2"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

#[tokio::test]
async fn restore_version_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!(
            "{base}/api/v1/dashboards/dash-test123/versions/1/restore"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

// ---- CORS on dashboard endpoints ----

#[tokio::test]
async fn dashboard_endpoints_have_cors_headers() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/dashboards"))
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
        "CORS should be present on dashboard endpoints"
    );
}

// ---- Error response format ----

#[tokio::test]
async fn dashboard_error_responses_have_detail_field() {
    let base = base_url().await;

    let endpoints = vec![
        ("GET", "/api/v1/dashboards"),
        ("GET", "/api/v1/dashboards/dash-nonexistent"),
        ("DELETE", "/api/v1/dashboards/dash-nonexistent"),
        ("GET", "/api/v1/dashboards/dash-nonexistent/versions"),
        ("GET", "/api/v1/dashboards/dash-nonexistent/versions/1"),
    ];

    for (method, path) in endpoints {
        let resp = match method {
            "GET" => client()
                .get(format!("{base}{path}"))
                .header("origin", "http://localhost:5173")
                .send()
                .await
                .unwrap(),
            "DELETE" => client()
                .delete(format!("{base}{path}"))
                .header("origin", "http://localhost:5173")
                .send()
                .await
                .unwrap(),
            _ => unreachable!(),
        };

        assert_eq!(resp.status(), 401, "expected 401 for {method} {path}");

        let body: Value = resp.json().await.unwrap();
        assert!(
            body.get("detail").is_some(),
            "{method} {path}: error response must have 'detail' field, got: {body}"
        );
    }
}
