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
#[cfg(target_arch = "wasm32")]
use crate::server_fns::sql_editor::generate_chart_from_results;

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

    // Local signal for re-run loading state (used by ResultsError button).
    let (is_rerunning, set_is_rerunning) = signal(false);

    // ── Auto-refresh for tabs restored from localStorage ─────────────────
    // Tabs with `needs_refresh = true` need their data re-fetched from the
    // server. Without this effect they show a spinner forever.
    #[cfg(target_arch = "wasm32")]
    {
        let state = state;
        Effect::new(move |_| {
            let Some(tab) = active_tab.get() else { return };
            if !tab.needs_refresh { return; }

            let Some(ref result) = tab.result else { return };
            let Some(ref handle) = result.query_handle else { return };

            let datasource_slug = handle.datasource_slug.clone();
            let sql = handle.sql.clone();
            let job_id = handle.job_id.clone();
            let tab_id = tab.id.clone();
            let ui_state = active_table_ui.get();

            leptos::task::spawn_local(async move {
                let fetch_result = fetch_query_page(
                    datasource_slug,
                    sql,
                    ui_state.current_page,
                    ui_state.page_size,
                    job_id,
                    Some(false),
                )
                .await;

                match fetch_result {
                    Ok(new_result) => {
                        state.update_tab(&tab_id, |tab| {
                            tab.status = QueryStatus::Success;
                            tab.error = None;
                            tab.needs_refresh = false;
                            tab.result = Some(new_result);
                        });
                    }
                    Err(err) => {
                        let msg = format!("{err}");
                        let error_msg = if msg.contains("Not found")
                            || msg.contains("404")
                            || msg.contains("not found")
                            || msg.contains("expired")
                        {
                            "Query results expired. Please re-run the query.".to_string()
                        } else {
                            msg
                        };

                        state.update_tab(&tab_id, |tab| {
                            tab.status = QueryStatus::Error;
                            tab.needs_refresh = false;
                            tab.error = Some(super::types::QueryError {
                                message: error_msg,
                                code: None,
                                line: None,
                                column: None,
                            });
                        });
                    }
                }
            });
        });
    }

    // ── Page change handler ──────────────────────────────────────────────

    let handle_page_change = {
        let _state = state;
        Callback::new(move |page: u32| {
            let Some(tab) = active_tab.get() else { return };
            let Some(ref result) = tab.result else { return };
            let Some(ref handle) = result.query_handle else { return };

            set_is_paginating.set(true);

            #[cfg(not(target_arch = "wasm32"))]
            let _ = (page, handle);

            #[cfg(target_arch = "wasm32")]
            {
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
            }
        })
    };

    // ── Page size change handler ─────────────────────────────────────────

    let handle_page_size_change = {
        let _state = state;
        Callback::new(move |new_page_size: u32| {
            let Some(tab) = active_tab.get() else { return };
            let Some(ref result) = tab.result else { return };
            let Some(ref handle) = result.query_handle else { return };

            // Save as user's default preference.
            state.set_default_page_size(new_page_size);

            set_is_paginating.set(true);

            #[cfg(not(target_arch = "wasm32"))]
            let _ = handle;

            #[cfg(target_arch = "wasm32")]
            {
                let datasource_slug = handle.datasource_slug.clone();
                let sql = handle.sql.clone();
                let tab_id = tab.id.clone();

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
            }
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
                is_rerunning.into(),
                set_is_rerunning,
            ),
        }
    };

    // ── Chart generation state ───────────────────────────────────────────
    let (chart_generating, set_chart_generating) = signal(false);
    let (chart_yaml, set_chart_yaml) = signal(None::<String>);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = set_chart_yaml;
    let (chart_error, set_chart_error) = signal(None::<String>);
    let (show_chart_modal, set_show_chart_modal) = signal(false);
    // Track clipboard feedback.
    let (copied, set_copied) = signal(false);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = set_copied;

    let handle_create_chart = {
        Callback::new(move |_: ()| {
            let Some(tab) = active_tab.get() else { return };
            let Some(ref result) = tab.result else { return };
            if result.columns.is_empty() || result.rows.is_empty() {
                return;
            }

            set_chart_generating.set(true);
            set_chart_error.set(None);

            #[cfg(target_arch = "wasm32")]
            {
                let columns: Vec<String> = result.columns.iter().map(|c| c.name.clone()).collect();
                // Take first 100 rows as sample.
                let sample_rows: Vec<Vec<serde_json::Value>> = result
                    .rows
                    .iter()
                    .take(100)
                    .cloned()
                    .collect();
                let sql = tab.query.clone();

                leptos::task::spawn_local(async move {
                    match generate_chart_from_results(columns, sample_rows, sql).await {
                        Ok(chart) => {
                            set_chart_yaml.set(Some(chart.chartml_yaml));
                            set_show_chart_modal.set(true);
                        }
                        Err(err) => {
                            set_chart_error.set(Some(format!("{err}")));
                        }
                    }
                    set_chart_generating.set(false);
                });
            }
        })
    };

    let handle_copy_yaml = move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(yaml) = chart_yaml.get_untracked() {
                if let Some(window) = web_sys::window() {
                    let clipboard = window.navigator().clipboard();
                    let _ = clipboard.write_text(&yaml);
                    set_copied.set(true);
                    // Reset after 2 seconds.
                    leptos::task::spawn_local(async move {
                        gloo_timers::future::TimeoutFuture::new(2000).await;
                        set_copied.set(false);
                    });
                }
            }
        }
    };

    let close_chart_modal = move |_| {
        set_show_chart_modal.set(false);
    };

    // ── Render ───────────────────────────────────────────────────────────

    // Don't render at all if there are no tabs.
    move || {
        let current_tabs = tabs.get();
        if current_tabs.is_empty() {
            return None;
        }

        // "Create Chart" button — shown only when there are results.
        let chart_button = {
            let generating = chart_generating;
            let error = chart_error;
            let create_chart = handle_create_chart;

            view! {
                <button
                    class="px-2.5 py-1 text-xs font-medium text-muted-foreground hover:text-foreground bg-background border border-input rounded-md hover:bg-accent transition-colors disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-1.5"
                    disabled=move || generating.get()
                    aria-label="Create chart from results"
                    on:click=move |_| create_chart.run(())
                >
                    {move || if generating.get() {
                        view! {
                            <Spinner class="!h-3 !w-3" />
                            <span>"Generating..."</span>
                        }.into_any()
                    } else {
                        view! {
                            <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path
                                    stroke-linecap="round"
                                    stroke-linejoin="round"
                                    stroke-width="2"
                                    d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z"
                                />
                            </svg>
                            <span>"Create Chart"</span>
                        }.into_any()
                    }}
                </button>
                // Inline error message.
                {move || error.get().map(|msg| {
                    let title = msg.clone();
                    view! {
                        <span class="text-xs text-destructive truncate max-w-[200px]" title=title>{msg}</span>
                    }
                })}
            }
        };

        Some(view! {
            <div class="flex-1 flex flex-col min-h-0 border border-input rounded-md overflow-hidden bg-card" role="tabpanel" aria-label="Query results">
                <TabBar on_restore_query=on_restore_query header_actions=chart_button />
                {tab_content}

                // ChartML preview modal
                <Show when=move || show_chart_modal.get()>
                    <ChartYamlModal
                        yaml=Signal::derive(move || chart_yaml.get().unwrap_or_default())
                        copied=Signal::derive(move || copied.get())
                        on_copy=handle_copy_yaml
                        on_close=close_chart_modal
                    />
                </Show>
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
    is_rerunning: Signal<bool>,
    set_is_rerunning: WriteSignal<bool>,
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
                    Callback::new(move |_: ()| {
                        set_is_rerunning.set(true);
                        cb.run(query.clone());
                    })
                })
                is_rerunning=is_rerunning
            />
        }
        .into_any();
    }

    // Success with result → table
    if let Some(result) = tab.result
        && (tab.status == QueryStatus::Success || tab.status == QueryStatus::Streaming)
    {
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

/// Check whether an error message indicates expired/unavailable results
/// (as opposed to a SQL execution error).
fn is_expired_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("expired")
        || lower.contains("not found")
        || lower.contains("failed to restore")
}

/// Error state displayed when a query fails or results expire.
///
/// Distinguishes between:
/// - **Expired results** (message matches "expired", "not found", "failed to restore")
///   → informational icon, friendly heading, re-run button.
/// - **SQL errors** → warning triangle icon with the raw error message.
#[component]
fn ResultsError(
    /// The error message to display.
    #[prop(into)]
    message: String,
    /// Called when the user clicks "Re-run Query". If `None`, the button is hidden.
    on_rerun: Option<Callback<()>>,
    /// Whether a re-run is currently in progress.
    #[prop(default = false.into())]
    is_rerunning: Signal<bool>,
) -> impl IntoView {
    let expired = is_expired_error(&message);

    if expired {
        // ── Expired / unavailable results ────────────────────────────────
        view! {
            <div class="flex-1 flex items-center justify-center p-6">
                <div class="text-center max-w-lg">
                    // Info circle icon
                    <svg
                        class="w-12 h-12 mx-auto mb-4 text-muted-foreground/60"
                        fill="none"
                        stroke="currentColor"
                        viewBox="0 0 24 24"
                    >
                        <path
                            stroke-linecap="round"
                            stroke-linejoin="round"
                            stroke-width="1.5"
                            d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
                        />
                    </svg>

                    <h3 class="text-sm font-semibold text-foreground mb-2">
                        "Results No Longer Available"
                    </h3>
                    <p class="text-sm text-muted-foreground mb-4">
                        {message}
                    </p>

                    {on_rerun.map(|cb| {
                        view! {
                            <button
                                class="px-4 py-2 text-sm font-medium bg-primary text-primary-foreground rounded-md hover:bg-primary/90 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                                disabled=move || is_rerunning.get()
                                on:click=move |_| {
                                    if !is_rerunning.get_untracked() {
                                        cb.run(());
                                    }
                                }
                            >
                                {move || if is_rerunning.get() { "Re-running Query..." } else { "Re-run Query" }}
                            </button>
                        }
                    })}
                </div>
            </div>
        }
        .into_any()
    } else {
        // ── SQL / execution error ────────────────────────────────────────
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
                                class="px-4 py-2 text-sm font-medium bg-primary text-primary-foreground rounded-md hover:bg-primary/90 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                                disabled=move || is_rerunning.get()
                                on:click=move |_| {
                                    if !is_rerunning.get_untracked() {
                                        cb.run(());
                                    }
                                }
                            >
                                {move || if is_rerunning.get() { "Re-running Query..." } else { "Re-run Query" }}
                            </button>
                        }
                    })}
                </div>
            </div>
        }
        .into_any()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ChartML YAML preview modal
// ─────────────────────────────────────────────────────────────────────────────

/// Simple modal that displays generated ChartML YAML with a copy button.
///
/// This is a lightweight alternative to the full ChartBuilder — shows the raw
/// YAML in a scrollable code block with copy-to-clipboard support.
#[component]
fn ChartYamlModal(
    /// The generated ChartML YAML string.
    #[prop(into)]
    yaml: Signal<String>,
    /// Whether the "Copied!" feedback is active.
    #[prop(into)]
    copied: Signal<bool>,
    /// Called when the copy button is clicked.
    on_copy: impl Fn(web_sys::MouseEvent) + Send + Sync + 'static,
    /// Called when the close button or backdrop is clicked.
    on_close: impl Fn(web_sys::MouseEvent) + Send + Sync + Clone + 'static,
) -> impl IntoView {
    let on_close_backdrop = on_close.clone();

    view! {
        // Backdrop
        <div
            class="fixed inset-0 bg-[var(--color-overlay)] z-50 flex items-center justify-center p-4"
            on:click=move |ev: web_sys::MouseEvent| {
                // Only close on direct backdrop click, not bubbled clicks.
                if ev.target() == ev.current_target() {
                    on_close_backdrop(ev);
                }
            }
            role="dialog"
            aria-modal="true"
            aria-label="Generated ChartML"
        >
            <div class="bg-card rounded-lg shadow-xl border border-border w-full max-w-2xl max-h-[80vh] flex flex-col">
                // Header
                <div class="flex items-center justify-between px-4 py-3 border-b border-border flex-shrink-0">
                    <h2 class="text-sm font-semibold text-foreground">"Generated ChartML"</h2>
                    <div class="flex items-center gap-2">
                        // Copy button
                        <button
                            class="px-3 py-1.5 text-xs font-medium bg-primary text-primary-foreground rounded-md hover:bg-primary/90 transition-colors flex items-center gap-1.5"
                            on:click=on_copy
                            aria-label="Copy ChartML to clipboard"
                        >
                            {move || if copied.get() {
                                view! {
                                    <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
                                    </svg>
                                    <span>"Copied!"</span>
                                }.into_any()
                            } else {
                                view! {
                                    <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 5H6a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2v-1M8 5a2 2 0 002 2h2a2 2 0 002-2M8 5a2 2 0 012-2h2a2 2 0 012 2m0 0h2a2 2 0 012 2v3m2 4H10m0 0l3-3m-3 3l3 3" />
                                    </svg>
                                    <span>"Copy"</span>
                                }.into_any()
                            }}
                        </button>
                        // Close button
                        <button
                            class="p-1 text-muted-foreground hover:text-foreground rounded transition-colors"
                            on:click=on_close
                            aria-label="Close modal"
                        >
                            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                            </svg>
                        </button>
                    </div>
                </div>
                // Body — scrollable YAML code block
                <div class="flex-1 overflow-auto p-4">
                    <pre class="text-xs font-mono text-foreground bg-muted rounded-md p-4 whitespace-pre-wrap break-words border border-border">
                        {move || yaml.get()}
                    </pre>
                </div>
            </div>
        </div>
    }
}
