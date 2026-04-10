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

use crate::pages::sql_editor::types::{CatalogNode, QueryHistoryEntry, QueryResult};
#[cfg(feature = "ssr")]
use crate::pages::sql_editor::types::{CatalogNodeType, ColumnMetadata, QueryHandle};

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

    // Resolve datasource to check if it's a direct BigQuery connection.
    let ds = kyomi_auth::datasource_service::resolve_datasource(
        &ctx.db,
        &datasource_slug,
        ws_id,
        false,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // BigQuery direct connections use the REST API for job_id-based pagination.
    // Connect-mode BigQuery goes through the provider path like all other types.
    if ds.datasource_type.as_ref() == "bigquery" && ds.connection_type != "connect" {
        let encryption_key = ctx
            .encryption_key
            .as_deref()
            .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

        let start = std::time::Instant::now();

        let (access_token, billing_project) = resolve_bq_access_for_datasource(
            &ctx.db,
            &ctx.config,
            encryption_key,
            &ds,
            &auth.user_id,
        )
        .await?;

        let clamped_page_size = page_size.clamp(1, 1000);
        let (columns, rows, total_rows, job_id) =
            execute_bigquery_query_rest(&access_token, &billing_project, &sql, clamped_page_size)
                .await?;

        let elapsed_ms = start.elapsed().as_millis() as u64;
        return bq_rest_to_query_result(
            columns, rows, total_rows, job_id, &ds.slug, &sql, elapsed_ms,
        );
    }

    // All other datasource types (and Connect-mode BigQuery): use provider path.
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

    // Resolve datasource to check type.
    let ds = kyomi_auth::datasource_service::resolve_datasource(
        &ctx.db,
        &datasource_slug,
        ws_id,
        false,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // BigQuery job_id pagination: fetch page directly from a completed job
    // without re-executing the query. Only for direct BigQuery connections.
    if let Some(ref bq_job_id) = job_id
        && ds.datasource_type.as_ref() == "bigquery" && ds.connection_type != "connect"
    {
        let encryption_key = ctx
            .encryption_key
            .as_deref()
            .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

        let start = std::time::Instant::now();

        let (access_token, billing_project) = resolve_bq_access_for_datasource(
            &ctx.db,
            &ctx.config,
            encryption_key,
            &ds,
            &auth.user_id,
        )
        .await?;

        let clamped_page_size = page_size.clamp(1, 1000);
        let start_index = (page.saturating_sub(1) as u64) * (clamped_page_size as u64);

        let (columns, rows, total_rows) = fetch_bq_job_page(
            &access_token,
            &billing_project,
            bq_job_id,
            start_index,
            clamped_page_size,
        )
        .await?;

        let elapsed_ms = start.elapsed().as_millis() as u64;
        return bq_rest_to_query_result(
            columns,
            rows,
            total_rows,
            job_id.clone(),
            &ds.slug,
            &sql,
            elapsed_ms,
        );
    }

    // All other datasource types: re-execute with LIMIT/OFFSET.
    let (_ds, provider) =
        super::datasources::create_query_provider(&ctx, &auth, ws_id, &datasource_slug).await?;

    let include_total = include_total.unwrap_or(false);
    let result = run_paginated_query(&*provider, &sql, page_size, page, include_total).await?;

    provider_result_to_query_result(result.0, result.1, &ds.slug, ds.datasource_type.as_ref(), &sql)
}

// ===========================================================================
// BigQuery REST API helpers (SSR-only)
// ===========================================================================

/// BigQuery REST API base URL.
#[cfg(feature = "ssr")]
const BIGQUERY_API_BASE: &str = "https://bigquery.googleapis.com/bigquery/v2";

/// Resolve a BigQuery access token and billing project for a datasource.
///
/// Supports all three auth modes:
/// - `kyomi_oauth` — user connected via the app's Google OAuth client
/// - `enterprise_oauth` — user connected via per-datasource OAuth credentials
/// - `service_account` — datasource uses a service account JSON key
///
/// Mirrors `resolve_bq_access()` in `apps/server/src/routes/bigquery.rs`.
#[cfg(feature = "ssr")]
async fn resolve_bq_access_for_datasource(
    db: &kyomi_core::DbPool,
    config: &kyomi_core::Config,
    encryption_key: &[u8; 32],
    datasource: &kyomi_core::models::datasource::DatasourceConfig,
    user_id: &str,
) -> Result<(String, String), ServerFnError> {
    let connection_config = &datasource.connection_config;
    let auth_mode = connection_config
        .get("auth_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("kyomi_oauth");

    // Resolve user-level credentials for billing project override
    let user_cred =
        kyomi_auth::datasource_service::get_user_credential(db, user_id, &datasource.id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    let user_cred_data = if let Some(ref cred) = user_cred {
        kyomi_auth::credential_service::decrypt_credentials(&cred.credentials, encryption_key).ok()
    } else {
        None
    };

    let (access_token, billing_project) = match auth_mode {
        "kyomi_oauth" => {
            let client_id = config.google_oauth_client_id.as_deref().ok_or_else(|| {
                ServerFnError::new("GOOGLE_OAUTH_CLIENT_ID not configured")
            })?;
            let client_secret = config.google_oauth_client_secret.as_deref().ok_or_else(|| {
                ServerFnError::new("GOOGLE_OAUTH_CLIENT_SECRET not configured")
            })?;

            let tokens = kyomi_auth::google_oauth::ensure_valid_google_token(
                db,
                user_id,
                encryption_key,
                client_id,
                client_secret,
            )
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

            let token = tokens.access_token.clone();

            let bp = kyomi_datasource_server::providers::bigquery::resolve_billing_project(
                connection_config,
                user_cred_data.as_ref().unwrap_or(&serde_json::json!({})),
                None,
            )
            .unwrap_or_default();

            (token, bp)
        }
        "enterprise_oauth" => {
            let cred_data = user_cred_data.ok_or_else(|| {
                ServerFnError::new(
                    "No credentials found for this datasource. Please configure OAuth.",
                )
            })?;

            // Refresh enterprise OAuth token if expired
            let ds_type = kyomi_core::datasource_registry::DatasourceType::BigQuery;
            let cred_data =
                match kyomi_datasource_server::ensure_valid_oauth_credentials(
                    &cred_data,
                    connection_config,
                    &ds_type,
                )
                .await
                {
                    Ok(refreshed) if refreshed != cred_data => {
                        // Persist refreshed token back to DB
                        if let Some(ref cred) = user_cred
                            && let Err(e) = kyomi_auth::datasource_service::save_user_credential(
                                db,
                                encryption_key,
                                user_id,
                                &datasource.id,
                                &cred.workspace_id,
                                &refreshed,
                            )
                            .await
                        {
                            tracing::warn!(
                                datasource_id = %datasource.id,
                                "Failed to persist refreshed enterprise OAuth token: {e}"
                            );
                        }
                        refreshed
                    }
                    Ok(unchanged) => unchanged,
                    Err(e) => {
                        tracing::warn!(
                            datasource_id = %datasource.id,
                            "Enterprise OAuth refresh failed, using existing token: {e}"
                        );
                        cred_data
                    }
                };

            let token = cred_data
                .get("oauth_access_token")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    ServerFnError::new("No OAuth access token found in credentials")
                })?
                .to_string();

            let bp = kyomi_datasource_server::providers::bigquery::resolve_billing_project(
                connection_config,
                &cred_data,
                None,
            )
            .unwrap_or_default();

            (token, bp)
        }
        "service_account" => {
            let client = kyomi_datasource_server::http_client()
                .map_err(|e| ServerFnError::new(e.to_string()))?;
            let (token, project_id) =
                kyomi_datasource_server::providers::bigquery::exchange_service_account_jwt(
                    &client,
                    connection_config,
                )
                .await
                .map_err(|e| ServerFnError::new(e.to_string()))?;

            let bp = kyomi_datasource_server::providers::bigquery::resolve_billing_project(
                connection_config,
                user_cred_data.as_ref().unwrap_or(&serde_json::json!({})),
                Some(&project_id),
            )
            .unwrap_or(project_id);

            (token, bp)
        }
        other => {
            return Err(ServerFnError::new(format!(
                "Unsupported BigQuery auth mode: {other}"
            )));
        }
    };

    if billing_project.is_empty() {
        return Err(ServerFnError::new(
            "No billing project configured. Please set a billing project in datasource settings.",
        ));
    }

    Ok((access_token, billing_project))
}

/// Execute a BigQuery query via the REST API and return the first page of results.
///
/// Returns `(columns, rows, total_rows, job_id)`.
///
/// POST `https://bigquery.googleapis.com/bigquery/v2/projects/{project}/queries`
/// with `{ "query": sql, "useLegacySql": false, "maxResults": max_results }`.
///
/// If `jobComplete` is false in the response, polls `getQueryResults` until complete.
#[cfg(feature = "ssr")]
async fn execute_bigquery_query_rest(
    access_token: &str,
    project_id: &str,
    sql: &str,
    max_results: u32,
) -> Result<(Vec<String>, Vec<Vec<serde_json::Value>>, usize, Option<String>), ServerFnError> {
    let client = kyomi_datasource_server::http_client()
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let url = format!("{BIGQUERY_API_BASE}/projects/{project_id}/queries");

    let body = serde_json::json!({
        "query": sql,
        "useLegacySql": false,
        "maxResults": max_results,
    });

    let response = tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_QUERY,
        client
            .post(&url)
            .bearer_auth(access_token)
            .json(&body)
            .send(),
    )
    .await
    .map_err(|_| ServerFnError::new("BigQuery query timed out"))?
    .map_err(|e| ServerFnError::new(format!("BigQuery query HTTP request failed: {e}")))?;

    let status_code = response.status();
    let response_body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to parse BigQuery response: {e}")))?;

    if status_code.is_client_error() || status_code.is_server_error() {
        let msg = response_body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("BigQuery query failed");
        return Err(ServerFnError::new(format!("BigQuery error: {msg}")));
    }

    // Extract job_id from jobReference
    let job_id = response_body
        .get("jobReference")
        .and_then(|jr| jr.get("jobId"))
        .and_then(|j| j.as_str())
        .map(String::from);

    // Check if job is complete; if not, poll until it is
    let job_complete = response_body
        .get("jobComplete")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !job_complete {
        // Job not complete — poll getQueryResults until done
        let job_id_str = job_id.as_deref().ok_or_else(|| {
            ServerFnError::new("BigQuery job not complete but no jobId returned")
        })?;
        return poll_bigquery_job_completion(
            &client,
            access_token,
            project_id,
            job_id_str,
            max_results,
        )
        .await;
    }

    // Parse the completed response
    let (columns, rows, total_rows) = parse_bq_query_response(&response_body)?;

    Ok((columns, rows, total_rows, job_id))
}

/// Poll a BigQuery job until it completes, then return the first page of results.
#[cfg(feature = "ssr")]
async fn poll_bigquery_job_completion(
    client: &reqwest::Client,
    access_token: &str,
    project_id: &str,
    job_id: &str,
    max_results: u32,
) -> Result<(Vec<String>, Vec<Vec<serde_json::Value>>, usize, Option<String>), ServerFnError> {
    let poll_timeout = kyomi_datasource_server::DATASOURCE_TIMEOUT_QUERY;
    let start = std::time::Instant::now();

    loop {
        if start.elapsed() >= poll_timeout {
            return Err(ServerFnError::new(
                "BigQuery query timed out waiting for job completion",
            ));
        }

        // Wait before polling (backoff: 1 second between polls)
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let url = format!(
            "{BIGQUERY_API_BASE}/projects/{project_id}/queries/{job_id}?maxResults={max_results}"
        );

        let response = client
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| {
                ServerFnError::new(format!("BigQuery poll request failed: {e}"))
            })?;

        let status_code = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| {
                ServerFnError::new(format!("Failed to parse BigQuery poll response: {e}"))
            })?;

        if status_code.is_client_error() || status_code.is_server_error() {
            let msg = body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("BigQuery poll failed");
            return Err(ServerFnError::new(format!("BigQuery error: {msg}")));
        }

        let job_complete = body
            .get("jobComplete")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if job_complete {
            let (columns, rows, total_rows) = parse_bq_query_response(&body)?;
            return Ok((columns, rows, total_rows, Some(job_id.to_string())));
        }
    }
}

/// Fetch a specific page from a completed BigQuery job.
///
/// GET `https://bigquery.googleapis.com/bigquery/v2/projects/{project}/queries/{job_id}?startIndex={start_index}&maxResults={max_results}`
///
/// Returns `(columns, rows, total_rows)`.
#[cfg(feature = "ssr")]
async fn fetch_bq_job_page(
    access_token: &str,
    project_id: &str,
    job_id: &str,
    start_index: u64,
    max_results: u32,
) -> Result<(Vec<String>, Vec<Vec<serde_json::Value>>, usize), ServerFnError> {
    let client = kyomi_datasource_server::http_client()
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let url = format!(
        "{BIGQUERY_API_BASE}/projects/{project_id}/queries/{job_id}?startIndex={start_index}&maxResults={max_results}"
    );

    let response = tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_QUERY,
        client.get(&url).bearer_auth(access_token).send(),
    )
    .await
    .map_err(|_| ServerFnError::new("BigQuery page fetch timed out"))?
    .map_err(|e| ServerFnError::new(format!("BigQuery page fetch failed: {e}")))?;

    let status_code = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| {
            ServerFnError::new(format!("Failed to parse BigQuery page response: {e}"))
        })?;

    if status_code.is_client_error() || status_code.is_server_error() {
        let msg = body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("BigQuery page fetch failed");
        return Err(ServerFnError::new(format!("BigQuery error: {msg}")));
    }

    parse_bq_query_response(&body)
}

/// Parse a BigQuery query response JSON into `(columns, rows, total_rows)`.
///
/// Handles the BigQuery response format:
/// - `schema.fields[].name` for column names
/// - `rows[].f[].v` for cell values
/// - `totalRows` (string or number) for the total row count
#[cfg(feature = "ssr")]
type BqQueryResult = (Vec<String>, Vec<Vec<serde_json::Value>>, usize);

#[cfg(feature = "ssr")]
fn parse_bq_query_response(
    body: &serde_json::Value,
) -> Result<BqQueryResult, ServerFnError> {
    // Extract column names from schema
    let columns: Vec<String> = body
        .get("schema")
        .and_then(|s| s.get("fields"))
        .and_then(|f| f.as_array())
        .map(|fields| {
            fields
                .iter()
                .filter_map(|f| f.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // Extract total rows
    let total_rows = body
        .get("totalRows")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<usize>().ok())
                .or_else(|| v.as_u64().map(|n| n as usize))
        })
        .unwrap_or(0);

    // Parse rows from BigQuery's rows[].f[].v format into Vec<Vec<Value>>
    let rows: Vec<Vec<serde_json::Value>> = body
        .get("rows")
        .and_then(|r| r.as_array())
        .map(|bq_rows| {
            bq_rows
                .iter()
                .map(|row| {
                    row.get("f")
                        .and_then(|f| f.as_array())
                        .map(|cells| {
                            cells
                                .iter()
                                .map(|cell| {
                                    cell.get("v")
                                        .cloned()
                                        .unwrap_or(serde_json::Value::Null)
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();

    Ok((columns, rows, total_rows))
}

/// Build a `QueryResult` from BigQuery REST API response data.
///
/// Converts the raw column names and row values into the typed `QueryResult`
/// expected by the SQL editor frontend.
#[cfg(feature = "ssr")]
fn bq_rest_to_query_result(
    columns: Vec<String>,
    rows: Vec<Vec<serde_json::Value>>,
    total_rows: usize,
    job_id: Option<String>,
    datasource_slug: &str,
    sql: &str,
    elapsed_ms: u64,
) -> Result<QueryResult, ServerFnError> {
    let col_metadata: Vec<ColumnMetadata> = columns
        .iter()
        .map(|name| ColumnMetadata {
            name: name.clone(),
            col_type: None,
            mode: None,
        })
        .collect();

    let row_count = rows.len();

    let query_handle = QueryHandle {
        datasource_type: "bigquery".to_string(),
        datasource_slug: datasource_slug.to_string(),
        sql: sql.to_string(),
        job_id,
    };

    Ok(QueryResult {
        columns: col_metadata,
        rows,
        row_count,
        total_rows: Some(total_rows),
        query_handle: Some(query_handle),
        execution_time: Some(elapsed_ms),
        bytes_processed: None,
        has_more: row_count < total_rows,
    })
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
    let ws_manager = ctx.ws_manager.clone()
        .ok_or_else(|| ServerFnError::new("WebSocket manager not available for streaming"))?;
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
/// Only workspace admins can trigger refreshes. Delegates to the shared
/// catalog refresh service in [`super::catalog_refresh`] which handles all
/// datasource types including BigQuery REST API indexing.
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

    let encryption_key = ctx
        .encryption_key
        .as_ref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    let ds_type: kyomi_core::datasource_registry::DatasourceType =
        datasource.datasource_type.into();

    // Resolve and decrypt user credentials.
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

    // Refresh OAuth credentials if needed.
    let credentials = kyomi_datasource_server::ensure_valid_oauth_credentials(
        &credentials,
        &datasource.connection_config,
        &ds_type,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Persist refreshed token if it changed.
    if let Some(ref cred) = user_cred {
        let _ = kyomi_auth::datasource_service::save_user_credential(
            &ctx.db,
            encryption_key,
            &auth.user_id,
            &datasource.id,
            &cred.workspace_id,
            &credentials,
        )
        .await;
    }

    let user_context =
        super::datasources::build_user_context(&ctx, &auth).await?;

    let embedding = ctx
        .embedding
        .wait_ready()
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let params = super::catalog_refresh::CatalogRefreshParams {
        db: &ctx.db,
        embedding,
        encryption_key,
        datasource,
        workspace_id: ws_id,
        user_id: &auth.user_id,
        force: false,
        connect_registry: ctx.connect_registry.as_ref(),
        user_context,
        credentials,
    };

    let result = super::catalog_refresh::execute_catalog_refresh(params)
        .await
        .map_err(|e: kyomi_core::Error| ServerFnError::new(e.to_string()))?;

    if result.status == "error" {
        Err(ServerFnError::new(result.message))
    } else {
        Ok(())
    }
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
    datasource_slug: String,
) -> Result<GeneratedChart, ServerFnError> {
    // Auth check — must be logged in.
    let _auth = extract_auth().await?;

    if columns.is_empty() {
        return Err(ServerFnError::new("No columns provided"));
    }

    let chart_yaml = generate_chartml_with_rules(&sql, &columns, &sample_rows, &datasource_slug)?;

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
    if let Some(cat) = analyses.iter().find(|a| !a.is_numeric && !a.is_date)
        && cat.cardinality <= 20
    {
        return "bar";
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
fn build_chart_data_section(datasource_slug: &str, sql_text: &str) -> serde_yaml::Mapping {
    let mut data = serde_yaml::Mapping::new();
    data.insert(yaml_str("datasource"), yaml_str(datasource_slug));
    data.insert(yaml_str("sql"), yaml_str(sql_text));
    data
}

/// Generate a metric card spec for single-value results.
#[cfg(feature = "ssr")]
fn generate_metric_card(column_name: &str, datasource_slug: &str, sql_text: &str) -> String {
    let mut spec = serde_yaml::Mapping::new();
    spec.insert(yaml_str("type"), yaml_str("chart"));
    spec.insert(yaml_str("version"), serde_yaml::Value::Number(1.into()));
    spec.insert(yaml_str("title"), yaml_str(column_name));
    spec.insert(
        yaml_str("data"),
        serde_yaml::Value::Mapping(build_chart_data_section(datasource_slug, sql_text)),
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
fn generate_table_fallback(datasource_slug: &str, sql_text: &str, columns: &[String]) -> String {
    let mut spec = serde_yaml::Mapping::new();
    spec.insert(yaml_str("type"), yaml_str("chart"));
    spec.insert(yaml_str("version"), serde_yaml::Value::Number(1.into()));
    spec.insert(yaml_str("title"), yaml_str("Query Results"));
    spec.insert(
        yaml_str("data"),
        serde_yaml::Value::Mapping(build_chart_data_section(datasource_slug, sql_text)),
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
    datasource_slug: &str,
) -> Result<String, ServerFnError> {
    let analyses: Vec<ChartColumnAnalysis> = columns
        .iter()
        .map(|col| analyze_chart_column(col, rows, columns))
        .collect();

    // Single value -> metric card.
    if columns.len() == 1 && rows.len() == 1 {
        return Ok(generate_metric_card(&columns[0], datasource_slug, sql_text));
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
            return Ok(generate_table_fallback(datasource_slug, sql_text, columns));
        }
    }

    let title = format!("{y_axis} by {x_axis}");

    let mut spec = serde_yaml::Mapping::new();
    spec.insert(yaml_str("type"), yaml_str("chart"));
    spec.insert(yaml_str("version"), serde_yaml::Value::Number(1.into()));
    spec.insert(yaml_str("title"), yaml_str(&title));
    spec.insert(
        yaml_str("data"),
        serde_yaml::Value::Mapping(build_chart_data_section(datasource_slug, sql_text)),
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
