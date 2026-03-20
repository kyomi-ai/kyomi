// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for member management, invitation, and ownership transfer endpoints.
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

// ===========================================================================
// Members — 401 without auth
// ===========================================================================

#[tokio::test]
async fn list_members_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/members"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /workspaces/members without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn update_member_role_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .patch(format!("{base}/api/v1/workspaces/members/test-user/role"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"role": "admin"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "PATCH /workspaces/members/test-user/role without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn remove_member_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .delete(format!("{base}/api/v1/workspaces/members/test-user"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "DELETE /workspaces/members/test-user without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

// ===========================================================================
// Invitations — 401 without auth
// ===========================================================================

#[tokio::test]
async fn create_invitation_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/workspaces/invitations"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"email": "test@example.com", "role": "admin"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /workspaces/invitations without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn list_invitations_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/invitations"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /workspaces/invitations without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn cancel_invitation_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .delete(format!(
            "{base}/api/v1/workspaces/invitations/inv-test123"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "DELETE /workspaces/invitations/inv-test123 without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn pending_invitations_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/invitations/pending"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /workspaces/invitations/pending without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn accept_invitation_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!(
            "{base}/api/v1/workspaces/invitations/inv-test123/accept"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /workspaces/invitations/inv-test123/accept without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn decline_invitation_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!(
            "{base}/api/v1/workspaces/invitations/inv-test123/decline"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /workspaces/invitations/inv-test123/decline without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

// ===========================================================================
// Ownership Transfer — 401 without auth
// ===========================================================================

#[tokio::test]
async fn initiate_transfer_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/workspaces/ownership/transfer"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"to_user_id": "user-test123"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /workspaces/ownership/transfer without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn accept_transfer_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!(
            "{base}/api/v1/workspaces/ownership/transfer/transfer-test123/accept"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /workspaces/ownership/transfer/transfer-test123/accept without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn decline_transfer_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!(
            "{base}/api/v1/workspaces/ownership/transfer/transfer-test123/decline"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /workspaces/ownership/transfer/transfer-test123/decline without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn cancel_transfer_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .delete(format!(
            "{base}/api/v1/workspaces/ownership/transfer/transfer-test123"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "DELETE /workspaces/ownership/transfer/transfer-test123 without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn list_transfers_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/ownership/transfers"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /workspaces/ownership/transfers without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

// ===========================================================================
// Error response format — all 4D endpoints return { "detail": ... }
// ===========================================================================

#[tokio::test]
async fn member_error_responses_have_detail_field() {
    let base = base_url().await;

    let endpoints = vec![
        ("GET", "/api/v1/workspaces/members"),
        ("GET", "/api/v1/workspaces/invitations"),
        ("GET", "/api/v1/workspaces/invitations/pending"),
        ("GET", "/api/v1/workspaces/ownership/transfers"),
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

// ===========================================================================
// CORS — verify headers on member endpoints
// ===========================================================================

#[tokio::test]
async fn member_endpoints_have_cors_headers() {
    let base = base_url().await;

    // Even 401 responses should have CORS headers
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/members"))
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
        "CORS should be present on member endpoints"
    );
}

#[tokio::test]
async fn member_endpoints_allow_credentials_cors() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/workspaces/members"))
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
