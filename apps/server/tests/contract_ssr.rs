// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for SSR + hydration on the login page.
//!
//! Verifies the HTTP-level contract: that `/login` returns server-rendered HTML
//! with the right structure for WASM hydration, while non-SSR routes still
//! return the CSR shell.

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

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    format!("http://{addr}")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

// ─── SSR login page ─────────────────────────────────────────────────────────

#[tokio::test]
async fn login_returns_200() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/login"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn login_has_data_ssr_attribute() {
    let base = base_url().await;
    let body = client()
        .get(format!("{base}/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        body.contains("data-ssr"),
        "SSR response must have data-ssr attribute on <body>"
    );
}

#[tokio::test]
async fn login_contains_prerendered_content() {
    let base = base_url().await;
    let body = client()
        .get(format!("{base}/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        body.contains("Welcome back"),
        "SSR response must contain the login heading"
    );
    assert!(
        body.contains(r#"type="email"#),
        "SSR response must contain the email input"
    );
    assert!(
        body.contains(r#"type="password"#),
        "SSR response must contain the password input"
    );
}

#[tokio::test]
async fn login_includes_wasm_loader() {
    let base = base_url().await;
    let body = client()
        .get(format!("{base}/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        body.contains("kyomi-ui-") && body.contains("_bg.wasm"),
        "SSR response must include the WASM loader script from the Trunk template"
    );
}

#[tokio::test]
async fn login_includes_serialized_resources() {
    let base = base_url().await;
    let body = client()
        .get(format!("{base}/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        body.contains("__RESOLVED_RESOURCES"),
        "SSR response must include Leptos serialized resource scripts"
    );
}

#[tokio::test]
async fn login_does_not_contain_loading_spinner() {
    let base = base_url().await;
    let body = client()
        .get(format!("{base}/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        !body.contains(r#"id="kyomi-loading""#),
        "SSR response must NOT contain the CSR loading spinner div"
    );
}

// ─── CSR pages remain unaffected ────────────────────────────────────────────

#[tokio::test]
async fn signup_complete_returns_csr_shell() {
    let base = base_url().await;
    let body = client()
        .get(format!("{base}/signup/complete"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        body.contains(r#"id="kyomi-loading""#),
        "Non-SSR page must contain the CSR loading spinner div"
    );
    assert!(
        !body.contains("<body data-ssr"),
        "Non-SSR page must NOT have data-ssr on the body tag"
    );
    assert!(
        !body.contains("Welcome back"),
        "Non-SSR page must NOT contain pre-rendered page content"
    );
}
