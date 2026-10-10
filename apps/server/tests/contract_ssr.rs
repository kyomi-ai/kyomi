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
        schema_drift: kyomi_server::schema_drift::SchemaDriftStatus::default(),
        process_instance: "test-instance:0".to_string(),
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

#[ignore = "Runs in the ssr-contract-tests CI job, which builds crates/kyomi-ui/dist/ first (KYO-255). Ignored by default so the clippy job's `cargo test --tests` sweep — which only mkdirs an empty dist/ — doesn't fail on it. Locally: cd crates/kyomi-ui && trunk build, then cargo test -p kyomi-server --test contract_ssr -- --include-ignored"]
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

#[ignore = "Runs in the ssr-contract-tests CI job, which builds crates/kyomi-ui/dist/ first (KYO-255). Ignored by default so the clippy job's `cargo test --tests` sweep — which only mkdirs an empty dist/ — doesn't fail on it. Locally: cd crates/kyomi-ui && trunk build, then cargo test -p kyomi-server --test contract_ssr -- --include-ignored"]
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

#[ignore = "Runs in the ssr-contract-tests CI job, which builds crates/kyomi-ui/dist/ first (KYO-255). Ignored by default so the clippy job's `cargo test --tests` sweep — which only mkdirs an empty dist/ — doesn't fail on it. Locally: cd crates/kyomi-ui && trunk build, then cargo test -p kyomi-server --test contract_ssr -- --include-ignored"]
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

#[ignore = "Runs in the ssr-contract-tests CI job, which builds crates/kyomi-ui/dist/ first (KYO-255). Ignored by default so the clippy job's `cargo test --tests` sweep — which only mkdirs an empty dist/ — doesn't fail on it. Locally: cd crates/kyomi-ui && trunk build, then cargo test -p kyomi-server --test contract_ssr -- --include-ignored"]
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

#[ignore = "Runs in the ssr-contract-tests CI job, which builds crates/kyomi-ui/dist/ first (KYO-255). Ignored by default so the clippy job's `cargo test --tests` sweep — which only mkdirs an empty dist/ — doesn't fail on it. Locally: cd crates/kyomi-ui && trunk build, then cargo test -p kyomi-server --test contract_ssr -- --include-ignored"]
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

#[ignore = "Runs in the ssr-contract-tests CI job, which builds crates/kyomi-ui/dist/ first (KYO-255). Ignored by default so the clippy job's `cargo test --tests` sweep — which only mkdirs an empty dist/ — doesn't fail on it. Locally: cd crates/kyomi-ui && trunk build, then cargo test -p kyomi-server --test contract_ssr -- --include-ignored"]
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

/// Exercise the two real HTML paths that serve the shared Trunk shell: the
/// static SPA route and the SSR-rendered login route. Every inline script
/// must carry the same nonce authorized by that response's CSP header.
#[ignore = "Requires the real Trunk-generated dist/index.html; run in the ssr-contract-tests CI job with --include-ignored"]
#[tokio::test]
async fn frontend_csp_nonce_matches_shell_and_ssr_inline_scripts() {
    let base = base_url().await;
    let mut nonces = std::collections::HashSet::new();
    for path in ["/signup/complete", "/login", "/signup/complete", "/login"] {
        let response = client()
            .get(format!("{base}{path}"))
            .send()
            .await
            .expect("frontend request should succeed");
        assert_eq!(response.status(), 200, "{path} should serve HTML");

        let csp = response
            .headers()
            .get("content-security-policy")
            .expect("frontend response should include CSP")
            .to_str()
            .expect("CSP should be valid ASCII")
            .to_owned();
        let script_src = csp
            .split(';')
            .find(|directive| directive.trim_start().starts_with("script-src "))
            .expect("frontend CSP should define script-src");
        assert!(!script_src.contains("'unsafe-inline'"), "{path}: {csp}");
        assert!(!script_src.contains("'unsafe-eval'"), "{path}: {csp}");
        assert!(script_src.contains("'wasm-unsafe-eval'"), "{path}: {csp}");
        assert!(
            script_src.contains("https://js.stripe.com"),
            "{path}: {csp}"
        );
        assert!(
            script_src.contains("https://static.cloudflareinsights.com"),
            "{path}: {csp}"
        );
        let style_src = csp
            .split(';')
            .find(|directive| directive.trim_start().starts_with("style-src "))
            .expect("frontend CSP should define style-src");
        assert!(style_src.contains("'unsafe-inline'"), "{path}: {csp}");

        let nonce = script_src
            .split_whitespace()
            .find_map(|source| source.strip_prefix("'nonce-")?.strip_suffix('\''))
            .expect("frontend CSP should include a nonce source");
        assert!(
            nonces.insert(nonce.to_owned()),
            "{path}: reused response nonce"
        );
        let html = response.text().await.expect("frontend body should be text");
        assert!(
            !html.contains("__KYOMI_CSP_NONCE__"),
            "{path}: shell nonce was not substituted"
        );
        if path == "/login" {
            assert!(
                html.contains("<body data-ssr"),
                "login should use the SSR path"
            );
        }

        let mut rest = html.as_str();
        let mut inline_script_count = 0;
        let mut wasm_initializer_count = 0;
        while let Some(start) = rest.find("<script") {
            rest = &rest[start..];
            let end = rest.find('>').expect("script opening tag should close");
            let tag = &rest[..=end];
            if !tag.contains("src=") {
                inline_script_count += 1;
                assert!(
                    tag.contains(&format!("nonce=\"{nonce}\"")),
                    "{path}: inline script did not carry CSP nonce: {tag}"
                );
            }
            let script_end = rest[end + 1..]
                .find("</script>")
                .expect("script element should close")
                + end
                + 1;
            let script = &rest[end + 1..script_end];
            if script.contains("TrunkApplicationStarted") {
                wasm_initializer_count += 1;
                assert!(
                    tag.contains(r#"type="module""#),
                    "{path}: bootstrap must be a module"
                );
                assert!(
                    script.contains("import init"),
                    "{path}: bootstrap must import WASM init"
                );
                assert!(
                    script.contains("kyomi-ui-") && script.contains("_bg.wasm"),
                    "{path}: bootstrap must initialize the actual Trunk WASM artifact"
                );
            }
            rest = &rest[script_end + "</script>".len()..];
        }
        assert_eq!(
            wasm_initializer_count, 1,
            "{path}: expected the actual generated WASM initializer"
        );
        assert!(
            inline_script_count > 0,
            "{path}: expected inline frontend scripts"
        );
    }
}

/// Runtime nonce substitution must not authorize unmarked rendered content.
#[tokio::test]
async fn frontend_csp_does_not_nonce_unmarked_rendered_scripts() {
    use tower::ServiceExt;

    if let Ok(path) = kyomi_core::constants::find_constants_file() {
        let _ = kyomi_core::constants::load(&path);
    }
    let html =
        r#"<script nonce="__KYOMI_CSP_NONCE__">trusted()</script><script>untrusted()</script>"#;
    let app = axum::Router::new()
        .route(
            "/",
            axum::routing::get(move || async move { axum::response::Html(html) }),
        )
        .layer(axum::middleware::from_fn(
            kyomi_server::middleware::security_headers,
        ));
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csp = response
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let nonce = csp
        .split_whitespace()
        .find_map(|source| source.strip_prefix("'nonce-")?.strip_suffix('\''))
        .expect("CSP must authorize a nonce");
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&body).unwrap(),
        format!(r#"<script nonce="{nonce}">trusted()</script><script>untrusted()</script>"#)
    );
}
