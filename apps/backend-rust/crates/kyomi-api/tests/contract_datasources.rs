// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for datasource management endpoints.
//!
//! These tests verify the HTTP-level contract (request/response shapes, headers,
//! status codes) and can run against either backend:
//!
//! - **Rust** (default): spins up the Rust server on a random port
//! - **Python**: set `CONTRACT_TEST_BASE_URL=http://localhost:8002` to test Python
//!
//! Tests are organized into sections:
//! 1. Unauthenticated 401 tests (no auth required)
//! 2. Types endpoint tests (no auth required for shape checks)
//! 3. Authenticated CRUD tests (require DB access for user setup)
//! 4. Credential tests
//! 5. Settings tests
//! 6. Toggle tests
//! 7. Credential status tests
//! 8. Test connection tests
//! 9. SSH key generation tests
//! 10. Affected users tests
//! 11. Sample datasource tests
//!
//! Authenticated tests create temporary users/workspaces in the DB and clean
//! up after themselves. They only run in Rust-backend mode (when
//! `CONTRACT_TEST_BASE_URL` is NOT set) because they need direct DB access
//! for test setup.

use serde_json::{json, Value};
use std::collections::HashMap;

// ===========================================================================
// Test infrastructure — server + client setup
// ===========================================================================

/// Shared server state for authenticated tests. The DB pool is only available
/// when running against the Rust backend (not when targeting Python via env var).
struct TestServer {
    base_url: String,
    db: Option<kyomi_core::DbPool>,
    jwt_secret: Option<String>,
    encryption_key: Option<std::sync::Arc<[u8; 32]>>,
}

/// An authenticated test context with a user, workspace, and JWT.
struct AuthContext {
    base_url: String,
    access_token: String,
    user_id: String,
    workspace_id: String,
    db: kyomi_core::DbPool,
    /// Available for tests that need to verify encrypted credential storage.
    encryption_key: std::sync::Arc<[u8; 32]>,
    /// JWT secret for minting additional tokens (e.g., non-admin member tokens).
    jwt_secret: String,
}

/// Get the base URL — either from env (for Python) or start a Rust server.
async fn base_url() -> String {
    setup_server().await.base_url
}

/// Set up the test server, returning the base URL and optionally the DB pool.
async fn setup_server() -> TestServer {
    if let Ok(url) = std::env::var("CONTRACT_TEST_BASE_URL") {
        return TestServer {
            base_url: url,
            db: None,
            jwt_secret: None,
            encryption_key: None,
        };
    }

    // Load shared constants (idempotent — OnceLock ignores second call)
    if let Ok(path) = kyomi_core::constants::find_constants_file() {
        let _ = kyomi_core::constants::load(&path);
    }

    let config = kyomi_core::Config::test_config();
    let db = kyomi_core::db::create_pool(&config.database_url)
        .await
        .expect("test DB should be running (docker compose up)");
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

    let jwt_secret = config.jwt_secret.clone();
    let encryption_key_arc = std::sync::Arc::new(encryption_key);

    let ws_manager = kyomi_auth::websocket::WebSocketManager::new(
        None, db.clone(),
    );

    let state = kyomi_api::state::AppState {
        db: db.clone(),
        kv: kv.clone(),
        redis: None,
        config: std::sync::Arc::new(config.clone()),
        encryption_key: encryption_key_arc.clone(),
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

    // Give the server a moment to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    TestServer {
        base_url: format!("http://{addr}"),
        db: Some(db),
        jwt_secret: Some(jwt_secret),
        encryption_key: Some(encryption_key_arc),
    }
}

/// Create an authenticated test context with a unique admin user and workspace.
///
/// This creates real DB rows (user, workspace, workspace_user) and mints a JWT
/// so that subsequent HTTP requests are authenticated. Only works in Rust-backend
/// mode (direct DB access).
///
/// Returns `None` when `CONTRACT_TEST_BASE_URL` is set (Python mode).
async fn setup_auth_context(suffix: &str) -> Option<AuthContext> {
    let server = setup_server().await;
    let db = server.db?;
    let jwt_secret = server.jwt_secret.expect("jwt_secret should be set in Rust mode");
    let encryption_key = server.encryption_key.expect("encryption_key should be set in Rust mode");

    let email = format!("ds-test-{suffix}@contract-test.local");

    // Clean up any leftover test data from a previous run
    cleanup_test_user(&db, &email).await;

    // Create a verified user
    let user = kyomi_auth::user_service::create_user(&db, &email, Some("DS Test User"), true)
        .await
        .expect("should create test user");

    // Create a workspace (user becomes admin + owner)
    let workspace_id =
        kyomi_auth::user_service::create_workspace_for_user(&db, &user.user_id, Some("DS Test User"), &email)
            .await
            .expect("should create test workspace");

    // Mint a JWT with workspace context
    let mut extra = HashMap::new();
    extra.insert("user_id".to_string(), json!(user.user_id));
    extra.insert("email".to_string(), json!(email));
    extra.insert("name".to_string(), json!("DS Test User"));
    extra.insert("workspace_id".to_string(), json!(workspace_id));
    extra.insert(
        "workspace_roles".to_string(),
        json!(["workspace_admin"]),
    );

    let access_token = kyomi_auth::jwt::create_access_token_str(
        &user.user_id,
        &jwt_secret,
        60, // 60 minutes — plenty for test run
        extra,
    )
    .expect("should create access token");

    Some(AuthContext {
        base_url: server.base_url,
        access_token,
        user_id: user.user_id,
        workspace_id,
        db,
        encryption_key,
        jwt_secret,
    })
}

/// Clean up a test user and all related data.
async fn cleanup_test_user(db: &kyomi_core::DbPool, email: &str) {
    // Find the user by email
    let user_id: Option<String> = match db {
        kyomi_core::db::DbPool::Postgres(pg) =>
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE email = $1")
                .bind(email).fetch_optional(pg).await.unwrap_or(None),
        kyomi_core::db::DbPool::Sqlite(sq) =>
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE email = $1")
                .bind(email).fetch_optional(sq).await.unwrap_or(None),
    };

    if let Some(uid) = user_id {
        // Find workspace(s) owned by this user
        let workspace_ids: Vec<String> = match db {
            kyomi_core::db::DbPool::Postgres(pg) =>
                sqlx::query_scalar::<_, String>("SELECT workspace_id FROM workspaces WHERE owner_user_id = $1")
                    .bind(&uid).fetch_all(pg).await.unwrap_or_default(),
            kyomi_core::db::DbPool::Sqlite(sq) =>
                sqlx::query_scalar::<_, String>("SELECT workspace_id FROM workspaces WHERE owner_user_id = $1")
                    .bind(&uid).fetch_all(sq).await.unwrap_or_default(),
        };

        for ws_id in &workspace_ids {
            // Delete SQL query history in this workspace
            let _ = kyomi_core::db_execute!(db, "DELETE FROM sql_query_history WHERE workspace_id = $1", ws_id);

            // Delete datasource credentials in this workspace
            let _ = kyomi_core::db_execute!(db, "DELETE FROM user_datasource_credentials WHERE workspace_id = $1", ws_id);

            // Delete datasource preferences for datasources in this workspace
            let _ = kyomi_core::db_execute!(
                db,
                "DELETE FROM user_datasource_preferences WHERE datasource_config_id IN \
                 (SELECT id FROM datasource_configs WHERE workspace_id = $1)",
                ws_id
            );

            // Delete datasource configs in this workspace
            let _ = kyomi_core::db_execute!(db, "DELETE FROM datasource_configs WHERE workspace_id = $1", ws_id);

            // Delete workspace users
            let _ = kyomi_core::db_execute!(db, "DELETE FROM workspace_users WHERE workspace_id = $1", ws_id);

            // Delete workspace
            let _ = kyomi_core::db_execute!(db, "DELETE FROM workspaces WHERE workspace_id = $1", ws_id);
        }

        // Delete user
        let _ = kyomi_core::db_execute!(db, "DELETE FROM users WHERE user_id = $1", &uid);
    }
}

/// Clean up a specific datasource by slug within a workspace.
async fn cleanup_datasource(db: &kyomi_core::DbPool, workspace_id: &str, slug: &str) {
    let ds_id: Option<String> = match db {
        kyomi_core::db::DbPool::Postgres(pg) =>
            sqlx::query_scalar::<_, String>("SELECT id FROM datasource_configs WHERE workspace_id = $1 AND slug = $2")
                .bind(workspace_id).bind(slug).fetch_optional(pg).await.unwrap_or(None),
        kyomi_core::db::DbPool::Sqlite(sq) =>
            sqlx::query_scalar::<_, String>("SELECT id FROM datasource_configs WHERE workspace_id = $1 AND slug = $2")
                .bind(workspace_id).bind(slug).fetch_optional(sq).await.unwrap_or(None),
    };

    if let Some(id) = ds_id {
        let _ = kyomi_core::db_execute!(db, "DELETE FROM user_datasource_credentials WHERE datasource_config_id = $1", &id);
        let _ = kyomi_core::db_execute!(db, "DELETE FROM user_datasource_preferences WHERE datasource_config_id = $1", &id);
        let _ = kyomi_core::db_execute!(db, "DELETE FROM datasource_configs WHERE id = $1", &id);
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// Build an authenticated request with the proper headers.
fn auth_get(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .get(format!("{base}{path}"))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={token}"))
}

fn auth_post(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .post(format!("{base}{path}"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("cookie", format!("access_token={token}"))
}

fn auth_put(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .put(format!("{base}{path}"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("cookie", format!("access_token={token}"))
}

fn auth_delete(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .delete(format!("{base}{path}"))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={token}"))
}

fn auth_patch(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .patch(format!("{base}{path}"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("cookie", format!("access_token={token}"))
}

/// Check whether we are in Python-target mode (skip authenticated tests).
fn is_python_mode() -> bool {
    std::env::var("CONTRACT_TEST_BASE_URL").is_ok()
}

// ===========================================================================
// 1. Unauthenticated 401 tests — datasource endpoints require auth
// ===========================================================================

#[tokio::test]
async fn list_datasources_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/datasources/"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /datasources/ without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn create_datasource_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/datasources/"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"name": "Test", "datasource_type": "postgres"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /datasources/ without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn get_datasource_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/datasources/test-slug"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /datasources/test-slug without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn update_datasource_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .put(format!("{base}/api/v1/datasources/test-slug"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"name": "Updated"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "PUT /datasources/test-slug without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn delete_datasource_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .delete(format!("{base}/api/v1/datasources/test-slug"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "DELETE /datasources/test-slug without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn credential_status_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/datasources/credential-status"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /datasources/credential-status without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn sample_available_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/datasources/sample/available"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /datasources/sample/available without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn create_sample_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/datasources/sample"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /datasources/sample without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn test_connection_standalone_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/datasources/test-connection"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"datasource_type": "postgres", "connection_config": {}}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /datasources/test-connection without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn get_credentials_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/datasources/test-slug/credentials"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /datasources/test-slug/credentials without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn save_credentials_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/datasources/test-slug/credentials"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"credentials": {"username": "test"}}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /datasources/test-slug/credentials without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn delete_credentials_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .delete(format!("{base}/api/v1/datasources/test-slug/credentials"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "DELETE /datasources/test-slug/credentials without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn get_settings_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/datasources/test-slug/settings"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /datasources/test-slug/settings without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn save_settings_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .put(format!("{base}/api/v1/datasources/test-slug/settings"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"credentials": {"username": "test"}}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "PUT /datasources/test-slug/settings without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn toggle_datasource_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/datasources/test-slug/toggle"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"enabled": true}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /datasources/test-slug/toggle without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn test_datasource_connection_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/datasources/test-slug/test"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /datasources/test-slug/test without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn generate_ssh_key_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/datasources/test-slug/generate-ssh-key"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /datasources/test-slug/generate-ssh-key without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

#[tokio::test]
async fn affected_users_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!(
            "{base}/api/v1/datasources/test-slug/affected-users?new_auth_mode=password"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /datasources/test-slug/affected-users without auth should be 401");

    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error response must have 'detail' field");
}

// ===========================================================================
// 2. Error response format — all endpoints return {"detail": ...}
// ===========================================================================

#[tokio::test]
async fn datasource_error_responses_have_detail_field() {
    let base = base_url().await;

    let endpoints = vec![
        ("GET", "/api/v1/datasources/"),
        ("GET", "/api/v1/datasources/credential-status"),
        ("GET", "/api/v1/datasources/sample/available"),
        ("GET", "/api/v1/datasources/test-slug"),
        ("GET", "/api/v1/datasources/test-slug/credentials"),
        ("GET", "/api/v1/datasources/test-slug/settings"),
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
// 3. CORS — datasource endpoints have proper CORS headers
// ===========================================================================

#[tokio::test]
async fn datasource_endpoints_have_cors_headers() {
    let base = base_url().await;

    // Even 401 responses should have CORS headers
    let resp = client()
        .get(format!("{base}/api/v1/datasources/"))
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
        "CORS should be present on datasource endpoints"
    );
}

#[tokio::test]
async fn datasource_endpoints_allow_credentials_cors() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/datasources/"))
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

// ===========================================================================
// 4. Types endpoint — returns all 9 datasource types with metadata
// ===========================================================================

#[tokio::test]
async fn types_returns_all_9_types() {
    // GET /types does not require auth in the Rust backend
    // (it's useful for the setup wizard before any datasource exists).
    // However, if the backend requires auth for this endpoint, we need auth context.
    // Let's try unauthenticated first and fall back to authenticated.
    let ctx = setup_auth_context("types-all").await;
    if ctx.is_none() && is_python_mode() {
        eprintln!("SKIP: types_returns_all_9_types — requires Rust-backend mode for auth setup");
        return;
    }
    let ctx = ctx.expect("need auth context for types test");

    let resp = auth_get(&ctx.base_url, "/api/v1/datasources/types", &ctx.access_token)
        .send()
        .await
        .expect("types request should succeed");

    // Clean up
    cleanup_test_user(&ctx.db, "ds-test-types-all@contract-test.local").await;

    assert_eq!(resp.status(), 200, "GET /types should return 200");

    let body: Value = resp.json().await.expect("types should return JSON");
    let types = body["types"]
        .as_array()
        .expect("response should have a 'types' array");

    assert_eq!(types.len(), 9, "should have exactly 9 datasource types, got {}", types.len());
}

#[tokio::test]
async fn types_sorted_by_popularity_postgres_first() {
    let ctx = setup_auth_context("types-sort").await;
    if ctx.is_none() {
        eprintln!("SKIP: types_sorted_by_popularity_postgres_first — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(&ctx.base_url, "/api/v1/datasources/types", &ctx.access_token)
        .send()
        .await
        .expect("types request should succeed");

    cleanup_test_user(&ctx.db, "ds-test-types-sort@contract-test.local").await;

    let body: Value = resp.json().await.expect("should be JSON");
    let types = body["types"].as_array().expect("'types' array");

    let first_type = types[0]["type_id"]
        .as_str()
        .expect("first type should have 'type_id'");
    assert_eq!(first_type, "postgres", "first type should be postgres (most popular), got {first_type}");
}

#[tokio::test]
async fn types_each_has_required_metadata_fields() {
    let ctx = setup_auth_context("types-fields").await;
    if ctx.is_none() {
        eprintln!("SKIP: types_each_has_required_metadata_fields — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(&ctx.base_url, "/api/v1/datasources/types", &ctx.access_token)
        .send()
        .await
        .expect("types request should succeed");

    cleanup_test_user(&ctx.db, "ds-test-types-fields@contract-test.local").await;

    let body: Value = resp.json().await.expect("should be JSON");
    let types = body["types"].as_array().expect("'types' array");

    for t in types {
        let type_id = t["type_id"].as_str().unwrap_or("unknown");

        assert!(t.get("type_id").is_some(), "{type_id}: missing 'type_id'");
        assert!(t["type_id"].is_string(), "{type_id}: 'type_id' should be string");

        assert!(t.get("display_name").is_some(), "{type_id}: missing 'display_name'");
        assert!(t["display_name"].is_string(), "{type_id}: 'display_name' should be string");

        assert!(t.get("description").is_some(), "{type_id}: missing 'description'");
        assert!(t["description"].is_string(), "{type_id}: 'description' should be string");

        assert!(t.get("credential_fields").is_some(), "{type_id}: missing 'credential_fields'");
        assert!(t["credential_fields"].is_array(), "{type_id}: 'credential_fields' should be array");

        assert!(t.get("requires_user_credentials").is_some(), "{type_id}: missing 'requires_user_credentials'");
        assert!(t["requires_user_credentials"].is_boolean(), "{type_id}: 'requires_user_credentials' should be bool");

        assert!(t.get("accepts_user_context").is_some(), "{type_id}: missing 'accepts_user_context'");

        // default_port can be null for BigQuery, but the field must be present
        assert!(t.get("default_port").is_some(), "{type_id}: missing 'default_port'");
        assert!(t["accepts_user_context"].is_boolean(), "{type_id}: 'accepts_user_context' should be bool");

        assert!(t.get("catalog_container_label").is_some(), "{type_id}: missing 'catalog_container_label'");
        assert!(t["catalog_container_label"].is_string(), "{type_id}: 'catalog_container_label' should be string");

        assert!(t.get("catalog_config_keys").is_some(), "{type_id}: missing 'catalog_config_keys'");
        assert!(t["catalog_config_keys"].is_array(), "{type_id}: 'catalog_config_keys' should be array");

        assert!(t.get("supports_catalog_discovery").is_some(), "{type_id}: missing 'supports_catalog_discovery'");
        assert!(t["supports_catalog_discovery"].is_boolean(), "{type_id}: 'supports_catalog_discovery' should be bool");

        assert!(t.get("auth_modes").is_some(), "{type_id}: missing 'auth_modes'");
        assert!(t["auth_modes"].is_array(), "{type_id}: 'auth_modes' should be array");

        assert!(t.get("sensitive_connection_config_fields").is_some(), "{type_id}: missing 'sensitive_connection_config_fields'");
        assert!(t["sensitive_connection_config_fields"].is_array(), "{type_id}: 'sensitive_connection_config_fields' should be array");
    }
}

// ===========================================================================
// 5. Datasource CRUD tests (authenticated, Rust-backend mode only)
// ===========================================================================

#[tokio::test]
async fn create_datasource_returns_201_with_correct_fields() {
    let ctx = setup_auth_context("crud-shape").await;
    if ctx.is_none() {
        eprintln!("SKIP: create_datasource_response_has_correct_fields — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(
            json!({
                "name": "Shape Test PG",
                "slug": "shape-test-pg",
                "datasource_type": "postgres",
                "connection_config": {
                    "host": "localhost",
                    "port": 5432
                }
            })
            .to_string(),
        )
        .send()
        .await
        .expect("create request should succeed");

    assert_eq!(resp.status(), 201, "should return 201 CREATED");

    let body: Value = resp.json().await.expect("should return JSON");

    // Verify response shape matches DatasourceResponse
    assert!(body.get("id").is_some(), "missing 'id'");
    assert!(body["id"].is_string(), "'id' should be string");
    assert!(
        body["id"].as_str().unwrap().starts_with("ds-"),
        "id should start with 'ds-' prefix"
    );

    assert_eq!(body["slug"], "shape-test-pg", "'slug' should match request");
    assert_eq!(body["name"], "Shape Test PG", "'name' should match request");
    assert_eq!(body["datasource_type"], "postgres", "'datasource_type' should match");
    assert_eq!(body["active"], true, "'active' should default to true");

    assert!(body.get("connection_config").is_some(), "missing 'connection_config'");
    assert!(body["connection_config"].is_object(), "'connection_config' should be object");

    assert!(body.get("created_at").is_some(), "missing 'created_at'");
    assert!(body["created_at"].is_string(), "'created_at' should be string");

    assert!(body.get("updated_at").is_some(), "missing 'updated_at'");
    assert!(body["updated_at"].is_string(), "'updated_at' should be string");

    assert!(body.get("has_user_credentials").is_some(), "missing 'has_user_credentials'");
    assert_eq!(body["has_user_credentials"], false, "'has_user_credentials' should be false for new datasource");

    assert!(body.get("auto_refresh_allowed").is_some(), "missing 'auto_refresh_allowed'");
    assert!(body["auto_refresh_allowed"].is_boolean(), "'auto_refresh_allowed' should be boolean");

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "shape-test-pg").await;
    cleanup_test_user(&ctx.db, "ds-test-crud-shape@contract-test.local").await;
}

#[tokio::test]
async fn create_duplicate_name_returns_409() {
    let ctx = setup_auth_context("crud-dup-name").await;
    if ctx.is_none() {
        eprintln!("SKIP: create_duplicate_name_returns_409 — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create first datasource
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(
            json!({
                "name": "Dup Name Test",
                "slug": "dup-name-1",
                "datasource_type": "postgres"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("first create should succeed");
    assert_eq!(resp.status(), 201, "first create should be 201");

    // Create second with same name, different slug
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(
            json!({
                "name": "Dup Name Test",
                "slug": "dup-name-2",
                "datasource_type": "postgres"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("duplicate name request should succeed");

    let status = resp.status();
    let body: Value = resp.json().await.expect("should return JSON");

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "dup-name-1").await;
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "dup-name-2").await;
    cleanup_test_user(&ctx.db, "ds-test-crud-dup-name@contract-test.local").await;

    assert_eq!(status, 409, "duplicate name should return 409");
    assert!(body.get("detail").is_some(), "conflict error should have 'detail'");
}

#[tokio::test]
async fn create_duplicate_slug_returns_409() {
    let ctx = setup_auth_context("crud-dup-slug").await;
    if ctx.is_none() {
        eprintln!("SKIP: create_duplicate_slug_returns_409 — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create first
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(
            json!({
                "name": "Slug Test A",
                "slug": "dup-slug-test",
                "datasource_type": "postgres"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("first create should succeed");
    assert_eq!(resp.status(), 201);

    // Create second with same slug
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(
            json!({
                "name": "Slug Test B",
                "slug": "dup-slug-test",
                "datasource_type": "postgres"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("duplicate slug request should succeed");

    let status = resp.status();
    let body: Value = resp.json().await.expect("should return JSON");

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "dup-slug-test").await;
    cleanup_test_user(&ctx.db, "ds-test-crud-dup-slug@contract-test.local").await;

    assert_eq!(status, 409, "duplicate slug should return 409");
    assert!(body.get("detail").is_some(), "conflict error should have 'detail'");
}

#[tokio::test]
async fn create_with_invalid_type_returns_400() {
    let ctx = setup_auth_context("crud-bad-type").await;
    if ctx.is_none() {
        eprintln!("SKIP: create_with_invalid_type_returns_400 — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(
            json!({
                "name": "Bad Type",
                "datasource_type": "nonexistent_db"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("invalid type request should succeed");

    let status = resp.status();
    let body: Value = resp.json().await.expect("should return JSON");

    cleanup_test_user(&ctx.db, "ds-test-crud-bad-type@contract-test.local").await;

    assert_eq!(status, 400, "invalid datasource type should return 400");
    assert!(body.get("detail").is_some(), "bad request should have 'detail'");
}

#[tokio::test]
async fn list_datasources_empty_workspace() {
    let ctx = setup_auth_context("list-empty").await;
    if ctx.is_none() {
        eprintln!("SKIP: list_datasources_empty_workspace — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .send()
        .await
        .expect("list request should succeed");

    assert_eq!(resp.status(), 200, "list should return 200");

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(body.is_array(), "list should return an array");
    assert_eq!(body.as_array().unwrap().len(), 0, "empty workspace should have 0 datasources");

    cleanup_test_user(&ctx.db, "ds-test-list-empty@contract-test.local").await;
}

#[tokio::test]
async fn list_datasources_with_data() {
    let ctx = setup_auth_context("list-data").await;
    if ctx.is_none() {
        eprintln!("SKIP: list_datasources_with_data — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create two datasources
    let resp1 = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "List DS 1", "slug": "list-ds-1", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .expect("create 1 should succeed");
    assert_eq!(resp1.status(), 201);

    let resp2 = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "List DS 2", "slug": "list-ds-2", "datasource_type": "mysql"}).to_string())
        .send()
        .await
        .expect("create 2 should succeed");
    assert_eq!(resp2.status(), 201);

    // List
    let resp = auth_get(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .send()
        .await
        .expect("list request should succeed");
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");
    let items = body.as_array().expect("should be array");
    assert_eq!(items.len(), 2, "should have 2 datasources");

    // Verify list item shape
    for item in items {
        assert!(item.get("id").is_some(), "list item missing 'id'");
        assert!(item.get("slug").is_some(), "list item missing 'slug'");
        assert!(item.get("name").is_some(), "list item missing 'name'");
        assert!(item.get("datasource_type").is_some(), "list item missing 'datasource_type'");
        assert!(item.get("active").is_some(), "list item missing 'active'");
        assert!(item.get("created_at").is_some(), "list item missing 'created_at'");
        assert!(item.get("auto_refresh_allowed").is_some(), "list item missing 'auto_refresh_allowed'");
        assert!(item.get("is_sample").is_some(), "list item missing 'is_sample'");
    }

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "list-ds-1").await;
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "list-ds-2").await;
    cleanup_test_user(&ctx.db, "ds-test-list-data@contract-test.local").await;
}

#[tokio::test]
async fn list_datasources_filters_inactive_by_default() {
    let ctx = setup_auth_context("list-inactive").await;
    if ctx.is_none() {
        eprintln!("SKIP: list_datasources_filters_inactive_by_default — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create a datasource, then deactivate it
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Inactive DS", "slug": "inactive-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .expect("create should succeed");
    assert_eq!(resp.status(), 201);
    let created: Value = resp.json().await.unwrap();
    let slug = created["slug"].as_str().unwrap();

    // Deactivate via update
    let resp = auth_put(
        &ctx.base_url,
        &format!("/api/v1/datasources/{slug}"),
        &ctx.access_token,
    )
    .body(json!({"active": false}).to_string())
    .send()
    .await
    .expect("deactivate should succeed");
    assert_eq!(resp.status(), 200);

    // List without include_inactive — should be empty
    let resp = auth_get(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .send()
        .await
        .expect("list should succeed");
    let body: Value = resp.json().await.unwrap();
    let items = body.as_array().expect("should be array");
    assert_eq!(items.len(), 0, "inactive datasource should be filtered out by default");

    // List with include_inactive — should include it
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/?include_inactive=true",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("list with inactive should succeed");
    let body: Value = resp.json().await.unwrap();
    let items = body.as_array().expect("should be array");
    assert_eq!(items.len(), 1, "include_inactive should show the deactivated datasource");

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "inactive-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-list-inactive@contract-test.local").await;
}

#[tokio::test]
async fn get_datasource_by_slug() {
    let ctx = setup_auth_context("get-slug").await;
    if ctx.is_none() {
        eprintln!("SKIP: get_datasource_by_slug — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Get Slug DS", "slug": "get-slug-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Get by slug
    let resp = auth_get(&ctx.base_url, "/api/v1/datasources/get-slug-ds", &ctx.access_token)
        .send()
        .await
        .expect("get by slug should succeed");
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["slug"], "get-slug-ds");
    assert_eq!(body["name"], "Get Slug DS");
    assert_eq!(body["datasource_type"], "postgres");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "get-slug-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-get-slug@contract-test.local").await;
}

#[tokio::test]
async fn get_datasource_by_uuid() {
    let ctx = setup_auth_context("get-uuid").await;
    if ctx.is_none() {
        eprintln!("SKIP: get_datasource_by_uuid — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Get UUID DS", "slug": "get-uuid-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let created: Value = resp.json().await.unwrap();
    let ds_id = created["id"].as_str().expect("created ds should have 'id'");
    assert!(ds_id.starts_with("ds-"), "id should start with ds- prefix");

    // Get by UUID
    let resp = auth_get(
        &ctx.base_url,
        &format!("/api/v1/datasources/{ds_id}"),
        &ctx.access_token,
    )
    .send()
    .await
    .expect("get by UUID should succeed");
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["id"], ds_id);
    assert_eq!(body["slug"], "get-uuid-ds");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "get-uuid-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-get-uuid@contract-test.local").await;
}

#[tokio::test]
async fn get_nonexistent_returns_404_with_available_slugs() {
    let ctx = setup_auth_context("get-404").await;
    if ctx.is_none() {
        eprintln!("SKIP: get_nonexistent_returns_404_with_available_slugs — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/nonexistent-slug",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("get nonexistent should succeed");

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();

    cleanup_test_user(&ctx.db, "ds-test-get-404@contract-test.local").await;

    assert_eq!(status, 404, "nonexistent datasource should return 404");
    assert!(body.get("detail").is_some(), "404 should have 'detail' field");
}

#[tokio::test]
async fn update_datasource_name() {
    let ctx = setup_auth_context("update-name").await;
    if ctx.is_none() {
        eprintln!("SKIP: update_datasource_name — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Original Name", "slug": "update-name-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Update name
    let resp = auth_put(
        &ctx.base_url,
        "/api/v1/datasources/update-name-ds",
        &ctx.access_token,
    )
    .body(json!({"name": "Updated Name"}).to_string())
    .send()
    .await
    .expect("update should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "Updated Name");
    assert_eq!(body["slug"], "update-name-ds", "slug should not change");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "update-name-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-update-name@contract-test.local").await;
}

#[tokio::test]
async fn update_connection_config_preserves_masked_sensitive_fields() {
    let ctx = setup_auth_context("update-mask").await;
    if ctx.is_none() {
        eprintln!("SKIP: update_connection_config_preserves_masked_sensitive_fields — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create with a password in connection_config
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(
            json!({
                "name": "Mask Test DS",
                "slug": "mask-test-ds",
                "datasource_type": "postgres",
                "connection_config": {
                    "host": "db.example.com",
                    "port": 5432,
                    "shared_password": "super-secret-pw"
                }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Update with masked password — should preserve original
    let resp = auth_put(
        &ctx.base_url,
        "/api/v1/datasources/mask-test-ds",
        &ctx.access_token,
    )
    .body(
        json!({
            "connection_config": {
                "host": "new-host.example.com",
                "port": 5432,
                "shared_password": "********"
            }
        })
        .to_string(),
    )
    .send()
    .await
    .expect("update with mask should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // The response should show the new host
    assert_eq!(body["connection_config"]["host"], "new-host.example.com");
    // The password should be masked in the response
    assert_eq!(
        body["connection_config"]["shared_password"], "********",
        "response should mask the password"
    );

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "mask-test-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-update-mask@contract-test.local").await;
}

#[tokio::test]
async fn update_sample_datasource_connection_config_returns_400() {
    let ctx = setup_auth_context("update-sample").await;
    if ctx.is_none() {
        eprintln!("SKIP: update_sample_datasource_connection_config_returns_400 — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create a datasource with is_sample=true directly in DB
    let ds_id = format!("ds-{}", uuid::Uuid::new_v4());
    let active_val = if ctx.db.is_postgres() { "true" } else { "1" };
    let config_json = serde_json::to_string(&json!({"is_sample": true, "host": "localhost"})).unwrap();
    let sql = format!(
        "INSERT INTO datasource_configs (id, workspace_id, name, slug, datasource_type, connection_config, active) \
         VALUES ($1, $2, $3, $4, $5, $6, {active_val})"
    );
    kyomi_core::db_execute!(
        &ctx.db,
        &sql,
        &ds_id,
        &ctx.workspace_id,
        "Sample DS",
        "sample-update-test",
        "clickhouse",
        &config_json
    )
    .expect("should insert sample ds");

    // Try to update connection_config
    let resp = auth_put(
        &ctx.base_url,
        "/api/v1/datasources/sample-update-test",
        &ctx.access_token,
    )
    .body(json!({"connection_config": {"host": "hacked"}}).to_string())
    .send()
    .await
    .expect("update sample should succeed");

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "sample-update-test").await;
    cleanup_test_user(&ctx.db, "ds-test-update-sample@contract-test.local").await;

    assert_eq!(status, 400, "updating sample datasource config should return 400");
    assert!(body.get("detail").is_some(), "should have 'detail'");
}

#[tokio::test]
async fn delete_datasource_returns_204() {
    let ctx = setup_auth_context("delete-ds").await;
    if ctx.is_none() {
        eprintln!("SKIP: delete_datasource_returns_204 — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Delete Me DS", "slug": "delete-me-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Delete
    let resp = auth_delete(
        &ctx.base_url,
        "/api/v1/datasources/delete-me-ds",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("delete should succeed");

    assert_eq!(resp.status(), 204, "DELETE should return 204 No Content");

    // Verify it's gone
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/delete-me-ds",
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 404, "deleted datasource should return 404");

    cleanup_test_user(&ctx.db, "ds-test-delete-ds@contract-test.local").await;
}

#[tokio::test]
async fn delete_cascades_credentials() {
    let ctx = setup_auth_context("delete-cascade").await;
    if ctx.is_none() {
        eprintln!("SKIP: delete_cascades_credentials — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create datasource
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Cascade DS", "slug": "cascade-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Save credentials
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/cascade-ds/credentials",
        &ctx.access_token,
    )
    .body(json!({"credentials": {"username": "test", "password": "secret"}}).to_string())
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200, "save credentials should succeed");

    // Delete datasource
    let resp = auth_delete(
        &ctx.base_url,
        "/api/v1/datasources/cascade-ds",
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 204);

    // Verify credentials are also gone (check DB directly)
    let cred_count: i64 = kyomi_core::db_fetch_scalar!(
        &ctx.db,
        i64,
        "SELECT COUNT(*) FROM user_datasource_credentials \
         WHERE user_id = $1 AND workspace_id = $2",
        &ctx.user_id,
        &ctx.workspace_id
    )
    .unwrap_or(0);

    cleanup_test_user(&ctx.db, "ds-test-delete-cascade@contract-test.local").await;

    assert_eq!(cred_count, 0, "credentials should be cascade-deleted with datasource");
}

#[tokio::test]
async fn non_admin_create_returns_403() {
    // This test requires creating a non-admin user. We set up a workspace
    // and then create a second user who is only a "member" (not admin).
    let ctx = setup_auth_context("non-admin").await;
    if ctx.is_none() {
        eprintln!("SKIP: non_admin_create_returns_403 — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create a non-admin user
    let member_email = "ds-test-non-admin-member@contract-test.local";
    cleanup_test_user(&ctx.db, member_email).await;

    let member = kyomi_auth::user_service::create_user(&ctx.db, member_email, Some("Member User"), true)
        .await
        .expect("should create member user");

    // Add as member (not admin) to the workspace
    let active_val = if ctx.db.is_postgres() { "true" } else { "1" };
    let insert_sql = format!(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active) VALUES ($1, $2, 'workspace_user', {active_val})"
    );
    kyomi_core::db_execute!(
        &ctx.db,
        &insert_sql,
        &ctx.workspace_id,
        &member.user_id
    )
    .expect("should add member");

    // Update member's last_workspace
    let _ = kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE users SET last_workspace_id = $1 WHERE user_id = $2",
        &ctx.workspace_id,
        &member.user_id
    );

    // Mint JWT for member with non-admin role
    let mut extra = HashMap::new();
    extra.insert("user_id".to_string(), json!(member.user_id));
    extra.insert("email".to_string(), json!(member_email));
    extra.insert("workspace_id".to_string(), json!(ctx.workspace_id));
    extra.insert("workspace_roles".to_string(), json!(["member"]));

    let member_token = kyomi_auth::jwt::create_access_token_str(
        &member.user_id,
        &ctx.jwt_secret,
        60,
        extra,
    )
    .expect("should create member token");

    // Try to create a datasource as non-admin
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &member_token)
        .body(json!({"name": "Forbidden DS", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .expect("non-admin create should succeed (HTTP level)");

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();

    // Clean up
    cleanup_test_user(&ctx.db, member_email).await;
    cleanup_test_user(&ctx.db, "ds-test-non-admin@contract-test.local").await;

    assert_eq!(status, 403, "non-admin should get 403 Forbidden");
    assert!(body.get("detail").is_some(), "403 should have 'detail'");
}

// ===========================================================================
// 6. Credential tests
// ===========================================================================

#[tokio::test]
async fn save_credentials_returns_200_with_masked_preview() {
    let ctx = setup_auth_context("cred-save").await;
    if ctx.is_none() {
        eprintln!("SKIP: save_credentials_returns_200_with_masked_preview — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create datasource
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Cred Test DS", "slug": "cred-test-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Save credentials
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/cred-test-ds/credentials",
        &ctx.access_token,
    )
    .body(json!({"credentials": {"username": "myuser", "password": "mypass"}}).to_string())
    .send()
    .await
    .expect("save credentials should succeed");

    assert_eq!(resp.status(), 200, "save credentials should return 200");
    let body: Value = resp.json().await.unwrap();

    // Verify response shape
    assert!(body.get("datasource_id").is_some(), "missing 'datasource_id'");
    assert!(body.get("datasource_slug").is_some(), "missing 'datasource_slug'");
    assert!(body.get("datasource_name").is_some(), "missing 'datasource_name'");
    assert!(body.get("datasource_type").is_some(), "missing 'datasource_type'");
    assert_eq!(body["has_credentials"], true, "'has_credentials' should be true");
    assert!(body.get("credentials_preview").is_some(), "missing 'credentials_preview'");
    assert!(body.get("created_at").is_some(), "missing 'created_at'");
    assert!(body.get("updated_at").is_some(), "missing 'updated_at'");

    // Password should be masked in preview
    let preview = &body["credentials_preview"];
    assert_eq!(preview["username"], "myuser", "username should not be masked");
    assert_eq!(preview["password"], "********", "password should be masked in preview");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "cred-test-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-cred-save@contract-test.local").await;
}

#[tokio::test]
async fn get_credentials_returns_masked_preview() {
    let ctx = setup_auth_context("cred-get").await;
    if ctx.is_none() {
        eprintln!("SKIP: get_credentials_returns_masked_preview — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create and save credentials
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Cred Get DS", "slug": "cred-get-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    auth_post(
        &ctx.base_url,
        "/api/v1/datasources/cred-get-ds/credentials",
        &ctx.access_token,
    )
    .body(json!({"credentials": {"username": "readuser", "password": "readpass"}}).to_string())
    .send()
    .await
    .unwrap();

    // Get credentials
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/cred-get-ds/credentials",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("get credentials should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["has_credentials"], true);
    assert_eq!(body["credentials_preview"]["password"], "********", "password should be masked on GET");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "cred-get-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-cred-get@contract-test.local").await;
}

#[tokio::test]
async fn delete_credentials_returns_204() {
    let ctx = setup_auth_context("cred-del").await;
    if ctx.is_none() {
        eprintln!("SKIP: delete_credentials_returns_204 — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create datasource and save credentials
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Cred Del DS", "slug": "cred-del-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();

    auth_post(
        &ctx.base_url,
        "/api/v1/datasources/cred-del-ds/credentials",
        &ctx.access_token,
    )
    .body(json!({"credentials": {"username": "del", "password": "me"}}).to_string())
    .send()
    .await
    .unwrap();

    // Delete credentials
    let resp = auth_delete(
        &ctx.base_url,
        "/api/v1/datasources/cred-del-ds/credentials",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("delete credentials should succeed");

    assert_eq!(resp.status(), 204, "DELETE credentials should return 204");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "cred-del-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-cred-del@contract-test.local").await;
}

#[tokio::test]
async fn save_credentials_for_nonexistent_datasource_returns_404() {
    let ctx = setup_auth_context("cred-404").await;
    if ctx.is_none() {
        eprintln!("SKIP: save_credentials_for_nonexistent_datasource — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/nonexistent-ds/credentials",
        &ctx.access_token,
    )
    .body(json!({"credentials": {"username": "test"}}).to_string())
    .send()
    .await
    .expect("request should succeed");

    let status = resp.status();
    cleanup_test_user(&ctx.db, "ds-test-cred-404@contract-test.local").await;

    assert_eq!(status, 404, "save credentials for nonexistent ds should return 404");
}

// ===========================================================================
// 7. Settings tests
// ===========================================================================

#[tokio::test]
async fn get_settings_returns_workspace_defaults_and_user_overrides() {
    let ctx = setup_auth_context("settings-get").await;
    if ctx.is_none() {
        eprintln!("SKIP: get_settings_returns_workspace_defaults_and_user_overrides — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create datasource
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Settings DS", "slug": "settings-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();

    // Get settings
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/settings-ds/settings",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("get settings should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // Verify response shape
    assert!(body.get("datasource_id").is_some(), "missing 'datasource_id'");
    assert!(body.get("datasource_slug").is_some(), "missing 'datasource_slug'");
    assert!(body.get("datasource_name").is_some(), "missing 'datasource_name'");
    assert!(body.get("datasource_type").is_some(), "missing 'datasource_type'");
    assert!(body.get("user_settings").is_some(), "missing 'user_settings'");
    assert!(body.get("workspace_defaults").is_some(), "missing 'workspace_defaults'");
    assert!(body.get("effective_settings").is_some(), "missing 'effective_settings'");
    assert!(body.get("has_oauth").is_some(), "missing 'has_oauth'");
    assert!(body.get("has_bigquery_scopes").is_some(), "missing 'has_bigquery_scopes'");
    assert!(body.get("needs_bigquery_connect").is_some(), "missing 'needs_bigquery_connect'");
    assert!(body.get("connection_config").is_some(), "missing 'connection_config'");
    assert!(body.get("shared_credentials").is_some(), "missing 'shared_credentials'");
    assert!(body.get("credential_status").is_some(), "missing 'credential_status'");
    assert!(body.get("auth_method").is_some(), "missing 'auth_method'");
    assert!(body.get("has_password").is_some(), "missing 'has_password'");
    assert!(body.get("has_username").is_some(), "missing 'has_username'");
    assert!(body.get("has_access_token").is_some(), "missing 'has_access_token'");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "settings-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-settings-get@contract-test.local").await;
}

#[tokio::test]
async fn get_settings_for_bigquery_includes_oauth_status() {
    let ctx = setup_auth_context("settings-bq").await;
    if ctx.is_none() {
        eprintln!("SKIP: get_settings_for_bigquery_includes_oauth_status — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create BigQuery datasource
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(
            json!({
                "name": "Settings BQ DS",
                "slug": "settings-bq-ds",
                "datasource_type": "bigquery",
                "connection_config": {
                    "default_project": "my-project",
                    "default_billing_project": "billing-project"
                }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    // Get settings
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/settings-bq-ds/settings",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("get BQ settings should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // BigQuery-specific fields
    assert!(body.get("has_oauth").is_some(), "BQ settings should have 'has_oauth'");
    assert!(body.get("oauth_email").is_some() || body["oauth_email"].is_null(), "BQ settings should have 'oauth_email'");
    assert!(body.get("has_bigquery_scopes").is_some(), "BQ settings should have 'has_bigquery_scopes'");
    assert!(body.get("needs_bigquery_connect").is_some(), "BQ settings should have 'needs_bigquery_connect'");
    assert!(body.get("auth_mode").is_some() || body["auth_mode"].is_null(), "BQ settings should have 'auth_mode'");
    assert!(body.get("enable_arrow_streaming").is_some() || body["enable_arrow_streaming"].is_null(), "BQ settings should have 'enable_arrow_streaming'");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "settings-bq-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-settings-bq@contract-test.local").await;
}

#[tokio::test]
async fn put_settings_aliases_to_save_credentials() {
    let ctx = setup_auth_context("settings-put").await;
    if ctx.is_none() {
        eprintln!("SKIP: put_settings_aliases_to_save_credentials — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create datasource
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Settings Put DS", "slug": "settings-put-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();

    // PUT settings (should save credentials)
    let resp = auth_put(
        &ctx.base_url,
        "/api/v1/datasources/settings-put-ds/settings",
        &ctx.access_token,
    )
    .body(json!({"credentials": {"username": "settings-user", "password": "settings-pw"}}).to_string())
    .send()
    .await
    .expect("put settings should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["has_credentials"], true, "settings PUT should save credentials");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "settings-put-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-settings-put@contract-test.local").await;
}

// ===========================================================================
// 8. Toggle tests
// ===========================================================================

#[tokio::test]
async fn toggle_enable_without_credentials_returns_400() {
    let ctx = setup_auth_context("toggle-no-cred").await;
    if ctx.is_none() {
        eprintln!("SKIP: toggle_enable_without_credentials_returns_400 — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create datasource (personal auth, no credentials)
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Toggle No Cred DS", "slug": "toggle-no-cred-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();

    // Try to enable without credentials
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/toggle-no-cred-ds/toggle",
        &ctx.access_token,
    )
    .body(json!({"enabled": true}).to_string())
    .send()
    .await
    .expect("toggle without creds should fail gracefully");

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "toggle-no-cred-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-toggle-no-cred@contract-test.local").await;

    assert_eq!(status, 400, "toggle enable without credentials should return 400");
    assert!(body.get("detail").is_some(), "should have error detail");
}

#[tokio::test]
async fn toggle_disable_always_succeeds() {
    let ctx = setup_auth_context("toggle-disable").await;
    if ctx.is_none() {
        eprintln!("SKIP: toggle_disable_always_succeeds — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create datasource with shared auth (so enable doesn't need personal creds)
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(
            json!({
                "name": "Toggle Disable DS",
                "slug": "toggle-disable-ds",
                "datasource_type": "clickhouse",
                "connection_config": {
                    "shared_credentials": true,
                    "shared_username": "admin",
                    "shared_password": "pw"
                }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    // Disable — should always succeed
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/toggle-disable-ds/toggle",
        &ctx.access_token,
    )
    .body(json!({"enabled": false}).to_string())
    .send()
    .await
    .expect("toggle disable should succeed");

    assert_eq!(resp.status(), 200, "disable should return 200");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["enabled"], false, "'enabled' should be false");
    assert!(body.get("message").is_some(), "should have 'message'");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "toggle-disable-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-toggle-disable@contract-test.local").await;
}

#[tokio::test]
async fn toggle_shared_auth_datasource_uses_preferences() {
    let ctx = setup_auth_context("toggle-shared").await;
    if ctx.is_none() {
        eprintln!("SKIP: toggle_shared_auth_datasource_uses_preferences — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create shared-auth datasource
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(
            json!({
                "name": "Shared Auth DS",
                "slug": "shared-auth-ds",
                "datasource_type": "clickhouse",
                "connection_config": {
                    "shared_credentials": true,
                    "shared_username": "reader",
                    "shared_password": "readonly"
                }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    // Enable shared auth — should use preferences table
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/shared-auth-ds/toggle",
        &ctx.access_token,
    )
    .body(json!({"enabled": true}).to_string())
    .send()
    .await
    .expect("toggle shared enable should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["enabled"], true);

    // Verify a preference record was created (not a credential record)
    let pref_count: i64 = kyomi_core::db_fetch_scalar!(
        &ctx.db,
        i64,
        "SELECT COUNT(*) FROM user_datasource_preferences \
         WHERE user_id = $1 AND datasource_config_id IN \
         (SELECT id FROM datasource_configs WHERE slug = 'shared-auth-ds' AND workspace_id = $2)",
        &ctx.user_id,
        &ctx.workspace_id
    )
    .unwrap_or(0);

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "shared-auth-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-toggle-shared@contract-test.local").await;

    assert!(pref_count > 0, "shared auth toggle should create a preference record");
}

#[tokio::test]
async fn toggle_enable_with_valid_credentials_succeeds() {
    let ctx = setup_auth_context("toggle-valid").await;
    if ctx.is_none() {
        eprintln!("SKIP: toggle_enable_with_valid_credentials_succeeds — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create datasource and save credentials
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Toggle Valid DS", "slug": "toggle-valid-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();

    auth_post(
        &ctx.base_url,
        "/api/v1/datasources/toggle-valid-ds/credentials",
        &ctx.access_token,
    )
    .body(json!({"credentials": {"username": "toggler", "password": "pass123"}}).to_string())
    .send()
    .await
    .unwrap();

    // Toggle enable — should succeed
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/toggle-valid-ds/toggle",
        &ctx.access_token,
    )
    .body(json!({"enabled": true}).to_string())
    .send()
    .await
    .expect("toggle enable with creds should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["enabled"], true);
    assert!(body.get("id").is_some(), "toggle response should have 'id'");
    assert!(body.get("slug").is_some(), "toggle response should have 'slug'");
    assert!(body.get("message").is_some(), "toggle response should have 'message'");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "toggle-valid-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-toggle-valid@contract-test.local").await;
}

// ===========================================================================
// 9. Credential status tests
// ===========================================================================

#[tokio::test]
async fn credential_status_reports_correct_shape() {
    let ctx = setup_auth_context("cred-status").await;
    if ctx.is_none() {
        eprintln!("SKIP: credential_status_reports_correct_shape — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create a datasource
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Status DS", "slug": "status-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();

    // Get credential status
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/credential-status",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("credential status should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // Verify top-level shape
    assert!(body.get("datasources").is_some(), "missing 'datasources' array");
    assert!(body["datasources"].is_array(), "'datasources' should be array");
    assert!(body.get("summary").is_some(), "missing 'summary' object");

    // Verify summary shape
    let summary = &body["summary"];
    assert!(summary.get("total").is_some(), "summary missing 'total'");
    assert!(summary.get("ready").is_some(), "summary missing 'ready'");
    assert!(summary.get("needs_credentials").is_some(), "summary missing 'needs_credentials'");
    assert!(summary.get("needs_oauth").is_some(), "summary missing 'needs_oauth'");
    assert!(summary.get("needs_password").is_some(), "summary missing 'needs_password'");

    // Verify each datasource status entry
    let datasources = body["datasources"].as_array().unwrap();
    for ds in datasources {
        assert!(ds.get("id").is_some(), "status entry missing 'id'");
        assert!(ds.get("slug").is_some(), "status entry missing 'slug'");
        assert!(ds.get("name").is_some(), "status entry missing 'name'");
        assert!(ds.get("datasource_type").is_some(), "status entry missing 'datasource_type'");
        assert!(ds.get("credential_status").is_some(), "status entry missing 'credential_status'");
        assert!(ds.get("auth_method").is_some(), "status entry missing 'auth_method'");
        assert!(ds.get("user_enabled").is_some(), "status entry missing 'user_enabled'");
        assert!(ds.get("can_enable").is_some(), "status entry missing 'can_enable'");
        assert!(ds.get("connection_config").is_some(), "status entry missing 'connection_config'");
    }

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "status-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-cred-status@contract-test.local").await;
}

#[tokio::test]
async fn credential_status_missing_shows_missing() {
    let ctx = setup_auth_context("cred-missing").await;
    if ctx.is_none() {
        eprintln!("SKIP: credential_status_missing_shows_missing — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create personal-auth datasource (no shared creds, no user creds)
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Missing Cred DS", "slug": "missing-cred-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/credential-status",
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();
    let body: Value = resp.json().await.unwrap();

    let ds_entry = body["datasources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["slug"] == "missing-cred-ds")
        .expect("should find the datasource in status");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "missing-cred-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-cred-missing@contract-test.local").await;

    assert_eq!(
        ds_entry["credential_status"], "missing",
        "datasource without credentials should have 'missing' status"
    );
}

#[tokio::test]
async fn credential_status_shared_shows_shared() {
    let ctx = setup_auth_context("cred-shared").await;
    if ctx.is_none() {
        eprintln!("SKIP: credential_status_shared_shows_shared — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create shared-auth datasource
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(
            json!({
                "name": "Shared Cred DS",
                "slug": "shared-cred-ds",
                "datasource_type": "clickhouse",
                "connection_config": {
                    "shared_credentials": true,
                    "shared_username": "reader",
                    "shared_password": "pass"
                }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/credential-status",
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();
    let body: Value = resp.json().await.unwrap();

    let ds_entry = body["datasources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["slug"] == "shared-cred-ds")
        .expect("should find the datasource in status");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "shared-cred-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-cred-shared@contract-test.local").await;

    assert_eq!(
        ds_entry["credential_status"], "shared",
        "shared-auth datasource should have 'shared' status"
    );
}

// ===========================================================================
// 10. Test connection tests
// ===========================================================================

#[tokio::test]
async fn standalone_test_connection_accepts_correct_shape() {
    let ctx = setup_auth_context("test-conn-standalone").await;
    if ctx.is_none() {
        eprintln!("SKIP: standalone_test_connection_accepts_correct_shape — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/test-connection",
        &ctx.access_token,
    )
    .body(
        json!({
            "datasource_type": "postgres",
            "connection_config": {"host": "localhost", "port": 5432},
            "credentials": {"username": "test", "password": "test"}
        })
        .to_string(),
    )
    .send()
    .await
    .expect("standalone test-connection should accept the request");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // Response shape must have success and message
    assert!(body.get("success").is_some(), "response missing 'success'");
    assert!(body["success"].is_boolean(), "'success' should be boolean");
    assert!(body.get("message").is_some(), "response missing 'message'");
    assert!(body["message"].is_string(), "'message' should be string");

    cleanup_test_user(&ctx.db, "ds-test-test-conn-standalone@contract-test.local").await;
}

#[tokio::test]
async fn per_datasource_test_connection_accepts_correct_shape() {
    let ctx = setup_auth_context("test-conn-ds").await;
    if ctx.is_none() {
        eprintln!("SKIP: per_datasource_test_connection — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create a datasource first
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Test Conn DS", "slug": "test-conn-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/test-conn-ds/test",
        &ctx.access_token,
    )
    .body(json!({}).to_string())
    .send()
    .await
    .expect("per-ds test should accept the request");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("success").is_some(), "response missing 'success'");
    assert!(body.get("message").is_some(), "response missing 'message'");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "test-conn-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-test-conn-ds@contract-test.local").await;
}

// ===========================================================================
// 11. SSH key generation tests
// ===========================================================================

#[tokio::test]
async fn generate_ssh_key_for_postgres_returns_public_key() {
    let ctx = setup_auth_context("ssh-pg").await;
    if ctx.is_none() {
        eprintln!("SKIP: generate_ssh_key_for_postgres — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create postgres datasource
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "SSH PG DS", "slug": "ssh-pg-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/ssh-pg-ds/generate-ssh-key",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("SSH key generation should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    assert!(body.get("public_key").is_some(), "missing 'public_key'");
    assert!(body["public_key"].is_string(), "'public_key' should be string");
    let pubkey = body["public_key"].as_str().unwrap();
    assert!(pubkey.starts_with("ssh-ed25519 "), "public_key should start with 'ssh-ed25519'");

    assert!(body.get("key_type").is_some(), "missing 'key_type'");
    assert_eq!(body["key_type"], "ed25519", "key_type should be 'ed25519'");

    assert!(body.get("message").is_some(), "missing 'message'");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "ssh-pg-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-ssh-pg@contract-test.local").await;
}

#[tokio::test]
async fn generate_ssh_key_for_non_postgres_returns_400() {
    let ctx = setup_auth_context("ssh-non-pg").await;
    if ctx.is_none() {
        eprintln!("SKIP: generate_ssh_key_for_non_postgres — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create MySQL datasource
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "SSH MySQL DS", "slug": "ssh-mysql-ds", "datasource_type": "mysql"}).to_string())
        .send()
        .await
        .unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/ssh-mysql-ds/generate-ssh-key",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("SSH key for non-postgres should return error");

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "ssh-mysql-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-ssh-non-pg@contract-test.local").await;

    assert_eq!(status, 400, "SSH key gen for non-postgres should return 400");
    assert!(body.get("detail").is_some(), "should have error detail");
}

#[tokio::test]
async fn generate_ssh_key_non_admin_returns_403() {
    let ctx = setup_auth_context("ssh-non-admin").await;
    if ctx.is_none() {
        eprintln!("SKIP: generate_ssh_key_non_admin — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create postgres datasource as admin
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "SSH Admin DS", "slug": "ssh-admin-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();

    // Create non-admin user
    let member_email = "ds-test-ssh-member@contract-test.local";
    cleanup_test_user(&ctx.db, member_email).await;
    let member = kyomi_auth::user_service::create_user(&ctx.db, member_email, Some("SSH Member"), true)
        .await
        .unwrap();
    {
        let active_val = if ctx.db.is_postgres() { "true" } else { "1" };
        let sql = format!(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active) VALUES ($1, $2, 'workspace_user', {active_val})"
        );
        kyomi_core::db_execute!(
            &ctx.db,
            &sql,
            &ctx.workspace_id,
            &member.user_id
        )
        .unwrap();
    }

    let mut extra = HashMap::new();
    extra.insert("user_id".to_string(), json!(member.user_id));
    extra.insert("email".to_string(), json!(member_email));
    extra.insert("workspace_id".to_string(), json!(ctx.workspace_id));
    extra.insert("workspace_roles".to_string(), json!(["member"]));
    let member_token = kyomi_auth::jwt::create_access_token_str(
        &member.user_id,
        &ctx.jwt_secret,
        60,
        extra,
    )
    .unwrap();

    // Try SSH key gen as non-admin
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/ssh-admin-ds/generate-ssh-key",
        &member_token,
    )
    .send()
    .await
    .unwrap();

    let status = resp.status();

    cleanup_test_user(&ctx.db, member_email).await;
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "ssh-admin-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-ssh-non-admin@contract-test.local").await;

    assert_eq!(status, 403, "non-admin SSH key gen should return 403");
}

// ===========================================================================
// 12. Affected users tests
// ===========================================================================

#[tokio::test]
async fn affected_users_no_auth_mode_change_returns_0() {
    let ctx = setup_auth_context("affected-same").await;
    if ctx.is_none() {
        eprintln!("SKIP: affected_users_no_auth_mode_change — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create datasource with a specific auth_mode
    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(
            json!({
                "name": "Affected DS",
                "slug": "affected-ds",
                "datasource_type": "bigquery",
                "connection_config": {
                    "auth_mode": "kyomi_oauth"
                }
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    // Query affected users for same auth mode
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/affected-ds/affected-users?new_auth_mode=kyomi_oauth",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("affected users should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    assert_eq!(body["affected_count"], 0, "same auth mode should affect 0 users");
    assert!(body.get("affected_users").is_some(), "missing 'affected_users'");
    assert!(body["affected_users"].is_array(), "'affected_users' should be array");
    assert!(body["warning_message"].is_null(), "no warning when 0 affected");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "affected-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-affected-same@contract-test.local").await;
}

#[tokio::test]
async fn affected_users_response_has_correct_shape() {
    let ctx = setup_auth_context("affected-shape").await;
    if ctx.is_none() {
        eprintln!("SKIP: affected_users_response_has_correct_shape — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Affected Shape DS", "slug": "affected-shape-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/affected-shape-ds/affected-users?new_auth_mode=password",
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // Verify shape
    assert!(body.get("affected_count").is_some(), "missing 'affected_count'");
    assert!(body["affected_count"].is_number(), "'affected_count' should be number");
    assert!(body.get("affected_users").is_some(), "missing 'affected_users'");
    assert!(body["affected_users"].is_array(), "'affected_users' should be array");
    // warning_message can be null or string
    assert!(
        body.get("warning_message").is_some(),
        "missing 'warning_message' field"
    );

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "affected-shape-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-affected-shape@contract-test.local").await;
}

#[tokio::test]
async fn affected_users_non_admin_returns_403() {
    let ctx = setup_auth_context("affected-403").await;
    if ctx.is_none() {
        eprintln!("SKIP: affected_users_non_admin — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Affected 403 DS", "slug": "affected-403-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();

    // Create non-admin
    let member_email = "ds-test-affected-member@contract-test.local";
    cleanup_test_user(&ctx.db, member_email).await;
    let member = kyomi_auth::user_service::create_user(&ctx.db, member_email, Some("Affected Member"), true)
        .await
        .unwrap();
    {
        let active_val = if ctx.db.is_postgres() { "true" } else { "1" };
        let sql = format!(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active) VALUES ($1, $2, 'workspace_user', {active_val})"
        );
        kyomi_core::db_execute!(
            &ctx.db,
            &sql,
            &ctx.workspace_id,
            &member.user_id
        )
        .unwrap();
    }

    let mut extra = HashMap::new();
    extra.insert("user_id".to_string(), json!(member.user_id));
    extra.insert("email".to_string(), json!(member_email));
    extra.insert("workspace_id".to_string(), json!(ctx.workspace_id));
    extra.insert("workspace_roles".to_string(), json!(["member"]));
    let member_token = kyomi_auth::jwt::create_access_token_str(
        &member.user_id,
        &ctx.jwt_secret,
        60,
        extra,
    )
    .unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/affected-403-ds/affected-users?new_auth_mode=password",
        &member_token,
    )
    .send()
    .await
    .unwrap();

    let status = resp.status();

    cleanup_test_user(&ctx.db, member_email).await;
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "affected-403-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-affected-403@contract-test.local").await;

    assert_eq!(status, 403, "non-admin affected users should return 403");
}

// ===========================================================================
// 13. Sample datasource tests
// ===========================================================================

#[tokio::test]
async fn sample_available_returns_configured_and_already_added_flags() {
    let ctx = setup_auth_context("sample-avail").await;
    if ctx.is_none() {
        eprintln!("SKIP: sample_available_returns_flags — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/sample/available",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("sample available should succeed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    assert!(body.get("configured").is_some(), "missing 'configured'");
    assert!(body["configured"].is_boolean(), "'configured' should be boolean");
    assert!(body.get("already_added").is_some(), "missing 'already_added'");
    assert!(body["already_added"].is_boolean(), "'already_added' should be boolean");

    cleanup_test_user(&ctx.db, "ds-test-sample-avail@contract-test.local").await;
}

#[tokio::test]
async fn create_sample_when_configured_returns_201() {
    // This test only works if SAMPLE_CLICKHOUSE_HOST is configured
    if std::env::var("SAMPLE_CLICKHOUSE_HOST").is_err() {
        eprintln!("SKIP: create_sample_when_configured — SAMPLE_CLICKHOUSE_HOST not set");
        return;
    }

    let ctx = setup_auth_context("sample-create").await;
    if ctx.is_none() {
        eprintln!("SKIP: create_sample_when_configured — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/sample", &ctx.access_token)
        .send()
        .await
        .expect("create sample should succeed");

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();

    // Clean up (the sample datasource slug is always "acme-analytics-sample")
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "acme-analytics-sample").await;
    cleanup_test_user(&ctx.db, "ds-test-sample-create@contract-test.local").await;

    assert_eq!(status, 201, "create sample should return 201");
    assert_eq!(body["datasource_type"], "clickhouse");
}

#[tokio::test]
async fn create_sample_twice_returns_409() {
    if std::env::var("SAMPLE_CLICKHOUSE_HOST").is_err() {
        eprintln!("SKIP: create_sample_twice — SAMPLE_CLICKHOUSE_HOST not set");
        return;
    }

    let ctx = setup_auth_context("sample-dup").await;
    if ctx.is_none() {
        eprintln!("SKIP: create_sample_twice — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create first
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/sample", &ctx.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201, "first sample should succeed");

    // Create second
    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/sample", &ctx.access_token)
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "acme-analytics-sample").await;
    cleanup_test_user(&ctx.db, "ds-test-sample-dup@contract-test.local").await;

    assert_eq!(status, 409, "duplicate sample should return 409");
    assert!(body.get("detail").is_some(), "conflict should have 'detail'");
}

// ===========================================================================
// 14. Resolver tests (tested via HTTP — same as get by slug/uuid)
// ===========================================================================

#[tokio::test]
async fn resolver_by_slug_works() {
    let ctx = setup_auth_context("resolve-slug").await;
    if ctx.is_none() {
        eprintln!("SKIP: resolver_by_slug_works — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Resolve Slug DS", "slug": "resolve-slug-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/resolve-slug-ds",
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200, "resolve by slug should succeed");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "resolve-slug-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-resolve-slug@contract-test.local").await;
}

#[tokio::test]
async fn resolver_by_uuid_works() {
    let ctx = setup_auth_context("resolve-uuid").await;
    if ctx.is_none() {
        eprintln!("SKIP: resolver_by_uuid_works — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(&ctx.base_url, "/api/v1/datasources/", &ctx.access_token)
        .body(json!({"name": "Resolve UUID DS", "slug": "resolve-uuid-ds", "datasource_type": "postgres"}).to_string())
        .send()
        .await
        .unwrap();
    let created: Value = resp.json().await.unwrap();
    let ds_id = created["id"].as_str().unwrap();

    let resp = auth_get(
        &ctx.base_url,
        &format!("/api/v1/datasources/{ds_id}"),
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200, "resolve by UUID should succeed");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["id"], ds_id, "resolved id should match");

    cleanup_datasource(&ctx.db, &ctx.workspace_id, "resolve-uuid-ds").await;
    cleanup_test_user(&ctx.db, "ds-test-resolve-uuid@contract-test.local").await;
}

#[tokio::test]
async fn resolver_nonexistent_returns_error() {
    let ctx = setup_auth_context("resolve-404").await;
    if ctx.is_none() {
        eprintln!("SKIP: resolver_nonexistent_returns_error — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/this-does-not-exist",
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 404, "nonexistent should return 404");
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "404 should have 'detail'");

    cleanup_test_user(&ctx.db, "ds-test-resolve-404@contract-test.local").await;
}

// ===========================================================================
// 12. SQL Query History tests
// ===========================================================================

#[tokio::test]
async fn sql_history_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/api/v1/sql/history"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "GET /sql/history without auth should be 401");
}

#[tokio::test]
async fn sql_history_create_and_list() {
    if is_python_mode() {
        eprintln!("SKIP: sql_history_create_and_list — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("sql-hist-crud").await.unwrap();

    // Create a history record
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/sql/history",
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({
        "query_text": "SELECT 1",
        "execution_time_ms": 42,
        "bytes_processed": 1024,
        "row_count": 1,
        "status": "success"
    })).unwrap())
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 201, "POST /sql/history should return 201 Created");
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("query_id").is_some(), "response must have query_id");
    assert_eq!(body["query_text"], "SELECT 1");
    assert_eq!(body["status"], "success");
    assert_eq!(body["execution_time_ms"], 42);
    assert_eq!(body["bytes_processed"], 1024);
    assert_eq!(body["row_count"], 1);
    assert_eq!(body["is_saved"], false);
    assert!(body.get("created_at").is_some(), "must have created_at");
    assert!(body.get("updated_at").is_some(), "must have updated_at");
    assert!(body.get("executed_at").is_some(), "must have executed_at");

    let query_id = body["query_id"].as_str().unwrap().to_string();

    // List history — should contain the record
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/sql/history",
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200, "GET /sql/history should be 200");
    let body: Value = resp.json().await.unwrap();
    let list = body.as_array().expect("response should be an array");
    assert!(!list.is_empty(), "list should contain at least one item");

    let found = list.iter().any(|item| item["query_id"].as_str() == Some(&query_id));
    assert!(found, "created query should appear in list");

    // Get single record
    let resp = auth_get(
        &ctx.base_url,
        &format!("/api/v1/sql/history/{query_id}"),
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200, "GET /sql/history/id should be 200");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["query_id"], query_id);
    assert_eq!(body["query_text"], "SELECT 1");

    cleanup_test_user(&ctx.db, "ds-test-sql-hist-crud@contract-test.local").await;
}

#[tokio::test]
async fn sql_history_update_saved_name_tags() {
    if is_python_mode() {
        eprintln!("SKIP: sql_history_update_saved_name_tags — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("sql-hist-update").await.unwrap();

    // Create a record first
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/sql/history",
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({
        "query_text": "SELECT * FROM orders",
        "status": "success"
    })).unwrap())
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 201);
    let body: Value = resp.json().await.unwrap();
    let query_id = body["query_id"].as_str().unwrap().to_string();
    assert_eq!(body["is_saved"], false);
    assert!(body["query_name"].is_null());
    assert!(body["tags"].is_null());

    // Update via PATCH
    let resp = auth_patch(
        &ctx.base_url,
        &format!("/api/v1/sql/history/{query_id}"),
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({
        "is_saved": true,
        "query_name": "My Orders Query",
        "tags": "revenue,monthly"
    })).unwrap())
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200, "PATCH /sql/history/id should be 200");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["is_saved"], true);
    assert_eq!(body["query_name"], "My Orders Query");
    assert_eq!(body["tags"], "revenue,monthly");

    cleanup_test_user(&ctx.db, "ds-test-sql-hist-update@contract-test.local").await;
}

#[tokio::test]
async fn sql_history_delete() {
    if is_python_mode() {
        eprintln!("SKIP: sql_history_delete — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("sql-hist-del").await.unwrap();

    // Create a record
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/sql/history",
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({
        "query_text": "SELECT 1",
        "status": "success"
    })).unwrap())
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 201);
    let body: Value = resp.json().await.unwrap();
    let query_id = body["query_id"].as_str().unwrap().to_string();

    // Delete it
    let resp = auth_delete(
        &ctx.base_url,
        &format!("/api/v1/sql/history/{query_id}"),
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200, "DELETE /sql/history/id should be 200");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);

    // Verify it's gone
    let resp = auth_get(
        &ctx.base_url,
        &format!("/api/v1/sql/history/{query_id}"),
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 404, "deleted query should return 404");

    cleanup_test_user(&ctx.db, "ds-test-sql-hist-del@contract-test.local").await;
}

#[tokio::test]
async fn sql_history_not_found() {
    if is_python_mode() {
        eprintln!("SKIP: sql_history_not_found — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("sql-hist-404").await.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/sql/history/00000000-0000-0000-0000-000000000000",
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 404, "nonexistent query should return 404");
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "404 should have 'detail'");

    cleanup_test_user(&ctx.db, "ds-test-sql-hist-404@contract-test.local").await;
}

#[tokio::test]
async fn sql_history_search_filter() {
    if is_python_mode() {
        eprintln!("SKIP: sql_history_search_filter — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("sql-hist-search").await.unwrap();

    // Create two records with different query text
    auth_post(&ctx.base_url, "/api/v1/sql/history", &ctx.access_token)
        .body(serde_json::to_string(&json!({
            "query_text": "SELECT * FROM unique_table_abc",
            "status": "success"
        })).unwrap())
        .send()
        .await
        .unwrap();

    auth_post(&ctx.base_url, "/api/v1/sql/history", &ctx.access_token)
        .body(serde_json::to_string(&json!({
            "query_text": "SELECT * FROM other_table",
            "status": "success"
        })).unwrap())
        .send()
        .await
        .unwrap();

    // Search for unique_table_abc
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/sql/history?search=unique_table_abc",
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let list = body.as_array().expect("response should be array");
    assert_eq!(list.len(), 1, "search should return exactly 1 result");
    assert!(
        list[0]["query_text"].as_str().unwrap().contains("unique_table_abc"),
        "search result should match"
    );

    cleanup_test_user(&ctx.db, "ds-test-sql-hist-search@contract-test.local").await;
}

#[tokio::test]
async fn sql_history_saved_only_filter() {
    if is_python_mode() {
        eprintln!("SKIP: sql_history_saved_only_filter — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("sql-hist-saved").await.unwrap();

    // Create a record and save it
    let resp = auth_post(&ctx.base_url, "/api/v1/sql/history", &ctx.access_token)
        .body(serde_json::to_string(&json!({
            "query_text": "SELECT saved_query",
            "status": "success"
        })).unwrap())
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let saved_id = body["query_id"].as_str().unwrap().to_string();

    // Mark it as saved
    auth_patch(
        &ctx.base_url,
        &format!("/api/v1/sql/history/{saved_id}"),
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({"is_saved": true})).unwrap())
    .send()
    .await
    .unwrap();

    // Create an unsaved record
    auth_post(&ctx.base_url, "/api/v1/sql/history", &ctx.access_token)
        .body(serde_json::to_string(&json!({
            "query_text": "SELECT unsaved_query",
            "status": "success"
        })).unwrap())
        .send()
        .await
        .unwrap();

    // Filter by saved_only
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/sql/history?saved_only=true",
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let list = body.as_array().expect("response should be array");
    assert_eq!(list.len(), 1, "saved_only should return exactly 1 result");
    assert_eq!(list[0]["is_saved"], true);

    cleanup_test_user(&ctx.db, "ds-test-sql-hist-saved@contract-test.local").await;
}

#[tokio::test]
async fn sql_history_pagination() {
    if is_python_mode() {
        eprintln!("SKIP: sql_history_pagination — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("sql-hist-page").await.unwrap();

    // Create 3 records
    for i in 0..3 {
        auth_post(&ctx.base_url, "/api/v1/sql/history", &ctx.access_token)
            .body(serde_json::to_string(&json!({
                "query_text": format!("SELECT {i}"),
                "status": "success"
            })).unwrap())
            .send()
            .await
            .unwrap();
    }

    // Get with limit=2
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/sql/history?limit=2",
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let list = body.as_array().expect("response should be array");
    assert_eq!(list.len(), 2, "limit=2 should return 2 results");

    // Get with offset=2
    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/sql/history?limit=10&offset=2",
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let list = body.as_array().expect("response should be array");
    assert_eq!(list.len(), 1, "offset=2 with 3 records should return 1");

    cleanup_test_user(&ctx.db, "ds-test-sql-hist-page@contract-test.local").await;
}

// ===========================================================================
// 13. Query Execute tests
// ===========================================================================

#[tokio::test]
async fn query_execute_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/datasources/query/execute"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"sql": "SELECT 1", "datasource": "test"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /query/execute without auth should be 401");
}

#[tokio::test]
async fn query_execute_unknown_datasource_returns_404() {
    if is_python_mode() {
        eprintln!("SKIP: query_execute_unknown_datasource_returns_404 — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("qe-404").await.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/query/execute",
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({
        "sql": "SELECT 1",
        "datasource": "nonexistent-datasource"
    })).unwrap())
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 404, "unknown datasource should return 404");
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "404 should have 'detail'");
    let detail = body["detail"].as_str().unwrap();
    assert!(
        detail.contains("not found"),
        "error should mention 'not found', got: {detail}"
    );

    cleanup_test_user(&ctx.db, "ds-test-qe-404@contract-test.local").await;
}

// ===========================================================================
// 14. Test Connection tests (Phase 6 — real providers)
// ===========================================================================

#[tokio::test]
async fn test_connection_standalone_invalid_type() {
    if is_python_mode() {
        eprintln!("SKIP: test_connection_standalone_invalid_type — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("tc-invalid").await.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/test-connection",
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({
        "datasource_type": "not_a_real_type",
        "connection_config": {}
    })).unwrap())
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 400, "invalid datasource type should return 400");
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "error should have 'detail'");

    cleanup_test_user(&ctx.db, "ds-test-tc-invalid@contract-test.local").await;
}

#[tokio::test]
async fn test_connection_standalone_valid_type_returns_result() {
    if is_python_mode() {
        eprintln!("SKIP: test_connection_standalone_valid_type_returns_result — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("tc-valid").await.unwrap();

    // Test connection with valid type but unreachable host — should return success=false
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/test-connection",
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({
        "datasource_type": "clickhouse",
        "connection_config": {
            "host": "198.51.100.1",
            "port": 9999,
            "database": "default"
        },
        "credentials": {
            "username": "test",
            "password": "test"
        }
    })).unwrap())
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200, "test-connection should return 200");
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("success").is_some(), "response must have 'success' field");
    assert!(body.get("message").is_some(), "response must have 'message' field");
    // success should be false since the host is unreachable
    assert_eq!(body["success"], false, "unreachable host should be success=false");

    cleanup_test_user(&ctx.db, "ds-test-tc-valid@contract-test.local").await;
}

// ===========================================================================
// 15. BigQuery Access Token tests
// ===========================================================================

#[tokio::test]
async fn bigquery_access_token_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/bigquery/request-access-token"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(r#"{"datasource_slug": "test"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401, "POST /bigquery/request-access-token without auth should be 401");
}

#[tokio::test]
async fn bigquery_access_token_unknown_datasource_returns_404() {
    if is_python_mode() {
        eprintln!("SKIP: bigquery_access_token_unknown_datasource — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("bq-token-404").await.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/bigquery/request-access-token",
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({
        "datasource_slug": "nonexistent-bq"
    })).unwrap())
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 404, "unknown datasource should return 404");
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some(), "404 should have 'detail'");

    cleanup_test_user(&ctx.db, "ds-test-bq-token-404@contract-test.local").await;
}

#[tokio::test]
async fn bigquery_access_token_non_bigquery_type_returns_400() {
    if is_python_mode() {
        eprintln!("SKIP: bigquery_access_token_non_bigquery_type — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("bq-token-type").await.unwrap();

    // Create a postgres datasource
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/",
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({
        "name": "Not BigQuery",
        "slug": "not-bigquery-ds",
        "datasource_type": "postgres",
        "connection_config": {"host": "localhost", "port": 5432}
    })).unwrap())
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 201);

    // Try to get BQ access token for a postgres datasource
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/bigquery/request-access-token",
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({
        "datasource_slug": "not-bigquery-ds"
    })).unwrap())
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 400, "non-bigquery type should return 400");
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("detail").is_some());
    let detail = body["detail"].as_str().unwrap();
    assert!(
        detail.contains("not 'bigquery'"),
        "error should explain the type mismatch, got: {detail}"
    );

    cleanup_test_user(&ctx.db, "ds-test-bq-token-type@contract-test.local").await;
}

// ===========================================================================
// 16. SQL History with datasource association
// ===========================================================================

#[tokio::test]
async fn sql_history_with_datasource_slug() {
    if is_python_mode() {
        eprintln!("SKIP: sql_history_with_datasource_slug — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("sql-hist-ds").await.unwrap();

    // Create a datasource
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/",
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({
        "name": "History Test DS",
        "slug": "history-test-ds",
        "datasource_type": "postgres",
        "connection_config": {"host": "localhost", "port": 5432}
    })).unwrap())
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 201);

    // Create a history record linked to the datasource
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/sql/history",
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({
        "query_text": "SELECT * FROM linked_table",
        "status": "success",
        "datasource": "history-test-ds"
    })).unwrap())
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 201);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["datasource_slug"], "history-test-ds");
    assert!(body["datasource_id"].is_string(), "should have datasource_id");

    // Verify listing also returns the slug
    let query_id = body["query_id"].as_str().unwrap();
    let resp = auth_get(
        &ctx.base_url,
        &format!("/api/v1/sql/history/{query_id}"),
        &ctx.access_token,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["datasource_slug"], "history-test-ds");

    cleanup_test_user(&ctx.db, "ds-test-sql-hist-ds@contract-test.local").await;
}

#[tokio::test]
async fn sql_history_error_status() {
    if is_python_mode() {
        eprintln!("SKIP: sql_history_error_status — requires Rust-backend mode");
        return;
    }

    let ctx = setup_auth_context("sql-hist-err").await.unwrap();

    // Create an error history record
    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/sql/history",
        &ctx.access_token,
    )
    .body(serde_json::to_string(&json!({
        "query_text": "INVALID SQL",
        "status": "error",
        "error_message": "syntax error at position 1"
    })).unwrap())
    .send()
    .await
    .unwrap();

    assert_eq!(resp.status(), 201);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "error");
    assert_eq!(body["error_message"], "syntax error at position 1");

    cleanup_test_user(&ctx.db, "ds-test-sql-hist-err@contract-test.local").await;
}
