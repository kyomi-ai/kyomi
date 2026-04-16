// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQL Editor page — types, state management, and UI components.

pub mod catalog_tree;
pub mod code_editor;
pub mod datasource_selector;
pub mod execution;
pub mod query_history;
pub mod results_container;
pub mod results_table;
pub mod sidebar;
pub mod state;
pub mod status_bar;
pub mod tab_bar;
pub mod table_info_modal;
pub mod types;

// Re-export the most commonly used items for convenience.
pub use catalog_tree::CatalogTree;
pub use code_editor::SqlCodeEditor;
pub use datasource_selector::{DatasourceSelection, DatasourceSelector};
pub use execution::run_query;
pub use query_history::QueryHistory;
pub use results_container::ResultsContainer;
pub use results_table::ResultsTable;
pub use sidebar::SqlEditorSidebar;
pub use state::SqlEditorState;
pub use status_bar::{DryRunStatus, StatusBar};
pub use tab_bar::TabBar;
pub use types::{
    CatalogNode, CatalogNodeType, ColumnMetadata, ColumnSort, NewTabData, QueryError, QueryHandle,
    QueryHistoryEntry, QueryResult, QueryStatus, ResultTab, SidebarTab, SortDirection,
    TableUIState, Visualization,
};

// ─── Page-level component ────────────────────────────────────────────────────

use leptos::prelude::*;
use phosphor_leptos::Icon;
#[cfg(target_arch = "wasm32")]
use kode_leptos::EditorHandle;

use crate::components::{ButtonSize, ButtonVariant, ToggleButton};

/// SQL Editor page — full-page component that assembles all sub-components.
///
/// React reference: `apps/frontend/src/pages/SQLEditorPage.jsx` (160 lines) +
/// `apps/frontend/src/components/SQLEditor.jsx` (layout / resize).
///
/// Layout:
/// ```text
/// ┌──────────────────────────────────────────────────────────┐
/// │  Header: Page title + Datasource Selector + Run Button   │
/// ├──────────────────────────────┬───────────────────────────┤
/// │  SQL Code Editor             │  Sidebar (Catalog/History)│
/// │  (kode-leptos)               │                           │
/// │──────── Status Bar ──────────│                           │
/// │  Results Panel               │                           │
/// │  ┌─────────────────────────┐ │                           │
/// │  │ Tab1 | Tab2 | Tab3      │ │                           │
/// │  ├─────────────────────────┤ │                           │
/// │  │ Results Table / Error   │ │                           │
/// │  │ / Loading / Empty       │ │                           │
/// │  │ Pagination              │ │                           │
/// │  └─────────────────────────┘ │                           │
/// └──────────────────────────────┴───────────────────────────┘
/// ```
#[component]
pub fn SqlEditorPage() -> impl IntoView {
    // ── Provide state contexts ───────────────────────────────────────────
    let state = SqlEditorState::provide();
    let ds_selection = DatasourceSelection::provide();

    // ── Editor handle (provided by CodeEditor via on_ready) ──────────────
    // Shared via context so both the page (sidebar click handlers) and the
    // SqlCodeEditor (dry run markers, selection) can access it.
    #[cfg(target_arch = "wasm32")]
    let editor_handle: RwSignal<Option<EditorHandle>> = RwSignal::new(None);
    #[cfg(target_arch = "wasm32")]
    provide_context(editor_handle);

    // ── Query running signal ─────────────────────────────────────────────
    let (query_running, set_query_running) = signal(false);

    // ── Editor ↔ Results vertical split (percentage-based) ───────────────
    let (editor_pct, _set_editor_pct) = signal(50.0_f64);
    let (is_resizing, _set_is_resizing) = signal(false);
    // Re-bind under wasm32 so the resize handler can use the write signals.
    #[cfg(target_arch = "wasm32")]
    let set_editor_pct = _set_editor_pct;
    #[cfg(target_arch = "wasm32")]
    let set_is_resizing = _set_is_resizing;

    // Track whether there are tabs (controls results panel visibility).
    let has_tabs = Memo::new(move |_| !state.tabs.get().is_empty());

    // ── Sidebar toggle ───────────────────────────────────────────────────
    // The sidebar open/close state is stored in SqlEditorState.active_right_tab.
    let sidebar_open = Memo::new(move |_| state.active_right_tab.get().is_some());

    let toggle_sidebar = move |_| {
        if state.active_right_tab.get_untracked().is_some() {
            state.set_active_right_tab(None);
        } else {
            state.set_active_right_tab(Some(SidebarTab::Catalog));
        }
    };

    // ── Callbacks for sidebar → editor insertion ─────────────────────────
    // Insert at cursor position via EditorHandle when available, otherwise
    // fall back to appending to the query text.
    let on_table_click = Callback::new(move |table_id: String| {
        let ds_type = ds_selection.datasource_type.get_untracked();
        let formatted = if ds_type.as_deref() == Some("bigquery") {
            format!("`{table_id}`")
        } else {
            table_id
        };

        #[cfg(target_arch = "wasm32")]
        if let Some(handle) = editor_handle.get_untracked() {
            handle.insert_at_cursor(&formatted);
            return;
        }

        // SSR / handle not yet ready — append to end.
        let current = state.query_text.get_untracked();
        let separator = if current.is_empty() { "" } else { " " };
        state.set_query_text(format!("{current}{separator}{formatted}"));
    });

    let on_column_click = Callback::new(move |col_name: String| {
        let text = format!("{col_name},\n");

        #[cfg(target_arch = "wasm32")]
        if let Some(handle) = editor_handle.get_untracked() {
            handle.insert_at_cursor(&text);
            return;
        }

        // SSR / handle not yet ready — append to end.
        let current = state.query_text.get_untracked();
        state.set_query_text(format!("{current}{text}"));
    });

    let on_query_select = Callback::new(move |(query_text, datasource_slug): (String, Option<String>)| {
        state.set_query_text(query_text);
        if let Some(slug) = datasource_slug {
            // Restore the datasource associated with the history entry.
            // The DatasourceSelector will auto-resolve the type from the slug.
            ds_selection.slug.set(Some(slug));
        }
    });

    // ── Table info modal state ──────────────────────────────────────────
    let (show_table_info, set_show_table_info) = signal(false);
    let (selected_table_id, set_selected_table_id) = signal(Option::<String>::None);

    let on_table_info = Callback::new(move |table_id: String| {
        set_selected_table_id.set(Some(table_id));
        set_show_table_info.set(true);
    });

    let close_table_info = Callback::new(move |()| {
        set_show_table_info.set(false);
    });

    // ── Restore query from results tab (double-click) ────────────────────
    let on_restore_query = Callback::new(move |(query_text, datasource_slug): (String, Option<String>)| {
        state.set_query_text(query_text);
        if let Some(slug) = datasource_slug {
            ds_selection.slug.set(Some(slug));
        }
    });

    // ── Re-run query from error tab ──────────────────────────────────────
    let on_rerun_query = Callback::new(move |sql: String| {
        let slug = ds_selection.slug.get_untracked().unwrap_or_default();
        let ds_type = ds_selection.datasource_type.get_untracked().unwrap_or_default();
        run_query(state, sql, slug, ds_type, set_query_running);
    });

    // ── Vertical resize (editor ↔ results) ──────────────────────────────
    // We set up the mouse-move / mouse-up listeners in a WASM-only effect
    // that activates when `is_resizing` becomes true.
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::JsCast;

        let resize_start_y: StoredValue<f64> = StoredValue::new(0.0);
        let resize_start_pct: StoredValue<f64> = StoredValue::new(50.0);
        let container_height: StoredValue<f64> = StoredValue::new(600.0);

        // Provide the start-resize callback via context for the drag handle.
        provide_context(ResizeStartCallback(Callback::new(move |(y, cont_h): (f64, f64)| {
            resize_start_y.set_value(y);
            resize_start_pct.set_value(editor_pct.get_untracked());
            container_height.set_value(cont_h);
            set_is_resizing.set(true);
        })));

        // Global mousemove + mouseup listeners when resizing.
        Effect::new(move |prev: Option<Option<(Closure<dyn FnMut(web_sys::MouseEvent)>, Closure<dyn FnMut(web_sys::MouseEvent)>)>>| {
            // Clean up previous listeners.
            if let Some(Some((ref move_cb, ref up_cb))) = prev {
                if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                    let _ = doc.remove_event_listener_with_callback(
                        "mousemove",
                        move_cb.as_ref().unchecked_ref(),
                    );
                    let _ = doc.remove_event_listener_with_callback(
                        "mouseup",
                        up_cb.as_ref().unchecked_ref(),
                    );
                }
                if let Some(body) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.body()) {
                    let _ = body.style().remove_property("cursor");
                    let _ = body.style().remove_property("user-select");
                }
            }

            if !is_resizing.get() {
                return None;
            }

            let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
                return None;
            };

            // Set cursor.
            if let Some(body) = doc.body() {
                let _ = body.style().set_property("cursor", "row-resize");
                let _ = body.style().set_property("user-select", "none");
            }

            let on_move = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |e: web_sys::MouseEvent| {
                let y = e.client_y() as f64;
                let start_y = resize_start_y.get_value();
                let start_pct = resize_start_pct.get_value();
                let h = container_height.get_value();
                if h > 0.0 {
                    let diff_pct = ((y - start_y) / h) * 100.0;
                    let new_pct = (start_pct + diff_pct).clamp(20.0, 80.0);
                    set_editor_pct.set(new_pct);
                }
            });

            let on_up = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |_: web_sys::MouseEvent| {
                set_is_resizing.set(false);
            });

            let _ = doc.add_event_listener_with_callback(
                "mousemove",
                on_move.as_ref().unchecked_ref(),
            );
            let _ = doc.add_event_listener_with_callback(
                "mouseup",
                on_up.as_ref().unchecked_ref(),
            );

            Some((on_move, on_up))
        });
    }

    // ── Keyboard shortcut: Cmd/Ctrl+Enter to run query ───────────────────
    // The SqlCodeEditor already handles Cmd+Enter via its `on_run` prop.
    // We wire it to the same handler.
    let on_editor_run = Callback::new(move |()| {
        let sql = state.query_text.get_untracked();
        let slug = ds_selection.slug.get_untracked().unwrap_or_default();
        let ds_type = ds_selection.datasource_type.get_untracked().unwrap_or_default();
        run_query(state, sql, slug, ds_type, set_query_running);
    });

    // ── Page-level keyboard shortcuts ───────────────────────────────────
    // - Cmd/Ctrl+S: prevent browser save dialog
    // - Escape: close sidebar if open
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::JsCast;

        let keydown_handler = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            let key = ev.key();
            let meta_or_ctrl = ev.meta_key() || ev.ctrl_key();

            // Cmd/Ctrl+S — prevent browser "Save As" dialog.
            if meta_or_ctrl && key == "s" {
                ev.prevent_default();
            }

            // Escape — close sidebar if open.
            if key == "Escape" && state.active_right_tab.get_untracked().is_some() {
                state.set_active_right_tab(None);
            }
        });

        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            let _ = doc.add_event_listener_with_callback(
                "keydown",
                keydown_handler.as_ref().unchecked_ref(),
            );
        }

        // Store the closure in an Owner-scoped cleanup via on_cleanup.
        // Wrap in SendWrapper because Closure is !Send but on_cleanup requires Send+Sync.
        let keydown_handler = send_wrapper::SendWrapper::new(keydown_handler);
        on_cleanup(move || {
            let handler = keydown_handler.take();
            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                let _ = doc.remove_event_listener_with_callback(
                    "keydown",
                    handler.as_ref().unchecked_ref(),
                );
            }
            drop(handler);
        });
    }

    // Datasource slug as a Signal for passing to child components.
    let ds_slug_signal: Signal<Option<String>> = Signal::derive(move || ds_selection.slug.get());
    // Track whether a datasource is selected (for empty state messaging).
    let has_datasource = Memo::new(move |_| ds_selection.slug.get().is_some());

    view! {
        <div class="flex flex-col h-full bg-background @container">
            // ── Header ───────────────────────────────────────────────────
            // Responsive via container queries (DESIGN.md §Responsive Toolbar
            // Pattern) — the outer `@container` wraps the content area, so
            // breakpoints fire at content width rather than viewport width.
            // Narrow: stacks vertically — title row + selector/catalog row.
            // Wide (@3xl+): single h-16 row with title + selector on the left,
            // catalog button on the right with its full label.
            <div class="page-header px-4 @3xl:px-6 @3xl:h-16 py-3 @3xl:py-0 flex-shrink-0 flex flex-col @3xl:flex-row @3xl:items-center @3xl:justify-between gap-3 @3xl:gap-0">
                // Left cluster: title + datasource selector (single h1
                // landmark, truncates rather than duplicates if the cluster
                // gets tight).
                <div class="flex items-center gap-3 @3xl:gap-4 min-w-0">
                    <h1 class="text-3xl font-display text-foreground shrink-0 truncate">
                        "SQL Editor"
                    </h1>
                    <div class="flex-1 @3xl:flex-initial min-w-0">
                        <DatasourceSelector/>
                    </div>
                </div>

                // Catalog toggle — label hidden at narrow widths so the
                // button stays compact, shown again at @3xl+ widths.
                <div class="flex items-center gap-2 self-start @3xl:self-auto">
                    <ToggleButton
                        variant=Signal::derive(move || if sidebar_open.get() { ButtonVariant::Active } else { ButtonVariant::Secondary })
                        size=ButtonSize::Sm
                        aria_label="Toggle catalog sidebar"
                        on:click=toggle_sidebar
                    >
                        <Icon icon=phosphor_leptos::DATABASE size="16px" />
                        <span class="hidden @3xl:inline whitespace-nowrap">"Catalog"</span>
                    </ToggleButton>
                </div>
            </div>

            // ── "No datasource" banner ────────────────────────────────────
            // Prose-style wrapping: a single <p> with an inline icon + link.
            // On narrow viewports the text wraps naturally instead of
            // producing an awkward multi-column flex row.
            <Show when=move || !has_datasource.get()>
                <p class="px-4 @3xl:px-6 py-2 bg-warning/10 border-b border-warning/30 text-sm text-warning-foreground flex-shrink-0">
                    <Icon icon=phosphor_leptos::WARNING attr:class="inline-block align-text-bottom mr-1.5 flex-shrink-0" size="16px" />
                    "No datasource selected. "
                    <a href="/settings" class="text-primary hover:underline font-medium">"Connect a datasource in Settings"</a>
                    " to start querying."
                </p>
            </Show>

            // ── Content area: editor + results (left) | sidebar (right) ──
            <div class="flex flex-1 min-h-0 relative">
                // Left column: editor + results, stacked vertically.
                <div class="flex flex-col flex-1 min-w-0 overflow-hidden">
                    // Editor section — height controlled by split percentage.
                    <div
                        class=move || {
                            let base = "flex flex-col min-w-0";
                            if !is_resizing.get() {
                                format!("{base} transition-all duration-300 ease-in-out")
                            } else {
                                base.to_string()
                            }
                        }
                        style:height=move || {
                            if has_tabs.get() {
                                format!("{}%", editor_pct.get())
                            } else {
                                "100%".to_string()
                            }
                        }
                        style:min-height="150px"
                        style:flex-shrink="0"
                    >
                        <div class="relative border-l border-r border-b border-border rounded-b-md overflow-hidden flex flex-col flex-1 min-h-0">
                            <SqlCodeEditor
                                content=state.query_text
                                on_run=on_editor_run
                                datasource_slug=ds_slug_signal
                                query_running=Signal::derive(move || query_running.get())
                                run_disabled=Signal::derive(move || !has_datasource.get())
                                on_run_query=Callback::new(move |()| {
                                    let sql = state.query_text.get_untracked();
                                    let slug = ds_selection.slug.get_untracked().unwrap_or_default();
                                    let ds_type = ds_selection.datasource_type.get_untracked().unwrap_or_default();
                                    run_query(state, sql, slug, ds_type, set_query_running);
                                })
                            />
                        </div>
                    </div>

                    // Resize handle — visible only when there are tabs.
                    <Show when=move || has_tabs.get()>
                        <ResizeHandle/>
                    </Show>

                    // Results panel — visible only when there are tabs.
                    <Show when=move || has_tabs.get()>
                        <div
                            class=move || {
                                let base = "flex-1 min-h-0 flex flex-col overflow-hidden";
                                if !is_resizing.get() {
                                    format!("{base} transition-all duration-300 ease-in-out")
                                } else {
                                    base.to_string()
                                }
                            }
                        >
                            <ResultsContainer
                                on_restore_query=on_restore_query
                                on_run_query=on_rerun_query
                            />
                        </div>
                    </Show>
                </div>

                // Right sidebar (catalog / history).
                <SqlEditorSidebar
                    datasource_slug=ds_slug_signal
                    on_table_click=on_table_click
                    on_column_click=on_column_click
                    on_query_select=on_query_select
                    on_table_info=on_table_info
                />
            </div>

            // ── Table info modal ─────────────────────────────────────────
            <table_info_modal::TableInfoModal
                show=show_table_info
                on_close=close_table_info
                table_id=Signal::derive(move || selected_table_id.get())
                datasource_slug=ds_slug_signal
            />
        </div>
    }
}

// ─── Resize handle sub-component ─────────────────────────────────────────────

/// Context wrapper for the vertical resize start callback.
///
/// On mousedown, the drag handle calls this with `(clientY, containerHeight)`
/// to begin the resize.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy)]
struct ResizeStartCallback(Callback<(f64, f64)>);

/// Horizontal drag handle between the editor and results panels.
///
/// On mousedown, starts the vertical resize gesture by calling the
/// `ResizeStartCallback` context.
#[component]
fn ResizeHandle() -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    let on_mousedown = {
        use wasm_bindgen::JsCast;
        move |ev: web_sys::MouseEvent| {
            ev.prevent_default();
            let y = ev.client_y() as f64;
            // Walk up to find the container height (the content area).
            let container_height = ev
                .current_target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
                .and_then(|el| el.parent_element())
                .map(|parent| parent.get_bounding_client_rect().height())
                .unwrap_or(600.0);

            if let Some(ResizeStartCallback(cb)) = use_context::<ResizeStartCallback>() {
                cb.run((y, container_height));
            }
        }
    };

    #[cfg(not(target_arch = "wasm32"))]
    let on_mousedown = move |_ev: leptos::ev::MouseEvent| {};

    view! {
        <div
            class="flex items-center justify-center cursor-row-resize select-none py-1 -my-2 relative z-10"
            on:mousedown=on_mousedown
            role="separator"
            aria-orientation="horizontal"
            aria-label="Drag to resize editor and results panels"
            tabindex="0"
        >
            <div class="h-1 w-12 bg-border hover:bg-muted-foreground rounded-md transition-colors"/>
        </div>
    }
}
