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

    // 2. Extract user_id and workspace_id from path — may be "{workspace_id}_{user_id}" format
    let actual_user_id = extract_user_id_from_path(&path_user_id);
    let workspace_id = extract_workspace_id_from_path(&path_user_id).to_string();

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
    let workspace_id_for_recv = workspace_id.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                ws::Message::Text(text) => {
                    handle_client_message(
                        &text,
                        &user_id_for_recv,
                        &workspace_id_for_recv,
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

/// Extract the workspace_id from a path that is "{workspace_id}_{user_id}".
///
/// Uses the same boundary detection as `extract_user_id_from_path` but returns
/// the prefix portion instead. Returns an empty string when no workspace prefix
/// is found (plain user_id paths).
fn extract_workspace_id_from_path(path: &str) -> &str {
    if let Some(idx) = path.find("_usr_") {
        return &path[..idx];
    }
    if let Some(idx) = path.find('_') {
        let prefix = &path[..idx];
        if prefix.starts_with("ws-") || prefix.starts_with("workspace-") {
            return &path[..idx];
        }
    }
    ""
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
    workspace_id: &str,
    manager: &kyomi_auth::websocket::WebSocketManager,
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
        "sync_bootstrap" => {
            handle_sync_bootstrap(manager, db, user_id, workspace_id).await;
        }
        "sync_delta" => {
            let last_sync_id = msg
                .get("last_sync_id")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            handle_sync_delta(manager, db, user_id, workspace_id, last_sync_id).await;
        }
        _ => {
            tracing::debug!(user_id, msg_type, "Received unknown client message type");
        }
    }
}

// ─── Sync protocol handlers ───────────────────────────────────────────────────

/// Handle a `sync_bootstrap` request.
///
/// Streams all Tier 1 entities for the workspace as `SyncAction` messages with
/// `action = insert`, then closes with a `SyncComplete` carrying the current
/// `latest_sync_id`. Clients should store this ID and use `sync_delta` for
/// subsequent reconnects.
async fn handle_sync_bootstrap(
    manager: &kyomi_auth::websocket::WebSocketManager,
    db: &kyomi_core::DbPool,
    user_id: &str,
    workspace_id: &str,
) {
    use kyomi_types::sync::{SyncResponse, entity_types};

    tracing::debug!(user_id, workspace_id, "Handling sync_bootstrap");

    // 1. Fetch all Tier 1 metadata. Kept as `Result`s (not yet unwrapped)
    // so counts (1b, below) can be derived from each fetch's success/failure
    // independently of the streaming step, which is allowed to degrade to
    // an empty vec on failure.
    let dashboards_res =
        kyomi_auth::dashboard_service::list_dashboards_for_sync(db, workspace_id, user_id).await;
    let knowledge_res =
        kyomi_auth::dashboard_service::list_knowledge_for_sync(db, workspace_id, user_id).await;
    let sessions_res =
        kyomi_auth::chat_service::list_sessions_for_sync(db, workspace_id, user_id).await;
    let watches_res =
        kyomi_auth::watch_service::list_watches_for_sync(db, workspace_id, user_id).await;
    let settings =
        kyomi_auth::workspace_service::get_workspace_settings_for_sync(db, workspace_id).await;

    // 1b. Per-entity-type counts for the sync_complete payload (KYO-480).
    // Derived from each fetch's `Result` *before* it is unwrapped for
    // streaming below (free: no additional queries beyond the ones already
    // run for step 1). Routed through `insert_count_if_present` — the same
    // function `compute_sync_counts` (the delta path) uses — so a failed
    // fetch is omitted from `counts` rather than reported as `0` in both
    // places, structurally, not just here. See `insert_count_if_present`'s
    // doc comment for why a bootstrap-path `0` is worse than a delta-path
    // one: every repair goes through this same untargeted bootstrap
    // (`cache::sync_engine`), so a transient failure here can either mark a
    // genuinely diverged type as falsely repaired, or fabricate a brand-new
    // false divergence in an unrelated, correctly-cached type.
    let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    insert_count_if_present(
        &mut counts,
        entity_types::DASHBOARD,
        dashboards_res.as_ref().ok().map(|v| v.len() as i64),
    );
    insert_count_if_present(
        &mut counts,
        entity_types::KNOWLEDGE,
        knowledge_res.as_ref().ok().map(|v| v.len() as i64),
    );
    insert_count_if_present(
        &mut counts,
        entity_types::CHAT_SESSION,
        sessions_res.as_ref().ok().map(|v| v.len() as i64),
    );
    insert_count_if_present(
        &mut counts,
        entity_types::WATCH,
        watches_res.as_ref().ok().map(|v| v.len() as i64),
    );

    // Unwrap each `Result` for streaming now — degrading to an empty vec on
    // failure is fine here (pre-existing behavior, unchanged): a partial or
    // absent stream this cycle is recoverable the same way it always was.
    // It is only the *counts* above that must never claim zero on a fetch
    // failure.
    let dashboards = dashboards_res.unwrap_or_else(|e| {
        tracing::warn!(user_id, workspace_id, error = %e, "list_dashboards_for_sync failed");
        vec![]
    });
    let knowledge = knowledge_res.unwrap_or_else(|e| {
        tracing::warn!(user_id, workspace_id, error = %e, "list_knowledge_for_sync failed");
        vec![]
    });
    let sessions = sessions_res.unwrap_or_else(|e| {
        tracing::warn!(user_id, workspace_id, error = %e, "list_sessions_for_sync failed");
        vec![]
    });
    let watches = watches_res.unwrap_or_else(|e| {
        tracing::warn!(user_id, workspace_id, error = %e, "list_watches_for_sync failed");
        vec![]
    });

    // 2. Get the current sync watermark.
    let latest_sync_id =
        kyomi_auth::sync_log_service::get_latest_sync_id(db, workspace_id)
            .await
            .unwrap_or(0);

    // 3. Stream each entity as a SyncAction with action=Insert.
    stream_entities(manager, user_id, workspace_id, entity_types::DASHBOARD, "dashboard_id", dashboards).await;
    stream_entities(manager, user_id, workspace_id, entity_types::KNOWLEDGE, "dashboard_id", knowledge).await;
    stream_entities(manager, user_id, workspace_id, entity_types::CHAT_SESSION, "session_id", sessions).await;
    stream_entities(manager, user_id, workspace_id, entity_types::WATCH, "watch_id", watches).await;

    if let Some(item) = settings {
        stream_entities(manager, user_id, workspace_id, entity_types::WORKSPACE_SETTINGS, "workspace_id", vec![item]).await;
    }

    // 4. Signal completion with the current sync watermark and counts.
    send_sync_response(
        manager,
        user_id,
        SyncResponse::SyncComplete {
            last_sync_id: latest_sync_id,
            counts,
        },
    )
    .await;

    tracing::debug!(user_id, workspace_id, latest_sync_id, "sync_bootstrap complete");
}

/// Handle a `sync_delta` request.
///
/// Streams all sync log entries with `sync_id > last_sync_id`. If the
/// requested `sync_id` is no longer in the log (pruned), sends `SyncReset`
/// so the client falls back to a full bootstrap.
async fn handle_sync_delta(
    manager: &kyomi_auth::websocket::WebSocketManager,
    db: &kyomi_core::DbPool,
    user_id: &str,
    workspace_id: &str,
    last_sync_id: i64,
) {
    use kyomi_types::sync::SyncResponse;

    tracing::debug!(user_id, workspace_id, last_sync_id, "Handling sync_delta");

    // 1. Verify the requested sync_id is still available (not pruned).
    if last_sync_id > 0 {
        match kyomi_auth::sync_log_service::is_sync_id_available(db, workspace_id, last_sync_id)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                tracing::info!(
                    user_id,
                    workspace_id,
                    last_sync_id,
                    "sync_id pruned — sending SyncReset"
                );
                send_sync_response(manager, user_id, SyncResponse::SyncReset).await;
                return;
            }
            Err(e) => {
                tracing::error!(
                    user_id,
                    workspace_id,
                    last_sync_id,
                    error = %e,
                    "DB error checking sync_id availability — sending SyncReset"
                );
                send_sync_response(manager, user_id, SyncResponse::SyncReset).await;
                return;
            }
        }
    }

    // 2. Fetch all entries since last_sync_id (capped at 10 000 rows).
    let entries =
        kyomi_auth::sync_log_service::get_entries_since(db, workspace_id, last_sync_id, user_id, 10_000)
            .await
            .unwrap_or_default();

    // 3. Stream each entry as a SyncAction message.
    for entry in &entries {
        send_sync_response(manager, user_id, SyncResponse::SyncAction(entry.clone())).await;
    }

    // 4. Send SyncComplete with the latest sync_id we streamed and the
    // caller's current per-entity-type counts (KYO-480), so the client can
    // detect a local cache that has diverged from the server's authoritative
    // set — including entities whose only mutation predates `sync_log`
    // coverage, which a delta can structurally never carry.
    let latest_id = entries.last().map(|e| e.sync_id).unwrap_or(last_sync_id);
    let counts = compute_sync_counts(db, workspace_id, user_id).await;
    send_sync_response(
        manager,
        user_id,
        SyncResponse::SyncComplete {
            last_sync_id: latest_id,
            counts,
        },
    )
    .await;

    tracing::debug!(user_id, workspace_id, latest_id, "sync_delta complete");
}

/// Record `key`'s count in `counts`, but only when `value` is `Some` —
/// i.e. only when the underlying fetch actually succeeded (KYO-480 review
/// fix). `None` is silently omitted, never recorded as `0`.
///
/// This is the **single** place either counts-producing path is allowed to
/// write into the map: `handle_sync_bootstrap` and `compute_sync_counts`
/// both route every entity type through this function rather than each
/// containing its own "insert on success" logic, specifically so the
/// omit-on-failure contract cannot drift between the two paths the way it
/// did before this fix (`compute_sync_counts` omitted correctly;
/// `handle_sync_bootstrap` derived counts from vecs that had already been
/// `.unwrap_or_else`'d to `vec![]` on failure, making a fetch error
/// indistinguishable from a genuinely empty result).
///
/// Why a bootstrap-path `0` is worse than it looks: every repair the client
/// runs (`cache::sync_engine`, `crates/kyomi-ui`) goes through this same
/// untargeted `sync_bootstrap` — it always re-fetches and re-streams every
/// `entity_types::RECONCILED` type, not just the one that diverged. A
/// transient failure on the type actually being repaired makes it converge
/// on a false `0` that matches the freshly-wiped local store and is
/// accepted as correct (the entity is gone but believed repaired); the same
/// failure on any *other*, correctly-cached type fabricates a brand-new
/// false divergence that `RepairGuard` will admit and wipe, since it has
/// never seen that type diverge on this connection before.
fn insert_count_if_present(counts: &mut std::collections::HashMap<String, i64>, key: &str, value: Option<i64>) {
    if let Some(n) = value {
        counts.insert(key.to_string(), n);
    }
}

/// Compute the caller's current row count for every entity type in
/// `entity_types::RECONCILED`, scoped identically to that type's
/// `list_*_for_sync` query (KYO-480).
///
/// Cost: up to 4 indexed `COUNT(*)` queries, run once per `sync_delta`
/// request (i.e. once per reconnect that already has a cursor) — not per
/// message and not on every bootstrap (`handle_sync_bootstrap` derives its
/// counts from the vecs it already fetched, at zero extra query cost).
/// `dashboard`/`knowledge` reuse the existing, already-tested
/// `dashboard_service::get_document_count`; `chat_session` and `watch` are
/// new count queries added by this change — see
/// `chat_service::count_sessions_for_sync` and
/// `watch_service::count_watches_for_sync` for their index coverage.
///
/// A type whose count query fails is omitted from the returned map via
/// [`insert_count_if_present`] rather than defaulting to `0` — a transient
/// DB error must not read to the client as "you now have zero of these,
/// delete them all."
async fn compute_sync_counts(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    user_id: &str,
) -> std::collections::HashMap<String, i64> {
    use kyomi_core::models::DocType;
    use kyomi_types::sync::entity_types;

    let mut counts = std::collections::HashMap::with_capacity(entity_types::RECONCILED.len());

    let dashboard_res = kyomi_auth::dashboard_service::get_document_count(
        db,
        workspace_id,
        Some(DocType::Dashboard),
        user_id,
    )
    .await;
    if let Err(e) = &dashboard_res {
        tracing::warn!(user_id, workspace_id, error = %e, "sync count: dashboard count failed");
    }
    insert_count_if_present(&mut counts, entity_types::DASHBOARD, dashboard_res.ok());

    let knowledge_res = kyomi_auth::dashboard_service::get_document_count(
        db,
        workspace_id,
        Some(DocType::Knowledge),
        user_id,
    )
    .await;
    if let Err(e) = &knowledge_res {
        tracing::warn!(user_id, workspace_id, error = %e, "sync count: knowledge count failed");
    }
    insert_count_if_present(&mut counts, entity_types::KNOWLEDGE, knowledge_res.ok());

    let sessions_res = kyomi_auth::chat_service::count_sessions_for_sync(db, workspace_id, user_id).await;
    if let Err(e) = &sessions_res {
        tracing::warn!(user_id, workspace_id, error = %e, "sync count: chat session count failed");
    }
    insert_count_if_present(&mut counts, entity_types::CHAT_SESSION, sessions_res.ok());

    let watches_res = kyomi_auth::watch_service::count_watches_for_sync(db, workspace_id, user_id).await;
    if let Err(e) = &watches_res {
        tracing::warn!(user_id, workspace_id, error = %e, "sync count: watch count failed");
    }
    insert_count_if_present(&mut counts, entity_types::WATCH, watches_res.ok());

    counts
}

/// Stream a batch of entities as individual `SyncAction(Insert)` messages.
///
/// Used by `handle_sync_bootstrap` to avoid copy-pasting the same loop for
/// each entity type. `id_field` is the JSON key that holds the entity's
/// primary key (e.g. `"dashboard_id"`, `"session_id"`).
async fn stream_entities(
    manager: &kyomi_auth::websocket::WebSocketManager,
    user_id: &str,
    workspace_id: &str,
    entity_type: &str,
    id_field: &str,
    items: Vec<serde_json::Value>,
) {
    use kyomi_types::sync::{SyncAction, SyncActionType, SyncResponse};

    for item in items {
        let entity_id = item
            .get(id_field)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let timestamp = item
            .get("updated_at")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let action = SyncAction {
            sync_id: 0,
            entity_type: entity_type.to_string(),
            entity_id,
            workspace_id: workspace_id.to_string(),
            action: SyncActionType::Insert,
            data: Some(item),
            timestamp,
        };
        send_sync_response(manager, user_id, SyncResponse::SyncAction(action)).await;
    }
}

/// Send a `SyncResponse` to a specific user over WebSocket.
async fn send_sync_response(
    manager: &kyomi_auth::websocket::WebSocketManager,
    user_id: &str,
    response: kyomi_types::sync::SyncResponse,
) {
    use kyomi_types::websocket::{MessageType, WebSocketMessage};

    let (msg_type, data) = match &response {
        kyomi_types::sync::SyncResponse::SyncAction(action) => (
            MessageType::SyncAction,
            Some(serde_json::to_value(action).unwrap_or_default()),
        ),
        kyomi_types::sync::SyncResponse::SyncComplete { last_sync_id, counts } => (
            MessageType::SyncComplete,
            Some(serde_json::json!({ "last_sync_id": last_sync_id, "counts": counts })),
        ),
        kyomi_types::sync::SyncResponse::SyncReset => (MessageType::SyncReset, None),
    };

    let message = WebSocketMessage {
        message_type: msg_type,
        session_id: None,
        message_id: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
        data,
    };

    manager.send_to_user(user_id, message).await;
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

    // ── insert_count_if_present (KYO-480 review fix) ────────────────────────
    //
    // `handle_sync_bootstrap` and `compute_sync_counts` are both DB-backed
    // async fns with no lightweight unit-test seam, so these tests target
    // `insert_count_if_present` directly instead — the one function both of
    // those paths are required to route every count through. Proving the
    // omit-on-failure contract holds here proves it holds in both call
    // sites structurally: neither path can drift from it without bypassing
    // this function entirely, which is a visible, reviewable change (unlike
    // two independent copies of the same "insert on Ok, skip on Err" logic
    // silently diverging, which is exactly what happened before this fix).

    #[test]
    fn insert_count_if_present_records_a_present_value() {
        let mut counts = std::collections::HashMap::new();
        insert_count_if_present(&mut counts, "dashboard", Some(2));
        assert_eq!(counts.get("dashboard"), Some(&2));
    }

    /// The exact bug this fixes: a failed fetch (`None`) must never be
    /// recorded as a count of `0`. `handle_sync_bootstrap` previously
    /// derived counts from vecs that had already been degraded to `vec![]`
    /// on a fetch error, making "zero rows" and "the fetch failed" the same
    /// observable count — and every repair re-fetches every reconciled
    /// type via an untargeted bootstrap, so that false `0` could either
    /// mark a genuinely diverged type as falsely repaired, or fabricate a
    /// new false divergence in an unrelated, correctly-cached type.
    #[test]
    fn insert_count_if_present_omits_a_failed_fetch() {
        let mut counts = std::collections::HashMap::new();
        insert_count_if_present(&mut counts, "watch", None);
        assert!(
            !counts.contains_key("watch"),
            "a failed fetch must be omitted entirely, never recorded as a count of 0; got {counts:?}"
        );
    }

    /// A mixed batch — the realistic shape both `handle_sync_bootstrap` and
    /// `compute_sync_counts` produce when only some of the four reconciled
    /// types' fetches succeed this cycle.
    #[test]
    fn insert_count_if_present_handles_a_mixed_batch() {
        let mut counts = std::collections::HashMap::new();
        insert_count_if_present(&mut counts, "dashboard", Some(3));
        insert_count_if_present(&mut counts, "knowledge", None);
        insert_count_if_present(&mut counts, "chat_session", Some(0));
        insert_count_if_present(&mut counts, "watch", None);

        assert_eq!(
            counts,
            std::collections::HashMap::from([
                ("dashboard".to_string(), 3),
                ("chat_session".to_string(), 0),
            ]),
            "only the types whose fetch actually succeeded may appear — \
             note chat_session's genuine 0 (an empty-but-successful fetch) \
             is recorded, which is exactly what distinguishes it from \
             knowledge/watch's omitted failures"
        );
    }

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
    fn extract_workspace_id_plain() {
        assert_eq!(extract_workspace_id_from_path("user-abc123"), "");
    }

    #[test]
    fn extract_workspace_id_with_ws_prefix() {
        assert_eq!(
            extract_workspace_id_from_path("ws-550e8400-e29b-41d4-a716-446655440000_user-abc123"),
            "ws-550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn extract_workspace_id_with_workspace_prefix() {
        assert_eq!(
            extract_workspace_id_from_path("workspace-99f24d05-673d25b8_user-PHjsNsAj8hqZXOGGM-em1Q"),
            "workspace-99f24d05-673d25b8"
        );
    }

    #[test]
    fn extract_workspace_id_e2e_format() {
        assert_eq!(
            extract_workspace_id_from_path("e2e-test-workspace-0001_usr_a0bda4c2e7af4be3a29d"),
            "e2e-test-workspace-0001"
        );
    }
}
