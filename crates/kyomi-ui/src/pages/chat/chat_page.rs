// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat page — session loading, message display, sending, and streaming.
//!
//! Route: `/chat` (new chat) or `/chat/:session_id` (existing session).
//!
//! Ported from `apps/frontend/src/pages/Chat.jsx` (lines 206-1715 for state,
//! session loading, and message rendering) and
//! `apps/frontend/src/components/ChatInterface.jsx` (lines 200-480 for
//! WebSocket subscriptions, send logic, and cancellation).
//!
//! CSS classes are copied verbatim from the React source.
//!
//! ## Features implemented:
//! - URL parameter parsing for `session_id`
//! - Session loading via `get_session_messages()` server function
//! - New chat mode with random greeting
//! - Messages container with `ChatMessage` rendering
//! - Pinned-only filter
//! - Smart scroll (auto-scroll only when near bottom)
//! - Loading state with spinner
//! - Message sending via `send_chat_message()` server function (Phase 8)
//! - WebSocket streaming: chat_stream, chat_complete, agent_thinking,
//!   session_created, title_update, token_usage_update, request_cancelled,
//!   error events (Phase 8)
//! - Cancellation flow (Phase 8)
//! - ChatInput component integration (Phase 8)
//!
//! ## Features deferred to later phases:
//! - Session header with title editing (Phase 9)

use std::collections::HashMap;

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};

use super::chat_message::ChatMessage;
use crate::components::chat::websocket_client::{ConnectionState, WebSocketContext};
use crate::components::chat::ChatInput;
use crate::components::chat::{ChatStateMachine, ThinkingEvent, ThinkingManager, ThinkingState, TokenUsage};
use crate::components::Spinner;
use crate::server_fns::chat::{
    get_session_messages, send_chat_message, ChatMessageItem, SessionDetail,
};

// ─── Greetings ──────────────────────────────────────────────────────────────

/// Random greetings shown on the new-chat screen.
///
/// Matches React's `generateGreeting()` in Chat.jsx (lines 299-324).
/// The `{name}` placeholder is replaced with the user's first name at runtime.
const GREETINGS: &[&str] = &[
    "Ready to dive into the data, {name}?",
    "What patterns shall we uncover today, {name}?",
    "Which datasets are calling your attention, {name}?",
    "What story do you think the numbers will tell us, {name}?",
    "Let's turn your data into decisions, {name}!",
    "Ready to crunch some numbers, {name}?",
    "What metrics should we examine today, {name}?",
    "Time to unlock some data-driven insights, {name}!",
    "What hidden trends can we discover, {name}?",
    "Ready to explore the data landscape, {name}?",
    "What analytics adventure awaits us, {name}?",
    "Let's dig deeper into your data, {name}!",
    "What analysis can I help you with today, {name}?",
    "Ready to transform data into actionable insights, {name}?",
    "What questions should we ask the data, {name}?",
    "Let's make sense of your numbers, {name}!",
    "What's the data puzzle we're solving today, {name}?",
    "Ready to connect the dots in your data, {name}?",
    "What trends are you curious about, {name}?",
    "Let's see what the data reveals, {name}!",
];

/// Pick a random greeting and substitute the user's first name.
///
/// Uses a simple time-based seed on WASM, falls back to index 0 on SSR.
fn generate_greeting(user_name: &str) -> String {
    let first_name = user_name
        .split_whitespace()
        .next()
        .unwrap_or(user_name);

    let index = pick_random_index(GREETINGS.len());
    GREETINGS[index].replace("{name}", first_name)
}

/// Pick a pseudo-random index in range `[0, len)`.
///
/// On WASM we use `js_sys::Math::random()`. On SSR we use the current
/// timestamp subsecond component for a reasonable spread.
fn pick_random_index(len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let r = js_sys::Math::random();
        (r * len as f64).floor() as usize % len
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Use subsecond nanos from std::time as a simple seed.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        nanos as usize % len
    }
}

// ─── Time Context ───────────────────────────────────────────────────────────

/// Compute the current time with timezone offset for agent awareness.
///
/// Returns a string in `YYYY-MM-DDTHH:MM:SS±HH:MM` format.
/// Matches React's `getTimeContext()` in ChatInterface.jsx (lines 384-407).
///
/// On SSR, returns an empty string (time context is only relevant client-side).
fn get_time_context() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let now = js_sys::Date::new_0();

        // getTimezoneOffset() returns minutes *west* of UTC (negative = east).
        // React: `const offsetMinutes = -now.getTimezoneOffset();`
        let raw_offset = now.get_timezone_offset() as i32; // minutes west of UTC
        let offset_minutes = -raw_offset; // minutes east of UTC

        let offset_sign = if offset_minutes >= 0 { '+' } else { '-' };
        let abs_offset = offset_minutes.unsigned_abs();
        let offset_hours = abs_offset / 60;
        let offset_mins = abs_offset % 60;

        let year = now.get_full_year();
        let month = now.get_month() + 1; // JS months are 0-indexed
        let date = now.get_date();
        let hours = now.get_hours();
        let minutes = now.get_minutes();
        let seconds = now.get_seconds();

        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}{:02}:{:02}",
            year, month, date, hours, minutes, seconds, offset_sign, offset_hours, offset_mins
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        String::new()
    }
}

/// Generate a unique user message ID.
///
/// Matches React's `id: `user-${Date.now()}-${Math.random().toString(36).substring(2, 11)}`
/// in ChatInterface.jsx line 417.
fn generate_user_message_id() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let timestamp = js_sys::Date::now() as u64;
        let random_part = js_sys::Math::random();
        // Convert to base-36-like suffix using the fractional part
        let suffix = format!("{:.10}", random_part)
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(9)
            .collect::<String>();
        format!("user-{}-{}", timestamp, suffix)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // On SSR, use timestamp + nanos for uniqueness (message sending only happens on WASM,
        // but we need this to compile on SSR).
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("user-{}", nanos)
    }
}

// ─── Chat Page Component ────────────────────────────────────────────────────

/// Main chat page component.
///
/// Handles session loading from URL parameters, displays messages, sends
/// messages, streams AI responses via WebSocket, and supports cancellation.
#[component]
pub fn ChatPage() -> impl IntoView {
    // ── URL parameter parsing ───────────────────────────────────────────
    let params = use_params_map();
    let url_session_id = Memo::new(move |_| {
        let p = params.get();
        let id = p.get("session_id").unwrap_or_default();
        if id.is_empty() { None } else { Some(id) }
    });

    // ── State signals ───────────────────────────────────────────────────
    // Matches React's useState declarations (Chat.jsx lines 216-233)
    let (messages, set_messages) = signal(Vec::<ChatMessageItem>::new());
    let (current_session_id, set_current_session_id) = signal(Option::<String>::None);
    // session_title read signal will be used in Phase 9 (session header).
    let (_session_title, set_session_title) = signal(String::new());
    let (session_metadata, set_session_metadata) = signal(SessionDetail::default());
    let (is_loading, set_is_loading) = signal(false);
    // set_show_pinned_only will be used in Phase 9 (pinned filter toggle button).
    let (show_pinned_only, _set_show_pinned_only) = signal(false);
    let (current_greeting, set_current_greeting) = signal(String::new());

    // Thinking state per message — keyed by message_id.
    // Populated when loading stored thinking events from session messages,
    // and updated live by WebSocket agent_thinking events.
    let (thinking_map, set_thinking_map) =
        signal(HashMap::<String, ThinkingState>::new());

    // ── Chat state machine ──────────────────────────────────────────────
    // Replaces fragmented state (is_loading, is_processing, etc.) with a
    // single source of truth. Matches React's useChatState() hook.
    let chat_state = ChatStateMachine::new();

    // ── Thinking manager ────────────────────────────────────────────────
    // Processes and deduplicates thinking events, matching React's
    // useAgentThinking() hook. Only used on WASM (WebSocket subscriptions).
    let _thinking_manager = ThinkingManager::new();

    // Wire is_streaming and active_message_id from ChatStateMachine.
    let is_streaming = chat_state.is_streaming;
    let chat_state_active_message_id = chat_state.active_message_id();
    let active_message_id = Signal::derive(move || chat_state_active_message_id.get());

    // ── WebSocket context ───────────────────────────────────────────────
    let ws_ctx = use_context::<WebSocketContext>();

    // Connection state as a string signal for ChatInput.
    let ws_ctx_for_connection = ws_ctx.clone();
    let connection_state_signal = Signal::derive(move || {
        ws_ctx_for_connection
            .as_ref()
            .map(|ctx| ctx.connection_state.get().to_string())
            .unwrap_or_else(|| "disconnected".to_string())
    });

    // ── Refs for smart scroll ───────────────────────────────────────────
    let messages_end_ref = NodeRef::<leptos::html::Div>::new();
    let messages_container_ref = NodeRef::<leptos::html::Div>::new();

    // ── Navigation ──────────────────────────────────────────────────────
    let navigate = use_navigate();

    // ── Session loading effect ──────────────────────────────────────────
    // When url_session_id changes, load the session from the server.
    // Matches React's useEffect for urlSessionId (Chat.jsx lines 782-804).
    let chat_state_for_load = chat_state.clone();
    Effect::new(move |_| {
        let new_session_id = url_session_id.get();
        let current = current_session_id.get_untracked();

        match (&new_session_id, &current) {
            // URL has a session ID and it's different from current — load it
            (Some(sid), _) if Some(sid) != current.as_ref() => {
                let sid = sid.clone();
                set_messages.set(Vec::new());
                set_current_greeting.set(String::new());
                set_is_loading.set(true);
                set_current_session_id.set(Some(sid.clone()));
                // Reset chat state when switching sessions
                chat_state_for_load.reset();

                leptos::task::spawn_local(async move {
                    match get_session_messages(sid.clone()).await {
                        Ok(response) => {
                            // Set session metadata
                            if let Some(ref title) = response.session.title {
                                set_session_title.set(title.clone());
                            }
                            set_session_metadata.set(response.session);

                            // Build thinking state from stored events
                            let mut thinking = HashMap::new();
                            for msg in &response.messages {
                                let events: Vec<ThinkingEvent> =
                                    msg.thinking_events
                                        .iter()
                                        .filter_map(|v| serde_json::from_value(v.clone()).ok())
                                        .collect();

                                let token_usage: Option<TokenUsage> =
                                    msg.token_usage
                                        .as_ref()
                                        .and_then(|v| serde_json::from_value(v.clone()).ok());

                                if !events.is_empty() || token_usage.is_some() {
                                    thinking.insert(
                                        msg.message_id.clone(),
                                        ThinkingState {
                                            events,
                                            is_active: false,
                                            cancelled: false,
                                            token_usage,
                                        },
                                    );
                                }
                            }
                            set_thinking_map.set(thinking);

                            set_messages.set(response.messages);
                        }
                        Err(_e) => {
                            // Gracefully handle by setting empty messages
                            // (matches React's catch block in loadSessionMessages)
                            set_messages.set(Vec::new());
                        }
                    }
                    set_is_loading.set(false);
                });
            }
            // URL has no session ID but we have a current session — clear state (new chat)
            (None, Some(_)) => {
                set_current_session_id.set(None);
                set_messages.set(Vec::new());
                set_session_title.set(String::new());
                set_session_metadata.set(SessionDetail::default());
                set_thinking_map.set(HashMap::new());
                chat_state_for_load.reset();
                // TODO: Use actual user name from auth context when wired in Phase 13
                set_current_greeting.set(generate_greeting("there"));
            }
            _ => {}
        }
    });

    // ── Generate greeting on mount if no messages ───────────────────────
    // Matches React's useEffect for greeting (Chat.jsx lines 760-765)
    Effect::new(move |_| {
        let msgs = messages.get();
        let greeting = current_greeting.get();
        if msgs.is_empty() && greeting.is_empty() && url_session_id.get().is_none() {
            // TODO: Use actual user name from auth context when wired in Phase 13
            set_current_greeting.set(generate_greeting("there"));
        }
    });

    // ── Smart scroll ────────────────────────────────────────────────────
    // Auto-scroll to bottom when messages change, but only if user is
    // near the bottom (within 100px). Matches React's scrollToBottom
    // with isNearBottom check (Chat.jsx lines 272-296).
    #[cfg(target_arch = "wasm32")]
    {
        Effect::new(move |_| {
            // Track messages to trigger on change
            let _ = messages.get();

            let container = messages_container_ref.get();
            let end_el = messages_end_ref.get();

            if let (Some(container), Some(end_el)) = (container, end_el) {
                let scroll_top = container.scroll_top();
                let scroll_height = container.scroll_height();
                let client_height = container.client_height();
                let distance_from_bottom = scroll_height - scroll_top - client_height;

                // Only auto-scroll if within 100px of bottom
                if distance_from_bottom < 100 {
                    let opts = web_sys::ScrollIntoViewOptions::new();
                    opts.set_behavior(web_sys::ScrollBehavior::Smooth);
                    end_el.scroll_into_view_with_scroll_into_view_options(&opts);
                }
            }
        });
    }

    // ── WebSocket subscriptions (Phase 8) ───────────────────────────────
    // Subscribe to all relevant WebSocket events for streaming, thinking,
    // cancellation, session creation, and errors.
    //
    // Matches React's useEffect in Chat.jsx (lines 345-694) and
    // ChatInterface.jsx (lines 206-381).
    //
    // All subscriptions are set up in this block and cleaned up on unmount
    // via on_cleanup().
    #[cfg(target_arch = "wasm32")]
    {
        let chat_state_ws = chat_state.clone();
        let thinking_manager_ws = _thinking_manager.clone();
        let navigate_ws = navigate.clone();

        Effect::new(move |_| {
            let Some(ws) = ws_ctx.as_ref().cloned() else {
                return;
            };

            // We need to clone these for each closure since they each need ownership.
            let chat_state_session = chat_state_ws.clone();
            let navigate_session = navigate_ws.clone();

            // ── session_created ─────────────────────────────────────────
            // Matches React: Chat.jsx lines 348-373
            // When a new session is created, navigate to it and update metadata.
            let unsub_session_created = ws.subscribe("session_created", move |msg| {
                let current_sid = current_session_id.get_untracked();
                let state = chat_state_session.state().get_untracked();

                // Only process if we don't have a session ID yet AND we're in SENDING state
                if current_sid.is_none()
                    && msg.session_id.is_some()
                    && msg.data.is_some()
                    && state == crate::components::chat::ChatState::Sending
                {
                    let session_id = msg.session_id.as_ref().unwrap().clone();
                    let data = msg.data.as_ref().unwrap();

                    // Navigate to the new session URL (replace: true for new chat)
                    let path = format!("/chat/{}", session_id);
                    navigate_session(&path, Default::default());

                    // Set title if present (null title = will arrive later via title_update)
                    if let Some(title) = data.get("title").and_then(|v| v.as_str()) {
                        set_session_title.set(title.to_string());
                    }

                    // Set session metadata from backend data
                    let shared = data.get("shared").and_then(|v| v.as_bool()).unwrap_or(false);
                    let created_by = data
                        .get("created_by")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    let slack_channel_id = data
                        .get("slack_channel_id")
                        .and_then(|v| v.as_str())
                        .map(String::from);

                    set_session_metadata.set(SessionDetail {
                        title: data.get("title").and_then(|v| v.as_str()).map(String::from),
                        shared,
                        created_by,
                        slack_channel_id,
                    });
                }
            });

            // ── title_update ────────────────────────────────────────────
            // Matches React: Chat.jsx lines 376-383
            let unsub_title_update = ws.subscribe("title_update", move |msg| {
                if let (Some(sid), Some(data)) = (&msg.session_id, &msg.data) {
                    if let Some(title) = data.get("title").and_then(|v| v.as_str()) {
                        let current_sid = current_session_id.get_untracked();
                        if current_sid.as_deref() == Some(sid.as_str()) {
                            set_session_title.set(title.to_string());
                        }
                    }
                }
            });

            // ── agent_thinking ──────────────────────────────────────────
            // Matches React: Chat.jsx lines 386-498 and ChatInterface.jsx lines 216-271
            let chat_state_thinking = chat_state_ws.clone();
            let thinking_manager_thinking = thinking_manager_ws.clone();
            let unsub_agent_thinking = ws.subscribe("agent_thinking", move |msg| {
                let data = match &msg.data {
                    Some(d) => d,
                    None => return,
                };

                let thinking_event: ThinkingEvent = match data.get("event").and_then(|v| {
                    serde_json::from_value(v.clone()).ok()
                }) {
                    Some(e) => e,
                    None => return,
                };

                let token_usage: Option<TokenUsage> = data
                    .get("token_usage")
                    .and_then(|v| serde_json::from_value(v.clone()).ok());

                let msg_session_id = match &msg.session_id {
                    Some(s) => s.clone(),
                    None => return,
                };
                let msg_message_id = match &msg.message_id {
                    Some(m) => m.clone(),
                    None => return,
                };

                // CRITICAL: Ignore events from other sessions (prevents chat bleed).
                // BUT: Allow events when currentSession is null (new chat race condition).
                // Matches React: Chat.jsx lines 395-397
                let current_sid = current_session_id.get_untracked();
                if current_sid.is_some() && current_sid.as_deref() != Some(&msg_session_id) {
                    return;
                }

                let state = chat_state_thinking.state().get_untracked();

                // If this is a new chat (currentSession is null) and we're in SENDING state,
                // we need to buffer/process the event. Matches React: Chat.jsx lines 401-444
                if current_sid.is_none() && state == crate::components::chat::ChatState::Sending {
                    // Create assistant message placeholder if it doesn't exist
                    set_messages.update(|msgs| {
                        if !msgs.iter().any(|m| m.message_id == msg_message_id) {
                            msgs.push(ChatMessageItem {
                                message_id: msg_message_id.clone(),
                                message_type: "assistant".to_string(),
                                content: String::new(),
                                timestamp: msg.timestamp.clone().unwrap_or_default(),
                                pinned: false,
                                sent_by: None,
                                thinking_events: Vec::new(),
                                token_usage: None,
                            });
                        }
                    });

                    // Transition to streaming when first thinking event arrives
                    let current_state = chat_state_thinking.state().get_untracked();
                    if current_state == crate::components::chat::ChatState::Sending {
                        chat_state_thinking.start_streaming(&msg_message_id);
                    }

                    // Process the thinking event via the thinking manager
                    thinking_manager_thinking.handle_thinking_event(
                        &msg_message_id,
                        thinking_event.clone(),
                        token_usage.clone(),
                    );

                    // Also update thinking_map for ChatMessage display
                    set_thinking_map.update(|map| {
                        let current = map
                            .get(&msg_message_id)
                            .cloned()
                            .unwrap_or_default();
                        let updated_events =
                            crate::components::chat::process_thinking_event(&current.events, thinking_event);
                        map.insert(
                            msg_message_id.clone(),
                            ThinkingState {
                                events: updated_events,
                                is_active: true,
                                cancelled: false,
                                token_usage: token_usage.or(current.token_usage),
                            },
                        );
                    });

                    return; // Buffered and displayed — done
                } else if current_sid.is_none() {
                    // Not in SENDING state and no session — ignore
                    return;
                }

                // Transition to STREAMING state when first thinking event arrives.
                // Matches React: Chat.jsx lines 449-453
                if state == crate::components::chat::ChatState::Sending {
                    chat_state_thinking.start_streaming(&msg_message_id);
                }

                // Create assistant message immediately if it doesn't exist yet.
                // Matches React: Chat.jsx lines 456-469
                set_messages.update(|msgs| {
                    if !msgs.iter().any(|m| m.message_id == msg_message_id) {
                        msgs.push(ChatMessageItem {
                            message_id: msg_message_id.clone(),
                            message_type: "assistant".to_string(),
                            content: String::new(),
                            timestamp: msg.timestamp.clone().unwrap_or_default(),
                            pinned: false,
                            sent_by: None,
                            thinking_events: Vec::new(),
                            token_usage: None,
                        });
                    }
                });

                // Process the thinking event. Matches React: Chat.jsx lines 472-497
                thinking_manager_thinking.handle_thinking_event(
                    &msg_message_id,
                    thinking_event.clone(),
                    token_usage.clone(),
                );

                // Update thinking_map for ChatMessage display
                set_thinking_map.update(|map| {
                    let current = map
                        .get(&msg_message_id)
                        .cloned()
                        .unwrap_or_default();

                    // Don't restart animation if message was cancelled
                    if current.cancelled {
                        return;
                    }

                    let updated_events =
                        crate::components::chat::process_thinking_event(&current.events, thinking_event);
                    map.insert(
                        msg_message_id.clone(),
                        ThinkingState {
                            events: updated_events,
                            is_active: true,
                            cancelled: false,
                            token_usage: token_usage.or(current.token_usage),
                        },
                    );
                });
            });

            // ── token_usage_update ──────────────────────────────────────
            // Matches React: Chat.jsx lines 501-528
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

                let msg_session_id = match &msg.session_id {
                    Some(s) => s.as_str(),
                    None => return,
                };
                let msg_message_id = match &msg.message_id {
                    Some(m) => m.clone(),
                    None => return,
                };

                // CRITICAL: Ignore events from other sessions.
                // Allow events when currentSession is null (new chat race condition).
                let current_sid = current_session_id.get_untracked();
                if current_sid.is_some() && current_sid.as_deref() != Some(msg_session_id) {
                    return;
                }

                set_thinking_map.update(|map| {
                    let current = map
                        .get(&msg_message_id)
                        .cloned()
                        .unwrap_or_default();

                    // Don't update if message was cancelled
                    if current.cancelled {
                        return;
                    }

                    map.insert(
                        msg_message_id,
                        ThinkingState {
                            token_usage: Some(token_update),
                            ..current
                        },
                    );
                });
            });

            // ── chat_stream ─────────────────────────────────────────────
            // Matches React: Chat.jsx lines 530-568 and ChatInterface.jsx lines 275-288
            let unsub_chat_stream = ws.subscribe("chat_stream", move |msg| {
                let content = msg
                    .data
                    .as_ref()
                    .and_then(|d| d.get("content"))
                    .and_then(|v| v.as_str());

                let content = match content {
                    Some(c) if !c.is_empty() => c.to_string(),
                    _ => return,
                };

                let msg_session_id = match &msg.session_id {
                    Some(s) => s.as_str(),
                    None => return,
                };
                let msg_message_id = match &msg.message_id {
                    Some(m) => m.clone(),
                    None => return,
                };

                // CRITICAL: Ignore events from other sessions.
                let current_sid = current_session_id.get_untracked();
                if current_sid.is_some() && current_sid.as_deref() != Some(msg_session_id) {
                    return;
                }

                set_messages.update(|msgs| {
                    if let Some(existing) = msgs
                        .iter_mut()
                        .find(|m| m.message_id == msg_message_id && m.message_type == "assistant")
                    {
                        // Append content chunk to existing message
                        existing.content.push_str(&content);
                    } else {
                        // Create new assistant message
                        msgs.push(ChatMessageItem {
                            message_id: msg_message_id,
                            message_type: "assistant".to_string(),
                            content,
                            timestamp: msg.timestamp.unwrap_or_default(),
                            pinned: false,
                            sent_by: None,
                            thinking_events: Vec::new(),
                            token_usage: None,
                        });
                    }
                });
            });

            // ── chat_complete ───────────────────────────────────────────
            // Matches React: Chat.jsx lines 570-637 and ChatInterface.jsx lines 291-321
            let chat_state_complete = chat_state_ws.clone();
            let unsub_chat_complete = ws.subscribe("chat_complete", move |msg| {
                let msg_session_id = match &msg.session_id {
                    Some(s) => s.clone(),
                    None => return,
                };
                let msg_message_id = match &msg.message_id {
                    Some(m) => m.clone(),
                    None => return,
                };

                // CRITICAL: Ignore events from other sessions.
                let current_sid = current_session_id.get_untracked();
                if current_sid.is_some() && current_sid.as_deref() != Some(&msg_session_id) {
                    return;
                }

                let state = chat_state_complete.state().get_untracked();

                // Ignore chat_complete if we're in cancelling/cancelled state.
                // The error message from backend should not overwrite our cancellation message.
                // Matches React: Chat.jsx lines 588-591
                if state == crate::components::chat::ChatState::Cancelling
                    || state == crate::components::chat::ChatState::Cancelled
                {
                    return;
                }

                // Complete the chat state if this is the active session.
                // Matches React: Chat.jsx lines 593-597
                if current_sid.as_deref() == Some(&msg_session_id)
                    && (state == crate::components::chat::ChatState::Sending
                        || state == crate::components::chat::ChatState::Streaming)
                {
                    chat_state_complete.complete();
                }

                let full_content = msg
                    .data
                    .as_ref()
                    .and_then(|d| d.get("content"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                // Update message with full content and mark as not streaming.
                // Matches React: Chat.jsx lines 599-612
                set_messages.update(|msgs| {
                    for m in msgs.iter_mut() {
                        if m.message_id == msg_message_id && m.message_type == "assistant" {
                            if let Some(ref content) = full_content {
                                m.content = content.clone();
                            }
                            // Note: ChatMessageItem doesn't have is_streaming/model/usage fields
                            // Those are handled through the chat_state machine signals
                        }
                    }
                });

                // Stop thinking animation if request wasn't cancelled.
                // Matches React: Chat.jsx lines 622-637
                set_thinking_map.update(|map| {
                    if let Some(entry) = map.get_mut(&msg_message_id) {
                        if !entry.cancelled {
                            entry.is_active = false;
                        }
                    }
                });
            });

            // ── error ───────────────────────────────────────────────────
            // Matches React: Chat.jsx lines 641-653 and ChatInterface.jsx lines 358-363
            let chat_state_error = chat_state_ws.clone();
            let unsub_error = ws.subscribe("error", move |msg| {
                let error_message = msg
                    .data
                    .as_ref()
                    .and_then(|d| d.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("An error occurred");

                chat_state_error.set_error(error_message);
                set_is_loading.set(false);
            });

            // ── request_cancelled ───────────────────────────────────────
            // Matches React: Chat.jsx lines 656-694 and ChatInterface.jsx lines 337-356
            let chat_state_cancelled = chat_state_ws.clone();
            let unsub_request_cancelled = ws.subscribe("request_cancelled", move |msg| {
                let msg_message_id = match &msg.message_id {
                    Some(m) => m.clone(),
                    None => return,
                };

                // Confirm cancellation if this is the active message.
                if chat_state_cancelled.is_active_message(&msg_message_id) {
                    chat_state_cancelled.confirm_cancelled();
                }

                // Update the assistant message to show it was cancelled.
                set_messages.update(|msgs| {
                    for m in msgs.iter_mut() {
                        if m.message_id == msg_message_id && m.message_type == "assistant" {
                            m.content = "_Request cancelled by user._".to_string();
                        }
                    }
                });

                // Mark thinking as cancelled.
                set_thinking_map.update(|map| {
                    if let Some(entry) = map.get_mut(&msg_message_id) {
                        entry.is_active = false;
                        entry.cancelled = true;
                    }
                });
            });

            // ── Cleanup: unsubscribe all on component unmount ───────────
            on_cleanup(move || {
                unsub_session_created();
                unsub_title_update();
                unsub_agent_thinking();
                unsub_token_usage();
                unsub_chat_stream();
                unsub_chat_complete();
                unsub_error();
                unsub_request_cancelled();
            });
        });
    }

    // ── Filtered messages (pinned filter) ───────────────────────────────
    let filtered_messages = Memo::new(move |_| {
        let msgs = messages.get();
        let pinned_only = show_pinned_only.get();
        if pinned_only {
            msgs.into_iter().filter(|m| m.pinned).collect::<Vec<_>>()
        } else {
            msgs
        }
    });

    // Whether any messages are pinned (controls filter button visibility).
    // Will be used in Phase 9 (session header with pinned filter toggle).
    let _has_pinned = Memo::new(move |_| messages.get().iter().any(|m| m.pinned));

    // ── Callbacks ───────────────────────────────────────────────────────

    let on_toggle_pin = Callback::new(move |message_id: String| {
        set_messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().find(|m| m.message_id == message_id) {
                msg.pinned = !msg.pinned;
            }
        });
        // TODO: Phase 9 — call toggle_message_pin server function
    });

    let on_open_dashboard_modal = Callback::new(move |_content: String| {
        // TODO: Phase 9 — open SaveDashboardModal
    });

    let on_message_update = Callback::new(move |(_message_id, _new_content): (String, String)| {
        // TODO: Phase 9 — call update_message_content server function
    });

    // ── Send message handler (Task 8.1) ─────────────────────────────────
    // Matches React's sendMessage() in ChatInterface.jsx (lines 410-460).
    let chat_state_send = chat_state.clone();
    let navigate_send = navigate.clone();
    let ws_ctx_for_send = ws_ctx.clone();
    let on_send = Callback::new(move |input_text: String| {
        // Validate: input not empty, can_send, WebSocket connected
        if input_text.trim().is_empty() {
            return;
        }
        if !chat_state_send.can_send.get_untracked() {
            return;
        }
        let ws_connected = ws_ctx_for_send
            .as_ref()
            .map(|ctx| ctx.connection_state.get_untracked() == ConnectionState::Connected)
            .unwrap_or(false);
        if !ws_connected {
            return;
        }

        // Create optimistic user message with generated ID
        let user_message_id = generate_user_message_id();
        let user_message = ChatMessageItem {
            message_id: user_message_id,
            message_type: "user".to_string(),
            content: input_text.clone(),
            timestamp: String::new(), // Will be set by server
            pinned: false,
            sent_by: None,
            thinking_events: Vec::new(),
            token_usage: None,
        };

        // Add optimistic user message to messages
        set_messages.update(|msgs| {
            msgs.push(user_message);
        });

        // Compute time context
        let time_context = get_time_context();

        // Get the current session ID (may be None for new chats)
        let session_id = current_session_id.get_untracked();

        // Transition to SENDING state
        chat_state_send.start_sending(session_id.as_deref().unwrap_or("new"));

        // Call send_chat_message server function
        let chat_state_inner = chat_state_send.clone();
        let navigate_inner = navigate_send.clone();
        leptos::task::spawn_local(async move {
            let time_ctx = if time_context.is_empty() {
                None
            } else {
                Some(time_context)
            };

            match send_chat_message(
                input_text,
                session_id.clone(),
                time_ctx,
                false, // skip_ai
                None,  // model (use default)
            )
            .await
            {
                Ok(response) => {
                    // Update current_session_id from response (for new chats)
                    if session_id.is_none() {
                        set_current_session_id.set(Some(response.session_id.clone()));

                        // Navigate to the new session URL
                        let path = format!("/chat/{}", response.session_id);
                        navigate_inner(&path, Default::default());
                    }
                }
                Err(err) => {
                    // Display error as an assistant message
                    let error_msg = ChatMessageItem {
                        message_id: generate_user_message_id().replace("user-", "error-"),
                        message_type: "assistant".to_string(),
                        content: "Sorry, I encountered an error. Please try again.".to_string(),
                        timestamp: String::new(),
                        pinned: false,
                        sent_by: None,
                        thinking_events: Vec::new(),
                        token_usage: None,
                    };
                    set_messages.update(|msgs| {
                        msgs.push(error_msg);
                    });

                    let error_text = err.to_string();
                    chat_state_inner.set_error(&error_text);
                }
            }
        });
    });

    // ── Cancel handler (Task 8.4) ───────────────────────────────────────
    // Matches React's handleCancel() in ChatInterface.jsx (lines 471-480).
    let chat_state_cancel = chat_state.clone();
    let ws_ctx_for_cancel = ws_ctx.clone();
    let on_cancel = Callback::new(move |_: ()| {
        // Request cancellation via state machine — returns false if invalid state
        if !chat_state_cancel.request_cancel() {
            return;
        }

        // Check WebSocket connected
        let ws_connected = ws_ctx_for_cancel
            .as_ref()
            .map(|ctx| ctx.connection_state.get_untracked() == ConnectionState::Connected)
            .unwrap_or(false);
        if !ws_connected {
            return;
        }

        // Get the active message ID for the cancel request
        let message_id = chat_state_cancel
            .active_message_id()
            .get_untracked()
            .unwrap_or_default();

        // Send cancel_request via WebSocket
        // Matches React: `sendWebSocketMessage({ type: 'cancel_request', message_id: ... })`
        if let Some(ws) = ws_ctx_for_cancel.as_ref() {
            ws.send(serde_json::json!({
                "type": "cancel_request",
                "message_id": message_id,
            }));
        }
    });

    // ── Derived signals for ChatInput ───────────────────────────────────
    let can_send_signal = chat_state.can_send;
    let show_stop_button_signal = chat_state.show_stop_button;

    // ── Render ──────────────────────────────────────────────────────────

    view! {
        <div class="flex flex-col h-full bg-muted overflow-x-hidden" style="flex-direction: column;">
            <div class="flex-1 flex flex-col overflow-hidden">
                // Messages container
                <div
                    node_ref=messages_container_ref
                    class=move || format!(
                        "flex-1 overflow-y-auto {}",
                        if !messages.get().is_empty() { "p-4 md:p-6" } else { "" }
                    )
                >
                    {move || {
                        let msgs = filtered_messages.get();
                        let loading = is_loading.get();
                        let session = current_session_id.get();
                        let greeting = current_greeting.get();

                        if msgs.is_empty() {
                            if loading && session.is_some() {
                                // Loading existing session — spinner
                                // Matches React: isLoadingSession && currentSessionId (Chat.jsx line 1618)
                                view! {
                                    <div class="h-full flex items-center justify-center">
                                        <Spinner class="text-muted-foreground" />
                                    </div>
                                }.into_any()
                            } else {
                                // New chat — greeting (vertically centered)
                                // Matches React: new chat greeting (Chat.jsx lines 1624-1689)
                                view! {
                                    <div class="h-full flex items-center justify-center px-4">
                                        <div class="text-center w-full max-w-2xl -mt-24">
                                            <div class="mb-12">
                                                <div class="mb-6">
                                                    // Kyomi sparkle logo (matches React's inline SVG)
                                                    <svg class="w-16 h-16 mx-auto" viewBox="0 0 80 80" xmlns="http://www.w3.org/2000/svg">
                                                        <g transform="translate(40, 40)">
                                                            <g fill="#d97706">
                                                                <polygon points="0,-20 3,-8 0,-5 -3,-8"/>
                                                                <polygon points="14,-14 8,-3 5,-5 8,-8"/>
                                                                <polygon points="20,0 8,3 5,0 8,-3"/>
                                                                <polygon points="14,14 3,8 0,5 3,8"/>
                                                                <polygon points="0,20 -3,8 0,5 3,8"/>
                                                                <polygon points="-14,14 -8,3 -5,5 -8,8"/>
                                                                <polygon points="-20,0 -8,-3 -5,0 -8,3"/>
                                                                <polygon points="-14,-14 -3,-8 0,-5 -3,-8"/>
                                                            </g>
                                                            <circle cx="0" cy="0" r="4" fill="#d97706"/>
                                                        </g>
                                                    </svg>
                                                </div>
                                                <h1 class="text-3xl md:text-4xl font-normal text-foreground mb-8">
                                                    {greeting}
                                                </h1>
                                            </div>
                                        </div>
                                    </div>
                                }.into_any()
                            }
                        } else {
                            // Messages list
                            // Matches React: <div className="w-full max-w-full space-y-6"> (Chat.jsx line 1692)
                            view! {
                                <div class="w-full max-w-full space-y-6" style="display: block;">
                                    {msgs.into_iter().map(|message| {
                                        let msg_id = message.message_id.clone();

                                        // Create a derived signal for this message's thinking state
                                        let thinking_signal = Signal::derive({
                                            let msg_id = msg_id.clone();
                                            move || {
                                                thinking_map.get()
                                                    .get(&msg_id)
                                                    .cloned()
                                                    .unwrap_or_default()
                                            }
                                        });

                                        view! {
                                            <ChatMessage
                                                message=message
                                                thinking_state=thinking_signal
                                                is_streaming=is_streaming
                                                active_message_id=active_message_id
                                                current_session_id=Signal::derive(move || current_session_id.get())
                                                session_metadata=Signal::derive(move || session_metadata.get())
                                                current_user_id="".to_string()
                                                on_toggle_pin=on_toggle_pin
                                                on_open_dashboard_modal=on_open_dashboard_modal
                                                on_message_update=on_message_update
                                            />
                                        }
                                    }).collect_view()}
                                </div>
                            }.into_any()
                        }
                    }}
                    // Scroll anchor — always at the bottom of the messages container
                    <div node_ref=messages_end_ref />
                </div>

                // Chat input area — wired to send_message and cancel handlers
                <ChatInput
                    on_send=on_send
                    on_cancel=on_cancel
                    can_send=can_send_signal
                    show_stop_button=show_stop_button_signal
                    connection_state=connection_state_signal
                />
            </div>
        </div>
    }
}
