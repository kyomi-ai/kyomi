// SPDX-License-Identifier: AGPL-3.0-or-later

//! Unified chat engine — reactive state container for all chat UIs.
//!
//! `ChatEngine` is NOT a Leptos component. It's a struct with reactive signals
//! and methods that consumers create in their component bodies. It owns all chat
//! state and WebSocket logic so that both `CopilotChat` (copilot sidebars) and
//! `chat_page.rs` (main chat page) can use the same engine with different configs.
//!
//! ## Session modes
//!
//! - **Ephemeral**: Creates a session on activate, deletes on deactivate. For
//!   copilot sidebars that spin up temporary sessions tied to a context.
//! - **External**: Session ID managed by the caller (URL-driven). For the main
//!   chat page where the session lifecycle is controlled externally.
//!
//! ## Filtering
//!
//! - If `context_type` is Some: filters WS events by context_type AND session_id
//!   (copilot pattern).
//! - If `context_type` is None: filters by session_id only (main chat pattern).
//! - **Default-deny (KYO-494)**: the session_id check never admits an event
//!   when either side is missing an identity — a `None` engine session_id
//!   (a brand-new, not-yet-created chat) is not "no filter," it's "nothing
//!   matches yet." See `should_handle`'s doc comment below.

use leptos::prelude::*;

#[cfg(target_arch = "wasm32")]
use super::websocket_client::WebSocketContext;
use super::{ChatStateMachine, ThinkingManager};
#[cfg(target_arch = "wasm32")]
use super::ChatState;
use crate::server_fns::chat::ChatMessageItem;
use crate::server_fns::copilot::{
    create_copilot_session, delete_copilot_session, send_copilot_message,
};

// ─── Session mode ──────────────────────────────────────────────────────────

/// Controls how the engine manages sessions.
pub enum SessionMode {
    /// Ephemeral: create on activate, delete on deactivate. For copilot sidebars.
    Ephemeral {
        /// The context type for session creation (e.g. "dashboard_copilot").
        context_type: String,
        /// When true, the session is created. When false, it is deleted.
        /// If `None`, the session is created immediately on mount.
        active: Option<Signal<bool>>,
    },
    /// External: session_id managed by the caller (URL-driven). For main chat page.
    External {
        /// Caller-managed session ID signal.
        session_id: Signal<Option<String>>,
    },
}

// ─── Config ────────────────────────────────────────────────────────────────

/// Configuration for creating a `ChatEngine`.
pub struct ChatEngineConfig {
    /// How the engine manages session lifecycle.
    pub session_mode: SessionMode,
    /// For WS event filtering. Copilot sets this to e.g. "dashboard_copilot".
    /// Main chat leaves it None (filters by session_id only).
    pub context_type: Option<String>,
    /// Custom WS event names to subscribe to (e.g. ["dashboard_update"]).
    pub custom_ws_events: Vec<String>,
    /// Handler for custom WS events. Receives (event_name, data).
    pub on_custom_ws_event: Option<Callback<(String, serde_json::Value)>>,
    /// Content context signal (dashboard markdown, chart YAML, etc.).
    pub context_content: Option<Signal<String>>,
    /// Label for context prefix ("Dashboard Content", "Chart Content", etc.).
    pub context_label: Option<String>,
}

// ─── SendRequest ───────────────────────────────────────────────────────────

/// Data prepared by the engine for a send operation.
///
/// In Ephemeral mode, the engine calls the copilot server function directly.
/// In External mode, consumers use `add_user_message()` to get the optimistic
/// message and handle the server call themselves.
pub struct SendRequest {
    /// The session ID to send to.
    pub session_id: String,
    /// The user's message text.
    pub message: String,
    /// The context type (for copilot sends).
    pub context_type: Option<String>,
    /// The context content with prefix label (for copilot sends).
    pub context: Option<String>,
}

// ─── ChatEngine ────────────────────────────────────────────────────────────

/// Unified reactive state container for chat UIs.
///
/// Owns all chat state (messages, thinking, session, chat state machine) and
/// WebSocket subscriptions. Consumers create this in their component body and
/// use the public API to drive the UI.
#[derive(Clone)]
pub struct ChatEngine {
    // Public read signals
    messages: RwSignal<Vec<ChatMessageItem>>,
    chat_state: ChatStateMachine,
    thinking: ThinkingManager,
    session_id: RwSignal<Option<String>>,

    // Internal state
    has_sent_first: RwSignal<bool>,
    user_msg_counter: RwSignal<u32>,
    context_type: StoredValue<Option<String>>,
    context_content: Option<Signal<String>>,
    context_label: StoredValue<Option<String>>,
}

impl ChatEngine {
    /// Create a new engine. Sets up session lifecycle and WS subscriptions.
    ///
    /// Must be called inside a Leptos reactive owner (component body or
    /// `Owner::with`). The engine registers effects and cleanup handlers
    /// that are tied to the component lifecycle.
    pub fn new(config: ChatEngineConfig) -> Self {
        let messages = RwSignal::new(Vec::<ChatMessageItem>::new());
        let chat_state = ChatStateMachine::new();
        let thinking = ThinkingManager::new();
        let session_id = RwSignal::new(None::<String>);
        let has_sent_first = RwSignal::new(false);
        let user_msg_counter = RwSignal::new(0u32);
        let context_type = StoredValue::new(config.context_type);
        let context_content = config.context_content;
        let context_label = StoredValue::new(config.context_label);

        // ── Session lifecycle ──────────────────────────────────────────
        match &config.session_mode {
            SessionMode::Ephemeral {
                context_type: ctx_type,
                active,
            } => {
                let ctx_type_stored = StoredValue::new(ctx_type.clone());
                let active_signal = *active;
                let chat_state_for_session = chat_state.clone();
                let thinking_for_session = thinking.clone();
                // Guard against concurrent session creation (Effect can fire
                // multiple times before the async create completes).
                let is_creating = RwSignal::new(false);

                Effect::new(move || {
                    let should_be_active =
                        active_signal.is_none_or(|s| s.get());

                    if should_be_active
                        && session_id.get_untracked().is_none()
                        && !is_creating.get_untracked()
                    {
                        // Reset all state for fresh session.
                        messages.set(Vec::new());
                        chat_state_for_session.reset();
                        thinking_for_session.clear_all();
                        has_sent_first.set(false);
                        is_creating.set(true);

                        let Some(ctx_type) = ctx_type_stored.try_get_value() else { return };
                        let chat_state_err = chat_state_for_session.clone();
                        leptos::task::spawn_local(async move {
                            match create_copilot_session(ctx_type).await {
                                Ok(sid) => { session_id.try_set(Some(sid)); }
                                Err(e) => {
                                    // Guard: the component may have been disposed
                                    // while the async create was in flight.
                                    if chat_state_err.state().try_get_untracked().is_some() {
                                        chat_state_err.set_error(&format!(
                                            "Failed to start copilot: {e}"
                                        ));
                                    }
                                }
                            }
                            is_creating.try_set(false);
                        });
                    } else if !should_be_active
                        && let Some(sid) = session_id.get_untracked()
                    {
                        session_id.set(None);
                        leptos::task::spawn_local(async move {
                            let _ = delete_copilot_session(sid).await;
                        });
                    }
                });

                // Cleanup session on component unmount.
                on_cleanup(move || {
                    if let Some(sid) = session_id.get_untracked() {
                        leptos::task::spawn_local(async move {
                            let _ = delete_copilot_session(sid).await;
                        });
                    }
                });
            }
            SessionMode::External {
                session_id: external_sid,
            } => {
                // Sync external session_id signal into our internal session_id.
                let external_sid = *external_sid;
                Effect::new(move || {
                    let sid = external_sid.get();
                    session_id.set(sid);
                });
            }
        }

        // ── WebSocket subscriptions ────────────────────────────────────
        #[cfg(target_arch = "wasm32")]
        {
            let ws_ctx = use_context::<WebSocketContext>();
            let chat_state_ws = chat_state.clone();
            let thinking_ws = thinking.clone();
            let custom_events = config.custom_ws_events;
            let on_custom_event = config.on_custom_ws_event;

            Effect::new(move |_| {
                let Some(ws) = ws_ctx.as_ref().cloned() else {
                    return;
                };

                setup_ws_subscriptions(
                    &ws,
                    EngineSignals {
                        session_id,
                        messages,
                        chat_state: &chat_state_ws,
                        thinking: &thinking_ws,
                        context_type,
                    },
                    &custom_events,
                    on_custom_event,
                );
            });
        }

        // Suppress unused variable warnings on SSR.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (&config.custom_ws_events, &config.on_custom_ws_event);
        }

        Self {
            messages,
            chat_state,
            thinking,
            session_id,
            has_sent_first,
            user_msg_counter,
            context_type,
            context_content,
            context_label,
        }
    }

    // ── Read signals ───────────────────────────────────────────────────

    /// Read signal for messages.
    pub fn messages(&self) -> ReadSignal<Vec<ChatMessageItem>> {
        self.messages.read_only()
    }

    /// Access the thinking manager.
    pub fn thinking(&self) -> &ThinkingManager {
        &self.thinking
    }

    /// Access the chat state machine.
    pub fn chat_state(&self) -> &ChatStateMachine {
        &self.chat_state
    }

    /// Read signal for session ID.
    pub fn session_id(&self) -> ReadSignal<Option<String>> {
        self.session_id.read_only()
    }

    // ── Send / add message ─────────────────────────────────────────────

    /// Full send for Ephemeral mode (copilot). Creates optimistic user message,
    /// builds context prefix, transitions state, and calls the copilot server function.
    ///
    /// For External mode callers who handle their own server call, use
    /// `add_user_message()` instead and manage the server call yourself.
    pub fn send(&self, message: String) {
        let sid = match self.session_id.get_untracked() {
            Some(sid) => sid,
            None => return,
        };

        // Add optimistic user message.
        let _user_msg_id = self.add_user_message(&message);

        // Build context prefix.
        let context_prefix = self.build_context_prefix();

        let ctx_type = self.context_type.try_get_value().flatten();
        self.chat_state.start_sending(&sid);

        let chat_state_err = self.chat_state.clone();

        // Compute timezone and time context before entering the async closure.
        let timezone = Some(crate::utils::time::get_user_timezone());
        let time_context = crate::utils::time::get_time_context();
        let time_ctx = if time_context.is_empty() {
            None
        } else {
            Some(time_context)
        };

        // For ephemeral mode, send via copilot server function.
        // The context_type for the server call comes from config.context_type,
        // which matches the session creation context_type.
        let ctx_type_for_send = ctx_type.unwrap_or_default();
        leptos::task::spawn_local(async move {
            if let Err(e) = send_copilot_message(
                sid,
                message,
                ctx_type_for_send,
                context_prefix,
                timezone,
                time_ctx,
            )
            .await
            {
                // Guard: the component may have been disposed while the async
                // call was in flight (user navigated away mid-send).
                if chat_state_err.state().try_get_untracked().is_some() {
                    chat_state_err.set_error(&format!("Failed to send: {e}"));
                }
            }
        });
    }

    /// Add a user message optimistically. Returns the generated message_id.
    ///
    /// Used by External mode callers who handle their own server call.
    /// Also used internally by `send()` for Ephemeral mode.
    pub fn add_user_message(&self, content: &str) -> String {
        let counter = self.user_msg_counter.get_untracked();
        self.user_msg_counter.set(counter + 1);
        let user_msg_id = format!("user_{counter}");

        self.messages.update(|msgs| {
            msgs.push(ChatMessageItem {
                message_id: user_msg_id.clone(),
                message_type: "user".to_string(),
                content: content.to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                pinned: false,
                sent_by: None,
                thinking_events: Vec::new(),
                token_usage: None,
            });
        });

        user_msg_id
    }

    /// Build the context prefix for the current message.
    ///
    /// First message: `[{label}]\n{content}`
    /// Subsequent: `[{label} has been updated]\n{content}`
    /// Returns `None` if no context content or it's empty.
    pub fn build_context_prefix(&self) -> Option<String> {
        let content_signal = self.context_content?;
        let content = content_signal.get_untracked();
        if content.is_empty() {
            return None;
        }

        let label = self.context_label.try_get_value().flatten().unwrap_or_default();
        let is_first = !self.has_sent_first.get_untracked();
        self.has_sent_first.set(true);

        if is_first {
            Some(format!("[{label}]\n{content}"))
        } else {
            Some(format!("[{label} has been updated]\n{content}"))
        }
    }

    /// Request cancellation. Sends cancel_request via WebSocket if state allows.
    pub fn cancel(&self) {
        if !self.chat_state.request_cancel() {
            return;
        }

        self.send_cancel_ws();
    }

    #[cfg(target_arch = "wasm32")]
    fn send_cancel_ws(&self) {
        let ws_ctx = use_context::<WebSocketContext>();

        let ws_connected = ws_ctx.as_ref().is_some_and(|ctx| {
            ctx.connection_state.get_untracked()
                == super::websocket_client::ConnectionState::Connected
        });

        if !ws_connected {
            return;
        }

        let session_id = self
            .session_id
            .get_untracked()
            .unwrap_or_default();

        let message_id = self
            .chat_state
            .active_message_id()
            .get_untracked();

        let mut payload = serde_json::json!({
            "type": "cancel_request",
            "session_id": session_id,
        });

        // Include message_id when available (Streaming state); during Sending
        // it is not yet set. The frontend subscriber uses it to confirm the
        // right message was cancelled.
        if let Some(mid) = message_id {
            payload["message_id"] = serde_json::Value::String(mid);
        }

        if let Some(ws) = ws_ctx.as_ref() {
            ws.send(payload);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn send_cancel_ws(&self) {}

    /// Set up smart scroll on a container element.
    ///
    /// Scrolls to bottom only when within 100px of bottom, with 50ms debounce
    /// and smooth scroll. Ported from chat_page.rs lines 520-568.
    pub fn setup_scroll(&self, container_ref: NodeRef<leptos::html::Div>) {
        let messages = self.messages;

        #[cfg(target_arch = "wasm32")]
        {
            Effect::new(move |_| {
                // Track messages to trigger on change.
                let _ = messages.try_get();

                let container_guard = container_ref.try_read_untracked();
                let Some(container_guard) = container_guard else {
                    return;
                };
                let Some(container) = container_guard.as_ref() else {
                    return;
                };

                let scroll_top = container.scroll_top();
                let scroll_height = container.scroll_height();
                let client_height = container.client_height();
                let distance_from_bottom = scroll_height - scroll_top - client_height;

                // Only auto-scroll if within 100px of bottom.
                // Uses smooth scroll with 50ms debounce, matching chat_page.rs.
                // Fire-and-forget is acceptable — the timeout fires once after 50ms.
                if distance_from_bottom < 100 {
                    let container = container.clone();
                    let timeout =
                        gloo_timers::callback::Timeout::new(50, move || {
                            let opts = web_sys::ScrollIntoViewOptions::new();
                            opts.set_behavior(web_sys::ScrollBehavior::Smooth);
                            // Scroll the last child element into view smoothly.
                            if let Some(last_child) = container.last_element_child() {
                                last_child
                                    .scroll_into_view_with_scroll_into_view_options(&opts);
                            }
                        });
                    std::mem::forget(send_wrapper::SendWrapper::new(timeout));
                }
            });
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (container_ref, messages);
        }
    }

    /// Reset all state (for session switches in External mode).
    pub fn reset(&self) {
        self.messages.set(Vec::new());
        self.chat_state.reset();
        self.thinking.clear_all();
        self.has_sent_first.set(false);
        self.user_msg_counter.set(0);
    }

    /// Set messages directly (for loading history in External mode).
    pub fn set_messages(&self, msgs: Vec<ChatMessageItem>) {
        self.messages.set(msgs);
    }

    /// Set messages from a deferred context (async block inside `spawn_local`,
    /// WebSocket callback, timer, etc.) where the component may have been
    /// disposed before this fires. Silently no-ops if the signal is disposed.
    pub fn try_set_messages(&self, msgs: Vec<ChatMessageItem>) {
        let _ = self.messages.try_set(msgs);
    }
}

// ─── WS event filtering (KYO-494) ──────────────────────────────────────────
//
// Pulled out of the WS-subscription closures as plain functions — no
// signals, no reactivity — so the default-deny invariant below is directly
// unit-testable without a WASM/reactive-owner harness, and so every
// subscription handler (there were four independent copies of this logic
// before KYO-494: `should_handle` itself, plus ad hoc duplicates in the
// token_usage_update, request_cancelled, and custom-event handlers) goes
// through one implementation instead of four that could silently drift.

/// Whether an event's `context_type` matches the engine's own filter.
///
/// `filter = None` means "no context filtering" — the main chat page's
/// mode, where every event that reaches the WS layer already belongs to
/// this user's connection. Copilot sidebars set `filter = Some(ctx)` and
/// require an exact match.
///
/// `cfg(any(test, wasm32))`: the only production caller is
/// `should_handle_event` inside the wasm32-only `setup_ws_subscriptions`
/// below. Compiled unconditionally would make this "unused" on a plain
/// non-wasm32, non-test host build; gating it here keeps that build clean
/// while still compiling for `cargo test` (host) and the real wasm32
/// target.
#[cfg(any(test, target_arch = "wasm32"))]
fn context_type_matches(filter: Option<&str>, event_context_type: Option<&str>) -> bool {
    match filter {
        Some(expected) => event_context_type == Some(expected),
        None => true,
    }
}

/// Whether a WS event's `session_id` matches the engine's current session.
///
/// **Default-deny**: `should_handle` must never return `true` when either
/// side is missing an identity. Before KYO-494, a `None` engine session
/// (the state of a brand-new, not-yet-persisted chat) was treated as "no
/// filter" and admitted events for *every* session — on the `/chat` route a
/// new chat sits in exactly that state, so another in-flight conversation's
/// `chat_stream`/`chat_complete`/`agent_thinking` frames rendered straight
/// into the empty new-chat window. The fix on the caller's side is for the
/// client to mint a session id before the first message ever leaves (see
/// `chat_page.rs`), so `current_session_id` should already be `Some` by the
/// time this engine's *own* new session starts producing events. This
/// function still gets called with `current_session_id = None` routinely
/// though — e.g. sitting on an empty `/chat` while another of the user's
/// sessions is still streaming — and must keep refusing to guess there too.
///
/// `cfg(any(test, wasm32))`: see `context_type_matches` above — same
/// reasoning, same set of production callers.
#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) fn should_handle(current_session_id: Option<&str>, msg_session_id: Option<&str>) -> bool {
    match (current_session_id, msg_session_id) {
        (Some(current), Some(msg)) => current == msg,
        _ => false,
    }
}

// ─── WebSocket subscription setup ──────────────────────────────────────────

/// Reactive signals owned by the engine — passed as a unit to `setup_ws_subscriptions`
/// to avoid exceeding the function argument limit.
#[cfg(target_arch = "wasm32")]
struct EngineSignals<'a> {
    session_id: RwSignal<Option<String>>,
    messages: RwSignal<Vec<ChatMessageItem>>,
    chat_state: &'a ChatStateMachine,
    thinking: &'a ThinkingManager,
    context_type: StoredValue<Option<String>>,
}

/// Set up all WebSocket subscriptions for the chat engine.
///
/// Extracted to a standalone function so it can be conditionally compiled
/// for `wasm32` only without making the entire `ChatEngine::new` conditional.
#[cfg(target_arch = "wasm32")]
fn setup_ws_subscriptions(
    ws: &WebSocketContext,
    signals: EngineSignals<'_>,
    custom_events: &[String],
    on_custom_event: Option<Callback<(String, serde_json::Value)>>,
) {
    let EngineSignals { session_id, messages, chat_state, thinking, context_type } = signals;
    use super::{ThinkingEvent, TokenUsage};

    // Disposed guard: shared flag set by on_cleanup. Every WS callback
    // checks this before touching any signal to prevent "already disposed"
    // panics during the race between cleanup and async WS delivery.
    let disposed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Helper: check if an event belongs to this engine instance. Reads the
    // engine's reactive state and delegates to the pure, unit-tested
    // `context_type_matches` / `should_handle` functions above — see their
    // doc comments for the KYO-494 default-deny invariant this must uphold.
    let should_handle_event = move |event_context_type: Option<&str>,
                                    msg_session_id: Option<&str>|
          -> bool {
        let ctx_type = context_type.try_get_value().flatten();
        if !context_type_matches(ctx_type.as_deref(), event_context_type) {
            return false;
        }

        let current_sid = session_id.try_get_untracked().flatten();
        should_handle(current_sid.as_deref(), msg_session_id)
    };

    // ── agent_thinking ─────────────────────────────────────────────
    let chat_state_thinking = chat_state.clone();
    let thinking_for_thinking = thinking.clone();
    let disposed_thinking = disposed.clone();
    let unsub_agent_thinking = ws.subscribe("agent_thinking", move |msg| {
        if disposed_thinking.load(std::sync::atomic::Ordering::Relaxed) { return; }
        let data = match &msg.data {
            Some(d) => d,
            None => return,
        };

        let thinking_event: ThinkingEvent = match data
            .get("event")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            Some(e) => e,
            None => return,
        };

        // For agent_thinking events, context_type is nested at
        // data.event.context_type (not data.context_type).
        let event_context_type = data
            .get("event")
            .and_then(|v| v.get("context_type"))
            .and_then(|v| v.as_str());

        if !should_handle_event(event_context_type, msg.session_id.as_deref()) {
            return;
        }

        let token_usage: Option<TokenUsage> = data
            .get("token_usage")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let msg_message_id = match &msg.message_id {
            Some(m) => m.clone(),
            None => return,
        };

        // Create assistant message placeholder if needed.
        // try_update: signal may be disposed if user navigated away.
        messages.try_update(|msgs| {
            if !msgs.iter().any(|m| m.message_id == msg_message_id) {
                msgs.push(ChatMessageItem {
                    message_id: msg_message_id.clone(),
                    message_type: "assistant".to_string(),
                    content: String::new(),
                    timestamp: msg.timestamp.clone(),
                    pinned: false,
                    sent_by: None,
                    thinking_events: Vec::new(),
                    token_usage: None,
                });
            }
        });

        // Transition to streaming state if still in Sending.
        if let Some(state) = chat_state_thinking.state().try_get_untracked()
            && state == ChatState::Sending {
                chat_state_thinking.start_streaming(&msg_message_id);
        }

        // Process thinking event via ThinkingManager.
        thinking_for_thinking.handle_thinking_event(
            &msg_message_id,
            thinking_event,
            token_usage,
        );
    });

    // ── chat_stream ────────────────────────────────────────────────
    let chat_state_stream = chat_state.clone();
    let disposed_stream = disposed.clone();
    let unsub_chat_stream = ws.subscribe("chat_stream", move |msg| {
        if disposed_stream.load(std::sync::atomic::Ordering::Relaxed) { return; }
        let event_context_type = msg
            .data
            .as_ref()
            .and_then(|d| d.get("context_type"))
            .and_then(|v| v.as_str());

        if !should_handle_event(event_context_type, msg.session_id.as_deref()) {
            return;
        }

        let content = msg
            .data
            .as_ref()
            .and_then(|d| d.get("content"))
            .and_then(|v| v.as_str());

        let content = match content {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => return,
        };

        let msg_message_id = match &msg.message_id {
            Some(m) => m.clone(),
            None => return,
        };

        // State recovery: if state is Idle, force start_streaming.
        if let Some(stream_state) = chat_state_stream.state().try_get_untracked()
            && stream_state == ChatState::Idle {
                chat_state_stream.start_streaming(&msg_message_id);
        }

        messages.try_update(|msgs| {
            if let Some(existing) = msgs
                .iter_mut()
                .find(|m| m.message_id == msg_message_id && m.message_type == "assistant")
            {
                existing.content.push_str(&content);
            } else {
                msgs.push(ChatMessageItem {
                    message_id: msg_message_id,
                    message_type: "assistant".to_string(),
                    content,
                    timestamp: msg.timestamp.clone(),
                    pinned: false,
                    sent_by: None,
                    thinking_events: Vec::new(),
                    token_usage: None,
                });
            }
        });
    });

    // ── chat_complete ──────────────────────────────────────────────
    let chat_state_complete = chat_state.clone();
    let thinking_for_complete = thinking.clone();
    let disposed_complete = disposed.clone();
    let unsub_chat_complete = ws.subscribe("chat_complete", move |msg| {
        if disposed_complete.load(std::sync::atomic::Ordering::Relaxed) { return; }
        let event_context_type = msg
            .data
            .as_ref()
            .and_then(|d| d.get("context_type"))
            .and_then(|v| v.as_str());

        if !should_handle_event(event_context_type, msg.session_id.as_deref()) {
            return;
        }

        let msg_message_id = match &msg.message_id {
            Some(m) => m.clone(),
            None => return,
        };

        let state = match chat_state_complete.state().try_get_untracked() {
            Some(s) => s,
            None => return,
        };

        // Cancellation guard: skip if we're in Cancelling or Cancelled state.
        if state == ChatState::Cancelling || state == ChatState::Cancelled {
            return;
        }

        let full_content = msg
            .data
            .as_ref()
            .and_then(|d| d.get("content"))
            .and_then(|v| v.as_str())
            .map(String::from);

        // Update message with full content.
        messages.try_update(|msgs| {
            for m in msgs.iter_mut() {
                if m.message_id == msg_message_id && m.message_type == "assistant"
                    && let Some(ref content) = full_content {
                        m.content = content.clone();
                    }
            }
        });

        // Complete thinking via ThinkingManager.
        thinking_for_complete.complete_thinking(&msg_message_id);

        // Only transition state machine if we're actually in Sending or Streaming.
        if state == ChatState::Sending || state == ChatState::Streaming {
            chat_state_complete.complete();
        }
    });

    // ── token_usage_update ─────────────────────────────────────────
    let thinking_for_token = thinking.clone();
    let disposed_token = disposed.clone();
    let unsub_token_usage = ws.subscribe("token_usage_update", move |msg| {
        if disposed_token.load(std::sync::atomic::Ordering::Relaxed) { return; }
        let data = match &msg.data {
            Some(d) => d,
            None => return,
        };

        let token_update: TokenUsage = match data
            .get("token_usage")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            Some(t) => t,
            None => return,
        };

        let msg_message_id = match &msg.message_id {
            Some(m) => m.clone(),
            None => return,
        };

        // Filter by session_id (no context_type filter for token updates).
        // Default-deny — see `should_handle`'s doc comment (KYO-494).
        let current_sid = session_id.try_get_untracked().flatten();
        if !should_handle(current_sid.as_deref(), msg.session_id.as_deref()) {
            return;
        }

        thinking_for_token.update_token_usage(&msg_message_id, token_update);
    });

    // ── error ──────────────────────────────────────────────────────
    let chat_state_error = chat_state.clone();
    let disposed_error = disposed.clone();
    let unsub_error = ws.subscribe("error", move |msg| {
        if disposed_error.load(std::sync::atomic::Ordering::Relaxed) { return; }
        let ctx_type = context_type.try_get_value().flatten();
        if let Some(ref expected_ctx) = ctx_type {
            let event_context_type = msg
                .data
                .as_ref()
                .and_then(|d| d.get("context_type"))
                .and_then(|v| v.as_str());

            if event_context_type != Some(expected_ctx.as_str()) {
                return;
            }
        }

        let error_msg = msg
            .data
            .as_ref()
            .and_then(|d| d.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("An error occurred")
            .to_string();

        chat_state_error.set_error(&error_msg);
    });

    // ── request_cancelled ──────────────────────────────────────────
    let chat_state_cancelled = chat_state.clone();
    let thinking_for_cancelled = thinking.clone();
    let disposed_cancelled = disposed.clone();
    let unsub_request_cancelled = ws.subscribe("request_cancelled", move |msg| {
        if disposed_cancelled.load(std::sync::atomic::Ordering::Relaxed) { return; }
        let msg_message_id = match &msg.message_id {
            Some(m) => m.clone(),
            None => return,
        };

        // Confirm cancellation if this event belongs to the active message OR,
        // when cancelling during Sending (no message_id set yet), if the event
        // session matches the active session. Default-deny (KYO-494): two
        // missing session ids are not a match, so this used `should_handle`
        // instead of `==` — `None == None` would otherwise have been `true`.
        let current_sid = session_id.try_get_untracked().flatten();
        let is_ours = chat_state_cancelled.is_active_message(&msg_message_id)
            || (chat_state_cancelled.active_message_id().try_get_untracked().flatten().is_none()
                && should_handle(current_sid.as_deref(), msg.session_id.as_deref()));

        if is_ours {
            chat_state_cancelled.confirm_cancelled();
        }

        // Update the assistant message to show it was cancelled.
        messages.try_update(|msgs| {
            for m in msgs.iter_mut() {
                if m.message_id == msg_message_id && m.message_type == "assistant" {
                    m.content = "_Request cancelled by user._".to_string();
                }
            }
        });

        // Cancel thinking via ThinkingManager.
        thinking_for_cancelled.cancel_thinking(&msg_message_id);
    });

    // ── Custom WS event subscriptions ──────────────────────────────
    let mut custom_unsubs: Vec<send_wrapper::SendWrapper<Box<dyn FnOnce()>>> = Vec::new();

    for event_name in custom_events {
        let event_name_clone = event_name.clone();
        let disposed_custom = disposed.clone();
        let unsub = ws.subscribe(event_name, move |msg| {
            if disposed_custom.load(std::sync::atomic::Ordering::Relaxed) { return; }
            // Apply the same filtering as other events.
            let ctx_type = context_type.try_get_value().flatten();
            let event_context_type = msg
                .data
                .as_ref()
                .and_then(|d| d.get("context_type"))
                .and_then(|v| v.as_str());
            if !context_type_matches(ctx_type.as_deref(), event_context_type) {
                return;
            }

            // Filter by session_id. Default-deny — see `should_handle`'s doc
            // comment (KYO-494).
            let current_sid = session_id.try_get_untracked().flatten();
            if !should_handle(current_sid.as_deref(), msg.session_id.as_deref()) {
                return;
            }

            if let Some(data) = msg.data
                && let Some(cb) = on_custom_event {
                    cb.run((event_name_clone.clone(), data));
                }
        });
        custom_unsubs.push(send_wrapper::SendWrapper::new(unsub));
    }

    // ── Cleanup: unsubscribe all on component unmount ──────────────
    let unsub_agent_thinking = send_wrapper::SendWrapper::new(unsub_agent_thinking);
    let unsub_chat_stream = send_wrapper::SendWrapper::new(unsub_chat_stream);
    let unsub_chat_complete = send_wrapper::SendWrapper::new(unsub_chat_complete);
    let unsub_token_usage = send_wrapper::SendWrapper::new(unsub_token_usage);
    let unsub_error = send_wrapper::SendWrapper::new(unsub_error);
    let unsub_request_cancelled = send_wrapper::SendWrapper::new(unsub_request_cancelled);

    on_cleanup(move || {
        disposed.store(true, std::sync::atomic::Ordering::Relaxed);
        unsub_agent_thinking.take()();
        unsub_chat_stream.take()();
        unsub_chat_complete.take()();
        unsub_token_usage.take()();
        unsub_error.take()();
        unsub_request_cancelled.take()();
        for unsub in custom_unsubs {
            unsub.take()();
        }
    });
}

#[cfg(test)]
mod tests {
    //! KYO-494: chat_stream/chat_complete/agent_thinking events leaked
    //! across sessions because `should_handle` treated "the engine has no
    //! session id yet" as "admit everything." These tests cover the
    //! default-deny invariant directly on the pure filtering functions —
    //! no WASM/reactive-owner harness needed, since `should_handle` and
    //! `context_type_matches` take plain values, not signals.

    use super::*;

    // ── should_handle: default-deny on missing identity ─────────────────

    #[test]
    fn should_handle_denies_when_engine_has_no_session_yet() {
        // The exact reproduction from the ticket: a brand-new chat (no
        // session id yet) must not admit another session's event.
        assert!(!should_handle(None, Some("other-session")));
    }

    #[test]
    fn should_handle_denies_when_event_carries_no_session_id() {
        assert!(!should_handle(Some("sess-1"), None));
    }

    #[test]
    fn should_handle_denies_when_both_sides_are_missing() {
        // Regression guard for the `request_cancelled` handler's old `==`
        // comparison, where `None == None` evaluated to `true`.
        assert!(!should_handle(None, None));
    }

    #[test]
    fn should_handle_denies_mismatched_sessions() {
        assert!(!should_handle(Some("sess-1"), Some("sess-2")));
    }

    #[test]
    fn should_handle_admits_matching_sessions() {
        assert!(should_handle(Some("sess-1"), Some("sess-1")));
    }

    // ── context_type_matches ─────────────────────────────────────────────

    #[test]
    fn context_type_matches_no_filter_admits_anything() {
        // Main chat mode: context_type is None, so this check never gates.
        assert!(context_type_matches(None, None));
        assert!(context_type_matches(None, Some("dashboard_copilot")));
    }

    #[test]
    fn context_type_matches_requires_exact_match_when_filtering() {
        assert!(context_type_matches(
            Some("dashboard_copilot"),
            Some("dashboard_copilot")
        ));
        assert!(!context_type_matches(Some("dashboard_copilot"), Some("chart_copilot")));
        assert!(!context_type_matches(Some("dashboard_copilot"), None));
    }

    // ── Copilot path: its own None-session window must stay closed ──────
    //
    // Ephemeral (copilot) engines set context_type = Some(..) and start
    // with session_id = None until `create_copilot_session` resolves. That
    // window must be denied exactly like the main chat's, not treated as
    // "context matched, so admit it anyway."

    #[test]
    fn copilot_pending_session_denies_events_even_with_matching_context_type() {
        let ctx_filter = Some("dashboard_copilot");
        let event_context_type = Some("dashboard_copilot");
        assert!(
            context_type_matches(ctx_filter, event_context_type),
            "context_type matching is a precondition, not the full story"
        );

        // The engine's own session_id is still None (session creation
        // in flight) — an event for some other copilot session must be
        // denied, not admitted just because the context_type matched.
        let engine_session_id: Option<&str> = None;
        assert!(!should_handle(engine_session_id, Some("some-other-copilot-session")));
    }

    #[test]
    fn copilot_admits_once_its_own_session_is_established() {
        let ctx_filter = Some("dashboard_copilot");
        let event_context_type = Some("dashboard_copilot");
        assert!(context_type_matches(ctx_filter, event_context_type));
        assert!(should_handle(Some("copilot-sess-1"), Some("copilot-sess-1")));
    }
}
