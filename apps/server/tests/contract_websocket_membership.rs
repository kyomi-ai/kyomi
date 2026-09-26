// SPDX-License-Identifier: AGPL-3.0-or-later

//! The WebSocket path workspace must be authorized before registration sends
//! its first heartbeat or accepts a sync request.

use futures_util::StreamExt;
use kyomi_test_harness::AuthContext;
use tokio_tungstenite::tungstenite::{Message, protocol::Role};

async fn first_ws_message(
    base_url: &str,
    workspace_id: &str,
    user_id: &str,
    token: &str,
) -> Message {
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
    let mut socket =
        tokio_tungstenite::WebSocketStream::from_raw_socket(stream, Role::Client, None).await;
    tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .expect("server must send a heartbeat or close")
        .expect("server must send a frame")
        .expect("valid WebSocket frame")
}

async fn context(suffix: &str) -> AuthContext {
    kyomi_test_harness::setup_auth_context("WebSocket membership", "ws-membership", suffix)
        .await
        .expect("requires the local Rust test server")
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
    let email = format!("ws-outsider-{}@example.invalid", uuid::Uuid::new_v4());
    let outsider = kyomi_auth::user_service::create_user(
        &ctx.db,
        &email,
        Some("Outsider"),
        true,
    )
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
