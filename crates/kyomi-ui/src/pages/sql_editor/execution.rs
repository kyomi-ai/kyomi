// SPDX-License-Identifier: AGPL-3.0-or-later

//! Query execution handler — orchestrates the full run-query flow.
//!
//! Mirrors the `handleRunQuery` logic in `apps/frontend/src/components/SQLEditor.jsx`
//! and `onRunQuery` in `apps/frontend/src/pages/SQLEditorPage.jsx`.
//!
//! Flow:
//! 1. Validate inputs (datasource selected, query non-empty)
//! 2. Create a new tab in `Running` state
//! 3. Choose streaming vs paginated execution based on datasource type
//! 4. For streaming: call `start_query_stream()` — WebSocket handler updates the tab
//! 5. For paginated (BigQuery): call `execute_sql_query()` and update the tab directly
//! 6. Fire-and-forget: save to query history

use leptos::prelude::*;

use super::state::SqlEditorState;
use super::types::{NewTabData, QueryError, QueryResult, QueryStatus};
use crate::server_fns::sql_editor::{execute_sql_query, save_query_history, start_query_stream};

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

    // ── Choose execution path ────────────────────────────────────────────
    // Streaming for non-BigQuery datasources (matches React logic).
    let use_streaming = datasource_type != "bigquery";

    if use_streaming {
        run_streaming_query(state, tab_id, query_text, datasource_slug, query_running);
    } else {
        run_paginated_query(state, tab_id, query_text, datasource_slug, query_running);
    }
}

/// Start a streaming query — calls `start_query_stream()` and lets the
/// WebSocket handler (in `streaming.rs`) update the tab progressively.
fn run_streaming_query(
    state: SqlEditorState,
    tab_id: String,
    query_text: String,
    datasource_slug: String,
    query_running: WriteSignal<bool>,
) {
    let sql = query_text.clone();
    let ds_slug = datasource_slug.clone();

    // Generate a request ID client-side so the WebSocket handler can
    // filter events immediately — avoids a race where the WS event
    // arrives before the HTTP response returns.
    let request_id = generate_request_id();

    // Store the request_id on the tab SYNCHRONOUSLY (before spawning the
    // async HTTP call) so the WebSocket handler can correlate events the
    // instant they arrive.
    let rid = request_id.clone();
    state.update_tab(&tab_id, move |tab| {
        tab.status = QueryStatus::Streaming;
        tab.result = Some(QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
            row_count: 0,
            total_rows: None,
            query_handle: Some(super::types::QueryHandle {
                datasource_type: String::new(),
                datasource_slug: String::new(),
                sql: String::new(),
                job_id: Some(rid),
            }),
            execution_time: None,
            bytes_processed: None,
            has_more: false,
        });
    });

    leptos::task::spawn_local(async move {
        match start_query_stream(ds_slug, sql.clone(), request_id, Some(10_000)).await {
            Ok(_stream_result) => {
                // Stream started successfully. The WebSocket handler in
                // streaming.rs will update the tab as events arrive.
                // query_running will be cleared by the streaming handler
                // on complete/error.
            }
            Err(e) => {
                // HTTP request to start the stream failed.
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

                // Fire-and-forget: save error to history.
                save_to_history(sql, None, None, None, "error", Some(e.to_string()), Some(datasource_slug));
            }
        }
    });
}

/// Execute a paginated (non-streaming) query — used for BigQuery.
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

                // Fire-and-forget: save success to history.
                save_to_history(
                    sql,
                    execution_time.map(|t| t as i32),
                    history_result.bytes_processed.map(|b| b as i64),
                    Some(total_rows.unwrap_or(row_count) as i32),
                    "success",
                    None,
                    Some(ds_slug),
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

                // Fire-and-forget: save error to history.
                save_to_history(
                    sql,
                    None,
                    None,
                    None,
                    "error",
                    Some(e.to_string()),
                    Some(ds_slug),
                );
            }
        }
    });
}

/// Fire-and-forget helper to save a query execution to history.
///
/// Matches the React `finally` block in `SQLEditorPage.handleRunQuery`.
pub(super) fn save_to_history(
    query_text: String,
    execution_time_ms: Option<i32>,
    bytes_processed: Option<i64>,
    row_count: Option<i32>,
    status: &str,
    error_message: Option<String>,
    datasource: Option<String>,
) {
    let status = status.to_string();
    leptos::task::spawn_local(async move {
        let _ = save_query_history(
            query_text,
            execution_time_ms,
            bytes_processed,
            row_count,
            status,
            error_message,
            datasource,
        )
        .await;
    });
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Generate a UUID-like request ID for correlating WebSocket events.
///
/// Uses `crypto.randomUUID()` on WASM, falling back to a timestamp-based ID.
fn generate_request_id() -> String {
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
