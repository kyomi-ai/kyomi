// SPDX-License-Identifier: AGPL-3.0-or-later

//! Results container — orchestrator for tabbed query results display.
//!
//! Mirrors the React `ResultsContainer.jsx`. Composes:
//! - `TabBar` — horizontal tab bar for switching between results
//! - `ResultsTable` — resizable, paginated table
//! - Loading / error / empty states
//!
//! ## Server-side pagination
//! Page changes call `fetch_arrow_buffered()` with the appropriate offset.
//! Page size changes re-execute from offset 0 with the new limit.
//!
//! ## Expired results (restored from localStorage)
//! When a tab is restored from localStorage, `result.data` is `None` (DataTable
//! is not serializable).  The tab is marked `needs_refresh = true` and shows
//! "Results expired — click to re-run" via `ResultsError`.  The user must
//! explicitly re-run — no auto-execute on restore.

use leptos::prelude::*;
use phosphor_leptos::Icon;
use super::state::SqlEditorState;
use super::results_table::ResultsTable;
use super::tab_bar::TabBar;
use super::types::{QueryStatus, ResultTab};
use crate::components::dashboard::chart_builder::ChartBuilderModal;
use crate::components::{Button, ButtonVariant, ButtonSize, Spinner};
#[cfg(target_arch = "wasm32")]
use crate::server_fns::sql_editor::generate_chart_from_results;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// ResultsContainer component
// ─────────────────────────────────────────────────────────────────────────────

/// Main orchestrator for tabbed results display.
///
/// Renders a tab bar at the top and the active tab's content below:
/// - **Loading** → spinner + "Running query..." message
/// - **Needs refresh** → "Results expired — click to re-run" with re-run button
/// - **Error** → error message + "Re-run Query" button
/// - **Success** → `ResultsTable` with data
/// - **Idle / no tab** → empty state
///
/// # Props
/// - `on_restore_query` — forwarded to `TabBar` for double-click restore.
/// - `on_run_query` — called when the user clicks "Re-run Query" on an expired/errored tab.
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
                use super::types::QueryResult;

                let datasource_slug = handle.datasource_slug.clone();
                let sql = handle.sql.clone();
                let job_id = handle.job_id.clone();
                let page_size = active_table_ui.get().page_size;
                let tab_id = tab.id.clone();

                // Offset is 0-based: page 1 starts at offset 0.
                let offset = page.saturating_sub(1) * page_size;

                // Preserve metadata from the original execution.
                let prev_total_rows = result.total_rows;
                let prev_execution_time = result.execution_time;
                let prev_bytes_processed = result.bytes_processed;
                let prev_query_handle = result.query_handle.clone();

                leptos::task::spawn_local(async move {
                    let fetch_result = crate::arrow_fetch::fetch_arrow_buffered(
                        &datasource_slug,
                        &sql,
                        page_size,
                        offset,
                        false,
                        job_id.as_deref(),
                    )
                    .await;

                    match fetch_result {
                        Ok(arrow_result) => {
                            let new_job_id = arrow_result.job_id.clone();

                            let updated_handle = prev_query_handle.map(|mut h| {
                                if new_job_id.is_some() {
                                    h.job_id = new_job_id;
                                }
                                h
                            });

                            let mut result = QueryResult::from_arrow(
                                arrow_result,
                                updated_handle,
                                prev_execution_time,
                            );
                            result.total_rows = prev_total_rows;
                            result.bytes_processed = prev_bytes_processed;

                            state.update_tab(&tab_id, |tab| {
                                tab.status = QueryStatus::Success;
                                tab.error = None;
                                tab.result = Some(result);
                            });
                            state.set_table_ui_state(&tab_id, |ui| {
                                ui.current_page = page;
                            });
                        }
                        Err(err) => {
                            let is_expired = {
                                let msg = err.to_lowercase();
                                msg.contains("not found")
                                    || msg.contains("404")
                                    || msg.contains("expired")
                            };
                            let error_msg = if is_expired {
                                "Query results expired. Please re-run the query.".to_string()
                            } else {
                                err
                            };
                            state.update_tab(&tab_id, |tab| {
                                tab.status = QueryStatus::Error;
                                tab.error = Some(super::types::QueryError {
                                    message: error_msg,
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

            state.set_default_page_size(new_page_size);
            set_is_paginating.set(true);

            #[cfg(not(target_arch = "wasm32"))]
            let _ = handle;

            #[cfg(target_arch = "wasm32")]
            {
                use super::types::{QueryHandle, QueryResult};

                let datasource_slug = handle.datasource_slug.clone();
                let datasource_type = handle.datasource_type.clone();
                let sql = handle.sql.clone();
                let job_id = handle.job_id.clone();
                let tab_id = tab.id.clone();

                leptos::task::spawn_local(async move {
                    // Re-execute from the beginning with the new page size.
                    let fetch_result = crate::arrow_fetch::fetch_arrow_buffered(
                        &datasource_slug,
                        &sql,
                        new_page_size,
                        0,
                        true,
                        job_id.as_deref(),
                    )
                    .await;

                    match fetch_result {
                        Ok(arrow_result) => {
                            let new_job_id = arrow_result.job_id.clone();

                            let query_handle = QueryHandle {
                                datasource_type: datasource_type.clone(),
                                datasource_slug: datasource_slug.clone(),
                                sql: sql.clone(),
                                job_id: new_job_id,
                            };

                            let result = QueryResult::from_arrow(
                                arrow_result,
                                Some(query_handle),
                                None,
                            );

                            state.update_tab(&tab_id, |tab| {
                                tab.status = QueryStatus::Success;
                                tab.error = None;
                                tab.result = Some(result);
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
                                    message: err,
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
                            <Icon icon=phosphor_leptos::FILE_TEXT attr:class="w-16 h-16 mx-auto mb-4 text-muted-foreground/50" />
                            <p class="text-sm">"No active result tab"</p>
                        </div>
                    </div>
                }.into_any()
            }
            Some(tab) => render_tab_content(TabContentProps {
                tab,
                is_paginating: paginating,
                ui_state,
                on_page_change: handle_page_change,
                on_page_size_change: handle_page_size_change,
                on_run_query,
                is_rerunning: is_rerunning.into(),
                set_is_rerunning,
            }),
        }
    };

    // ── Chart generation state ───────────────────────────────────────────
    let (chart_generating, set_chart_generating) = signal(false);
    let (chart_yaml, set_chart_yaml) = signal(None::<String>);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = set_chart_yaml;
    let (chart_error, set_chart_error) = signal(None::<String>);
    let (show_chart_modal, set_show_chart_modal) = signal(false);
    let handle_create_chart = {
        Callback::new(move |_: ()| {
            let Some(tab) = active_tab.get() else { return };
            let Some(ref result) = tab.result else { return };

            // Need at least some columns to generate a chart.
            let has_data = result.data.as_ref().map(|d| !d.is_empty()).unwrap_or(false)
                || !result.rows.is_empty();
            if !has_data {
                return;
            }

            set_chart_generating.set(true);
            set_chart_error.set(None);

            #[cfg(target_arch = "wasm32")]
            {
                // Build column names and sample rows for the server function.
                // For the Arrow path, convert DataTable to JSON rows (first 100).
                // For the legacy JSON path, use the rows directly.
                let (columns, sample_rows): (Vec<String>, Vec<Vec<serde_json::Value>>) =
                    if let Some(ref data) = result.data {
                        let col_names = data.field_names();
                        // Precompute numeric flag per column by probing first
                        // non-null value (avoids a direct arrow-schema dep).
                        let numeric_flags: Vec<bool> = col_names
                            .iter()
                            .map(|name| super::results_table::is_datatable_column_numeric(data, name))
                            .collect();

                        let rows: Vec<Vec<serde_json::Value>> = (0..data.num_rows().min(100))
                            .map(|row_idx| {
                                col_names
                                    .iter()
                                    .zip(numeric_flags.iter())
                                    .map(|(col_name, &is_numeric)| {
                                        match data.get_string(row_idx, col_name) {
                                            None => serde_json::Value::Null,
                                            Some(s) => {
                                                if is_numeric {
                                                    s.parse::<f64>()
                                                        .map(|n| serde_json::json!(n))
                                                        .unwrap_or(serde_json::Value::String(s))
                                                } else {
                                                    serde_json::Value::String(s)
                                                }
                                            }
                                        }
                                    })
                                    .collect()
                            })
                            .collect();
                        (col_names, rows)
                    } else {
                        let col_names =
                            result.columns.iter().map(|c| c.name.clone()).collect();
                        let rows = result.rows.iter().take(100).cloned().collect();
                        (col_names, rows)
                    };

                let sql = tab.query.clone();
                let ds_slug = tab.datasource_slug.clone().unwrap_or_default();

                leptos::task::spawn_local(async move {
                    match generate_chart_from_results(columns, sample_rows, sql, ds_slug).await {
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

    // ── Render ───────────────────────────────────────────────────────────

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
                <Button
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Xs
                    disabled=MaybeProp::derive(move || Some(generating.get()))
                    aria_label="Create chart from results"
                    on:click=move |_| create_chart.run(())
                >
                    {move || if generating.get() {
                        view! {
                            <Spinner size="h-3 w-3" />
                            <span>"Generating..."</span>
                        }.into_any()
                    } else {
                        view! {
                            <Icon icon=phosphor_leptos::CHART_BAR attr:class="w-3.5 h-3.5" />
                            <span>"Create Chart"</span>
                        }.into_any()
                    }}
                </Button>
                // Inline error message.
                {move || error.get().map(|msg| {
                    let title = msg.clone();
                    view! {
                        <span class="text-xs text-error-foreground truncate max-w-[200px]" title=title>{msg}</span>
                    }
                })}
            }
        };

        Some(view! {
            <div class="flex-1 flex flex-col min-h-0 border border-border rounded-md overflow-hidden bg-card" role="tabpanel" aria-label="Query results">
                <TabBar on_restore_query=on_restore_query header_actions=chart_button />
                {tab_content}

                // Chart builder modal — mounted when YAML is ready
                <Show when=move || show_chart_modal.get()>
                    {move || {
                        let yaml = chart_yaml.get().unwrap_or_default();
                        view! {
                            <ChartBuilderModal
                                open=Signal::stored(true)
                                existing_yaml=yaml
                                on_close=Callback::new(move |()| {
                                    set_show_chart_modal.set(false);
                                    set_chart_yaml.set(None);
                                })
                                on_insert=Callback::new(move |_yaml: String| {
                                    set_show_chart_modal.set(false);
                                    set_chart_yaml.set(None);
                                })
                            />
                        }
                    }}
                </Show>
            </div>
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tab content rendering
// ─────────────────────────────────────────────────────────────────────────────

/// Props for [`render_tab_content`] — groups the arguments to stay under clippy's limit.
struct TabContentProps {
    tab: ResultTab,
    is_paginating: bool,
    ui_state: super::types::TableUIState,
    on_page_change: Callback<u32>,
    on_page_size_change: Callback<u32>,
    on_run_query: Option<Callback<String>>,
    is_rerunning: Signal<bool>,
    set_is_rerunning: WriteSignal<bool>,
}

/// Build a re-run callback that updates the existing tab in-place rather than
/// creating a new one. Extracts the tab's query, datasource, and ID, then calls
/// `rerun_query` which sets the tab to Running and re-executes.
///
/// Falls back to the parent's `on_run_query` callback if the tab doesn't have
/// enough context for an in-place re-run (missing query handle or datasource).
fn build_rerun_callback(
    tab: &super::types::ResultTab,
    on_run_query: Option<Callback<String>>,
    set_is_rerunning: WriteSignal<bool>,
) -> Option<Callback<()>> {
    let query = tab.query.clone();
    let tab_id = tab.id.clone();
    let ds_slug = tab.datasource_slug.clone();
    let ds_type = tab.datasource_type.clone();

    if let (Some(slug), Some(dtype)) = (ds_slug, ds_type) {
        let state = super::state::SqlEditorState::use_state();
        Some(Callback::new(move |_: ()| {
            set_is_rerunning.set(true);
            let reset = set_is_rerunning;
            super::execution::rerun_query(
                state,
                tab_id.clone(),
                query.clone(),
                slug.clone(),
                dtype.clone(),
                Some(Callback::new(move |_| reset.set(false))),
            );
        }))
    } else {
        // Fallback: no datasource context — use the parent callback.
        // Don't set is_rerunning here — the parent callback (run_query) has
        // no mechanism to reset it, which would leave the button stuck.
        on_run_query.map(move |cb| {
            Callback::new(move |_: ()| {
                cb.run(query.clone());
            })
        })
    }
}

/// Render the content area for the active tab based on its status.
fn render_tab_content(props: TabContentProps) -> AnyView {
    let TabContentProps {
        tab,
        is_paginating,
        ui_state,
        on_page_change,
        on_page_size_change,
        on_run_query,
        is_rerunning,
        set_is_rerunning,
    } = props;

    // Running → loading state
    if tab.status == QueryStatus::Running {
        return view! {
            <ResultsLoading message="Running query..." />
        }
        .into_any();
    }

    // Needs refresh: data was not persisted (DataTable is not serializable).
    // Show expiry message — the user clicks "Re-run Query" to fetch again.
    if tab.needs_refresh
        || (tab.result.as_ref().map(|r| r.data.is_none() && r.rows.is_empty()).unwrap_or(false)
            && tab.result.as_ref().and_then(|r| r.query_handle.as_ref()).is_some())
    {
        let rerun_cb = build_rerun_callback(&tab, on_run_query, set_is_rerunning);
        return view! {
            <ResultsError
                message="Results expired — click to re-run.".to_string()
                on_rerun=rerun_cb
                is_rerunning=is_rerunning
            />
        }
        .into_any();
    }

    // Error → error display with optional re-run
    if let Some(ref error) = tab.error {
        let error_message = error.message.clone();
        let rerun_cb = build_rerun_callback(&tab, on_run_query, set_is_rerunning);
        return view! {
            <ResultsError
                message=error_message
                on_rerun=rerun_cb
                is_rerunning=is_rerunning
            />
        }
        .into_any();
    }

    // Success with result → table
    if let Some(result) = tab.result
        && tab.status == QueryStatus::Success
    {
        return view! {
            <div class="flex-1 flex flex-col min-h-0 relative">
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

/// Loading state displayed while a query is running.
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
                    <Icon icon=phosphor_leptos::INFO attr:class="w-12 h-12 mx-auto mb-4 text-muted-foreground/60" />

                    <h3 class="text-sm font-semibold text-foreground mb-2">
                        "Results No Longer Available"
                    </h3>
                    <p class="text-sm text-muted-foreground mb-4">
                        {message}
                    </p>

                    {on_rerun.map(|cb| {
                        view! {
                            <Button
                                disabled=MaybeProp::derive(move || Some(is_rerunning.get()))
                                on:click=move |_| {
                                    if !is_rerunning.get_untracked() {
                                        cb.run(());
                                    }
                                }
                            >
                                {move || if is_rerunning.get() { "Re-running Query..." } else { "Re-run Query" }}
                            </Button>
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
                    <Icon icon=phosphor_leptos::WARNING attr:class="w-12 h-12 mx-auto mb-4 text-error-foreground/60" />

                    <p class="text-sm text-error-foreground mb-4 font-mono whitespace-pre-wrap break-words">
                        {message}
                    </p>

                    {on_rerun.map(|cb| {
                        view! {
                            <Button
                                disabled=MaybeProp::derive(move || Some(is_rerunning.get()))
                                on:click=move |_| {
                                    if !is_rerunning.get_untracked() {
                                        cb.run(());
                                    }
                                }
                            >
                                {move || if is_rerunning.get() { "Re-running Query..." } else { "Re-run Query" }}
                            </Button>
                        }
                    })}
                </div>
            </div>
        }
        .into_any()
    }
}
