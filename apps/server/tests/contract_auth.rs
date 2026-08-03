// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for the token-refresh endpoint.
//!
//! Only `POST /api/v1/auth/refresh` remains as an internal REST route after
//! KYO-73 Group 1 — all other auth endpoints are now Leptos server_fns.

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

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    format!("http://{addr}")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

// ─── Token refresh contract tests ────────────────────────────────────────────

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

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
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
