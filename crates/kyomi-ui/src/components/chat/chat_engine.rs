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

                        let ctx_type = ctx_type_stored.get_value();
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

        let ctx_type = self.context_type.get_value();
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
                timestamp: String::new(),
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

        let label = self.context_label.get_value().unwrap_or_default();
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

        let message_id = self
            .chat_state
            .active_message_id()
            .get_untracked()
            .unwrap_or_default();

        if let Some(ws) = ws_ctx.as_ref() {
            ws.send(serde_json::json!({
                "type": "cancel_request",
                "message_id": message_id,
            }));
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
                let _ = messages.get();

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

    // Helper: check if an event belongs to this engine instance.
    // When context_type is Some, filter by both context_type AND session_id.
    // When context_type is None, filter by session_id only (allowing null
    // session during new chat creation race condition).
    let should_handle = move |event_context_type: Option<&str>,
                              msg_session_id: Option<&str>|
          -> bool {
        let ctx_type = context_type.get_value();

        // If we have a context_type filter, the event must match it.
        if let Some(ref expected_ctx) = ctx_type
            && event_context_type != Some(expected_ctx.as_str()) {
                return false;
            }

        // Session ID check: if we have a current session, the event must match.
        // If we don't have a session yet (new chat race condition in External mode),
        // allow the event through.
        let current_sid = session_id.get_untracked();
        if let Some(sid) = &current_sid
            && let Some(msg_sid) = msg_session_id
                && msg_sid != sid.as_str() {
                    return false;
                }

        true
    };

    // ── agent_thinking ─────────────────────────────────────────────
    let chat_state_thinking = chat_state.clone();
    let thinking_for_thinking = thinking.clone();
    let unsub_agent_thinking = ws.subscribe("agent_thinking", move |msg| {
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

        if !should_handle(event_context_type, msg.session_id.as_deref()) {
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
        messages.update(|msgs| {
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
        let state = chat_state_thinking.state().get_untracked();
        if state == ChatState::Sending {
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
    let unsub_chat_stream = ws.subscribe("chat_stream", move |msg| {
        let event_context_type = msg
            .data
            .as_ref()
            .and_then(|d| d.get("context_type"))
            .and_then(|v| v.as_str());

        if !should_handle(event_context_type, msg.session_id.as_deref()) {
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
        // Handles URL transitions during new chat creation where reactive
        // effects may reset state to Idle before all WS events are processed.
        let stream_state = chat_state_stream.state().get_untracked();
        if stream_state == ChatState::Idle {
            chat_state_stream.start_streaming(&msg_message_id);
        }

        messages.update(|msgs| {
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
    let unsub_chat_complete = ws.subscribe("chat_complete", move |msg| {
        let event_context_type = msg
            .data
            .as_ref()
            .and_then(|d| d.get("context_type"))
            .and_then(|v| v.as_str());

        if !should_handle(event_context_type, msg.session_id.as_deref()) {
            return;
        }

        let msg_message_id = match &msg.message_id {
            Some(m) => m.clone(),
            None => return,
        };

        let state = chat_state_complete.state().get_untracked();

        // Cancellation guard: skip if we're in Cancelling or Cancelled state.
        // The error message from backend should not overwrite our cancellation.
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
        messages.update(|msgs| {
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
        // Prevents stale chat_complete events from resetting state during a
        // different interaction (matches chat_page.rs lines 972-977).
        if state == ChatState::Sending || state == ChatState::Streaming {
            chat_state_complete.complete();
        }
    });

    // ── token_usage_update ─────────────────────────────────────────
    let thinking_for_token = thinking.clone();
    let unsub_token_usage = ws.subscribe("token_usage_update", move |msg| {
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
        let current_sid = session_id.get_untracked();
        if let Some(sid) = &current_sid
            && msg.session_id.as_deref() != Some(sid.as_str()) {
                return;
            }

        thinking_for_token.update_token_usage(&msg_message_id, token_update);
    });

    // ── error ──────────────────────────────────────────────────────
    let chat_state_error = chat_state.clone();
    let unsub_error = ws.subscribe("error", move |msg| {
        // For error events, check context_type if we have one.
        let ctx_type = context_type.get_value();
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
    let unsub_request_cancelled = ws.subscribe("request_cancelled", move |msg| {
        let msg_message_id = match &msg.message_id {
            Some(m) => m.clone(),
            None => return,
        };

        // Only confirm if this is the active message (chat_page.rs pattern).
        if chat_state_cancelled.is_active_message(&msg_message_id) {
            chat_state_cancelled.confirm_cancelled();
        }

        // Update the assistant message to show it was cancelled.
        messages.update(|msgs| {
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

        let unsub = ws.subscribe(event_name, move |msg| {
            // Apply the same filtering as other events.
            let ctx_type = context_type.get_value();
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

            // Filter by session_id.
            let current_sid = session_id.get_untracked();
            if let Some(sid) = &current_sid
                && msg.session_id.as_deref() != Some(sid.as_str()) {
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
