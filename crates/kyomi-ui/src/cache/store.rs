// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reactive in-memory store for synced metadata (KYO-169).
//!
//! `SyncStore` is the single source of truth for list pages (dashboards,
//! knowledge, chat sessions, watches). It is populated from IndexedDB on
//! startup (hydration) and kept current by the sync engine as delta events
//! arrive over WebSocket.
//!
//! The store itself is **not** cfg-gated to `wasm32`. Page components that
//! read from it compile on both SSR and CSR targets; they receive empty vectors
//! on SSR and wait for `initialized()` to become `true` on the client. Only the
//! IndexedDB hydration call-site is wasm32-only.

use leptos::prelude::*;
use send_wrapper::SendWrapper;

use crate::server_fns::dashboards::DashboardListItem;
use crate::server_fns::chat::ChatSessionItem;
use crate::types::{WatchListItem, WorkspaceSettingsData};

// ── Inner storage ─────────────────────────────────────────────────────────────

/// Non-Clone, non-Send inner storage for the store's reactive signals.
///
/// Wrapped in `SendWrapper` so it can be placed in a `StoredValue`, which
/// requires `Send + Sync` even though this crate only ever runs on WASM
/// (single-threaded). This matches the pattern used by `QueryCache`.
struct SyncStoreInner {
    dashboards: ArcRwSignal<Vec<DashboardListItem>>,
    chat_sessions: ArcRwSignal<Vec<ChatSessionItem>>,
    knowledge_docs: ArcRwSignal<Vec<DashboardListItem>>,
    watches: ArcRwSignal<Vec<WatchListItem>>,
    workspace_settings: ArcRwSignal<Option<WorkspaceSettingsData>>,
    initialized: ArcRwSignal<bool>,
}

// ── Public handle ─────────────────────────────────────────────────────────────

/// Reactive in-memory store for synced metadata.
///
/// Cheaply `Copy`able — the actual data lives behind a `StoredValue`.
/// Provide at the `Layout` level with [`provide_context`] and access on any
/// child page with `expect_context::<SyncStore>()`.
#[derive(Clone, Copy)]
pub struct SyncStore {
    inner: StoredValue<SendWrapper<SyncStoreInner>>,
}

impl Default for SyncStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncStore {
    /// Create a new, empty `SyncStore`.
    ///
    /// All lists start empty and `initialized` starts `false`. The Layout
    /// hydration effect fills the store from IndexedDB once the workspace ID
    /// is available, then marks it initialized.
    pub fn new() -> Self {
        Self {
            inner: StoredValue::new(SendWrapper::new(SyncStoreInner {
                dashboards: ArcRwSignal::new(Vec::new()),
                chat_sessions: ArcRwSignal::new(Vec::new()),
                knowledge_docs: ArcRwSignal::new(Vec::new()),
                watches: ArcRwSignal::new(Vec::new()),
                workspace_settings: ArcRwSignal::new(None),
                initialized: ArcRwSignal::new(false),
            })),
        }
    }

    // ── Read-only derived signals ─────────────────────────────────────────────

    /// Reactive signal over the current dashboard list.
    pub fn dashboards(&self) -> Signal<Vec<DashboardListItem>> {
        let sig = self.inner.with_value(|inner| inner.dashboards.clone());
        Signal::derive(move || sig.get())
    }

    /// Reactive signal over the current chat session list.
    pub fn chat_sessions(&self) -> Signal<Vec<ChatSessionItem>> {
        let sig = self.inner.with_value(|inner| inner.chat_sessions.clone());
        Signal::derive(move || sig.get())
    }

    /// Reactive signal over the current knowledge document list.
    pub fn knowledge_docs(&self) -> Signal<Vec<DashboardListItem>> {
        let sig = self.inner.with_value(|inner| inner.knowledge_docs.clone());
        Signal::derive(move || sig.get())
    }

    /// Reactive signal over the current watch list.
    pub fn watches(&self) -> Signal<Vec<WatchListItem>> {
        let sig = self.inner.with_value(|inner| inner.watches.clone());
        Signal::derive(move || sig.get())
    }

    /// Reactive signal over the current workspace settings (singleton).
    pub fn workspace_settings(&self) -> Signal<Option<WorkspaceSettingsData>> {
        let sig = self.inner.with_value(|inner| inner.workspace_settings.clone());
        Signal::derive(move || sig.get())
    }

    /// `true` once the store has been hydrated from IndexedDB.
    ///
    /// Pages that read from this store should show a loading state until this
    /// signal becomes `true`.
    pub fn initialized(&self) -> Signal<bool> {
        let sig = self.inner.with_value(|inner| inner.initialized.clone());
        Signal::derive(move || sig.get())
    }

    // ── Bulk setters (bootstrap / hydration) ─────────────────────────────────

    /// Replace the entire dashboard list (called during IDB hydration).
    pub fn set_dashboards(&self, items: Vec<DashboardListItem>) {
        self.inner.with_value(|inner| inner.dashboards.set(items));
    }

    /// Replace the entire chat session list (called during IDB hydration).
    pub fn set_chat_sessions(&self, items: Vec<ChatSessionItem>) {
        self.inner.with_value(|inner| inner.chat_sessions.set(items));
    }

    /// Replace the entire knowledge document list (called during IDB hydration).
    pub fn set_knowledge_docs(&self, items: Vec<DashboardListItem>) {
        self.inner.with_value(|inner| inner.knowledge_docs.set(items));
    }

    /// Replace the entire watch list (called during IDB hydration).
    pub fn set_watches(&self, items: Vec<WatchListItem>) {
        self.inner.with_value(|inner| inner.watches.set(items));
    }

    /// Set the workspace settings singleton (called during IDB hydration).
    pub fn set_workspace_settings(&self, settings: Option<WorkspaceSettingsData>) {
        self.inner.with_value(|inner| inner.workspace_settings.set(settings));
    }

    // ── Single-item upserts (live sync) ───────────────────────────────────────

    /// Insert or update a dashboard by `dashboard_id`.
    pub fn upsert_dashboard(&self, item: DashboardListItem) {
        self.inner.with_value(|inner| {
            inner.dashboards.update(|list| {
                if let Some(existing) = list.iter_mut().find(|d| d.dashboard_id == item.dashboard_id) {
                    *existing = item;
                } else {
                    list.push(item);
                }
            });
        });
    }

    /// Insert or update a chat session by `session_id`.
    pub fn upsert_chat_session(&self, item: ChatSessionItem) {
        self.inner.with_value(|inner| {
            inner.chat_sessions.update(|list| {
                if let Some(existing) = list.iter_mut().find(|s| s.session_id == item.session_id) {
                    *existing = item;
                } else {
                    list.push(item);
                }
            });
        });
    }

    /// Insert or update a knowledge document by `dashboard_id`.
    pub fn upsert_knowledge_doc(&self, item: DashboardListItem) {
        self.inner.with_value(|inner| {
            inner.knowledge_docs.update(|list| {
                if let Some(existing) = list.iter_mut().find(|d| d.dashboard_id == item.dashboard_id) {
                    *existing = item;
                } else {
                    list.push(item);
                }
            });
        });
    }

    /// Insert or update a watch by `watch_id`.
    pub fn upsert_watch(&self, item: WatchListItem) {
        self.inner.with_value(|inner| {
            inner.watches.update(|list| {
                if let Some(existing) = list.iter_mut().find(|w| w.watch_id == item.watch_id) {
                    *existing = item;
                } else {
                    list.push(item);
                }
            });
        });
    }

    /// Update the workspace settings singleton (live sync).
    ///
    /// Since workspace settings are a singleton (one per workspace), upsert is
    /// equivalent to a simple set.
    pub fn upsert_workspace_settings(&self, settings: WorkspaceSettingsData) {
        self.inner.with_value(|inner| inner.workspace_settings.set(Some(settings)));
    }

    /// Apply a mutation to the current workspace settings in place.
    pub fn update_workspace_setting(&self, f: impl FnOnce(&mut WorkspaceSettingsData)) {
        self.inner.with_value(|inner| {
            inner.workspace_settings.update(|opt| {
                if let Some(ws) = opt.as_mut() {
                    f(ws);
                }
            });
        });
    }

    // ── Single-item removes (delete sync) ────────────────────────────────────

    /// Remove a dashboard by `dashboard_id`.
    pub fn remove_dashboard(&self, dashboard_id: &str) {
        self.inner.with_value(|inner| {
            inner.dashboards.update(|list| {
                list.retain(|d| d.dashboard_id != dashboard_id);
            });
        });
    }

    /// Remove a chat session by `session_id`.
    pub fn remove_chat_session(&self, session_id: &str) {
        self.inner.with_value(|inner| {
            inner.chat_sessions.update(|list| {
                list.retain(|s| s.session_id != session_id);
            });
        });
    }

    /// Remove a knowledge document by `dashboard_id`.
    pub fn remove_knowledge_doc(&self, dashboard_id: &str) {
        self.inner.with_value(|inner| {
            inner.knowledge_docs.update(|list| {
                list.retain(|d| d.dashboard_id != dashboard_id);
            });
        });
    }

    /// Remove a watch by `watch_id`.
    pub fn remove_watch(&self, watch_id: &str) {
        self.inner.with_value(|inner| {
            inner.watches.update(|list| {
                list.retain(|w| w.watch_id != watch_id);
            });
        });
    }

    /// Remove the workspace settings singleton (delete sync).
    pub fn remove_workspace_settings(&self) {
        self.inner.with_value(|inner| inner.workspace_settings.set(None));
    }

    // ── Count reconciliation (KYO-480) ────────────────────────────────────────

    /// Current row count for each entity type covered by count
    /// reconciliation (`kyomi_types::sync::entity_types::RECONCILED`), keyed
    /// by the same entity-type constants the server uses in `sync_complete`.
    ///
    /// Read with `get_untracked` — this is called from the `sync_complete`
    /// WebSocket message handler (`cache::sync_engine`), not from inside a
    /// reactive tracking scope, so there is nothing to subscribe to here.
    pub fn reconciliation_counts(&self) -> std::collections::HashMap<String, i64> {
        use kyomi_types::sync::entity_types;

        self.inner.with_value(|inner| {
            [
                (entity_types::DASHBOARD, inner.dashboards.get_untracked().len()),
                (entity_types::KNOWLEDGE, inner.knowledge_docs.get_untracked().len()),
                (entity_types::CHAT_SESSION, inner.chat_sessions.get_untracked().len()),
                (entity_types::WATCH, inner.watches.get_untracked().len()),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v as i64))
            .collect()
        })
    }

    /// Clear every locally cached row of one entity type.
    ///
    /// Used by the KYO-480 repair path: when [`reconcile::diverged_types`]
    /// (`crate::cache::reconcile`) flags an entity type as diverged, the
    /// caller wipes it from both this store and IndexedDB before requesting
    /// a fresh `sync_bootstrap`, so a stale-extra row that only exists
    /// locally does not survive the repair (a plain re-bootstrap only
    /// *inserts* — it never deletes rows the client already has that the
    /// server no longer does).
    ///
    /// A type outside `entity_types::RECONCILED` (currently only
    /// `workspace_settings`) is a no-op — reconciliation never targets it.
    pub fn clear_entity_type(&self, entity_type: &str) {
        use kyomi_types::sync::entity_types;

        match entity_type {
            et if et == entity_types::DASHBOARD => self.set_dashboards(Vec::new()),
            et if et == entity_types::KNOWLEDGE => self.set_knowledge_docs(Vec::new()),
            et if et == entity_types::CHAT_SESSION => self.set_chat_sessions(Vec::new()),
            et if et == entity_types::WATCH => self.set_watches(Vec::new()),
            other => {
                tracing::debug!(entity_type = other, "clear_entity_type: not a reconciled type — ignoring");
            }
        }
    }

    // ── State transitions ─────────────────────────────────────────────────────

    /// Mark the store as fully hydrated from IndexedDB.
    ///
    /// Called after all entity types have been read from IDB (or after the sync
    /// engine confirms an existing cursor). Pages waiting on `initialized()`
    /// will update reactively.
    pub fn mark_initialized(&self) {
        self.inner.with_value(|inner| inner.initialized.set(true));
    }

    /// Clear all lists and reset initialized to false.
    ///
    /// Called before hydrating from a different workspace's cache so stale
    /// data from the previous workspace doesn't leak into the new one.
    pub fn reset(&self) {
        self.inner.with_value(|inner| {
            inner.dashboards.set(Vec::new());
            inner.chat_sessions.set(Vec::new());
            inner.knowledge_docs.set(Vec::new());
            inner.watches.set(Vec::new());
            inner.workspace_settings.set(None);
            inner.initialized.set(false);
        });
    }
}

#[cfg(test)]
mod reconciliation_tests {
    //! KYO-480: tests for `reconciliation_counts` and `clear_entity_type`,
    //! the two `SyncStore` methods the count-reconciliation repair path
    //! (`cache::sync_engine`, wasm32-only, driven by
    //! `cache::reconcile::diverged_types` / `RepairGuard`) actually calls.
    //!
    //! Per KYO-427 ("a gate that shipped with a passing wiring test and did
    //! nothing" — see `cache::reconcile`'s tests for the pure decision
    //! logic), a test that only proves `diverged_types` returns the right
    //! `Vec<String>` is not enough: it proves the *decision* is right, not
    //! that acting on it actually restores the missing data. These tests
    //! instead drive the exact same `SyncStore` methods production calls —
    //! `clear_entity_type` (the repair's wipe step) and `upsert_watch` (what
    //! `apply_sync_action` calls for every `sync_action` message, including
    //! the ones the repair's own `sync_bootstrap` re-request triggers) — and
    //! assert on the store's real, observable contents afterwards, not on a
    //! return value.

    use super::*;
    use crate::types::WatchListItem;

    fn watch(id: &str) -> WatchListItem {
        WatchListItem {
            watch_id: id.to_string(),
            name: format!("Watch {id}"),
            prompt: "Check something".to_string(),
            schedule: "0 9 * * *".to_string(),
            mode: "alert".to_string(),
            enabled: true,
            last_run_at: None,
            last_run_status: None,
            next_run_at: None,
            created_at: "2026-08-23T00:00:00Z".to_string(),
            created_by: "user-1".to_string(),
            alert_emails: None,
            alert_emails_enabled: false,
            queries: None,
            slack_channel_id: None,
            slack_channel_name: None,
        }
    }

    /// `reconciliation_counts` must report the true current length of every
    /// reconciled list, not a stale or partial view.
    #[test]
    fn reconciliation_counts_reflects_current_store_contents() {
        let owner = Owner::new();
        owner.set();

        let store = SyncStore::new();
        store.set_watches(vec![watch("watch-1")]);
        store.set_dashboards(vec![]);
        store.set_knowledge_docs(vec![]);
        store.set_chat_sessions(vec![]);

        let counts = store.reconciliation_counts();
        assert_eq!(counts.get(kyomi_types::sync::entity_types::WATCH), Some(&1));
        assert_eq!(counts.get(kyomi_types::sync::entity_types::DASHBOARD), Some(&0));
    }

    /// The KYO-479 failure mode, reproduced and repaired end to end: a
    /// client whose local cache is missing an entity the server has.
    ///
    /// Seeds the store with only one of two "true" watches (the diverged
    /// cache), calls the exact repair-wipe method the sync engine calls on
    /// a detected mismatch, then applies the exact upsert method
    /// `apply_sync_action` calls for each entity in a `sync_bootstrap`
    /// response — proving the previously-missing watch is genuinely present
    /// afterwards, not just that a decision function said it should be.
    #[test]
    fn missing_entity_is_actually_present_after_wipe_and_reapply() {
        let owner = Owner::new();
        owner.set();

        let store = SyncStore::new();

        // Diverged cache: local only has watch-1; the server (per its count
        // in `sync_complete`) actually has two — watch-1 and watch-2, the
        // one that can never arrive via a plain delta (KYO-479's failure
        // mode: mutated before `sync_log` coverage began).
        store.set_watches(vec![watch("watch-1")]);
        assert_eq!(store.watches().get_untracked().len(), 1);

        // Repair step 1: wipe (`cache::sync_engine`'s repair path calls this
        // on every entity type `RepairGuard::admit` returns).
        store.clear_entity_type(kyomi_types::sync::entity_types::WATCH);
        assert_eq!(
            store.reconciliation_counts()[kyomi_types::sync::entity_types::WATCH],
            0,
            "wipe must actually empty the store, not just report a decision to wipe"
        );

        // Repair step 2: the `sync_bootstrap` re-request's resulting
        // `sync_action` stream is handled by `apply_sync_action`, which for
        // each watch calls exactly this — `SyncStore::upsert_watch`.
        store.upsert_watch(watch("watch-1"));
        store.upsert_watch(watch("watch-2"));

        let final_watches = store.watches().get_untracked();
        assert_eq!(final_watches.len(), 2, "both authoritative watches must be present after repair");
        assert!(
            final_watches.iter().any(|w| w.watch_id == "watch-2"),
            "the entity that was missing before the repair must actually be present now — \
             not merely that a repair was decided or dispatched"
        );
    }

    /// The stale-extras direction: local holds a row the server no longer
    /// has. A repair must not simply re-insert the authoritative set on top
    /// of the old one (upsert alone would still leave the stale extra
    /// behind) — it must wipe first.
    #[test]
    fn stale_extra_is_actually_gone_after_wipe_and_reapply() {
        let owner = Owner::new();
        owner.set();

        let store = SyncStore::new();

        // Local has three watches; the server's count says two — watch-3 is
        // a stale extra (e.g. deleted server-side while this client was
        // offline, with no delta ever reaching it).
        store.set_watches(vec![watch("watch-1"), watch("watch-2"), watch("watch-3")]);
        assert_eq!(store.watches().get_untracked().len(), 3);

        store.clear_entity_type(kyomi_types::sync::entity_types::WATCH);
        assert_eq!(store.reconciliation_counts()[kyomi_types::sync::entity_types::WATCH], 0);

        // Repair bootstrap re-applies only the two watches the server
        // actually has.
        store.upsert_watch(watch("watch-1"));
        store.upsert_watch(watch("watch-2"));

        let final_watches = store.watches().get_untracked();
        assert_eq!(final_watches.len(), 2, "stale extra must not survive the repair");
        assert!(
            !final_watches.iter().any(|w| w.watch_id == "watch-3"),
            "watch-3 (the stale extra) must be gone, not merely undercounted"
        );
    }

    /// `clear_entity_type` on a non-reconciled type (`workspace_settings`)
    /// is a documented no-op — reconciliation never targets it, since a
    /// singleton has no "count" to diverge from.
    #[test]
    fn clear_entity_type_ignores_non_reconciled_types() {
        let owner = Owner::new();
        owner.set();

        let store = SyncStore::new();
        store.set_watches(vec![watch("watch-1")]);
        store.clear_entity_type(kyomi_types::sync::entity_types::WORKSPACE_SETTINGS);
        assert_eq!(
            store.watches().get_untracked().len(),
            1,
            "clearing an unrelated entity type must not touch watches"
        );
    }
}
