// SPDX-License-Identifier: AGPL-3.0-or-later

//! Query execution handler — orchestrates the full run-query flow.
//!
//! Mirrors the `handleRunQuery` logic in `apps/frontend/src/components/SQLEditor.jsx`
//! and `onRunQuery` in `apps/frontend/src/pages/SQLEditorPage.jsx`.
//!
//! Flow:
//! 1. Validate inputs (datasource selected, query non-empty)
//! 2. Create a new tab in `Running` state
//! 3. Call `execute_sql_query()` — returns first page of results
//! 4. Update tab with results; subsequent pages fetched on demand
//! 5. Fire-and-forget: save to query history

use leptos::prelude::*;

use super::state::SqlEditorState;
use super::types::{NewTabData, QueryError, QueryStatus};
use crate::server_fns::sql_editor::{execute_sql_query, save_query_history};

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

    // ── Execute via paginated server function ─────────────────────────────
    // All datasource types use the same path: execute server-side, return
    // one page of results, paginate on demand. No WebSocket streaming.
    run_paginated_query(state, tab_id, query_text, datasource_slug, query_running);
}

/// Execute a paginated query — calls `execute_sql_query` server function.
fn run_paginated_query(
    state: SqlEditorState,
    tab_id: String,
    query_text: String,
    datasource_slug: String,
    query_running: WriteSignal<bool>,
) {
    let sql = query_text.clone();
    let ds_slug = datasource_slug.clone();

    leptos::task::spawn_local(async move {
        let start = instant_now();

        match execute_sql_query(ds_slug.clone(), sql.clone(), 50, 1).await {
            Ok(result) => {
                let frontend_time_ms = elapsed_ms(start);
                let execution_time = result.execution_time.or(Some(frontend_time_ms));
                let _bytes_processed = result.bytes_processed;
                let row_count = result.row_count;
                let total_rows = result.total_rows;

                let history_result = result.clone();
                state.update_tab(&tab_id, move |tab| {
                    tab.status = QueryStatus::Success;
                    tab.result = Some(result);
                    // Overwrite execution_time with the best available value.
                    if let Some(ref mut r) = tab.result {
                        r.execution_time = execution_time;
                    }
                });

                query_running.set(false);

                // Fire-and-forget: save success to history, then bump the
                // shared history refresh tick so the sidebar's QueryHistory
                // panel refetches and shows this run without a page reload.
                save_to_history(
                    state,
                    HistoryRecord {
                        query_text: sql,
                        execution_time_ms: execution_time.map(|t| t as i32),
                        bytes_processed: history_result.bytes_processed.map(|b| b as i64),
                        row_count: Some(total_rows.unwrap_or(row_count) as i32),
                        status: "success".to_string(),
                        error_message: None,
                        datasource: Some(ds_slug),
                    },
                );
            }
            Err(e) => {
                let error_msg = e.to_string();
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

                // Fire-and-forget: save error to history, then bump the
                // sidebar refresh tick (same reason as the success path —
                // failed queries still appear in the history list).
                save_to_history(
                    state,
                    HistoryRecord {
                        query_text: sql,
                        execution_time_ms: None,
                        bytes_processed: None,
                        row_count: None,
                        status: "error".to_string(),
                        error_message: Some(e.to_string()),
                        datasource: Some(ds_slug),
                    },
                );
            }
        }
    });
}

/// A single query-history entry to be written asynchronously.
///
/// Bundled as a struct so [`save_to_history`] stays under clippy's
/// `too_many_arguments` threshold.
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
/// so the sidebar's QueryHistory panel refetches and shows the new entry
/// without a page reload. The tick is bumped even on save failure — the row
/// in the tab's result area is the source of truth for the user, and the
/// tick bump is cheap; if the save itself failed the history list just won't
/// contain a new row, which is the desired behaviour.
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

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Generate a UUID-like request ID for correlating WebSocket events.
///
/// Uses `crypto.randomUUID()` on WASM, falling back to a timestamp-based ID.
fn _generate_request_id() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(crypto) = web_sys::window().and_then(|w| w.crypto().ok()) {
            return crypto.random_uuid();
        }
        // Fallback: timestamp + random suffix (matches React fallback).
        let now = js_sys::Date::now() as u64;
        let rand = js_sys::Math::random();
        format!("{now}-{rand:.10}")
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("ssr-req-{n}")
    }
}

/// Get an "instant" timestamp for measuring elapsed time.
fn instant_now() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.performance())
            .map(|p| p.now())
            .unwrap_or(0.0)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        0.0
    }
}

/// Compute elapsed milliseconds since `start`.
fn elapsed_ms(start: f64) -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        let now = web_sys::window()
            .and_then(|w| w.performance())
            .map(|p| p.now())
            .unwrap_or(0.0);
        (now - start).round() as u64
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = start;
        0
    }
}
