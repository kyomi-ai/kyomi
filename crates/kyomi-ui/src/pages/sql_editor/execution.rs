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
//!
//! ## Action-based execution
//!
//! Both run-query paths dispatch a context-provided [`RunQueryContext`] action
//! rather than spawning raw futures.  The Actions are created in `SqlEditorPage`
//! (which has reactive / component scope) and provided via Leptos context.
//! Effects in that same component scope consume the results and update tab
//! state — ensuring no signal access can outlive the owning scope.
//!
//! All Action types are gated under `#[cfg(target_arch = "wasm32")]` because:
//! - They exist purely to orchestrate browser-side async work.
//! - `fetch_arrow_buffered` and `save_query_history` are themselves WASM-only.
//! - On SSR, `run_query` / `rerun_query` are no-ops (they return early).

use leptos::prelude::*;

use super::state::SqlEditorState;

// ─── WASM-only types and implementations ────────────────────────────────────

#[cfg(target_arch = "wasm32")]
use super::types::{NewTabData, QueryStatus};
#[cfg(target_arch = "wasm32")]
use crate::server_fns::sql_editor::save_query_history;

// ─── Action input / output types (WASM-only) ────────────────────────────────

/// Input for the run-query action (new-tab path).
///
/// All values are captured at dispatch time so the result-handling Effect sees
/// exactly what the server received, not live signal values that may have
/// changed during the async call.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Debug)]
pub struct RunQueryInput {
    pub tab_id: String,
    pub sql: String,
    pub datasource_slug: String,
    pub datasource_type: String,
    pub page_size: u32,
}

/// Input for the rerun-query action (update-existing-tab path).
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Debug)]
pub struct RerunQueryInput {
    pub tab_id: String,
    pub sql: String,
    pub datasource_slug: String,
    pub datasource_type: String,
    pub page_size: u32,
    /// Callback invoked after execution completes (success or error).
    pub on_complete: Option<Callback<()>>,
}

/// Outcome of a query execution, returned through the Action result signal so
/// the Effect in `SqlEditorPage` can apply tab state updates using the
/// dispatch-time values (not live signals).
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Debug)]
pub struct QueryOutcome {
    pub tab_id: String,
    pub result: Result<QuerySuccess, String>,
    /// The history record to save fire-and-forget.  Bundled here so the Effect
    /// can hand it off to [`save_to_history`] without repeating the field logic.
    ///
    /// Visibility is `pub(super)` so the Effect in `SqlEditorPage` — which is in
    /// the same `sql_editor` module — can pass it straight to `save_to_history`.
    pub(super) history_record: HistoryRecord,
}

/// The success payload returned inside [`QueryOutcome::result`].
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Debug)]
pub struct QuerySuccess {
    pub query_result: super::types::QueryResult,
}

/// Context struct that holds the two query-execution Actions.
///
/// Created in `SqlEditorPage` via [`build_run_action`] / [`build_rerun_action`]
/// and consumed by `run_query` / `rerun_query` via [`RunQueryContext::use_context`].
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy)]
pub struct RunQueryContext {
    pub run_action: Action<RunQueryInput, QueryOutcome>,
    pub rerun_action: Action<RerunQueryInput, QueryOutcome>,
}

#[cfg(target_arch = "wasm32")]
impl RunQueryContext {
    /// Retrieve the context provided by [`SqlEditorPage`].
    ///
    /// # Panics
    ///
    /// Panics if called outside a component tree that called
    /// `RunQueryContext::provide(...)` first.
    pub fn use_context() -> Self {
        use_context::<Self>()
            .expect("RunQueryContext not provided — call RunQueryContext::provide() from SqlEditorPage first")
    }
}

// ─── Public entry points ─────────────────────────────────────────────────────

/// Execute a query: validate, create a tab, dispatch the run action.
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
    // Query execution only runs in the browser — on SSR this is a no-op.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (state, query_text, datasource_slug, datasource_type, query_running);
        return;
    }

    #[cfg(target_arch = "wasm32")]
    {
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

        // ── Guard: reject if a query is already in flight ────────────────
        let ctx = RunQueryContext::use_context();
        if ctx.run_action.pending().get_untracked() {
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

        let page_size = state.default_page_size.get_untracked();
        ctx.run_action.dispatch(RunQueryInput {
            tab_id,
            sql: query_text,
            datasource_slug,
            datasource_type,
            page_size,
        });
    }
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
    // Query execution only runs in the browser — on SSR this is a no-op.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (state, tab_id, query_text, datasource_slug, datasource_type, on_complete);
        return;
    }

    #[cfg(target_arch = "wasm32")]
    {
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

        let ctx = RunQueryContext::use_context();
        let page_size = state.default_page_size.get_untracked();
        ctx.rerun_action.dispatch(RerunQueryInput {
            tab_id,
            sql: query_text,
            datasource_slug,
            datasource_type,
            page_size,
            on_complete,
        });
    }
}

// ─── Action factories (WASM-only) ────────────────────────────────────────────

/// Build the run-query Action for use in `SqlEditorPage`.
///
/// Must be called inside a reactive owner (component scope) because
/// [`Action::new_unsync_local`] registers with the current owner.
///
/// Uses `new_unsync_local` rather than `new` because `fetch_arrow_buffered`
/// uses `!Send` browser APIs (`JsFuture`, `web_sys`, `wasm_bindgen_futures`).
///
/// The returned Action wraps the entire `fetch_arrow_buffered` call.  Signal
/// writes happen in the Effect that watches `action.value()`, NOT inside the
/// async block — keeping the async closure free of reactive graph access and
/// preventing disposal races.
#[cfg(target_arch = "wasm32")]
pub fn build_run_action() -> Action<RunQueryInput, QueryOutcome> {
    Action::new_unsync_local(|input: &RunQueryInput| {
        let tab_id = input.tab_id.clone();
        let sql = input.sql.clone();
        let ds_slug = input.datasource_slug.clone();
        let datasource_type = input.datasource_type.clone();
        let page_size = input.page_size;

        async move {
            use super::types::{QueryHandle, QueryResult};

            let start = instant_now();

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

                    let query_result = QueryResult::from_arrow(
                        arrow_result,
                        Some(query_handle),
                        Some(frontend_time_ms),
                    );

                    let history_row_count = total_rows.unwrap_or(row_count);

                    QueryOutcome {
                        tab_id,
                        result: Ok(QuerySuccess { query_result }),
                        history_record: HistoryRecord {
                            query_text: sql,
                            execution_time_ms: Some(frontend_time_ms as i32),
                            bytes_processed: None,
                            row_count: Some(history_row_count as i32),
                            status: "success".to_string(),
                            error_message: None,
                            datasource: Some(ds_slug),
                        },
                    }
                }
                Err(error_msg) => {
                    let err_for_history = error_msg.clone();
                    QueryOutcome {
                        tab_id,
                        result: Err(error_msg),
                        history_record: HistoryRecord {
                            query_text: sql,
                            execution_time_ms: None,
                            bytes_processed: None,
                            row_count: None,
                            status: "error".to_string(),
                            error_message: Some(err_for_history),
                            datasource: Some(ds_slug),
                        },
                    }
                }
            }
        }
    })
}

/// Build the rerun-query Action for use in `SqlEditorPage`.
///
/// Same structure as [`build_run_action`] but does NOT create a new tab —
/// it updates the existing tab identified by `tab_id` and calls `on_complete`
/// after the async work is done.
#[cfg(target_arch = "wasm32")]
pub fn build_rerun_action() -> Action<RerunQueryInput, QueryOutcome> {
    Action::new_unsync_local(|input: &RerunQueryInput| {
        let tab_id = input.tab_id.clone();
        let sql = input.sql.clone();
        let ds_slug = input.datasource_slug.clone();
        let datasource_type = input.datasource_type.clone();
        let page_size = input.page_size;
        let on_complete = input.on_complete;

        async move {
            use super::types::{QueryHandle, QueryResult};

            let start = instant_now();

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
                        datasource_type,
                        datasource_slug: ds_slug.clone(),
                        sql: sql.clone(),
                        job_id,
                    };

                    let query_result = QueryResult::from_arrow(
                        arrow_result,
                        Some(query_handle),
                        Some(frontend_time_ms),
                    );

                    let history_row_count = total_rows.unwrap_or(row_count);

                    if let Some(cb) = on_complete {
                        cb.try_run(());
                    }

                    QueryOutcome {
                        tab_id,
                        result: Ok(QuerySuccess { query_result }),
                        history_record: HistoryRecord {
                            query_text: sql,
                            execution_time_ms: Some(frontend_time_ms as i32),
                            bytes_processed: None,
                            row_count: Some(history_row_count as i32),
                            status: "success".to_string(),
                            error_message: None,
                            datasource: Some(ds_slug),
                        },
                    }
                }
                Err(error_msg) => {
                    let err_for_history = error_msg.clone();

                    if let Some(cb) = on_complete {
                        cb.try_run(());
                    }

                    QueryOutcome {
                        tab_id,
                        result: Err(error_msg),
                        history_record: HistoryRecord {
                            query_text: sql,
                            execution_time_ms: None,
                            bytes_processed: None,
                            row_count: None,
                            status: "error".to_string(),
                            error_message: Some(err_for_history),
                            datasource: Some(ds_slug),
                        },
                    }
                }
            }
        }
    })
}

// ─── WASM-only helpers ──────────────────────────────────────────────────────

/// A single query-history entry to be written asynchronously.
///
/// Bundled as a struct so [`save_to_history`] stays under clippy's
/// `too_many_arguments` threshold.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Debug)]
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
        state.history_refresh_tick.try_update(|n| *n += 1);
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
