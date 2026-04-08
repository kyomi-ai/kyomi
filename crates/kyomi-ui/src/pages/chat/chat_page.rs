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
//! - Session header: inline editable title, Slack sync badge, share/private
//!   badge, share dropdown, pinned filter button (Phase 9)
//! - Message actions: toggle pin via server function, dashboard modal,
//!   chart info modal (Phase 9)
//! - Shared conversation features: skip AI checkbox, shared_chat_message
//!   WebSocket subscription, mark session read, sender attribution (Phase 9)
//! - Special navigation contexts: chart exploration ("Ask about this chart"),
//!   watch creation context (Phase 11)
//! - Empty states: no datasources, personal mode without LLM (Phase 12)

use std::collections::HashMap;

use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::components::Outlet;
use leptos_router::hooks::{use_location, use_navigate};

use super::chat_message::ChatMessage;
use crate::components::chat::websocket_client::{ConnectionState, WebSocketContext};
use crate::components::chat::ChatInput;
use crate::components::chat::{
    ChatStateMachine, InlineEditableTitle, ThinkingEvent, ThinkingManager, ThinkingState,
    TokenUsage,
};
use crate::components::dashboard::{ChartInfoModal, SaveDashboardModal};
use crate::components::{ConfirmDialog, Skeleton};
#[cfg(target_arch = "wasm32")]
use crate::server_fns::chat::get_chart_context;
use crate::server_fns::chat::{
    get_session_messages, mark_session_read, send_chat_message, share_session,
    toggle_message_pin, unshare_session, update_message_content, update_session_title,
    ChatMessageItem, SessionDetail,
};
use crate::server_fns::context::get_user_context;

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
    // NOTE: ChatPage is mounted as a ParentRoute view (path="/chat"). In Leptos
    // router, use_params_map() in a ParentRoute component only sees the PARENT
    // route's params — NOT the child routes' params. Since /chat has no params,
    // use_params_map().get("session_id") always returns empty string.
    //
    // Fix: parse session_id directly from use_location().pathname, which always
    // reflects the actual browser URL regardless of route nesting depth.
    let location = use_location();
    let url_session_id = Memo::new(move |_| {
        let pathname = location.pathname.get();
        // pathname is "/chat" or "/chat/:session_id"
        // Strip the "/chat/" prefix and treat the remainder as session_id.
        pathname
            .strip_prefix("/chat/")
            .map(|s| s.trim_matches('/'))
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    });

    // ── User context (for ownership checks, multi_user_enabled, personal mode) ──
    let user_ctx_resource = Resource::new(|| (), |_| get_user_context());

    // Derive the user's display name from the user context resource.
    // Uses the user's name if available, falls back to email prefix, then "there".
    let user_display_name = Memo::new(move |_| {
        user_ctx_resource
            .get()
            .and_then(|res| res.ok())
            .map(|ctx| {
                ctx.name
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| {
                        ctx.email
                            .split('@')
                            .next()
                            .unwrap_or("there")
                            .to_string()
                    })
            })
            .unwrap_or_else(|| "there".to_string())
    });

    // ── Query parameters (for chart exploration and watch creation) ──────
    // The location is used on WASM for query parameter parsing (Phase 11).
    let _location = use_location();

    // ── State signals ───────────────────────────────────────────────────
    // Matches React's useState declarations (Chat.jsx lines 216-233)
    let (messages, set_messages) = signal(Vec::<ChatMessageItem>::new());
    let (current_session_id, set_current_session_id) = signal(Option::<String>::None);
    let (session_title, set_session_title) = signal(String::new());
    let (session_metadata, set_session_metadata) = signal(SessionDetail::default());
    let (is_loading, set_is_loading) = signal(false);
    let (show_pinned_only, set_show_pinned_only) = signal(false);
    let (current_greeting, set_current_greeting) = signal(String::new());

    // Phase 9 — shared conversation: skip AI response checkbox
    let (skip_ai_response, set_skip_ai_response) = signal(false);

    // M8 — Track just-created sessions to skip redundant reload when URL changes.
    // When on_send creates a new session and navigates, the URL change triggers
    // the session loading effect. This signal lets us skip that redundant load.
    let just_created_session: RwSignal<Option<String>> = RwSignal::new(None);

    // Phase 9 — dashboard modal state
    let (dashboard_modal_open, set_dashboard_modal_open) = signal(false);
    let (dashboard_modal_content, set_dashboard_modal_content) = signal(String::new());
    let (_dashboard_modal_message_id, set_dashboard_modal_message_id) =
        signal(Option::<String>::None);

    // Phase 9 — chart info modal state
    let (chart_info_modal_open, set_chart_info_modal_open) = signal(false);
    let (chart_info_spec, set_chart_info_spec) = signal(Option::<String>::None);

    // Confirmation dialog for unshare action
    // Matches React: Chat.jsx lines 1405-1428 (confirm before unsharing)
    let (confirm_unshare_open, set_confirm_unshare_open) = signal(false);

    // Phase 11 — chart exploration context
    // Stores chart markdown to prepend to the first user message.
    let (chart_context, set_chart_context) = signal(Option::<String>::None);

    // Phase 11 — watch creation context (signals used on WASM only for
    // query parameter handling)
    let (_watch_context, _set_watch_context) = signal(false);

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

    // ── Session loading resource ────────────────────────────────────────
    // When url_session_id changes, load the session from the server.
    // Matches React's useEffect for urlSessionId (Chat.jsx lines 782-804).
    //
    // Uses Resource::new (the correct Leptos 0.8 pattern for async data
    // fetching) rather than Effect::new + spawn_local. The key Memo filters
    // out just-created sessions so we don't reload data that already arrived
    // via WebSocket.
    let session_id_to_load = Memo::new(move |_| {
        let session_id = url_session_id.get(); // tracked — triggers reload
        let just_created = just_created_session.get_untracked(); // untracked — no extra rerun
        if just_created.is_some() && just_created.as_deref() == session_id.as_deref() {
            // This session was just created by on_send; data arrives via WebSocket.
            // Return None to skip the fetch — clearing the flag is done in the Effect below.
            None
        } else {
            session_id
        }
    });

    let session_messages_resource = Resource::new(
        move || session_id_to_load.get(),
        move |session_id| async move {
            match session_id {
                Some(sid) => Some((sid.clone(), get_session_messages(sid).await)),
                None => None,
            }
        },
    );

    // ── State management effect: clear / reset on URL change ────────────
    // Handles the two navigation cases:
    //   1. Navigating TO a session — set is_loading, clear messages
    //   2. Navigating AWAY from all sessions — clear all state (new chat)
    let chat_state_for_load = chat_state.clone();
    Effect::new(move |_| {
        let new_session_id = url_session_id.get();
        let current = current_session_id.get_untracked();

        match (&new_session_id, &current) {
            // URL has a session ID and it's different from current — prepare for load
            (Some(sid), _) if Some(sid) != current.as_ref() => {
                // Check if this is a session we just created in on_send.
                // If so, streaming is already in progress via WebSocket — don't reset chat_state.
                let just_created = just_created_session.get_untracked();
                let is_just_created = just_created.as_deref() == Some(sid.as_str());
                // Clear just_created_session if we're navigating to a different session.
                // This must happen here (Effect) not in the Memo — writing to signals
                // inside Memo::new is illegal in Leptos 0.8 and causes WASM panics.
                if !is_just_created {
                    just_created_session.set(None);
                }
                // Do NOT clear messages here — clearing causes <For> to dispose item scopes
                // while ChatMessage's reactive effects are still pending, causing WASM panic:
                // "Tried to access a reactive value that has already been disposed."
                // The spinner is shown based on is_loading=true (checked first in the render logic),
                // and messages are replaced atomically when the new session's data loads.
                set_current_greeting.set(String::new());
                if !is_just_created {
                    set_is_loading.set(true);
                }
                set_current_session_id.set(Some(sid.clone()));
                // Only reset streaming state when navigating to a DIFFERENT session
                // AND we're not actively chatting. The WS `session_created` event often
                // arrives before the HTTP response, so `just_created_session` might not
                // be set yet when this Effect first fires. The state check provides a
                // second safety net: never reset if we're mid-conversation.
                let actively_chatting = matches!(
                    chat_state_for_load.state().get_untracked(),
                    crate::components::chat::ChatState::Sending
                    | crate::components::chat::ChatState::Streaming
                    | crate::components::chat::ChatState::Cancelling
                );
                if !is_just_created && !actively_chatting {
                    chat_state_for_load.reset();
                }
            }
            // URL has no session ID but we have a current session — clear state (new chat)
            (None, Some(_)) => {
                just_created_session.set(None);
                set_current_session_id.set(None);
                set_messages.set(Vec::new());
                set_session_title.set(String::new());
                set_session_metadata.set(SessionDetail::default());
                set_thinking_map.set(HashMap::new());
                chat_state_for_load.reset();
                set_current_greeting.set(generate_greeting(&user_display_name.get()));
            }
            _ => {}
        }
    });

    // ── Resource sync effect: apply loaded session data to signals ───────
    // Fires when session_messages_resource resolves. Populates messages,
    // session metadata, thinking state, and clears is_loading.
    Effect::new(move |_| {
        match session_messages_resource.get() {
            Some(Some((sid, Ok(response)))) => {
                // Set session metadata
                if let Some(ref title) = response.session.title {
                    set_session_title.set(title.clone());
                }
                let is_shared = response.session.shared;
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

                // M16 — Capture last message ID before the move
                let last_msg_id = response.messages.last().map(|m| m.message_id.clone());
                set_messages.set(response.messages);
                set_is_loading.set(false);

                // M16 — Mark session read for shared conversations
                if is_shared {
                    let read_sid = sid.clone();
                    leptos::task::spawn_local(async move {
                        let _ = mark_session_read(read_sid, last_msg_id).await;
                    });
                }
            }
            Some(Some((_, Err(_e)))) => {
                // Gracefully handle by setting empty messages
                // (matches React's catch block in loadSessionMessages)
                set_messages.set(Vec::new());
                set_is_loading.set(false);
            }
            Some(None) => {
                // session_id_to_load returned None — just_created_session skip path.
                // State is already set by the URL change effect; nothing to do here.
                set_is_loading.set(false);
            }
            None => {
                // Resource not yet resolved — is_loading was set by the URL change effect.
            }
        }
    });

    // ── Generate greeting on mount if no messages ───────────────────────
    // Matches React's useEffect for greeting (Chat.jsx lines 760-765)
    //
    // This effect is reactive to `user_display_name`, which starts as "there"
    // (before user_ctx_resource resolves) then updates to the real name.
    // We regenerate the greeting when the name changes from the placeholder
    // so the user sees their actual name, not "there".
    let (greeting_was_placeholder, set_greeting_was_placeholder) = signal(false);
    Effect::new(move |_| {
        let msgs = messages.get();
        let greeting = current_greeting.get();
        let name = user_display_name.get();
        let is_placeholder = name == "there";

        if msgs.is_empty() && url_session_id.get().is_none() {
            if greeting.is_empty() {
                // First render — generate greeting (may use placeholder name)
                set_current_greeting.set(generate_greeting(&name));
                set_greeting_was_placeholder.set(is_placeholder);
            } else if greeting_was_placeholder.get_untracked() && !is_placeholder {
                // User context resolved — regenerate with real name
                set_current_greeting.set(generate_greeting(&name));
                set_greeting_was_placeholder.set(false);
            }
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

            // Use try_read_untracked to avoid panicking if NodeRefs are disposed
            // (e.g., when the chat component unmounts while this Effect is pending).
            let container_guard = messages_container_ref.try_read_untracked();
            let end_guard = messages_end_ref.try_read_untracked();

            let (Some(container_guard), Some(end_guard)) = (container_guard, end_guard) else {
                return;
            };
            let (Some(container), Some(end_el)) = (container_guard.as_ref(), end_guard.as_ref())
            else {
                return;
            };

            let scroll_top = container.scroll_top();
            let scroll_height = container.scroll_height();
            let client_height = container.client_height();
            let distance_from_bottom = scroll_height - scroll_top - client_height;

            // Only auto-scroll if within 100px of bottom.
            // MINOR: 50ms debounce matches React's scrollToBottom behavior.
            if distance_from_bottom < 100 {
                let end_el = end_el.clone();
                gloo_timers::callback::Timeout::new(50, move || {
                    let opts = web_sys::ScrollIntoViewOptions::new();
                    opts.set_behavior(web_sys::ScrollBehavior::Smooth);
                    end_el.scroll_into_view_with_scroll_into_view_options(&opts);
                })
                .forget();
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

        let ws_ctx_for_effect = ws_ctx.clone();
        Effect::new(move |_| {
            let Some(ws) = ws_ctx_for_effect.as_ref().cloned() else {
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

                // Skip if this session was just created by on_send — metadata is already set
                if let Some(msg_sid) = &msg.session_id {
                    if just_created_session.get_untracked().as_deref() == Some(msg_sid.as_str()) {
                        return;
                    }
                }

                // Only process if we don't have a session ID yet AND we're in SENDING state
                if current_sid.is_none()
                    && msg.session_id.is_some()
                    && msg.data.is_some()
                    && state == crate::components::chat::ChatState::Sending
                {
                    let session_id = msg.session_id.as_ref().unwrap().clone();
                    let data = msg.data.as_ref().unwrap();

                    // Mark as just-created BEFORE navigating so the URL change Effect
                    // sees is_just_created=true and does NOT reset chat_state.
                    // The WS session_created event can arrive BEFORE the HTTP response
                    // (the backend sends WS events then returns the HTTP response), so
                    // just_created_session may still be None here. Setting it now prevents
                    // the URL change Effect from calling chat_state_for_load.reset().
                    just_created_session.set(Some(session_id.clone()));

                    // Navigate to the new session URL (replace: true for new chat)
                    let path = format!("/chat/{}", session_id);
                    navigate_session(&path, leptos_router::NavigateOptions {
                        replace: true,
                        ..Default::default()
                    });

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
            let chat_state_stream = chat_state_ws.clone();
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

                // Recover streaming state if it was lost during URL change.
                // During new chat creation, the URL transitions from /chat to
                // /chat/:session_id. Reactive effects may reset the chat state
                // to Idle before all WS events are processed. If we're receiving
                // stream data but the state isn't Streaming, restore it.
                // Only transition Idle→Streaming (Sending is fine, Cancelling should not be overridden).
                let stream_state = chat_state_stream.state().get_untracked();
                if stream_state == crate::components::chat::ChatState::Idle {
                    chat_state_stream.start_streaming(&msg_message_id);
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

            // ── shared_chat_message ────────────────────────────────────
            // Phase 9 — Handle messages from other users in shared conversations.
            // Matches React: Chat.jsx lines 695-744.
            let unsub_shared_chat_message = ws.subscribe("shared_chat_message", move |msg| {
                let data = match &msg.data {
                    Some(d) => d,
                    None => return,
                };

                // Only process if this belongs to our current session
                let msg_session_id = match &msg.session_id {
                    Some(s) => s.clone(),
                    None => return,
                };
                let current_sid = current_session_id.get_untracked();
                if current_sid.as_deref() != Some(&msg_session_id) {
                    return;
                }

                // Extract message fields from data
                let message_id = data
                    .get("message_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let client_msg_id = data
                    .get("client_msg_id")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let content = data
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let msg_type = data
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user")
                    .to_string();
                let timestamp = data
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let sent_by: Option<crate::server_fns::chat::SessionUser> = data
                    .get("sent_by")
                    .and_then(|v| serde_json::from_value(v.clone()).ok());

                set_messages.update(|msgs| {
                    // Dedup by client_msg_id (optimistic message from this user)
                    if let Some(ref cid) = client_msg_id {
                        if let Some(existing) = msgs.iter_mut().find(|m| m.message_id == *cid) {
                            existing.message_id = message_id.clone();
                            return;
                        }
                    }

                    // Dedup by message_id
                    if msgs.iter().any(|m| m.message_id == message_id) {
                        return;
                    }

                    // Add new message from other user
                    msgs.push(ChatMessageItem {
                        message_id,
                        message_type: msg_type,
                        content,
                        timestamp,
                        pinned: false,
                        sent_by,
                        thinking_events: Vec::new(),
                        token_usage: None,
                    });
                });

                // Mark session as read (fire-and-forget)
                let sid = msg_session_id;
                leptos::task::spawn_local(async move {
                    let _ = mark_session_read(sid, None).await;
                });
            });

            // ── Cleanup: unsubscribe all on component unmount ───────────
            // Wrap in SendWrapper because Box<dyn FnOnce()> is !Send but
            // on_cleanup requires Send+Sync.
            let unsub_session_created = send_wrapper::SendWrapper::new(unsub_session_created);
            let unsub_title_update = send_wrapper::SendWrapper::new(unsub_title_update);
            let unsub_agent_thinking = send_wrapper::SendWrapper::new(unsub_agent_thinking);
            let unsub_token_usage = send_wrapper::SendWrapper::new(unsub_token_usage);
            let unsub_chat_stream = send_wrapper::SendWrapper::new(unsub_chat_stream);
            let unsub_chat_complete = send_wrapper::SendWrapper::new(unsub_chat_complete);
            let unsub_error = send_wrapper::SendWrapper::new(unsub_error);
            let unsub_request_cancelled = send_wrapper::SendWrapper::new(unsub_request_cancelled);
            let unsub_shared_chat_message = send_wrapper::SendWrapper::new(unsub_shared_chat_message);
            on_cleanup(move || {
                unsub_session_created.take()();
                unsub_title_update.take()();
                unsub_agent_thinking.take()();
                unsub_token_usage.take()();
                unsub_chat_stream.take()();
                unsub_chat_complete.take()();
                unsub_error.take()();
                unsub_request_cancelled.take()();
                unsub_shared_chat_message.take()();
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
    let has_pinned = Memo::new(move |_| messages.get().iter().any(|m| m.pinned));

    // ── C5 — MCP deep-link chart context (?chart=<id>) ─────────────────
    // When a user clicks "Continue in Kyomi" in the MCP chart app (Claude.ai),
    // they arrive at /chat?chart=<contextId>. We fetch the stored context
    // from KV and bootstrap the conversation with the chart.
    // Matches React: Chat.jsx lines 835-886.
    #[cfg(target_arch = "wasm32")]
    {
        let location_search_chart = _location.search.clone();
        Effect::new(move |_| {
            let search = location_search_chart.get();
            if search.is_empty() {
                return;
            }

            let url_params = web_sys::UrlSearchParams::new_with_str(&search).ok();
            let chart_id = url_params.as_ref().and_then(|p| p.get("chart"));

            let Some(chart_id) = chart_id else {
                return;
            };

            // Fetch chart context from KV via server function
            leptos::task::spawn_local(async move {
                match get_chart_context(chart_id).await {
                    Ok(Some(ctx)) => {
                        // Store chart context for prepending to first user message
                        set_chart_context.set(Some(ctx.chart_markdown.clone()));

                        // Create an initial assistant message with the chart
                        let initial_message = ChatMessageItem {
                            message_id: format!(
                                "chart-context-{}",
                                js_sys::Date::now() as u64
                            ),
                            message_type: "assistant".to_string(),
                            content: format!(
                                "I'm ready to help you explore this chart:\n\n{}\n\nWhat would you like to know about it?",
                                ctx.chart_markdown
                            ),
                            timestamp: String::new(),
                            pinned: false,
                            sent_by: None,
                            thinking_events: Vec::new(),
                            token_usage: None,
                        };

                        set_messages.set(vec![initial_message]);
                        set_session_title.set(
                            if ctx.title.is_empty() {
                                "Chart Exploration".to_string()
                            } else {
                                format!("Exploring: {}", ctx.title)
                            },
                        );
                        set_current_greeting.set(String::new());
                    }
                    Ok(None) => {
                        // Chart context expired or invalid — show friendly message
                        let initial_message = ChatMessageItem {
                            message_id: format!(
                                "chart-context-error-{}",
                                js_sys::Date::now() as u64
                            ),
                            message_type: "assistant".to_string(),
                            content: "This chart link has expired or is no longer available. You can still ask me anything about your data!".to_string(),
                            timestamp: String::new(),
                            pinned: false,
                            sent_by: None,
                            thinking_events: Vec::new(),
                            token_usage: None,
                        };

                        set_messages.set(vec![initial_message]);
                        set_current_greeting.set(String::new());
                    }
                    Err(e) => {
                        tracing::warn!("Failed to fetch chart context: {e}");
                        let initial_message = ChatMessageItem {
                            message_id: format!(
                                "chart-context-error-{}",
                                js_sys::Date::now() as u64
                            ),
                            message_type: "assistant".to_string(),
                            content: "This chart link has expired or is no longer available. You can still ask me anything about your data!".to_string(),
                            timestamp: String::new(),
                            pinned: false,
                            sent_by: None,
                            thinking_events: Vec::new(),
                            token_usage: None,
                        };

                        set_messages.set(vec![initial_message]);
                        set_current_greeting.set(String::new());
                    }
                }

                // Clear the query parameter to prevent re-triggering on refresh
                if let Some(window) = web_sys::window() {
                    if let Ok(pathname) = window.location().pathname() {
                        let navigate = leptos_router::hooks::use_navigate();
                        navigate(&pathname, leptos_router::NavigateOptions {
                            replace: true,
                            ..Default::default()
                        });
                    }
                }
            });
        });
    }

    // ── Phase 11 — Chart exploration context ─────────────────────────────
    // Handle "Ask about this chart" navigation. When the user navigates to
    // /chat with ?exploreChart=true&chartMarkdown=... or ?createWatch=true,
    // we store the context and display an initial assistant message.
    // Matches React: Chat.jsx lines 806-833, 888-916.
    #[cfg(target_arch = "wasm32")]
    {
        let location_search = _location.search.clone();
        Effect::new(move |_| {
            let search = location_search.get();
            if search.is_empty() {
                return;
            }

            // Parse query params using web_sys::UrlSearchParams for proper decoding
            let url_params = web_sys::UrlSearchParams::new_with_str(&search).ok();
            let get_param = |name: &str| -> Option<String> {
                url_params.as_ref().and_then(|p| p.get(name))
            };

            let has_explore_chart = get_param("exploreChart").as_deref() == Some("true");
            let chart_markdown = get_param("chartMarkdown");
            let chart_title = get_param("chartTitle");
            let has_create_watch = get_param("createWatch").as_deref() == Some("true");

            if has_explore_chart {
                if let Some(ref markdown) = chart_markdown {
                    // Store chart context for prepending to first user message
                    set_chart_context.set(Some(markdown.clone()));

                    // Create an initial assistant message with the chart
                    let initial_message = ChatMessageItem {
                        message_id: format!("chart-context-{}", js_sys::Date::now() as u64),
                        message_type: "assistant".to_string(),
                        content: format!(
                            "I'm ready to help you explore this chart:\n\n{}\n\nWhat would you like to know about it?",
                            markdown
                        ),
                        timestamp: String::new(),
                        pinned: false,
                        sent_by: None,
                        thinking_events: Vec::new(),
                        token_usage: None,
                    };

                    set_messages.set(vec![initial_message]);
                    set_session_title.set(
                        chart_title
                            .map(|t| format!("Exploring: {}", t))
                            .unwrap_or_else(|| "Chart Exploration".to_string()),
                    );
                    set_current_greeting.set(String::new());
                }
            } else if has_create_watch {
                // Phase 11 — Watch creation context
                // Matches React: Chat.jsx lines 888-916
                _set_watch_context.set(true);

                let initial_message = ChatMessageItem {
                    message_id: format!("watch-create-{}", js_sys::Date::now() as u64),
                    message_type: "assistant".to_string(),
                    content: "I can help you set up a watch to monitor your data. What would you like me to keep an eye on?\n\n\
                        For example, you could say:\n\
                        - \"Alert me if daily revenue drops more than 10%\"\n\
                        - \"Watch for unusual spikes in error rates\"\n\
                        - \"Monitor our conversion rate and tell me if it changes significantly\"\n\
                        - \"Check our inventory levels daily and warn me if anything is running low\"\n\n\
                        Just describe what you want to monitor, and I'll set it up for you.".to_string(),
                    timestamp: String::new(),
                    pinned: false,
                    sent_by: None,
                    thinking_events: Vec::new(),
                    token_usage: None,
                };

                set_messages.set(vec![initial_message]);
                set_session_title.set("Setting up a Watch".to_string());
                set_current_greeting.set(String::new());
            }

            // M14 — Clear query params after processing so browser refresh
            // doesn't re-trigger the context effect.
            if has_explore_chart || has_create_watch {
                if let Some(window) = web_sys::window() {
                    if let Ok(location) = window.location().pathname() {
                        let navigate = leptos_router::hooks::use_navigate();
                        navigate(&location, leptos_router::NavigateOptions {
                            replace: true,
                            ..Default::default()
                        });
                    }
                }
            }
        });
    }

    // M16 — mark-session-read is now called inside the session loading
    // spawn_local (above) right after messages are loaded, rather than in a
    // separate Effect that over-fires on metadata/session_id changes.

    // ── Callbacks ───────────────────────────────────────────────────────

    // Phase 9 — Toggle pin: update local state + call server function
    // Matches React: Chat.jsx lines 1345-1359
    let on_toggle_pin = Callback::new(move |message_id: String| {
        // Optimistically toggle local state
        set_messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().find(|m| m.message_id == message_id) {
                msg.pinned = !msg.pinned;
            }
        });

        // Call server function (fire-and-forget)
        let sid = current_session_id.get_untracked();
        if let Some(session_id) = sid {
            let mid = message_id.clone();
            leptos::task::spawn_local(async move {
                if let Err(_e) = toggle_message_pin(session_id, mid).await {
                    // Revert on error would be ideal but matches React's
                    // fire-and-forget pattern (no error handling in React either)
                }
            });
        }
    });

    // Phase 9 — Show chart info modal with the chart's YAML spec
    // Matches React: handleShowChartInfo (Chat.jsx line 1324)
    let on_show_chart_info = Callback::new(move |spec: String| {
        set_chart_info_spec.set(Some(spec));
        set_chart_info_modal_open.set(true);
    });

    // Phase 9 — Open dashboard modal with message content
    // Matches React: Chat.jsx lines 1305-1311
    let on_open_dashboard_modal = Callback::new(move |content: String| {
        set_dashboard_modal_content.set(content);
        set_dashboard_modal_message_id.set(None);
        set_dashboard_modal_open.set(true);
    });

    // Phase 9 — Update message content via server function
    // Matches React: Chat.jsx lines 1361-1377
    let on_message_update = Callback::new(move |(message_id, new_content): (String, String)| {
        // Update local state immediately
        let updated_content = new_content.clone();
        set_messages.update(|msgs| {
            if let Some(msg) = msgs.iter_mut().find(|m| m.message_id == message_id) {
                msg.content = updated_content;
            }
        });

        // Call server function
        let sid = current_session_id.get_untracked();
        if let Some(session_id) = sid {
            let mid = message_id;
            leptos::task::spawn_local(async move {
                let _ = update_message_content(session_id, mid, new_content).await;
            });
        }
    });

    // Phase 9 — Share session handler
    // Matches React: Chat.jsx lines 1379-1403
    let handle_share = move |_| {
        let sid = current_session_id.get_untracked();
        if let Some(session_id) = sid {
            leptos::task::spawn_local(async move {
                match share_session(session_id).await {
                    Ok(()) => {
                        set_session_metadata.update(|m| m.shared = true);
                    }
                    Err(_e) => {
                        // Error handling — could show toast in future
                    }
                }
            });
        }
    };

    // Phase 9 — Unshare session handler
    // Matches React: Chat.jsx lines 1405-1428 — opens confirmation dialog first
    let handle_unshare = move |_| {
        set_confirm_unshare_open.set(true);
    };

    // Called when user confirms the "Make Private?" dialog
    let on_confirm_unshare = Callback::new(move |()| {
        set_confirm_unshare_open.set(false);
        let sid = current_session_id.get_untracked();
        if let Some(session_id) = sid {
            leptos::task::spawn_local(async move {
                match unshare_session(session_id).await {
                    Ok(()) => {
                        set_session_metadata.update(|m| m.shared = false);
                    }
                    Err(_e) => {
                        // Error handling — could show toast in future
                    }
                }
            });
        }
    });

    let on_cancel_unshare = Callback::new(move |()| {
        set_confirm_unshare_open.set(false);
    });

    // Phase 9 — Update session title handler
    // Matches React: Chat.jsx lines 1293-1303
    let on_title_save = Callback::new(move |new_title: String| {
        // M15 — Guard against empty titles (matches React: if (!newTitle.trim()) return)
        let trimmed = new_title.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        set_session_title.set(trimmed.clone());
        let sid = current_session_id.get_untracked();
        if let Some(session_id) = sid {
            leptos::task::spawn_local(async move {
                let _ = update_session_title(session_id, trimmed).await;
            });
        }
    });

    // ── Send message handler (Task 8.1, updated Phase 9/11) ─────────────
    // Matches React's sendMessage() in ChatInterface.jsx (lines 410-460)
    // and Chat.jsx lines 1121-1126 (chart context prepending).
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
            message_id: user_message_id.clone(),
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

        // Phase 11 — If this is the first message in a chart exploration context,
        // prepend the chart markdown. Matches React: Chat.jsx lines 1121-1126.
        let mut message_to_send = input_text.clone();
        if let Some(chart_md) = chart_context.get_untracked()
            && current_session_id.get_untracked().is_none()
        {
            message_to_send = format!(
                "Here's a chart I'd like to explore:\n\n{}\n\nMy question: {}",
                chart_md, input_text
            );
            set_chart_context.set(None); // Clear after first use
        }

        // Phase 9 — Read skip_ai flag for shared conversations
        let skip_ai = skip_ai_response.get_untracked();

        // Compute time context
        let time_context = get_time_context();

        // Get the current session ID (may be None for new chats)
        let session_id = current_session_id.get_untracked();

        // Transition to SENDING state
        chat_state_send.start_sending(session_id.as_deref().unwrap_or("new"));

        // Call send_chat_message server function
        let chat_state_inner = chat_state_send.clone();
        let navigate_inner = navigate_send.clone();
        // C4 — Keep optimistic message ID for updating after server response
        let optimistic_id = user_message_id.clone();
        // M9 — Pass client_msg_id for shared conversation deduplication
        let client_msg_id = Some(user_message_id);
        leptos::task::spawn_local(async move {
            let time_ctx = if time_context.is_empty() {
                None
            } else {
                Some(time_context)
            };

            match send_chat_message(
                message_to_send,
                session_id.clone(),
                time_ctx,
                skip_ai,
                None, // model (use default)
                client_msg_id,
            )
            .await
            {
                Ok(response) => {
                    // C4 — Update optimistic user message ID to server-assigned ID.
                    // React does: response.user_message_id → update optimistic msg.
                    if !response.user_message_id.is_empty() {
                        let server_id = response.user_message_id.clone();
                        set_messages.update(|msgs| {
                            if let Some(msg) = msgs.iter_mut().find(|m| m.message_id == optimistic_id) {
                                msg.message_id = server_id;
                            }
                        });
                    }

                    // Phase 9 — If skip_ai was enabled, reset state and return
                    // (no AI response expected). Matches React: Chat.jsx lines 1147-1151.
                    if response.skip_ai {
                        chat_state_inner.reset();
                        set_skip_ai_response.set(false);
                        // Still need to update session_id if new
                        if session_id.is_none() {
                            // M8 — Mark as just-created to skip redundant reload.
                            // Do NOT set current_session_id here — setting it before navigate
                            // causes a race: effects flush with url_session_id=None but
                            // current_session_id=Some(sid), triggering the (None, Some(_)) branch
                            // which resets chat_state and hides the stop button.
                            // The URL change Effect sets current_session_id after navigate completes.
                            just_created_session.set(Some(response.session_id.clone()));
                            let path = format!("/chat/{}", response.session_id);
                            navigate_inner(&path, leptos_router::NavigateOptions {
                                replace: true,
                                ..Default::default()
                            });
                        }
                        return;
                    }

                    // Update current_session_id from response (for new chats)
                    if session_id.is_none() {
                        // M8 — Mark as just-created to skip redundant reload.
                        // Do NOT set current_session_id here — setting it before navigate
                        // causes a race: effects flush with url_session_id=None but
                        // current_session_id=Some(sid), triggering the (None, Some(_)) branch
                        // which resets chat_state and hides the stop button.
                        // The URL change Effect sets current_session_id after navigate completes.
                        just_created_session.set(Some(response.session_id.clone()));

                        // Navigate to the new session URL
                        let path = format!("/chat/{}", response.session_id);
                        navigate_inner(&path, leptos_router::NavigateOptions {
                            replace: true,
                            ..Default::default()
                        });
                    }
                }
                Err(err) => {
                    // M11 — Display actual error text instead of generic message.
                    // React distinguishes budget-exhausted errors, etc.
                    let error_text = err.to_string();
                    let error_content = if error_text.is_empty() {
                        "Sorry, I encountered an error. Please try again.".to_string()
                    } else {
                        error_text.clone()
                    };
                    let error_msg = ChatMessageItem {
                        message_id: generate_user_message_id().replace("user-", "error-"),
                        message_type: "assistant".to_string(),
                        content: error_content,
                        timestamp: String::new(),
                        pinned: false,
                        sent_by: None,
                        thinking_events: Vec::new(),
                        token_usage: None,
                    };
                    set_messages.update(|msgs| {
                        msgs.push(error_msg);
                    });

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

    // Credits exhausted — disable chat input when workspace has no AI budget.
    // Matches React: Chat.jsx uses `creditsExhausted` from useCapabilities() to
    // disable the textarea and send button (Chat.jsx lines 1668-1677).
    let credits_exhausted = Signal::derive(move || {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|ctx| ctx.capabilities.get("credits_exhausted").copied())
            .unwrap_or(false)
    });

    // ── Derived: user context fields ──────────────────────────────────────
    let is_personal_mode = Signal::derive(move || {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .map(|ctx| ctx.is_personal_mode)
            .unwrap_or(false)
    });

    let multi_user_enabled = Signal::derive(move || {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|ctx| ctx.capabilities.get("multi_user_enabled").copied())
            .unwrap_or(false)
    });

    let current_user_id = Signal::derive(move || {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .map(|ctx| ctx.user_id.clone())
            .unwrap_or_default()
    });

    // Check if LLM is configured — read from capabilities.
    // In personal mode without an LLM, we show a special empty state.
    // The `llm_configured` capability is set by the backend when an API key
    // is present. We derive it from the user context resource.
    let llm_configured = Signal::derive(move || {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|ctx| ctx.capabilities.get("llm_configured").copied())
            .unwrap_or(true) // Default to true so we don't flash the empty state
    });

    // Check if any datasources exist (for empty state).
    let has_datasources = Signal::derive(move || {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .and_then(|ctx| ctx.capabilities.get("has_datasources").copied())
            .unwrap_or(true) // Default to true so we don't flash the empty state
    });

    // Is the current user the session owner?
    let is_owner = Signal::derive(move || {
        let uid = current_user_id.get();
        let metadata = session_metadata.get();
        metadata
            .created_by
            .as_ref()
            .map(|cb| cb.user_id == uid)
            .unwrap_or(true) // If no created_by, assume owner (single-user)
    });

    // ── Render ──────────────────────────────────────────────────────────

    // Phase 12 — Personal mode without LLM: show settings prompt
    // Matches React: ChatInterface.jsx lines 488-526 and Chat.jsx lines 1433-1457
    let personal_no_llm = move || is_personal_mode.get() && !llm_configured.get();

    // Phase 12 — No datasources empty state
    // Matches React: Chat.jsx lines 1459-1461
    let no_datasources = move || !has_datasources.get();

    view! {
        <>
        <Show
            when=move || !personal_no_llm()
            fallback=move || {
                // Phase 12.2 — Personal mode without LLM configured
                view! {
                    <div class="flex flex-col items-center justify-center h-full w-full p-8 bg-muted">
                        <div class="max-w-md w-full bg-card border border-border rounded-lg p-8 shadow text-center">
                            <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
                                <Icon icon=icondata_lu::LuMessageSquare attr:class="text-primary" width="32" height="32" />
                            </div>
                            <h2 class="text-xl font-semibold text-foreground mb-3">
                                "Chat requires an AI provider"
                            </h2>
                            <p class="text-muted-foreground mb-6">
                                "Use Kyomi from Claude Code via MCP, or add your own API key in Settings."
                            </p>
                            <div class="flex flex-col gap-3">
                                <a
                                    href="/settings/profile"
                                    class="inline-flex items-center justify-center rounded-md bg-primary px-4 py-2.5 text-sm font-medium text-primary-foreground hover:bg-primary/90 transition-colors"
                                >
                                    "Open Settings"
                                </a>
                                <a
                                    href="/setup"
                                    class="inline-flex items-center justify-center rounded-md border border-input bg-background px-4 py-2.5 text-sm font-medium hover:bg-secondary hover:text-accent-foreground transition-colors"
                                >
                                    "Learn about MCP"
                                </a>
                            </div>
                        </div>
                    </div>
                }
            }
        >
            <Show
                when=move || !no_datasources()
                fallback=move || {
                    // Phase 12.1 — No datasources empty state
                    // Matches React: NoDatasourcesEmptyState for context="chat"
                    view! {
                        <div class="flex flex-col items-center justify-center h-full w-full p-8 bg-muted">
                            <div class="max-w-md w-full bg-card border border-border rounded-lg p-8 shadow text-center">
                                <div class="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-6">
                                    <Icon icon=icondata_lu::LuDatabase attr:class="text-primary" width="32" height="32" />
                                </div>
                                <h2 class="text-xl font-semibold text-foreground mb-3">
                                    "Connect a data source to start chatting"
                                </h2>
                                <p class="text-muted-foreground mb-6">
                                    "Kyomi needs access to your data warehouse to answer questions about your data."
                                </p>
                                <a
                                    href="/settings/datasources"
                                    class="inline-flex items-center justify-center rounded-md bg-primary px-4 py-2.5 text-sm font-medium text-primary-foreground hover:bg-primary/90 transition-colors"
                                >
                                    "Connect Data Source"
                                </a>
                            </div>
                        </div>
                    }
                }
            >
                <div class="flex flex-col h-full bg-muted overflow-x-hidden" style="flex-direction: column;">
                    <div class="flex-1 flex flex-col overflow-hidden">
                        // Phase 9 — Chat Header (only shown when messages exist)
                        // Matches React: Chat.jsx lines 1467-1613
                        <Show when=move || !messages.get().is_empty()>
                            <div class="page-header h-16 px-4 md:px-6 flex-shrink-0 z-20 flex items-center justify-between gap-4">
                                    // Left side: title + badges
                                    <div class="flex items-center gap-2 min-w-0 flex-1 overflow-hidden">
                                        <Show
                                            when=move || current_session_id.get().is_some()
                                            fallback=move || view! {
                                                <div class="text-base font-semibold text-muted-foreground py-1">{"\u{00A0}"}</div>
                                            }
                                        >
                                            // Inline editable title
                                            <InlineEditableTitle
                                                value=Signal::derive(move || session_title.get())
                                                on_save=on_title_save
                                                placeholder="New Chat"
                                            />

                                            // Slack sync badge — show for channel threads (C/G prefix), not DMs (D prefix)
                                            // Matches React: Chat.jsx lines 1480-1497
                                            {move || {
                                                let meta = session_metadata.get();
                                                let is_slack_channel = meta.slack_channel_id.as_ref().map(|id| {
                                                    meta.shared && !id.starts_with('D')
                                                }).unwrap_or(false);

                                                if is_slack_channel {
                                                    view! {
                                                        <span class="inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold flex-shrink-0 gap-1" title="Synced with Slack channel thread">
                                                            // Slack icon
                                                            <svg class="w-3 h-3" viewBox="0 0 24 24" fill="currentColor">
                                                                <path d="M6 15a2 2 0 0 1-2 2a2 2 0 0 1-2-2a2 2 0 0 1 2-2h2v2zm1 0a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-5zm2-8a2 2 0 0 1-2-2a2 2 0 0 1 2-2a2 2 0 0 1 2 2v2H9zm0 1a2 2 0 0 1 2 2a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5zm8 2a2 2 0 0 1 2-2a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-2v-2zm-1 0a2 2 0 0 1-2 2a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2a2 2 0 0 1 2 2v5zm-2 8a2 2 0 0 1 2 2a2 2 0 0 1-2 2a2 2 0 0 1-2-2v-2h2zm0-1a2 2 0 0 1-2-2a2 2 0 0 1 2-2h5a2 2 0 0 1 2 2a2 2 0 0 1-2 2h-5z"/>
                                                            </svg>
                                                            "Slack Sync"
                                                        </span>
                                                    }.into_any()
                                                } else if multi_user_enabled.get() {
                                                    // Share/Private status badge
                                                    // Matches React: Chat.jsx lines 1497-1512
                                                    let shared = meta.shared;
                                                    let tooltip = if shared {
                                                        meta.created_by
                                                            .as_ref()
                                                            .map(|cb| format!("Owner: {}", cb.display_name))
                                                            .unwrap_or_else(|| "Owner: Unknown".to_string())
                                                    } else {
                                                        "Only you can see this conversation".to_string()
                                                    };
                                                    let badge_class = if shared {
                                                        "inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold flex-shrink-0 bg-primary text-primary-foreground"
                                                    } else {
                                                        "inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold flex-shrink-0 bg-secondary text-secondary-foreground"
                                                    };
                                                    let label = if shared { "Shared" } else { "Private" };
                                                    view! {
                                                        <span class=badge_class title=tooltip>
                                                            {label}
                                                        </span>
                                                    }.into_any()
                                                } else {
                                                    view! { <span /> }.into_any()
                                                }
                                            }}
                                        </Show>
                                    </div>

                                    // Right side: actions
                                    <div class="flex items-center gap-2">
                                        // Share dropdown (owner only, multi-user only)
                                        // Matches React: Chat.jsx lines 1522-1586
                                        <Show when=move || {
                                            current_session_id.get().is_some()
                                            && is_owner.get()
                                            && multi_user_enabled.get()
                                        }>
                                            {move || {
                                                let shared = session_metadata.get().shared;
                                                let slack_id = session_metadata.get().slack_channel_id.clone();
                                                let is_slack_channel = slack_id.as_ref().map(|id| !id.starts_with('D')).unwrap_or(false);

                                                if shared {
                                                    if is_slack_channel {
                                                        // Slack channel conversations cannot be made private
                                                        view! {
                                                            <button
                                                                disabled
                                                                class="flex items-center gap-1.5 px-3 py-1.5 text-sm text-muted-foreground opacity-50 cursor-not-allowed rounded-lg border border-border"
                                                                title="Slack channel conversations are always shared with your team"
                                                            >
                                                                <Icon icon=icondata_lu::LuLock width="16" height="16" />
                                                                <span class="hidden sm:inline">"Make Private"</span>
                                                            </button>
                                                        }.into_any()
                                                    } else {
                                                        view! {
                                                            <button
                                                                on:click=handle_unshare
                                                                class="flex items-center gap-1.5 px-3 py-1.5 text-sm text-muted-foreground hover:text-foreground hover:bg-secondary rounded-lg transition-colors border border-border"
                                                                title="Make this conversation private"
                                                            >
                                                                <Icon icon=icondata_lu::LuLock width="16" height="16" />
                                                                <span class="hidden sm:inline">"Make Private"</span>
                                                            </button>
                                                        }.into_any()
                                                    }
                                                } else {
                                                    view! {
                                                        <button
                                                            on:click=handle_share
                                                            class="flex items-center gap-1.5 px-3 py-1.5 text-sm text-muted-foreground hover:text-foreground hover:bg-secondary rounded-lg transition-colors border border-border"
                                                            title="Share this conversation with your workspace"
                                                        >
                                                            <Icon icon=icondata_lu::LuShare2 width="16" height="16" />
                                                            <span class="hidden sm:inline">"Share"</span>
                                                        </button>
                                                    }.into_any()
                                                }
                                            }}
                                        </Show>

                                        // Pinned messages filter button
                                        // Matches React: Chat.jsx lines 1588-1609
                                        <Show when=move || current_session_id.get().is_some() && has_pinned.get()>
                                            <button
                                                on:click=move |_| set_show_pinned_only.update(|v| *v = !*v)
                                                class=move || format!(
                                                    "flex items-center gap-2 px-3 py-1.5 rounded-lg transition-colors text-sm {}",
                                                    if show_pinned_only.get() {
                                                        "bg-accent text-foreground"
                                                    } else {
                                                        "text-muted-foreground hover:text-foreground hover:bg-secondary transition-colors"
                                                    }
                                                )
                                                aria-label=move || if show_pinned_only.get() { "Show all messages" } else { "Show only pinned messages" }
                                            >
                                                <Icon icon=icondata_lu::LuStar attr:class=move || if show_pinned_only.get() { "fill-current" } else { "" } width="16" height="16" />
                                                <span>{move || if show_pinned_only.get() { "Pinned Only" } else { "Pinned" }}</span>
                                            </button>
                                        </Show>
                                    </div>
                            </div>
                        </Show>

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

                                if loading && session.is_some() {
                                    // Loading existing session — skeleton placeholders shown immediately.
                                    // Checked FIRST (before msgs.is_empty()) to prevent reactive scope
                                    // disposal panic: old messages stay in the signal while loading,
                                    // so we must not let the <For> render while is_loading=true.
                                    // DESIGN.md: "Never use a bare spinner for page content."
                                    view! {
                                        <div class="p-4 md:p-6 space-y-6">
                                            // Skeleton: user message (right-aligned)
                                            <div class="flex flex-col items-end">
                                                <Skeleton class="h-12 w-48 rounded-2xl" />
                                                <Skeleton class="h-3 w-24 mt-1" />
                                            </div>
                                            // Skeleton: assistant message (full-width card)
                                            <div class="flex flex-col items-start w-full">
                                                <div class="w-full rounded-2xl border border-border bg-card shadow p-6 space-y-3">
                                                    <Skeleton class="h-4 w-3/4" />
                                                    <Skeleton class="h-4 w-full" />
                                                    <Skeleton class="h-4 w-2/3" />
                                                    <Skeleton class="h-32 w-full mt-2" />
                                                </div>
                                                <Skeleton class="h-3 w-20 mt-1" />
                                            </div>
                                            // Skeleton: another user message
                                            <div class="flex flex-col items-end">
                                                <Skeleton class="h-10 w-64 rounded-2xl" />
                                                <Skeleton class="h-3 w-20 mt-1" />
                                            </div>
                                        </div>
                                    }.into_any()
                                } else if msgs.is_empty() {
                                    // New chat — greeting + inline input (vertically centered)
                                    // Matches React: new chat greeting (Chat.jsx lines 1624-1689)
                                    // React renders the input INSIDE the centered greeting area, NOT
                                    // at the bottom of the viewport. The bottom ChatInput is hidden
                                    // when messages are empty.
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
                                                    <h1 class="text-3xl md:text-4xl font-serif font-normal text-foreground mb-8">
                                                        {greeting}
                                                    </h1>
                                                </div>
                                                // Inline chat input — centered with the greeting.
                                                // Matches React: Chat.jsx lines 1650-1686.
                                                // No cancel button needed on new chat screen.
                                                <div class="mt-8">
                                                    <ChatInput
                                                        on_send=on_send
                                                        on_cancel=on_cancel
                                                        can_send=can_send_signal
                                                        show_stop_button=show_stop_button_signal
                                                        connection_state=connection_state_signal
                                                        credits_exhausted=credits_exhausted.get()
                                                        inline=true
                                                    />
                                                </div>
                                            </div>
                                        </div>
                                    }.into_any()
                                } else {
                                    // Messages list
                                    // Matches React: <div className="w-full max-w-full space-y-6"> (Chat.jsx line 1692)
                                    // Use <For> keyed list so Leptos can reconcile items without
                                    // creating reactive signals inside a non-reactive closure,
                                    // which caused "disposed reactive value" WASM panics.
                                    view! {
                                        <div class="w-full max-w-full space-y-6" style="display: block;">
                                            <For
                                                each=move || filtered_messages.get()
                                                key=|msg| msg.message_id.clone()
                                                children=move |message| {
                                                    let msg_id = message.message_id.clone();
                                                    // Signal::derive inside <For> children is correct —
                                                    // each child has its own reactive scope.
                                                    let thinking_signal = Signal::derive({
                                                        let msg_id = msg_id.clone();
                                                        move || {
                                                            thinking_map.get()
                                                                .get(&msg_id)
                                                                .cloned()
                                                                .unwrap_or_default()
                                                        }
                                                    });
                                                    // Reactive pin state — derived from the messages signal
                                                    // so that on_toggle_pin updates propagate without
                                                    // re-running the <For> children closure.
                                                    let is_pinned = Signal::derive({
                                                        let msg_id = msg_id.clone();
                                                        move || {
                                                            messages.get()
                                                                .iter()
                                                                .find(|m| m.message_id == msg_id)
                                                                .map(|m| m.pinned)
                                                                .unwrap_or(false)
                                                        }
                                                    });
                                                    view! {
                                                        <ChatMessage
                                                            message=message
                                                            thinking_state=thinking_signal
                                                            is_streaming=is_streaming
                                                            active_message_id=active_message_id
                                                            current_session_id=current_session_id.into()
                                                            session_metadata=session_metadata.into()
                                                            current_user_id=current_user_id.get_untracked()
                                                            on_toggle_pin=on_toggle_pin
                                                            on_open_dashboard_modal=on_open_dashboard_modal
                                                            on_show_chart_info=on_show_chart_info
                                                            on_message_update=on_message_update
                                                            is_pinned=is_pinned
                                                        />
                                                    }
                                                }
                                            />
                                        </div>
                                    }.into_any()
                                }
                            }}
                            // Scroll anchor — always at the bottom of the messages container
                            <div node_ref=messages_end_ref />
                        </div>

                        // Bottom-pinned input area — only shown when messages exist.
                        // Matches React: Chat.jsx line 1718 — `{messages.length > 0 && (`
                        // When messages are empty (new chat), the input is rendered inline
                        // with the greeting above, not at the bottom.
                        <Show when=move || !messages.get().is_empty()>
                            // Phase 9 — "Skip AI response" checkbox for existing sessions
                            // M13: React shows this for ALL existing sessions (currentSessionId &&),
                            // not just shared ones.
                            <Show when=move || current_session_id.get().is_some()>
                                <div class="flex items-center gap-2 px-4 py-2 bg-muted">
                                    <input
                                        type="checkbox"
                                        id="skip-ai-checkbox"
                                        prop:checked=move || skip_ai_response.get()
                                        on:change=move |ev| {
                                            #[cfg(target_arch = "wasm32")]
                                            {
                                                use wasm_bindgen::JsCast;
                                                if let Some(target) = ev.target() {
                                                    if let Some(input) = target.dyn_ref::<web_sys::HtmlInputElement>() {
                                                        set_skip_ai_response.set(input.checked());
                                                    }
                                                }
                                            }
                                            #[cfg(not(target_arch = "wasm32"))]
                                            {
                                                let _ = ev;
                                            }
                                        }
                                        class="h-4 w-4 rounded-md border-input text-primary focus:ring-ring"
                                    />
                                    <label for="skip-ai-checkbox" class="text-sm text-muted-foreground">
                                        "Post as comment (skip AI response)"
                                    </label>
                                </div>
                            </Show>

                            // Chat input area — wired to send_message and cancel handlers
                            <ChatInput
                                on_send=on_send
                                on_cancel=on_cancel
                                can_send=can_send_signal
                                show_stop_button=show_stop_button_signal
                                connection_state=connection_state_signal
                                credits_exhausted=credits_exhausted.get()
                            />
                        </Show>
                    </div>
                </div>
            </Show>
        </Show>

        // Unshare confirmation dialog
        // Matches React: Chat.jsx lines 1405-1428 (confirm before making private)
        <ConfirmDialog
            open=Signal::derive(move || confirm_unshare_open.get())
            title="Make Private?"
            message="Are you sure you want to make this conversation private? Other workspace members will lose access to it."
            confirm_text="Make Private"
            destructive=false
            on_confirm=on_confirm_unshare
            on_cancel=on_cancel_unshare
        />

        // Save to Dashboard modal
        // Matches React: Chat.jsx lines 1803-1809
        {move || {
            let content = dashboard_modal_content.get();
            view! {
                <SaveDashboardModal
                    open=Signal::derive(move || dashboard_modal_open.get())
                    chart_yaml=content
                    on_close=Callback::new(move |()| set_dashboard_modal_open.set(false))
                    on_saved=Callback::new(move |_dashboard_id: String| {
                        set_dashboard_modal_open.set(false);
                    })
                />
            }
        }}

        // Chart info modal — shown when user clicks chart info button in MarkdownRenderer.
        // Matches React: <ChartInfoModal open={chartInfoModal.isOpen} spec={chartInfoModal.spec} onClose={...} />
        {move || chart_info_spec.get().map(|spec| {
            let yaml_signal = Signal::derive({
                let spec = spec.clone();
                move || spec.clone()
            });
            view! {
                <ChartInfoModal
                    open=Signal::derive(move || chart_info_modal_open.get())
                    yaml=yaml_signal
                    on_close=Callback::new(move |()| set_chart_info_modal_open.set(false))
                />
            }
        })}

        // Required by ParentRoute — renders the matched child route's view.
        // Child routes (/chat and /chat/:session_id) have empty views, so this
        // renders nothing but must be present for the router to function.
        <Outlet/>
        </>
    }
}
