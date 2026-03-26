// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket endpoints for real-time communication.
//!
//! Two endpoints:
//! - `GET /ws/{user_id}` — Authenticated WebSocket for logged-in users
//! - `GET /ws/trial/{session_id}` — Trial WebSocket for anonymous sessions
//!
//! Wire-compatible with the Python backend's WebSocket protocol.

use axum::{
    extract::{ws, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

use kyomi_auth::{jwt, user_service};

use crate::state::AppState;

/// Query parameters for WebSocket authentication.
#[derive(Debug, Deserialize)]
pub struct WsAuthParams {
    /// JWT or trial token for authentication.
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

// ---------------------------------------------------------------------------
// GET /ws/trial/{session_id} — Trial WebSocket
// ---------------------------------------------------------------------------

/// Trial WebSocket upgrade handler.
///
/// For anonymous trial users. Validates HMAC-signed trial token, then
/// subscribes to Redis pub/sub channel `ws:trial:{session_id}` and forwards
/// messages to the WebSocket client.
///
/// Returns 503 before upgrading if `REDIS_URL` is not configured — trial
/// chat requires Redis for pub/sub message delivery.
pub async fn ws_trial_handler(
    ws: ws::WebSocketUpgrade,
    State(state): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    Path(session_id): Path<String>,
    Query(params): Query<WsAuthParams>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Trial WebSocket requires Redis for pub/sub — reject before upgrading if absent.
    let Some(redis_url) = state.config.redis_url.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Trial chat requires Redis (not configured)" })),
        )
            .into_response();
    };

    ws.max_message_size(MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| {
            handle_trial_ws(socket, state, session_id, params, headers, peer, redis_url)
        })
}

async fn handle_trial_ws(
    socket: ws::WebSocket,
    state: AppState,
    session_id: String,
    params: WsAuthParams,
    headers: HeaderMap,
    peer: std::net::SocketAddr,
    redis_url: String,
) {
    // --- Token validation ---
    let token = match params.token {
        Some(t) if !t.is_empty() => t,
        _ => {
            close_with_code(socket, CLOSE_AUTH_REQUIRED, "Missing access token").await;
            return;
        }
    };

    let client_ip = extract_client_ip_with_peer(&headers, peer);

    if let Err(e) = validate_trial_ws_token(&state.config.jwt_secret, &token, &client_ip) {
        tracing::warn!("Trial WS invalid token from {client_ip}: {e}");
        close_with_code(socket, CLOSE_AUTH_REQUIRED, "Invalid token").await;
        return;
    }

    let session_prefix = session_id[..8.min(session_id.len())].to_string();
    tracing::info!("Trial WS connected: session={session_prefix}..., ip={client_ip}");

    // --- Split socket ---
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Send connected confirmation
    let connected_msg = serde_json::json!({
        "type": "connected",
        "session_id": session_id,
        "message": "Connected to trial thinking events"
    });
    if ws_sender
        .send(ws::Message::text(connected_msg.to_string()))
        .await
        .is_err()
    {
        return;
    }

    // --- Subscribe to Redis channel ---
    let redis_channel = format!("ws:trial:{session_id}");

    let client = match redis::Client::open(redis_url.as_str()) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Trial WS Redis client creation failed: {e}");
            let _ = ws_sender.close().await;
            return;
        }
    };

    let mut pubsub = match client.get_async_pubsub().await {
        Ok(ps) => ps,
        Err(e) => {
            tracing::error!("Trial WS Redis SUBSCRIBE connection failed: {e}");
            let _ = ws_sender.close().await;
            return;
        }
    };

    if let Err(e) = pubsub.subscribe(&redis_channel).await {
        tracing::error!("Trial WS Redis SUBSCRIBE to {redis_channel} failed: {e}");
        let _ = ws_sender.close().await;
        return;
    }

    // Spawn a task to bridge Redis pub/sub messages to the WebSocket sender,
    // with periodic pings to keep the connection alive through Cloudflare (100s).
    let redis_task = {
        let mut stream = pubsub.into_on_message();
        tokio::spawn(async move {
            let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(45));
            ping_interval.tick().await; // consume immediate first tick

            loop {
                tokio::select! {
                    msg = stream.next() => {
                        match msg {
                            Some(msg) => {
                                let payload: String = match msg.get_payload() {
                                    Ok(p) => p,
                                    Err(e) => {
                                        tracing::warn!("Trial WS bad Redis payload: {e}");
                                        continue;
                                    }
                                };
                                if ws_sender.send(ws::Message::text(payload)).await.is_err() {
                                    break;
                                }
                            }
                            None => break, // stream ended
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
        })
    };

    // Handle client messages (ping/pong keepalive, close)
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                ws::Message::Text(ref text) if text.as_str() == "ping" => {
                    // Application-level ping from trial client
                    tracing::trace!("Trial WS application ping from session {session_id}");
                }
                ws::Message::Close(_) => break,
                _ => {} // Axum handles WS-level ping/pong automatically
            }
        }
    });

    // Wait for either side to finish
    tokio::select! {
        _ = redis_task => {}
        _ = recv_task => {}
    }

    tracing::info!("Trial WS disconnected: session={session_prefix}...");
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

/// Extract client IP with peer socket address fallback (for trial WebSocket).
///
/// Delegates to the shared [`crate::helpers::extract_client_ip`] which uses the
/// same header priority as `trial_chat.rs` — ensuring consistent HMAC IP binding
/// between HTTP endpoints and WebSocket connections.
fn extract_client_ip_with_peer(headers: &HeaderMap, peer: std::net::SocketAddr) -> String {
    crate::helpers::extract_client_ip(headers, Some(peer))
}

/// Validate a trial access token (HMAC-SHA256 signed, IP-bound, time-limited).
///
/// Wire format: `{session_token}:{expires_at}:{signature_32hex}`
/// HMAC payload: `{session_token}:{ip}:{expires_at}` — IP is baked into
/// the signature but not present in the wire token (matches Python backend).
fn validate_trial_ws_token(
    secret: &str,
    token: &str,
    expected_ip: &str,
) -> Result<String, String> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;

    // Split into session_token:expiry:signature (3 colon-separated parts).
    let parts: Vec<&str> = token.splitn(3, ':').collect();
    if parts.len() != 3 {
        return Err("Invalid token format".into());
    }

    let session_token = parts[0];
    let expires_at_str = parts[1];
    let provided_sig = parts[2];

    // Reconstruct HMAC payload: session_token:ip:expiry (IP included for binding).
    let payload = format!("{session_token}:{expected_ip}:{expires_at_str}");
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    let result = mac.finalize().into_bytes();
    // Truncate to 32 hex chars to match Python's hexdigest()[:32].
    let expected_sig: String = result.iter().take(16).map(|b| format!("{b:02x}")).collect();

    if provided_sig != expected_sig {
        return Err("Invalid token signature".into());
    }

    let expires_at: i64 = expires_at_str.parse().map_err(|_| "Invalid expiry")?;
    let now = chrono::Utc::now().timestamp();
    if now > expires_at {
        return Err("Token expired".into());
    }

    Ok(session_token.to_string())
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

    #[test]
    fn trial_token_validation_rejects_bad_signature() {
        // Wire format: session:expiry:signature
        let result = validate_trial_ws_token("secret", "session:9999999999:badsig", "127.0.0.1");
        assert!(result.is_err());
    }

    #[test]
    fn trial_token_validation_rejects_expired() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let secret = "test-secret";
        let expiry = "1000000000"; // expired long ago
        // HMAC payload includes IP: session_token:ip:expiry
        let hmac_payload = format!("session123:127.0.0.1:{expiry}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(hmac_payload.as_bytes());
        let sig: String = mac.finalize().into_bytes().iter().take(16).map(|b| format!("{b:02x}")).collect();
        // Wire format: session_token:expiry:signature (no IP)
        let token = format!("session123:{expiry}:{sig}");

        let result = validate_trial_ws_token(secret, &token, "127.0.0.1");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expired"));
    }

    #[test]
    fn trial_token_validation_rejects_ip_mismatch() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let secret = "test-secret";
        let expires = chrono::Utc::now().timestamp() + 3600;
        // HMAC payload uses the ORIGINAL IP (10.0.0.1)
        let hmac_payload = format!("session123:10.0.0.1:{expires}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(hmac_payload.as_bytes());
        let sig: String = mac.finalize().into_bytes().iter().take(16).map(|b| format!("{b:02x}")).collect();
        // Wire format has no IP
        let token = format!("session123:{expires}:{sig}");

        // Validate with DIFFERENT IP — signature won't match
        let result = validate_trial_ws_token(secret, &token, "192.168.1.1");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("signature"));
    }

    #[test]
    fn trial_token_validation_accepts_valid() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let secret = "test-secret";
        let expires = chrono::Utc::now().timestamp() + 3600;
        // HMAC payload: session_token:ip:expiry
        let hmac_payload = format!("session-abc:127.0.0.1:{expires}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(hmac_payload.as_bytes());
        let sig: String = mac.finalize().into_bytes().iter().take(16).map(|b| format!("{b:02x}")).collect();
        // Wire format: session_token:expiry:signature
        let token = format!("session-abc:{expires}:{sig}");

        let result = validate_trial_ws_token(secret, &token, "127.0.0.1");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "session-abc");
    }
}
