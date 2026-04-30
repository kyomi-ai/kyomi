// SPDX-License-Identifier: AGPL-3.0-or-later

//! Query execution handler — orchestrates the full run-query flow.
//!
//! Mirrors the `handleRunQuery` logic in `apps/frontend/src/components/SQLEditor.jsx`
//! and `onRunQuery` in `apps/frontend/src/pages/SQLEditorPage.jsx`.
//!
//! Flow:
//! 1. Validate inputs (datasource selected, query non-empty)
//! 2. Create a new tab in `Running` state
//! 3. Call `fetch_arrow_buffered()` — returns first page of results as Arrow IPC
//! 4. Update tab with results; subsequent pages fetched on demand
//! 5. Fire-and-forget: save to query history

use leptos::prelude::*;

use super::state::SqlEditorState;
use super::types::{NewTabData, QueryStatus};
#[cfg(target_arch = "wasm32")]
use crate::server_fns::sql_editor::save_query_history;

/// Execute a query: validate, create a tab, run async, update tab on completion.
///
/// This is the main entry point called when the user presses Cmd/Ctrl+Enter or
/// clicks the Run button.
///
/// # Arguments
/// - `state` — the shared SQL editor state (tabs, active tab, etc.)
/// - `query_text` — the SQL to execute (full editor content)
/// - `datasource_slug` — slug of the selected datasource (e.g. "production-postgres")
/// - `datasource_type` — type of the datasource (e.g. "bigquery", "postgres")
/// - `query_running` — write signal to track whether a query is currently executing
pub fn run_query(
    state: SqlEditorState,
    query_text: String,
    datasource_slug: String,
    datasource_type: String,
    query_running: WriteSignal<bool>,
) {
    // ── Validate ─────────────────────────────────────────────────────────

    if datasource_slug.is_empty() {
        tracing::warn!("run_query: no datasource selected");
        return;
    }

    let trimmed = query_text.trim();
    if trimmed.is_empty() {
        tracing::warn!("run_query: empty query text");
        return;
    }

    // ── Create tab in Running state ──────────────────────────────────────

    let tab_id = state.add_tab(NewTabData {
        label: "Query".to_string(),
        query: query_text.clone(),
        status: QueryStatus::Running,
        result: None,
        error: None,
        visualization: None,
        needs_refresh: false,
        datasource_slug: Some(datasource_slug.clone()),
        datasource_type: Some(datasource_type.clone()),
    });

    query_running.set(true);

    // ── Execute via Arrow endpoint ────────────────────────────────────────
    // All datasource types use the same path: POST to /api/v1/query-arrow,
    // receive Arrow IPC bytes, paginate on demand with offset/limit.
    run_arrow_query(state, tab_id, query_text, datasource_slug, datasource_type, query_running);
}

/// Re-run a query in an existing tab (e.g. after "Results expired — click to re-run").
///
/// Unlike [`run_query`] which creates a new tab, this sets the existing tab to
/// Running state and re-executes in place. The `on_complete` callback is called
/// after execution finishes (success or error) so the caller can reset UI state.
pub fn rerun_query(
    state: SqlEditorState,
    tab_id: String,
    query_text: String,
    datasource_slug: String,
    datasource_type: String,
    on_complete: Option<Callback<()>>,
) {
    if datasource_slug.is_empty() {
        tracing::warn!("rerun_query: no datasource context on tab");
        return;
    }
    if query_text.trim().is_empty() {
        tracing::warn!("rerun_query: empty query text");
        return;
    }

    state.update_tab(&tab_id, |tab| {
        tab.status = QueryStatus::Running;
        tab.error = None;
        tab.needs_refresh = false;
    });

    rerun_arrow_query(state, tab_id, query_text, datasource_slug, datasource_type, on_complete);
}

/// Execute a query using the Arrow endpoint (`fetch_arrow_buffered`).
///
/// Calls `POST /api/v1/query-arrow` with `limit` / `offset` and decodes
/// the response as Arrow IPC bytes via `DataTable`.
#[cfg(target_arch = "wasm32")]
fn run_arrow_query(
    state: SqlEditorState,
    tab_id: String,
    query_text: String,
    datasource_slug: String,
    datasource_type: String,
    query_running: WriteSignal<bool>,
) {
    use super::types::{QueryError, QueryHandle, QueryResult};

    let sql = query_text;
    let ds_slug = datasource_slug;

    leptos::task::spawn_local(async move {
        let start = instant_now();

        // Default page size for the first page.
        let page_size = state
            .default_page_size
            .get_untracked();

        match crate::arrow_fetch::fetch_arrow_buffered(
            &ds_slug,
            &sql,
            page_size,
            0,
            true,
            None,
        )
        .await
        {
            Ok(arrow_result) => {
                let frontend_time_ms = elapsed_ms(start);
                let row_count = arrow_result.data.num_rows();
                let total_rows = arrow_result.total_rows.map(|t| t as usize);
                let job_id = arrow_result.job_id.clone();

                let query_handle = QueryHandle {
                    datasource_type: datasource_type.clone(),
                    datasource_slug: ds_slug.clone(),
                    sql: sql.clone(),
                    job_id,
                };

                let result = QueryResult::from_arrow(
                    arrow_result,
                    Some(query_handle),
                    Some(frontend_time_ms),
                );

                let history_row_count = total_rows.unwrap_or(row_count);

                state.update_tab(&tab_id, move |tab| {
                    tab.status = QueryStatus::Success;
                    tab.result = Some(result);
                });

                query_running.set(false);

                save_to_history(
                    state,
                    HistoryRecord {
                        query_text: sql,
                        execution_time_ms: Some(frontend_time_ms as i32),
                        bytes_processed: None,
                        row_count: Some(history_row_count as i32),
                        status: "success".to_string(),
                        error_message: None,
                        datasource: Some(ds_slug),
                    },
                );
            }
            Err(error_msg) => {
                let error_msg_for_history = error_msg.clone();
                state.update_tab(&tab_id, move |tab| {
                    tab.status = QueryStatus::Error;
                    tab.error = Some(QueryError {
                        message: error_msg,
                        code: None,
                        line: None,
                        column: None,
                    });
                });

                query_running.set(false);

                save_to_history(
                    state,
                    HistoryRecord {
                        query_text: sql,
                        execution_time_ms: None,
                        bytes_processed: None,
                        row_count: None,
                        status: "error".to_string(),
                        error_message: Some(error_msg_for_history),
                        datasource: Some(ds_slug),
                    },
                );
            }
        }
    });
}

/// Non-WASM stub — execution only runs in the browser.
#[cfg(not(target_arch = "wasm32"))]
fn run_arrow_query(
    _state: SqlEditorState,
    _tab_id: String,
    _query_text: String,
    _datasource_slug: String,
    _datasource_type: String,
    _query_running: WriteSignal<bool>,
) {
    // No-op on SSR — query execution is WASM-only.
}

/// Re-execute a query into an existing tab (WASM-only).
///
/// Same as `run_arrow_query` but does NOT create a new tab — updates the
/// existing `tab_id` and calls `on_complete` when done so the caller can
/// reset loading state.
#[cfg(target_arch = "wasm32")]
fn rerun_arrow_query(
    state: SqlEditorState,
    tab_id: String,
    query_text: String,
    datasource_slug: String,
    datasource_type: String,
    on_complete: Option<Callback<()>>,
) {
    use super::types::{QueryError, QueryHandle, QueryResult};

    let sql = query_text;
    let ds_slug = datasource_slug;

    leptos::task::spawn_local(async move {
        let start = instant_now();
        let page_size = state.default_page_size.get_untracked();

        match crate::arrow_fetch::fetch_arrow_buffered(&ds_slug, &sql, page_size, 0, true, None)
            .await
        {
            Ok(arrow_result) => {
                let frontend_time_ms = elapsed_ms(start);
                let row_count = arrow_result.data.num_rows();
                let total_rows = arrow_result.total_rows.map(|t| t as usize);
                let job_id = arrow_result.job_id.clone();

                let query_handle = QueryHandle {
                    datasource_type,
                    datasource_slug: ds_slug.clone(),
                    sql: sql.clone(),
                    job_id,
                };

                let result = QueryResult::from_arrow(arrow_result, Some(query_handle), Some(frontend_time_ms));

                state.update_tab(&tab_id, move |tab| {
                    tab.status = QueryStatus::Success;
                    tab.error = None;
                    tab.needs_refresh = false;
                    tab.result = Some(result);
                });

                let history_row_count = total_rows.unwrap_or(row_count);
                save_to_history(
                    state,
                    HistoryRecord {
                        query_text: sql,
                        execution_time_ms: Some(frontend_time_ms as i32),
                        bytes_processed: None,
                        row_count: Some(history_row_count as i32),
                        status: "success".to_string(),
                        error_message: None,
                        datasource: Some(ds_slug),
                    },
                );
            }
            Err(error_msg) => {
                let error_msg_for_history = error_msg.clone();
                state.update_tab(&tab_id, move |tab| {
                    tab.status = QueryStatus::Error;
                    tab.error = Some(QueryError {
                        message: error_msg,
                        code: None,
                        line: None,
                        column: None,
                    });
                });

                save_to_history(
                    state,
                    HistoryRecord {
                        query_text: sql,
                        execution_time_ms: None,
                        bytes_processed: None,
                        row_count: None,
                        status: "error".to_string(),
                        error_message: Some(error_msg_for_history),
                        datasource: Some(ds_slug),
                    },
                );
            }
        }

        if let Some(cb) = on_complete {
            cb.run(());
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn rerun_arrow_query(
    _state: SqlEditorState,
    _tab_id: String,
    _query_text: String,
    _datasource_slug: String,
    _datasource_type: String,
    _on_complete: Option<Callback<()>>,
) {
}

// ─── WASM-only helpers ──────────────────────────────────────────────────────

/// A single query-history entry to be written asynchronously.
///
/// Bundled as a struct so [`save_to_history`] stays under clippy's
/// `too_many_arguments` threshold.
#[cfg(target_arch = "wasm32")]
pub(super) struct HistoryRecord {
    pub query_text: String,
    pub execution_time_ms: Option<i32>,
    pub bytes_processed: Option<i64>,
    pub row_count: Option<i32>,
    pub status: String,
    pub error_message: Option<String>,
    pub datasource: Option<String>,
}

/// Fire-and-forget helper to save a query execution to history.
///
/// After the server acknowledges the write, bumps `state.history_refresh_tick`
/// so the sidebar's QueryHistory panel refetches and shows this run without a
/// page reload. The tick is bumped even on save failure — the row in the tab's
/// result area is the source of truth for the user, and the tick bump is cheap;
/// if the save itself failed the history list just won't contain a new row,
/// which is the desired behaviour.
#[cfg(target_arch = "wasm32")]
pub(super) fn save_to_history(state: SqlEditorState, record: HistoryRecord) {
    leptos::task::spawn_local(async move {
        let _ = save_query_history(
            record.query_text,
            record.execution_time_ms,
            record.bytes_processed,
            record.row_count,
            record.status,
            record.error_message,
            record.datasource,
        )
        .await;
        state.history_refresh_tick.update(|n| *n += 1);
    });
}

/// Get an "instant" timestamp for measuring elapsed time.
#[cfg(target_arch = "wasm32")]
fn instant_now() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

/// Compute elapsed milliseconds since `start`.
#[cfg(target_arch = "wasm32")]
fn elapsed_ms(start: f64) -> u64 {
    let now = web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0);
    (now - start).round() as u64
}
