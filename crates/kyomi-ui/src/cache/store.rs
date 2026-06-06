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
