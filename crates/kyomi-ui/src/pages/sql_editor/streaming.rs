// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket streaming handler for SQL Editor query results.
//!
//! Listens for `query_stream_*` WebSocket messages and updates the
//! corresponding result tab progressively as data arrives.
//!
//! Follows the same WebSocket pattern as `crates/kyomi-ui/src/utils/websocket.rs`
//! (`use_dashboard_updates`).
//!
//! React reference: `apps/frontend/src/hooks/useQueryStream.js` and
//! `apps/frontend/src/components/SQLEditor.jsx` (`startStreamingQuery`).

use super::state::SqlEditorState;
#[cfg(target_arch = "wasm32")]
use super::types::{ColumnMetadata, QueryError, QueryResult, QueryStatus};

// ---------------------------------------------------------------------------
// WASM implementation (browser)
// ---------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
mod inner {
    use super::*;

    use leptos::prelude::*;
    use send_wrapper::SendWrapper;
    use wasm_bindgen::prelude::*;
    use web_sys::{CloseEvent, MessageEvent, WebSocket};

    /// Maximum reconnect attempts before giving up.
    const MAX_RECONNECT_ATTEMPTS: u32 = 10;

    /// Holds a live WebSocket connection and its event handler closures.
    struct WsHandle {
        _ws: WebSocket,
        _on_open: Closure<dyn FnMut(JsValue)>,
        _on_message: Closure<dyn FnMut(MessageEvent)>,
        _on_close: Closure<dyn FnMut(CloseEvent)>,
        _on_error: Closure<dyn FnMut(JsValue)>,
    }

    /// Set up a WebSocket listener that handles `query_stream_*` events and
    /// updates the corresponding result tabs in `state`.
    ///
    /// Call this once at the SQL Editor page level. It connects to the same
    /// WebSocket endpoint as dashboard updates and filters for query stream
    /// message types.
    ///
    /// The `query_running` signal is set to `false` when a stream completes
    /// or errors, matching the React behavior where streaming callbacks
    /// clear the running state.
    pub fn use_query_stream_handler(
        user_id: String,
        workspace_id: String,
        state: SqlEditorState,
        query_running: WriteSignal<bool>,
    ) {
        let ws_handle: StoredValue<Option<SendWrapper<WsHandle>>> = StoredValue::new(None);
        let attempt: StoredValue<u32> = StoredValue::new(0);

        let uid = user_id.clone();
        let wid = workspace_id.clone();
        leptos::task::spawn_local(async move {
            connect(uid, wid, state, query_running, ws_handle, attempt).await;
        });

        on_cleanup(move || {
            ws_handle.update_value(|h| {
                if let Some(handle) = h.take() {
                    let _ = handle._ws.close();
                }
            });
        });
    }

    // TODO: `fetch_ws_token` and `build_ws_url` duplicate logic from
    // `crates/kyomi-ui/src/utils/websocket.rs`. Consolidate into a shared
    // `utils/ws.rs` helper in a future cleanup pass.

    /// Fetch a one-time WebSocket authentication token.
    async fn fetch_ws_token() -> Result<String, String> {
        let window = web_sys::window().ok_or("No window object")?;
        let resp_value =
            wasm_bindgen_futures::JsFuture::from(window.fetch_with_str("/api/v1/auth/websocket-token"))
                .await
                .map_err(|e| format!("fetch failed: {e:?}"))?;

        let resp: web_sys::Response = resp_value
            .dyn_into()
            .map_err(|_| "response is not a Response object")?;

        if !resp.ok() {
            return Err(format!("WS token request failed with status {}", resp.status()));
        }

        let json = wasm_bindgen_futures::JsFuture::from(
            resp.json().map_err(|e| format!("json() failed: {e:?}"))?,
        )
        .await
        .map_err(|e| format!("json parse failed: {e:?}"))?;

        let token = js_sys::Reflect::get(&json, &JsValue::from_str("token"))
            .map_err(|_| "no 'token' field in response")?
            .as_string()
            .ok_or_else(|| "token is not a string".to_string())?;

        Ok(token)
    }

    /// Build the WebSocket URL.
    fn build_ws_url(user_id: &str, workspace_id: &str, token: &str) -> Result<String, String> {
        let window = web_sys::window().ok_or("No window object")?;
        let location = window.location();
        let protocol = location.protocol().map_err(|_| "no protocol")?;
        let host = location.host().map_err(|_| "no host")?;
        let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
        Ok(format!(
            "{ws_protocol}//{host}/ws/{workspace_id}_{user_id}?token={token}"
        ))
    }

    /// Open a WebSocket connection and wire up event handlers for query
    /// streaming events.
    async fn connect(
        user_id: String,
        workspace_id: String,
        state: SqlEditorState,
        query_running: WriteSignal<bool>,
        ws_handle: StoredValue<Option<SendWrapper<WsHandle>>>,
        attempt: StoredValue<u32>,
    ) {
        let token = match fetch_ws_token().await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("Query stream WS: failed to fetch token: {e}");
                schedule_reconnect(user_id, workspace_id, state, query_running, ws_handle, attempt);
                return;
            }
        };

        let url = match build_ws_url(&user_id, &workspace_id, &token) {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("Query stream WS: failed to build URL: {e}");
                schedule_reconnect(user_id, workspace_id, state, query_running, ws_handle, attempt);
                return;
            }
        };

        let ws = match WebSocket::new(&url) {
            Ok(ws) => ws,
            Err(e) => {
                tracing::error!("Query stream WS: WebSocket::new failed: {:?}", e);
                schedule_reconnect(user_id, workspace_id, state, query_running, ws_handle, attempt);
                return;
            }
        };

        // -- onopen -------------------------------------------------------
        let on_open = Closure::<dyn FnMut(JsValue)>::new(move |_event: JsValue| {
            tracing::info!("Query stream WebSocket connected");
            attempt.set_value(0);
        });
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));

        // -- onmessage ----------------------------------------------------
        let on_message = {
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let Some(text) = event.data().as_string() else {
                    return;
                };
                handle_ws_message(&text, state, query_running);
            })
        };
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        // -- onclose ------------------------------------------------------
        let on_close = {
            let user_id = user_id.clone();
            let workspace_id = workspace_id.clone();
            Closure::<dyn FnMut(CloseEvent)>::new(move |_event: CloseEvent| {
                tracing::info!("Query stream WebSocket closed, scheduling reconnect");
                ws_handle.update_value(|h| {
                    drop(h.take());
                });
                schedule_reconnect(
                    user_id.clone(),
                    workspace_id.clone(),
                    state,
                    query_running,
                    ws_handle,
                    attempt,
                );
            })
        };
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));

        // -- onerror ------------------------------------------------------
        let on_error = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            tracing::error!("Query stream WebSocket error: {:?}", e);
        });
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        // -- store handle -------------------------------------------------
        ws_handle.set_value(Some(SendWrapper::new(WsHandle {
            _ws: ws,
            _on_open: on_open,
            _on_message: on_message,
            _on_close: on_close,
            _on_error: on_error,
        })));
    }

    /// Schedule a reconnect with exponential backoff.
    fn schedule_reconnect(
        user_id: String,
        workspace_id: String,
        state: SqlEditorState,
        query_running: WriteSignal<bool>,
        ws_handle: StoredValue<Option<SendWrapper<WsHandle>>>,
        attempt: StoredValue<u32>,
    ) {
        let current_attempt = attempt.get_value();
        if current_attempt >= MAX_RECONNECT_ATTEMPTS {
            tracing::error!(
                "Query stream WS: giving up after {} attempts",
                MAX_RECONNECT_ATTEMPTS
            );

            // Any tabs still in Streaming status will never receive
            // further events — mark them as errors so the UI is not
            // stuck forever.
            let tabs = state.tabs.get_untracked();
            for tab in &tabs {
                if tab.status == QueryStatus::Streaming {
                    let tid = tab.id.clone();
                    state.update_tab(&tid, |t| {
                        t.status = QueryStatus::Error;
                        t.error = Some(QueryError {
                            message: "WebSocket connection lost".to_string(),
                            code: None,
                            line: None,
                            column: None,
                        });
                    });
                }
            }
            query_running.set(false);

            return;
        }

        let delay_ms = std::cmp::min(
            1000u32.saturating_mul(2u32.saturating_pow(current_attempt)),
            30_000,
        );
        attempt.set_value(current_attempt.saturating_add(1));

        tracing::info!(
            "Query stream WS: reconnect attempt {} in {}ms",
            current_attempt + 1,
            delay_ms
        );

        let timeout = gloo_timers::callback::Timeout::new(delay_ms, move || {
            leptos::task::spawn_local(async move {
                connect(user_id, workspace_id, state, query_running, ws_handle, attempt).await;
            });
        });
        timeout.forget();
    }

    /// Parse and dispatch a single WebSocket message.
    ///
    /// The server sends JSON envelopes:
    /// ```json
    /// { "type": "query_stream_header", "data": { "request_id": "...", "columns": [...], ... } }
    /// { "type": "query_stream_chunk",  "data": { "request_id": "...", "rows": [...] } }
    /// { "type": "query_stream_complete", "data": { "request_id": "...", ... } }
    /// { "type": "query_stream_error",  "data": { "request_id": "...", "error": "..." } }
    /// ```
    ///
    /// We match the `request_id` to the tab that has it stored in
    /// `result.query_handle.job_id` (set by `execution.rs` when starting a stream).
    fn handle_ws_message(
        text: &str,
        state: SqlEditorState,
        query_running: WriteSignal<bool>,
    ) {
        #[derive(serde::Deserialize)]
        struct RawMessage {
            #[serde(rename = "type")]
            message_type: String,
            #[serde(default)]
            data: Option<serde_json::Value>,
        }

        let Ok(msg) = serde_json::from_str::<RawMessage>(text) else {
            return;
        };

        // Only handle query_stream_* messages.
        if !msg.message_type.starts_with("query_stream_") {
            return;
        }

        let Some(data) = msg.data else {
            return;
        };

        let Some(request_id) = data.get("request_id").and_then(|v| v.as_str()) else {
            return;
        };

        // Find the tab that matches this request_id.
        // The request_id is stored in result.query_handle.job_id by execution.rs.
        let request_id_owned = request_id.to_string();
        let matching_tab_id = state.tabs.get_untracked().iter().find_map(|tab| {
            tab.result
                .as_ref()
                .and_then(|r| r.query_handle.as_ref())
                .and_then(|qh| qh.job_id.as_ref())
                .filter(|jid| jid.as_str() == request_id_owned.as_str())
                .map(|_| tab.id.clone())
        });

        let Some(tab_id) = matching_tab_id else {
            return;
        };

        match msg.message_type.as_str() {
            "query_stream_header" => {
                // Parse columns from the header message.
                let columns: Vec<ColumnMetadata> = data
                    .get("columns")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();

                let total_rows = data
                    .get("total_rows")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);

                state.update_tab(&tab_id, move |tab| {
                    tab.status = QueryStatus::Streaming;
                    tab.result = Some(QueryResult {
                        columns,
                        rows: Vec::new(),
                        row_count: 0,
                        total_rows,
                        // Preserve the query_handle so subsequent events can
                        // still match by request_id.
                        query_handle: tab.result.as_ref().and_then(|r| r.query_handle.clone()),
                        execution_time: None,
                        bytes_processed: None,
                        has_more: false,
                    });
                });
            }

            "query_stream_chunk" => {
                // Parse rows from the chunk message.
                let chunk_rows: Vec<Vec<serde_json::Value>> = data
                    .get("rows")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();

                state.update_tab(&tab_id, move |tab| {
                    tab.status = QueryStatus::Streaming;
                    if let Some(ref mut result) = tab.result {
                        result.rows.extend(chunk_rows);
                        result.row_count = result.rows.len();
                    }
                });
            }

            "query_stream_complete" => {
                let execution_time = data
                    .get("execution_time_ms")
                    .and_then(|v| v.as_u64());
                let bytes_processed = data
                    .get("bytes_processed")
                    .and_then(|v| v.as_u64());
                let total_rows_returned = data
                    .get("total_rows_returned")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);

                state.update_tab(&tab_id, move |tab| {
                    tab.status = QueryStatus::Success;
                    if let Some(ref mut result) = tab.result {
                        result.execution_time = execution_time;
                        result.bytes_processed = bytes_processed;
                        if let Some(total) = total_rows_returned {
                            result.total_rows = Some(total);
                        } else {
                            result.total_rows = Some(result.rows.len());
                        }
                    }
                });

                query_running.set(false);

                // Fire-and-forget: save streaming query to history.
                let tabs = state.tabs.get_untracked();
                if let Some(tab) = tabs.iter().find(|t| t.id == tab_id) {
                    let query_text = tab.query.clone();
                    let ds_slug = tab.datasource_slug.clone();
                    let (exec_time, bytes, rows) = tab
                        .result
                        .as_ref()
                        .map(|r| {
                            (
                                r.execution_time.map(|t| t as i32),
                                r.bytes_processed.map(|b| b as i64),
                                Some(r.row_count as i32),
                            )
                        })
                        .unwrap_or((None, None, None));

                    super::super::execution::save_to_history(
                        query_text, exec_time, bytes, rows, "success", None, ds_slug,
                    );
                }
            }

            "query_stream_error" => {
                let error_msg = data
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Stream error")
                    .to_string();

                // Extract query info before updating the tab.
                let tabs = state.tabs.get_untracked();
                let query_info = tabs.iter().find(|t| t.id == tab_id).map(|tab| {
                    (tab.query.clone(), tab.datasource_slug.clone())
                });

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

                // Fire-and-forget: save error to history.
                if let Some((query_text, ds_slug)) = query_info {
                    super::super::execution::save_to_history(
                        query_text, None, None, None, "error", Some(error_msg_for_history), ds_slug,
                    );
                }
            }

            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// SSR stub (server-side rendering — no WebSocket available)
// ---------------------------------------------------------------------------
#[cfg(not(target_arch = "wasm32"))]
mod inner {
    use super::*;
    use leptos::prelude::*;

    /// SSR stub: no-op. WebSocket is browser-only.
    pub fn use_query_stream_handler(
        _user_id: String,
        _workspace_id: String,
        _state: SqlEditorState,
        _query_running: WriteSignal<bool>,
    ) {
        // Nothing to do on the server.
    }
}

// Re-export the platform-appropriate implementation.
pub use inner::use_query_stream_handler;
