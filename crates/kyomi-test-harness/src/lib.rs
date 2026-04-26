// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared test harness for the `contract_*` integration tests in `apps/server`.
//!
//! Lives as a library crate (not a `tests/common/` module) so that `pub` items
//! are treated as public API and don't trigger `dead_code` in consumer test
//! binaries that only read a subset of `AuthContext` fields.

use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

/// Shared server state for authenticated tests.
///
/// When `CONTRACT_TEST_BASE_URL` is set, the harness points at an external
/// server and the optional fields are `None`.
pub struct TestServer {
    pub base_url: String,
    pub db: Option<kyomi_core::DbPool>,
    pub kv: Option<kyomi_core::KVPool>,
    pub jwt_secret: Option<String>,
    pub encryption_key: Option<Arc<[u8; 32]>>,
}

/// An authenticated test context with a user, workspace, and JWT.
pub struct AuthContext {
    pub base_url: String,
    pub access_token: String,
    pub user_id: String,
    pub workspace_id: String,
    pub db: kyomi_core::DbPool,
    pub kv: kyomi_core::KVPool,
    pub encryption_key: Arc<[u8; 32]>,
    pub jwt_secret: String,
}

/// Spin up an in-process test server (or point at `CONTRACT_TEST_BASE_URL`).
pub async fn setup_server() -> TestServer {
    if let Ok(url) = std::env::var("CONTRACT_TEST_BASE_URL") {
        return TestServer {
            base_url: url,
            db: None,
            kv: None,
            jwt_secret: None,
            encryption_key: None,
        };
    }

    if let Ok(path) = kyomi_core::constants::find_constants_file() {
        let _ = kyomi_core::constants::load(&path);
    }

    let config = kyomi_core::Config::test_config();
    let db = kyomi_core::db::create_pool(&config.database_url)
        .await
        .expect("test DB should be running (docker compose up)");
    let kv: kyomi_core::KVPool =
        kyomi_core::kv_store::create_kv_store(config.redis_url.as_deref())
            .await
            .expect("failed to create KV store");

    let encryption_key = kyomi_auth::encryption::derive_key(&config.encryption_key)
        .expect("test encryption key should be valid base64url");

    let rp_origin =
        url::Url::parse(&config.frontend_url).expect("frontend_url must be a valid URL");
    let webauthn = kyomi_auth::webauthn::build_webauthn(
        &config.webauthn_rp_id,
        &config.webauthn_rp_name,
        &rp_origin,
    )
    .expect("webauthn build");

    let jwt_secret = config.jwt_secret.clone();
    let encryption_key_arc = Arc::new(encryption_key);

    let ws_manager = kyomi_auth::websocket::WebSocketManager::new(None, db.clone());

    let state = kyomi_server::state::AppState {
        db: db.clone(),
        kv: kv.clone(),
        redis: None,
        config: Arc::new(config.clone()),
        encryption_key: encryption_key_arc.clone(),
        webauthn: Arc::new(webauthn),
        embedding: kyomi_embed::LazyEmbedding::loaded(
            kyomi_embed::EmbeddingService::new().expect("embedding model"),
        ),
        ws_manager,
        stripe: None,
        mcp_sessions: kyomi_auth::mcp_session_manager::MCPSessionManager::new(kv.clone()),
        cancel_registry: kyomi_server::cancel_registry::CancelRegistry::default(),
        connect_token: None,
        connect_registry: kyomi_server::connect::registry::ConnectRegistry::new_local(),
        platforms: Arc::new(kyomi_core::platform::PlatformRegistry::new()),
    };

    let app = kyomi_server::build_service(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind test server");
    let addr = listener.local_addr().expect("failed to get local addr");

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("test server exited with error");
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    TestServer {
        base_url: format!("http://{addr}"),
        db: Some(db),
        kv: Some(kv),
        jwt_secret: Some(jwt_secret),
        encryption_key: Some(encryption_key_arc),
    }
}

/// Base URL for unauthenticated tests.
pub async fn base_url() -> String {
    setup_server().await.base_url
}

/// Create an authenticated test context with a unique admin user and workspace.
///
/// `display_name` and `email_prefix` vary per test file so parallel runs don't
/// collide (e.g. `"Billing Test User"` + `"bill"` produces emails of the form
/// `bill-test-<suffix>@contract-test.local`).
pub async fn setup_auth_context(
    display_name: &str,
    email_prefix: &str,
    suffix: &str,
) -> Option<AuthContext> {
    let server = setup_server().await;
    let db = server.db?;
    let kv = server.kv.expect("kv should be set when db is set");
    let jwt_secret = server
        .jwt_secret
        .expect("jwt_secret should be set in Rust mode");
    let encryption_key = server
        .encryption_key
        .expect("encryption_key should be set in Rust mode");

    let email = format!("{email_prefix}-test-{suffix}@contract-test.local");

    cleanup_test_user(&db, &email).await;

    let user = kyomi_auth::user_service::create_user(&db, &email, Some(display_name), true)
        .await
        .expect("should create test user");

    let workspace_id = kyomi_auth::user_service::create_workspace_for_user(
        &db,
        &user.user_id,
        Some(display_name),
        &email,
        None,
    )
    .await
    .expect("should create test workspace");

    let mut extra = HashMap::new();
    extra.insert("user_id".to_string(), json!(user.user_id));
    extra.insert("email".to_string(), json!(email));
    extra.insert("name".to_string(), json!(display_name));
    extra.insert("workspace_id".to_string(), json!(workspace_id));
    extra.insert("workspace_roles".to_string(), json!(["workspace_admin"]));

    let access_token =
        kyomi_auth::jwt::create_access_token_str(&user.user_id, &jwt_secret, 60, extra)
            .expect("should create access token");

    Some(AuthContext {
        base_url: server.base_url,
        access_token,
        user_id: user.user_id,
        workspace_id,
        db,
        kv,
        encryption_key,
        jwt_secret,
    })
}

/// Remove every trace of a test user: owned workspaces, memberships, and any
/// per-workspace domain data (datasources, feedback, SQL history, …).
///
/// Deletes are ordered for FK safety and are idempotent — missing rows are
/// no-ops, so a test that only touches some of these tables still cleans up
/// fine.
pub async fn cleanup_test_user(db: &kyomi_core::DbPool, email: &str) {
    let user_id: Option<String> = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE email = $1")
                .bind(email)
                .fetch_optional(pg)
                .await
                .unwrap_or(None)
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE email = $1")
                .bind(email)
                .fetch_optional(sq)
                .await
                .unwrap_or(None)
        }
    };

    let Some(uid) = user_id else { return };

    let _ = kyomi_core::db_execute!(db, "DELETE FROM feedback WHERE user_id = $1", &uid);

    let workspace_ids: Vec<String> = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            sqlx::query_scalar::<_, String>(
                "SELECT workspace_id FROM workspaces WHERE owner_user_id = $1",
            )
            .bind(&uid)
            .fetch_all(pg)
            .await
            .unwrap_or_default()
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            sqlx::query_scalar::<_, String>(
                "SELECT workspace_id FROM workspaces WHERE owner_user_id = $1",
            )
            .bind(&uid)
            .fetch_all(sq)
            .await
            .unwrap_or_default()
        }
    };

    for ws_id in &workspace_ids {
        let _ = kyomi_core::db_execute!(
            db,
            "DELETE FROM sql_query_history WHERE workspace_id = $1",
            ws_id
        );
        let _ = kyomi_core::db_execute!(
            db,
            "DELETE FROM datasource_search_embeddings WHERE datasource_config_id IN \
             (SELECT id FROM datasource_configs WHERE workspace_id = $1)",
            ws_id
        );
        let _ = kyomi_core::db_execute!(
            db,
            "DELETE FROM datasource_table_cache WHERE datasource_config_id IN \
             (SELECT id FROM datasource_configs WHERE workspace_id = $1)",
            ws_id
        );
        let _ = kyomi_core::db_execute!(
            db,
            "DELETE FROM user_datasource_preferences WHERE datasource_config_id IN \
             (SELECT id FROM datasource_configs WHERE workspace_id = $1)",
            ws_id
        );
        let _ = kyomi_core::db_execute!(
            db,
            "DELETE FROM user_datasource_credentials WHERE workspace_id = $1",
            ws_id
        );
        let _ = kyomi_core::db_execute!(
            db,
            "DELETE FROM datasource_configs WHERE workspace_id = $1",
            ws_id
        );
        let _ = kyomi_core::db_execute!(
            db,
            "DELETE FROM workspace_users WHERE workspace_id = $1",
            ws_id
        );
        let _ =
            kyomi_core::db_execute!(db, "DELETE FROM workspaces WHERE workspace_id = $1", ws_id);
    }

    let _ = kyomi_core::db_execute!(
        db,
        "DELETE FROM workspace_users WHERE user_id = $1",
        &uid
    );
    let _ = kyomi_core::db_execute!(db, "DELETE FROM users WHERE user_id = $1", &uid);
}
