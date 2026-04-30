// SPDX-License-Identifier: AGPL-3.0-or-later

//! Browser-side helpers for consuming the Arrow IPC endpoint.
//!
//! Both functions POST to `POST /api/v1/query-arrow` and decode the response
//! body as Arrow IPC bytes via [`chartml_core::data::DataTable::from_ipc_bytes`].
//!
//! ## Two execution paths
//!
//! - [`fetch_arrow_buffered`] — passes `limit` / `offset` in the request body.
//!   The server uses the **paginated** path and returns `X-Total-Rows`,
//!   `X-Job-Id`, and `X-Has-More` response headers.
//! - [`fetch_arrow_stream`] — omits `limit` so the server uses the **streaming**
//!   path.  The response is consumed fully as an `arrayBuffer()` for now; true
//!   incremental decoding via `ReadableStream` will be wired up when DataFusion
//!   streaming input lands (KYO-244).
//!
//! Both functions are `#[cfg(target_arch = "wasm32")]` because they rely on
//! browser-only APIs (`web_sys`, `js_sys`, `wasm_bindgen_futures`).

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, RequestCredentials, RequestInit, Response};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result of a buffered (paginated) Arrow fetch.
pub struct BufferedArrowResult {
    /// Decoded query result as a `DataTable`.
    pub data: chartml_core::data::DataTable,
    /// Total row count across all pages, if `include_total` was `true` and the
    /// server computed it.
    pub total_rows: Option<i64>,
    /// Server-side job identifier for continuation queries.
    pub job_id: Option<String>,
    /// `true` when more rows are available beyond the current page.
    pub has_more: bool,
}

// ---------------------------------------------------------------------------
// fetch_arrow_buffered — SQL editor paginated path
// ---------------------------------------------------------------------------

/// Fetch a page of query results from the Arrow endpoint.
///
/// Sends `datasource_slug`, `sql`, `limit`, `offset`, `include_total`, and
/// (optionally) `job_id` as a JSON body.  The server executes via the
/// paginated path and returns Arrow IPC bytes plus pagination metadata in
/// response headers.
///
/// # Errors
///
/// Returns `Err(String)` if:
/// - The network request fails.
/// - The server returns a 4xx / 5xx status (error message from the JSON body
///   `{"error": "..."}` or the raw response text is included).
/// - Arrow IPC decoding fails (e.g. truncated response from a query timeout).
pub async fn fetch_arrow_buffered(
    datasource_slug: &str,
    sql: &str,
    limit: u32,
    offset: u32,
    include_total: bool,
    job_id: Option<&str>,
) -> Result<BufferedArrowResult, String> {
    let mut body = serde_json::json!({
        "datasource_slug": datasource_slug,
        "sql": sql,
        "limit": limit,
        "offset": offset,
        "include_total": include_total,
    });
    if let Some(jid) = job_id {
        body["job_id"] = serde_json::Value::String(jid.to_string());
    }

    let resp = post_arrow_request(&body).await?;

    // Read pagination headers before consuming the body.
    let total_rows = read_header_i64(&resp, "x-total-rows");
    let job_id_out = read_header_str(&resp, "x-job-id");
    let has_more = read_header_str(&resp, "x-has-more")
        .as_deref()
        == Some("true");

    let bytes = response_to_bytes(resp).await?;

    let data =
        chartml_core::data::DataTable::from_ipc_bytes(&bytes).map_err(classify_ipc_error)?;

    Ok(BufferedArrowResult {
        data,
        total_rows,
        job_id: job_id_out,
        has_more,
    })
}

// ---------------------------------------------------------------------------
// fetch_arrow_stream — chartml streaming path (no limit)
// ---------------------------------------------------------------------------

/// Fetch a full (unlimited) query result from the Arrow endpoint.
///
/// Omitting `limit` from the request body causes the server to use the
/// **streaming** execution path, which avoids server-side buffering of large
/// result sets.  The browser still receives the full response as a single
/// `arrayBuffer()` call — true incremental decoding will be wired up in
/// KYO-244 when DataFusion streaming input is implemented.
///
/// # Errors
///
/// Returns `Err(String)` if:
/// - The network request fails.
/// - The server returns a 4xx / 5xx status.
/// - Arrow IPC decoding fails.
pub async fn fetch_arrow_stream(
    datasource_slug: &str,
    sql: &str,
) -> Result<chartml_core::data::DataTable, String> {
    let body = serde_json::json!({
        "datasource_slug": datasource_slug,
        "sql": sql,
    });

    let resp = post_arrow_request(&body).await?;
    let bytes = response_to_bytes(resp).await?;

    chartml_core::data::DataTable::from_ipc_bytes(&bytes).map_err(classify_ipc_error)
}

fn classify_ipc_error(e: impl std::fmt::Display) -> String {
    let msg = e.to_string();
    if msg.contains("unexpected end") || msg.contains("truncated") || msg.contains("EOS") {
        "Query interrupted — the response was incomplete. Please retry.".to_string()
    } else {
        format!("Failed to decode query results: {e}")
    }
}

// ---------------------------------------------------------------------------
// Shared fetch helpers
// ---------------------------------------------------------------------------

/// POST `body` as JSON to `/api/v1/query-arrow` with `credentials: same-origin`.
///
/// Returns the [`Response`] on HTTP 2xx, or `Err(String)` on:
/// - Fetch / network error.
/// - HTTP 4xx / 5xx (error message extracted from `{"error": "..."}` body).
async fn post_arrow_request(body: &serde_json::Value) -> Result<Response, String> {
    let opts = RequestInit::new();
    opts.set_method("POST");
    opts.set_credentials(RequestCredentials::SameOrigin);

    let headers = Headers::new().map_err(|e| format!("{e:?}"))?;
    headers
        .set("Content-Type", "application/json")
        .map_err(|e| format!("{e:?}"))?;
    opts.set_headers(&headers);

    let body_str = serde_json::to_string(body).map_err(|e| e.to_string())?;
    opts.set_body(&wasm_bindgen::JsValue::from_str(&body_str));

    let window = web_sys::window().ok_or("no window")?;
    let resp_value = JsFuture::from(window.fetch_with_str_and_init("/api/v1/query-arrow", &opts))
        .await
        .map_err(|e| format!("{e:?}"))?;
    let resp: Response = resp_value
        .dyn_into()
        .map_err(|v| format!("response is not a Response object: {v:?}"))?;

    if resp.ok() {
        return Ok(resp);
    }

    // --- Error path: extract message from JSON body if possible ---
    let status = resp.status();
    if let Ok(text_promise) = resp.text() {
        if let Ok(text_value) = JsFuture::from(text_promise).await {
            if let Some(text) = text_value.as_string() {
                // Try to parse {"error": "..."} envelope.
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(msg) = json.get("error").and_then(|v| v.as_str()) {
                        return Err(msg.to_string());
                    }
                }
                if !text.is_empty() {
                    return Err(text);
                }
            }
        }
    }
    Err(format!("HTTP {status}"))
}

/// Consume the response body as raw bytes via `arrayBuffer()`.
async fn response_to_bytes(resp: Response) -> Result<Vec<u8>, String> {
    let buf_promise = resp.array_buffer().map_err(|e| format!("{e:?}"))?;
    let buf = JsFuture::from(buf_promise)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let uint8_arr = js_sys::Uint8Array::new(&buf);
    Ok(uint8_arr.to_vec())
}

// ---------------------------------------------------------------------------
// Header-reading helpers
// ---------------------------------------------------------------------------

/// Read a response header as a string, returning `None` if absent or on error.
fn read_header_str(resp: &Response, name: &str) -> Option<String> {
    resp.headers().get(name).ok().flatten()
}

/// Read a response header and parse it as `i64`.
fn read_header_i64(resp: &Response, name: &str) -> Option<i64> {
    read_header_str(resp, name)?.parse().ok()
}
