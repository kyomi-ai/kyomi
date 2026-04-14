// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL Editor state management — reactive signals + localStorage persistence.
//!
//! This module provides `SqlEditorState`, which mirrors the Zustand store in
//! `apps/frontend/src/features/sql-editor/store.ts`. It is provided as a
//! Leptos context at the SQL Editor page level.
//!
//! **localStorage persistence** (WASM-only):
//! - State is saved on every mutation (debounced).
//! - On load: tabs are restored with row data stripped and `needs_refresh` set.
//! - Row data is never persisted (can be huge, Date objects don't serialize).

use std::collections::HashMap;

use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use serde::{Deserialize, Serialize};

use super::types::{NewTabData, ResultTab, SidebarTab, TableUIState};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// localStorage key — matches the React store's key.
#[cfg(target_arch = "wasm32")]
const STORAGE_KEY: &str = "sql-editor-storage";

/// Maximum number of unpinned tabs before the oldest is evicted.
const MAX_UNPINNED_TABS: usize = 5;

/// Number of tab colors that cycle (0..=7).
const COLOR_COUNT: u8 = 8;

/// Default page size for new tabs.
const DEFAULT_PAGE_SIZE: u32 = 50;

/// Default sidebar width percentage.
const DEFAULT_SIDEBAR_PERCENTAGE: u32 = 30;

// ─────────────────────────────────────────────────────────────────────────────
// Persisted shape — what we write to / read from localStorage
// ─────────────────────────────────────────────────────────────────────────────

/// The subset of state that gets persisted to localStorage.
/// Matches the Zustand `partialize` output exactly.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedState {
    tabs: Vec<ResultTab>,
    active_tab_id: Option<String>,
    query_text: String,
    table_ui_state: HashMap<String, TableUIState>,
    next_color_index: u8,
    default_page_size: u32,
    active_right_tab: Option<SidebarTab>,
    right_sidebar_percentage: u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// SqlEditorState — the reactive context
// ─────────────────────────────────────────────────────────────────────────────

/// Reactive SQL Editor state, provided via Leptos context.
///
/// All fields are `RwSignal` so that fine-grained reactivity works across
/// components. Methods match the Zustand actions one-for-one.
#[derive(Clone, Copy)]
pub struct SqlEditorState {
    /// All result tabs (pinned + unpinned).
    pub tabs: RwSignal<Vec<ResultTab>>,
    /// ID of the currently active tab, or `None`.
    pub active_tab_id: RwSignal<Option<String>>,
    /// SQL text in the editor (shared, separate from tab results).
    pub query_text: RwSignal<String>,
    /// Per-tab table UI state (sort, pagination).
    pub table_ui_state: RwSignal<HashMap<String, TableUIState>>,
    /// Next color index to assign (0-7, cycles).
    pub next_color_index: RwSignal<u8>,
    /// User's preferred page size for new tabs.
    pub default_page_size: RwSignal<u32>,
    /// Which right-sidebar panel is open, or `None` (closed).
    pub active_right_tab: RwSignal<Option<SidebarTab>>,
    /// Right sidebar width percentage (0-50).
    pub right_sidebar_percentage: RwSignal<u32>,
    /// Monotonic counter the sidebar's query-history Resource keys on. Bumped
    /// after each successful `save_query_history` call in
    /// `execution::save_to_history` so the history list reflects the query
    /// the user just ran without waiting for a page reload. Not persisted.
    pub history_refresh_tick: RwSignal<u32>,
}

impl SqlEditorState {
    // ── Construction ─────────────────────────────────────────────────────

    /// Create a new `SqlEditorState` with default values, then attempt to
    /// restore from localStorage (WASM only). Call this once at the SQL Editor
    /// page level and immediately `provide_context`.
    pub fn provide() -> Self {
        let state = Self {
            tabs: RwSignal::new(Vec::new()),
            active_tab_id: RwSignal::new(None),
            query_text: RwSignal::new(String::new()),
            table_ui_state: RwSignal::new(HashMap::new()),
            next_color_index: RwSignal::new(0),
            default_page_size: RwSignal::new(DEFAULT_PAGE_SIZE),
            active_right_tab: RwSignal::new(None),
            right_sidebar_percentage: RwSignal::new(DEFAULT_SIDEBAR_PERCENTAGE),
            history_refresh_tick: RwSignal::new(0),
        };

        // Restore from localStorage on the client.
        #[cfg(target_arch = "wasm32")]
        state.restore_from_storage();

        // Set up auto-save effect (debounced).
        #[cfg(target_arch = "wasm32")]
        state.setup_persistence_effect();

        provide_context(state);
        state
    }

    /// Retrieve the `SqlEditorState` from Leptos context.
    ///
    /// # Panics
    /// Panics if called outside a context that called `SqlEditorState::provide()`.
    pub fn use_state() -> Self {
        use_context::<Self>().expect("SqlEditorState not provided — call SqlEditorState::provide() first")
    }

    // ── Actions (match Zustand store 1:1) ────────────────────────────────

    /// Create a new result tab from the given data.
    ///
    /// - Assigns auto-generated `id`, `created_at`, `updated_at`, `pinned`,
    ///   `color_index`.
    /// - Enforces the 5-unpinned-tab limit by evicting the oldest unpinned tab.
    /// - Cycles `next_color_index` through 0-7.
    /// - Returns the new tab's ID.
    pub fn add_tab(&self, data: NewTabData) -> String {
        let id = generate_tab_id();
        let now = js_now();
        let color_index = self.next_color_index.get_untracked();

        let new_tab = ResultTab {
            id: id.clone(),
            label: data.label,
            query: data.query,
            status: data.status,
            result: data.result,
            error: data.error,
            visualization: data.visualization,
            pinned: false,
            color_index,
            created_at: now,
            updated_at: now,
            needs_refresh: data.needs_refresh,
            datasource_slug: data.datasource_slug,
            datasource_type: data.datasource_type,
        };

        let default_page_size = self.default_page_size.get_untracked();

        self.tabs.update(|tabs| {
            // Enforce 5-unpinned limit: if >= 5 unpinned, remove oldest.
            let unpinned_count = tabs.iter().filter(|t| !t.pinned).count();
            if unpinned_count >= MAX_UNPINNED_TABS
                && let Some(oldest_id) = tabs
                    .iter()
                    .filter(|t| !t.pinned)
                    .min_by(|a, b| a.created_at.partial_cmp(&b.created_at).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|t| t.id.clone())
            {
                // Clean up UI state for the evicted tab.
                self.table_ui_state.update(|ui| {
                    ui.remove(&oldest_id);
                });
                tabs.retain(|t| t.id != oldest_id);
            }

            tabs.push(new_tab);
        });

        // Initialize table UI state for the new tab.
        self.table_ui_state.update(|ui| {
            ui.insert(
                id.clone(),
                TableUIState {
                    page_size: default_page_size,
                    ..Default::default()
                },
            );
        });

        self.active_tab_id.set(Some(id.clone()));
        self.next_color_index.set((color_index + 1) % COLOR_COUNT);

        id
    }

    /// Update properties of an existing tab.
    ///
    /// The `updater` closure receives a mutable reference to the tab and can
    /// modify any fields. `updated_at` is automatically set.
    pub fn update_tab(&self, tab_id: &str, updater: impl FnOnce(&mut ResultTab)) {
        let now = js_now();
        self.tabs.update(|tabs| {
            if let Some(tab) = tabs.iter_mut().find(|t| t.id == tab_id) {
                updater(tab);
                tab.updated_at = now;
            }
        });
    }

    /// Remove a tab. If the removed tab was active, select the adjacent tab
    /// (preferring the right neighbor, falling back to last).
    pub fn remove_tab(&self, tab_id: &str) {
        let current_active = self.active_tab_id.get_untracked();
        let need_new_active = current_active.as_deref() == Some(tab_id);

        let mut new_active_id: Option<String> = None;

        self.tabs.update(|tabs| {
            if need_new_active {
                let removed_index = tabs.iter().position(|t| t.id == tab_id);
                // Build new list without the removed tab.
                let new_tabs: Vec<ResultTab> = tabs.iter().filter(|t| t.id != tab_id).cloned().collect();
                if let Some(idx) = removed_index
                    && !new_tabs.is_empty()
                {
                    let new_index = idx.min(new_tabs.len() - 1);
                    new_active_id = Some(new_tabs[new_index].id.clone());
                }
                *tabs = new_tabs;
            } else {
                tabs.retain(|t| t.id != tab_id);
            }
        });

        // Clean up table UI state.
        self.table_ui_state.update(|ui| {
            ui.remove(tab_id);
        });

        if need_new_active {
            self.active_tab_id.set(new_active_id);
        }
    }

    /// Switch the active tab.
    pub fn set_active_tab(&self, tab_id: Option<String>) {
        self.active_tab_id.set(tab_id);
    }

    /// Toggle the pinned state of a tab.
    pub fn toggle_pin(&self, tab_id: &str) {
        self.tabs.update(|tabs| {
            if let Some(tab) = tabs.iter_mut().find(|t| t.id == tab_id) {
                tab.pinned = !tab.pinned;
            }
        });
    }

    /// Update the SQL editor text.
    pub fn set_query_text(&self, text: String) {
        self.query_text.set(text);
    }

    /// Update the table UI state (sort, pagination) for a specific tab.
    ///
    /// Merges with existing state — the closure mutates a `&mut TableUIState`,
    /// creating a default entry if absent. This matches React's
    /// `setTableUIState` which spreads updates over existing state.
    pub fn set_table_ui_state(&self, tab_id: &str, updater: impl FnOnce(&mut TableUIState)) {
        self.table_ui_state.update(|ui| {
            let entry = ui.entry(tab_id.to_owned()).or_default();
            updater(entry);
        });
    }

    /// Update the user's preferred default page size.
    pub fn set_default_page_size(&self, page_size: u32) {
        self.default_page_size.set(page_size);
    }

    /// Set which right-sidebar panel is open, or `None` to close it.
    pub fn set_active_right_tab(&self, tab: Option<SidebarTab>) {
        self.active_right_tab.set(tab);
    }

    /// Set the right sidebar width percentage (clamped 0-50).
    pub fn set_right_sidebar_percentage(&self, percentage: u32) {
        self.right_sidebar_percentage.set(percentage.min(50));
    }

    /// Clear all tabs and reset tab-related state.
    pub fn clear_all_tabs(&self) {
        self.tabs.set(Vec::new());
        self.active_tab_id.set(None);
        self.table_ui_state.set(HashMap::new());
    }

    // ── Derived / convenience ────────────────────────────────────────────

    /// Get the currently active tab (reactive — re-runs when tabs or
    /// active_tab_id change).
    pub fn active_tab(&self) -> Memo<Option<ResultTab>> {
        let tabs = self.tabs;
        let active_id = self.active_tab_id;
        Memo::new(move |_| {
            let id = active_id.get()?;
            tabs.get().into_iter().find(|t| t.id == id)
        })
    }

    /// Get the table UI state for the currently active tab (reactive).
    pub fn active_table_ui_state(&self) -> Memo<TableUIState> {
        let active_id = self.active_tab_id;
        let table_ui = self.table_ui_state;
        Memo::new(move |_| {
            let id = active_id.get();
            match id {
                Some(id) => table_ui.get().get(&id).cloned().unwrap_or_default(),
                None => TableUIState::default(),
            }
        })
    }

    // ── localStorage (WASM-only) ─────────────────────────────────────────

    /// Restore state from localStorage. Called once during `provide()`.
    #[cfg(target_arch = "wasm32")]
    fn restore_from_storage(&self) {
        let Some(storage) = web_sys::window()
            .and_then(|w| w.local_storage().ok())
            .flatten()
        else {
            return;
        };

        let Some(json) = storage.get_item(STORAGE_KEY).ok().flatten() else {
            // No saved state — sidebar stays closed (matches React default).
            return;
        };

        // The React Zustand persist middleware wraps the state in
        // `{ state: { ... }, version: 0 }`. Try both shapes.
        let persisted: Option<PersistedState> = serde_json::from_str::<serde_json::Value>(&json)
            .ok()
            .and_then(|val| {
                // Try wrapped shape first: { state: { ... } }
                if let Some(inner) = val.get("state") {
                    serde_json::from_value(inner.clone()).ok()
                } else {
                    // Try flat shape
                    serde_json::from_value(val).ok()
                }
            });

        let Some(persisted) = persisted else {
            return;
        };

        self.tabs.set(persisted.tabs);
        self.active_tab_id.set(persisted.active_tab_id);
        self.query_text.set(persisted.query_text);
        self.table_ui_state.set(persisted.table_ui_state);
        self.next_color_index.set(persisted.next_color_index);
        self.default_page_size.set(persisted.default_page_size);
        self.active_right_tab.set(persisted.active_right_tab);
        self.right_sidebar_percentage.set(persisted.right_sidebar_percentage);
    }

    /// Set up a reactive effect that persists state to localStorage on every
    /// change. Uses `gloo_timers` to debounce writes.
    #[cfg(target_arch = "wasm32")]
    fn setup_persistence_effect(&self) {
        use gloo_timers::callback::Timeout;
        use send_wrapper::SendWrapper;
        use std::cell::Cell;
        use std::rc::Rc;

        let tabs = self.tabs;
        let active_tab_id = self.active_tab_id;
        let query_text = self.query_text;
        let table_ui_state = self.table_ui_state;
        let next_color_index = self.next_color_index;
        let default_page_size = self.default_page_size;
        let active_right_tab = self.active_right_tab;
        let right_sidebar_percentage = self.right_sidebar_percentage;

        // Debounce handle stored in a SendWrapper so it can live in the
        // reactive graph.
        let pending = Rc::new(Cell::new(None::<SendWrapper<Timeout>>));

        Effect::new(move |_| {
            // Read all signals to subscribe to them.
            let tabs_val = tabs.get();
            let active_tab_id_val = active_tab_id.get();
            let query_text_val = query_text.get();
            let table_ui_state_val = table_ui_state.get();
            let next_color_index_val = next_color_index.get();
            let default_page_size_val = default_page_size.get();
            let active_right_tab_val = active_right_tab.get();
            let right_sidebar_percentage_val = right_sidebar_percentage.get();

            // Strip row data from tabs before persisting (matches React's
            // partialize).
            let persisted_tabs: Vec<ResultTab> = tabs_val
                .into_iter()
                .map(|mut tab| {
                    if let Some(ref mut result) = tab.result {
                        result.rows = Vec::new();
                        result.row_count = 0;
                        // Keep columns, totalRows, queryHandle, executionTime,
                        // bytesProcessed.
                        tab.needs_refresh = true;
                    }
                    tab
                })
                .collect();

            let persisted = PersistedState {
                tabs: persisted_tabs,
                active_tab_id: active_tab_id_val,
                query_text: query_text_val,
                table_ui_state: table_ui_state_val,
                next_color_index: next_color_index_val,
                default_page_size: default_page_size_val,
                active_right_tab: active_right_tab_val,
                right_sidebar_percentage: right_sidebar_percentage_val,
            };

            // Cancel any pending write and schedule a new one (100ms debounce).
            let pending = pending.clone();
            pending.set(None); // drop previous timeout
            let timeout = Timeout::new(100, move || {
                if let Ok(json) = serde_json::to_string(&persisted) {
                    if let Some(storage) = web_sys::window()
                        .and_then(|w| w.local_storage().ok())
                        .flatten()
                    {
                        let _ = storage.set_item(STORAGE_KEY, &json);
                    }
                }
            });
            pending.set(Some(SendWrapper::new(timeout)));
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Generate a unique tab ID.
/// Matches the React `generateTabId()`: `tab_{timestamp}_{random}`.
///
/// The random suffix replicates `Math.random().toString(36).substr(2, 9)` —
/// a 9-character string using digits 0-9 and letters a-z.
fn generate_tab_id() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let now = js_sys::Date::now() as u64;
        let rand_str = to_base36(js_sys::Math::random(), 9);
        format!("tab_{now}_{rand_str}")
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        // Server-side fallback — use a simple counter. Tab IDs are only
        // generated client-side in practice, but this keeps the code
        // compilable under SSR.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("tab_ssr_{n}")
    }
}

/// Convert the fractional part of a float (0.0..1.0) to a base-36 string.
///
/// Replicates `Math.random().toString(36).substr(2, len)` in JavaScript:
/// repeatedly multiplies the fractional part by 36 and extracts digits from
/// the charset `0-9a-z`.
#[cfg(target_arch = "wasm32")]
fn to_base36(value: f64, len: usize) -> String {
    const CHARSET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut frac = value.fract().abs();
    let mut result = String::with_capacity(len);
    for _ in 0..len {
        frac *= 36.0;
        let digit = frac as usize;
        result.push(CHARSET[digit % 36] as char);
        frac -= digit as f64;
    }
    result
}

/// Get the current timestamp in milliseconds (matching `Date.now()`).
fn js_now() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0)
    }
}
