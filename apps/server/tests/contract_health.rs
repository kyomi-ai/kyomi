// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for health and root endpoints.
//!
//! These tests verify the HTTP-level contract (request/response shapes, headers,
//! status codes) and can run against either backend:
//!
//! - **Rust** (default): spins up the Rust server on a random port
//! - **Python**: set `CONTRACT_TEST_BASE_URL=http://localhost:8002` to test Python
//!
//! This dual-target design ensures both backends satisfy the same API contract.

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

    // Start the Rust server on a random port with test infrastructure
    let config = kyomi_core::Config::test_config();
    let db = kyomi_core::db::create_pool(&config.database_url)
        .await
        .expect("test DB should be running (docker compose up)");
    let kv: kyomi_core::KVPool = kyomi_core::kv_store::create_kv_store(config.redis_url.as_deref())
        .await
        .expect("failed to create KV store");

    // Derive encryption key at startup (same as main.rs)
    let encryption_key = kyomi_auth::encryption::derive_key(&config.encryption_key)
        .expect("test encryption key should be valid base64url");

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

    // Give the server a moment to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    format!("http://{addr}")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

// ─── Health endpoint ─────────────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_200() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/health"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn health_response_has_required_fields() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/health"))
        .send()
        .await
        .unwrap();

    let body: Value = resp.json().await.unwrap();

    // Must have status, version, and services
    assert!(body.get("status").is_some(), "missing 'status' field");
    assert!(body.get("version").is_some(), "missing 'version' field");
    assert!(body.get("services").is_some(), "missing 'services' field");

    // Status must be a string
    assert!(body["status"].is_string());
    assert!(body["version"].is_string());

    // Services must be an object
    assert!(body["services"].is_object());
}

#[tokio::test]
async fn health_services_has_expected_keys() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/health"))
        .send()
        .await
        .unwrap();

    let body: Value = resp.json().await.unwrap();
    let services = body["services"].as_object().unwrap();

    // Must check database and kv_store at minimum
    assert!(
        services.contains_key("database"),
        "services missing 'database'"
    );
    assert!(services.contains_key("kv_store"), "services missing 'kv_store'");
}

#[tokio::test]
async fn health_status_reflects_service_health() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/health"))
        .send()
        .await
        .unwrap();

    let body: Value = resp.json().await.unwrap();
    let status = body["status"].as_str().unwrap();

    // Status must be one of these two values
    assert!(
        status == "healthy" || status == "degraded",
        "unexpected status: {status}"
    );
}

// ─── Root endpoint ───────────────────────────────────────────────────────────
//
// The root route (`GET /`) now serves the SPA frontend (rust-embed), so it
// returns HTML — not the former JSON API info.  We only assert a 200 status
// and an HTML content-type.

#[tokio::test]
async fn root_returns_200_html() {
    let base = base_url().await;
    let resp = client().get(format!("{base}/")).send().await.unwrap();

    assert_eq!(resp.status(), 200);

    let ct = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(
        ct.contains("text/html"),
        "root should serve HTML (SPA), got content-type: {ct}"
    );
}

// ─── Security headers ────────────────────────────────────────────────────────

#[tokio::test]
async fn has_security_headers() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/health"))
        .send()
        .await
        .unwrap();

    let headers = resp.headers();

    assert_eq!(
        headers.get("x-frame-options").map(|v| v.to_str().unwrap()),
        Some("DENY"),
        "missing or wrong X-Frame-Options"
    );
    assert_eq!(
        headers
            .get("x-content-type-options")
            .map(|v| v.to_str().unwrap()),
        Some("nosniff"),
        "missing or wrong X-Content-Type-Options"
    );
    assert_eq!(
        headers
            .get("x-xss-protection")
            .map(|v| v.to_str().unwrap()),
        Some("1; mode=block"),
        "missing or wrong X-XSS-Protection"
    );
}

#[tokio::test]
async fn has_hsts_header_when_not_demo() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/health"))
        .send()
        .await
        .unwrap();

    let hsts = resp
        .headers()
        .get("strict-transport-security")
        .map(|v| v.to_str().unwrap().to_string());

    // In non-demo mode, HSTS should be present
    // (test config has demo_mode=false)
    assert_eq!(
        hsts.as_deref(),
        Some("max-age=31536000; includeSubDomains"),
        "HSTS header should be set in non-demo mode"
    );
}

// ─── CORS headers ────────────────────────────────────────────────────────────

#[tokio::test]
async fn cors_allows_frontend_origin() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/health"))
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
        "CORS should allow frontend dev origin"
    );
}

#[tokio::test]
async fn cors_allows_credentials() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/health"))
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
        "CORS should allow credentials for HTTPOnly cookies"
    );
}

#[tokio::test]
async fn cors_preflight_returns_allowed_methods() {
    let base = base_url().await;
    let resp = client()
        .request(reqwest::Method::OPTIONS, format!("{base}/api/health"))
        .header("origin", "http://localhost:5173")
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();

    let methods = resp
        .headers()
        .get("access-control-allow-methods")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();

    // Should allow at least GET and POST
    assert!(methods.contains("GET"), "missing GET in allowed methods");
    assert!(methods.contains("POST"), "missing POST in allowed methods");
}

#[tokio::test]
async fn cors_rejects_unknown_origin() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/health"))
        .header("origin", "https://evil.example.com")
        .send()
        .await
        .unwrap();

    // Should NOT have access-control-allow-origin for unknown origins
    let acao = resp.headers().get("access-control-allow-origin");
    assert!(
        acao.is_none(),
        "CORS should not allow unknown origins, got: {:?}",
        acao
    );
}
