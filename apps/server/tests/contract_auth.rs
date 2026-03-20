// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for authentication endpoints.
//!
//! These tests verify the HTTP-level contract (request/response shapes, headers,
//! status codes) and can run against either backend:
//!
//! - **Rust** (default): spins up the Rust server on a random port
//! - **Python**: set `CONTRACT_TEST_BASE_URL=http://localhost:8002` to test Python
//!
//! Tests that require a database user create one via direct SQL.

use serde_json::Value;

/// Clear a rate limit key in the KV store so tests aren't affected by prior runs.
async fn clear_rate_limit(ip: &str, endpoint: &str) {
    let config = kyomi_core::Config::test_config();
    if let Ok(kv) = kyomi_core::kv_store::create_kv_store(config.redis_url.as_deref()).await {
        let key = format!("ratelimit:ip:{ip}:{endpoint}");
        let _ = kv.del(&key).await;
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

// ─── Unauthenticated endpoint tests ──────────────────────────────────────────
// These tests don't require a database user.

#[tokio::test]
async fn check_token_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/auth/check-token"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "check-token without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn me_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/auth/me"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /me without auth should be 401");
}

#[tokio::test]
async fn profile_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/auth/profile"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn sessions_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/auth/sessions"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn logout_all_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/logout-all"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn refresh_returns_401_without_cookie() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/refresh"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "refresh without cookie should be 401");
}

#[tokio::test]
async fn refresh_returns_401_with_invalid_token() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/refresh"))
        .header("origin", "http://localhost:5173")
        .header("cookie", "refresh_token=rt_invalid_token_value")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "refresh with invalid token should be 401");
}

#[tokio::test]
async fn logout_succeeds_even_without_cookie() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/logout"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    // Logout is always 200 (clears cookies regardless)
    assert_eq!(resp.status(), 200, "logout should always return 200");

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);
    assert_eq!(body["message"], "Logged out successfully");
}

#[tokio::test]
async fn logout_clears_cookies() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/logout"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    // Check that Set-Cookie headers clear both tokens
    let set_cookies: Vec<String> = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .collect();

    let clears_access = set_cookies.iter().any(|c| c.contains("access_token=") && c.contains("Max-Age=0"));
    let clears_refresh = set_cookies.iter().any(|c| c.contains("refresh_token=") && c.contains("Max-Age=0"));

    assert!(clears_access, "logout should clear access_token cookie, got: {set_cookies:?}");
    assert!(clears_refresh, "logout should clear refresh_token cookie, got: {set_cookies:?}");
}

#[tokio::test]
async fn check_email_returns_email_available() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/check-email"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "nonexistent-test@example.com"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["exists"], false);
    assert_eq!(body["email"], "nonexistent-test@example.com");
}

#[tokio::test]
async fn check_email_requires_email() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/check-email"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": ""}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn verify_with_invalid_token_returns_400() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/auth/verify?token=invalid_token_value"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400, "verify with invalid token should be 400");
}

#[tokio::test]
async fn resend_verification_returns_success_message() {
    // Clear the "register" rate limit bucket — other tests (registration,
    // passkey signup) consume tokens from the same IP-based bucket.
    // The fallback IP is "0.0.0.0" when no proxy headers are present.
    clear_rate_limit("0.0.0.0", "register").await;

    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/resend-verification"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "unknown@example.com"}"#)
        .send()
        .await
        .unwrap();

    // Always returns success to prevent email enumeration
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert!(body["message"].is_string(), "should return a message");
}

#[tokio::test]
async fn switch_workspace_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/switch-workspace/ws-test"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn websocket_token_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/auth/websocket-token"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
}

// ─── Error response format tests ────────────────────────────────────────────

#[tokio::test]
async fn error_responses_have_detail_field() {
    let base = base_url().await;

    // Test several unauthenticated endpoints
    let endpoints = vec![
        ("GET", "/api/v1/auth/me"),
        ("GET", "/api/v1/auth/profile"),
        ("GET", "/api/v1/auth/sessions"),
    ];

    for (method, path) in endpoints {
        let resp = match method {
            "GET" => client().get(format!("{base}{path}")).send().await.unwrap(),
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

// ─── Cookie security tests ──────────────────────────────────────────────────

#[tokio::test]
async fn logout_cookies_have_security_attributes() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/auth/logout"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    let set_cookies: Vec<String> = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .collect();

    for cookie in &set_cookies {
        assert!(
            cookie.contains("HttpOnly"),
            "cookie should be HttpOnly: {cookie}"
        );
        assert!(
            cookie.contains("Secure"),
            "cookie should be Secure: {cookie}"
        );
        assert!(
            cookie.contains("SameSite"),
            "cookie should have SameSite: {cookie}"
        );
    }
}

// ─── CORS on auth endpoints ─────────────────────────────────────────────────

#[tokio::test]
async fn auth_endpoints_have_cors_headers() {
    let base = base_url().await;

    // Even 401 responses should have CORS headers (so the browser can read the error)
    let resp = client()
        .get(format!("{base}/api/v1/auth/me"))
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
        "CORS should be present on auth endpoints"
    );
}

#[tokio::test]
async fn auth_endpoints_allow_credentials_cors() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/auth/me"))
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
