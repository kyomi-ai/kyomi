// SPDX-License-Identifier: AGPL-3.0-or-later

//! Results container — orchestrator for tabbed query results display.
//!
//! Mirrors the React `ResultsContainer.jsx`. Composes:
//! - `TabBar` — horizontal tab bar for switching between results
//! - `ResultsTable` — resizable, paginated table
//! - Loading / error / empty states
//!
//! ## Server-side pagination
//! Page changes call `fetch_query_page()` via `spawn_local`. The page
//! size change re-executes the query via `execute_sql_query()`.

use leptos::prelude::*;

use super::state::SqlEditorState;
use super::tab_bar::TabBar;
use super::results_table::ResultsTable;
use super::types::{QueryStatus, ResultTab};
use crate::components::Spinner;
#[cfg(target_arch = "wasm32")]
use crate::server_fns::sql_editor::fetch_query_page;

// ─────────────────────────────────────────────────────────────────────────────
// ResultsContainer component
// ─────────────────────────────────────────────────────────────────────────────

/// Main orchestrator for tabbed results display.
///
/// Renders a tab bar at the top and the active tab's content below:
/// - **Loading** → spinner + "Running query..." message
/// - **Error** → error message + "Re-run Query" button
/// - **Success** → `ResultsTable` with data
/// - **Idle / no tab** → empty state
///
/// # Props
/// - `on_restore_query` — forwarded to `TabBar` for double-click restore.
/// - `on_run_query` — called when the user clicks "Re-run Query" on an error tab.
///   Receives the SQL text and should return a future that resolves when the query
///   completes (tab update is the caller's responsibility).
#[component]
pub fn ResultsContainer(
    /// Called when a tab is double-clicked to restore its query + datasource.
    /// Arguments: `(query_text, datasource_slug)`.
    #[prop(optional)]
    on_restore_query: Option<Callback<(String, Option<String>)>>,
    /// Called when the user clicks "Re-run Query" on an expired/errored tab.
    /// Receives the SQL text. The caller is responsible for re-executing and
    /// updating the tab via `SqlEditorState`.
    #[prop(optional)]
    on_run_query: Option<Callback<String>>,
) -> impl IntoView {
    let state = SqlEditorState::use_state();
    let active_tab = state.active_tab();
    let active_table_ui = state.active_table_ui_state();
    let tabs = state.tabs;

    // Local signal for pagination loading overlay.
    let (is_paginating, set_is_paginating) = signal(false);

    // ── Page change handler ──────────────────────────────────────────────

    let handle_page_change = {
        let state = state;
        Callback::new(move |page: u32| {
            let Some(tab) = active_tab.get() else { return };
            let Some(ref result) = tab.result else { return };
            let Some(ref handle) = result.query_handle else { return };

            let datasource_slug = handle.datasource_slug.clone();
            let sql = handle.sql.clone();
            let job_id = handle.job_id.clone();
            let page_size = active_table_ui.get().page_size;
            let tab_id = tab.id.clone();

            // Preserve fields from the existing result for the update.
            let prev_columns = result.columns.clone();
            let prev_total_rows = result.total_rows;
            let prev_execution_time = result.execution_time;
            let prev_bytes_processed = result.bytes_processed;
            let prev_query_handle = result.query_handle.clone();

            set_is_paginating.set(true);

            #[cfg(target_arch = "wasm32")]
            leptos::task::spawn_local(async move {
                let result = fetch_query_page(
                    datasource_slug,
                    sql,
                    page,
                    page_size,
                    job_id,
                    Some(false),
                )
                .await;

                match result {
                    Ok(new_result) => {
                        // Merge: use new rows but preserve metadata from original execution.
                        state.update_tab(&tab_id, |tab| {
                            tab.status = QueryStatus::Success;
                            tab.error = None;
                            tab.result = Some(super::types::QueryResult {
                                columns: if new_result.columns.is_empty() {
                                    prev_columns.clone()
                                } else {
                                    new_result.columns
                                },
                                rows: new_result.rows,
                                row_count: new_result.row_count,
                                total_rows: prev_total_rows,
                                query_handle: prev_query_handle.clone(),
                                execution_time: prev_execution_time,
                                bytes_processed: prev_bytes_processed,
                                has_more: new_result.has_more,
                            });
                        });
                        state.set_table_ui_state(&tab_id, |ui| {
                            ui.current_page = page;
                        });
                    }
                    Err(err) => {
                        let msg = format!("{err}");
                        let is_expired = msg.contains("Not found")
                            || msg.contains("404")
                            || msg.contains("not found")
                            || msg.contains("expired");

                        let error_msg = if is_expired {
                            "Query results expired. Please re-run the query.".to_string()
                        } else {
                            msg
                        };

                        state.update_tab(&tab_id, |tab| {
                            tab.status = QueryStatus::Error;
                            tab.error = Some(super::types::QueryError {
                                message: error_msg.clone(),
                                code: None,
                                line: None,
                                column: None,
                            });
                        });
                    }
                }
                set_is_paginating.set(false);
            });
        })
    };

    // ── Page size change handler ─────────────────────────────────────────

    let handle_page_size_change = {
        let state = state;
        Callback::new(move |new_page_size: u32| {
            let Some(tab) = active_tab.get() else { return };
            let Some(ref result) = tab.result else { return };
            let Some(ref handle) = result.query_handle else { return };

            let datasource_slug = handle.datasource_slug.clone();
            let sql = handle.sql.clone();
            let tab_id = tab.id.clone();

            // Save as user's default preference.
            state.set_default_page_size(new_page_size);

            set_is_paginating.set(true);

            #[cfg(target_arch = "wasm32")]
            leptos::task::spawn_local(async move {
                use crate::server_fns::sql_editor::execute_sql_query;

                let result = execute_sql_query(
                    datasource_slug,
                    sql,
                    new_page_size,
                    1, // Reset to page 1
                )
                .await;

                match result {
                    Ok(new_result) => {
                        state.update_tab(&tab_id, |tab| {
                            tab.status = QueryStatus::Success;
                            tab.error = None;
                            tab.result = Some(new_result);
                        });
                        state.set_table_ui_state(&tab_id, |ui| {
                            ui.page_size = new_page_size;
                            ui.current_page = 1;
                        });
                    }
                    Err(err) => {
                        state.update_tab(&tab_id, |tab| {
                            tab.status = QueryStatus::Error;
                            tab.error = Some(super::types::QueryError {
                                message: format!("{err}"),
                                code: None,
                                line: None,
                                column: None,
                            });
                        });
                    }
                }
                set_is_paginating.set(false);
            });
        })
    };

    // ── Tab content based on status ──────────────────────────────────────

    let tab_content = move || {
        let tab = active_tab.get();
        let paginating = is_paginating.get();
        let ui_state = active_table_ui.get();

        match tab {
            None => {
                // No active tab — empty state
                view! {
                    <div class="flex-1 flex items-center justify-center text-muted-foreground">
                        <div class="text-center">
                            <svg class="w-16 h-16 mx-auto mb-4 text-muted-foreground/50" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path
                                    stroke-linecap="round"
                                    stroke-linejoin="round"
                                    stroke-width="1.5"
                                    d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
                                />
                            </svg>
                            <p class="text-sm">"No active result tab"</p>
                        </div>
                    </div>
                }.into_any()
            }
            Some(tab) => render_tab_content(
                tab,
                paginating,
                ui_state,
                handle_page_change,
                handle_page_size_change,
                on_run_query,
            ),
        }
    };

    // ── Render ───────────────────────────────────────────────────────────

    // Don't render at all if there are no tabs.
    move || {
        let current_tabs = tabs.get();
        if current_tabs.is_empty() {
            return None;
        }

        Some(view! {
            <div class="flex-1 flex flex-col min-h-0 border border-input rounded-md overflow-hidden bg-card">
                <TabBar on_restore_query=on_restore_query />
                {tab_content}
            </div>
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tab content rendering
// ─────────────────────────────────────────────────────────────────────────────

/// Render the content area for the active tab based on its status.
fn render_tab_content(
    tab: ResultTab,
    is_paginating: bool,
    ui_state: super::types::TableUIState,
    on_page_change: Callback<u32>,
    on_page_size_change: Callback<u32>,
    on_run_query: Option<Callback<String>>,
) -> AnyView {
    // Needs refresh → loading state
    if tab.needs_refresh {
        return view! {
            <ResultsLoading message="Restoring query results..." />
        }
        .into_any();
    }

    // Running → loading state
    if tab.status == QueryStatus::Running {
        return view! {
            <ResultsLoading message="Running query..." />
        }
        .into_any();
    }

    // Error → error display with optional re-run
    if let Some(ref error) = tab.error {
        let error_message = error.message.clone();
        let query = tab.query.clone();
        return view! {
            <ResultsError
                message=error_message
                on_rerun=on_run_query.map(move |cb| {
                    let query = query.clone();
                    Callback::new(move |_: ()| cb.run(query.clone()))
                })
            />
        }
        .into_any();
    }

    // Success with result → table
    if (tab.status == QueryStatus::Success || tab.status == QueryStatus::Streaming)
        && tab.result.is_some()
    {
        let result = tab.result.unwrap();
        let is_streaming = tab.status == QueryStatus::Streaming;
        let row_count = result.row_count;

        return view! {
            <div class="flex-1 flex flex-col min-h-0 relative">
                // Streaming indicator
                {is_streaming.then(move || {
                    view! {
                        <div class="flex items-center gap-2 px-3 py-1.5 bg-primary/5 border-b border-border text-xs text-muted-foreground">
                            <div class="w-2 h-2 rounded-full bg-primary animate-pulse" />
                            "Streaming rows... ("{row_count}" received)"
                        </div>
                    }
                })}

                <div class="flex-1 min-h-0">
                    <ResultsTable
                        result=result
                        current_page=ui_state.current_page
                        page_size=ui_state.page_size
                        is_paginating=is_paginating
                        on_page_change=on_page_change
                        on_page_size_change=on_page_size_change
                    />
                </div>
            </div>
        }
        .into_any();
    }

    // Idle / other — blank
    view! { <div /> }.into_any()
}

// ─────────────────────────────────────────────────────────────────────────────
// Loading state
// ─────────────────────────────────────────────────────────────────────────────

/// Loading state displayed while a query is running or results are being
/// restored from localStorage.
#[component]
fn ResultsLoading(
    /// Message to display below the spinner.
    #[prop(into)]
    message: String,
) -> impl IntoView {
    view! {
        <div class="flex-1 flex items-center justify-center">
            <div class="text-center">
                <Spinner class="text-primary mx-auto mb-3 !h-8 !w-8" />
                <p class="text-sm text-muted-foreground">{message}</p>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error state
// ─────────────────────────────────────────────────────────────────────────────

/// Error state displayed when a query fails or results expire.
///
/// Shows the error message and an optional "Re-run Query" button.
#[component]
fn ResultsError(
    /// The error message to display.
    #[prop(into)]
    message: String,
    /// Called when the user clicks "Re-run Query". If `None`, the button is hidden.
    on_rerun: Option<Callback<()>>,
) -> impl IntoView {
    view! {
        <div class="flex-1 flex items-center justify-center p-6">
            <div class="text-center max-w-lg">
                // Warning triangle icon
                <svg
                    class="w-12 h-12 mx-auto mb-4 text-destructive/60"
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                >
                    <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="1.5"
                        d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126zM12 15.75h.007v.008H12v-.008z"
                    />
                </svg>

                <p class="text-sm text-destructive mb-4 font-mono whitespace-pre-wrap break-words">
                    {message}
                </p>

                {on_rerun.map(|cb| {
                    view! {
                        <button
                            class="px-4 py-2 text-sm font-medium bg-primary text-white rounded-md hover:bg-primary/90 transition-colors"
                            on:click=move |_| cb.run(())
                        >
                            "Re-run Query"
                        </button>
                    }
                })}
            </div>
        </div>
    }
}
