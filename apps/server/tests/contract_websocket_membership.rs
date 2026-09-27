// SPDX-License-Identifier: AGPL-3.0-or-later

//! The WebSocket path workspace must be authorized at registration and again
//! for established-socket requests and outbound workspace data.

use futures_util::{SinkExt, StreamExt};
use kyomi_auth::{user_service, workspace_service};
use serde_json::json;
use std::sync::Arc;
use tokio_tungstenite::tungstenite::{Message, protocol::Role};

type Socket = tokio_tungstenite::WebSocketStream<reqwest::Upgraded>;

async fn connect_ws(base_url: &str, workspace_id: &str, user_id: &str, token: &str) -> Socket {
    let url = format!("{base_url}/ws/{workspace_id}_{user_id}?token={token}");
    let response = reqwest::Client::new()
        .get(url)
        .version(reqwest::Version::HTTP_11)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .expect("WebSocket upgrade request");
    assert_eq!(response.status(), reqwest::StatusCode::SWITCHING_PROTOCOLS);

    let stream = response.upgrade().await.expect("upgraded connection");
    tokio_tungstenite::WebSocketStream::from_raw_socket(stream, Role::Client, None).await
}

async fn next_frame(socket: &mut Socket) -> Message {
    tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .expect("server must answer")
        .expect("server must send a frame")
        .expect("valid WebSocket frame")
}

async fn first_ws_message(
    base_url: &str,
    workspace_id: &str,
    user_id: &str,
    token: &str,
) -> Message {
    next_frame(&mut connect_ws(base_url, workspace_id, user_id, token).await).await
}

struct Context {
    base_url: String,
    workspace_id: String,
    user_id: String,
    access_token: String,
    jwt_secret: String,
    db: kyomi_core::DbPool,
    manager: kyomi_auth::websocket::WebSocketManager,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Context {
    fn drop(&mut self) {
        self.server.abort();
    }
}

// Use the compiled production route with an isolated, migrated database: an
// external contract-test URL must never silently select an older server binary.
async fn context(suffix: &str) -> Context {
    static LOAD_CONSTANTS: std::sync::Once = std::sync::Once::new();
    LOAD_CONSTANTS.call_once(|| {
        kyomi_core::constants::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/constants.toml"),
        )
        .unwrap();
    });
    let db = kyomi_core::DbPool::connect("sqlite::memory:")
        .await
        .unwrap();
    let mut config = kyomi_core::Config::test_config();
    config.jwt_secret = "synthetic-websocket-membership-test-secret".into();
    config.self_hosted = false;
    config.stripe_secret_key = None;
    let email = format!("{suffix}@example.com");
    let user = user_service::create_user(&db, &email, Some("Member"), true)
        .await
        .unwrap();
    let workspace_id = user_service::create_workspace_for_user(
        &db,
        &user.user_id,
        Some("Workspace"),
        &email,
        Some(&config),
    )
    .await
    .unwrap();
    let access_token = kyomi_auth::jwt::create_access_token_str(
        &user.user_id,
        &config.jwt_secret,
        60,
        std::collections::HashMap::from([("workspace_id".into(), json!(workspace_id))]),
    )
    .unwrap();
    let kv = kyomi_core::kv_store::create_kv_store(None).await.unwrap();
    let manager = kyomi_auth::websocket::WebSocketManager::new(None, db.clone());
    let state = kyomi_server::state::AppState {
        db: db.clone(),
        kv: kv.clone(),
        redis: None,
        config: Arc::new(config.clone()),
        encryption_key: Arc::new([0; 32]),
        webauthn: Arc::new(
            kyomi_auth::webauthn::build_webauthn(
                "localhost",
                "Test",
                &url::Url::parse("http://localhost").unwrap(),
            )
            .unwrap(),
        ),
        embedding: kyomi_embed::LazyEmbedding::new(),
        ws_manager: manager.clone(),
        stripe: None,
        mcp_sessions: kyomi_auth::mcp_session_manager::MCPSessionManager::new(kv),
        cancel_registry: kyomi_server::cancel_registry::CancelRegistry::default(),
        connect_token: None,
        connect_registry: kyomi_server::connect::registry::ConnectRegistry::new_local(),
        platforms: Arc::new(kyomi_core::platform::PlatformRegistry::new()),
        schema_drift: kyomi_server::schema_drift::SchemaDriftStatus::default(),
        process_instance: "websocket-test:0".into(),
    };
    let router = axum::Router::new()
        .route(
            "/ws/{user_id}",
            axum::routing::get(kyomi_server::routes::websocket::ws_handler),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Context {
        base_url,
        workspace_id,
        user_id: user.user_id,
        access_token,
        jwt_secret: config.jwt_secret,
        db,
        manager,
        server,
    }
}

#[tokio::test]
async fn member_receives_heartbeat() {
    let ctx = context("member").await;
    let first = first_ws_message(
        &ctx.base_url,
        &ctx.workspace_id,
        &ctx.user_id,
        &ctx.access_token,
    )
    .await;
    assert!(
        matches!(first, Message::Text(ref body) if body.contains("heartbeat")),
        "member must reach registration: {first:?}"
    );
}

#[tokio::test]
async fn nonmember_is_closed_before_heartbeat() {
    let ctx = context("nonmember").await;
    let email = format!("ws-outsider-{}@example.com", uuid::Uuid::new_v4());
    let outsider = kyomi_auth::user_service::create_user(&ctx.db, &email, Some("Outsider"), true)
        .await
        .unwrap();
    let other_workspace = kyomi_auth::user_service::create_workspace_for_user(
        &ctx.db,
        &outsider.user_id,
        Some("Other"),
        &email,
        None,
    )
    .await
    .unwrap();
    let extra = std::collections::HashMap::from([(
        "workspace_id".to_string(),
        serde_json::json!(other_workspace),
    )]);
    let token =
        kyomi_auth::jwt::create_access_token_str(&outsider.user_id, &ctx.jwt_secret, 60, extra)
            .unwrap();

    let first = first_ws_message(&ctx.base_url, &ctx.workspace_id, &outsider.user_id, &token).await;
    assert!(
        matches!(first, Message::Close(Some(ref frame)) if u16::from(frame.code) == 4003),
        "nonmember must receive CLOSE_FORBIDDEN before heartbeat: {first:?}"
    );
}

#[tokio::test]
async fn inactive_member_is_closed_before_heartbeat() {
    let ctx = context("inactive").await;
    let sql = "UPDATE workspace_users SET active = false WHERE workspace_id = $1 AND user_id = $2";
    match &ctx.db {
        kyomi_core::DbPool::Postgres(pool) => {
            sqlx::query(sql)
                .bind(&ctx.workspace_id)
                .bind(&ctx.user_id)
                .execute(pool)
                .await
                .unwrap();
        }
        kyomi_core::DbPool::Sqlite(pool) => {
            sqlx::query(sql)
                .bind(&ctx.workspace_id)
                .bind(&ctx.user_id)
                .execute(pool)
                .await
                .unwrap();
        }
    }

    let first = first_ws_message(
        &ctx.base_url,
        &ctx.workspace_id,
        &ctx.user_id,
        &ctx.access_token,
    )
    .await;
    assert!(
        matches!(first, Message::Close(Some(ref frame)) if u16::from(frame.code) == 4003),
        "inactive member must receive CLOSE_FORBIDDEN before heartbeat: {first:?}"
    );
}

async fn member_socket(ctx: &Context, user_id: &str, token: &str) -> Socket {
    let mut socket = connect_ws(&ctx.base_url, &ctx.workspace_id, user_id, token).await;
    let first = next_frame(&mut socket).await;
    assert!(matches!(first, Message::Text(ref body) if body.contains("heartbeat")));
    socket
}

async fn sync(socket: &mut Socket, request: &str) -> Vec<serde_json::Value> {
    socket.send(Message::text(request)).await.unwrap();
    let mut actions = Vec::new();
    loop {
        let frame = next_frame(socket).await;
        let Message::Text(body) = frame else {
            panic!("expected sync data, got {frame:?}");
        };
        let message: serde_json::Value = serde_json::from_str(&body).unwrap();
        match message["type"].as_str().unwrap() {
            "sync_action" => actions.push(message["data"].clone()),
            "sync_complete" => return actions,
            other => panic!("unexpected sync response: {other}"),
        }
    }
}

async fn seed_delta(ctx: &Context) {
    kyomi_core::db_execute!(&ctx.db,
        "INSERT INTO dashboards (dashboard_id, user_id, workspace_id, title) VALUES ($1, $2, $3, $4)",
        "private-dashboard", &ctx.user_id, &ctx.workspace_id, "Private workspace data"
    ).unwrap();
    kyomi_auth::sync_log_service::write_sync_entry(
        &ctx.db,
        kyomi_auth::sync_log_service::SyncEntryParams {
            entity_type: "dashboard",
            entity_id: "private-dashboard",
            workspace_id: &ctx.workspace_id,
            action: kyomi_types::sync::SyncActionType::Update,
            data: Some(json!({"title": "Private workspace data"})),
            owner_user_id: Some(&ctx.user_id),
            is_workspace_visible: false,
        },
    )
    .await
    .unwrap();
    kyomi_auth::sync_log_service::write_sync_entry(
        &ctx.db,
        kyomi_auth::sync_log_service::SyncEntryParams {
            entity_type: "dashboard",
            entity_id: "shared-dashboard",
            workspace_id: &ctx.workspace_id,
            action: kyomi_types::sync::SyncActionType::Update,
            data: Some(json!({"title": "Shared workspace data"})),
            owner_user_id: Some(&ctx.user_id),
            is_workspace_visible: true,
        },
    )
    .await
    .unwrap();
}

async fn deactivate(ctx: &Context) {
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE workspace_users SET active = false WHERE workspace_id = $1 AND user_id = $2",
        &ctx.workspace_id,
        &ctx.user_id
    )
    .unwrap();
}

async fn assert_forbidden(socket: &mut Socket) {
    let frame = next_frame(socket).await;
    assert!(
        matches!(frame, Message::Close(Some(ref close)) if u16::from(close.code) == 4003),
        "must close before sending any further workspace data: {frame:?}"
    );
}

async fn live_revocation(request: &str) {
    let capture = kyomi_test_tracing::capture_tracing();
    let dispatch_count = || {
        capture
            .events()
            .iter()
            .filter(|(_, message)| message.contains("Handling sync_"))
            .count()
    };
    let ctx = context("live-revocation").await;
    seed_delta(&ctx).await;
    let control = user_service::create_user(&ctx.db, "control@example.com", Some("Control"), true)
        .await
        .unwrap();
    workspace_service::create_workspace_user(
        &ctx.db,
        &ctx.workspace_id,
        &control.user_id,
        "workspace_user",
    )
    .await
    .unwrap();
    let control_token = kyomi_auth::jwt::create_access_token_str(
        &control.user_id,
        &ctx.jwt_secret,
        60,
        std::collections::HashMap::new(),
    )
    .unwrap();
    let mut socket = member_socket(&ctx, &ctx.user_id, &ctx.access_token).await;
    let mut control_socket = member_socket(&ctx, &control.user_id, &control_token).await;
    assert!(
        sync(&mut socket, request)
            .await
            .iter()
            .any(|action| action["entity_id"] == "private-dashboard"),
        "member must receive real private data before revocation"
    );
    assert!(!sync(&mut control_socket, request).await.is_empty());
    deactivate(&ctx).await;
    let before_denied_request = dispatch_count();
    assert!(
        before_denied_request > 0,
        "healthy sync must reach the production handler"
    );
    socket.send(Message::text(request)).await.unwrap();
    assert_forbidden(&mut socket).await;
    assert_eq!(
        dispatch_count(),
        before_denied_request,
        "revoked requests must stop before sync reads, not only at outbound delivery"
    );
    assert!(
        !sync(&mut control_socket, request).await.is_empty(),
        "still-active member must retain access"
    );
    let mut reconnected = member_socket(&ctx, &control.user_id, &control_token).await;
    assert!(
        !sync(&mut reconnected, request).await.is_empty(),
        "normal reconnect remains functional"
    );
}

#[tokio::test]
async fn established_socket_bootstrap_stops_after_membership_deactivation() {
    live_revocation(r#"{"type":"sync_bootstrap"}"#).await;
}

#[tokio::test]
async fn established_socket_delta_stops_after_membership_deactivation() {
    live_revocation(r#"{"type":"sync_delta","last_sync_id":0}"#).await;
}

#[tokio::test]
async fn established_socket_membership_database_error_fails_closed() {
    let ctx = context("lookup-error").await;
    let mut socket = member_socket(&ctx, &ctx.user_id, &ctx.access_token).await;
    assert!(
        !sync(&mut socket, r#"{"type":"sync_bootstrap"}"#)
            .await
            .is_empty()
    );
    // Break only the authorization read; workspace data remains available.
    kyomi_core::db_execute!(
        &ctx.db,
        "ALTER TABLE workspace_users RENAME TO unavailable_memberships"
    )
    .unwrap();
    socket
        .send(Message::text(r#"{"type":"sync_bootstrap"}"#))
        .await
        .unwrap();
    assert_forbidden(&mut socket).await;
}

async fn push_sync(ctx: &Context, workspace_id: &str) {
    ctx.manager
        .send_to_user(
            &ctx.user_id,
            kyomi_types::websocket::WebSocketMessage::new(
                kyomi_types::websocket::MessageType::SyncAction,
            )
            .with_data(json!({"workspace_id": workspace_id, "entity_id": "private-live-data"})),
        )
        .await;
}

#[tokio::test]
async fn live_push_stops_without_an_inbound_request_after_revocation() {
    let ctx = context("outbound").await;
    let mut socket = member_socket(&ctx, &ctx.user_id, &ctx.access_token).await;
    push_sync(&ctx, &ctx.workspace_id).await;
    assert!(
        matches!(next_frame(&mut socket).await, Message::Text(body) if body.contains("private-live-data"))
    );
    deactivate(&ctx).await;
    push_sync(&ctx, &ctx.workspace_id).await;
    assert_forbidden(&mut socket).await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while ctx.manager.local_connection_count() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("closed socket must unregister from the manager");
}

#[tokio::test]
async fn user_wide_push_checks_the_payload_workspace_too() {
    let ctx = context("different-workspace").await;
    let mut socket = member_socket(&ctx, &ctx.user_id, &ctx.access_token).await;
    push_sync(&ctx, "ws-not-a-membership").await;
    assert_forbidden(&mut socket).await;
}

#[tokio::test]
async fn lapsed_member_keeps_billing_notifications_and_sync_refusal() {
    let ctx = context("billing").await;
    kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE workspaces SET subscription_status = 'past_due' WHERE workspace_id = $1",
        &ctx.workspace_id
    )
    .unwrap();
    let mut socket = member_socket(&ctx, &ctx.user_id, &ctx.access_token).await;
    socket
        .send(Message::text(r#"{"type":"sync_bootstrap"}"#))
        .await
        .unwrap();
    let frame = next_frame(&mut socket).await;
    assert!(matches!(frame, Message::Text(body) if body.contains("payment_required")));
    kyomi_auth::websocket::helpers::broadcast_billing_status_changed(
        &ctx.manager,
        &ctx.workspace_id,
    )
    .await;
    let frame = next_frame(&mut socket).await;
    assert!(matches!(frame, Message::Text(body) if body.contains("billing_status_changed")));
}

#[tokio::test]
async fn account_lifecycle_notices_reach_nonmembers() {
    let ctx = context("lifecycle").await;
    let mut socket = member_socket(&ctx, &ctx.user_id, &ctx.access_token).await;
    deactivate(&ctx).await;
    kyomi_auth::websocket::helpers::send_workspace_removed(
        &ctx.manager,
        &ctx.user_id,
        &ctx.workspace_id,
        "Workspace",
        "Removed",
    )
    .await;
    assert!(
        matches!(next_frame(&mut socket).await, Message::Text(body) if body.contains("workspace_removed"))
    );
    kyomi_auth::websocket::helpers::send_workspace_invitation(
        kyomi_auth::websocket::helpers::WorkspaceInvitationParams {
            manager: &ctx.manager,
            user_id: &ctx.user_id,
            invitation_id: "invite-test",
            workspace_id: "ws-new-invitation",
            workspace_name: "Invited workspace",
            invited_by_name: "Inviter",
            role: "workspace_user",
            message: "Invited",
        },
    )
    .await;
    assert!(
        matches!(next_frame(&mut socket).await, Message::Text(body) if body.contains("workspace_invitation"))
    );
}

#[tokio::test]
async fn live_push_membership_database_error_fails_closed() {
    let ctx = context("outbound-lookup-error").await;
    let mut socket = member_socket(&ctx, &ctx.user_id, &ctx.access_token).await;
    kyomi_core::db_execute!(
        &ctx.db,
        "ALTER TABLE workspace_users RENAME TO unavailable_memberships"
    )
    .unwrap();
    push_sync(&ctx, &ctx.workspace_id).await;
    assert_forbidden(&mut socket).await;
}

#[tokio::test]
async fn user_wide_push_preserves_another_active_workspace() {
    let ctx = context("authorized-other-workspace").await;
    let other_workspace = user_service::create_workspace_for_user(
        &ctx.db,
        &ctx.user_id,
        Some("Other"),
        "other@example.com",
        None,
    )
    .await
    .unwrap();
    let mut socket = member_socket(&ctx, &ctx.user_id, &ctx.access_token).await;
    push_sync(&ctx, &other_workspace).await;
    let frame = next_frame(&mut socket).await;
    assert!(matches!(frame, Message::Text(body) if body.contains(&other_workspace)));
}
