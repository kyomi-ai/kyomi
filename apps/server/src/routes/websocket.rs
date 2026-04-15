// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket endpoints for real-time communication.
//!
//! One endpoint:
//! - `GET /ws/{user_id}` — Authenticated WebSocket for logged-in users
//!
//! Wire-compatible with the Python backend's WebSocket protocol.

use axum::{
    extract::{ws, Path, Query, State},
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

use kyomi_auth::{jwt, user_service};

use crate::state::AppState;

/// Query parameters for WebSocket authentication.
#[derive(Debug, Deserialize)]
pub struct WsAuthParams {
    /// JWT access token for authentication.
    token: Option<String>,
}

/// Close codes matching the Python backend.
const CLOSE_AUTH_REQUIRED: u16 = 4001;
const CLOSE_FORBIDDEN: u16 = 4003;
const CLOSE_TOO_MANY_CONNECTIONS: u16 = 4029;

/// Maximum size of a single WebSocket message from the client (64 KB).
/// Prevents DoS via oversized messages. Client→server messages are small
/// JSON payloads (cancel_request, oauth_cancel, ping), so 64 KB is generous.
const MAX_MESSAGE_SIZE: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// GET /ws/{user_id} — Authenticated WebSocket
// ---------------------------------------------------------------------------

/// Authenticated WebSocket upgrade handler.
///
/// Authentication flow:
/// 1. Extract JWT from `?token=` query parameter
/// 2. Validate JWT signature and expiry
/// 3. Look up user in database, verify active status
/// 4. Verify path user_id matches JWT user_id
/// 5. Register connection with WebSocketManager
/// 6. Spawn inbound/outbound tasks
pub async fn ws_handler(
    ws: ws::WebSocketUpgrade,
    State(state): State<AppState>,
    Path(path_user_id): Path<String>,
    Query(params): Query<WsAuthParams>,
) -> impl IntoResponse {
    ws.max_message_size(MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| handle_authenticated_ws(socket, state, path_user_id, params))
}

async fn handle_authenticated_ws(
    socket: ws::WebSocket,
    state: AppState,
    path_user_id: String,
    params: WsAuthParams,
) {
    // 1. Extract and validate JWT token
    let token = match params.token {
        Some(t) if !t.is_empty() => t,
        _ => {
            tracing::warn!("WebSocket connection rejected: no token provided");
            close_with_code(socket, CLOSE_AUTH_REQUIRED, "Authentication required").await;
            return;
        }
    };

    let claims = match jwt::validate_token(&token, &state.config.jwt_secret) {
        Ok(token_data) => token_data.claims,
        Err(e) => {
            tracing::warn!("WebSocket JWT validation failed: {e}");
            close_with_code(socket, CLOSE_AUTH_REQUIRED, "Authentication failed").await;
            return;
        }
    };

    let jwt_user_id = &claims.sub;

    // 2. Extract user_id from path — may be "{workspace_id}_{user_id}" format
    let actual_user_id = extract_user_id_from_path(&path_user_id);

    // 3. Verify path user_id matches JWT user_id
    if actual_user_id != jwt_user_id {
        tracing::warn!(
            "WebSocket user_id mismatch: path={actual_user_id}, jwt={jwt_user_id}"
        );
        close_with_code(socket, CLOSE_FORBIDDEN, "User ID mismatch").await;
        return;
    }

    // 4. Look up user in database and verify active status
    let user = match user_service::get_user_by_id(&state.db, jwt_user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            tracing::warn!("WebSocket rejected: user not found: {jwt_user_id}");
            close_with_code(socket, CLOSE_FORBIDDEN, "User not found").await;
            return;
        }
        Err(e) => {
            tracing::error!("WebSocket user lookup failed: {e}");
            close_with_code(socket, CLOSE_AUTH_REQUIRED, "Authentication failed").await;
            return;
        }
    };

    if !user.active {
        tracing::warn!("WebSocket rejected: user disabled: {jwt_user_id}");
        close_with_code(socket, CLOSE_FORBIDDEN, "Account disabled").await;
        return;
    }

    // 5. Register with WebSocketManager (heartbeat sent automatically)
    let (connection_id, mut manager_rx) = match state.ws_manager.connect(jwt_user_id) {
        Ok(conn) => conn,
        Err(_) => {
            close_with_code(socket, CLOSE_TOO_MANY_CONNECTIONS, "Too many connections").await;
            return;
        }
    };
    tracing::info!(
        user_id = jwt_user_id,
        connection_id,
        "WebSocket connected"
    );

    // 6. Split socket and run concurrent tasks
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Outbound task: forwards messages from WebSocketManager + periodic pings.
    // Pings every 45s keep the connection alive through Cloudflare (100s idle
    // timeout), nginx, and Vite dev proxy (120s).
    let user_id_for_send = jwt_user_id.clone();
    let send_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(45));
        ping_interval.tick().await; // consume the immediate first tick

        loop {
            tokio::select! {
                msg = manager_rx.recv() => {
                    match msg {
                        Some(json) => {
                            if ws_sender.send(ws::Message::text(json)).await.is_err() {
                                break;
                            }
                        }
                        None => break, // channel closed
                    }
                }
                _ = ping_interval.tick() => {
                    if ws_sender.send(ws::Message::Ping(vec![].into())).await.is_err() {
                        break;
                    }
                }
            }
        }

        let _ = ws_sender.close().await;
        tracing::debug!("WS send task ended for user {user_id_for_send}");
    });

    let manager_clone = state.ws_manager.clone();
    let db_clone = state.db.clone();
    let cancel_registry_clone = state.cancel_registry.clone();
    let user_id_for_recv = jwt_user_id.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                ws::Message::Text(text) => {
                    handle_client_message(
                        &text,
                        &user_id_for_recv,
                        &manager_clone,
                        &db_clone,
                        &cancel_registry_clone,
                    )
                    .await;
                }
                ws::Message::Pong(_) => {} // expected response to our pings
                ws::Message::Close(_) => break,
                _ => {}
            }
        }
        tracing::debug!("WS recv task ended for user {user_id_for_recv}");
    });

    // Wait for either side to finish
    tokio::select! {
        _ = send_task => {}
        _ = recv_task => {}
    }

    // Cleanup
    state.ws_manager.disconnect(jwt_user_id, connection_id);
    tracing::info!(
        user_id = jwt_user_id,
        connection_id,
        "WebSocket disconnected"
    );
}

/// Extract the actual user_id from a path that may be "{workspace_id}_{user_id}".
///
/// The frontend sends `{workspace_id}_{user_id}` as the path parameter.
/// User IDs in this system always start with `usr_`, so we search for the
/// `_usr_` boundary first — this handles any workspace ID format regardless
/// of prefix (including E2E test workspaces like `e2e-test-workspace-0001`).
///
/// Legacy fallback: check for known workspace_id prefix formats:
/// - `ws-{uuid}` (e.g. `ws-550e8400-e29b-41d4-a716-446655440000`)
/// - `workspace-{short_id}` (e.g. `workspace-99f24d05-673d25b8`)
fn extract_user_id_from_path(path: &str) -> &str {
    // User IDs in this system always start with `usr_`.
    // Search for `_usr_` to find the workspace/user boundary, regardless of workspace ID format.
    if let Some(idx) = path.find("_usr_") {
        return &path[idx + 1..];
    }
    // Legacy fallback: check for known workspace_id prefix formats.
    if let Some(idx) = path.find('_') {
        let prefix = &path[..idx];
        if prefix.starts_with("ws-") || prefix.starts_with("workspace-") {
            return &path[idx + 1..];
        }
    }
    path
}

/// Handle a client→server message on the authenticated WebSocket.
async fn handle_client_message(
    text: &str,
    user_id: &str,
    _manager: &kyomi_auth::websocket::WebSocketManager,
    db: &kyomi_core::DbPool,
    cancel_registry: &crate::cancel_registry::CancelRegistry,
) {
    let msg: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!("WS received invalid JSON from user {user_id}");
            return;
        }
    };

    let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match msg_type {
        "cancel_request" => {
            let session_id = msg.get("session_id").and_then(|v| v.as_str());
            if let Some(sid) = session_id {
                let cancelled = cancel_registry.cancel(user_id, sid);
                tracing::info!(user_id, session_id = sid, cancelled, "cancel_request");
            } else {
                tracing::warn!(user_id, "cancel_request missing session_id");
            }
        }
        "oauth_cancel" => {
            tracing::info!(user_id, "Received oauth_cancel");
            // Set oauth_reconnect_cancelled flag in user's extra_metadata.oauth_data
            let is_pg = db.is_postgres();
            let sql = if is_pg {
                "UPDATE users \
                 SET extra_metadata = jsonb_set(\
                     COALESCE(extra_metadata::jsonb, '{}'::jsonb), \
                     '{oauth_data,oauth_reconnect_cancelled}', \
                     'true'::jsonb\
                 )::json \
                 WHERE user_id = $1"
            } else {
                "UPDATE users \
                 SET extra_metadata = json_set(\
                     COALESCE(extra_metadata, '{}'), \
                     '$.oauth_data.oauth_reconnect_cancelled', \
                     json('true')\
                 ) \
                 WHERE user_id = $1"
            };
            if let Err(e) = kyomi_core::db_execute!(db, sql, user_id)
            {
                tracing::error!("Failed to set oauth_reconnect_cancelled for {user_id}: {e}");
            }
        }
        _ => {
            tracing::debug!(user_id, msg_type, "Received unknown client message type");
        }
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Close a WebSocket with a custom close code and reason.
async fn close_with_code(socket: ws::WebSocket, code: u16, reason: &str) {
    let (mut sender, _) = socket.split();
    let close_frame = ws::CloseFrame {
        code,
        reason: reason.to_string().into(),
    };
    let _ = sender.send(ws::Message::Close(Some(close_frame))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_user_id_plain() {
        assert_eq!(extract_user_id_from_path("user-abc123"), "user-abc123");
    }

    #[test]
    fn extract_user_id_with_ws_prefix() {
        assert_eq!(
            extract_user_id_from_path("ws-550e8400-e29b-41d4-a716-446655440000_user-abc123"),
            "user-abc123"
        );
    }

    #[test]
    fn extract_user_id_with_workspace_prefix() {
        assert_eq!(
            extract_user_id_from_path("workspace-99f24d05-673d25b8_user-PHjsNsAj8hqZXOGGM-em1Q"),
            "user-PHjsNsAj8hqZXOGGM-em1Q"
        );
    }

    #[test]
    fn extract_user_id_with_underscore_in_user_id() {
        assert_eq!(
            extract_user_id_from_path("ws-uuid-here_user-abc_123"),
            "user-abc_123"
        );
    }

    #[test]
    fn extract_user_id_no_workspace_prefix_with_underscore() {
        assert_eq!(extract_user_id_from_path("user-abc_123"), "user-abc_123");
    }

    #[test]
    fn extract_user_id_with_non_standard_workspace_prefix() {
        assert_eq!(
            extract_user_id_from_path("e2e-test-workspace-0001_usr_a0bda4c2e7af4be3a29d"),
            "usr_a0bda4c2e7af4be3a29d"
        );
    }

}
