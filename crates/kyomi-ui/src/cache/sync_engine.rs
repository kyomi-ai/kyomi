// SPDX-License-Identifier: AGPL-3.0-or-later

//! Client-side sync engine for the KYO-169 offline-first sync protocol.
//!
//! The sync engine manages three sync phases over the shared WebSocket:
//!
//! 1. **Bootstrap** (`sync_bootstrap`): sent on first connect when no local
//!    cursor exists — the server sends the full workspace dataset as a stream
//!    of `sync_action` messages followed by `sync_complete`.
//!
//! 2. **Delta** (`sync_delta`): sent on reconnect when a cursor exists — the
//!    server sends only actions that occurred after `last_sync_id`.
//!
//! 3. **Reset** (`sync_reset`): the server signals that local state is
//!    irrecoverably stale (e.g. cursor too old). The engine nukes IndexedDB,
//!    resets the reactive store, and re-bootstraps.
//!
//! ## Reconnect handling
//!
//! The engine watches the `WebSocketContext::connection_state` signal and
//! re-sends the appropriate request on every transition to `Connected`.
//! Previous subscriptions survive the reconnect (the WS provider keeps the
//! subscriber map across reconnects), so there is no need to re-subscribe.
//!
//! ## Thread safety / `!Send` types
//!
//! This module is `wasm32`-only. All async tasks use `spawn_local` (single-
//! threaded WASM event loop). Cleanup closures that capture `Box<dyn FnOnce()>`
//! (the unsubscribe functions returned by `ws.subscribe`) are wrapped in
//! `SendWrapper` so they satisfy `on_cleanup`'s `Send + 'static` bound.

use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;

use crate::cache::store::SyncStore;
use crate::components::chat::websocket_client::{ConnectionState, WebSocketContext};
use kyomi_types::sync::entity_types;

// ── Public entry point ────────────────────────────────────────────────────────

/// Start the sync engine. Call **once** from the Layout after the WebSocket
/// connects (i.e. from inside a `<WebSocketProvider>` subtree).
///
/// Subscribes to `sync_action`, `sync_complete`, and `sync_reset` messages.
/// Sends an immediate bootstrap or delta request based on the stored cursor.
///
/// On every reconnection (transition to `Connected`) the engine re-sends the
/// appropriate request so the client catches up with any events it missed
/// while offline.
pub fn start_sync_engine(
    ws: WebSocketContext,
    store: SyncStore,
    workspace_id: String,
) {
    // ── Subscribe to sync_action ──────────────────────────────────────────────
    let unsub_action = ws.subscribe("sync_action", {
        let store = store;
        let workspace_id = workspace_id.clone();
        move |msg| {
            if let Some(data) = &msg.data {
                apply_sync_action(&store, &workspace_id, data);
            }
        }
    });

    // ── Subscribe to sync_complete ────────────────────────────────────────────
    let unsub_complete = ws.subscribe("sync_complete", {
        let store = store;
        let workspace_id = workspace_id.clone();
        move |msg| {
            if let Some(data) = &msg.data {
                if let Some(sync_id) = data.get("last_sync_id").and_then(|v| v.as_i64()) {
                    let wid = workspace_id.clone();
                    spawn_local(async move {
                        match crate::cache::db::init_cache_db(&wid).await {
                            Ok(db) => {
                                if let Err(e) =
                                    crate::cache::db::set_last_sync_id(&db, &wid, &sync_id.to_string())
                                        .await
                                {
                                    tracing::warn!("sync_complete: failed to persist cursor: {e}");
                                }
                            }
                            Err(e) => {
                                tracing::warn!("sync_complete: failed to open cache db: {e}");
                            }
                        }
                        store.mark_initialized();
                        tracing::info!(last_sync_id = sync_id, "sync_complete: cursor persisted, store initialized");
                    });
                }
            }
        }
    });

    // ── Subscribe to sync_reset ───────────────────────────────────────────────
    let unsub_reset = ws.subscribe("sync_reset", {
        let ws = ws.clone();
        let store = store;
        let workspace_id = workspace_id.clone();
        move |_msg| {
            tracing::info!("sync_reset: nuking local cache and re-bootstrapping");
            store.reset();
            let wid = workspace_id.clone();
            let ws_clone = ws.clone();
            spawn_local(async move {
                match crate::cache::db::init_cache_db(&wid).await {
                    Ok(db) => {
                        for et in [
                            entity_types::DASHBOARD,
                            entity_types::KNOWLEDGE,
                            entity_types::CHAT_SESSION,
                            entity_types::WATCH,
                            entity_types::WORKSPACE_SETTINGS,
                        ] {
                            if let Err(e) =
                                crate::cache::db::delete_all_of_type(&db, et, &wid).await
                            {
                                tracing::warn!(entity_type = et, "sync_reset: delete_all_of_type failed: {e}");
                            }
                        }
                        if let Err(e) =
                            crate::cache::db::set_last_sync_id(&db, &wid, "0").await
                        {
                            tracing::warn!("sync_reset: failed to reset cursor: {e}");
                        }
                    }
                    Err(e) => {
                        tracing::warn!("sync_reset: failed to open cache db: {e}");
                    }
                }
                // Request a fresh bootstrap after the cache is cleared.
                ws_clone.send(serde_json::json!({"type": "sync_bootstrap"}));
            });
        }
    });

    // ── Register cleanup ──────────────────────────────────────────────────────
    // Unsubscribe when the component that called start_sync_engine is dropped.
    // The unsubscribe closures are `Box<dyn FnOnce()>` which is !Send, so wrap
    // them in SendWrapper to satisfy on_cleanup's `Send + 'static` requirement.
    let unsub_action = SendWrapper::new(unsub_action);
    let unsub_complete = SendWrapper::new(unsub_complete);
    let unsub_reset = SendWrapper::new(unsub_reset);
    on_cleanup(move || {
        unsub_action.take()();
        unsub_complete.take()();
        unsub_reset.take()();
    });

    // ── Watch connection state to (re-)send bootstrap/delta on connect ────────
    // This Effect tracks `connection_state`. Every time the socket transitions
    // to `Connected` (initial connect or reconnect) it sends the appropriate
    // sync request.
    let ws_for_state = ws.clone();
    let wid_for_state = workspace_id.clone();
    Effect::new(move |_| {
        let state = ws_for_state.connection_state.get();
        if state != ConnectionState::Connected {
            return;
        }
        let ws_send = ws_for_state.clone();
        let wid = wid_for_state.clone();
        spawn_local(async move {
            request_sync(&ws_send, &wid).await;
        });
    });
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Determine which sync request to send based on the stored cursor, then send it.
async fn request_sync(ws: &WebSocketContext, workspace_id: &str) {
    let last_sync_id = match crate::cache::db::init_cache_db(workspace_id).await {
        Ok(db) => crate::cache::db::get_last_sync_id(&db, workspace_id)
            .await
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0),
        Err(e) => {
            tracing::warn!("request_sync: failed to open cache db: {e}");
            0
        }
    };

    if last_sync_id == 0 {
        tracing::info!("sync: no cursor — sending sync_bootstrap");
        ws.send(serde_json::json!({"type": "sync_bootstrap"}));
    } else {
        tracing::info!(last_sync_id, "sync: cursor found — sending sync_delta");
        ws.send(serde_json::json!({
            "type": "sync_delta",
            "last_sync_id": last_sync_id
        }));
    }
}

/// Apply a single `sync_action` message to the reactive store and IndexedDB.
///
/// Actions arrive as raw `serde_json::Value`s (the `data` field of the WS
/// message). This function extracts the action type, entity type, and entity
/// data, then dispatches to the appropriate store mutator and IDB writer.
fn apply_sync_action(
    store: &SyncStore,
    workspace_id: &str,
    data: &serde_json::Value,
) {
    use crate::server_fns::chat::ChatSessionItem;
    use crate::server_fns::dashboards::DashboardListItem;
    use crate::types::WatchListItem;

    let action_str = data.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let entity_type = data.get("entity_type").and_then(|v| v.as_str()).unwrap_or("");
    let entity_id = data
        .get("entity_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let timestamp = data
        .get("timestamp")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let entity_data = data.get("data");

    match action_str {
        "insert" | "update" => {
            let Some(entity_data) = entity_data else {
                tracing::warn!(
                    action = action_str,
                    entity_type,
                    entity_id = %entity_id,
                    "sync_action insert/update: missing data field — skipping"
                );
                return;
            };

            // Persist to IndexedDB (best-effort async write).
            let json_str = match serde_json::to_string(entity_data) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(entity_type, entity_id = %entity_id, "sync_action: failed to re-serialize entity data: {e}");
                    return;
                }
            };
            let et = entity_type.to_string();
            let eid = entity_id.clone();
            let wid = workspace_id.to_string();
            let ts = timestamp.clone();
            spawn_local(async move {
                match crate::cache::db::init_cache_db(&wid).await {
                    Ok(db) => {
                        if let Err(e) =
                            crate::cache::db::upsert(&db, &et, &eid, &wid, &json_str, &ts).await
                        {
                            tracing::warn!(
                                entity_type = %et,
                                entity_id = %eid,
                                "sync_action upsert to IDB failed: {e}"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!("sync_action: failed to open cache db: {e}");
                    }
                }
            });

            // Update the reactive store.
            match entity_type {
                et if et == entity_types::DASHBOARD => {
                    match serde_json::from_value::<DashboardListItem>(entity_data.clone()) {
                        Ok(item) => store.upsert_dashboard(item),
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id = %entity_id,
                            "sync_action: failed to deserialize dashboard: {e}"
                        ),
                    }
                }
                et if et == entity_types::KNOWLEDGE => {
                    match serde_json::from_value::<DashboardListItem>(entity_data.clone()) {
                        Ok(item) => store.upsert_knowledge_doc(item),
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id = %entity_id,
                            "sync_action: failed to deserialize knowledge doc: {e}"
                        ),
                    }
                }
                et if et == entity_types::CHAT_SESSION => {
                    match serde_json::from_value::<ChatSessionItem>(entity_data.clone()) {
                        Ok(item) => store.upsert_chat_session(item),
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id = %entity_id,
                            "sync_action: failed to deserialize chat session: {e}"
                        ),
                    }
                }
                et if et == entity_types::WATCH => {
                    match serde_json::from_value::<WatchListItem>(entity_data.clone()) {
                        Ok(item) => store.upsert_watch(item),
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id = %entity_id,
                            "sync_action: failed to deserialize watch: {e}"
                        ),
                    }
                }
                other => {
                    tracing::debug!(entity_type = other, "sync_action: unhandled entity type — ignoring");
                }
            }
        }
        "delete" => {
            // Remove from IndexedDB (best-effort async).
            let et = entity_type.to_string();
            let eid = entity_id.clone();
            let wid = workspace_id.to_string();
            spawn_local(async move {
                match crate::cache::db::init_cache_db(&wid).await {
                    Ok(db) => {
                        if let Err(e) =
                            crate::cache::db::delete(&db, &et, &eid, &wid).await
                        {
                            tracing::warn!(
                                entity_type = %et,
                                entity_id = %eid,
                                "sync_action delete from IDB failed: {e}"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!("sync_action delete: failed to open cache db: {e}");
                    }
                }
            });

            // Remove from the reactive store.
            match entity_type {
                et if et == entity_types::DASHBOARD => store.remove_dashboard(&entity_id),
                et if et == entity_types::KNOWLEDGE => store.remove_knowledge_doc(&entity_id),
                et if et == entity_types::CHAT_SESSION => store.remove_chat_session(&entity_id),
                et if et == entity_types::WATCH => store.remove_watch(&entity_id),
                other => {
                    tracing::debug!(entity_type = other, "sync_action delete: unhandled entity type — ignoring");
                }
            }
        }
        other => {
            tracing::debug!(action = other, "sync_action: unhandled action type — ignoring");
        }
    }
}
