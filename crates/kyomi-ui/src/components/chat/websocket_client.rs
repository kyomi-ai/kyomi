// SPDX-License-Identifier: AGPL-3.0-or-later

//! Centralized WebSocket context provider for the Leptos frontend.
//!
//! Matches `apps/frontend/src/context/WebSocketContext.jsx` exactly:
//! - Single connection per user (no duplicates)
//! - Automatic reconnection with exponential backoff (max 30s, 10 attempts)
//! - Event subscription system (components subscribe to specific message types)
//! - Proper cleanup on unmount
//! - Connection state tracking
//! - Message deduplication using `type_sessionId_messageId_data` key
//!
//! On SSR: provides a no-op context (WebSocket is browser-only).

use leptos::prelude::*;
use send_wrapper::SendWrapper;
use serde::{Deserialize, Serialize};

/// WebSocket connection state — matches React's `connectionState` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => write!(f, "disconnected"),
            Self::Connecting => write!(f, "connecting"),
            Self::Connected => write!(f, "connected"),
            Self::Reconnecting => write!(f, "reconnecting"),
        }
    }
}

/// An incoming WebSocket message parsed from JSON.
///
/// Uses `serde_json::Value` for the `data` field to avoid tight coupling
/// with every possible message payload. Subscribers pattern-match on
/// `message_type` and parse `data` as needed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebSocketMessage {
    #[serde(rename = "type")]
    pub message_type: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

/// Context provided to all child components via `use_context::<WebSocketContext>()`.
///
/// Provides:
/// - `connection_state` — reactive signal of current connection state
/// - `subscribe` — register a callback for a specific message type
/// - `send` — send a JSON message through the WebSocket
#[derive(Clone)]
pub struct WebSocketContext {
    /// Current connection state as a reactive signal.
    pub connection_state: ReadSignal<ConnectionState>,

    /// Subscribe to a specific message type. Returns an unsubscribe function.
    ///
    /// Usage:
    /// ```ignore
    /// let unsub = ws_ctx.subscribe("chat_stream", move |msg| { /* handle */ });
    /// // Call unsub() to unsubscribe, or it auto-cleans on component drop.
    /// ```
    subscribe: SubscribeFn,

    /// Send a message through the WebSocket.
    ///
    /// Returns `true` if the message was sent, `false` if not connected.
    send: SendFn,
}

// Type aliases for the closure types stored in the context.
// Wrapped in `SendWrapper` because JS closures are `!Send` but Leptos
// `StoredValue` requires `Send + Sync`. On WASM, `SendWrapper` is a
// transparent no-op; on SSR the closures are simple no-ops that are
// never actually sent across threads.
type SubscribeBox = Box<dyn Fn(String, Box<dyn Fn(WebSocketMessage)>) -> Box<dyn FnOnce()>>;
type SubscribeFn = StoredValue<SendWrapper<SubscribeBox>>;
type SendBox = Box<dyn Fn(serde_json::Value) -> bool>;
type SendFn = StoredValue<SendWrapper<SendBox>>;

impl WebSocketContext {
    /// Subscribe to messages of a given type. Returns an unsubscribe closure.
    pub fn subscribe(
        &self,
        message_type: &str,
        callback: impl Fn(WebSocketMessage) + 'static,
    ) -> Box<dyn FnOnce()> {
        let sub_fn = self.subscribe;
        sub_fn.with_value(|f| (**f)(message_type.to_string(), Box::new(callback)))
    }

    /// Send a JSON message through the WebSocket.
    /// Returns `true` if sent successfully, `false` if not connected.
    pub fn send(&self, message: serde_json::Value) -> bool {
        let send_fn = self.send;
        send_fn.with_value(|f| (**f)(message))
    }
}

// ===========================================================================
// WASM implementation (browser)
// ===========================================================================
#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use send_wrapper::SendWrapper;
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};
    use std::rc::Rc;
    use wasm_bindgen::prelude::*;
    use web_sys::{CloseEvent, MessageEvent, WebSocket};

    use crate::server_fns::chat::get_websocket_config;

    /// Maximum reconnect attempts before giving up — matches React.
    const MAX_RECONNECT_ATTEMPTS: u32 = 10;

    /// Base reconnect delay in milliseconds — matches React's `baseReconnectDelay`.
    const BASE_RECONNECT_DELAY_MS: u32 = 1000;

    /// Shared mutable state for the WebSocket connection.
    ///
    /// Wrapped in `Rc<RefCell<>>` because WASM is single-threaded and we need
    /// interior mutability from multiple closures (event handlers, subscribe, send).
    struct WsState {
        /// The live WebSocket connection, if any.
        ws: Option<WebSocket>,
        /// Event handler closures — stored to prevent GC.
        _closures: Vec<SendWrapper<JsValue>>,
        /// Subscribers: message_type -> list of callbacks.
        subscribers: HashMap<String, Vec<(usize, Box<dyn Fn(WebSocketMessage)>)>>,
        /// Monotonic subscriber ID counter for unsubscribe.
        next_sub_id: usize,
        /// Set of seen message keys for deduplication.
        seen_messages: HashSet<String>,
        /// Whether close was intentional (disconnect called).
        intentional_close: bool,
        /// Current reconnect attempt count.
        reconnect_attempts: u32,
        /// Handle to the pending reconnect timeout (if any).
        reconnect_timeout: Option<SendWrapper<gloo_timers::callback::Timeout>>,
    }

    impl WsState {
        fn new() -> Self {
            Self {
                ws: None,
                _closures: Vec::new(),
                subscribers: HashMap::new(),
                next_sub_id: 0,
                seen_messages: HashSet::new(),
                intentional_close: false,
                reconnect_attempts: 0,
                reconnect_timeout: None,
            }
        }
    }

    /// Provide the WebSocket context to all children. Connects when
    /// `user_id` and `workspace_id` are available (authenticated).
    pub fn provide_websocket_context(
        user_id: Signal<Option<String>>,
        workspace_id: Signal<Option<String>>,
    ) -> WebSocketContext {
        let (connection_state, set_connection_state) = signal(ConnectionState::Disconnected);
        let state = Rc::new(RefCell::new(WsState::new()));

        // -- subscribe function -----------------------------------------------
        let subscribe_state = state.clone();
        let subscribe_fn: Box<dyn Fn(String, Box<dyn Fn(WebSocketMessage)>) -> Box<dyn FnOnce()>> =
            Box::new(move |message_type: String, callback: Box<dyn Fn(WebSocketMessage)>| {
                let sub_id = {
                    let mut s = subscribe_state.borrow_mut();
                    let id = s.next_sub_id;
                    s.next_sub_id += 1;
                    s.subscribers
                        .entry(message_type.clone())
                        .or_default()
                        .push((id, callback));
                    id
                };

                // Return unsubscribe closure
                let unsub_state = subscribe_state.clone();
                let msg_type = message_type;
                Box::new(move || {
                    let mut s = unsub_state.borrow_mut();
                    if let Some(callbacks) = s.subscribers.get_mut(&msg_type) {
                        callbacks.retain(|(id, _)| *id != sub_id);
                        if callbacks.is_empty() {
                            s.subscribers.remove(&msg_type);
                        }
                    }
                })
            });

        // -- send function ----------------------------------------------------
        let send_state = state.clone();
        let send_fn: Box<dyn Fn(serde_json::Value) -> bool> = Box::new(move |message| {
            let s = send_state.borrow();
            if let Some(ref ws) = s.ws {
                if ws.ready_state() == WebSocket::OPEN {
                    if let Ok(json) = serde_json::to_string(&message) {
                        return ws.send_with_str(&json).is_ok();
                    }
                }
            }
            false
        });

        // -- connect/disconnect effect ----------------------------------------
        let connect_state = state.clone();
        Effect::new(move |_| {
            let uid = user_id.get();
            let wid = workspace_id.get();

            match (uid, wid) {
                (Some(uid), Some(wid)) if !uid.is_empty() && !wid.is_empty() => {
                    // Reset intentional close flag before connecting
                    connect_state.borrow_mut().intentional_close = false;

                    let cs = connect_state.clone();
                    let set_state = set_connection_state;
                    leptos::task::spawn_local(async move {
                        connect(cs, set_state).await;
                    });
                }
                _ => {
                    // Not authenticated — disconnect
                    disconnect(connect_state.clone(), set_connection_state);
                }
            }
        });

        // -- cleanup on owner drop -------------------------------------------
        let cleanup_state = state.clone();
        on_cleanup(move || {
            disconnect(cleanup_state, set_connection_state);
        });

        WebSocketContext {
            connection_state,
            subscribe: StoredValue::new(SendWrapper::new(subscribe_fn)),
            send: StoredValue::new(SendWrapper::new(send_fn)),
        }
    }

    /// Establish a WebSocket connection.
    ///
    /// Fetches a WS token via server function, builds the URL, and wires up
    /// event handlers matching the React implementation exactly.
    async fn connect(
        state: Rc<RefCell<WsState>>,
        set_connection_state: WriteSignal<ConnectionState>,
    ) {
        // Don't connect if already open. Clean up stale connections
        // in non-OPEN states (CONNECTING, CLOSING) — matches React's pattern
        // of nulling onclose and closing stale connections before reconnecting.
        {
            let mut s = state.borrow_mut();
            if let Some(ref ws) = s.ws {
                if ws.ready_state() == WebSocket::OPEN {
                    return;
                }
                // Clean up stale non-OPEN connection — null event handlers
                // to prevent spurious reconnect triggers, then close.
                ws.set_onclose(None);
                ws.set_onerror(None);
                ws.set_onmessage(None);
                ws.set_onopen(None);
                let _ = ws.close();
            }
            s.ws = None;
            s._closures.clear();
        }

        set_connection_state.set(ConnectionState::Connecting);

        // Fetch WebSocket config (token + user_id + workspace_id) via server function
        let config = match get_websocket_config().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to get WebSocket config: {e}");
                set_connection_state.set(ConnectionState::Disconnected);
                schedule_reconnect(state, set_connection_state);
                return;
            }
        };

        // Check if intentional close was requested while awaiting config
        // (e.g., user logged out mid-connection-attempt)
        if state.borrow().intentional_close {
            return;
        }

        // Build WebSocket URL — matches React: `${protocol}//${host}/ws/${workspaceId}_${userId}?token={wsToken}`
        // JWT uses Base64url alphabet which is safe for URL query parameters — no encoding needed.
        let url = match build_ws_url(&config.workspace_id, &config.user_id, &config.token) {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("Failed to build WebSocket URL: {e}");
                set_connection_state.set(ConnectionState::Disconnected);
                schedule_reconnect(state, set_connection_state);
                return;
            }
        };

        let ws = match WebSocket::new(&url) {
            Ok(ws) => ws,
            Err(e) => {
                tracing::error!("WebSocket::new failed: {:?}", e);
                set_connection_state.set(ConnectionState::Disconnected);
                schedule_reconnect(state, set_connection_state);
                return;
            }
        };

        let mut closures: Vec<SendWrapper<JsValue>> = Vec::new();

        // -- onopen -----------------------------------------------------------
        let onopen_state = state.clone();
        let on_open = Closure::<dyn FnMut(JsValue)>::new(move |_event: JsValue| {
            tracing::info!("WebSocket connected");
            set_connection_state.set(ConnectionState::Connected);
            // Server sends WebSocket-protocol-level pings every 45s (see
            // websocket.rs lines 146-166). No application-level ping needed
            // from the client — matching React, which has no ping either.
            onopen_state.borrow_mut().reconnect_attempts = 0;
        });
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        closures.push(SendWrapper::new(on_open.into_js_value()));

        // -- onmessage --------------------------------------------------------
        let onmsg_state = state.clone();
        let on_message =
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let Some(text) = event.data().as_string() else {
                    return;
                };

                let msg: WebSocketMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(_) => return,
                };

                let mut s = onmsg_state.borrow_mut();

                // Deduplication — matches React: `${message.type}_${message.session_id}_${message.message_id}_${JSON.stringify(message.data)}`
                let dedup_key = format!(
                    "{}_{}_{}_{}",
                    msg.message_type,
                    msg.session_id.as_deref().unwrap_or(""),
                    msg.message_id.as_deref().unwrap_or(""),
                    msg.data
                        .as_ref()
                        .map(|d| d.to_string())
                        .unwrap_or_else(|| "null".to_string())
                );

                if s.seen_messages.contains(&dedup_key) {
                    return; // Skip duplicate
                }
                // Cap dedup set to prevent unbounded growth in long sessions.
                // Duplicates only occur from concurrent reconnections, so a
                // modest cap is sufficient.
                if s.seen_messages.len() >= 1000 {
                    s.seen_messages.clear();
                }
                s.seen_messages.insert(dedup_key);

                // Notify subscribers for this message type
                if let Some(callbacks) = s.subscribers.get(&msg.message_type) {
                    // Clone the callback list to avoid borrow issues
                    let cb_refs: Vec<&Box<dyn Fn(WebSocketMessage)>> =
                        callbacks.iter().map(|(_, cb)| cb).collect();
                    for cb in cb_refs {
                        cb(msg.clone());
                    }
                }
            });
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        closures.push(SendWrapper::new(on_message.into_js_value()));

        // -- onerror ----------------------------------------------------------
        let on_error = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| {
            tracing::error!("WebSocket error: {:?}", e);
            set_connection_state.set(ConnectionState::Disconnected);
        });
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        closures.push(SendWrapper::new(on_error.into_js_value()));

        // -- onclose ----------------------------------------------------------
        let onclose_state = state.clone();
        let on_close = Closure::<dyn FnMut(CloseEvent)>::new(move |_event: CloseEvent| {
            tracing::info!("WebSocket closed");
            set_connection_state.set(ConnectionState::Disconnected);

            // Clear the ws reference
            onclose_state.borrow_mut().ws = None;

            // Only attempt reconnection if not intentionally closed — matches React
            let intentional = onclose_state.borrow().intentional_close;
            if !intentional {
                schedule_reconnect(onclose_state.clone(), set_connection_state);
            }
        });
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        closures.push(SendWrapper::new(on_close.into_js_value()));

        // Store the connection
        let mut s = state.borrow_mut();
        s.ws = Some(ws);
        s._closures = closures;
    }

    /// Intentionally disconnect the WebSocket.
    fn disconnect(
        state: Rc<RefCell<WsState>>,
        set_connection_state: WriteSignal<ConnectionState>,
    ) {
        let mut s = state.borrow_mut();
        s.intentional_close = true;

        // Cancel any pending reconnect timeout
        s.reconnect_timeout = None;

        // Close the WebSocket
        if let Some(ref ws) = s.ws {
            // Null out event handlers before closing to prevent the onclose
            // handler from firing and triggering a spurious reconnect —
            // matches React's critical pattern.
            ws.set_onclose(None);
            ws.set_onerror(None);
            ws.set_onmessage(None);
            ws.set_onopen(None);
            let _ = ws.close();
        }
        s.ws = None;
        s._closures.clear();

        set_connection_state.set(ConnectionState::Disconnected);
    }

    /// Schedule a reconnect with exponential backoff — matches React exactly.
    ///
    /// Delay formula: `min(1000 * 2^attempt, 30000)` — matches React's
    /// `Math.min(baseReconnectDelay * Math.pow(2, reconnectAttemptsRef.current), 30000)`.
    fn schedule_reconnect(
        state: Rc<RefCell<WsState>>,
        set_connection_state: WriteSignal<ConnectionState>,
    ) {
        let (intentional, attempts) = {
            let s = state.borrow();
            (s.intentional_close, s.reconnect_attempts)
        };

        if intentional || attempts >= MAX_RECONNECT_ATTEMPTS {
            if attempts >= MAX_RECONNECT_ATTEMPTS {
                tracing::error!(
                    "WebSocket reconnect: giving up after {} attempts",
                    MAX_RECONNECT_ATTEMPTS,
                );
            }
            return;
        }

        let delay_ms = std::cmp::min(
            BASE_RECONNECT_DELAY_MS.saturating_mul(2u32.saturating_pow(attempts)),
            30_000,
        );

        {
            let mut s = state.borrow_mut();
            s.reconnect_attempts = attempts + 1;
        }

        set_connection_state.set(ConnectionState::Reconnecting);

        tracing::info!(
            "WebSocket reconnect attempt {} in {}ms",
            attempts + 1,
            delay_ms,
        );

        let reconnect_state = state.clone();
        let timeout = gloo_timers::callback::Timeout::new(delay_ms, move || {
            leptos::task::spawn_local(async move {
                connect(reconnect_state, set_connection_state).await;
            });
        });

        state.borrow_mut().reconnect_timeout = Some(SendWrapper::new(timeout));
    }

    /// Build the WebSocket URL — matches React:
    /// `${protocol}//${host}/ws/${workspaceId}_${userId}?token=${wsToken}`
    fn build_ws_url(
        workspace_id: &str,
        user_id: &str,
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
}

// ===========================================================================
// SSR stub (server-side rendering — no WebSocket available)
// ===========================================================================
#[cfg(not(target_arch = "wasm32"))]
mod ssr {
    use super::*;

    /// SSR stub: provides a no-op WebSocket context.
    ///
    /// On the server, WebSocket is not available. The context provides
    /// a permanently-disconnected state and no-op subscribe/send functions.
    pub fn provide_websocket_context(
        _user_id: Signal<Option<String>>,
        _workspace_id: Signal<Option<String>>,
    ) -> WebSocketContext {
        let (connection_state, _) = signal(ConnectionState::Disconnected);

        let subscribe_fn: Box<
            dyn Fn(String, Box<dyn Fn(WebSocketMessage)>) -> Box<dyn FnOnce()>,
        > = Box::new(|_msg_type, _callback| Box::new(|| {}));

        let send_fn: Box<dyn Fn(serde_json::Value) -> bool> = Box::new(|_| false);

        WebSocketContext {
            connection_state,
            subscribe: StoredValue::new(SendWrapper::new(subscribe_fn)),
            send: StoredValue::new(SendWrapper::new(send_fn)),
        }
    }
}

// ===========================================================================
// Public API
// ===========================================================================

/// WebSocket provider component.
///
/// Wraps children with a `WebSocketContext` available via `use_context()`.
/// Connects when user is authenticated (user_id + workspace_id available),
/// disconnects on logout or component teardown.
///
/// Usage in Layout:
/// ```ignore
/// <WebSocketProvider user_id=user_id_signal workspace_id=workspace_id_signal>
///     {children()}
/// </WebSocketProvider>
/// ```
#[component]
pub fn WebSocketProvider(
    /// Reactive signal with the authenticated user's ID, or `None` if not logged in.
    user_id: Signal<Option<String>>,
    /// Reactive signal with the current workspace ID, or `None` if not logged in.
    workspace_id: Signal<Option<String>>,
    children: Children,
) -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    let ctx = wasm::provide_websocket_context(user_id, workspace_id);
    #[cfg(not(target_arch = "wasm32"))]
    let ctx = ssr::provide_websocket_context(user_id, workspace_id);

    provide_context(ctx);

    children()
}
