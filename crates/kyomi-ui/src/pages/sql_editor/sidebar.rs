// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL Editor right sidebar — tabbed panel with catalog tree and query history.
//!
//! Matches the React `SQLEditor.jsx` sidebar section:
//! - Two tabs: "Catalog" and "History"
//! - Resizable width via drag handle (pixel-based, clamped 280-480px)
//! - Toggle button to show/hide
//! - On mobile (<768px): slide-in overlay with backdrop

use leptos::prelude::*;
use phosphor_leptos::Icon;
#[cfg(feature = "hydrate")]
use wasm_bindgen::prelude::*;

use super::catalog_tree::CatalogTree;
use super::query_history::QueryHistory;
use super::state::SqlEditorState;
use super::types::SidebarTab;
use crate::components::dashboard::shared::use_is_mobile;
use crate::components::{Button, ButtonSize, ButtonVariant, SearchInput};

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
    /// Callback when user clicks the info button on a table in the catalog.
    on_table_info: Callback<String>,
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
    // History refresh tick lives on the shared SqlEditorState so
    // `execution::save_to_history` can bump it after a query runs and the
    // QueryHistory panel will refetch automatically.
    let history_refresh_trigger =
        Signal::derive(move || state.history_refresh_tick.get());

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

    // ── Tab button class helper (segmented control pattern from watches_page.rs) ──
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
                    <div class="flex items-center rounded-lg bg-muted p-1" role="tablist" aria-label="Sidebar panels">
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
                        // Close button
                        <Button
                            variant=ButtonVariant::GhostMuted
                            size=ButtonSize::IconSm
                            aria_label="Close sidebar"
                            on:click=close_sidebar
                        >
                            <Icon icon=phosphor_leptos::X size="16px" />
                        </Button>
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
                                <SearchInput
                                    value=Signal::derive(move || catalog_search_input.get())
                                    on_input=Callback::new(move |val: String| set_catalog_search_input.set(val))
                                    placeholder="Search tables..."
                                />
                            </div>
                            // Catalog tree
                            <div class="flex-1 overflow-auto">
                                <CatalogTree
                                    datasource_slug=ds_slug
                                    search_query=catalog_search_input
                                    refresh_trigger=catalog_refresh_trigger
                                    on_table_click=on_table
                                    on_column_click=on_column
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
    //
    // The sidebar is always in the DOM to enable CSS transitions (matching
    // the left nav sidebar pattern). When closed, desktop width → 0 and
    // mobile panel slides off-screen via transform.

    view! {
        // ── Desktop sidebar (always mounted, width animates) ────────────
        <div
            class=move || {
                let base = "h-full overflow-hidden flex-shrink-0 transition-[width] duration-300 ease-in-out";
                if is_resizing.get() {
                    format!("{base} select-none")
                } else {
                    base.to_string()
                }
            }
            style=move || {
                let open = is_open.get();
                let mobile = is_mobile.get();
                if mobile {
                    // Hide desktop sidebar on mobile
                    "width: 0px; display: none".to_string()
                } else if open {
                    format!("width: {}px", sidebar_width.get())
                } else {
                    "width: 0px".to_string()
                }
            }
        >
            // Inner container with fixed min-width prevents content from
            // collapsing during the width transition. Only mounted on desktop
            // to avoid double-rendering panel_content on mobile.
            <Show when=move || !is_mobile.get()>
                <div
                    class="flex h-full border-l border-t border-border bg-muted"
                    style=move || format!("min-width: {}px", DEFAULT_WIDTH)
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
                        <div class="w-1 h-12 bg-border hover:bg-muted-foreground/50 rounded-md transition-colors" />
                    </div>

                    {panel_content()}
                </div>
            </Show>
        </div>

        // ── Mobile overlay (always mounted, slides in/out) ──────────────
        <Show when=move || is_mobile.get()>
            // Backdrop — fade in/out
            <div
                class="fixed top-[7.5rem] sm:top-[8rem] left-0 right-0 bottom-0 z-40 transition-opacity duration-300 ease-in-out"
                class:opacity-0=move || !is_open.get()
                class:pointer-events-none=move || !is_open.get()
                style="background: var(--color-overlay)"
                on:click=close_sidebar_backdrop
            />
            // Panel — slide from right
            <div
                class="fixed top-[7.5rem] sm:top-[8rem] right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-muted flex flex-col shadow-xl transition-transform duration-300 ease-in-out"
                style=move || {
                    if is_open.get() {
                        "transform: translateX(0)"
                    } else {
                        "transform: translateX(100%)"
                    }
                }
            >
                {panel_content()}
            </div>
        </Show>
    }
}
