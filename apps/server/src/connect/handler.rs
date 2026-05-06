// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket handler for Kyomi Connect connections.
//!
//! Accepts inbound WebSocket connections from the customer-deployed Connect
//! binary.  Authentication uses JWT tokens (ES256) issued by the Connect token
//! service, verified against the stored `connect_token_jti` for revocation.
//!
//! ## Message loop
//!
//! The handler multiplexes three event sources via `tokio::select!`:
//!
//! 1. **Commands from the registry** (via mpsc) — serialized as JSON and sent
//!    over the WebSocket to Connect.
//! 2. **Messages from Connect** (via WebSocket) — deserialized as
//!    `ConnectResponse` and routed back through the matching oneshot channel.
//! 3. **Heartbeat timer** — sends WebSocket Ping frames every 30 seconds and
//!    closes the connection if no Pong is received within 40 seconds.

use std::collections::HashMap;
use std::error::Error as StdError;
use std::time::Duration;

use axum::extract::ws::{self, WebSocket};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite;

use kyomi_core::connect_protocol::{ConnectResponse, ConnectResponseBody};

use crate::state::AppState;

use super::extract_bearer_token;
use super::registry::{CommandPayload, ResponseChannel};

/// Maximum size of a single WebSocket message from Connect (16 MB).
///
/// Connect responses can contain query result sets, so they need more room
/// than the 64 KB limit on user WebSockets.  16 MB accommodates large result
/// sets while still providing a safety cap until streaming is implemented.
const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Heartbeat ping interval.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Maximum time without a pong before we consider the connection dead.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(40);

/// Buffer size for the per-connection command channel.
///
/// Commands are dispatched one at a time (send request, await response), so a
/// small buffer is sufficient.  If the buffer fills up, callers will wait
/// asynchronously until a slot opens.
const COMMAND_CHANNEL_BUFFER: usize = 32;

/// WebSocket close codes.
const CLOSE_AUTH_REQUIRED: u16 = 4001;
const CLOSE_FORBIDDEN: u16 = 4003;

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Axum handler for `GET /connect/v1`.
///
/// Extracts the JWT from the `Authorization: Bearer <token>` header, verifies
/// it, loads the datasource config, checks the `jti` for revocation, and
/// upgrades to a WebSocket.
pub async fn connect_websocket_handler(
    ws: ws::WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    ws.max_message_size(MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| handle_connect_ws(socket, state, headers))
}

/// Post-upgrade handler.  Runs authentication checks and then enters the
/// message loop.
async fn handle_connect_ws(socket: WebSocket, state: AppState, headers: HeaderMap) {
    // -----------------------------------------------------------------------
    // 1. Extract Bearer token from Authorization header
    // -----------------------------------------------------------------------
    let token = match extract_bearer_token(&headers) {
        Some(t) => t,
        None => {
            tracing::warn!("Connect WS rejected: missing or invalid Authorization header");
            close_with_code(socket, CLOSE_AUTH_REQUIRED, "Authorization header required").await;
            return;
        }
    };

    // -----------------------------------------------------------------------
    // 2. Verify JWT via ConnectTokenService
    // -----------------------------------------------------------------------
    let connect_token_service = match &state.connect_token {
        Some(svc) => svc.clone(),
        None => {
            tracing::warn!("Connect WS rejected: Connect token service not configured");
            close_with_code(socket, CLOSE_FORBIDDEN, "Connect not configured").await;
            return;
        }
    };

    let claims = match connect_token_service.verify(&token) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "Connect WS JWT verification failed");
            close_with_code(socket, CLOSE_AUTH_REQUIRED, "Invalid token").await;
            return;
        }
    };

    let datasource_config_id = &claims.dsid;
    let workspace_id = &claims.wid;

    // -----------------------------------------------------------------------
    // 3. Load datasource config and verify connection_type + jti
    // -----------------------------------------------------------------------
    let ds_config = match kyomi_auth::datasource_service::get_datasource(
        &state.db,
        datasource_config_id,
        workspace_id,
    )
    .await
    {
        Ok(Some(ds)) => ds,
        Ok(None) => {
            tracing::warn!(
                datasource_config_id,
                workspace_id,
                "Connect WS rejected: datasource not found"
            );
            close_with_code(socket, CLOSE_FORBIDDEN, "Datasource not found").await;
            return;
        }
        Err(e) => {
            tracing::error!(
                datasource_config_id,
                error = %e,
                "Connect WS rejected: database error loading datasource"
            );
            close_with_code(socket, CLOSE_FORBIDDEN, "Internal error").await;
            return;
        }
    };

    // Must be a "connect" type datasource
    if ds_config.connection_type != "connect" {
        tracing::warn!(
            datasource_config_id,
            connection_type = %ds_config.connection_type,
            "Connect WS rejected: datasource is not a Connect type"
        );
        close_with_code(socket, CLOSE_FORBIDDEN, "Datasource is not Connect type").await;
        return;
    }

    // Verify the token's jti matches the stored jti (revocation check)
    match &ds_config.connect_token_jti {
        Some(stored_jti) if stored_jti == &claims.jti => {
            // Token is current — proceed
        }
        Some(_) => {
            tracing::warn!(
                datasource_config_id,
                "Connect WS rejected: token has been revoked (jti mismatch)"
            );
            close_with_code(socket, CLOSE_FORBIDDEN, "Token revoked").await;
            return;
        }
        None => {
            tracing::warn!(
                datasource_config_id,
                "Connect WS rejected: no token jti stored (token not yet issued?)"
            );
            close_with_code(socket, CLOSE_FORBIDDEN, "Token not recognized").await;
            return;
        }
    }

    // -----------------------------------------------------------------------
    // 4. Authentication passed — set up connection
    // -----------------------------------------------------------------------
    let dsid = datasource_config_id.to_string();
    tracing::info!(
        datasource_config_id = %dsid,
        datasource_name = %ds_config.name,
        datasource_type = %ds_config.datasource_type,
        "Connect WebSocket authenticated"
    );

    // Create the command channel and register with the registry
    let (cmd_tx, cmd_rx) = mpsc::channel::<CommandPayload>(COMMAND_CHANNEL_BUFFER);
    let connection_id = state.connect_registry.register(&dsid, cmd_tx).await;

    // Start Redis command subscriber for cross-replica routing.
    // Other pods can forward commands to this connection via Redis pub/sub.
    state.connect_registry.start_command_subscriber(&dsid, connection_id);

    // Run the message loop
    run_message_loop(socket, cmd_rx, &state, &dsid, connection_id).await;

    // Cleanup on disconnect — removes connection, subscriber, and Redis presence key
    // (only if this connection still owns the entry)
    state.connect_registry.unregister(&dsid, connection_id).await;
    tracing::info!(
        datasource_config_id = %dsid,
        "Connect WebSocket disconnected"
    );
}

/// Main message loop — multiplexes commands, WebSocket messages, and heartbeats.
async fn run_message_loop(
    socket: WebSocket,
    mut cmd_rx: mpsc::Receiver<CommandPayload>,
    state: &AppState,
    datasource_config_id: &str,
    connection_id: u64,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Track pending commands by request ID so we can route responses.
    // Supports both oneshot (single response) and mpsc (streaming) channels.
    let mut pending: HashMap<String, ResponseChannel> = HashMap::new();

    let mut heartbeat_interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat_interval.tick().await; // consume the immediate first tick

    let mut last_pong = Instant::now();

    loop {
        tokio::select! {
            // --- New command from the registry ---
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some((request, response_channel)) => {
                        let request_id = request.id.clone();
                        let json = match serde_json::to_string(&request) {
                            Ok(j) => j,
                            Err(e) => {
                                tracing::error!(
                                    datasource_config_id,
                                    error = %e,
                                    "Failed to serialize ConnectRequest"
                                );
                                // Drop the channel — caller will get a RecvError
                                drop(response_channel);
                                continue;
                            }
                        };

                        pending.insert(request_id.clone(), response_channel);

                        if ws_sender.send(ws::Message::text(json)).await.is_err() {
                            tracing::warn!(
                                datasource_config_id,
                                request_id,
                                "Failed to send command over Connect WebSocket"
                            );
                            break;
                        }
                    }
                    None => {
                        // Command channel closed — registry dropped us
                        tracing::debug!(datasource_config_id, "Command channel closed");
                        break;
                    }
                }
            }

            // --- Message from Connect (response or close) ---
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(ws::Message::Text(text))) => {
                        let byte_size = text.len();
                        tracing::debug!(
                            datasource_config_id,
                            byte_size,
                            "Received Connect response"
                        );

                        match serde_json::from_str::<ConnectResponse>(&text) {
                            Ok(response) => {
                                route_response(&mut pending, datasource_config_id, response).await;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    datasource_config_id,
                                    error = %e,
                                    "Failed to deserialize Connect message as ConnectResponse"
                                );
                            }
                        }
                    }
                    Some(Ok(ws::Message::Pong(_))) => {
                        // Heartbeat pong received — refresh Redis presence key
                        last_pong = Instant::now();
                        state.connect_registry.refresh_heartbeat(datasource_config_id, connection_id).await;
                    }
                    Some(Ok(ws::Message::Close(_))) => {
                        tracing::info!(datasource_config_id, "Connect sent Close frame");
                        break;
                    }
                    Some(Ok(_)) => {
                        // Ping, Binary, etc. — ignore
                    }
                    Some(Err(e)) => {
                        // Downcast through axum::Error → tungstenite::Error to
                        // detect oversized messages structurally (not by string matching).
                        let too_long = e.source()
                            .and_then(|src| src.downcast_ref::<tungstenite::Error>())
                            .and_then(|te| match te {
                                tungstenite::Error::Capacity(
                                    tungstenite::error::CapacityError::MessageTooLong { size, max_size }
                                ) => Some((*size, *max_size)),
                                _ => None,
                            });

                        if let Some((actual_size, max_size)) = too_long {
                            tracing::error!(
                                datasource_config_id,
                                actual_byte_size = actual_size,
                                max_message_size = max_size,
                                "Connect response exceeded MAX_MESSAGE_SIZE — \
                                 query result too large for WebSocket transport"
                            );
                        } else {
                            tracing::warn!(
                                datasource_config_id,
                                error = %e,
                                "Connect WebSocket error"
                            );
                        }
                        break;
                    }
                    None => {
                        // Stream ended
                        tracing::debug!(datasource_config_id, "Connect WebSocket stream ended");
                        break;
                    }
                }
            }

            // --- Heartbeat timer ---
            _ = heartbeat_interval.tick() => {
                if last_pong.elapsed() > HEARTBEAT_TIMEOUT {
                    tracing::warn!(
                        datasource_config_id,
                        elapsed_secs = last_pong.elapsed().as_secs(),
                        "Connect heartbeat timeout — closing connection"
                    );
                    break;
                }

                if ws_sender.send(ws::Message::Ping(vec![].into())).await.is_err() {
                    tracing::warn!(datasource_config_id, "Failed to send heartbeat ping");
                    break;
                }
            }
        }
    }

    // Drop all pending commands — callers will get RecvError from their oneshot
    let pending_count = pending.len();
    if pending_count > 0 {
        tracing::warn!(
            datasource_config_id,
            pending_count,
            "Dropping pending commands on disconnect"
        );
    }
    drop(pending);

    // Try to send a close frame (best-effort)
    let _ = ws_sender.close().await;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Route a response to the appropriate pending channel.
///
/// For `ResponseChannel::Once`: sends the response and removes from pending.
/// For `ResponseChannel::Stream`: routes based on response type:
///   - `ArrowHeader`/`ArrowBatch`: send without removing (more messages coming)
///   - `ArrowComplete`: send and remove (stream finished, dropping tx signals end)
///   - `Result`/`Error`: send and remove (terminal messages)
async fn route_response(
    pending: &mut HashMap<String, ResponseChannel>,
    datasource_config_id: &str,
    response: ConnectResponse,
) {
    let is_terminal = matches!(
        &response.body,
        ConnectResponseBody::Result { .. }
            | ConnectResponseBody::Error { .. }
            | ConnectResponseBody::ArrowComplete { .. }
    );

    match pending.get(&response.id) {
        Some(ResponseChannel::Once(_)) => {
            // Oneshot: always remove and send
            if let Some(ResponseChannel::Once(tx)) = pending.remove(&response.id) {
                let _ = tx.send(response);
            }
        }
        Some(ResponseChannel::Stream(_)) => {
            if is_terminal {
                // Terminal message: send then remove (dropping tx closes the stream)
                if let Some(ResponseChannel::Stream(tx)) = pending.remove(&response.id) {
                    let _ = tx.send(response).await;
                }
            } else {
                // Non-terminal (Header, Chunk): send without removing
                let id = response.id.clone();
                if let Some(ResponseChannel::Stream(tx)) = pending.get(&id)
                    && tx.send(response).await.is_err() {
                        // Receiver dropped — clean up
                        pending.remove(&id);
                    }
            }
        }
        None => {
            tracing::warn!(
                datasource_config_id,
                response_id = %response.id,
                "Received response for unknown request ID"
            );
        }
    }
}

/// Close a WebSocket with a custom close code and reason.
async fn close_with_code(socket: WebSocket, code: u16, reason: &str) {
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
    fn extract_bearer_token_valid() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer my-jwt-token".parse().unwrap());
        assert_eq!(
            extract_bearer_token(&headers),
            Some("my-jwt-token".to_string())
        );
    }

    #[test]
    fn extract_bearer_token_missing_header() {
        let headers = HeaderMap::new();
        assert_eq!(extract_bearer_token(&headers), None);
    }

    #[test]
    fn extract_bearer_token_wrong_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Basic abc123".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers), None);
    }

    #[test]
    fn extract_bearer_token_empty_token() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer ".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers), None);
    }

    #[test]
    fn extract_bearer_token_no_space_after_bearer() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearertoken".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers), None);
    }

    #[test]
    fn max_message_size_is_16mb() {
        assert_eq!(MAX_MESSAGE_SIZE, 16 * 1024 * 1024);
    }

    #[test]
    fn detects_message_too_long_via_downcast() {
        use std::error::Error as StdError;

        // Construct the same error chain axum produces: axum::Error wrapping tungstenite::Error
        let tungstenite_err = tungstenite::Error::Capacity(
            tungstenite::error::CapacityError::MessageTooLong {
                size: 20_000_000,
                max_size: MAX_MESSAGE_SIZE,
            },
        );
        let axum_err = axum::Error::new(tungstenite_err);

        // Verify our downcast logic works
        let too_long = axum_err
            .source()
            .and_then(|src| src.downcast_ref::<tungstenite::Error>())
            .and_then(|te| match te {
                tungstenite::Error::Capacity(
                    tungstenite::error::CapacityError::MessageTooLong { size, max_size },
                ) => Some((*size, *max_size)),
                _ => None,
            });

        assert_eq!(too_long, Some((20_000_000, MAX_MESSAGE_SIZE)));
    }
}
