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

use crate::pages::sql_editor::types::{
    CatalogNode, CatalogNodeType, ColumnMetadata, QueryHandle, QueryHistoryEntry, QueryResult,
};

// ---------------------------------------------------------------------------
// Response types (server-fn-specific — not shared with client-side state)
// ---------------------------------------------------------------------------

/// Result from dry-run query validation.
///
/// Returned by the `dry_run_sql` server function. Includes error location
/// information for inline editor markers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

// ===========================================================================
// Task 1.6: SQL history server functions
// ===========================================================================

// ---------------------------------------------------------------------------
// list_query_history — paginated query history with search/filter
// ---------------------------------------------------------------------------

/// List query history for the current user.
///
/// Supports text search on query_text, filtering to saved-only, and
/// pagination via limit/offset.
#[server(prefix = "/leptos-api")]
pub async fn list_query_history(
    search: Option<String>,
    saved_only: Option<bool>,
    limit: u32,
    offset: u32,
) -> Result<Vec<QueryHistoryEntry>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let records = kyomi_auth::sql_history_service::list_query_history(
        &ctx.db,
        ws_id,
        &auth.user_id,
        limit.clamp(1, 1000) as i64,
        offset as i64,
        saved_only.unwrap_or(false),
        search.as_deref(),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let entries: Vec<QueryHistoryEntry> = records
        .into_iter()
        .map(|(h, slug)| QueryHistoryEntry {
            id: h.query_id,
            query_text: h.query_text,
            execution_time_ms: h.execution_time_ms,
            bytes_processed: h.bytes_processed,
            row_count: h.row_count,
            status: h.status,
            error_message: h.error_message,
            datasource: slug,
            is_saved: h.is_saved,
            created_at: h.executed_at.to_rfc3339(),
        })
        .collect();

    Ok(entries)
}

// ---------------------------------------------------------------------------
// save_query_history — create a new history entry
// ---------------------------------------------------------------------------

/// Save a new query history entry.
///
/// If `datasource` slug is provided, resolves it to a datasource_config_id.
/// Returns the new query_id.
#[server(prefix = "/leptos-api")]
pub async fn save_query_history(
    query_text: String,
    execution_time_ms: Option<i32>,
    bytes_processed: Option<i64>,
    row_count: Option<i32>,
    status: String,
    error_message: Option<String>,
    datasource: Option<String>,
) -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Resolve datasource slug to ID if provided.
    let datasource_config_id = if let Some(ref slug) = datasource {
        let ds = kyomi_auth::datasource_service::get_datasource_by_slug(
            &ctx.db,
            slug,
            ws_id,
        )
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
        ds.map(|d| d.id)
    } else {
        None
    };

    let record = kyomi_auth::sql_history_service::create_query_history(
        &ctx.db,
        ws_id,
        &auth.user_id,
        datasource_config_id.as_deref(),
        &query_text,
        execution_time_ms,
        bytes_processed,
        row_count,
        &status,
        error_message.as_deref(),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(record.query_id)
}

// ---------------------------------------------------------------------------
// update_query_history — toggle saved status
// ---------------------------------------------------------------------------

/// Update a query history entry (e.g., toggle saved/bookmark).
#[server(prefix = "/leptos-api")]
pub async fn update_query_history(
    query_id: String,
    is_saved: Option<bool>,
) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let result = kyomi_auth::sql_history_service::update_query_history(
        &ctx.db,
        &query_id,
        ws_id,
        &auth.user_id,
        is_saved,
        None, // query_name
        None, // tags
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    if result.is_none() {
        return Err(ServerFnError::new(format!(
            "Query history '{query_id}' not found"
        )));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// delete_query_history — remove a history entry
// ---------------------------------------------------------------------------

/// Delete a query history entry.
#[server(prefix = "/leptos-api")]
pub async fn delete_query_history(
    query_id: String,
) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let deleted = kyomi_auth::sql_history_service::delete_query_history(
        &ctx.db,
        &query_id,
        ws_id,
        &auth.user_id,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    if !deleted {
        return Err(ServerFnError::new(format!(
            "Query history '{query_id}' not found"
        )));
    }

    Ok(())
}

// ===========================================================================
// Task 1.7: Catalog server functions
// ===========================================================================

/// Result from the catalog tree endpoint.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogTreeResult {
    pub tree: Vec<CatalogNode>,
    pub datasource_type: String,
    pub table_count: usize,
}

// ---------------------------------------------------------------------------
// get_catalog_tree — hierarchical catalog tree from cache
// ---------------------------------------------------------------------------

/// Fetch the catalog tree for a datasource.
///
/// Replicates the tree-building logic from `GET /{identifier}/catalog/tree`
/// in the REST handler. Builds a hierarchical tree from the
/// `datasource_table_cache` table: project > dataset/schema > table > column.
#[server(prefix = "/leptos-api")]
pub async fn get_catalog_tree(
    datasource_slug: String,
    include_columns: bool,
) -> Result<CatalogTreeResult, ServerFnError> {
    use std::collections::BTreeMap;

    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Resolve datasource.
    let datasource = kyomi_auth::datasource_service::resolve_datasource(
        &ctx.db,
        &datasource_slug,
        ws_id,
        false,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Sample datasources use a shared sentinel workspace.
    let is_sample = datasource
        .connection_config
        .get("is_sample")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Fetch all non-archived cached tables.
    let is_pg = ctx.db.is_postgres();
    let bf = kyomi_core::sql_compat::bool_false(is_pg);
    let cached_tables: Vec<kyomi_core::models::table_cache::DatasourceTableCache> = if is_sample {
        kyomi_core::db_fetch_all!(
            &ctx.db, kyomi_core::models::table_cache::DatasourceTableCache,
            &format!(
                "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
                 table_metadata, column_descriptions, created_at, updated_at, \
                 structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
                 FROM datasource_table_cache \
                 WHERE workspace_id = $1 AND is_archived = {bf}"
            ),
            kyomi_auth::catalog::indexers::sample_data::SAMPLE_DATA_WORKSPACE_ID
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?
    } else {
        kyomi_core::db_fetch_all!(
            &ctx.db, kyomi_core::models::table_cache::DatasourceTableCache,
            &format!(
                "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
                 table_metadata, column_descriptions, created_at, updated_at, \
                 structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
                 FROM datasource_table_cache \
                 WHERE datasource_config_id = $1 AND is_archived = {bf}"
            ),
            &datasource.id
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?
    };

    // BigQuery public datasets: include if enabled (defaults to true).
    let mut cached_tables = cached_tables;
    if !is_sample && datasource.datasource_type == kyomi_core::DatasourceType::Bigquery {
        let include_public = datasource
            .connection_config
            .get("include_public_datasets")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        if include_public {
            let public_tables: Vec<kyomi_core::models::table_cache::DatasourceTableCache> =
                kyomi_core::db_fetch_all!(
                    &ctx.db, kyomi_core::models::table_cache::DatasourceTableCache,
                    &format!(
                        "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
                         table_metadata, column_descriptions, created_at, updated_at, \
                         structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
                         FROM datasource_table_cache \
                         WHERE workspace_id = $1 AND is_archived = {bf}"
                    ),
                    kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID
                )
                .map_err(|e| ServerFnError::new(e.to_string()))?;
            cached_tables.extend(public_tables);
        }
    }

    let table_count = cached_tables.len();

    // Build tree: {project_id: {dataset_id: [table_nodes]}}
    let mut tree_dict: BTreeMap<String, BTreeMap<String, Vec<CatalogNode>>> = BTreeMap::new();

    for table in &cached_tables {
        let project = &table.project_id;
        let dataset = &table.dataset_id;
        let table_name = &table.table_id;

        let project_map = tree_dict.entry(project.clone()).or_default();
        let table_list = project_map.entry(dataset.clone()).or_default();

        // Build fully-qualified table name.
        let full_name = if project.is_empty() {
            format!("{dataset}.{table_name}")
        } else {
            format!("{project}.{dataset}.{table_name}")
        };

        // Build column children if requested.
        let children = if include_columns {
            let columns = table
                .table_metadata
                .get("columns")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            let col_nodes: Vec<CatalogNode> = columns
                .iter()
                .filter_map(|col| {
                    let col_name = col.get("name")?.as_str()?;
                    let col_type = col
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let col_full = if project.is_empty() {
                        format!("{dataset}.{table_name}.{col_name}")
                    } else {
                        format!("{project}.{dataset}.{table_name}.{col_name}")
                    };
                    Some(CatalogNode {
                        name: col_name.to_string(),
                        node_type: CatalogNodeType::Column(col_type),
                        children: Vec::new(),
                        full_name: Some(col_full),
                    })
                })
                .collect();

            col_nodes
        } else {
            Vec::new()
        };

        // Determine table vs view from metadata.
        let table_type_str = table
            .table_metadata
            .get("table_type")
            .and_then(|v| v.as_str())
            .unwrap_or("TABLE");
        let node_type = if table_type_str.to_uppercase().contains("VIEW") {
            CatalogNodeType::View
        } else {
            CatalogNodeType::Table
        };

        table_list.push(CatalogNode {
            name: table_name.clone(),
            node_type,
            children,
            full_name: Some(full_name),
        });
    }

    // Convert tree_dict to CatalogNode structure using registry metadata.
    let meta = kyomi_core::datasource_registry::get_metadata_by_str(
        datasource.datasource_type.as_ref(),
    )
    .ok_or_else(|| {
        ServerFnError::new(format!(
            "Unknown datasource type: '{}'",
            datasource.datasource_type.as_ref()
        ))
    })?;

    let level2_type = match meta.tree_level2_type {
        "dataset" => CatalogNodeType::Dataset,
        "schema" => CatalogNodeType::Schema,
        "database" => CatalogNodeType::Database,
        _ => CatalogNodeType::Schema, // fallback
    };

    let level1_type = match meta.tree_level1_type {
        "project" => CatalogNodeType::Project,
        "database" => CatalogNodeType::Database,
        "catalog" => CatalogNodeType::Database,
        _ => CatalogNodeType::Project, // fallback
    };

    let mut tree: Vec<CatalogNode> = Vec::new();

    for (project_id, datasets) in &tree_dict {
        let mut dataset_nodes: Vec<CatalogNode> = Vec::new();

        for (dataset_id, tables) in datasets {
            let ds_full = if project_id.is_empty() {
                dataset_id.clone()
            } else {
                format!("{project_id}.{dataset_id}")
            };

            let mut sorted_tables = tables.clone();
            sorted_tables.sort_by(|a, b| a.name.cmp(&b.name));

            dataset_nodes.push(CatalogNode {
                name: dataset_id.clone(),
                node_type: level2_type.clone(),
                children: sorted_tables,
                full_name: Some(ds_full),
            });
        }

        dataset_nodes.sort_by(|a, b| a.name.cmp(&b.name));

        let skip_wrapper = (meta.skip_empty_project_wrapper && project_id.is_empty())
            || (meta.skip_single_project_wrapper && tree_dict.len() == 1);

        if skip_wrapper {
            tree.extend(dataset_nodes);
        } else {
            tree.push(CatalogNode {
                name: project_id.clone(),
                node_type: level1_type.clone(),
                children: dataset_nodes,
                full_name: Some(project_id.clone()),
            });
        }
    }

    Ok(CatalogTreeResult {
        tree,
        datasource_type: datasource.datasource_type.as_ref().to_string(),
        table_count,
    })
}

// ---------------------------------------------------------------------------
// search_catalog — simple text search on table names
// ---------------------------------------------------------------------------

/// Search the catalog for tables matching a substring query.
///
/// Returns a flat list of matching table nodes (no hierarchy).
#[server(prefix = "/leptos-api")]
pub async fn search_catalog(
    datasource_slug: String,
    query: String,
) -> Result<Vec<CatalogNode>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let datasource = kyomi_auth::datasource_service::resolve_datasource(
        &ctx.db,
        &datasource_slug,
        ws_id,
        false,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let is_sample = datasource
        .connection_config
        .get("is_sample")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let is_pg = ctx.db.is_postgres();
    let bf = kyomi_core::sql_compat::bool_false(is_pg);
    let ilike = kyomi_core::sql_compat::ilike(is_pg, "table_id", "$2");

    let search_pattern = format!("%{query}%");

    let cached_tables: Vec<kyomi_core::models::table_cache::DatasourceTableCache> = if is_sample {
        let sql = format!(
            "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
             table_metadata, column_descriptions, created_at, updated_at, \
             structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
             FROM datasource_table_cache \
             WHERE workspace_id = $1 AND is_archived = {bf} AND {ilike} \
             ORDER BY table_id \
             LIMIT 50"
        );
        match &ctx.db {
            kyomi_core::db::DbPool::Postgres(pg) =>
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(kyomi_auth::catalog::indexers::sample_data::SAMPLE_DATA_WORKSPACE_ID)
                    .bind(&search_pattern)
                    .fetch_all(pg)
                    .await
                    .map_err(|e| ServerFnError::new(e.to_string()))?,
            kyomi_core::db::DbPool::Sqlite(sq) =>
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(kyomi_auth::catalog::indexers::sample_data::SAMPLE_DATA_WORKSPACE_ID)
                    .bind(&search_pattern)
                    .fetch_all(sq)
                    .await
                    .map_err(|e| ServerFnError::new(e.to_string()))?,
        }
    } else {
        let sql = format!(
            "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
             table_metadata, column_descriptions, created_at, updated_at, \
             structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
             FROM datasource_table_cache \
             WHERE datasource_config_id = $1 AND is_archived = {bf} AND {ilike} \
             ORDER BY table_id \
             LIMIT 50"
        );
        match &ctx.db {
            kyomi_core::db::DbPool::Postgres(pg) =>
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(&datasource.id)
                    .bind(&search_pattern)
                    .fetch_all(pg)
                    .await
                    .map_err(|e| ServerFnError::new(e.to_string()))?,
            kyomi_core::db::DbPool::Sqlite(sq) =>
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(&datasource.id)
                    .bind(&search_pattern)
                    .fetch_all(sq)
                    .await
                    .map_err(|e| ServerFnError::new(e.to_string()))?,
        }
    };

    // BigQuery public datasets: include matching tables if enabled.
    let mut cached_tables = cached_tables;
    if !is_sample && datasource.datasource_type == kyomi_core::DatasourceType::Bigquery {
        let include_public = datasource
            .connection_config
            .get("include_public_datasets")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        if include_public {
            let public_sql = format!(
                "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
                 table_metadata, column_descriptions, created_at, updated_at, \
                 structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
                 FROM datasource_table_cache \
                 WHERE workspace_id = $1 AND is_archived = {bf} AND {ilike} \
                 ORDER BY table_id \
                 LIMIT 50"
            );
            let public_tables: Vec<kyomi_core::models::table_cache::DatasourceTableCache> = match &ctx.db {
                kyomi_core::db::DbPool::Postgres(pg) =>
                    sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&public_sql)
                        .bind(kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID)
                        .bind(&search_pattern)
                        .fetch_all(pg)
                        .await
                        .map_err(|e| ServerFnError::new(e.to_string()))?,
                kyomi_core::db::DbPool::Sqlite(sq) =>
                    sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&public_sql)
                        .bind(kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID)
                        .bind(&search_pattern)
                        .fetch_all(sq)
                        .await
                        .map_err(|e| ServerFnError::new(e.to_string()))?,
            };
            cached_tables.extend(public_tables);
        }
    }

    // Cap combined results (primary + public datasets) to 50
    cached_tables.truncate(50);

    let results: Vec<CatalogNode> = cached_tables
        .into_iter()
        .map(|table| {
            let full_name = if table.project_id.is_empty() {
                format!("{}.{}", table.dataset_id, table.table_id)
            } else {
                format!("{}.{}.{}", table.project_id, table.dataset_id, table.table_id)
            };

            let table_type_str = table
                .table_metadata
                .get("table_type")
                .and_then(|v| v.as_str())
                .unwrap_or("TABLE");
            let node_type = if table_type_str.to_uppercase().contains("VIEW") {
                CatalogNodeType::View
            } else {
                CatalogNodeType::Table
            };

            CatalogNode {
                name: table.table_id,
                node_type,
                children: Vec::new(),
                full_name: Some(full_name),
            }
        })
        .collect();

    Ok(results)
}

// ---------------------------------------------------------------------------
// refresh_catalog — trigger manual catalog refresh
// ---------------------------------------------------------------------------

/// Trigger a manual catalog refresh for a datasource.
///
/// Only workspace admins can trigger refreshes. The actual indexing is
/// complex and involves provider connections, so this server function
/// delegates to the same service-layer helpers used by the REST handler.
/// For the Leptos frontend, we perform a simplified version that triggers
/// the refresh via the same underlying mechanisms.
#[server(prefix = "/leptos-api")]
pub async fn refresh_catalog(
    datasource_slug: String,
) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Check admin permission.
    let is_admin = auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
        || auth.workspace.is_owner;

    if !is_admin {
        return Err(ServerFnError::new(
            "Only workspace admins can trigger catalog refresh",
        ));
    }

    let datasource = kyomi_auth::datasource_service::resolve_datasource(
        &ctx.db,
        &datasource_slug,
        ws_id,
        false,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Sample datasources cannot be refreshed.
    let is_sample = datasource
        .connection_config
        .get("is_sample")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if is_sample {
        return Err(ServerFnError::new(
            "Sample datasource catalog is managed automatically and cannot be refreshed manually",
        ));
    }

    // BigQuery direct datasources use the REST API for catalog indexing,
    // which is handled by the existing REST endpoint. The Leptos server function
    // doesn't yet support this path.
    let ds_type_str = datasource.datasource_type.to_string();
    if datasource.connection_type != "connect" && ds_type_str == "bigquery" {
        return Err(ServerFnError::new(
            "BigQuery catalog refresh is not yet supported from this interface. Please use the REST API endpoint."
        ));
    }

    // Check rate limit — manual refresh uses 0 threshold so any
    // non-running datasource is eligible.
    let can_refresh =
        kyomi_auth::catalog::helpers::can_refresh_now(&ctx.db, &datasource.id, 0).await;

    if !can_refresh {
        return Err(ServerFnError::new(
            "Catalog indexing is already in progress for this datasource",
        ));
    }

    // For Connect datasources, send discover_catalog via the Connect provider.
    if datasource.connection_type == "connect" {
        let registry = ctx
            .connect_registry
            .as_ref()
            .ok_or_else(|| ServerFnError::new("Connect registry not available"))?;

        let provider = kyomi_datasource_server::ConnectProvider::with_timeout(
            registry.clone(),
            datasource.id.clone(),
            std::time::Duration::from_secs(120),
        );

        // Update status to running.
        let _ = kyomi_auth::catalog::helpers::update_workspace_status(
            &ctx.db,
            ws_id,
            &datasource.id,
            "running",
            None,
        )
        .await;

        // Test connection first.
        use kyomi_datasource_server::provider::DatasourceProvider as _;
        if let Err(e) = provider.test_connection().await {
            let _ = kyomi_auth::catalog::helpers::update_workspace_status(
                &ctx.db,
                ws_id,
                &datasource.id,
                "failed",
                None,
            )
            .await;
            return Err(ServerFnError::new(format!(
                "Connection test failed: {e}"
            )));
        }

        let catalog_result = match provider.discover_catalog().await {
            Ok(cr) => cr,
            Err(e) => {
                let _ = kyomi_auth::catalog::helpers::update_workspace_status(
                    &ctx.db,
                    ws_id,
                    &datasource.id,
                    "failed",
                    None,
                )
                .await;
                return Err(ServerFnError::new(format!(
                    "Catalog discovery failed: {e}"
                )));
            }
        };

        // Process results into cache.
        let encryption_key = ctx
            .encryption_key
            .as_deref()
            .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

        let indexer_ctx = kyomi_auth::catalog::helpers::IndexerContext {
            workspace_id: ws_id.to_string(),
            datasource_config_id: datasource.id.clone(),
            connection_config: datasource.connection_config.clone(),
            encryption_key: std::sync::Arc::from(*encryption_key),
        };

        let mut seen_table_ids = std::collections::HashSet::new();

        let embedding = ctx
            .embedding
            .wait_ready()
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

        for container in &catalog_result.containers {
            for table in &container.tables {
                let columns: Vec<kyomi_auth::catalog::types::ColumnEntry> = table
                    .columns
                    .iter()
                    .map(|col| kyomi_auth::catalog::types::ColumnEntry {
                        name: col.name.clone(),
                        col_type: Some(col.native_type.clone()),
                        native_type: Some(col.native_type.clone()),
                        description: col.description.clone(),
                    })
                    .collect();

                let project_id = "";
                let dataset_id = container.name.as_str();
                let table_name = table.name.as_str();
                let table_type = table.native_type.as_deref().unwrap_or("TABLE");
                let full_table_id = format!("{}.{}", container.name, table.name);
                let archive_id =
                    kyomi_core::build_full_table_name(project_id, dataset_id, table_name);
                seen_table_ids.insert(archive_id);

                let _ = kyomi_auth::catalog::helpers::cache_table(
                    &ctx.db,
                    embedding,
                    &indexer_ctx,
                    project_id,
                    dataset_id,
                    table_name,
                    table_type,
                    &columns,
                    &full_table_id,
                )
                .await;
            }
        }

        // Archive missing tables.
        let _ = kyomi_auth::catalog::helpers::archive_missing_tables(
            &ctx.db,
            ws_id,
            &datasource.id,
            &seen_table_ids,
        )
        .await;

        // Update timestamps.
        let _ = kyomi_auth::catalog::helpers::update_datasource_last_refresh(
            &ctx.db,
            &datasource.id,
        )
        .await;
        let _ = kyomi_auth::catalog::helpers::update_workspace_status(
            &ctx.db,
            ws_id,
            &datasource.id,
            "idle",
            None,
        )
        .await;

        return Ok(());
    }

    // --- Direct datasources: create provider, run catalog indexing ---

    let encryption_key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    let ds_type: kyomi_core::datasource_registry::DatasourceType =
        datasource.datasource_type.into();

    let user_cred = kyomi_auth::datasource_service::get_user_credential(
        &ctx.db,
        &auth.user_id,
        &datasource.id,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let credentials = if let Some(ref cred) = user_cred {
        kyomi_auth::credential_service::decrypt_credentials(&cred.credentials, encryption_key)
            .unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let credentials = kyomi_datasource_server::ensure_valid_oauth_credentials(
        &credentials,
        &datasource.connection_config,
        &ds_type,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let user_context =
        super::datasources::build_user_context(&ctx, &auth).await?;
    let user_context_ref = user_context.as_ref();

    let provider = kyomi_datasource_server::create_provider(
        &ds_type,
        &datasource.connection_config,
        &credentials,
        user_context_ref,
    )
    .await
    .map_err(|e| ServerFnError::new(format!("Failed to connect: {e}")))?;

    // Test connection.
    if let Err(e) = provider.test_connection().await {
        provider.close().await;
        return Err(ServerFnError::new(format!(
            "Connection test failed: {e}"
        )));
    }

    // Update workspace status to running.
    let _ = kyomi_auth::catalog::helpers::update_workspace_status(
        &ctx.db,
        ws_id,
        &datasource.id,
        "running",
        None,
    )
    .await;

    // Resolve containers to index.
    let meta = kyomi_core::datasource_registry::get_metadata(&ds_type);
    let container_key = meta.catalog_config_keys.first().copied().unwrap_or("");

    let configured = if !container_key.is_empty() {
        datasource.connection_config.get(container_key)
    } else {
        None
    };

    let containers: Vec<String> = if let Some(serde_json::Value::Array(arr)) = configured {
        let configured_containers: Vec<String> = arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        // If the user explicitly configured an empty array, skip indexing.
        if configured_containers.is_empty() {
            provider.close().await;
            let _ = kyomi_auth::catalog::helpers::update_workspace_status(
                &ctx.db,
                ws_id,
                &datasource.id,
                "idle",
                None,
            )
            .await;
            return Ok(());
        }

        configured_containers
    } else {
        // Discover all — use provider's discovery method.
        let discovery_result = provider.list_schemas().await;
        if let Some(err) = discovery_result.error {
            provider.close().await;
            let _ = kyomi_auth::catalog::helpers::update_workspace_status(
                &ctx.db,
                ws_id,
                &datasource.id,
                "failed",
                None,
            )
            .await;
            return Err(ServerFnError::new(format!(
                "Failed to discover catalog containers: {err}"
            )));
        }
        discovery_result.items
    };

    // Index tables in each container using INFORMATION_SCHEMA SQL queries
    // executed via the provider (matching the REST handler pattern).
    let indexer_ctx = kyomi_auth::catalog::helpers::IndexerContext {
        workspace_id: ws_id.to_string(),
        datasource_config_id: datasource.id.clone(),
        connection_config: datasource.connection_config.clone(),
        encryption_key: std::sync::Arc::from(*encryption_key),
    };

    let embedding = ctx
        .embedding
        .wait_ready()
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let mut seen_table_ids = std::collections::HashSet::new();
    let type_id = ds_type.as_str();

    for container in &containers {
        // Get table listing SQL for this datasource type + container.
        let tables_sql = match get_tables_in_container_sql(type_id, container) {
            Some(sql) => sql,
            None => {
                tracing::warn!(
                    container = %container,
                    type_id = %type_id,
                    "No table listing SQL for datasource type"
                );
                continue;
            }
        };

        // Execute the table listing query.
        let result = match provider
            .execute_query(&tables_sql, None, None, false)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    container = %container,
                    error = %e,
                    "Failed to list tables in container"
                );
                continue;
            }
        };

        let rows = result.rows.unwrap_or_default();
        for row in &rows {
            // First column is always the table name.
            let table_name = match row.first().and_then(|v| v.as_str()) {
                Some(name) => name,
                None => continue,
            };

            let project_id = "";
            let full_table_id = format!("{container}.{table_name}");
            let archive_id =
                kyomi_core::build_full_table_name(project_id, container, table_name);
            seen_table_ids.insert(archive_id);

            // For the initial cache entry we don't fetch column details
            // (that's done by the background indexer). We just record the
            // table existence.
            let _ = kyomi_auth::catalog::helpers::cache_table(
                &ctx.db,
                embedding,
                &indexer_ctx,
                project_id,
                container,
                table_name,
                "TABLE",
                &[],
                &full_table_id,
            )
            .await;
        }
    }

    provider.close().await;

    // Archive missing tables.
    let _ = kyomi_auth::catalog::helpers::archive_missing_tables(
        &ctx.db,
        ws_id,
        &datasource.id,
        &seen_table_ids,
    )
    .await;

    // Update timestamps.
    let _ = kyomi_auth::catalog::helpers::update_datasource_last_refresh(
        &ctx.db,
        &datasource.id,
    )
    .await;
    let _ = kyomi_auth::catalog::helpers::update_workspace_status(
        &ctx.db,
        ws_id,
        &datasource.id,
        "idle",
        None,
    )
    .await;

    Ok(())
}

// ---------------------------------------------------------------------------
// get_table_info — table metadata from cache
// ---------------------------------------------------------------------------

/// Get detailed table metadata (columns, descriptions, etc.) for a
/// single table from the `datasource_table_cache`.
///
/// Requires `datasource_slug` to verify the caller has workspace access
/// to the datasource that owns this table. Without this check, any
/// authenticated user could enumerate table metadata across workspaces.
#[server(prefix = "/leptos-api")]
pub async fn get_table_info(
    datasource_slug: String,
    table_id: String,
) -> Result<serde_json::Value, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Resolve datasource to verify workspace access.
    let datasource = kyomi_auth::datasource_service::resolve_datasource(
        &ctx.db,
        &datasource_slug,
        ws_id,
        false,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Parse the table_id (e.g., "project.dataset.table" or "dataset.table").
    let parts: Vec<&str> = table_id.splitn(3, '.').collect();
    let (project_id, dataset_id, table_name) = match parts.len() {
        3 => (parts[0], parts[1], parts[2]),
        2 => ("", parts[0], parts[1]),
        _ => {
            return Err(ServerFnError::new(format!(
                "Invalid table_id format: '{table_id}'. Expected 'dataset.table' or 'project.dataset.table'"
            )));
        }
    };

    let is_pg = ctx.db.is_postgres();
    let bf = kyomi_core::sql_compat::bool_false(is_pg);

    // Query matching rows from table cache, filtered by datasource_config_id
    // to ensure the table belongs to the resolved datasource.
    let sql = if project_id.is_empty() {
        format!(
            "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
             table_metadata, column_descriptions, created_at, updated_at, \
             structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
             FROM datasource_table_cache \
             WHERE datasource_config_id = $1 AND dataset_id = $2 AND table_id = $3 AND is_archived = {bf} \
             LIMIT 1"
        )
    } else {
        format!(
            "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
             table_metadata, column_descriptions, created_at, updated_at, \
             structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
             FROM datasource_table_cache \
             WHERE datasource_config_id = $1 AND project_id = $2 AND dataset_id = $3 AND table_id = $4 AND is_archived = {bf} \
             LIMIT 1"
        )
    };

    let table: Option<kyomi_core::models::table_cache::DatasourceTableCache> = if project_id
        .is_empty()
    {
        match &ctx.db {
            kyomi_core::db::DbPool::Postgres(pg) => {
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(&datasource.id)
                    .bind(dataset_id)
                    .bind(table_name)
                    .fetch_optional(pg)
                    .await
                    .map_err(|e| ServerFnError::new(e.to_string()))?
            }
            kyomi_core::db::DbPool::Sqlite(sq) => {
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(&datasource.id)
                    .bind(dataset_id)
                    .bind(table_name)
                    .fetch_optional(sq)
                    .await
                    .map_err(|e| ServerFnError::new(e.to_string()))?
            }
        }
    } else {
        match &ctx.db {
            kyomi_core::db::DbPool::Postgres(pg) => {
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(&datasource.id)
                    .bind(project_id)
                    .bind(dataset_id)
                    .bind(table_name)
                    .fetch_optional(pg)
                    .await
                    .map_err(|e| ServerFnError::new(e.to_string()))?
            }
            kyomi_core::db::DbPool::Sqlite(sq) => {
                sqlx::query_as::<_, kyomi_core::models::table_cache::DatasourceTableCache>(&sql)
                    .bind(&datasource.id)
                    .bind(project_id)
                    .bind(dataset_id)
                    .bind(table_name)
                    .fetch_optional(sq)
                    .await
                    .map_err(|e| ServerFnError::new(e.to_string()))?
            }
        }
    };

    match table {
        Some(t) => Ok(t.table_metadata),
        None => Err(ServerFnError::new(format!(
            "Table '{table_id}' not found in catalog"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Helper: SQL for listing tables in a container (per datasource type)
// ---------------------------------------------------------------------------

/// Generate the SQL query to list tables within a container (schema/database)
/// for a given datasource type. Matches the REST handler's
/// `get_tables_in_container_sql()` logic.
#[cfg(feature = "ssr")]
fn get_tables_in_container_sql(type_id: &str, container: &str) -> Option<String> {
    // Escape single quotes for SQL literal interpolation.
    let escaped = container.replace('\'', "''");

    match type_id {
        "postgres" | "redshift" => Some(format!(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = '{escaped}' ORDER BY table_name"
        )),
        "mysql" => Some(format!(
            "SELECT TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = '{escaped}' ORDER BY TABLE_NAME"
        )),
        "clickhouse" => Some(format!(
            "SELECT name FROM system.tables \
             WHERE database = '{escaped}' ORDER BY name"
        )),
        "sqlserver" | "synapse" => Some(format!(
            "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA = '{escaped}' ORDER BY TABLE_NAME"
        )),
        "snowflake" => {
            let quoted = format!("\"{}\"", escaped.replace('"', "\"\""));
            Some(format!(
                "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE \
                 FROM {quoted}.INFORMATION_SCHEMA.TABLES \
                 WHERE TABLE_SCHEMA NOT IN ('INFORMATION_SCHEMA') \
                   AND TABLE_TYPE IN ('BASE TABLE', 'VIEW') \
                 ORDER BY TABLE_SCHEMA, TABLE_NAME"
            ))
        }
        "databricks" => {
            let quoted = format!("`{}`", escaped.replace('`', "``"));
            Some(format!(
                "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE \
                 FROM {quoted}.INFORMATION_SCHEMA.TABLES \
                 WHERE TABLE_SCHEMA NOT IN ('information_schema') \
                 ORDER BY TABLE_SCHEMA, TABLE_NAME"
            ))
        }
        _ => None,
    }
}

// ===========================================================================
// Task 1.8: Chart generation server function
// ===========================================================================

/// Result from the chart generation endpoint.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeneratedChart {
    pub chartml_yaml: String,
    pub title: Option<String>,
}

// ---------------------------------------------------------------------------
// generate_chart_from_results — rule-based ChartML from SQL results
// ---------------------------------------------------------------------------

/// Generate a ChartML visualization from SQL query results.
///
/// Uses rule-based inference (same logic as the REST handler in
/// `chart_generate.rs`): analyzes column types and cardinality to pick
/// the best chart type, then builds a ChartML YAML spec.
#[server(prefix = "/leptos-api")]
pub async fn generate_chart_from_results(
    columns: Vec<String>,
    sample_rows: Vec<Vec<serde_json::Value>>,
    sql: String,
) -> Result<GeneratedChart, ServerFnError> {
    // Auth check — must be logged in.
    let _auth = extract_auth().await?;

    if columns.is_empty() {
        return Err(ServerFnError::new("No columns provided"));
    }

    let chart_yaml = generate_chartml_with_rules(&sql, &columns, &sample_rows)?;

    // Extract title from the generated YAML.
    let title = serde_yaml::from_str::<serde_yaml::Value>(&chart_yaml)
        .ok()
        .and_then(|v| v.get("title")?.as_str().map(String::from));

    Ok(GeneratedChart {
        chartml_yaml: chart_yaml,
        title,
    })
}

// ---------------------------------------------------------------------------
// Rule-based chart generation helpers (inlined from chart_generate.rs)
// ---------------------------------------------------------------------------

/// Column analysis result for chart type inference.
#[cfg(feature = "ssr")]
struct ChartColumnAnalysis {
    name: String,
    is_numeric: bool,
    is_date: bool,
    cardinality: usize,
}

/// Check whether a JSON value looks like a date/datetime string.
#[cfg(feature = "ssr")]
fn is_date_value(v: &serde_json::Value) -> bool {
    let s = match v.as_str() {
        Some(s) => s,
        None => return false,
    };
    if chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        return true;
    }
    if chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").is_ok() {
        return true;
    }
    if chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").is_ok() {
        return true;
    }
    if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
        return true;
    }
    false
}

/// Analyze a single column across the provided rows.
#[cfg(feature = "ssr")]
fn analyze_chart_column(
    col_name: &str,
    rows: &[Vec<serde_json::Value>],
    columns: &[String],
) -> ChartColumnAnalysis {
    let col_index = match columns.iter().position(|c| c == col_name) {
        Some(i) => i,
        None => {
            return ChartColumnAnalysis {
                name: col_name.to_string(),
                is_numeric: false,
                is_date: false,
                cardinality: 0,
            };
        }
    };

    let values: Vec<&serde_json::Value> = rows
        .iter()
        .filter_map(|row| row.get(col_index))
        .filter(|v| !v.is_null())
        .collect();

    if values.is_empty() {
        return ChartColumnAnalysis {
            name: col_name.to_string(),
            is_numeric: false,
            is_date: false,
            cardinality: 0,
        };
    }

    let is_numeric = values.iter().all(|v| v.is_number());
    let is_date = values.iter().all(|v| is_date_value(v));
    let cardinality = values
        .iter()
        .map(|v| v.to_string())
        .collect::<std::collections::HashSet<_>>()
        .len();

    ChartColumnAnalysis {
        name: col_name.to_string(),
        is_numeric,
        is_date,
        cardinality,
    }
}

/// Infer the best chart type from column analyses.
#[cfg(feature = "ssr")]
fn infer_chart_type(analyses: &[ChartColumnAnalysis]) -> &'static str {
    if analyses.iter().any(|a| a.is_date) {
        return "line";
    }
    if let Some(cat) = analyses.iter().find(|a| !a.is_numeric && !a.is_date) {
        if cat.cardinality <= 20 {
            return "bar";
        }
    }
    "table"
}

/// Infer x and y axes from column analyses.
#[cfg(feature = "ssr")]
fn infer_axes<'a>(
    analyses: &'a [ChartColumnAnalysis],
    columns: &'a [String],
) -> (&'a str, &'a str) {
    let x_col = analyses
        .iter()
        .find(|a| a.is_date)
        .or_else(|| analyses.iter().find(|a| !a.is_numeric))
        .or_else(|| analyses.first());

    let x_name = x_col.map(|a| a.name.as_str()).unwrap_or(&columns[0]);

    let y_col = analyses
        .iter()
        .find(|a| a.is_numeric && a.name != x_name)
        .or_else(|| {
            if analyses.len() > 1 {
                Some(&analyses[1])
            } else {
                analyses.first()
            }
        });

    let y_name = y_col.map(|a| a.name.as_str()).unwrap_or_else(|| {
        if columns.len() > 1 {
            &columns[1]
        } else {
            &columns[0]
        }
    });

    (x_name, y_name)
}

/// Shorthand: create a `serde_yaml::Value::String`.
#[cfg(feature = "ssr")]
fn yaml_str(s: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(s.to_string())
}

/// Build the `data:` section of the ChartML spec.
#[cfg(feature = "ssr")]
fn build_chart_data_section(sql_text: &str) -> serde_yaml::Mapping {
    let mut data = serde_yaml::Mapping::new();
    data.insert(yaml_str("query"), yaml_str(sql_text));
    let mut cache = serde_yaml::Mapping::new();
    cache.insert(yaml_str("ttl"), yaml_str("24h"));
    data.insert(yaml_str("cache"), serde_yaml::Value::Mapping(cache));
    data
}

/// Generate a metric card spec for single-value results.
#[cfg(feature = "ssr")]
fn generate_metric_card(column_name: &str, sql_text: &str) -> String {
    let mut spec = serde_yaml::Mapping::new();
    spec.insert(yaml_str("type"), yaml_str("chart"));
    spec.insert(yaml_str("version"), serde_yaml::Value::Number(1.into()));
    spec.insert(yaml_str("title"), yaml_str(column_name));
    spec.insert(
        yaml_str("data"),
        serde_yaml::Value::Mapping(build_chart_data_section(sql_text)),
    );
    let mut vis = serde_yaml::Mapping::new();
    vis.insert(yaml_str("type"), yaml_str("metric"));
    vis.insert(yaml_str("value"), yaml_str(column_name));
    vis.insert(yaml_str("label"), yaml_str(column_name));
    spec.insert(yaml_str("visualize"), serde_yaml::Value::Mapping(vis));
    serde_yaml::to_string(&spec).unwrap_or_default()
}

/// Generate a table fallback spec.
#[cfg(feature = "ssr")]
fn generate_table_fallback(sql_text: &str, columns: &[String]) -> String {
    let mut spec = serde_yaml::Mapping::new();
    spec.insert(yaml_str("type"), yaml_str("chart"));
    spec.insert(yaml_str("version"), serde_yaml::Value::Number(1.into()));
    spec.insert(yaml_str("title"), yaml_str("Query Results"));
    spec.insert(
        yaml_str("data"),
        serde_yaml::Value::Mapping(build_chart_data_section(sql_text)),
    );
    let mut vis = serde_yaml::Mapping::new();
    vis.insert(yaml_str("type"), yaml_str("table"));
    vis.insert(
        yaml_str("columns"),
        serde_yaml::Value::Sequence(columns.iter().map(|c| yaml_str(c)).collect()),
    );
    spec.insert(yaml_str("visualize"), serde_yaml::Value::Mapping(vis));
    serde_yaml::to_string(&spec).unwrap_or_default()
}

/// Rule-based ChartML generation.
///
/// Matches the logic in `apps/server/src/routes/chart_generate.rs`
/// `generate_with_rules()`.
#[cfg(feature = "ssr")]
fn generate_chartml_with_rules(
    sql_text: &str,
    columns: &[String],
    rows: &[Vec<serde_json::Value>],
) -> Result<String, ServerFnError> {
    let analyses: Vec<ChartColumnAnalysis> = columns
        .iter()
        .map(|col| analyze_chart_column(col, rows, columns))
        .collect();

    // Single value -> metric card.
    if columns.len() == 1 && rows.len() == 1 {
        return Ok(generate_metric_card(&columns[0], sql_text));
    }

    let chart_type = infer_chart_type(&analyses);
    let (x_axis, mut y_axis) = infer_axes(&analyses, columns);

    // Same column for both axes -> try picking a different y.
    if x_axis == y_axis && columns.len() > 1 {
        if let Some(alt) = analyses
            .iter()
            .find(|a| a.name != x_axis && a.is_numeric)
            .or_else(|| analyses.iter().find(|a| a.name != x_axis))
        {
            y_axis = &alt.name;
        } else {
            return Ok(generate_table_fallback(sql_text, columns));
        }
    }

    let title = format!("{y_axis} by {x_axis}");

    let mut spec = serde_yaml::Mapping::new();
    spec.insert(yaml_str("type"), yaml_str("chart"));
    spec.insert(yaml_str("version"), serde_yaml::Value::Number(1.into()));
    spec.insert(yaml_str("title"), yaml_str(&title));
    spec.insert(
        yaml_str("data"),
        serde_yaml::Value::Mapping(build_chart_data_section(sql_text)),
    );
    let mut vis = serde_yaml::Mapping::new();
    vis.insert(yaml_str("type"), yaml_str(chart_type));
    vis.insert(yaml_str("columns"), yaml_str(x_axis));
    vis.insert(yaml_str("rows"), yaml_str(y_axis));
    spec.insert(yaml_str("visualize"), serde_yaml::Value::Mapping(vis));

    Ok(serde_yaml::to_string(&spec).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Task 6.1: WebSocket connection info (user_id + workspace_id for WS URL)
// ---------------------------------------------------------------------------

/// Connection info needed to build the WebSocket URL on the client side.
///
/// The streaming handler (`use_query_stream_handler`) connects to
/// `/ws/{workspace_id}_{user_id}?token=...` and needs both IDs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WsConnectionInfo {
    pub user_id: String,
    pub workspace_id: String,
}

/// Return the current user's ID and workspace ID for WebSocket connection.
///
/// Called once when the SQL Editor page mounts to set up the streaming handler.
#[server(prefix = "/leptos-api")]
pub async fn get_ws_connection_info() -> Result<WsConnectionInfo, ServerFnError> {
    let auth = extract_auth().await?;
    let ws_id = workspace_id(&auth)?;

    Ok(WsConnectionInfo {
        user_id: auth.user_id.clone(),
        workspace_id: ws_id.to_string(),
    })
}
