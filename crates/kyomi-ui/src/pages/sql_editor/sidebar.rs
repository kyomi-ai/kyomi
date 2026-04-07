// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL Editor right sidebar — tabbed panel with catalog tree and query history.
//!
//! Matches the React `SQLEditor.jsx` sidebar section:
//! - Two tabs: "Catalog" and "History"
//! - Resizable width via drag handle (pixel-based, clamped 280-480px)
//! - Toggle button to show/hide
//! - On mobile (<768px): slide-in overlay with backdrop

use leptos::prelude::*;
#[cfg(feature = "hydrate")]
use wasm_bindgen::prelude::*;

use super::catalog_tree::CatalogTree;
use super::query_history::QueryHistory;
use super::state::SqlEditorState;
use super::types::SidebarTab;
use crate::components::dashboard::shared::use_is_mobile;

// ─── Constants ──────────────────────────────────────────────────────────────

#[cfg(feature = "hydrate")]
const MIN_WIDTH: f64 = 280.0;
#[cfg(feature = "hydrate")]
const MAX_WIDTH: f64 = 480.0;
const DEFAULT_WIDTH: f64 = 320.0;

// ─── Main component ────────────────────────────────────────────────────────

/// Right sidebar for the SQL Editor page.
///
/// Contains "Catalog" and "History" tabs in a resizable panel (desktop) or
/// slide-in overlay (mobile). Reads/writes `active_right_tab` from
/// `SqlEditorState` context.
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
) -> impl IntoView {
    let state = SqlEditorState::use_state();
    let is_mobile = use_is_mobile();

    // Local sidebar width (pixel-based, clamped MIN_WIDTH..MAX_WIDTH).
    let (sidebar_width, _set_sidebar_width) = signal(DEFAULT_WIDTH);
    // Re-bind under hydrate so the resize handler can use it.
    #[cfg(feature = "hydrate")]
    let set_sidebar_width = _set_sidebar_width;
    let (is_resizing, set_is_resizing) = signal(false);

    // Derived: which tab is active within the sidebar.
    let active_tab = Memo::new(move |_| {
        state
            .active_right_tab
            .get()
            .unwrap_or(SidebarTab::Catalog)
    });

    // Derived: is the sidebar open at all?
    let is_open = Memo::new(move |_| state.active_right_tab.get().is_some());

    // ── Tab switchers ───────────────────────────────────────────────────
    let set_catalog_tab = move |_| {
        state.set_active_right_tab(Some(SidebarTab::Catalog));
    };
    let set_history_tab = move |_| {
        state.set_active_right_tab(Some(SidebarTab::History));
    };
    let close_sidebar = move |_: web_sys::MouseEvent| {
        state.set_active_right_tab(None);
    };
    let close_sidebar_backdrop = move |_: web_sys::MouseEvent| {
        state.set_active_right_tab(None);
    };

    // ── Resize drag handling (desktop) ──────────────────────────────────
    // Stores active drag cleanup so on_cleanup can remove listeners if the
    // component unmounts mid-drag.

    #[cfg(feature = "hydrate")]
    let drag_cleanup: StoredValue<Option<send_wrapper::SendWrapper<Box<dyn FnOnce()>>>> =
        StoredValue::new(None);

    let handle_resize_start = move |ev: web_sys::MouseEvent| {
        ev.prevent_default();
        set_is_resizing.set(true);

        #[cfg(feature = "hydrate")]
        {
            use std::cell::RefCell;
            use std::rc::Rc;
            use wasm_bindgen::closure::Closure;

            let start_x = ev.client_x() as f64;
            let start_w = sidebar_width.get_untracked();

            let Some(window) = web_sys::window() else {
                return;
            };
            let Some(document) = window.document() else {
                return;
            };

            let move_handler =
                Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |ev: web_sys::MouseEvent| {
                    let diff = start_x - ev.client_x() as f64;
                    let new_width = (start_w + diff).clamp(MIN_WIDTH, MAX_WIDTH);
                    set_sidebar_width.set(new_width);
                });

            let move_ref = move_handler
                .as_ref()
                .unchecked_ref::<js_sys::Function>()
                .clone();
            let document_for_up = document.clone();
            let move_fn_for_up = move_ref.clone();

            let closures: Rc<RefCell<Option<(
                Closure<dyn FnMut(web_sys::MouseEvent)>,
                Closure<dyn FnMut()>,
            )>>> = Rc::new(RefCell::new(None));
            let closures_for_up = closures.clone();

            let up_handler = Closure::<dyn FnMut()>::new(move || {
                set_is_resizing.set(false);
                let _ = document_for_up
                    .remove_event_listener_with_callback("mousemove", &move_fn_for_up);
                if let Some((_, ref up_cb)) = *closures_for_up.borrow() {
                    let _ = document_for_up.remove_event_listener_with_callback(
                        "mouseup",
                        up_cb.as_ref().unchecked_ref(),
                    );
                }
                if let Some(body) = document_for_up.body() {
                    let _ = body.style().set_property("cursor", "");
                    let _ = body.style().set_property("user-select", "");
                }
                closures_for_up.borrow_mut().take();
                drag_cleanup.set_value(None);
            });

            let _ =
                document.add_event_listener_with_callback("mousemove", move_ref.unchecked_ref());
            let _ = document
                .add_event_listener_with_callback("mouseup", up_handler.as_ref().unchecked_ref());

            *closures.borrow_mut() = Some((move_handler, up_handler));

            let closures_for_teardown = closures;
            let document_for_teardown = document.clone();
            let move_ref_for_teardown = move_ref.clone();
            let teardown: Box<dyn FnOnce()> = Box::new(move || {
                if let Some((_, ref up_cb)) = *closures_for_teardown.borrow() {
                    let _ = document_for_teardown
                        .remove_event_listener_with_callback("mousemove", &move_ref_for_teardown);
                    let _ = document_for_teardown.remove_event_listener_with_callback(
                        "mouseup",
                        up_cb.as_ref().unchecked_ref(),
                    );
                }
                closures_for_teardown.borrow_mut().take();
            });
            drag_cleanup.set_value(Some(send_wrapper::SendWrapper::new(teardown)));

            if let Some(body) = document.body() {
                let _ = body.style().set_property("cursor", "col-resize");
                let _ = body.style().set_property("user-select", "none");
            }
        }
    };

    #[cfg(feature = "hydrate")]
    on_cleanup(move || {
        if let Some(teardown) = drag_cleanup.try_update_value(|v| v.take()).flatten() {
            teardown.take()();
        }
    });

    // ── Catalog search state (immediate — client-side filtering only) ──
    let (catalog_search_input, set_catalog_search_input) = signal(String::new());
    let (catalog_refresh_trigger, set_catalog_refresh_trigger) = signal(0u32);

    // ── History search state (with 300ms debounce) ──────────────────────
    let (history_search_input, set_history_search_input) = signal(String::new());
    let (history_search, _set_history_search) = signal(String::new());
    let (history_refresh_trigger, _set_history_refresh_trigger) = signal(0u32);

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
            let input = history_search_input.get();
            let pending = pending.clone();
            pending.set(None); // cancel previous timeout
            let timeout = Timeout::new(300, move || {
                set_history_search.set(input);
            });
            pending.set(Some(SendWrapper::new(timeout)));
        });
    }

    // ── Tab button class helper ─────────────────────────────────────────
    let tab_class = move |tab: SidebarTab| {
        let base = "px-3 py-1.5 text-sm font-medium rounded-md transition-colors";
        if active_tab.get() == tab {
            format!("{base} bg-card text-foreground shadow")
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
            let result = crate::server_fns::sql_editor::refresh_catalog(slug).await;
            if result.is_ok() {
                set_catalog_refresh_trigger.update(|n| *n += 1);
            }
            set_refreshing_catalog.set(false);
        });
    };

    // ── Panel content (shared between mobile and desktop) ───────────────
    let panel_content = move || {
        let ds_slug = datasource_slug;
        let on_table = on_table_click;
        let on_column = on_column_click;
        let on_query = on_query_select;

        view! {
            <div class="flex flex-col flex-1 min-w-0 h-full">
                // Sidebar header with tabs
                <div class="p-3 border-b border-border flex items-center justify-between flex-shrink-0">
                    <div class="flex items-center gap-1 bg-accent rounded-lg p-1" role="tablist" aria-label="Sidebar panels">
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
                    <div class="flex items-center gap-1">
                        // Refresh button (catalog tab only)
                        <Show when=move || active_tab.get() == SidebarTab::Catalog>
                            <button
                                class="p-1.5 text-muted-foreground hover:text-primary hover:bg-secondary rounded-md transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                                aria-label=move || {
                                    if refreshing_catalog.get() {
                                        "Refreshing catalog..."
                                    } else {
                                        "Refresh catalog"
                                    }
                                }
                                disabled=move || refreshing_catalog.get()
                                on:click=handle_refresh_catalog
                            >
                                <svg
                                    class=move || {
                                        if refreshing_catalog.get() {
                                            "w-4 h-4 animate-spin"
                                        } else {
                                            "w-4 h-4"
                                        }
                                    }
                                    fill="none"
                                    stroke="currentColor"
                                    viewBox="0 0 24 24"
                                >
                                    <path
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                        stroke-width="2"
                                        d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"
                                    />
                                </svg>
                            </button>
                        </Show>
                        // Close button
                        <button
                            class="p-1 text-muted-foreground hover:text-foreground rounded-md transition-colors"
                            aria-label="Close"
                            on:click=close_sidebar
                        >
                            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                            </svg>
                        </button>
                    </div>
                </div>

                // Tab content
                <div class="flex-1 overflow-hidden">
                    // Catalog tab
                    <div class=move || {
                        if active_tab.get() == SidebarTab::Catalog { "h-full block" } else { "h-full hidden" }
                    }>
                        <div class="flex flex-col h-full">
                            // Catalog search input
                            <div class="px-3 py-2 border-b border-border">
                                <div class="relative">
                                    <input
                                        type="text"
                                        placeholder="Search tables..."
                                        prop:value=move || catalog_search_input.get()
                                        on:input=move |ev| {
                                            set_catalog_search_input.set(event_target_value(&ev));
                                        }
                                        class="w-full px-3 py-2 pr-8 text-sm border border-border rounded-md focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring bg-background text-foreground"
                                    />
                                    <Show when=move || !catalog_search_input.get().is_empty()>
                                        <button
                                            class="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground transition-colors p-0.5"
                                            aria-label="Clear search"
                                            on:click=move |_| set_catalog_search_input.set(String::new())
                                        >
                                            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                                            </svg>
                                        </button>
                                    </Show>
                                </div>
                            </div>
                            // Catalog tree
                            <div class="flex-1 overflow-auto">
                                <CatalogTree
                                    datasource_slug=ds_slug
                                    search_query=catalog_search_input
                                    refresh_trigger=catalog_refresh_trigger
                                    on_table_click=on_table
                                    on_column_click=on_column
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
                                <div class="relative">
                                    <input
                                        type="text"
                                        placeholder="Search query history..."
                                        prop:value=move || history_search_input.get()
                                        on:input=move |ev| {
                                            set_history_search_input.set(event_target_value(&ev));
                                        }
                                        class="w-full px-3 py-2 pr-8 text-sm border border-border rounded-md focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring bg-background text-foreground"
                                    />
                                    <Show when=move || !history_search_input.get().is_empty()>
                                        <button
                                            class="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground transition-colors p-0.5"
                                            aria-label="Clear search"
                                            on:click=move |_| set_history_search_input.set(String::new())
                                        >
                                            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                                            </svg>
                                        </button>
                                    </Show>
                                </div>
                            </div>
                            // History list
                            <div class="flex-1 min-h-0">
                                <QueryHistory
                                    search_query=history_search
                                    refresh_trigger=history_refresh_trigger
                                    on_query_select=on_query
                                    on_search_change=Callback::new(move |s: String| {
                                        set_history_search_input.set(s);
                                    })
                                />
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        }
    };

    // ── Render ───────────────────────────────────────────────────────────

    view! {
        <Show when=move || is_open.get()>
            {move || {
                if is_mobile.get() {
                    // Mobile: Fixed overlay with backdrop
                    // React offsets: top-[7.5rem] sm:top-[8rem]
                    view! {
                        <div>
                            <div
                                class="fixed top-[7.5rem] sm:top-[8rem] left-0 right-0 bottom-0 bg-[var(--color-overlay)] z-40"
                                on:click=close_sidebar_backdrop
                            />
                            <div class="fixed top-[7.5rem] sm:top-[8rem] right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-card flex flex-col shadow-xl">
                                {panel_content()}
                            </div>
                        </div>
                    }.into_any()
                } else {
                    // Desktop: Inline resizable sidebar
                    let width_style = move || format!("width: {}px", sidebar_width.get());

                    let outer_class = move || {
                        if is_resizing.get() {
                            "border-l border-border bg-card flex h-full overflow-hidden flex-shrink-0 select-none"
                        } else {
                            "border-l border-border bg-card flex h-full overflow-hidden flex-shrink-0"
                        }
                    };

                    view! {
                        <div
                            class=outer_class
                            style=width_style
                        >
                            // Resize handle
                            <div
                                class="flex items-center justify-center cursor-col-resize select-none px-1 -mr-2 relative z-10"
                                on:mousedown=handle_resize_start
                                role="separator"
                                aria-orientation="vertical"
                                aria-label="Drag to resize sidebar"
                                tabindex="0"
                            >
                                <div class="w-1 h-12 bg-border hover:bg-muted-foreground rounded-md transition-colors" />
                            </div>

                            {panel_content()}
                        </div>
                    }.into_any()
                }
            }}
        </Show>
    }
}
