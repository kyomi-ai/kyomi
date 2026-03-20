// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contract tests for MCP server endpoints.
//!
//! These tests verify the HTTP-level contract (request/response shapes, headers,
//! status codes) for the MCP endpoints under `/mcp`:
//!
//! - `POST /mcp` — JSON-RPC 2.0 request/response with session management
//! - `GET  /mcp` — SSE notification stream (requires valid session)
//! - `DELETE /mcp` — Session termination
//!
//! Test organization:
//! - Section 1: Unauthenticated 401 tests
//! - Section 2: Free tier gets 402 (Payment Required)
//! - Section 3: Initialize response shape + session header
//! - Section 4: Tools/list response shape
//! - Section 5: Tools/call — unknown tool returns error
//! - Section 6: Resources/list response shape
//! - Section 7: Ping returns pong
//! - Section 8: MCP-only tools visible (render_chart)
//! - Section 9: Unknown method returns -32601
//! - Section 10: Session management (GET SSE, DELETE, missing/invalid session)

use serde_json::{json, Value};
use std::collections::HashMap;

// ===========================================================================
// Test infrastructure
// ===========================================================================

struct TestServer {
    base_url: String,
    db: Option<kyomi_core::DbPool>,
    jwt_secret: Option<String>,
    encryption_key: Option<std::sync::Arc<[u8; 32]>>,
}

struct AuthContext {
    base_url: String,
    access_token: String,
    user_id: String,
    workspace_id: String,
    db: kyomi_core::DbPool,
    encryption_key: std::sync::Arc<[u8; 32]>,
    jwt_secret: String,
}

async fn setup_server() -> TestServer {
    if let Ok(url) = std::env::var("CONTRACT_TEST_BASE_URL") {
        return TestServer {
            base_url: url,
            db: None,
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

    let state = kyomi_server::state::AppState {
        db: db.clone(),
        kv: kv.clone(),
        redis: None,
        config: std::sync::Arc::new(config.clone()),
        encryption_key: encryption_key_arc.clone(),
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

    TestServer {
        base_url: format!("http://{addr}"),
        db: Some(db),
        jwt_secret: Some(jwt_secret),
        encryption_key: Some(encryption_key_arc),
    }
}

async fn base_url() -> String {
    setup_server().await.base_url
}

/// Create an authenticated test context with Starter tier (MCP requires Starter+).
async fn setup_auth_context(suffix: &str) -> Option<AuthContext> {
    let server = setup_server().await;
    let db = server.db?;
    let jwt_secret = server
        .jwt_secret
        .expect("jwt_secret should be set in Rust mode");
    let encryption_key = server
        .encryption_key
        .expect("encryption_key should be set in Rust mode");

    let email = format!("mcp-test-{suffix}@contract-test.local");

    cleanup_test_user(&db, &email).await;

    let user = kyomi_auth::user_service::create_user(&db, &email, Some("MCP Test User"), true)
        .await
        .expect("should create test user");

    let workspace_id = kyomi_auth::user_service::create_workspace_for_user(
        &db,
        &user.user_id,
        Some("MCP Test User"),
        &email,
    )
    .await
    .expect("should create test workspace");

    // Upgrade workspace to starter tier for MCP access
    kyomi_core::db_execute!(
        &db,
        "UPDATE workspaces SET subscription_tier = 'starter' WHERE workspace_id = $1",
        &workspace_id
    )
    .expect("should upgrade to starter tier");

    let mut extra = HashMap::new();
    extra.insert("user_id".to_string(), json!(user.user_id));
    extra.insert("email".to_string(), json!(email));
    extra.insert("name".to_string(), json!("MCP Test User"));
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

/// Create an authenticated test context with free tier (MCP should be denied).
async fn setup_free_auth_context(suffix: &str) -> Option<AuthContext> {
    let server = setup_server().await;
    let db = server.db?;
    let jwt_secret = server
        .jwt_secret
        .expect("jwt_secret should be set in Rust mode");
    let encryption_key = server
        .encryption_key
        .expect("encryption_key should be set in Rust mode");

    let email = format!("mcp-test-{suffix}@contract-test.local");

    cleanup_test_user(&db, &email).await;

    let user = kyomi_auth::user_service::create_user(&db, &email, Some("MCP Free User"), true)
        .await
        .expect("should create test user");

    let workspace_id = kyomi_auth::user_service::create_workspace_for_user(
        &db,
        &user.user_id,
        Some("MCP Free User"),
        &email,
    )
    .await
    .expect("should create test workspace");

    // Workspace stays at free tier (default)

    let mut extra = HashMap::new();
    extra.insert("user_id".to_string(), json!(user.user_id));
    extra.insert("email".to_string(), json!(email));
    extra.insert("name".to_string(), json!("MCP Free User"));
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
            let _ = kyomi_core::db_execute!(db, "DELETE FROM workspace_users WHERE workspace_id = $1", ws_id);

            let _ = kyomi_core::db_execute!(db, "DELETE FROM workspaces WHERE workspace_id = $1", ws_id);
        }

        let _ = kyomi_core::db_execute!(db, "DELETE FROM workspace_users WHERE user_id = $1", &uid);

        let _ = kyomi_core::db_execute!(db, "DELETE FROM users WHERE user_id = $1", &uid);
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// Helper to send a JSON-RPC 2.0 request to the MCP endpoint with cookie auth.
fn mcp_post(base: &str, token: &str) -> reqwest::RequestBuilder {
    client()
        .post(format!("{base}/mcp"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .header("cookie", format!("access_token={token}"))
}

/// Helper to send a JSON-RPC 2.0 request with session ID header.
fn mcp_post_with_session(base: &str, token: &str, session_id: &str) -> reqwest::RequestBuilder {
    mcp_post(base, token).header("mcp-session-id", session_id)
}

/// Helper: call initialize and extract the Mcp-Session-Id header.
async fn get_session_id(ctx: &AuthContext) -> String {
    let resp = mcp_post(&ctx.base_url, &ctx.access_token)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("initialize should succeed");

    assert_eq!(resp.status(), 200, "initialize should return 200");

    let session_id = resp
        .headers()
        .get("mcp-session-id")
        .expect("initialize should return Mcp-Session-Id header")
        .to_str()
        .expect("session ID should be valid string")
        .to_string();

    assert!(!session_id.is_empty(), "session ID should not be empty");
    session_id
}

// ===========================================================================
// 1. Unauthenticated 401 tests
// ===========================================================================

#[tokio::test]
async fn mcp_post_returns_401_without_auth() {
    let base = base_url().await;
    let resp = client()
        .post(format!("{base}/mcp"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize"
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "POST /mcp without auth should be 401"
    );
}

// ===========================================================================
// 2. Free tier gets 402 (Payment Required) for MCP
// ===========================================================================

#[tokio::test]
async fn mcp_returns_402_for_free_tier() {
    let ctx = setup_free_auth_context("free-402").await;
    if ctx.is_none() {
        eprintln!("SKIP: mcp_returns_402_for_free_tier — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = mcp_post(&ctx.base_url, &ctx.access_token)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("MCP request should succeed");

    assert_eq!(
        resp.status(),
        402,
        "MCP for free tier should return 402 Payment Required"
    );

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(
        body.get("detail").is_some(),
        "402 response should have 'detail'"
    );

    cleanup_test_user(&ctx.db, "mcp-test-free-402@contract-test.local").await;
}

// ===========================================================================
// 3. Initialize — response shape + session header
// ===========================================================================

#[tokio::test]
async fn initialize_returns_correct_response_shape() {
    let ctx = setup_auth_context("init-shape").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: initialize_returns_correct_response_shape — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = mcp_post(&ctx.base_url, &ctx.access_token)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("initialize request should succeed");

    assert_eq!(resp.status(), 200, "initialize should return 200");

    // Verify Mcp-Session-Id header is present
    let session_id = resp
        .headers()
        .get("mcp-session-id")
        .expect("initialize should return Mcp-Session-Id header");
    assert!(
        !session_id.to_str().unwrap().is_empty(),
        "session ID should not be empty"
    );

    let body: Value = resp.json().await.expect("should return JSON");

    // Verify JSON-RPC 2.0 envelope
    assert_eq!(body["jsonrpc"], "2.0", "jsonrpc should be '2.0'");
    assert_eq!(body["id"], 1, "id should match request");

    // Verify result fields
    let result = &body["result"];
    assert_eq!(
        result["protocolVersion"], "2025-03-26",
        "protocolVersion should match spec 2025-03-26"
    );

    // Verify capabilities — tools.listChanged = true (notifications via GET SSE stream)
    let capabilities = &result["capabilities"];
    assert!(
        capabilities.get("tools").is_some(),
        "capabilities should have 'tools'"
    );
    assert_eq!(
        capabilities["tools"]["listChanged"], true,
        "tools.listChanged should be true (SSE notification channel)"
    );
    assert!(
        capabilities.get("resources").is_some(),
        "capabilities should have 'resources'"
    );

    // Verify server info
    let server_info = &result["serverInfo"];
    assert_eq!(
        server_info["name"], "kyomi-mcp",
        "serverInfo.name should be 'kyomi-mcp'"
    );
    assert_eq!(
        server_info["version"], "1.4.0",
        "serverInfo.version should be '1.4.0'"
    );

    cleanup_test_user(&ctx.db, "mcp-test-init-shape@contract-test.local").await;
}

// ===========================================================================
// 4. Tools/list — response shape
// ===========================================================================

#[tokio::test]
async fn tools_list_returns_non_empty_array() {
    let ctx = setup_auth_context("tools-list").await;
    if ctx.is_none() {
        eprintln!("SKIP: tools_list_returns_non_empty_array — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    let session_id = get_session_id(&ctx).await;

    let resp = mcp_post_with_session(&ctx.base_url, &ctx.access_token, &session_id)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("tools/list request should succeed");

    assert_eq!(resp.status(), 200, "tools/list should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 2);

    let result = &body["result"];
    let tools = result["tools"].as_array().expect("tools should be an array");
    assert!(!tools.is_empty(), "tools/list should return at least one tool");

    // Each tool should have name, description, inputSchema
    for tool in tools {
        assert!(
            tool["name"].is_string(),
            "tool missing 'name': {:?}",
            tool
        );
        assert!(
            tool["description"].is_string(),
            "tool '{}' missing 'description'",
            tool["name"]
        );
        assert!(
            tool["inputSchema"].is_object(),
            "tool '{}' missing 'inputSchema'",
            tool["name"]
        );
    }

    cleanup_test_user(&ctx.db, "mcp-test-tools-list@contract-test.local").await;
}

// ===========================================================================
// 5. Tools/list — MCP-only tools visible, copilot-only hidden
// ===========================================================================

#[tokio::test]
async fn tools_list_includes_render_chart() {
    let ctx = setup_auth_context("tools-render").await;
    if ctx.is_none() {
        eprintln!("SKIP: tools_list_includes_render_chart — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    let session_id = get_session_id(&ctx).await;

    let resp = mcp_post_with_session(&ctx.base_url, &ctx.access_token, &session_id)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/list"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("tools/list request should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");
    let tools = body["result"]["tools"].as_array().unwrap();

    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();

    // render_chart (MCP-only) should be present
    assert!(
        tool_names.contains(&"render_chart"),
        "render_chart should be in MCP tools list"
    );

    // render_chart should have _meta.ui.resourceUri
    let render_chart = tools
        .iter()
        .find(|t| t["name"] == "render_chart")
        .expect("render_chart should exist");

    assert_eq!(
        render_chart["_meta"]["ui"]["resourceUri"],
        "ui://kyomi/chart",
        "render_chart should have correct resourceUri"
    );

    // update_dashboard (copilot-only) should NOT be present
    assert!(
        !tool_names.contains(&"update_dashboard"),
        "update_dashboard (copilot-only) should NOT be in MCP tools"
    );

    cleanup_test_user(&ctx.db, "mcp-test-tools-render@contract-test.local").await;
}

// ===========================================================================
// 6. Tools/call — unknown tool returns JSON-RPC error
// ===========================================================================

#[tokio::test]
async fn tools_call_unknown_tool_returns_error() {
    let ctx = setup_auth_context("tools-unknown").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: tools_call_unknown_tool_returns_error — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();
    let session_id = get_session_id(&ctx).await;

    let resp = mcp_post_with_session(&ctx.base_url, &ctx.access_token, &session_id)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "nonexistent_tool_xyz",
                    "arguments": {}
                }
            })
            .to_string(),
        )
        .send()
        .await
        .expect("tools/call request should succeed");

    assert_eq!(resp.status(), 200, "tools/call should still return 200 (error in JSON-RPC body)");

    let body: Value = resp.json().await.expect("should return JSON");

    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 4);

    // Should have an error field
    assert!(
        body.get("error").is_some(),
        "unknown tool should return JSON-RPC error"
    );
    assert_eq!(
        body["error"]["code"], -32602,
        "error code should be -32602 (Invalid params)"
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("nonexistent_tool_xyz"),
        "error message should mention the unknown tool name"
    );

    cleanup_test_user(&ctx.db, "mcp-test-tools-unknown@contract-test.local").await;
}

// ===========================================================================
// 7. Tools/call — missing tool name returns error
// ===========================================================================

#[tokio::test]
async fn tools_call_missing_name_returns_error() {
    let ctx = setup_auth_context("tools-noname").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: tools_call_missing_name_returns_error — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();
    let session_id = get_session_id(&ctx).await;

    let resp = mcp_post_with_session(&ctx.base_url, &ctx.access_token, &session_id)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "arguments": {}
                }
            })
            .to_string(),
        )
        .send()
        .await
        .expect("tools/call request should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");

    assert!(body.get("error").is_some(), "missing name should return error");
    assert_eq!(body["error"]["code"], -32602);

    cleanup_test_user(&ctx.db, "mcp-test-tools-noname@contract-test.local").await;
}

// ===========================================================================
// 8. Resources/list — response shape
// ===========================================================================

#[tokio::test]
async fn resources_list_returns_chart_viewer() {
    let ctx = setup_auth_context("res-list").await;
    if ctx.is_none() {
        eprintln!("SKIP: resources_list_returns_chart_viewer — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    let session_id = get_session_id(&ctx).await;

    let resp = mcp_post_with_session(&ctx.base_url, &ctx.access_token, &session_id)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "resources/list"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("resources/list request should succeed");

    assert_eq!(resp.status(), 200, "resources/list should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 6);

    let result = &body["result"];
    let resources = result["resources"]
        .as_array()
        .expect("resources should be an array");

    assert_eq!(resources.len(), 1, "should have 1 resource (chart viewer)");

    let chart_viewer = &resources[0];
    assert_eq!(
        chart_viewer["uri"], "ui://kyomi/chart",
        "resource URI should be 'ui://kyomi/chart'"
    );
    assert_eq!(
        chart_viewer["name"], "Kyomi Chart Viewer",
        "resource name should be 'Kyomi Chart Viewer'"
    );
    assert_eq!(
        chart_viewer["mimeType"], "text/html;profile=mcp-app",
        "resource mimeType should be 'text/html;profile=mcp-app'"
    );

    cleanup_test_user(&ctx.db, "mcp-test-res-list@contract-test.local").await;
}

// ===========================================================================
// 9. Ping — returns empty result
// ===========================================================================

#[tokio::test]
async fn ping_returns_pong() {
    let ctx = setup_auth_context("ping").await;
    if ctx.is_none() {
        eprintln!("SKIP: ping_returns_pong — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    let session_id = get_session_id(&ctx).await;

    let resp = mcp_post_with_session(&ctx.base_url, &ctx.access_token, &session_id)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "ping"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("ping request should succeed");

    assert_eq!(resp.status(), 200, "ping should return 200");

    let body: Value = resp.json().await.expect("should return JSON");

    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 7);
    assert!(body.get("result").is_some(), "ping should have a result");
    assert!(body.get("error").is_none(), "ping should not have an error");

    cleanup_test_user(&ctx.db, "mcp-test-ping@contract-test.local").await;
}

// ===========================================================================
// 10. Unknown method — returns -32601
// ===========================================================================

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let ctx = setup_auth_context("unknown-method").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: unknown_method_returns_method_not_found — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();
    let session_id = get_session_id(&ctx).await;

    let resp = mcp_post_with_session(&ctx.base_url, &ctx.access_token, &session_id)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 8,
                "method": "nonexistent/method"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");

    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], 8);

    assert!(
        body.get("error").is_some(),
        "unknown method should return error"
    );
    assert_eq!(
        body["error"]["code"], -32601,
        "error code should be -32601 (Method not found)"
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("nonexistent/method"),
        "error message should mention the unknown method"
    );

    cleanup_test_user(&ctx.db, "mcp-test-unknown-method@contract-test.local").await;
}

// ===========================================================================
// 11. Initialize with string ID
// ===========================================================================

#[tokio::test]
async fn initialize_with_string_id_returns_same_id() {
    let ctx = setup_auth_context("str-id").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: initialize_with_string_id_returns_same_id — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = mcp_post(&ctx.base_url, &ctx.access_token)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": "abc-123",
                "method": "initialize"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("initialize request should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");

    // JSON-RPC 2.0 allows string IDs
    assert_eq!(body["id"], "abc-123", "id should match string request id");

    cleanup_test_user(&ctx.db, "mcp-test-str-id@contract-test.local").await;
}

// ===========================================================================
// 12. Tools/list — tools have annotations
// ===========================================================================

#[tokio::test]
async fn tools_list_tools_have_annotations() {
    let ctx = setup_auth_context("tools-ann").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: tools_list_tools_have_annotations — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();
    let session_id = get_session_id(&ctx).await;

    let resp = mcp_post_with_session(&ctx.base_url, &ctx.access_token, &session_id)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 9,
                "method": "tools/list"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("tools/list request should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");
    let tools = body["result"]["tools"]
        .as_array()
        .expect("tools should be an array");

    for tool in tools {
        assert!(
            tool.get("annotations").is_some(),
            "Tool '{}' should have 'annotations'",
            tool["name"]
        );
    }

    cleanup_test_user(&ctx.db, "mcp-test-tools-ann@contract-test.local").await;
}

// ===========================================================================
// 13. Copilot-only tools are hidden from tools/list
// ===========================================================================

#[tokio::test]
async fn tools_list_hides_copilot_only_tools() {
    let ctx = setup_auth_context("tools-copilot").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: tools_list_hides_copilot_only_tools — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();
    let session_id = get_session_id(&ctx).await;

    let resp = mcp_post_with_session(&ctx.base_url, &ctx.access_token, &session_id)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "tools/list"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("tools/list request should succeed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.expect("should return JSON");
    let tools = body["result"]["tools"]
        .as_array()
        .expect("tools should be an array");

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap_or(""))
        .collect();

    // update_dashboard is copilot-only and should NOT appear in MCP tools/list
    assert!(
        !tool_names.contains(&"update_dashboard"),
        "update_dashboard (copilot-only) should not appear in MCP tools/list"
    );

    // update_chart is copilot-only and should NOT appear in MCP tools/list
    assert!(
        !tool_names.contains(&"update_chart"),
        "update_chart (copilot-only) should not appear in MCP tools/list"
    );

    // preview_watch is copilot-only and should NOT appear in MCP tools/list
    assert!(
        !tool_names.contains(&"preview_watch"),
        "preview_watch (copilot-only) should not appear in MCP tools/list"
    );

    cleanup_test_user(&ctx.db, "mcp-test-tools-copilot@contract-test.local").await;
}

// ===========================================================================
// 14. Invalid JSON returns parse error
// ===========================================================================

#[tokio::test]
async fn invalid_json_returns_parse_error() {
    let ctx = setup_auth_context("invalid-json").await;
    if ctx.is_none() {
        eprintln!(
            "SKIP: invalid_json_returns_parse_error — requires Rust-backend mode"
        );
        return;
    }
    let ctx = ctx.unwrap();

    let resp = mcp_post(&ctx.base_url, &ctx.access_token)
        .body("this is not json")
        .send()
        .await
        .expect("request should succeed at transport level");

    // Invalid JSON should return an error (either 400 or 200 with JSON-RPC error)
    let status = resp.status().as_u16();
    assert!(
        status == 200 || status == 400 || status == 422,
        "invalid JSON should return 200 (JSON-RPC error), 400, or 422, got {status}"
    );

    cleanup_test_user(&ctx.db, "mcp-test-invalid-json@contract-test.local").await;
}

// ===========================================================================
// 15. GET /mcp — SSE notification stream
// ===========================================================================

#[tokio::test]
async fn get_without_auth_returns_401() {
    let base = base_url().await;
    let resp = client()
        .get(format!("{base}/mcp"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /mcp without auth should return 401"
    );
}

#[tokio::test]
async fn get_without_session_returns_400() {
    let ctx = setup_auth_context("get-no-session").await;
    if ctx.is_none() {
        eprintln!("SKIP: get_without_session_returns_400 — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = client()
        .get(format!("{}/mcp", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "GET /mcp without Mcp-Session-Id should return 400"
    );

    cleanup_test_user(&ctx.db, "mcp-test-get-no-session@contract-test.local").await;
}

#[tokio::test]
async fn get_with_invalid_session_returns_404() {
    let ctx = setup_auth_context("get-bad-session").await;
    if ctx.is_none() {
        eprintln!("SKIP: get_with_invalid_session_returns_404 — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    let resp = client()
        .get(format!("{}/mcp", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .header("mcp-session-id", "bogus-session-id")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        404,
        "GET /mcp with invalid session should return 404"
    );

    cleanup_test_user(&ctx.db, "mcp-test-get-bad-session@contract-test.local").await;
}

#[tokio::test]
async fn get_with_valid_session_returns_sse_stream() {
    let ctx = setup_auth_context("get-sse").await;
    if ctx.is_none() {
        eprintln!("SKIP: get_with_valid_session_returns_sse_stream — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    let session_id = get_session_id(&ctx).await;

    let resp = client()
        .get(format!("{}/mcp", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .header("mcp-session-id", &session_id)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "GET /mcp with valid session should return 200"
    );

    // Should have SSE content type
    let content_type = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(
        content_type.contains("text/event-stream"),
        "GET /mcp should return text/event-stream, got: {content_type}"
    );

    // Should echo back the session ID
    let resp_session = resp
        .headers()
        .get("mcp-session-id")
        .map(|v| v.to_str().unwrap().to_string());
    assert_eq!(
        resp_session.as_deref(),
        Some(session_id.as_str()),
        "Response should include Mcp-Session-Id header"
    );

    cleanup_test_user(&ctx.db, "mcp-test-get-sse@contract-test.local").await;
}

// ===========================================================================
// 16. DELETE /mcp removes session
// ===========================================================================

#[tokio::test]
async fn delete_removes_session() {
    let ctx = setup_auth_context("delete-sess").await;
    if ctx.is_none() {
        eprintln!("SKIP: delete_removes_session — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();
    let session_id = get_session_id(&ctx).await;

    // DELETE the session
    let resp = client()
        .delete(format!("{}/mcp", ctx.base_url))
        .header("origin", "http://localhost:5173")
        .header("cookie", format!("access_token={}", ctx.access_token))
        .header("mcp-session-id", &session_id)
        .send()
        .await
        .expect("DELETE should succeed");

    assert_eq!(resp.status(), 200, "DELETE should return 200");

    // Now try to use the deleted session — should get 404
    let resp = mcp_post_with_session(&ctx.base_url, &ctx.access_token, &session_id)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("request should succeed at transport level");

    assert_eq!(
        resp.status(),
        404,
        "Using deleted session should return 404"
    );

    cleanup_test_user(&ctx.db, "mcp-test-delete-sess@contract-test.local").await;
}

// ===========================================================================
// 17. Request without session header is allowed (backwards compatibility)
// ===========================================================================

#[tokio::test]
async fn request_without_session_is_allowed() {
    let ctx = setup_auth_context("no-session").await;
    if ctx.is_none() {
        eprintln!("SKIP: request_without_session_is_allowed — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Send tools/list without Mcp-Session-Id header — should still work
    // (backwards compatibility with clients that don't implement sessions)
    let resp = mcp_post(&ctx.base_url, &ctx.access_token)
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("request should succeed at transport level");

    assert_eq!(
        resp.status(),
        200,
        "tools/list without Mcp-Session-Id should return 200 (backwards compat)"
    );

    let body: Value = resp.json().await.expect("should return JSON");
    assert!(
        body["result"]["tools"].as_array().is_some(),
        "should still return tools"
    );

    cleanup_test_user(&ctx.db, "mcp-test-no-session@contract-test.local").await;
}

// ===========================================================================
// 18. Request with stale session auto-heals (returns 200 + new session ID)
// ===========================================================================

#[tokio::test]
async fn request_with_invalid_session_auto_heals() {
    let ctx = setup_auth_context("bad-session").await;
    if ctx.is_none() {
        eprintln!("SKIP: request_with_invalid_session_auto_heals — requires Rust-backend mode");
        return;
    }
    let ctx = ctx.unwrap();

    // Send tools/list with a bogus session ID — server should auto-heal
    // by creating a new session instead of returning 404.
    let resp = mcp_post_with_session(&ctx.base_url, &ctx.access_token, "bogus-session-id-12345")
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list"
            })
            .to_string(),
        )
        .send()
        .await
        .expect("request should succeed at transport level");

    assert_eq!(
        resp.status(),
        200,
        "tools/list with stale session should auto-heal and return 200"
    );

    // The response should include a new session ID header
    assert!(
        resp.headers().get("mcp-session-id").is_some(),
        "auto-healed response should include new Mcp-Session-Id header"
    );

    cleanup_test_user(&ctx.db, "mcp-test-bad-session@contract-test.local").await;
}
