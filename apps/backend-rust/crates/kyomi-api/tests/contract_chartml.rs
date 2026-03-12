// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for ChartML validation, Usage, and Feedback endpoints.
//!
//! Verifies HTTP-level contract (status codes, auth enforcement, response format)
//! for endpoints under `/api/v1/chartml`, `/api/v1/usage`, and `/api/v1/feedback`.

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

// ===========================================================================
// ChartML validation endpoints (/api/v1/chartml)
// ===========================================================================

#[tokio::test]
async fn chartml_schema_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/chartml/schema"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

#[tokio::test]
async fn chartml_validate_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/chartml/validate"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"chartml": "data:\n  x: 1\nvisualize:\n  type: bar"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

#[tokio::test]
async fn chartml_validate_markdown_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/chartml/validate-markdown"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"content": "Test markdown content"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

// ===========================================================================
// Usage endpoints (/api/v1/usage)
// ===========================================================================

#[tokio::test]
async fn usage_llm_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/usage/llm"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

#[tokio::test]
async fn usage_llm_with_params_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!(
            "{base}/api/v1/usage/llm?days=7&group_by=model&component=chat_agent"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

// ===========================================================================
// Feedback endpoints (/api/v1/feedback)
// ===========================================================================

#[tokio::test]
async fn submit_feedback_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/feedback"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"type": "bug", "description": "Something is broken in the dashboard"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
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

    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
}

// ===========================================================================
// CORS on all Phase 12-4 endpoints
// ===========================================================================

#[tokio::test]
async fn phase_12_4_endpoints_have_cors_headers() {
    let base = base_url().await;

    let paths = vec![
        "/api/v1/chartml/schema",
        "/api/v1/usage/llm",
        "/api/v1/feedback",
    ];

    for path in paths {
        let resp = client()
            .get(format!("{base}{path}"))
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
            "CORS should be present on {path}"
        );
    }
}

// ===========================================================================
// Error response format validation
// ===========================================================================

#[tokio::test]
async fn phase_12_4_error_responses_have_detail_field() {
    let base = base_url().await;

    let endpoints = vec![
        ("GET", "/api/v1/chartml/schema"),
        ("GET", "/api/v1/usage/llm"),
        ("GET", "/api/v1/feedback"),
    ];

    for (method, path) in endpoints {
        let resp = client()
            .get(format!("{base}{path}"))
            .header("origin", "http://localhost:5173")
            .send()
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            401,
            "expected 401 for {method} {path}"
        );

        let body: Value = resp.json().await.unwrap();
        assert!(
            body.get("detail").is_some(),
            "{method} {path}: error response must have 'detail' field, got: {body}"
        );
    }
}
