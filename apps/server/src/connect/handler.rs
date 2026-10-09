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
use kyomi_core::DbPool;

use crate::state::AppState;

use super::extract_bearer_token;
use super::registry::{CommandPayload, ConnectRegistry, ResponseChannel};

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
    let (connection_id, revoked) = match state.connect_registry.register_authenticated(&dsid, &claims.jti, cmd_tx).await {
        Ok(connection) => connection,
        Err(e) => {
            tracing::warn!(datasource_config_id = %dsid, error = %e, "Connect WS registration rejected");
            close_with_code(socket, CLOSE_FORBIDDEN, "Token revoked").await;
            return;
        }
    };

    // A rotation can commit between the first JTI read and registration.
    if !session_is_current(&state.db, &state.connect_registry, &dsid, &claims.wid, &claims.jti).await {
        state.connect_registry.unregister(&dsid, connection_id).await;
        close_with_code(socket, CLOSE_FORBIDDEN, "Token revoked").await;
        return;
    }

    // Start Redis command subscriber for cross-replica routing.
    // Other pods can forward commands to this connection via Redis pub/sub.
    state.connect_registry.start_command_subscriber(&dsid, connection_id);

    // Run the message loop
    run_message_loop(socket, cmd_rx, revoked, ConnectSessionContext {
        db: &state.db,
        registry: &state.connect_registry,
        datasource_config_id: &dsid,
        workspace_id: &claims.wid,
        jti: &claims.jti,
        connection_id,
    }).await;

    // Cleanup on disconnect — removes connection, subscriber, and Redis presence key
    // (only if this connection still owns the entry)
    state.connect_registry.unregister(&dsid, connection_id).await;
    tracing::info!(
        datasource_config_id = %dsid,
        "Connect WebSocket disconnected"
    );
}

/// Main message loop — multiplexes commands, WebSocket messages, and heartbeats.
struct ConnectSessionContext<'a> {
    db: &'a DbPool,
    registry: &'a ConnectRegistry,
    datasource_config_id: &'a str,
    workspace_id: &'a str,
    jti: &'a str,
    connection_id: u64,
}

async fn run_message_loop(
    socket: WebSocket,
    mut cmd_rx: mpsc::Receiver<CommandPayload>,
    mut revoked: tokio::sync::watch::Receiver<bool>,
    context: ConnectSessionContext<'_>,
) {
    let ConnectSessionContext {
        db,
        registry,
        datasource_config_id,
        workspace_id,
        jti,
        connection_id,
    } = context;
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Track pending commands by request ID so we can route responses.
    // Supports both oneshot (single response) and mpsc (streaming) channels.
    let mut pending: HashMap<String, ResponseChannel> = HashMap::new();

    let mut heartbeat_interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat_interval.tick().await; // consume the immediate first tick

    let mut last_pong = Instant::now();
    let mut auth_interval = tokio::time::interval(Duration::from_secs(1));
    auth_interval.tick().await;
    let mut revoked_session = false;

    loop {
        tokio::select! {
            biased;
            _ = revoked.changed() => {
                tracing::info!(datasource_config_id, "Connect session revoked");
                revoked_session = true;
                break;
            }
            _ = auth_interval.tick() => {
                if !session_is_current(db, registry, datasource_config_id, workspace_id, jti).await {
                    revoked_session = true;
                    break;
                }
            }
            // --- New command from the registry ---
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some((request, response_channel)) => {
                        if *revoked.borrow() || !session_is_current(db, registry, datasource_config_id, workspace_id, jti).await {
                            drop(response_channel);
                            revoked_session = true;
                            break;
                        }
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

                        match send_while_current(&mut revoked, ws_sender.send(ws::Message::text(json))).await {
                            Ok(true) => {}
                            Ok(false) => {
                                revoked_session = true;
                                break;
                            }
                            Err(_) => {
                                tracing::warn!(datasource_config_id, request_id, "Failed to send command over Connect WebSocket");
                                revoked_session = true;
                                break;
                            }
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
                        if *revoked.borrow() || !session_is_current(db, registry, datasource_config_id, workspace_id, jti).await {
                            revoked_session = true;
                            break;
                        }
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
                        registry.refresh_heartbeat(datasource_config_id, connection_id).await;
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

                match send_while_current(&mut revoked, ws_sender.send(ws::Message::Ping(vec![].into()))).await {
                    Ok(true) => {}
                    Ok(false) => {
                        revoked_session = true;
                        break;
                    }
                    Err(_) => {
                        tracing::warn!(datasource_config_id, "Failed to send heartbeat ping");
                        revoked_session = true;
                        break;
                    }
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

    // A canceled SinkExt::send may already have queued a command inside the
    // WebSocket sink. Graceful close flushes that queue, so revoke by dropping
    // the transport. Ordinary disconnects can still send a close frame.
    if !revoked_session {
        let _ = tokio::time::timeout(Duration::from_secs(1), ws_sender.close()).await;
    }
}

async fn session_is_current(db: &DbPool, registry: &ConnectRegistry, datasource_config_id: &str, workspace_id: &str, jti: &str) -> bool {
    if !matches!(registry.is_revoked(datasource_config_id, jti).await, Ok(false)) {
        return false;
    }
    matches!(
        kyomi_auth::datasource_service::get_datasource(db, datasource_config_id, workspace_id).await,
        Ok(Some(ds)) if ds.connection_type == "connect" && ds.connect_token_jti.as_deref() == Some(jti)
    )
}

async fn send_while_current<F, E>(revoked: &mut tokio::sync::watch::Receiver<bool>, send: F) -> Result<bool, E>
where
    F: std::future::Future<Output = Result<(), E>>,
{
    if *revoked.borrow() {
        return Ok(false);
    }
    tokio::select! {
        biased;
        _ = revoked.changed() => Ok(false),
        result = send => result.map(|_| true),
    }
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
    use axum::routing::get;
    use axum::Router;
    use p256::pkcs8::EncodePrivateKey;

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

    #[tokio::test]
    async fn revoked_session_cancels_a_backpressured_websocket_send() {
        use futures_util::{Sink, SinkExt};
        use std::pin::Pin;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::task::{Context, Poll};

        struct BufferedSink {
            queued: std::sync::Arc<AtomicBool>,
            flushed: std::sync::Arc<AtomicBool>,
        }

        impl Sink<&'static str> for BufferedSink {
            type Error = ();

            fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                Poll::Ready(Ok(()))
            }

            fn start_send(self: Pin<&mut Self>, _item: &'static str) -> Result<(), Self::Error> {
                self.queued.store(true, Ordering::SeqCst);
                Ok(())
            }

            fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                Poll::Pending
            }

            fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                self.flushed.store(true, Ordering::SeqCst);
                Poll::Ready(Ok(()))
            }
        }

        let queued = std::sync::Arc::new(AtomicBool::new(false));
        let flushed = std::sync::Arc::new(AtomicBool::new(false));
        let mut sink = BufferedSink { queued: queued.clone(), flushed: flushed.clone() };
        let (revoke, mut receiver) = tokio::sync::watch::channel(false);
        let sending = tokio::spawn(async move {
            let result = send_while_current(&mut receiver, sink.send("old command")).await;
            // The production revoked path drops the sink without poll_close.
            drop(sink);
            result
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while !queued.load(Ordering::SeqCst) { tokio::task::yield_now().await; }
        }).await.unwrap();
        revoke.send(true).unwrap();
        assert_eq!(tokio::time::timeout(Duration::from_millis(100), sending).await.unwrap().unwrap(), Ok(false));
        assert!(!flushed.load(Ordering::SeqCst), "revocation must not flush a queued command");
    }

    #[tokio::test]
    async fn rotation_and_disconnect_close_live_websocket_and_pending_command() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        async fn read_frame(socket: &mut tokio::net::TcpStream) -> (u8, Vec<u8>) {
            let mut header = [0_u8; 2];
            socket.read_exact(&mut header).await.unwrap();
            let length = match header[1] & 0x7f {
                n @ 0..=125 => n as usize,
                126 => {
                    let mut extended = [0; 2];
                    socket.read_exact(&mut extended).await.unwrap();
                    u16::from_be_bytes(extended) as usize
                }
                _ => panic!("unexpected large test frame"),
            };
            let mut payload = vec![0; length];
            socket.read_exact(&mut payload).await.unwrap();
            (header[0] & 0x0f, payload)
        }

        async fn connect(address: std::net::SocketAddr, token: &str) -> tokio::net::TcpStream {
            let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
            socket.write_all(format!(
                "GET /test HTTP/1.1\r\nHost: {address}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nAuthorization: Bearer {token}\r\n\r\n"
            ).as_bytes()).await.unwrap();
            let mut handshake = Vec::new();
            while !handshake.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).await.unwrap();
                handshake.push(byte[0]);
            }
            assert!(handshake.starts_with(b"HTTP/1.1 101"));
            socket
        }

        let db = DbPool::connect("sqlite::memory:").await.unwrap();
        let DbPool::Sqlite(sqlite) = &db else { panic!("test requires SQLite"); };
        sqlx::query("INSERT INTO users (user_id, email) VALUES ('u-connect', 'connect-test@example.com')")
            .execute(sqlite).await.unwrap();
        sqlx::query("INSERT INTO workspaces (workspace_id, owner_user_id) VALUES ('w-connect', 'u-connect')")
            .execute(sqlite).await.unwrap();

        for disconnect in [false, true] {
            let dsid = if disconnect { "ds-disconnect" } else { "ds-rotate" };
            sqlx::query("INSERT INTO datasource_configs (id, workspace_id, name, datasource_type, slug, connection_type, connect_token_jti) VALUES (?1, 'w-connect', ?1, 'postgres', ?1, 'connect', 'old-jti')")
                .bind(dsid).execute(sqlite).await.unwrap();
            let config = std::sync::Arc::new(kyomi_core::Config::test_config());
            let kv = kyomi_core::create_kv_store(None).await.unwrap();
            let pem = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng)
                .to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap();
            let token_service = std::sync::Arc::new(
                kyomi_auth::connect_token::ConnectTokenService::new(&pem, "wss://connect.test/v1").unwrap()
            );
            let (old_token, old_jti) = token_service.generate(dsid, "w-connect", "postgres").unwrap();
            kyomi_auth::datasource_service::update_connect_jti(&db, dsid, &old_jti).await.unwrap();
            let registry = ConnectRegistry::new_local();
            let webauthn = kyomi_auth::webauthn::build_webauthn(
                "localhost", "Kyomi Test", &url::Url::parse("http://localhost:5173").unwrap()
            ).unwrap();
            let state = AppState {
                db: db.clone(),
                kv: kv.clone(),
                redis: None,
                config,
                encryption_key: std::sync::Arc::new([0; 32]),
                webauthn: std::sync::Arc::new(webauthn),
                embedding: kyomi_embed::LazyEmbedding::new(),
                ws_manager: kyomi_auth::websocket::WebSocketManager::new(None, db.clone()),
                stripe: None,
                mcp_sessions: kyomi_auth::mcp_session_manager::MCPSessionManager::new(kv),
                cancel_registry: crate::cancel_registry::CancelRegistry::default(),
                connect_token: Some(token_service.clone()),
                connect_registry: registry.clone(),
                platforms: std::sync::Arc::new(kyomi_core::platform::PlatformRegistry::new()),
                schema_drift: crate::schema_drift::SchemaDriftStatus::default(),
                process_instance: "connect-test".into(),
            };
            let app = Router::new().route("/test", get(connect_websocket_handler)).with_state(state);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
            let mut socket = connect(address, &old_token).await;
            tokio::time::timeout(Duration::from_secs(2), async {
                while !registry.is_connected(dsid).await { tokio::task::yield_now().await; }
            }).await.unwrap();

            let command = tokio::spawn({
                let registry = registry.clone();
                let dsid = dsid.to_owned();
                async move { registry.send_command(&dsid, kyomi_core::connect_protocol::ConnectRequest {
                    id: "pending".into(), op: kyomi_core::connect_protocol::ConnectOp::TestConnection,
                    params: None, streaming: false,
                }, Duration::from_secs(30)).await }
            });
            let (opcode, _) = tokio::time::timeout(Duration::from_secs(2), read_frame(&mut socket)).await.unwrap();
            assert_eq!(opcode, 1);

            if disconnect {
                kyomi_auth::datasource_service::clear_connect_jti(&db, dsid).await.unwrap();
            } else {
                kyomi_auth::datasource_service::update_connect_jti(&db, dsid, "new-jti").await.unwrap();
            }
            registry.revoke_generation(dsid, &old_jti).await.unwrap();
            assert!(!session_is_current(&db, &registry, dsid, "w-connect", &old_jti).await);
            let mut next = [0_u8; 2];
            let bytes = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut next)).await.unwrap().unwrap();
            assert_eq!(bytes, 0, "revoked socket must close without flushing a queued frame");
            assert!(tokio::time::timeout(Duration::from_secs(2), command).await.unwrap().unwrap().is_err());

            let mut denied = connect(address, &old_token).await;
            let (closed, frame) = tokio::time::timeout(Duration::from_secs(2), read_frame(&mut denied)).await.unwrap();
            assert_eq!(closed, 8);
            assert_eq!(u16::from_be_bytes([frame[0], frame[1]]), CLOSE_FORBIDDEN);
            if !disconnect {
                let (new_token, new_jti) = token_service.generate(dsid, "w-connect", "postgres").unwrap();
                kyomi_auth::datasource_service::update_connect_jti(&db, dsid, &new_jti).await.unwrap();
                let _replacement = connect(address, &new_token).await;
                tokio::time::timeout(Duration::from_secs(2), async {
                    while !registry.is_connected(dsid).await { tokio::task::yield_now().await; }
                }).await.unwrap();
            }
            server.abort();
        }
    }
}
