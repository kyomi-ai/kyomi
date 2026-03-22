// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for SQL Editor query execution, dry-run validation, and streaming.
//!
//! These replace the REST API calls:
//! - `POST /api/v1/datasources/query/execute` → `execute_sql_query()` / `fetch_query_page()`
//! - `POST /api/v1/datasources/query/execute` (dry_run=true) → `dry_run_sql()`
//! - `POST /api/v1/datasources/query/stream` → `start_query_stream()`

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, workspace_id};

use crate::pages::sql_editor::types::{ColumnMetadata, QueryHandle, QueryResult};

// ---------------------------------------------------------------------------
// Response types (server-fn-specific — not shared with client-side state)
// ---------------------------------------------------------------------------

/// Result from dry-run query validation.
///
/// Returned by the `dry_run_sql` server function. Includes error location
/// information for inline editor markers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DryRunResult {
    /// `true` if the query is syntactically valid.
    pub valid: bool,
    /// Provider-formatted message to display in the status bar.
    pub message: String,
    /// Error line number (1-indexed). `None` if valid or location unavailable.
    pub line: Option<u32>,
    /// Error column number. `None` if valid or location unavailable.
    pub column: Option<u32>,
    /// Bytes that would be processed (BigQuery only). `None` for other providers.
    pub bytes_processed: Option<u64>,
}

/// Result from starting a streaming query.
///
/// The actual data arrives via WebSocket events (`query_stream_header`,
/// `query_stream_chunk`, `query_stream_complete`, `query_stream_error`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamStartResult {
    /// Always `"streaming"` on success.
    pub status: String,
    /// The request ID that correlates WebSocket events to this query.
    pub request_id: String,
}

// ---------------------------------------------------------------------------
// Task 1.3: execute_sql_query — paginated query execution
// ---------------------------------------------------------------------------

/// Execute a SQL query with server-side pagination.
///
/// Replaces `POST /api/v1/datasources/query/execute` for the Leptos SQL Editor.
/// Returns typed `QueryResult` with column metadata, rows, and a `QueryHandle`
/// for subsequent page fetches. Always requests total row count on first page.
#[server(prefix = "/leptos-api")]
pub async fn execute_sql_query(
    datasource_slug: String,
    sql: String,
    page_size: u32,
    page: u32,
) -> Result<QueryResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let (ds, provider) =
        super::datasources::create_query_provider(&ctx, &auth, ws_id, &datasource_slug).await?;

    // First-page execution always requests the total row count.
    let result = run_paginated_query(&*provider, &sql, page_size, page, true).await?;

    provider_result_to_query_result(result.0, result.1, &ds.slug, ds.datasource_type.as_ref(), &sql)
}

// ---------------------------------------------------------------------------
// Task 1.3: fetch_query_page — subsequent page fetches
// ---------------------------------------------------------------------------

/// Fetch a specific page of query results.
///
/// For BigQuery, `job_id` enables instant random page access without
/// re-executing the query. For all other providers, re-executes with
/// LIMIT/OFFSET. `include_total` controls whether the provider computes the
/// total row count (defaults to `false` to avoid expensive COUNT queries on
/// subsequent pages).
#[server(prefix = "/leptos-api")]
pub async fn fetch_query_page(
    datasource_slug: String,
    sql: String,
    page: u32,
    page_size: u32,
    job_id: Option<String>,
    include_total: Option<bool>,
) -> Result<QueryResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // TODO: When BigQuery job_id pagination is supported at the provider level,
    // use `job_id` to fetch the page directly instead of re-executing.
    // For now, all providers use the same LIMIT/OFFSET re-execution path.
    let _ = job_id;

    let (ds, provider) =
        super::datasources::create_query_provider(&ctx, &auth, ws_id, &datasource_slug).await?;

    let include_total = include_total.unwrap_or(false);
    let result = run_paginated_query(&*provider, &sql, page_size, page, include_total).await?;

    provider_result_to_query_result(result.0, result.1, &ds.slug, ds.datasource_type.as_ref(), &sql)
}

// ---------------------------------------------------------------------------
// Shared helper: execute a paginated query with timeout and error handling
// ---------------------------------------------------------------------------

/// Execute a paginated query against a provider, handling timeout and error
/// status. Returns the provider `QueryResult` and elapsed time in milliseconds.
///
/// Consolidates the shared execution logic from `execute_sql_query` and
/// `fetch_query_page`.
///
/// Page size is clamped to 1..=1000 (matching the REST handler's limit for
/// paginated table display). The streaming path (`start_query_stream`) has a
/// separate higher limit of 10,000 rows since it streams progressively.
#[cfg(feature = "ssr")]
async fn run_paginated_query(
    provider: &dyn kyomi_datasource_server::DatasourceProvider,
    sql: &str,
    page_size: u32,
    page: u32,
    include_total: bool,
) -> Result<(kyomi_datasource_server::QueryResult, u64), ServerFnError> {
    let page_size = page_size.clamp(1, 1000);
    let offset = page.saturating_sub(1) * page_size;

    let start = std::time::Instant::now();

    let result = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_QUERY,
        provider.execute_query(sql, Some(page_size), Some(offset), include_total),
    )
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            provider.close().await;
            return Err(ServerFnError::new(format!("Query failed: {e}")));
        }
        Err(_) => {
            provider.close().await;
            return Err(ServerFnError::new(format!(
                "Query timed out after {} seconds",
                kyomi_datasource_server::DATASOURCE_TIMEOUT_QUERY.as_secs()
            )));
        }
    };

    provider.close().await;

    if result.status == kyomi_datasource_server::QueryStatus::Error {
        return Err(ServerFnError::new(
            result
                .error
                .unwrap_or_else(|| "Query execution failed".to_string()),
        ));
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;

    Ok((result, elapsed_ms))
}

// ---------------------------------------------------------------------------
// Task 1.4: dry_run_sql — SQL validation without execution
// ---------------------------------------------------------------------------

/// Validate SQL syntax without executing the query.
///
/// Uses database-native mechanisms (e.g., BigQuery `dryRun: true`, PostgreSQL
/// `EXPLAIN`, SQL Server `SET NOEXEC ON`) to check syntax and estimate cost.
#[server(prefix = "/leptos-api")]
pub async fn dry_run_sql(
    datasource_slug: String,
    sql: String,
) -> Result<DryRunResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let (_ds, provider) =
        super::datasources::create_query_provider(&ctx, &auth, ws_id, &datasource_slug).await?;

    let result = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_DRY_RUN,
        provider.dry_run(&sql),
    )
    .await
    {
        Ok(Ok(dr)) => DryRunResult {
            valid: dr.valid,
            message: dr.message,
            line: dr.line,
            column: dr.column,
            // bytes_processed is not part of the driver DryRunResult;
            // BigQuery returns it in the message string. Future enhancement
            // could parse it out, but for now we leave it as None.
            bytes_processed: None,
        },
        Ok(Err(e)) => DryRunResult {
            valid: false,
            message: format!("Validation failed: {e}"),
            line: None,
            column: None,
            bytes_processed: None,
        },
        Err(_) => DryRunResult {
            valid: false,
            message: "SQL validation timed out".to_string(),
            line: None,
            column: None,
            bytes_processed: None,
        },
    };

    provider.close().await;

    Ok(result)
}

// ---------------------------------------------------------------------------
// Task 1.5: start_query_stream — streaming query via WebSocket
// ---------------------------------------------------------------------------

/// Start a streaming query execution.
///
/// Returns immediately with a `StreamStartResult` containing the `request_id`.
/// The actual data is delivered via WebSocket events:
/// - `query_stream_header` — column metadata + optional total row count
/// - `query_stream_chunk` — batches of rows
/// - `query_stream_complete` — execution time, bytes processed, totals
/// - `query_stream_error` — error message if the query fails
///
/// The `request_id` correlates all WebSocket events to this specific query
/// execution, allowing the frontend to match events to the correct tab.
#[server(prefix = "/leptos-api")]
pub async fn start_query_stream(
    datasource_slug: String,
    sql: String,
    request_id: String,
    limit: Option<u32>,
) -> Result<StreamStartResult, ServerFnError> {
    use futures_util::StreamExt;

    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let (_ds, provider) =
        super::datasources::create_query_provider(&ctx, &auth, ws_id, &datasource_slug).await?;

    // Clamp limit to 1..=10000, default 10000 if None.
    let clamped_limit = limit.unwrap_or(10_000).clamp(1, 10_000);

    // Use client-provided request ID if non-empty, otherwise generate one.
    let request_id = if request_id.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        request_id
    };

    let user_id = auth.user_id.clone();
    let ws_manager = ctx.ws_manager.clone();
    let rid = request_id.clone();

    // Spawn the streaming task — runs independently, sends WS messages as
    // events arrive from the provider.
    tokio::spawn(async move {
        let start = std::time::Instant::now();

        let mut stream = match provider
            .execute_query_stream(
                &sql,
                Some(clamped_limit),
                None,  // no offset
                true,  // include total row count
                None,  // default chunk size
            )
            .await
        {
            Ok(s) => s,
            Err(e) => {
                let msg = kyomi_core::WebSocketMessage::new(
                    kyomi_core::MessageType::QueryStreamError,
                )
                .with_data(serde_json::json!({
                    "request_id": rid,
                    "error": e.to_string(),
                }));
                ws_manager.send_to_user(&user_id, msg).await;
                return;
            }
        };

        // Per-event timeout: if no event arrives within 120s, the datasource
        // is likely disconnected or the query is hung.
        let event_timeout = std::time::Duration::from_secs(120);
        let mut completed = false;

        loop {
            let next = tokio::time::timeout(event_timeout, stream.next()).await;

            let event = match next {
                Ok(Some(event)) => event,
                Ok(None) => break, // Stream ended
                Err(_) => {
                    // Timeout waiting for next event
                    let msg = kyomi_core::WebSocketMessage::new(
                        kyomi_core::MessageType::QueryStreamError,
                    )
                    .with_data(serde_json::json!({
                        "request_id": rid,
                        "error": "Query timed out — the datasource may be disconnected or the query is taking too long",
                    }));
                    ws_manager.send_to_user(&user_id, msg).await;
                    break;
                }
            };

            match event {
                Ok(kyomi_connect_protocol::QueryStreamEvent::Header {
                    columns,
                    total_rows,
                }) => {
                    let cols: Vec<serde_json::Value> = columns
                        .iter()
                        .map(|c| {
                            serde_json::json!({
                                "name": c.name,
                                "type": c.col_type.as_str(),
                            })
                        })
                        .collect();

                    let msg = kyomi_core::WebSocketMessage::new(
                        kyomi_core::MessageType::QueryStreamHeader,
                    )
                    .with_data(serde_json::json!({
                        "request_id": rid,
                        "columns": cols,
                        "total_rows": total_rows,
                    }));
                    ws_manager.send_to_user(&user_id, msg).await;
                }
                Ok(kyomi_connect_protocol::QueryStreamEvent::Chunk {
                    rows,
                    chunk_index,
                }) => {
                    let msg = kyomi_core::WebSocketMessage::new(
                        kyomi_core::MessageType::QueryStreamChunk,
                    )
                    .with_data(serde_json::json!({
                        "request_id": rid,
                        "rows": rows,
                        "chunk_index": chunk_index,
                    }));
                    ws_manager.send_to_user(&user_id, msg).await;
                }
                Ok(kyomi_connect_protocol::QueryStreamEvent::Complete {
                    execution_time_ms,
                    bytes_processed,
                    total_chunks,
                    total_rows_returned,
                }) => {
                    completed = true;
                    let elapsed = start.elapsed().as_millis() as i64;
                    let msg = kyomi_core::WebSocketMessage::new(
                        kyomi_core::MessageType::QueryStreamComplete,
                    )
                    .with_data(serde_json::json!({
                        "request_id": rid,
                        "execution_time_ms": execution_time_ms.unwrap_or(elapsed),
                        "bytes_processed": bytes_processed,
                        "total_chunks": total_chunks,
                        "total_rows_returned": total_rows_returned,
                    }));
                    ws_manager.send_to_user(&user_id, msg).await;
                    break;
                }
                Err(e) => {
                    let msg = kyomi_core::WebSocketMessage::new(
                        kyomi_core::MessageType::QueryStreamError,
                    )
                    .with_data(serde_json::json!({
                        "request_id": rid,
                        "error": e.to_string(),
                    }));
                    ws_manager.send_to_user(&user_id, msg).await;
                    break;
                }
            }
        }

        // If the stream ended without a Complete event (e.g., datasource
        // disconnected mid-query, handler dropped the channel), notify the
        // frontend so it doesn't stay stuck in "streaming" state.
        if !completed {
            let msg = kyomi_core::WebSocketMessage::new(
                kyomi_core::MessageType::QueryStreamError,
            )
            .with_data(serde_json::json!({
                "request_id": rid,
                "error": "Query stream ended unexpectedly — the datasource may have disconnected",
            }));
            ws_manager.send_to_user(&user_id, msg).await;
        }

        provider.close().await;
    });

    Ok(StreamStartResult {
        status: "streaming".to_string(),
        request_id,
    })
}

// ---------------------------------------------------------------------------
// Shared helper: convert provider QueryResult to our typed QueryResult
// ---------------------------------------------------------------------------

/// Convert a `kyomi_datasource_server::QueryResult` into the SQL editor's
/// `QueryResult` type, mapping column metadata and building a pagination handle.
#[cfg(feature = "ssr")]
fn provider_result_to_query_result(
    result: kyomi_datasource_server::QueryResult,
    elapsed_ms: u64,
    datasource_slug: &str,
    datasource_type: &str,
    sql: &str,
) -> Result<QueryResult, ServerFnError> {
    let columns: Vec<ColumnMetadata> = result
        .columns
        .unwrap_or_default()
        .into_iter()
        .map(|c| ColumnMetadata {
            name: c.name,
            col_type: Some(c.col_type.as_str().to_string()),
            mode: None,
        })
        .collect();

    let rows = result.rows.unwrap_or_default();
    let row_count = rows.len();

    let total_rows = result.total_rows.map(|t| t as usize);

    let query_handle = QueryHandle {
        datasource_type: datasource_type.to_string(),
        datasource_slug: datasource_slug.to_string(),
        sql: sql.to_string(),
        job_id: None, // BigQuery job_id support is a future enhancement
    };

    let execution_time = result
        .execution_time_ms
        .map(|t| t as u64)
        .or(Some(elapsed_ms));

    let bytes_processed = result.bytes_processed.map(|b| b as u64);

    Ok(QueryResult {
        columns,
        rows,
        row_count,
        total_rows,
        query_handle: Some(query_handle),
        execution_time,
        bytes_processed,
        has_more: result.has_more,
    })
}
