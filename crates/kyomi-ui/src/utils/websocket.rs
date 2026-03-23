// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket client for receiving real-time dashboard update events.
//!
//! On WASM: fetches a WebSocket authentication token from `/api/v1/auth/websocket-token`,
//! connects to `/ws/{workspace_id}_{user_id}?token={ws_token}`, parses incoming JSON
//! messages, and updates a reactive signal when dashboard-related events arrive.
//! Handles auto-reconnect on disconnect with exponential backoff (max 10 attempts).
//!
//! On SSR: returns a never-updating signal (WebSocket is browser-only).

use serde::{Deserialize, Serialize};

/// A dashboard update event received via WebSocket.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DashboardUpdateEvent {
    pub dashboard_id: String,
    pub event_type: String,
    pub data: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// WASM implementation (browser)
// ---------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
mod inner {
    use super::DashboardUpdateEvent;

    /// Maximum number of consecutive reconnect attempts before giving up.
    const MAX_RECONNECT_ATTEMPTS: u32 = 10;
    use leptos::prelude::*;
    use send_wrapper::SendWrapper;
    use wasm_bindgen::prelude::*;
    use web_sys::{CloseEvent, MessageEvent, WebSocket};

    /// Holds a live WebSocket connection and its event handler closures.
    ///
    /// The closures must be stored to prevent them from being dropped (which
    /// would unregister the JS callbacks). Wrapped in `SendWrapper` because
    /// JS values are `!Send` but Leptos signals require `Send` on the server
    /// (on WASM `SendWrapper` is a transparent no-op).
    struct WsHandle {
        _ws: WebSocket,
        _on_open: Closure<dyn FnMut(JsValue)>,
        _on_message: Closure<dyn FnMut(MessageEvent)>,
        _on_close: Closure<dyn FnMut(CloseEvent)>,
        _on_error: Closure<dyn FnMut(JsValue)>,
    }

    /// Create a reactive signal that receives dashboard update events via
    /// WebSocket. Connects to `/ws/{workspace_id}_{user_id}?token={ws_token}`
    /// and auto-reconnects on close with exponential backoff (1 s, 2 s, 4 s,
    /// ... capped at 30 s, max 10 attempts).
    pub fn use_dashboard_updates(
        user_id: String,
        workspace_id: String,
    ) -> ReadSignal<Option<DashboardUpdateEvent>> {
        let (signal, set_signal) = signal::<Option<DashboardUpdateEvent>>(None);

        // Store the current connection handle so it lives as long as the
        // owning component. `StoredValue` keeps it across re-renders.
        let ws_handle: StoredValue<Option<SendWrapper<WsHandle>>> =
            StoredValue::new(None);

        // Reconnect attempt counter — shared via StoredValue so the
        // reconnect closure can read/write it.
        let attempt: StoredValue<u32> = StoredValue::new(0);

        // Initial connection — must be async to fetch the WS token first.
        let uid = user_id.clone();
        let wid = workspace_id.clone();
        leptos::task::spawn_local(async move {
            connect(uid, wid, set_signal, ws_handle, attempt).await;
        });

        // Clean up on component teardown.
        on_cleanup(move || {
            ws_handle.update_value(|h| {
                if let Some(handle) = h.take() {
                    // Close the WebSocket; closures are dropped automatically.
                    let _ = handle._ws.close();
                }
            });
        });

        signal
    }

    /// Fetch a one-time WebSocket authentication token from the server.
    ///
    /// Calls `/api/v1/auth/websocket-token` and returns the token string.
    /// Used by both dashboard updates and SQL editor query streaming.
    pub async fn fetch_ws_token() -> Result<String, String> {
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

        // Expected shape: { "token": "..." }
        let token = js_sys::Reflect::get(&json, &JsValue::from_str("token"))
            .map_err(|_| "no 'token' field in response")?
            .as_string()
            .ok_or_else(|| "token is not a string".to_string())?;

        Ok(token)
    }

    /// Build the WebSocket URL with the correct protocol, path, and token.
    ///
    /// Derives the WebSocket protocol (`ws:` / `wss:`) from the current page's
    /// `location.protocol` and constructs the standard path:
    /// `{ws_protocol}//{host}/ws/{workspace_id}_{user_id}?token={token}`
    ///
    /// Used by dashboard updates, SQL editor query streaming, and the chat
    /// WebSocket client.
    pub fn build_ws_url(
        user_id: &str,
        workspace_id: &str,
        token: &str,
    ) -> Result<String, String> {
        let window = web_sys::window().ok_or("No window object")?;
        let location = window.location();
        let protocol = location.protocol().map_err(|_| "no protocol")?;
        let host = location.host().map_err(|_| "no host")?;

        let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };

        Ok(format!(
            "{ws_protocol}//{host}/ws/{workspace_id}_{user_id}?token={token}"
        ))
    }

    /// Open a WebSocket connection and wire up event handlers.
    async fn connect(
        user_id: String,
        workspace_id: String,
        set_signal: WriteSignal<Option<DashboardUpdateEvent>>,
        ws_handle: StoredValue<Option<SendWrapper<WsHandle>>>,
        attempt: StoredValue<u32>,
    ) {
        // Fetch a WS authentication token.
        let token = match fetch_ws_token().await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("Failed to fetch WebSocket token: {e}");
                schedule_reconnect(user_id, workspace_id, set_signal, ws_handle, attempt);
                return;
            }
        };

        let url = match build_ws_url(&user_id, &workspace_id, &token) {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("Failed to build WebSocket URL: {e}");
                schedule_reconnect(user_id, workspace_id, set_signal, ws_handle, attempt);
                return;
            }
        };

        let ws = match WebSocket::new(&url) {
            Ok(ws) => ws,
            Err(e) => {
                tracing::error!("WebSocket::new failed: {:?}", e);
                schedule_reconnect(user_id, workspace_id, set_signal, ws_handle, attempt);
                return;
            }
        };

        // -- onopen -----------------------------------------------------------
        let on_open = {
            Closure::<dyn FnMut(JsValue)>::new(move |_event: JsValue| {
                tracing::info!("WebSocket connected");
                // Reset attempt counter on successful connection.
                attempt.set_value(0);
            })
        };

        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));

        // -- onmessage --------------------------------------------------------
        let on_message = {
            let set_signal = set_signal;
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let Some(text) = event.data().as_string() else {
                    return;
                };

                // Parse the raw JSON envelope. The server sends:
                //   { "type": "dashboard_update", "data": { "dashboard_id": "...", "context_type": "...", ... } }
                // `dashboard_id` and `context_type` are inside `data`, not top-level.
                #[derive(serde::Deserialize)]
                struct RawMessage {
                    #[serde(rename = "type")]
                    message_type: String,
                    #[serde(default)]
                    data: Option<serde_json::Value>,
                }

                let Ok(msg) = serde_json::from_str::<RawMessage>(&text) else {
                    return;
                };

                if msg.message_type != "dashboard_update" {
                    return;
                }

                let Some(data) = msg.data else {
                    return;
                };

                // Extract dashboard_id and context_type from the data object.
                let dashboard_id = data
                    .get("dashboard_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                if dashboard_id.is_empty() {
                    return;
                }

                let event_type = data
                    .get("context_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                set_signal.set(Some(DashboardUpdateEvent {
                    dashboard_id,
                    event_type,
                    data: Some(data),
                }));
            })
        };

        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        // -- onclose ----------------------------------------------------------
        let on_close = {
            let user_id = user_id.clone();
            let workspace_id = workspace_id.clone();
            Closure::<dyn FnMut(CloseEvent)>::new(move |_event: CloseEvent| {
                tracing::info!("WebSocket closed, scheduling reconnect");
                // Drop the old handle so the previous closures are freed.
                ws_handle.update_value(|h| {
                    drop(h.take());
                });
                schedule_reconnect(
                    user_id.clone(),
                    workspace_id.clone(),
                    set_signal,
                    ws_handle,
                    attempt,
                );
            })
        };

        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));

        // -- onerror ----------------------------------------------------------
        let on_error = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            tracing::error!("WebSocket error: {:?}", e);
        });

        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        // -- store handle -----------------------------------------------------
        ws_handle.set_value(Some(SendWrapper::new(WsHandle {
            _ws: ws,
            _on_open: on_open,
            _on_message: on_message,
            _on_close: on_close,
            _on_error: on_error,
        })));
    }

    /// Schedule a reconnect with exponential backoff.
    ///
    /// Stops reconnecting after `MAX_RECONNECT_ATTEMPTS` consecutive failures.
    fn schedule_reconnect(
        user_id: String,
        workspace_id: String,
        set_signal: WriteSignal<Option<DashboardUpdateEvent>>,
        ws_handle: StoredValue<Option<SendWrapper<WsHandle>>>,
        attempt: StoredValue<u32>,
    ) {
        let current_attempt = attempt.get_value();

        if current_attempt >= MAX_RECONNECT_ATTEMPTS {
            tracing::error!(
                "WebSocket reconnect: giving up after {} attempts",
                MAX_RECONNECT_ATTEMPTS,
            );
            return;
        }

        // Saturating exponent to avoid overflow with large attempt values.
        let delay_ms = std::cmp::min(
            1000u32.saturating_mul(2u32.saturating_pow(current_attempt)),
            30_000,
        );
        attempt.set_value(current_attempt.saturating_add(1));

        tracing::info!(
            "WebSocket reconnect attempt {} in {}ms",
            current_attempt + 1,
            delay_ms
        );

        // Fire-and-forget timeout — the Timeout handle is consumed by
        // `forget()` so the callback runs even though we don't store it.
        let timeout = gloo_timers::callback::Timeout::new(delay_ms, move || {
            leptos::task::spawn_local(async move {
                connect(user_id, workspace_id, set_signal, ws_handle, attempt).await;
            });
        });
        timeout.forget();
    }
}

// ---------------------------------------------------------------------------
// SSR stub (server-side rendering — no WebSocket available)
// ---------------------------------------------------------------------------
#[cfg(not(target_arch = "wasm32"))]
mod inner {
    use super::DashboardUpdateEvent;
    use leptos::prelude::*;

    /// SSR stub: returns a signal that never updates.
    pub fn use_dashboard_updates(
        _user_id: String,
        _workspace_id: String,
    ) -> ReadSignal<Option<DashboardUpdateEvent>> {
        let (signal, _) = signal(None);
        signal
    }
}

// Re-export the platform-appropriate implementation.
pub use inner::use_dashboard_updates;

// Re-export shared WebSocket helpers (WASM-only) so other modules
// (sql_editor::streaming, components::chat::websocket_client) can use them
// without duplicating the logic.
#[cfg(target_arch = "wasm32")]
pub use inner::{build_ws_url, fetch_ws_token};
