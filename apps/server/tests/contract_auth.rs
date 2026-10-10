// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for the token-refresh endpoint.
//!
//! Only `POST /api/v1/auth/refresh` remains as an internal REST route after
//! KYO-73 Group 1 — all other auth endpoints are now Leptos server_fns.

use serde_json::Value;

fn load_constants() {
    static LOAD: std::sync::Once = std::sync::Once::new();
    LOAD.call_once(|| {
        kyomi_core::constants::load_with_fallback().expect("test constants");
    });
}

/// Get the base URL — either from env (for Python) or start a Rust server.
async fn base_url() -> String {
    if let Ok(url) = std::env::var("CONTRACT_TEST_BASE_URL") {
        return url;
    }

    let db = kyomi_core::test_db::connect_test_pool()
        .await
        .expect("test DB should be reachable and migratable — see the error for the remedy");
    let (url, _server) = start_server(db).await;
    url
}

/// Exercise the compiled browser route against the supplied real database.
async fn start_server(db: kyomi_core::DbPool) -> (String, tokio::task::JoinHandle<()>) {
    load_constants();
    let config = kyomi_core::Config::test_config();
    let kv: kyomi_core::KVPool = kyomi_core::kv_store::create_kv_store(None)
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
        embedding: kyomi_embed::LazyEmbedding::new(),
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

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://{addr}"), server)
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

struct BrowserContext {
    base: String,
    db: kyomi_core::DbPool,
    user_id: String,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for BrowserContext {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl BrowserContext {
    async fn new(db: kyomi_core::DbPool) -> Self {
        load_constants();
        let email = format!("browser-expiry-{}@example.com", uuid::Uuid::new_v4());
        let user = kyomi_auth::user_service::create_user(&db, &email, Some("Expiry test"), true)
            .await.expect("create browser user");
        let (base, server) = start_server(db.clone()).await;
        Self { base, db, user_id: user.user_id, server }
    }

    async fn token(&self, expires_at: chrono::DateTime<chrono::Utc>) -> (String, String) {
        let raw = kyomi_auth::jwt::create_refresh_token();
        let hash = kyomi_auth::token_service::hash_refresh_token(&raw);
        let family = kyomi_auth::token_service::generate_family_id();
        let device = kyomi_auth::token_service::DeviceInfo {
            user_agent: None, ip_address: None, country_code: None, oauth_client_id: None,
        };
        let id = kyomi_auth::token_service::store_refresh_token(
            &self.db, &self.user_id, &hash, expires_at, &device, &family,
        ).await.expect("store real browser refresh token");
        (raw, id)
    }

    async fn refresh(&self, raw: &str) -> reqwest::Response {
        let cookie_name = &kyomi_core::constants::get().cookies.refresh_token_name;
        client().post(format!("{}/api/v1/auth/refresh", self.base))
            .header("origin", "http://localhost:5173")
            .header("cookie", format!("{cookie_name}={raw}"))
            .send().await.expect("browser refresh request")
    }

    async fn deny(&self, raw: &str) {
        let response = self.refresh(raw).await;
        assert_eq!(response.status(), 401);
        assert!(!response.headers().contains_key("set-cookie"), "denial must not mint cookies");
        let body: Value = response.json().await.expect("denial JSON");
        assert!(body["detail"].is_string());
    }

    async fn rotate(&self, raw: &str) -> String {
        let response = self.refresh(raw).await;
        assert_eq!(response.status(), 200, "future token must refresh a browser session");
        let cookie_name = &kyomi_core::constants::get().cookies.refresh_token_name;
        let access_name = &kyomi_core::constants::get().cookies.access_token_name;
        let cookies: Vec<_> = response.headers().get_all("set-cookie").iter()
            .map(|header| header.to_str().expect("cookie text").to_string()).collect();
        assert!(cookies.iter().any(|cookie| cookie.starts_with(&format!("{access_name}="))));
        // The browser applies the last cookie when middleware and the explicit
        // handler both refresh a request that lacks an access cookie.
        let rotated = cookies.iter().rev().find_map(|cookie| {
            cookie.strip_prefix(&format!("{cookie_name}="))
                .map(|value| value.split(';').next().unwrap().to_string())
        }).expect("rotated refresh cookie");
        assert_ne!(rotated, raw);
        let body: Value = response.json().await.expect("refresh JSON");
        assert_eq!(body["token_type"], "bearer");
        assert_eq!(body["user"]["user_id"], self.user_id);
        assert!(body["access_token"].as_str().is_some_and(|value| !value.is_empty()));
        assert!(body["expires_in"].as_i64().is_some_and(|value| value > 0));
        rotated
    }
}

async fn browser_expiry_contract(db: kyomi_core::DbPool) {
    let ctx = BrowserContext::new(db).await;
    // Same UTC date is essential: the old SQLite comparator admits RFC3339
    // midnight because 'T' sorts after the space in datetime('now').
    let midnight = chrono::Utc::now().date_naive().and_hms_opt(0, 0, 0)
        .unwrap().and_utc();
    let (expired, expired_id) = ctx.token(midnight).await;
    ctx.deny(&expired).await;
    let (untouched,): (bool,) = kyomi_core::db_fetch_one!(
        &ctx.db, (bool,),
        "SELECT last_used IS NULL AND replaced_at IS NULL FROM refresh_tokens WHERE token_id = $1",
        &expired_id
    ).unwrap();
    assert!(untouched, "expired token must not be used or rotated");

    let (boundary, boundary_id) = ctx.token(chrono::Utc::now()).await;
    // Capture the database clock at the expiry boundary in RFC3339 on SQLite.
    // By the HTTP request it is at or before now and must be denied.
    let clock = if ctx.db.is_postgres() { "NOW()" } else { "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')" };
    let sql = format!("UPDATE refresh_tokens SET expires_at = {clock} WHERE token_id = $1");
    kyomi_core::db_execute!(&ctx.db, &sql, &boundary_id).unwrap();
    ctx.deny(&boundary).await;

    let (future, future_id) = ctx.token(chrono::Utc::now() + chrono::Duration::days(1)).await;
    let rotated = ctx.rotate(&future).await;
    let (replaced,): (bool,) = kyomi_core::db_fetch_one!(
        &ctx.db, (bool,), "SELECT replaced_at IS NOT NULL FROM refresh_tokens WHERE token_id = $1", &future_id
    ).unwrap();
    assert!(replaced);
    let next = ctx.rotate(&rotated).await;
    let grace = ctx.rotate(&future).await;
    assert_ne!(grace, next, "a concurrent tab inside grace gets its own token");

    let grace_seconds = kyomi_core::constants::get().jwt.refresh_token_grace_period_seconds;
    let outside_grace = chrono::Utc::now() - chrono::Duration::seconds(grace_seconds + 60);
    kyomi_core::db_execute!(
        &ctx.db, "UPDATE refresh_tokens SET replaced_at = $1 WHERE token_id = $2",
        outside_grace, &future_id
    ).unwrap();
    ctx.deny(&future).await;
    ctx.deny(&next).await;
    ctx.deny(&grace).await;
    let (active_family,): (i64,) = kyomi_core::db_fetch_one!(
        &ctx.db, (i64,),
        "SELECT COUNT(*) FROM refresh_tokens WHERE family_id = (SELECT family_id FROM refresh_tokens WHERE token_id = $1) AND is_active = $2",
        &future_id, true
    ).unwrap();
    assert_eq!(active_family, 0, "theft must revoke the whole family");

    let (revoked, _) = ctx.token(chrono::Utc::now() + chrono::Duration::days(1)).await;
    kyomi_auth::token_service::revoke_all_user_refresh_tokens(&ctx.db, &ctx.user_id)
        .await.expect("revoke browser sessions");
    ctx.deny(&revoked).await;
    let (inactive, _) = ctx.token(chrono::Utc::now() + chrono::Duration::days(1)).await;
    kyomi_core::db_execute!(&ctx.db, "UPDATE users SET active = $1 WHERE user_id = $2", false, &ctx.user_id)
        .unwrap();
    ctx.deny(&inactive).await;
    kyomi_core::db_execute!(&ctx.db, "DELETE FROM refresh_tokens WHERE user_id = $1", &ctx.user_id)
        .unwrap();
    kyomi_core::db_execute!(&ctx.db, "DELETE FROM users WHERE user_id = $1", &ctx.user_id)
        .unwrap();
}

#[tokio::test]
async fn sqlite_browser_refresh_rejects_expiry_and_preserves_rotation_security() {
    browser_expiry_contract(kyomi_core::DbPool::connect("sqlite::memory:").await.unwrap()).await;
}

#[tokio::test]
async fn postgres_browser_refresh_rejects_expiry_and_preserves_rotation_security() {
    let db = kyomi_core::test_db::connect_test_pool().await
        .expect("worktree Postgres contract database must be available");
    assert!(db.is_postgres(), "Postgres contract must exercise Postgres");
    browser_expiry_contract(db).await;
}
