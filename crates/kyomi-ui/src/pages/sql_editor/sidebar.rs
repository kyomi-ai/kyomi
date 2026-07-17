// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL Editor right sidebar — tabbed panel with catalog tree and query history.
//!
//! Hosts its content inside the shared [`RightPanel`] (Editorial Margin
//! pattern, see DESIGN.md). The header slot renders a segmented tab control
//! (Catalog / History) plus a context-aware refresh button; the close button
//! is owned by `RightPanel`. Body switches between `CatalogTree` and
//! `QueryHistory` based on the active tab.

use std::sync::Arc;

use leptos::prelude::*;
use phosphor_leptos::Icon;

use super::catalog_tree::CatalogTree;
use super::query_history::QueryHistory;
use super::state::SqlEditorState;
use super::types::SidebarTab;
use crate::components::toast::toast_success;
use crate::components::{Button, ButtonSize, ButtonVariant, RightPanel, SearchInput};

// ─── Constants ──────────────────────────────────────────────────────────────

const MIN_WIDTH: f64 = 280.0;
const MAX_WIDTH: f64 = 480.0;
const DEFAULT_WIDTH: f64 = 320.0;

// ─── Main component ────────────────────────────────────────────────────────

/// Right sidebar for the SQL Editor page.
///
/// Contains "Catalog" and "History" tabs. Opens/closes via
/// `SqlEditorState::active_right_tab`.
#[component]
pub fn SqlEditorSidebar(
    /// Currently selected datasource slug (for catalog tree + history).
    #[prop(into)]
    datasource_slug: Signal<Option<String>>,
    /// Callback when user clicks a table name in the catalog tree.
    on_table_click: Callback<String>,
    /// Callback when user clicks a column name in the catalog tree.
    on_column_click: Callback<String>,
    /// Callback when user selects a query from history (query_text, datasource_slug).
    on_query_select: Callback<(String, Option<String>)>,
    /// Callback when user clicks the info button on a table in the catalog.
    on_table_info: Callback<String>,
) -> impl IntoView {
    let state = SqlEditorState::use_state();
    let sidebar_width = RwSignal::new(DEFAULT_WIDTH);

    // Derived: which tab is active within the sidebar. Use try_get() so the
    // memo is safe during scope disposal (parent signals may be freed first).
    let active_tab = Memo::new(move |_| {
        state
            .active_right_tab
            .try_get()
            .flatten()
            .unwrap_or(SidebarTab::Catalog)
    });

    // Derived: is the sidebar open at all?
    let is_open = Memo::new(move |_| state.active_right_tab.try_get().flatten().is_some());

    // ── Tab + close handlers ────────────────────────────────────────────
    let set_catalog_tab = move |_| {
        state.set_active_right_tab(Some(SidebarTab::Catalog));
    };
    let set_history_tab = move |_| {
        state.set_active_right_tab(Some(SidebarTab::History));
    };
    let on_close = Callback::new(move |()| {
        state.set_active_right_tab(None);
    });

    // ── Catalog search state (immediate — client-side filtering only) ──
    let (catalog_search_input, set_catalog_search_input) = signal(String::new());
    // `_set_catalog_refresh_trigger` has no callers yet — catalog indexing now
    // runs in the background (KYO-143), so bumping the tree refresh right
    // after `refresh_catalog` returns would reload stale pre-index data.
    // KYO-144 (poll for completion) will call this once indexing is done.
    let (catalog_refresh_trigger, _set_catalog_refresh_trigger) = signal(0u32);

    // ── History search state (with 300ms debounce) ──────────────────────
    let (history_search_input, set_history_search_input) = signal(String::new());
    let (history_search, _set_history_search) = signal(String::new());
    // History refresh tick lives on the shared SqlEditorState so
    // `execution::save_to_history` can bump it after a query runs and the
    // QueryHistory panel will refetch automatically.
    let history_refresh_trigger =
        Signal::derive(move || state.history_refresh_tick.try_get().unwrap_or(0));

    // Debounce history search input → actual search query.
    #[cfg(feature = "hydrate")]
    {
        use gloo_timers::callback::Timeout;
        use send_wrapper::SendWrapper;
        use std::cell::Cell;
        use std::rc::Rc;

        let set_history_search = _set_history_search;
        let pending = Rc::new(Cell::new(None::<SendWrapper<Timeout>>));

        Effect::new(move |_| {
            let Some(input) = history_search_input.try_get() else { return };
            let pending = pending.clone();
            pending.set(None); // cancel previous timeout
            let timeout = Timeout::new(300, move || {
                set_history_search.try_set(input);
            });
            pending.set(Some(SendWrapper::new(timeout)));
        });
    }

    // ── Tab button class helper (segmented control pattern) ─────────────
    let tab_class = move |tab: SidebarTab| {
        let base = "flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-md transition-colors";
        if active_tab.get() == tab {
            format!("{base} bg-background text-foreground shadow-sm")
        } else {
            format!("{base} text-muted-foreground hover:text-foreground")
        }
    };

    // ── Refresh catalog handler ─────────────────────────────────────────
    let (refreshing_catalog, set_refreshing_catalog) = signal(false);

    let handle_refresh_catalog = move |_: web_sys::MouseEvent| {
        let slug = datasource_slug.get_untracked();
        let Some(slug) = slug else { return };
        set_refreshing_catalog.set(true);

        leptos::task::spawn_local(async move {
            match crate::server_fns::sql_editor::refresh_catalog(slug).await {
                Ok(message) => {
                    // Indexing now runs in the background — `message` says
                    // "started", not "done". Don't bump the catalog tree's
                    // refresh trigger here, it would just reload the same
                    // stale pre-index data.
                    toast_success(message);
                }
                Err(e) => {
                    crate::components::toast::toast_error(format!("Catalog refresh failed: {e}"));
                }
            }
            set_refreshing_catalog.try_set(false);
        });
    };

    // ── Header slot: segmented tabs + refresh button ────────────────────
    let header_fn: ChildrenFn = Arc::new(move || {
        view! {
            <div class="flex items-center gap-2 min-w-0">
                <div
                    class="flex items-center rounded-lg bg-muted p-1"
                    role="tablist"
                    aria-label="Sidebar panels"
                >
                    <button
                        class=move || tab_class(SidebarTab::Catalog)
                        on:click=set_catalog_tab
                        role="tab"
                        aria-selected=move || if active_tab.get() == SidebarTab::Catalog { "true" } else { "false" }
                    >
                        "Catalog"
                    </button>
                    <button
                        class=move || tab_class(SidebarTab::History)
                        on:click=set_history_tab
                        role="tab"
                        aria-selected=move || if active_tab.get() == SidebarTab::History { "true" } else { "false" }
                    >
                        "History"
                    </button>
                </div>
                // Refresh button — catalog tab only
                <Show when=move || active_tab.get() == SidebarTab::Catalog>
                    <Button
                        variant=ButtonVariant::GhostMuted
                        size=ButtonSize::IconSm
                        disabled=MaybeProp::derive(move || Some(refreshing_catalog.get()))
                        aria_label="Refresh catalog"
                        on:click=handle_refresh_catalog
                    >
                        <Icon
                            icon=phosphor_leptos::ARROWS_CLOCKWISE
                            size="14px"
                            attr:class=move || if refreshing_catalog.get() { "animate-spin" } else { "" }
                        />
                    </Button>
                </Show>
            </div>
        }
        .into_any()
    });

    // ── Body: tabbed content ────────────────────────────────────────────
    view! {
        <RightPanel
            open=Signal::from(is_open)
            on_close=on_close
            width=sidebar_width
            min_width=MIN_WIDTH
            max_width=MAX_WIDTH
            header=header_fn
            close_label="Close sidebar".to_string()
        >
            <div class="flex-1 h-full">
                // Catalog tab
                <div class=move || {
                    if active_tab.get() == SidebarTab::Catalog { "h-full block" } else { "h-full hidden" }
                }>
                    <div class="flex flex-col h-full">
                        // Catalog search input
                        <div class="px-3 py-2 border-b border-border">
                            <SearchInput
                                value=Signal::derive(move || catalog_search_input.get())
                                on_input=Callback::new(move |val: String| set_catalog_search_input.set(val))
                                placeholder="Search tables..."
                            />
                        </div>
                        // Catalog tree
                        <div class="flex-1 overflow-auto">
                            <CatalogTree
                                datasource_slug=datasource_slug
                                search_query=catalog_search_input
                                refresh_trigger=catalog_refresh_trigger
                                on_table_click=on_table_click
                                on_column_click=on_column_click
                                on_table_info=on_table_info
                            />
                        </div>
                    </div>
                </div>

                // History tab
                <div class=move || {
                    if active_tab.get() == SidebarTab::History { "h-full block" } else { "h-full hidden" }
                }>
                    <div class="flex flex-col h-full">
                        // History search input
                        <div class="px-3 py-2 border-b border-border flex-shrink-0">
                            <SearchInput
                                value=Signal::derive(move || history_search_input.get())
                                on_input=Callback::new(move |val: String| set_history_search_input.set(val))
                                placeholder="Search query history..."
                            />
                        </div>
                        // History list
                        <div class="flex-1 min-h-0">
                            <QueryHistory
                                search_query=history_search
                                refresh_trigger=history_refresh_trigger
                                on_query_select=on_query_select
                                on_search_change=Callback::new(move |s: String| {
                                    set_history_search_input.set(s);
                                })
                            />
                        </div>
                    </div>
                </div>
            </div>
        </RightPanel>
    }
}
