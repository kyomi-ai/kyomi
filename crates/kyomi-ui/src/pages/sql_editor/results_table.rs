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
//! buttons. Changing page calls `fetch_arrow_buffered()`.
//!
//! ## Data sources
//! The component accepts a `QueryResult` which may carry data in two forms:
//! - **Arrow path**: `result.data` is `Some(DataTable)` — rendered via
//!   `DataTable::get_string` and numeric detection from the Arrow schema.
//! - **JSON path** (legacy): `result.data` is `None`, rows come from
//!   `result.rows` (Vec<Vec<Value>>).  Used by `chart_builder.rs` which
//!   still calls the old server function.

use std::collections::HashMap;

use leptos::prelude::*;
use phosphor_leptos::Icon;
use wasm_bindgen::JsCast;

use super::types::QueryResult;
use crate::components::{Button, ButtonSize, ButtonVariant, StyledSelect};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "hydrate")]
const MIN_COL_WIDTH: f64 = 50.0;
const DEFAULT_COL_WIDTH: f64 = 150.0;

/// Page size options as static string pairs for `StyledSelect`.
const PAGE_SIZE_OPTIONS: [(&str, &str); 5] = [
    ("10", "10"), ("25", "25"), ("50", "50"), ("100", "100"), ("200", "200"),
];

// ─────────────────────────────────────────────────────────────────────────────
// Cell rendering helpers — Arrow path
// ─────────────────────────────────────────────────────────────────────────────

/// Detect whether a `DataTable` column is numeric by probing the first
/// non-null value with `get_f64`.
///
/// `DataTable::get_f64` returns `Some(_)` for all Arrow numeric types
/// (int, uint, float, decimal) and `None` for strings, booleans, etc.
/// This avoids a direct dependency on `arrow-schema::DataType`.
pub(super) fn is_datatable_column_numeric(
    data: &chartml_core::data::DataTable,
    col_name: &str,
) -> bool {
    for row_idx in 0..data.num_rows() {
        // Skip null values — get_f64 returns None for both null AND
        // non-numeric types; get_string would also return None for null.
        // We need to distinguish: try get_string first, then get_f64.
        if data.get_string(row_idx, col_name).is_none() {
            continue; // null — skip
        }
        // Non-null value: numeric if get_f64 succeeds.
        return data.get_f64(row_idx, col_name).is_some();
    }
    false // all rows null or column empty — default to non-numeric
}

// ─────────────────────────────────────────────────────────────────────────────
// Cell rendering helpers — JSON path (legacy)
// ─────────────────────────────────────────────────────────────────────────────

/// Render a JSON cell value as a display string.
///
/// - `null` → italic "NULL" (matched by the template's null branch)
/// - Numbers → right-aligned (handled by CSS class)
/// - Objects/arrays → JSON-stringified
/// - Strings → as-is, truncated by CSS
fn format_json_cell(value: &serde_json::Value) -> (String, bool) {
    match value {
        serde_json::Value::Null => ("NULL".to_string(), true),
        serde_json::Value::String(s) => (s.clone(), false),
        serde_json::Value::Number(n) => (n.to_string(), false),
        serde_json::Value::Bool(b) => (b.to_string(), false),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            (serde_json::to_string(value).unwrap_or_default(), false)
        }
    }
}

/// Check if a JSON value is numeric (for right-alignment in the JSON path).
fn is_json_numeric(value: &serde_json::Value) -> bool {
    value.is_number()
}

// ─────────────────────────────────────────────────────────────────────────────
// Unified row/column view types
// ─────────────────────────────────────────────────────────────────────────────

/// A single cell value ready for rendering.
struct CellValue {
    display: String,
    is_null: bool,
    align_right: bool,
}

/// A fully materialized row for rendering.
type RenderedRow = Vec<CellValue>;

/// Materialize all rows from a `QueryResult` into display-ready form.
///
/// Prefers the Arrow `data` field when present; falls back to the JSON `rows`
/// field for the legacy path.
fn materialize_rows(result: &QueryResult) -> (Vec<String>, Vec<RenderedRow>) {
    if let Some(ref data) = result.data {
        // ── Arrow path ────────────────────────────────────────────────────
        let col_names = data.field_names();

        // Precompute numeric flags per column by probing the first non-null
        // value (avoids a direct arrow-schema dependency).
        let numeric_flags: Vec<bool> = col_names
            .iter()
            .map(|name| is_datatable_column_numeric(data, name))
            .collect();

        let rows = (0..data.num_rows())
            .map(|row_idx| {
                col_names
                    .iter()
                    .zip(numeric_flags.iter())
                    .map(|(col_name, &is_numeric)| {
                        match data.get_string(row_idx, col_name) {
                            None => CellValue {
                                display: "NULL".to_string(),
                                is_null: true,
                                align_right: false,
                            },
                            Some(s) => CellValue {
                                display: s,
                                is_null: false,
                                align_right: is_numeric,
                            },
                        }
                    })
                    .collect()
            })
            .collect();

        (col_names, rows)
    } else {
        // ── JSON path (legacy) ────────────────────────────────────────────
        let col_names: Vec<String> = result.columns.iter().map(|c| c.name.clone()).collect();

        // Build a per-column "is numeric type" flag from ColumnMetadata.
        let numeric_type_flags: Vec<bool> = result
            .columns
            .iter()
            .map(|col| {
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
            })
            .collect();

        let rows = result
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(cell_idx, cell)| {
                        let (display, is_null) = format_json_cell(cell);
                        let align_right = is_json_numeric(cell)
                            || numeric_type_flags.get(cell_idx).copied().unwrap_or(false);
                        CellValue {
                            display,
                            is_null,
                            align_right,
                        }
                    })
                    .collect()
            })
            .collect();

        (col_names, rows)
    }
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
    // ── Materialize rows and column names ────────────────────────────────
    let (col_names, rendered_rows) = materialize_rows(&result);
    let num_columns = col_names.len();
    let num_rows_this_page = rendered_rows.len();

    // ── Pagination calculations ──────────────────────────────────────────

    let has_known_total = result.total_rows.is_some();
    let total_rows = result.total_rows.unwrap_or(if result.has_more {
        usize::MAX
    } else {
        result.row_count
    });
    let page_size = page_size.max(1);
    let total_pages = if has_known_total {
        ((total_rows as f64) / (page_size as f64)).ceil().max(1.0) as u32
    } else if result.has_more {
        u32::MAX
    } else {
        1
    };
    let current_page = current_page.clamp(1, total_pages);

    let display_start = ((current_page - 1) * page_size) as usize;
    let display_end = (display_start + num_rows_this_page).min(total_rows);

    // ── Column widths (resizable) ────────────────────────────────────────

    let column_widths = RwSignal::new(HashMap::<usize, f64>::new());
    let user_resized = RwSignal::new(false);

    // ── Resize state ─────────────────────────────────────────────────────

    let resizing_col = RwSignal::new(None::<usize>);
    let resize_start_x = RwSignal::new(0.0_f64);
    let resize_start_width = RwSignal::new(0.0_f64);

    let table_ref = NodeRef::<leptos::html::Table>::new();

    let col_style = move |idx: usize| -> String {
        if user_resized.get()
            && let Some(w) = column_widths.get().get(&idx).copied()
        {
            return format!(
                "flex: 0 0 {w}px; width: {w}px; min-width: {w}px; max-width: {w}px;"
            );
        }
        "flex: 1 1 0; min-width: 100px;".to_string()
    };

    // ── Mousedown on resize handle ───────────────────────────────────────

    let on_resize_start = move |col_idx: usize, ev: web_sys::MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();

        let start_x = ev.client_x() as f64;

        let current_width = table_ref.get().and_then(|table| {
            let el: &web_sys::HtmlElement = &table;
            let ths = el.query_selector_all("thead th").ok()?;
            let th = ths.item((col_idx + 1) as u32)?;
            let rect = th.unchecked_ref::<web_sys::Element>().get_bounding_client_rect();
            Some(rect.width())
        }).unwrap_or(DEFAULT_COL_WIDTH);

        resizing_col.set(Some(col_idx));
        resize_start_x.set(start_x);
        resize_start_width.set(current_width);

        if !user_resized.get_untracked()
            && let Some(table) = table_ref.get()
        {
            let el: &web_sys::HtmlElement = &table;
            if let Ok(ths) = el.query_selector_all("thead th") {
                let mut widths = HashMap::new();
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

        #[cfg(feature = "hydrate")]
        {
            use std::cell::RefCell;
            use std::rc::Rc;
            use wasm_bindgen::closure::Closure;
            use wasm_bindgen::JsCast;

            let Some(window) = web_sys::window() else { return };
            let Some(document) = window.document() else { return };

            type ColResizeClosures = Rc<RefCell<Option<(Closure<dyn FnMut(web_sys::MouseEvent)>, Closure<dyn FnMut()>)>>>;
            let closures: ColResizeClosures = Rc::new(RefCell::new(None));

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
                if let Some((_, ref up_closure)) = *closures_for_up.borrow() {
                    let up_fn: &js_sys::Function = up_closure.as_ref().unchecked_ref();
                    let _ = document_clone
                        .remove_event_listener_with_callback("mouseup", up_fn);
                }
                if let Some(body) = document_clone.body() {
                    let _ = body.style().set_property("cursor", "");
                    let _ = body.style().set_property("user-select", "");
                }
                closures_for_up.borrow_mut().take();
            });

            if let Some(body) = document.body() {
                let _ = body.style().set_property("cursor", "col-resize");
                let _ = body.style().set_property("user-select", "none");
            }

            let _ = document
                .add_event_listener_with_callback("mousemove", move_fn.unchecked_ref());
            let _ = document
                .add_event_listener_with_callback("mouseup", up_handler.as_ref().unchecked_ref());

            *closures.borrow_mut() = Some((move_handler, up_handler));
        }
    };

    // ── Render ───────────────────────────────────────────────────────────

    view! {
        <div class="flex-1 min-h-0 h-full flex flex-col rounded-md overflow-hidden relative border border-border bg-card">
            // Header: "Results: Showing X-Y of Z rows x N columns"
            <div class="px-4 py-3 border-b text-sm flex-shrink-0 flex items-center justify-between bg-muted border-border text-muted-foreground">
                <span>
                    "Results: Showing "
                    {display_start + 1}"-"{display_end}
                    {if has_known_total {
                        format!(" of {total_rows}")
                    } else {
                        String::new()
                    }}
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
                            <th class="resizable-table-row-number-header bg-muted border-b border-r-2 border-border text-center font-normal text-muted-foreground"
                                style="flex: 0 0 50px; width: 50px; min-width: 50px; max-width: 50px; padding: 6px 4px; position: sticky; top: 0; font-size: 0.75rem;"
                            />
                            {col_names
                                .iter()
                                .enumerate()
                                .map(|(idx, col_name)| {
                                    let col_name = col_name.clone();
                                    view! {
                                        <th
                                            class="resizable-table-header text-left relative transition-colors bg-muted border-b border-border font-semibold text-foreground"
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
                                                on:mousedown=move |ev: web_sys::MouseEvent| {
                                                    on_resize_start(idx, ev);
                                                }
                                            >
                                                <div
                                                    class="resizable-table-resize-handle-inner bg-border hover:bg-muted-foreground transition-colors"
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
                        {rendered_rows
                            .into_iter()
                            .enumerate()
                            .map(|(row_idx, row)| {
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
                                            .into_iter()
                                            .enumerate()
                                            .map(|(cell_idx, cell)| {
                                                let text_align = if cell.align_right {
                                                    "text-align: right;"
                                                } else {
                                                    ""
                                                };
                                                let is_null = cell.is_null;
                                                let display_value = cell.display;

                                                view! {
                                                    <td
                                                        class="resizable-table-cell border-b border-border text-foreground"
                                                        style=move || format!(
                                                            "{} padding: 4px 8px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; {text_align}", col_style(cell_idx)
                                                        )
                                                    >
                                                        {if is_null {
                                                            view! { <span class="italic text-muted-foreground">"NULL"</span> }
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
                has_known_total=has_known_total
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
    has_known_total: bool,
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
                <div class="w-20">
                    <StyledSelect
                        value=page_size.to_string()
                        options=PAGE_SIZE_OPTIONS.to_vec()
                        on_change=move |val: String| {
                            if let Ok(size) = val.parse::<u32>() {
                                on_page_size_change.run(size);
                            }
                        }
                    />
                </div>
            </div>

            // Page info + navigation
            <div class="flex items-center gap-2 sm:gap-4 min-w-0">
                // Row range display
                <div class="text-xs whitespace-nowrap overflow-hidden text-ellipsis text-muted-foreground">
                    <span class="hidden sm:inline">
                        {display_start + 1}"-"{display_end}
                        {if has_known_total {
                            format!(" of {total_rows}")
                        } else {
                            String::new()
                        }}
                    </span>
                    <span class="sm:hidden">
                        {display_start + 1}"-"{display_end}
                    </span>
                </div>

                // Navigation buttons
                <div class="flex items-center gap-1 flex-shrink-0">
                    <Button variant=ButtonVariant::GhostMuted size=ButtonSize::IconSm
                        disabled=MaybeProp::derive(move || Some(is_first))
                        aria_label="First page" on:click=move |_| on_page_change.run(1)
                    >
                        <Icon icon=phosphor_leptos::CARET_DOUBLE_LEFT size="16px" />
                    </Button>
                    <Button variant=ButtonVariant::GhostMuted size=ButtonSize::IconSm
                        disabled=MaybeProp::derive(move || Some(is_first))
                        aria_label="Previous page"
                        on:click=move |_| on_page_change.run(current_page.saturating_sub(1).max(1))
                    >
                        <Icon icon=phosphor_leptos::CARET_LEFT size="16px" />
                    </Button>
                    <span class="text-xs px-2 text-muted-foreground">
                        "Page "{current_page}
                        {if has_known_total {
                            format!(" of {total_pages}")
                        } else {
                            String::new()
                        }}
                    </span>
                    <Button variant=ButtonVariant::GhostMuted size=ButtonSize::IconSm
                        disabled=MaybeProp::derive(move || Some(is_last))
                        aria_label="Next page"
                        on:click=move |_| on_page_change.run((current_page + 1).min(total_pages))
                    >
                        <Icon icon=phosphor_leptos::CARET_RIGHT size="16px" />
                    </Button>
                    <Button variant=ButtonVariant::GhostMuted size=ButtonSize::IconSm
                        disabled=MaybeProp::derive(move || Some(is_last))
                        aria_label="Last page" on:click=move |_| on_page_change.run(total_pages)
                    >
                        <Icon icon=phosphor_leptos::CARET_DOUBLE_RIGHT size="16px" />
                    </Button>
                </div>
            </div>
        </div>
    }
}
