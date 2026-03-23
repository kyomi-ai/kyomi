// SPDX-License-Identifier: AGPL-3.0-or-later

//! Results table — native HTML table with fixed sticky header, resizable
//! columns, scrollable body, row striping, and server-side pagination.
//!
//! Mirrors the React `ResizableTable.jsx` + `ResizableTable.css` +
//! `ResultsTable.jsx` components.
//!
//! ## Column resizing
//! Drag handles between header cells allow the user to resize columns
//! (minimum 50px). On first resize, all column widths are captured from
//! the browser layout and locked so that resizing one column does not
//! cause the others to reflow.
//!
//! ## Pagination
//! Server-side pagination with page size dropdown and prev/next/first/last
//! buttons. Changing page calls `fetch_query_page()` server function.

use std::collections::HashMap;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::types::{ColumnMetadata, QueryResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const _MIN_COL_WIDTH: f64 = 50.0;
const DEFAULT_COL_WIDTH: f64 = 150.0;

const PAGE_SIZE_OPTIONS: [u32; 5] = [10, 25, 50, 100, 200];

// ─────────────────────────────────────────────────────────────────────────────
// Cell rendering helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Render a cell value as a display string.
///
/// - `null` → italic "null"
/// - Numbers → right-aligned (handled by CSS class)
/// - Objects/arrays → JSON-stringified
/// - Strings → as-is, truncated by CSS
fn format_cell(value: &serde_json::Value) -> (String, bool) {
    match value {
        serde_json::Value::Null => ("null".to_string(), true),
        serde_json::Value::String(s) => (s.clone(), false),
        serde_json::Value::Number(n) => (n.to_string(), false),
        serde_json::Value::Bool(b) => (b.to_string(), false),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            (serde_json::to_string(value).unwrap_or_default(), false)
        }
    }
}

/// Check if a value is numeric (for right-alignment).
fn is_numeric(value: &serde_json::Value) -> bool {
    value.is_number()
}

/// Check if a column type suggests numeric data.
fn is_numeric_type(col: &ColumnMetadata) -> bool {
    col.col_type
        .as_deref()
        .map(|t| {
            let t = t.to_lowercase();
            t.contains("int")
                || t.contains("float")
                || t.contains("double")
                || t.contains("decimal")
                || t.contains("numeric")
                || t == "number"
        })
        .unwrap_or(false)
}

// ─────────────────────────────────────────────────────────────────────────────
// ResizableTable component
// ─────────────────────────────────────────────────────────────────────────────

/// Native HTML table with fixed header, resizable columns, scrollable body,
/// row striping, and server-side pagination controls.
///
/// Mirrors `ResizableTable.jsx` + `ResultsTable.jsx`.
#[component]
pub fn ResultsTable(
    /// The query result to display.
    result: QueryResult,
    /// Current page number (1-indexed).
    current_page: u32,
    /// Rows per page.
    page_size: u32,
    /// Whether a page fetch is in progress (shows loading overlay).
    #[prop(default = false)]
    is_paginating: bool,
    /// Called when the user navigates to a different page.
    on_page_change: Callback<u32>,
    /// Called when the user changes the page size.
    on_page_size_change: Callback<u32>,
    // NOTE: header_actions prop removed — chart button is rendered in TabBar instead.
    // ResultsTable focuses on data display only.
) -> impl IntoView {
    // ── Pagination calculations ──────────────────────────────────────────

    let total_rows = result.total_rows.unwrap_or(result.row_count);
    let total_pages = if total_rows == 0 {
        1
    } else {
        (total_rows as f64 / page_size as f64).ceil() as u32
    };

    // Server-side pagination: rows already represent the current page.
    // Display indices use the page offset.
    let display_start = ((current_page - 1) * page_size) as usize;
    let display_end = display_start + result.rows.len();
    let num_columns = result.columns.len();

    // ── Column widths (resizable) ────────────────────────────────────────

    let column_widths = RwSignal::new(HashMap::<usize, f64>::new());
    let user_resized = RwSignal::new(false);

    // ── Resize state ─────────────────────────────────────────────────────
    // Track which column is being resized. Using signals so that the
    // mousedown handler can communicate with the document-level mousemove/mouseup
    // handlers installed via web_sys.

    let resizing_col = RwSignal::new(None::<usize>);
    let resize_start_x = RwSignal::new(0.0_f64);
    let resize_start_width = RwSignal::new(0.0_f64);

    let table_ref = NodeRef::<leptos::html::Table>::new();

    // Style string for a column header/cell when user has resized.
    // Uses reactive `.get()` so Leptos can track dependencies when called
    // inside a reactive closure (e.g. `style=move || col_style(idx)`).
    let col_style = move |idx: usize| -> String {
        if user_resized.get() {
            if let Some(w) = column_widths.get().get(&idx).copied() {
                return format!(
                    "flex: 0 0 {w}px; width: {w}px; min-width: {w}px; max-width: {w}px;"
                );
            }
        }
        "flex: 1 1 0; min-width: 100px;".to_string()
    };

    // ── Mousedown on resize handle ───────────────────────────────────────

    #[allow(unused_variables)]
    let on_resize_start = move |col_idx: usize, ev: web_sys::MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();

        let start_x = ev.client_x() as f64;

        // Get actual column width from the DOM.
        let current_width = table_ref.get().and_then(|table| {
            let el: &web_sys::HtmlElement = &table;
            let ths = el.query_selector_all("thead th").ok()?;
            // +1 because first th is the row-number column
            let th = ths.item((col_idx + 1) as u32)?;
            let rect = th.unchecked_ref::<web_sys::Element>().get_bounding_client_rect();
            Some(rect.width())
        }).unwrap_or(DEFAULT_COL_WIDTH);

        resizing_col.set(Some(col_idx));
        resize_start_x.set(start_x);
        resize_start_width.set(current_width);

        // If this is the first-ever resize, capture all column widths from DOM.
        if !user_resized.get_untracked() {
            if let Some(table) = table_ref.get() {
                let el: &web_sys::HtmlElement = &table;
                if let Ok(ths) = el.query_selector_all("thead th") {
                    let mut widths = HashMap::new();
                    // Skip index 0 (row number column)
                    for i in 1..ths.length() {
                        if let Some(th) = ths.item(i) {
                            let rect = th
                                .unchecked_ref::<web_sys::Element>()
                                .get_bounding_client_rect();
                            widths.insert((i - 1) as usize, rect.width());
                        }
                    }
                    column_widths.set(widths);
                    user_resized.set(true);
                }
            }
        }

        // Install document-level mousemove/mouseup handlers.
        // Both closures are stored in an Rc<RefCell<..>> so the up_handler
        // can drop them after firing, preventing memory leaks.
        #[cfg(feature = "hydrate")]
        {
            use std::cell::RefCell;
            use std::rc::Rc;
            use wasm_bindgen::closure::Closure;
            use wasm_bindgen::JsCast;

            let Some(window) = web_sys::window() else { return };
            let Some(document) = window.document() else { return };

            // Shared storage for both closures — the up_handler takes
            // ownership and drops them when the mouseup fires.
            let closures: Rc<
                RefCell<
                    Option<(
                        Closure<dyn FnMut(web_sys::MouseEvent)>,
                        Closure<dyn FnMut()>,
                    )>,
                >,
            > = Rc::new(RefCell::new(None));

            let move_handler = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(
                move |ev: web_sys::MouseEvent| {
                    let diff = ev.client_x() as f64 - resize_start_x.get_untracked();
                    let new_width = (resize_start_width.get_untracked() + diff).max(MIN_COL_WIDTH);
                    if let Some(idx) = resizing_col.get_untracked() {
                        column_widths.update(|widths| {
                            widths.insert(idx, new_width);
                        });
                    }
                },
            );

            let move_fn: js_sys::Function = move_handler
                .as_ref()
                .unchecked_ref::<js_sys::Function>()
                .clone();
            let document_clone = document.clone();
            let move_fn_clone = move_fn.clone();
            let closures_for_up = Rc::clone(&closures);

            let up_handler = Closure::<dyn FnMut()>::once(move || {
                resizing_col.set(None);
                let _ = document_clone
                    .remove_event_listener_with_callback("mousemove", &move_fn_clone);
                // Remove the mouseup listener itself.
                if let Some((_, ref up_closure)) = *closures_for_up.borrow() {
                    let up_fn: &js_sys::Function = up_closure.as_ref().unchecked_ref();
                    let _ = document_clone
                        .remove_event_listener_with_callback("mouseup", up_fn);
                }
                if let Some(body) = document_clone.body() {
                    let _ = body.style().set_property("cursor", "");
                    let _ = body.style().set_property("user-select", "");
                }
                // Drop both closures, freeing WASM memory.
                closures_for_up.borrow_mut().take();
            });

            // Set cursor + disable selection while resizing
            if let Some(body) = document.body() {
                let _ = body.style().set_property("cursor", "col-resize");
                let _ = body.style().set_property("user-select", "none");
            }

            let _ = document
                .add_event_listener_with_callback("mousemove", move_fn.unchecked_ref());
            let _ = document
                .add_event_listener_with_callback("mouseup", up_handler.as_ref().unchecked_ref());

            // Store closures so they stay alive until the up_handler drops them.
            *closures.borrow_mut() = Some((move_handler, up_handler));
        }
    };

    // ── Render ───────────────────────────────────────────────────────────

    let columns = result.columns.clone();
    let rows = result.rows.clone();
    let columns_for_header = columns.clone();
    let columns_for_body = columns.clone();

    view! {
        <div class="flex-1 min-h-0 h-full flex flex-col rounded-md overflow-hidden relative border border-border bg-card">
            // Header: "Results: Showing X-Y of Z rows x N columns"
            <div class="px-4 py-3 border-b text-sm flex-shrink-0 flex items-center justify-between bg-muted border-border text-muted-foreground">
                <span>
                    "Results: Showing "
                    {display_start + 1}"-"{display_end}" of "
                    <strong>{total_rows}</strong>
                    {if total_rows != 1 { " rows" } else { " row" }}
                    " \u{00d7} "
                    {num_columns}
                    {if num_columns != 1 { " columns" } else { " column" }}
                </span>
                <div class="flex items-center gap-2">
                </div>
            </div>

            // Pagination loading overlay
            {is_paginating.then(|| {
                view! {
                    <div class="absolute inset-0 bg-card/70 z-20 flex items-center justify-center pointer-events-none">
                        <div class="flex items-center gap-3 bg-card px-4 py-3 rounded-lg shadow-lg border border-border pointer-events-auto">
                            <crate::components::Spinner class="text-primary" />
                            <span class="text-xs text-muted-foreground">"Loading..."</span>
                        </div>
                    </div>
                }
            })}

            // Scrollable table area
            <div class="flex-1 overflow-auto relative text-xs">
                <table
                    node_ref=table_ref
                    class="border-collapse"
                    style="table-layout: fixed; width: 100%; display: table;"
                >
                    <thead class="sticky top-0 z-10">
                        <tr class="resizable-table-row" style="display: flex; width: 100%;">
                            // Row number header
                            <th class="resizable-table-row-number-header bg-accent border-b border-r-2 border-input text-center font-normal text-muted-foreground"
                                style="flex: 0 0 50px; width: 50px; min-width: 50px; max-width: 50px; padding: 6px 4px; position: sticky; top: 0; font-size: 0.75rem;"
                            />
                            {columns_for_header
                                .iter()
                                .enumerate()
                                .map(|(idx, col)| {
                                    let col_name = col.name.clone();
                                    view! {
                                        <th
                                            class="resizable-table-header text-left relative transition-colors bg-accent border-b border-border font-semibold text-foreground"
                                            style=move || format!("{} padding: 6px 8px; position: sticky; top: 0; overflow: hidden;", col_style(idx))
                                        >
                                            <div class="truncate pr-2 flex items-center">
                                                {col_name}
                                            </div>
                                            // Resize handle
                                            <div
                                                class="resize-handle resizable-table-resize-handle absolute right-0 top-0 bottom-0 cursor-col-resize"
                                                style="user-select: none; width: 12px; display: flex; align-items: center; justify-content: center;"
                                                role="separator"
                                                aria-orientation="vertical"
                                                tabindex="0"
                                                on:mousedown={
                                                    let on_resize_start = on_resize_start.clone();
                                                    move |ev: web_sys::MouseEvent| {
                                                        on_resize_start(idx, ev);
                                                    }
                                                }
                                            >
                                                <div
                                                    class="resizable-table-resize-handle-inner bg-border hover:bg-muted-foreground"
                                                    style="transition: background-color 0.15s; width: 2px; height: 40%;"
                                                />
                                            </div>
                                        </th>
                                    }
                                })
                                .collect_view()}
                        </tr>
                    </thead>
                    <tbody>
                        {rows
                            .iter()
                            .enumerate()
                            .map(|(row_idx, row)| {
                                // Calculate actual row number with page offset
                                let actual_row_num = display_start + row_idx + 1;
                                let row_bg = if row_idx % 2 == 1 { "bg-muted" } else { "bg-card" };

                                view! {
                                    <tr
                                        class=format!("resizable-table-row {row_bg}")
                                        style="display: flex; width: 100%;"
                                    >
                                        // Row number cell
                                        <td
                                            class="resizable-table-row-number-cell border-b border-r-2 border-border text-center text-muted-foreground select-none"
                                            style="flex: 0 0 50px; width: 50px; min-width: 50px; max-width: 50px; padding: 4px 4px; font-size: 0.75rem; font-family: monospace;"
                                        >
                                            {actual_row_num}
                                        </td>
                                        {row
                                            .iter()
                                            .enumerate()
                                            .map(|(cell_idx, cell)| {
                                                let (display_value, is_null) = format_cell(cell);
                                                let align_right = is_numeric(cell)
                                                    || columns_for_body
                                                        .get(cell_idx)
                                                        .map(is_numeric_type)
                                                        .unwrap_or(false);
                                                let text_align = if align_right {
                                                    "text-align: right;"
                                                } else {
                                                    ""
                                                };

                                                view! {
                                                    <td
                                                        class="resizable-table-cell border-b border-accent text-foreground"
                                                        style=move || format!(
                                                            "{} padding: 4px 8px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; {text_align}", col_style(cell_idx)
                                                        )
                                                    >
                                                        {if is_null {
                                                            view! { <span class="italic text-muted-foreground">"null"</span> }
                                                                .into_any()
                                                        } else {
                                                            view! { <span>{display_value}</span> }.into_any()
                                                        }}
                                                    </td>
                                                }
                                            })
                                            .collect_view()}
                                    </tr>
                                }
                            })
                            .collect_view()}
                    </tbody>
                </table>
            </div>

            // Pagination toolbar
            <PaginationControls
                current_page=current_page
                total_pages=total_pages
                total_rows=total_rows
                page_size=page_size
                display_start=display_start
                display_end=display_end
                on_page_change=on_page_change
                on_page_size_change=on_page_size_change
            />
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pagination controls
// ─────────────────────────────────────────────────────────────────────────────

/// Pagination toolbar at the bottom of the results table.
///
/// Shows rows-per-page selector, "X-Y of Z" text, and first/prev/next/last
/// page buttons. Mirrors the pagination section of `ResizableTable.jsx`.
#[component]
fn PaginationControls(
    current_page: u32,
    total_pages: u32,
    total_rows: usize,
    page_size: u32,
    display_start: usize,
    display_end: usize,
    on_page_change: Callback<u32>,
    on_page_size_change: Callback<u32>,
) -> impl IntoView {
    let is_first = current_page <= 1;
    let is_last = current_page >= total_pages;

    view! {
        <div class="px-2 sm:px-4 py-2 border-t flex items-center justify-between flex-shrink-0 min-w-0 bg-muted border-border" role="navigation" aria-label="Results pagination">
            // Page size selector
            <div class="flex items-center gap-1 sm:gap-2 text-xs whitespace-nowrap text-muted-foreground">
                <span class="hidden sm:inline">"Rows per page:"</span>
                <select
                    class="h-7 text-xs rounded-md border border-input bg-transparent px-2 py-1 text-foreground cursor-pointer focus:outline-none focus:ring-1 focus:ring-ring"
                    aria-label="Rows per page"
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        if let Ok(size) = value.parse::<u32>() {
                            on_page_size_change.run(size);
                        }
                    }
                >
                    {PAGE_SIZE_OPTIONS
                        .iter()
                        .map(|&size| {
                            let selected = size == page_size;
                            view! {
                                <option value=size.to_string() selected=selected>
                                    {size}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </div>

            // Page info + navigation
            <div class="flex items-center gap-2 sm:gap-4 min-w-0">
                // Row range display
                <div class="text-xs whitespace-nowrap overflow-hidden text-ellipsis text-muted-foreground">
                    <span class="hidden sm:inline">
                        {display_start + 1}"-"{display_end}" of "{total_rows}
                    </span>
                    <span class="sm:hidden">
                        {display_start + 1}"-"{display_end}
                    </span>
                </div>

                // Navigation buttons
                <div class="flex items-center gap-1 flex-shrink-0">
                    // First page
                    <button
                        class="p-1 rounded text-muted-foreground hover:bg-foreground/10 disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-transparent"
                        disabled=is_first
                        aria-label="First page"
                        on:click=move |_| on_page_change.run(1)
                    >
                        <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11 19l-7-7 7-7m8 14l-7-7 7-7" />
                        </svg>
                    </button>

                    // Previous page
                    <button
                        class="p-1 rounded text-muted-foreground hover:bg-foreground/10 disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-transparent"
                        disabled=is_first
                        aria-label="Previous page"
                        on:click=move |_| {
                            on_page_change.run(current_page.saturating_sub(1).max(1))
                        }
                    >
                        <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7" />
                        </svg>
                    </button>

                    // Page X of Y
                    <span class="text-xs px-2 text-muted-foreground">
                        "Page "{current_page}" of "{total_pages}
                    </span>

                    // Next page
                    <button
                        class="p-1 rounded text-muted-foreground hover:bg-foreground/10 disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-transparent"
                        disabled=is_last
                        aria-label="Next page"
                        on:click=move |_| {
                            on_page_change.run((current_page + 1).min(total_pages))
                        }
                    >
                        <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
                        </svg>
                    </button>

                    // Last page
                    <button
                        class="p-1 rounded text-muted-foreground hover:bg-foreground/10 disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:bg-transparent"
                        disabled=is_last
                        aria-label="Last page"
                        on:click=move |_| on_page_change.run(total_pages)
                    >
                        <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 5l7 7-7 7M5 5l7 7-7 7" />
                        </svg>
                    </button>
                </div>
            </div>
        </div>
    }
}
