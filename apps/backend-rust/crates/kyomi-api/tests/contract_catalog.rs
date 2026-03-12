// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for catalog management endpoints.
//!
//! These tests verify the HTTP-level contract (request/response shapes, headers,
//! status codes) for the 6 catalog endpoints:
//!
//! 1. `POST /api/v1/datasources/discover-catalog` — Discover catalog items
//! 2. `POST /api/v1/datasources/discover` — Discover ALL resources
//! 3. `GET /api/v1/datasources/{id}/catalog/tree` — Hierarchical tree from cache
//! 4. `GET /api/v1/datasources/{id}/catalog/status` — Indexing status + stats
//! 5. `GET /api/v1/datasources/{id}/schemas` — Live schema list
//! 6. `POST /api/v1/datasources/{id}/catalog/refresh` — Trigger manual refresh
//!
//! Test organization:
//! - Section 1: Unauthenticated 401 tests (all 6 endpoints)
//! - Section 2: Authenticated response shape tests (tree, status)
//! - Section 3: Catalog tree hierarchical structure test
//! - Section 4: Catalog refresh admin-only test
//!
//! Authenticated tests create temporary users/workspaces/datasources in the DB
//! and clean up after themselves. They only run in Rust-backend mode.

use serde_json::{json, Value};
use std::collections::HashMap;

// ===========================================================================
// Test infrastructure
// ===========================================================================

/// Shared server state for authenticated tests.
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
    encryption_key: std::sync::Arc<[u8; 32]>,
    jwt_secret: String,
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

    // Load shared constants (idempotent)
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

    let rp_origin =
        url::Url::parse(&config.frontend_url).expect("frontend_url must be a valid URL");
    let webauthn = kyomi_auth::webauthn::build_webauthn(
        &config.webauthn_rp_id,
        &config.webauthn_rp_name,
        &rp_origin,
    )
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

/// Get the base URL for unauthenticated tests.
async fn base_url() -> String {
    setup_server().await.base_url
}

/// Create an authenticated test context with a unique admin user and workspace.
async fn setup_auth_context(suffix: &str) -> Option<AuthContext> {
    let server = setup_server().await;
    let db = server.db?;
    let jwt_secret = server
        .jwt_secret
        .expect("jwt_secret should be set in Rust mode");
    let encryption_key = server
        .encryption_key
        .expect("encryption_key should be set in Rust mode");

    let email = format!("cat-test-{suffix}@contract-test.local");

    // Clean up any leftover test data from a previous run
    cleanup_test_user(&db, &email).await;

    // Create a verified user
    let user = kyomi_auth::user_service::create_user(&db, &email, Some("Catalog Test User"), true)
        .await
        .expect("should create test user");

    // Create a workspace (user becomes admin + owner)
    let workspace_id = kyomi_auth::user_service::create_workspace_for_user(
        &db,
        &user.user_id,
        Some("Catalog Test User"),
        &email,
    )
    .await
    .expect("should create test workspace");

    // Mint a JWT with workspace context
    let mut extra = HashMap::new();
    extra.insert("user_id".to_string(), json!(user.user_id));
    extra.insert("email".to_string(), json!(email));
    extra.insert("name".to_string(), json!("Catalog Test User"));
    extra.insert("workspace_id".to_string(), json!(workspace_id));
    extra.insert(
        "workspace_roles".to_string(),
        json!(["workspace_admin"]),
    );

    let access_token = kyomi_auth::jwt::create_access_token_str(
        &user.user_id,
        &jwt_secret,
        60,
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

/// Create a non-admin member token within an existing admin context.
///
/// This adds a new user to the admin's workspace with role "member" (not admin,
/// not owner). The middleware does a DB lookup for `is_owner` and `workspace_roles`,
/// so JWT claims alone are insufficient — the user must actually be a non-owner
/// member in the DB.
async fn create_member_token(admin_ctx: &AuthContext, suffix: &str) -> (String, String) {
    let member_email = format!("cat-member-{suffix}@contract-test.local");

    cleanup_test_user(&admin_ctx.db, &member_email).await;

    let member_user = kyomi_auth::user_service::create_user(
        &admin_ctx.db,
        &member_email,
        Some("Catalog Member User"),
        true,
    )
    .await
    .expect("should create member user");

    // Add member to admin's workspace with "user" role (not owner, not admin)
    kyomi_core::db_execute!(
        &admin_ctx.db,
        "INSERT INTO workspace_users (user_id, workspace_id, role) VALUES ($1, $2, 'workspace_user')",
        &member_user.user_id,
        &admin_ctx.workspace_id
    )
    .expect("should add member to workspace");

    // Mint JWT for the member pointing to the admin's workspace
    let mut extra = HashMap::new();
    extra.insert("user_id".to_string(), json!(member_user.user_id));
    extra.insert("email".to_string(), json!(member_email));
    extra.insert("name".to_string(), json!("Catalog Member User"));
    extra.insert(
        "workspace_id".to_string(),
        json!(admin_ctx.workspace_id),
    );
    extra.insert("workspace_roles".to_string(), json!(["user"]));

    let access_token = kyomi_auth::jwt::create_access_token_str(
        &member_user.user_id,
        &admin_ctx.jwt_secret,
        60,
        extra,
    )
    .expect("should create member access token");

    (access_token, member_email.to_string())
}

/// Clean up a test user and all related data.
async fn cleanup_test_user(db: &kyomi_core::DbPool, email: &str) {
    let user_id: Option<String> = match db {
        kyomi_core::db::DbPool::Postgres(pg) =>
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE email = $1")
                .bind(email).fetch_optional(pg).await.unwrap_or(None),
        kyomi_core::db::DbPool::Sqlite(sq) =>
            sqlx::query_scalar::<_, String>("SELECT user_id FROM users WHERE email = $1")
                .bind(email).fetch_optional(sq).await.unwrap_or(None),
    };

    if let Some(uid) = user_id {
        let workspace_ids: Vec<String> = match db {
            kyomi_core::db::DbPool::Postgres(pg) =>
                sqlx::query_scalar::<_, String>("SELECT workspace_id FROM workspaces WHERE owner_user_id = $1")
                    .bind(&uid).fetch_all(pg).await.unwrap_or_default(),
            kyomi_core::db::DbPool::Sqlite(sq) =>
                sqlx::query_scalar::<_, String>("SELECT workspace_id FROM workspaces WHERE owner_user_id = $1")
                    .bind(&uid).fetch_all(sq).await.unwrap_or_default(),
        };

        for ws_id in &workspace_ids {
            // Delete table cache entries for datasources in this workspace
            let _ = kyomi_core::db_execute!(
                db,
                "DELETE FROM datasource_table_cache WHERE datasource_config_id IN \
                 (SELECT id FROM datasource_configs WHERE workspace_id = $1)",
                ws_id
            );

            // Delete datasource credentials
            let _ = kyomi_core::db_execute!(db, "DELETE FROM user_datasource_credentials WHERE workspace_id = $1", ws_id);

            // Delete datasource preferences
            let _ = kyomi_core::db_execute!(
                db,
                "DELETE FROM user_datasource_preferences WHERE datasource_config_id IN \
                 (SELECT id FROM datasource_configs WHERE workspace_id = $1)",
                ws_id
            );

            // Delete datasource configs
            let _ = kyomi_core::db_execute!(db, "DELETE FROM datasource_configs WHERE workspace_id = $1", ws_id);

            // Delete workspace users
            let _ = kyomi_core::db_execute!(db, "DELETE FROM workspace_users WHERE workspace_id = $1", ws_id);

            // Delete workspace
            let _ = kyomi_core::db_execute!(db, "DELETE FROM workspaces WHERE workspace_id = $1", ws_id);
        }

        let _ = kyomi_core::db_execute!(db, "DELETE FROM users WHERE user_id = $1", &uid);
    }
}

/// Clean up a specific datasource and its table cache entries.
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
        let _ = kyomi_core::db_execute!(db, "DELETE FROM datasource_table_cache WHERE datasource_config_id = $1", &id);
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

/// Create a test datasource directly via the service layer (bypasses HTTP routing).
///
/// Uses the service function instead of POST /datasources/ to avoid coupling
/// catalog tests to the datasource creation endpoint (which may have its own
/// pre-existing test infrastructure issues).
async fn create_test_datasource(ctx: &AuthContext, slug: &str) -> (String, String) {
    let ds = kyomi_auth::datasource_service::create_datasource(
        &ctx.db,
        &ctx.workspace_id,
        &format!("Catalog Test {slug}"),
        Some(slug),
        "postgres",
        json!({
            "host": "localhost",
            "port": 5433
        }),
        None, // direct connection
    )
    .await
    .expect("should create test datasource via service");

    (ds.id, ds.slug)
}

/// Insert test rows into datasource_table_cache for a given datasource.
async fn insert_test_tables(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    datasource_id: &str,
    tables: &[(&str, &str, &str, Value)],
) {
    let is_archived_val = if db.is_postgres() { "false" } else { "0" };
    let sql = format!(
        "INSERT INTO datasource_table_cache \
         (workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
          table_metadata, is_archived) \
         VALUES ($1, $2, $3, $4, $5, $6, {is_archived_val})"
    );
    for (project_id, dataset_id, table_id, metadata) in tables {
        let metadata_str = serde_json::to_string(metadata).unwrap();
        kyomi_core::db_execute!(
            db,
            &sql,
            workspace_id,
            datasource_id,
            *project_id,
            *dataset_id,
            *table_id,
            &metadata_str
        )
        .expect("should insert test table cache row");
    }
}

// ===========================================================================
// 1. Unauthenticated 401 tests — all catalog endpoints require auth
// ===========================================================================

#[tokio::test]
async fn discover_catalog_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/datasources/discover-catalog"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(
            json!({
                "datasource_type": "postgres",
                "connection_config": {},
                "credentials": {}
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /discover-catalog without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "error response must have 'detail' field"
    );
}

#[tokio::test]
async fn discover_resources_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/api/v1/datasources/discover"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(
            json!({
                "datasource_type": "postgres",
                "connection_config": {},
                "credentials": {}
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /discover without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "error response must have 'detail' field"
    );
}

#[tokio::test]
async fn catalog_tree_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!(
            "{base}/api/v1/datasources/test-slug/catalog/tree"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /catalog/tree without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "error response must have 'detail' field"
    );
}

#[tokio::test]
async fn catalog_status_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!(
            "{base}/api/v1/datasources/test-slug/catalog/status"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /catalog/status without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "error response must have 'detail' field"
    );
}

#[tokio::test]
async fn schemas_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .get(format!(
            "{base}/api/v1/datasources/test-slug/schemas"
        ))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /schemas without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "error response must have 'detail' field"
    );
}

#[tokio::test]
async fn catalog_refresh_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!(
            "{base}/api/v1/datasources/test-slug/catalog/refresh"
        ))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /catalog/refresh without auth should be 401"
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("detail").is_some(),
        "error response must have 'detail' field"
    );
}

// ===========================================================================
// 2. Catalog tree response shape (authenticated, Rust-backend mode only)
// ===========================================================================

#[tokio::test]
async fn catalog_tree_returns_correct_response_shape() {
    let ctx = setup_auth_context("tree-shape").await;
    if ctx.is_none() {
        eprintln!("SKIP: catalog_tree_returns_correct_response_shape — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Create a test datasource
    let (ds_id, ds_slug) = create_test_datasource(&ctx, "cat-tree-test").await;

    // Insert test table cache entries
    insert_test_tables(
        &ctx.db,
        &ctx.workspace_id,
        &ds_id,
        &[
            (
                "",
                "public",
                "users",
                json!({
                    "columns": [
                        {"name": "id", "type": "integer"},
                        {"name": "email", "type": "varchar"}
                    ],
                    "description": "User accounts",
                    "row_count": 100
                }),
            ),
            (
                "",
                "public",
                "orders",
                json!({
                    "columns": [
                        {"name": "id", "type": "integer"},
                        {"name": "user_id", "type": "integer"},
                        {"name": "total", "type": "numeric"}
                    ],
                    "description": "Customer orders",
                    "row_count": 5000
                }),
            ),
            (
                "",
                "analytics",
                "events",
                json!({
                    "columns": [
                        {"name": "event_id", "type": "bigint"},
                        {"name": "event_type", "type": "varchar"}
                    ],
                    "description": "Analytics events",
                    "row_count": 1000000
                }),
            ),
        ],
    )
    .await;

    // Request the catalog tree
    let resp = auth_get(
        &ctx.base_url,
        &format!("/api/v1/datasources/{ds_slug}/catalog/tree"),
        &ctx.access_token,
    )
    .send()
    .await
    .expect("tree request should succeed");

    assert_eq!(resp.status(), 200, "catalog tree should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    // Verify top-level response fields
    assert_eq!(
        body["datasource_id"], ds_id,
        "datasource_id should match"
    );
    assert!(
        body.get("datasource_name").is_some(),
        "missing 'datasource_name'"
    );
    assert_eq!(
        body["datasource_type"], "postgres",
        "datasource_type should match"
    );
    assert_eq!(
        body["table_count"], 3,
        "table_count should be 3"
    );
    assert!(
        body.get("tree").is_some(),
        "missing 'tree' field"
    );
    assert!(
        body["tree"].is_array(),
        "'tree' should be an array"
    );

    // For postgres with empty project_id + skip_single_project_wrapper=true,
    // tree should be flat: [schema_node_1, schema_node_2]
    let tree = body["tree"].as_array().unwrap();

    // Should have 2 schema nodes (analytics, public) — sorted alphabetically
    assert_eq!(
        tree.len(),
        2,
        "should have 2 schema nodes (analytics, public)"
    );

    // Each schema node should have correct structure
    let first_schema = &tree[0];
    assert!(
        first_schema.get("id").is_some(),
        "schema node missing 'id'"
    );
    assert!(
        first_schema.get("name").is_some(),
        "schema node missing 'name'"
    );
    assert!(
        first_schema.get("type").is_some(),
        "schema node missing 'type'"
    );
    assert_eq!(
        first_schema["type"], "schema",
        "level2 type for postgres should be 'schema'"
    );
    assert!(
        first_schema.get("children").is_some(),
        "schema node should have 'children'"
    );
    assert!(
        first_schema["children"].is_array(),
        "'children' should be an array"
    );

    // Verify node order is alphabetical
    assert_eq!(tree[0]["name"], "analytics", "first schema should be 'analytics'");
    assert_eq!(tree[1]["name"], "public", "second schema should be 'public'");

    // Verify analytics schema has 1 table
    let analytics_children = tree[0]["children"].as_array().unwrap();
    assert_eq!(
        analytics_children.len(),
        1,
        "analytics schema should have 1 table"
    );
    assert_eq!(
        analytics_children[0]["name"], "events",
        "analytics table should be 'events'"
    );
    assert_eq!(
        analytics_children[0]["type"], "table",
        "table node type should be 'table'"
    );

    // Verify public schema has 2 tables (orders, users) sorted alphabetically
    let public_children = tree[1]["children"].as_array().unwrap();
    assert_eq!(
        public_children.len(),
        2,
        "public schema should have 2 tables"
    );
    assert_eq!(
        public_children[0]["name"], "orders",
        "first public table should be 'orders'"
    );
    assert_eq!(
        public_children[1]["name"], "users",
        "second public table should be 'users'"
    );

    // Verify table nodes have metadata
    let table_node = &public_children[1]; // users
    assert!(
        table_node.get("metadata").is_some(),
        "table node should have 'metadata'"
    );
    let metadata = &table_node["metadata"];
    assert_eq!(
        metadata["description"], "User accounts",
        "metadata description should match"
    );
    assert_eq!(
        metadata["row_count"], 100,
        "metadata row_count should match"
    );

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "cat-tree-test").await;
    cleanup_test_user(&ctx.db, "cat-test-tree-shape@contract-test.local").await;
}

// ===========================================================================
// 3. Catalog tree with include_columns=true
// ===========================================================================

#[tokio::test]
async fn catalog_tree_with_columns_returns_column_children() {
    let ctx = setup_auth_context("tree-cols").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: catalog_tree_with_columns_returns_column_children — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (ds_id, ds_slug) = create_test_datasource(&ctx, "cat-tree-cols").await;

    insert_test_tables(
        &ctx.db,
        &ctx.workspace_id,
        &ds_id,
        &[(
            "",
            "public",
            "users",
            json!({
                "columns": [
                    {"name": "id", "type": "integer"},
                    {"name": "email", "type": "varchar"}
                ],
                "description": "User table",
                "row_count": 10
            }),
        )],
    )
    .await;

    let resp = auth_get(
        &ctx.base_url,
        &format!("/api/v1/datasources/{ds_slug}/catalog/tree?include_columns=true"),
        &ctx.access_token,
    )
    .send()
    .await
    .expect("tree request with columns should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");

    // Navigate to table node: tree[0] = schema "public" -> children[0] = table "users"
    let tree = body["tree"].as_array().unwrap();
    let schema_node = &tree[0];
    let table_node = &schema_node["children"].as_array().unwrap()[0];

    // Table node should have column children
    assert!(
        table_node.get("children").is_some(),
        "table node should have column children when include_columns=true"
    );

    let columns = table_node["children"].as_array().unwrap();
    assert_eq!(columns.len(), 2, "should have 2 column nodes");

    // Verify column node structure
    let col = &columns[0];
    assert!(col.get("id").is_some(), "column node missing 'id'");
    assert!(col.get("name").is_some(), "column node missing 'name'");
    assert_eq!(col["type"], "column", "column node type should be 'column'");
    assert!(
        col.get("metadata").is_some(),
        "column node should have 'metadata'"
    );

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "cat-tree-cols").await;
    cleanup_test_user(&ctx.db, "cat-test-tree-cols@contract-test.local").await;
}

// ===========================================================================
// 4. Catalog tree for empty datasource (no cached tables)
// ===========================================================================

#[tokio::test]
async fn catalog_tree_empty_datasource_returns_empty_tree() {
    let ctx = setup_auth_context("tree-empty").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: catalog_tree_empty_datasource_returns_empty_tree — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (_ds_id, ds_slug) = create_test_datasource(&ctx, "cat-tree-empty").await;

    // No table cache entries — datasource is freshly created

    let resp = auth_get(
        &ctx.base_url,
        &format!("/api/v1/datasources/{ds_slug}/catalog/tree"),
        &ctx.access_token,
    )
    .send()
    .await
    .expect("tree request should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");

    assert_eq!(body["table_count"], 0, "table_count should be 0");
    assert!(
        body["tree"].as_array().unwrap().is_empty(),
        "tree should be empty for datasource with no cached tables"
    );
    assert!(
        body["last_indexed"].is_null(),
        "last_indexed should be null for unindexed datasource"
    );

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "cat-tree-empty").await;
    cleanup_test_user(&ctx.db, "cat-test-tree-empty@contract-test.local").await;
}

// ===========================================================================
// 5. Catalog status response shape (authenticated)
// ===========================================================================

#[tokio::test]
async fn catalog_status_returns_correct_fields() {
    let ctx = setup_auth_context("status-fields").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: catalog_status_returns_correct_fields — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (ds_id, ds_slug) = create_test_datasource(&ctx, "cat-status-test").await;

    // Insert some test table cache entries across 2 schemas
    insert_test_tables(
        &ctx.db,
        &ctx.workspace_id,
        &ds_id,
        &[
            (
                "",
                "public",
                "users",
                json!({"columns": [], "description": "Users"}),
            ),
            (
                "",
                "public",
                "orders",
                json!({"columns": [], "description": "Orders"}),
            ),
            (
                "",
                "analytics",
                "events",
                json!({"columns": [], "description": "Events"}),
            ),
        ],
    )
    .await;

    let resp = auth_get(
        &ctx.base_url,
        &format!("/api/v1/datasources/{ds_slug}/catalog/status"),
        &ctx.access_token,
    )
    .send()
    .await
    .expect("status request should succeed");

    assert_eq!(resp.status(), 200, "catalog status should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    // Verify all required fields
    assert_eq!(body["datasource_id"], ds_id, "datasource_id should match");
    assert!(
        body.get("datasource_name").is_some(),
        "missing 'datasource_name'"
    );
    assert_eq!(
        body["datasource_type"], "postgres",
        "datasource_type should match"
    );
    assert_eq!(body["table_count"], 3, "table_count should be 3");
    assert_eq!(
        body["schema_count"], 2,
        "schema_count should be 2 (public, analytics)"
    );

    // indexing_status should be present and be "idle" (no indexing in progress)
    assert!(
        body.get("indexing_status").is_some(),
        "missing 'indexing_status'"
    );
    assert_eq!(
        body["indexing_status"], "idle",
        "indexing_status should be 'idle' for freshly created datasource"
    );

    // catalog_config should be present (with keys from registry metadata)
    assert!(
        body.get("catalog_config").is_some(),
        "missing 'catalog_config'"
    );
    assert!(
        body["catalog_config"].is_object(),
        "'catalog_config' should be an object"
    );

    // For postgres, catalog_config should have 'catalog_schemas' key
    let config = body["catalog_config"].as_object().unwrap();
    assert!(
        config.contains_key("catalog_schemas"),
        "postgres catalog_config should have 'catalog_schemas'"
    );

    // last_indexed should be a string (because we just inserted rows)
    assert!(
        body.get("last_indexed").is_some(),
        "missing 'last_indexed'"
    );
    assert!(
        body["last_indexed"].is_string(),
        "'last_indexed' should be a string timestamp"
    );

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "cat-status-test").await;
    cleanup_test_user(&ctx.db, "cat-test-status-fields@contract-test.local").await;
}

// ===========================================================================
// 6. Catalog status for empty datasource
// ===========================================================================

#[tokio::test]
async fn catalog_status_empty_datasource_returns_zero_counts() {
    let ctx = setup_auth_context("status-empty").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: catalog_status_empty_datasource_returns_zero_counts — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (_ds_id, ds_slug) = create_test_datasource(&ctx, "cat-status-empty").await;

    let resp = auth_get(
        &ctx.base_url,
        &format!("/api/v1/datasources/{ds_slug}/catalog/status"),
        &ctx.access_token,
    )
    .send()
    .await
    .expect("status request should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");

    assert_eq!(body["table_count"], 0, "table_count should be 0");
    assert_eq!(body["schema_count"], 0, "schema_count should be 0");
    assert!(
        body["last_indexed"].is_null(),
        "last_indexed should be null when no tables cached"
    );

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "cat-status-empty").await;
    cleanup_test_user(&ctx.db, "cat-test-status-empty@contract-test.local").await;
}

// ===========================================================================
// 7. Catalog tree 404 for nonexistent datasource
// ===========================================================================

#[tokio::test]
async fn catalog_tree_returns_404_for_nonexistent_datasource() {
    let ctx = setup_auth_context("tree-404").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: catalog_tree_returns_404_for_nonexistent_datasource — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/nonexistent-slug-xyz/catalog/tree",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("tree request should succeed");

    assert_eq!(
        resp.status(),
        404,
        "catalog tree for nonexistent datasource should return 404"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "cat-test-tree-404@contract-test.local").await;
}

// ===========================================================================
// 8. Catalog status 404 for nonexistent datasource
// ===========================================================================

#[tokio::test]
async fn catalog_status_returns_404_for_nonexistent_datasource() {
    let ctx = setup_auth_context("status-404").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: catalog_status_returns_404_for_nonexistent_datasource — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/nonexistent-slug-xyz/catalog/status",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("status request should succeed");

    assert_eq!(
        resp.status(),
        404,
        "catalog status for nonexistent datasource should return 404"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "cat-test-status-404@contract-test.local").await;
}

// ===========================================================================
// 9. Discover-catalog response shape (authenticated, invalid type test)
// ===========================================================================

#[tokio::test]
async fn discover_catalog_unsupported_type_returns_error_shape() {
    let ctx = setup_auth_context("disc-bad-type").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: discover_catalog_unsupported_type_returns_error_shape — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/discover-catalog",
        &ctx.access_token,
    )
    .body(
        json!({
            "datasource_type": "unsupported_db_type",
            "connection_config": {},
            "credentials": {}
        })
        .to_string(),
    )
    .send()
    .await
    .expect("discover-catalog request should succeed");

    assert_eq!(
        resp.status(),
        200,
        "discover-catalog always returns 200 (success=false for errors)"
    );

    let body: Value = resp.json().await.expect("should return JSON");

    // Verify response shape even on error
    assert_eq!(body["success"], false, "success should be false");
    assert!(body.get("items").is_some(), "missing 'items'");
    assert!(body["items"].is_array(), "'items' should be an array");
    assert!(
        body["items"].as_array().unwrap().is_empty(),
        "'items' should be empty on error"
    );
    assert!(body.get("item_type").is_some(), "missing 'item_type'");
    assert!(body.get("message").is_some(), "missing 'message'");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("unsupported"),
        "message should mention unsupported type"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "cat-test-disc-bad-type@contract-test.local").await;
}

// ===========================================================================
// 10. Discover resources response shape (authenticated, invalid type test)
// ===========================================================================

#[tokio::test]
async fn discover_resources_unsupported_type_returns_error_shape() {
    let ctx = setup_auth_context("disc-res-bad").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: discover_resources_unsupported_type_returns_error_shape — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/discover",
        &ctx.access_token,
    )
    .body(
        json!({
            "datasource_type": "invalid_type_xyz",
            "connection_config": {},
            "credentials": {}
        })
        .to_string(),
    )
    .send()
    .await
    .expect("discover request should succeed");

    assert_eq!(
        resp.status(),
        200,
        "discover always returns 200 (success=false for errors)"
    );

    let body: Value = resp.json().await.expect("should return JSON");

    assert_eq!(body["success"], false, "success should be false");
    assert!(body.get("resources").is_some(), "missing 'resources'");
    assert!(
        body["resources"].is_object(),
        "'resources' should be an object"
    );
    assert!(body.get("message").is_some(), "missing 'message'");

    // Clean up
    cleanup_test_user(&ctx.db, "cat-test-disc-res-bad@contract-test.local").await;
}

// ===========================================================================
// 11. Catalog refresh requires admin
// ===========================================================================

#[tokio::test]
async fn catalog_refresh_requires_admin_role() {
    let ctx = setup_auth_context("refresh-nonadmin").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: catalog_refresh_requires_admin_role — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    // Create a datasource as admin
    let (_ds_id, ds_slug) = create_test_datasource(&ctx, "cat-refresh-nonadmin").await;

    // Add a non-admin member to the workspace and get their token
    let (member_token, member_email) = create_member_token(&ctx, "refresh-nonadmin").await;

    // Try to refresh as the non-admin member
    let resp = auth_post(
        &ctx.base_url,
        &format!("/api/v1/datasources/{ds_slug}/catalog/refresh"),
        &member_token,
    )
    .body(json!({"force": false}).to_string())
    .send()
    .await
    .expect("refresh request should succeed");

    assert_eq!(
        resp.status(),
        403,
        "catalog refresh by non-admin should return 403"
    );

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "cat-refresh-nonadmin").await;
    cleanup_test_user(&ctx.db, &member_email).await;
    cleanup_test_user(&ctx.db, "cat-test-refresh-nonadmin@contract-test.local").await;
}

// ===========================================================================
// 12. Catalog refresh returns started for admin
// ===========================================================================

#[tokio::test]
async fn catalog_refresh_returns_result_for_admin() {
    let ctx = setup_auth_context("refresh-admin").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: catalog_refresh_returns_result_for_admin — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let (ds_id, ds_slug) = create_test_datasource(&ctx, "cat-refresh-admin").await;

    let resp = auth_post(
        &ctx.base_url,
        &format!("/api/v1/datasources/{ds_slug}/catalog/refresh"),
        &ctx.access_token,
    )
    .body(json!({"force": false}).to_string())
    .send()
    .await
    .expect("refresh request should succeed");

    // Should return 200 with a result status (runs synchronously like Python).
    // For test datasources with no real connection, expect "error" or "skipped".
    assert_eq!(
        resp.status(),
        200,
        "catalog refresh by admin should return 200"
    );

    let body: Value = resp.json().await.expect("should return JSON");

    let status = body["status"].as_str().unwrap();
    assert!(
        ["completed", "skipped", "error"].contains(&status),
        "status should be completed/skipped/error, got '{status}'"
    );
    assert!(
        body.get("message").is_some(),
        "missing 'message'"
    );
    assert_eq!(
        body["datasource_id"], ds_id,
        "datasource_id should match"
    );

    // Clean up
    cleanup_datasource(&ctx.db, &ctx.workspace_id, "cat-refresh-admin").await;
    cleanup_test_user(&ctx.db, "cat-test-refresh-admin@contract-test.local").await;
}

// ===========================================================================
// 13. Schemas endpoint for nonexistent datasource
// ===========================================================================

#[tokio::test]
async fn schemas_returns_404_for_nonexistent_datasource() {
    let ctx = setup_auth_context("schemas-404").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: schemas_returns_404_for_nonexistent_datasource — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_get(
        &ctx.base_url,
        "/api/v1/datasources/nonexistent-slug-xyz/schemas",
        &ctx.access_token,
    )
    .send()
    .await
    .expect("schemas request should succeed");

    assert_eq!(
        resp.status(),
        404,
        "schemas for nonexistent datasource should return 404"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "cat-test-schemas-404@contract-test.local").await;
}

// ===========================================================================
// 14. Catalog refresh for nonexistent datasource
// ===========================================================================

#[tokio::test]
async fn catalog_refresh_returns_404_for_nonexistent_datasource() {
    let ctx = setup_auth_context("refresh-404").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: catalog_refresh_returns_404_for_nonexistent_datasource — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = auth_post(
        &ctx.base_url,
        "/api/v1/datasources/nonexistent-slug-xyz/catalog/refresh",
        &ctx.access_token,
    )
    .body(json!({"force": false}).to_string())
    .send()
    .await
    .expect("refresh request should succeed");

    assert_eq!(
        resp.status(),
        404,
        "catalog refresh for nonexistent datasource should return 404"
    );

    // Clean up
    cleanup_test_user(&ctx.db, "cat-test-refresh-404@contract-test.local").await;
}
