// SPDX-License-Identifier: AGPL-3.0-or-later

//! Arrow IPC HTTP endpoint.
//!
//! `POST /api/v1/query-arrow` — Execute a SQL query against a datasource and
//! return results in Apache Arrow IPC streaming format.
//!
//! ## Two execution paths
//!
//! - **Paginated** (`limit` set): calls `execute_query`, writes a single batch.
//!   Response headers carry `X-Total-Rows`, `X-Job-Id`, and `X-Has-More`.
//! - **Streaming** (no `limit`): calls `execute_query_stream_arrow`, streams
//!   Arrow IPC bytes to the HTTP body as each batch arrives via a background
//!   task and `tokio::io::duplex`. No server-side buffering of the full result.
//!
//! ## Content-Type
//!
//! `application/vnd.apache.arrow.stream`

use axum::{
    extract::{Json, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use futures_util::StreamExt;
use serde::Deserialize;
use tracing::instrument;

use kyomi_datasource_server::QueryStatus;

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

pub fn routes() -> Router<AppState> {
    Router::new().route("/query-arrow", post(query_arrow))
}

// ===========================================================================
// Request / error types
// ===========================================================================

#[derive(Deserialize)]
struct QueryArrowRequest {
    datasource_slug: String,
    sql: String,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    include_total: Option<bool>,
    #[serde(default)]
    job_id: Option<String>,
}

/// Serialise an error message as `{"error": "..."}` with the given status code.
///
/// The raw message is logged at `warn` level before sanitization so that
/// internal details (credentials, hostnames) are preserved server-side for
/// debugging while the client receives only safe text.
fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let raw = message.into();
    tracing::warn!(raw_error = %raw, "query error (sanitized for client)");
    let body = serde_json::json!({ "error": kyomi_core::sanitize_error(&raw) });
    (status, axum::Json(body)).into_response()
}

// ===========================================================================
// Handler
// ===========================================================================

/// `POST /api/v1/query-arrow`
///
/// Execute SQL against a workspace datasource and return Arrow IPC bytes.
#[instrument(skip_all, fields(slug = %req.datasource_slug))]
async fn query_arrow(
    State(state): State<AppState>,
    auth: kyomi_auth::middleware::AuthUser,
    Json(req): Json<QueryArrowRequest>,
) -> Response {
    // ------------------------------------------------------------------
    // 1. Input validation — limit and offset must not be negative.
    // ------------------------------------------------------------------
    if let Some(l) = req.limit
        && l < 0
    {
        return error_response(StatusCode::BAD_REQUEST, "limit must not be negative");
    }
    if let Some(o) = req.offset
        && o < 0
    {
        return error_response(StatusCode::BAD_REQUEST, "offset must not be negative");
    }

    let limit = req.limit.map(|v| v as u32);
    let offset = req.offset.map(|v| v as u32);
    let include_total = req.include_total.unwrap_or(false);

    // ------------------------------------------------------------------
    // 2. Resolve workspace_id from auth context.
    // ------------------------------------------------------------------
    let workspace_id = match auth.workspace.workspace_id.as_deref() {
        Some(id) => id.to_string(),
        None => {
            return error_response(StatusCode::BAD_REQUEST, "workspace context required");
        }
    };

    // ------------------------------------------------------------------
    // 3. Resolve datasource — 403 if not found/accessible.
    // ------------------------------------------------------------------
    let ds = match kyomi_auth::datasource_service::resolve_datasource(
        &state.db,
        &req.datasource_slug,
        &workspace_id,
        false,
    )
    .await
    {
        Ok(ds) => ds,
        Err(kyomi_core::Error::NotFound(_)) => {
            return error_response(
                StatusCode::FORBIDDEN,
                format!(
                    "datasource '{}' not found or not accessible",
                    req.datasource_slug
                ),
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "datasource resolution failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error",
            );
        }
    };

    // ------------------------------------------------------------------
    // 4. Resolve per-user credentials (decrypt from DB).
    // ------------------------------------------------------------------
    let credentials = if ds.connection_type != "connect" {
        let user_cred = match kyomi_auth::datasource_service::get_user_credential(
            &state.db,
            &auth.user_id,
            &ds.id,
        )
        .await
        {
            Ok(cred) => cred,
            Err(e) => {
                tracing::error!("failed to load user credential: {e}");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error",
                );
            }
        };
        if let Some(ref cred) = user_cred {
            kyomi_auth::encryption::decrypt_json(&cred.credentials, &state.encryption_key)
                .unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        }
    } else {
        serde_json::json!({})
    };

    // ------------------------------------------------------------------
    // 5. Build user context for BigQuery OAuth (kyomi_oauth auth mode).
    // ------------------------------------------------------------------
    let user_context = build_user_context(&state, &auth).await;

    // ------------------------------------------------------------------
    // 6. Create provider (Connect-vs-direct branching, timeout).
    // ------------------------------------------------------------------
    // `ds.connection_config` came straight from the database and may hold
    // encrypted `COMMON_SENSITIVE` fields (e.g. `ssh_private_key`) — the
    // driver always needs plaintext.
    let decrypted_config = kyomi_auth::credential_service::decrypt_connection_config_secrets(
        &ds.connection_config,
        &state.encryption_key,
    );

    let ds_type: kyomi_core::datasource_registry::DatasourceType = ds.datasource_type.into();
    let provider = match kyomi_datasource_server::create_provider_from_parts(
        &ds.id,
        &ds.connection_type,
        &decrypted_config,
        ds_type,
        credentials,
        user_context,
        Some(&state.connect_registry),
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("timed out") {
                return error_response(StatusCode::UNPROCESSABLE_ENTITY, "connection timed out");
            }
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("failed to connect to datasource: {e}"),
            );
        }
    };

    // ------------------------------------------------------------------
    // 7. Dispatch — paginated vs. streaming path.
    // ------------------------------------------------------------------
    if limit.is_some() {
        execute_paginated(provider, &req.sql, limit, offset, include_total, req.job_id.as_deref()).await
    } else {
        execute_streaming(provider, &req.sql).await
    }
}

// ===========================================================================
// Paginated path
// ===========================================================================

/// Execute with `limit` / `offset` (SQL editor path).
///
/// Calls `execute_query`, serialises the single `RecordBatch` (or an empty
/// schema) as a self-contained Arrow IPC stream, and sets pagination headers.
async fn execute_paginated(
    provider: Box<dyn kyomi_datasource_server::DatasourceProvider>,
    sql: &str,
    limit: Option<u32>,
    offset: Option<u32>,
    include_total: bool,
    job_id: Option<&str>,
) -> Response {
    let result = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_QUERY,
        provider.execute_query(sql, limit, offset, include_total, job_id),
    )
    .await
    {
        Ok(Ok(r)) => {
            provider.close().await;
            r
        }
        Ok(Err(e)) => {
            provider.close().await;
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("query execution failed: {e}"),
            );
        }
        Err(_) => {
            provider.close().await;
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "query timed out",
            );
        }
    };

    if result.status == QueryStatus::Error {
        let msg = result.error.unwrap_or_else(|| "query execution failed".into());
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, msg);
    }

    // Serialise to Arrow IPC streaming bytes.
    let ipc_bytes = match result_to_ipc_bytes(&result) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("Arrow IPC serialisation failed: {e}");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal server error");
        }
    };

    // Build response headers.
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/vnd.apache.arrow.stream"),
    );
    if let Some(total) = result.total_rows
        && let Ok(v) = HeaderValue::from_str(&total.to_string())
    {
        headers.insert(HeaderName::from_static("x-total-rows"), v);
    }
    if let Some(ref jid) = result.job_id
        && let Ok(v) = HeaderValue::from_str(jid)
    {
        headers.insert(HeaderName::from_static("x-job-id"), v);
    }
    headers.insert(
        HeaderName::from_static("x-has-more"),
        HeaderValue::from_static(if result.has_more { "true" } else { "false" }),
    );

    (StatusCode::OK, headers, ipc_bytes).into_response()
}

/// Serialise a `QueryResult` (with or without a `record_batch`) as Arrow IPC.
///
/// If `record_batch` is `None` (e.g. DDL), writes schema-only with zero rows.
fn result_to_ipc_bytes(
    result: &kyomi_datasource_server::QueryResult,
) -> Result<Vec<u8>, arrow_schema::ArrowError> {
    use arrow_ipc::writer::StreamWriter;

    if let Some(batch) = &result.record_batch {
        let mut buf = Vec::new();
        let mut writer = StreamWriter::try_new(&mut buf, batch.schema_ref())?;
        writer.write(batch)?;
        writer.finish()?;
        Ok(buf)
    } else {
        // No data batch: build an empty schema from column metadata and write
        // a zero-row stream so the client receives a valid IPC container.
        let columns = result.columns.as_deref().unwrap_or(&[]);
        let fields: Vec<arrow_schema::Field> = columns
            .iter()
            .map(|c| simple_type_to_field(&c.name, c.col_type))
            .collect();
        let schema = std::sync::Arc::new(arrow_schema::Schema::new(fields));

        let mut buf = Vec::new();
        let mut writer = StreamWriter::try_new(&mut buf, &schema)?;
        writer.finish()?;
        Ok(buf)
    }
}

// ===========================================================================
// Streaming path
// ===========================================================================

/// Execute without a limit (chartml streaming path).
///
/// Calls `execute_query_stream_arrow` and streams Arrow IPC bytes to the HTTP
/// body as each batch arrives — no server-side buffering of the full result set.
/// Uses a tokio `DuplexStream` to bridge the Arrow IPC writer (sync writes) to
/// the axum `Body` (async reads).
async fn execute_streaming(
    provider: Box<dyn kyomi_datasource_server::DatasourceProvider>,
    sql: &str,
) -> Response {
    let arrow_stream = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_QUERY,
        provider.execute_query_stream_arrow(sql, None, None, false, None),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            provider.close().await;
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("query stream failed: {e}"),
            );
        }
        Err(_) => {
            provider.close().await;
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "query timed out",
            );
        }
    };

    let (writer_half, reader_half) = tokio::io::duplex(64 * 1024);

    tokio::spawn(drive_arrow_stream(arrow_stream, writer_half, provider));

    let reader_stream =
        tokio_util::io::ReaderStream::new(reader_half);

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/vnd.apache.arrow.stream"),
    );
    (
        StatusCode::OK,
        headers,
        axum::body::Body::from_stream(reader_stream),
    )
        .into_response()
}

/// Background task: reads `ArrowStreamEvent`s and writes Arrow IPC bytes to
/// the duplex writer. Drops the writer on completion or error, which signals
/// EOF to the reader half (and thus to the HTTP body stream).
async fn drive_arrow_stream(
    mut arrow_stream: kyomi_connect_protocol::ArrowStream,
    writer_half: tokio::io::DuplexStream,
    provider: Box<dyn kyomi_datasource_server::DatasourceProvider>,
) {
    use arrow_ipc::reader::StreamReader as IpcReader;
    use arrow_ipc::writer::StreamWriter as IpcWriter;
    use kyomi_connect_protocol::ArrowStreamEvent;
    use tokio::io::AsyncWriteExt;

    let mut writer_half = writer_half;
    let mut ipc_writer: Option<IpcWriter<Vec<u8>>> = None;

    'stream: while let Some(event_result) = arrow_stream.next().await {
        let event = match event_result {
            Ok(e) => e,
            Err(e) => {
                tracing::error!("Arrow stream error: {e}");
                break 'stream;
            }
        };

        match event {
            ArrowStreamEvent::Schema { schema_ipc, .. } => {
                let reader = match IpcReader::try_new(std::io::Cursor::new(schema_ipc), None) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!("failed to decode Arrow schema: {e}");
                        break 'stream;
                    }
                };
                let decoded_schema = reader.schema();
                match IpcWriter::try_new(Vec::new(), &decoded_schema) {
                    Ok(w) => ipc_writer = Some(w),
                    Err(e) => {
                        tracing::error!("failed to create Arrow IPC writer: {e}");
                        break 'stream;
                    }
                }
            }
            ArrowStreamEvent::Batch { ipc_bytes, .. } => {
                let w = match ipc_writer.as_mut() {
                    Some(w) => w,
                    None => {
                        tracing::error!("received Arrow batch before schema");
                        break 'stream;
                    }
                };

                let batch_reader = match IpcReader::try_new(std::io::Cursor::new(ipc_bytes), None)
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!("failed to decode Arrow batch: {e}");
                        break 'stream;
                    }
                };

                let mut batch_error = false;
                for batch_result in batch_reader {
                    let batch = match batch_result {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::error!("failed to read Arrow batch: {e}");
                            batch_error = true;
                            break;
                        }
                    };
                    if let Err(e) = w.write(&batch) {
                        tracing::error!("failed to write Arrow batch: {e}");
                        batch_error = true;
                        break;
                    }
                }
                if batch_error {
                    break 'stream;
                }

                // Flush buffered IPC bytes to the HTTP body.
                let buf = w.get_mut();
                if !buf.is_empty() {
                    let chunk = std::mem::take(buf);
                    if writer_half.write_all(&chunk).await.is_err() {
                        break 'stream;
                    }
                }
            }
            // Arrow IPC streaming format does not support per-batch schema
            // metadata — it's sent once in the initial Schema message. Execution
            // statistics from the Complete event are not transmitted to streaming
            // clients. Callers that need execution_time_ms/bytes_processed should
            // use the paginated path (with `limit` set) which exposes them via
            // HTTP response headers.
            ArrowStreamEvent::Complete { .. } => {}
        }
    }

    // Finish the IPC stream (writes EOS marker) and flush remaining bytes.
    if let Some(mut w) = ipc_writer {
        if let Err(e) = w.finish() {
            tracing::warn!("failed to finish Arrow IPC stream: {e}");
        }
        let buf = w.get_mut();
        if !buf.is_empty() {
            let chunk = std::mem::take(buf);
            let _ = writer_half.write_all(&chunk).await;
        }
    }

    provider.close().await;
    // Drop writer_half → signals EOF to the reader → HTTP body ends.
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Build a `UserContext` for BigQuery provider creation (OAuth path).
///
/// Delegates to `kyomi_auth::google_oauth::build_datasource_user_context`.
/// Errors are swallowed — a missing or expired token causes `oauth_data`
/// to be `None`, and BigQuery falls back to other auth modes.
async fn build_user_context(
    state: &AppState,
    auth: &kyomi_auth::middleware::AuthUser,
) -> Option<kyomi_datasource_server::UserContext> {
    let workspace_id = auth.workspace.workspace_id.clone().unwrap_or_default();
    kyomi_auth::google_oauth::build_datasource_user_context(
        &state.db,
        &auth.user_id,
        Some(&state.encryption_key),
        state.config.google_oauth_client_id.as_deref(),
        state.config.google_oauth_client_secret.as_deref(),
        auth.email.clone(),
        workspace_id,
    )
    .await
    .ok()
    .flatten()
}

/// Map a `SimpleType` to an Arrow `Field` with the appropriate data type.
fn simple_type_to_field(
    name: &str,
    col_type: kyomi_datasource_server::SimpleType,
) -> arrow_schema::Field {
    use kyomi_datasource_server::SimpleType;

    let data_type = match col_type {
        SimpleType::Number => arrow_schema::DataType::Float64,
        SimpleType::Boolean => arrow_schema::DataType::Boolean,
        SimpleType::Date => arrow_schema::DataType::Date32,
        SimpleType::Time => arrow_schema::DataType::Time64(arrow_schema::TimeUnit::Microsecond),
        SimpleType::Timestamp => {
            arrow_schema::DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None)
        }
        SimpleType::TimestampTz => arrow_schema::DataType::Timestamp(
            arrow_schema::TimeUnit::Microsecond,
            Some("UTC".into()),
        ),
        SimpleType::String | SimpleType::Unknown => arrow_schema::DataType::Utf8,
    };
    arrow_schema::Field::new(name, data_type, true)
}
