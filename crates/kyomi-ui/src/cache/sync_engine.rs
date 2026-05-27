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
        let workspace_id = workspace_id.clone();
        move |msg| {
            if let Some(data) = &msg.data {
                apply_sync_action(&store, &workspace_id, data);
            }
        }
    });

    // ── Subscribe to sync_complete ────────────────────────────────────────────
    let unsub_complete = ws.subscribe("sync_complete", {
        let workspace_id = workspace_id.clone();
        move |msg| {
            if let Some(data) = &msg.data
                && let Some(sync_id) = data.get("last_sync_id").and_then(|v| v.as_i64())
            {
                    // Persist cursor + schema hash to IDB.
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
                                let _ = crate::cache::db::set_meta(
                                    &db, "schemaHash", crate::cache::db::SCHEMA_HASH
                                ).await;
                            }
                            Err(e) => {
                                tracing::warn!("sync_complete: failed to open cache db: {e}");
                            }
                        }
                        store.mark_initialized();
                        tracing::info!(last_sync_id = sync_id, "sync_complete: store initialized");
                    });
            }
        }
    });

    // ── Subscribe to sync_reset ───────────────────────────────────────────────
    let unsub_reset = ws.subscribe("sync_reset", {
        let ws = ws.clone();
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
                            // Tier 2 detail caches (KYO-215)
                            entity_types::DASHBOARD_DETAIL,
                            entity_types::CHAT_MESSAGES,
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

    // ── Watch connection state to send bootstrap or delta on connect ────────
    //
    // On each connect/reconnect, read the IDB cursor to decide:
    //   cursor == 0 → full bootstrap (first visit or after schema wipe)
    //   cursor > 0  → delta catch-up (return visit or reconnect)
    //
    // Always reads IDB rather than trusting an in-memory cursor — a schema
    // hash wipe clears IDB but cannot reach in-memory state, so the
    // in-memory value can be stale after a deploy.
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
            let idb_cursor = match crate::cache::db::init_cache_db(&wid).await {
                Ok(db) => crate::cache::db::get_last_sync_id(&db, &wid)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(0),
                Err(_) => 0,
            };

            if idb_cursor == 0 {
                tracing::info!("sync: no cursor — sending sync_bootstrap");
                ws_send.send(serde_json::json!({"type": "sync_bootstrap"}));
            } else {
                tracing::info!(idb_cursor, "sync: IDB cursor found — sending sync_delta");
                ws_send.send(serde_json::json!({
                    "type": "sync_delta",
                    "last_sync_id": idb_cursor
                }));
            }
        });
    });
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Extract `WorkspaceSettingsData` from the raw sync entity JSON.
///
/// Missing or null fields fall back to defaults — always returns a populated struct.
/// Mirrors the extraction logic in `server_fns::workspace::get_workspace_settings`.
fn parse_workspace_settings(data: &serde_json::Value) -> crate::types::WorkspaceSettingsData {
    let settings = data.get("settings");
    let custom = settings.and_then(|s| s.get("custom_settings"));

    let workspace_name = data
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let default_model = custom
        .and_then(|cs| cs.get("default_model"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let chart_palette = custom
        .and_then(|cs| cs.get("chartml_config"))
        .and_then(|cfg| cfg.get("style"))
        .and_then(|v| v.as_str())
        .unwrap_or("kyomi")
        .to_string();

    let show_token_usage = custom
        .and_then(|cs| cs.get("show_token_usage"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let title_model = custom
        .and_then(|cs| cs.get("title_model"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    crate::types::WorkspaceSettingsData {
        workspace_name,
        default_model,
        chart_palette,
        show_token_usage,
        title_model,
    }
}

/// Read all entity types from IndexedDB and populate the store.
pub async fn hydrate_store_from_db(
    db: &crate::cache::db::CacheDb,
    workspace_id: &str,
    store: &SyncStore,
) {
    use crate::server_fns::chat::ChatSessionItem;
    use crate::server_fns::dashboards::DashboardListItem;
    use crate::types::WatchListItem;
    use kyomi_types::sync::entity_types;

    fn deser<T: serde::de::DeserializeOwned>(
        entries: &[(String, String, String)],
        entity_type: &str,
    ) -> Vec<T> {
        let mut items = Vec::with_capacity(entries.len());
        for (id, json, _ts) in entries {
            match serde_json::from_str(json) {
                Ok(item) => items.push(item),
                Err(e) => tracing::warn!(
                    entity_type,
                    entity_id = %id,
                    "hydration deser failed: {e}"
                ),
            }
        }
        items
    }

    if let Ok(entries) = crate::cache::db::read_all(db, entity_types::DASHBOARD, workspace_id).await {
        store.set_dashboards(deser::<DashboardListItem>(&entries, entity_types::DASHBOARD));
    }
    if let Ok(entries) = crate::cache::db::read_all(db, entity_types::KNOWLEDGE, workspace_id).await {
        store.set_knowledge_docs(deser::<DashboardListItem>(&entries, entity_types::KNOWLEDGE));
    }
    if let Ok(entries) = crate::cache::db::read_all(db, entity_types::CHAT_SESSION, workspace_id).await {
        store.set_chat_sessions(deser::<ChatSessionItem>(&entries, entity_types::CHAT_SESSION));
    }
    if let Ok(entries) = crate::cache::db::read_all(db, entity_types::WATCH, workspace_id).await {
        store.set_watches(deser::<WatchListItem>(&entries, entity_types::WATCH));
    }
    if let Ok(entries) = crate::cache::db::read_all(db, entity_types::WORKSPACE_SETTINGS, workspace_id).await
        && let Some((_id, json, _ts)) = entries.first()
    {
        match serde_json::from_str::<serde_json::Value>(json) {
            Ok(v) => store.set_workspace_settings(Some(parse_workspace_settings(&v))),
            Err(e) => tracing::warn!(
                entity_type = entity_types::WORKSPACE_SETTINGS,
                "hydration: failed to parse workspace settings JSON: {e}"
            ),
        }
    }

    if let Ok(Some(_cursor)) = crate::cache::db::get_last_sync_id(db, workspace_id).await {
        store.mark_initialized();
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
                    // Tier 2 invalidation (KYO-215): any mutation to the dashboard
                    // list entry means the cached detail may be stale.  Evict it so
                    // the next visit re-fetches from the server.
                    {
                        let wid_inv = workspace_id.to_string();
                        let eid_inv = entity_id.clone();
                        spawn_local(async move {
                            if let Ok(db) = crate::cache::db::init_cache_db(&wid_inv).await
                                && let Err(e) = crate::cache::db::delete(
                                    &db,
                                    entity_types::DASHBOARD_DETAIL,
                                    &eid_inv,
                                    &wid_inv,
                                )
                                .await
                                {
                                    tracing::warn!(
                                        entity_id = %eid_inv,
                                        "sync_action: dashboard_detail cache invalidation failed: {e}"
                                    );
                                }
                        });
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
                    // Tier 2 invalidation (KYO-215): knowledge docs share the
                    // DASHBOARD_DETAIL cache store (same viewer page).
                    {
                        let wid_inv = workspace_id.to_string();
                        let eid_inv = entity_id.clone();
                        spawn_local(async move {
                            if let Ok(db) = crate::cache::db::init_cache_db(&wid_inv).await
                                && let Err(e) = crate::cache::db::delete(
                                    &db,
                                    entity_types::DASHBOARD_DETAIL,
                                    &eid_inv,
                                    &wid_inv,
                                )
                                .await
                                {
                                    tracing::warn!(
                                        entity_id = %eid_inv,
                                        "sync_action: dashboard_detail cache invalidation (knowledge) failed: {e}"
                                    );
                                }
                        });
                    }
                }
                et if et == entity_types::CHAT_SESSION => {
                    match serde_json::from_value::<ChatSessionItem>(entity_data.clone()) {
                        Ok(item) => {
                            if item.session_type.as_deref() != Some("chat") && item.session_type.is_some() {
                                tracing::debug!(
                                    entity_id = %entity_id,
                                    session_type = ?item.session_type,
                                    "sync_action: skipping non-chat session"
                                );
                            } else {
                                store.upsert_chat_session(item);
                            }
                        }
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id = %entity_id,
                            "sync_action: failed to deserialize chat session: {e}"
                        ),
                    }
                    // Tier 2 invalidation (KYO-215): a session metadata change
                    // (rename, share/unshare, etc.) means the cached messages may
                    // be out of date.  Evict so next visit is fresh.
                    {
                        let wid_inv = workspace_id.to_string();
                        let eid_inv = entity_id.clone();
                        spawn_local(async move {
                            if let Ok(db) = crate::cache::db::init_cache_db(&wid_inv).await
                                && let Err(e) = crate::cache::db::delete(
                                    &db,
                                    entity_types::CHAT_MESSAGES,
                                    &eid_inv,
                                    &wid_inv,
                                )
                                .await
                                {
                                    tracing::warn!(
                                        entity_id = %eid_inv,
                                        "sync_action: chat_messages cache invalidation failed: {e}"
                                    );
                                }
                        });
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
                et if et == entity_types::WORKSPACE_SETTINGS => {
                    store.upsert_workspace_settings(parse_workspace_settings(entity_data));
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

            // Tier 2 invalidation on delete (KYO-215): when the Tier 1 entity is
            // deleted, also remove any Tier 2 detail cache entries so stale content
            // doesn't survive a later recreate-with-the-same-id scenario.
            match entity_type {
                et if et == entity_types::DASHBOARD || et == entity_types::KNOWLEDGE => {
                    let wid_inv = workspace_id.to_string();
                    let eid_inv = entity_id.clone();
                    spawn_local(async move {
                        if let Ok(db) = crate::cache::db::init_cache_db(&wid_inv).await {
                            let _ = crate::cache::db::delete(
                                &db,
                                entity_types::DASHBOARD_DETAIL,
                                &eid_inv,
                                &wid_inv,
                            )
                            .await;
                        }
                    });
                }
                et if et == entity_types::CHAT_SESSION => {
                    let wid_inv = workspace_id.to_string();
                    let eid_inv = entity_id.clone();
                    spawn_local(async move {
                        if let Ok(db) = crate::cache::db::init_cache_db(&wid_inv).await {
                            let _ = crate::cache::db::delete(
                                &db,
                                entity_types::CHAT_MESSAGES,
                                &eid_inv,
                                &wid_inv,
                            )
                            .await;
                        }
                    });
                }
                _ => {}
            }

            // Remove from the reactive store.
            match entity_type {
                et if et == entity_types::DASHBOARD => store.remove_dashboard(&entity_id),
                et if et == entity_types::KNOWLEDGE => store.remove_knowledge_doc(&entity_id),
                et if et == entity_types::CHAT_SESSION => store.remove_chat_session(&entity_id),
                et if et == entity_types::WATCH => store.remove_watch(&entity_id),
                et if et == entity_types::WORKSPACE_SETTINGS => store.remove_workspace_settings(),
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
