// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scoped sync-action application, shared by the browser and host regression tests.

use crate::cache::store::SyncStore;
use kyomi_types::sync::entity_types;

/// Browser writes are scheduled asynchronously; tests record the same call boundary.
pub(crate) trait SyncPersistence {
    fn upsert(
        &self,
        entity_type: &str,
        entity_id: &str,
        workspace_id: &str,
        json: &str,
        timestamp: &str,
    );
    fn delete(&self, entity_type: &str, entity_id: &str, workspace_id: &str);
}

/// Extract `WorkspaceSettingsData` from the raw sync entity JSON.
///
/// Missing or null fields fall back to defaults — always returns a populated struct.
/// Mirrors the extraction logic in `server_fns::workspace::get_workspace_settings`.
pub(crate) fn parse_workspace_settings(
    data: &serde_json::Value,
) -> crate::types::WorkspaceSettingsData {
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

    let title_model = custom
        .and_then(|cs| cs.get("title_model"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    crate::types::WorkspaceSettingsData {
        workspace_name,
        default_model,
        chart_palette,
        title_model,
    }
}

/// Apply a single `sync_action` message to the reactive store and IndexedDB.
///
/// The full envelope is required: its origin and the payload origin must both
/// match the tab before dispatching any store mutation or persistence call.
pub(crate) fn apply_sync_action(
    store: &SyncStore,
    workspace_id: &str,
    message: &kyomi_types::websocket::WebSocketMessage,
    persistence: &impl SyncPersistence,
) {
    use crate::server_fns::chat::ChatSessionItem;
    use crate::server_fns::dashboards::DashboardListItem;
    use crate::types::WatchListItem;

    // KYO-861 rollout: fail closed. Servers must ship KYO-856's envelope
    // before this client; legacy unscoped actions are never inferred from entities.
    let Some(origin) = message.workspace_id.as_deref() else {
        return;
    };
    let Some(data) = message.data.as_ref() else {
        return;
    };
    let Some(payload_origin) = data.get("workspace_id").and_then(serde_json::Value::as_str) else {
        return;
    };
    if origin.trim().is_empty() || origin != workspace_id || payload_origin != origin {
        return;
    }

    let action_str = data.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let entity_type = data
        .get("entity_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
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
            persistence.upsert(&et, &eid, &wid, &json_str, &ts);

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
                        persistence.delete(entity_types::DASHBOARD_DETAIL, &eid_inv, &wid_inv);
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
                        persistence.delete(entity_types::DASHBOARD_DETAIL, &eid_inv, &wid_inv);
                    }
                }
                et if et == entity_types::CHAT_SESSION => {
                    match serde_json::from_value::<ChatSessionItem>(entity_data.clone()) {
                        Ok(item) => {
                            if item.session_type.as_deref() != Some("chat")
                                && item.session_type.is_some()
                            {
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
                        persistence.delete(entity_types::CHAT_MESSAGES, &eid_inv, &wid_inv);
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
                    tracing::debug!(
                        entity_type = other,
                        "sync_action: unhandled entity type — ignoring"
                    );
                }
            }
        }
        "delete" => {
            // Remove from IndexedDB (best-effort async).
            let et = entity_type.to_string();
            let eid = entity_id.clone();
            let wid = workspace_id.to_string();
            persistence.delete(&et, &eid, &wid);

            // Tier 2 invalidation on delete (KYO-215): when the Tier 1 entity is
            // deleted, also remove any Tier 2 detail cache entries so stale content
            // doesn't survive a later recreate-with-the-same-id scenario.
            match entity_type {
                et if et == entity_types::DASHBOARD || et == entity_types::KNOWLEDGE => {
                    let wid_inv = workspace_id.to_string();
                    let eid_inv = entity_id.clone();
                    persistence.delete(entity_types::DASHBOARD_DETAIL, &eid_inv, &wid_inv);
                }
                et if et == entity_types::CHAT_SESSION => {
                    let wid_inv = workspace_id.to_string();
                    let eid_inv = entity_id.clone();
                    persistence.delete(entity_types::CHAT_MESSAGES, &eid_inv, &wid_inv);
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
                    tracing::debug!(
                        entity_type = other,
                        "sync_action delete: unhandled entity type — ignoring"
                    );
                }
            }
        }
        other => {
            tracing::debug!(
                action = other,
                "sync_action: unhandled action type — ignoring"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leptos::prelude::*;
    use serde_json::{Value, json};
    use std::cell::RefCell;

    #[derive(Debug, PartialEq)]
    enum Write {
        Upsert(String, String, String, Value, String),
        Delete(String, String, String),
    }

    #[derive(Default)]
    struct RecordingPersistence(RefCell<Vec<Write>>);
    impl SyncPersistence for RecordingPersistence {
        fn upsert(&self, et: &str, eid: &str, wid: &str, json: &str, ts: &str) {
            self.0.borrow_mut().push(Write::Upsert(
                et.into(),
                eid.into(),
                wid.into(),
                serde_json::from_str(json).unwrap(),
                ts.into(),
            ));
        }
        fn delete(&self, et: &str, eid: &str, wid: &str) {
            self.0
                .borrow_mut()
                .push(Write::Delete(et.into(), eid.into(), wid.into()));
        }
    }

    fn message(
        action: &str,
        entity_type: &str,
        title: &str,
    ) -> kyomi_types::websocket::WebSocketMessage {
        serde_json::from_value(json!({
            "type": "sync_action", "workspace_id": "workspace-a",
            "data": {"workspace_id": "workspace-a", "action": action,
                "entity_type": entity_type, "entity_id": "entity-1", "timestamp": "2026-01-01T00:00:00Z",
                "data": {"dashboard_id": "entity-1", "title": title,
                    "workspace_id": "workspace-a", "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"}
            }
        })).unwrap()
    }

    fn snapshot(store: &SyncStore) -> Value {
        json!({"dashboards": store.dashboards().get_untracked(),
            "knowledge": store.knowledge_docs().get_untracked(),
            "chats": store.chat_sessions().get_untracked(),
            "watches": store.watches().get_untracked(),
            "settings": store.workspace_settings().get_untracked(),
            "initialized": store.initialized().get_untracked()})
    }

    #[test]
    fn rejected_scope_has_no_store_or_persistence_effects() {
        let owner = Owner::new();
        owner.with(|| {
            let store = SyncStore::new();
            let persistence = RecordingPersistence::default();
            apply_sync_action(
                &store,
                "workspace-a",
                &message("insert", "dashboard", "Original"),
                &persistence,
            );
            let before = snapshot(&store);
            persistence.0.borrow_mut().clear();
            for action in ["insert", "update", "delete"] {
                let valid = message(action, "dashboard", "Contamination");
                let mut cases = Vec::new();
                for origin in [None, Some(""), Some(" "), Some("workspace-b")] {
                    let mut msg = valid.clone();
                    msg.workspace_id = origin.map(str::to_string);
                    cases.push(msg);
                }
                for origin in [
                    Value::Null,
                    json!(12),
                    json!([]),
                    json!({}),
                    json!(""),
                    json!(" "),
                    json!("workspace-b"),
                ] {
                    let mut msg = valid.clone();
                    msg.data.as_mut().unwrap()["workspace_id"] = origin;
                    cases.push(msg);
                }
                let mut missing = valid.clone();
                missing
                    .data
                    .as_mut()
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove("workspace_id");
                cases.push(missing);
                for data in [None, Some(Value::Null), Some(json!([]))] {
                    let mut msg = valid.clone();
                    msg.data = data;
                    cases.push(msg);
                }
                let mut foreign = valid.clone();
                foreign.workspace_id = Some("workspace-b".into());
                foreign.data.as_mut().unwrap()["workspace_id"] = json!("workspace-b");
                cases.push(foreign);
                for msg in cases {
                    apply_sync_action(&store, "workspace-a", &msg, &persistence);
                    assert_eq!(snapshot(&store), before, "rejected {msg:?}");
                    assert!(
                        persistence.0.borrow().is_empty(),
                        "rejected {msg:?} scheduled persistence"
                    );
                }
            }
        });
    }

    #[test]
    fn matching_origin_insert_update_delete_apply_and_invalidate() {
        let owner = Owner::new();
        owner.with(|| {
            let store = SyncStore::new();
            let persistence = RecordingPersistence::default();
            for entity_type in [entity_types::DASHBOARD, entity_types::KNOWLEDGE] {
                for (action, title) in [("insert", "First"), ("update", "Renamed")] {
                    let msg = message(action, entity_type, title);
                    apply_sync_action(&store, "workspace-a", &msg, &persistence);
                    let list = if entity_type == entity_types::DASHBOARD {
                        store.dashboards().get_untracked()
                    } else {
                        store.knowledge_docs().get_untracked()
                    };
                    assert_eq!(list.len(), 1);
                    assert_eq!(list[0].title, title);
                    assert_eq!(
                        *persistence.0.borrow(),
                        vec![
                            Write::Upsert(
                                entity_type.into(),
                                "entity-1".into(),
                                "workspace-a".into(),
                                msg.data.as_ref().unwrap()["data"].clone(),
                                "2026-01-01T00:00:00Z".into()
                            ),
                            Write::Delete(
                                entity_types::DASHBOARD_DETAIL.into(),
                                "entity-1".into(),
                                "workspace-a".into()
                            )
                        ]
                    );
                    persistence.0.borrow_mut().clear();
                }
                apply_sync_action(
                    &store,
                    "workspace-a",
                    &message("delete", entity_type, ""),
                    &persistence,
                );
                assert_eq!(store.reconciliation_counts()[entity_type], 0);
                assert_eq!(
                    *persistence.0.borrow(),
                    vec![
                        Write::Delete(entity_type.into(), "entity-1".into(), "workspace-a".into()),
                        Write::Delete(
                            entity_types::DASHBOARD_DETAIL.into(),
                            "entity-1".into(),
                            "workspace-a".into()
                        )
                    ]
                );
                persistence.0.borrow_mut().clear();
            }
        });
    }

    #[test]
    fn matching_chat_scope_preserves_session_type_filter() {
        let owner = Owner::new();
        owner.with(|| {
            let store = SyncStore::new();
            let persistence = RecordingPersistence::default();
            for session_type in [Some("watch"), Some("copilot"), Some("chat"), None] {
                let mut msg = message("insert", entity_types::CHAT_SESSION, "Session");
                msg.data.as_mut().unwrap()["data"] = json!({"session_id":"entity-1", "title":"Session", "session_type": session_type, "created_at":"2026-01-01T00:00:00Z", "updated_at":"2026-01-01T00:00:00Z"});
                store.set_chat_sessions(vec![]);
                apply_sync_action(&store, "workspace-a", &msg, &persistence);
                assert_eq!(store.chat_sessions().get_untracked().len(), usize::from(session_type == Some("chat") || session_type.is_none()));
                assert_eq!(persistence.0.borrow().len(), 2);
                persistence.0.borrow_mut().clear();
            }
        });
    }
}
