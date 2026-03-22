// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat page — session loading, message display, and new-chat greeting.
//!
//! Route: `/chat` (new chat) or `/chat/:session_id` (existing session).
//!
//! Ported from `apps/frontend/src/pages/Chat.jsx` (lines 206-1715 for state,
//! session loading, and message rendering). CSS classes are copied verbatim
//! from the React source.
//!
//! ## Features implemented in this phase:
//! - URL parameter parsing for `session_id`
//! - Session loading via `get_session_messages()` server function
//! - New chat mode with random greeting
//! - Messages container with `ChatMessage` rendering
//! - Pinned-only filter
//! - Smart scroll (auto-scroll only when near bottom)
//! - Loading state with spinner
//!
//! ## Features deferred to later phases:
//! - Message sending + WebSocket streaming (Phase 8)
//! - Session header with title editing (Phase 9)
//! - Chat input component wiring (Phase 8)

use std::collections::HashMap;

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use super::chat_message::ChatMessage;
use crate::components::chat::ThinkingState;
use crate::components::Spinner;
use crate::server_fns::chat::{get_session_messages, ChatMessageItem, SessionDetail};

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

// ─── Chat Page Component ────────────────────────────────────────────────────

/// Main chat page component.
///
/// Handles session loading from URL parameters, displays messages, and
/// shows a greeting screen for new chats. The message sending logic
/// (input, WebSocket streaming) is wired in Phase 8.
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
    // Populated when loading stored thinking events from session messages.
    let (thinking_map, set_thinking_map) =
        signal(HashMap::<String, ThinkingState>::new());

    // Streaming state — will be wired to WebSocket in Phase 8.
    // For now, these are static signals so ChatMessage can accept them.
    let is_streaming = Signal::derive(|| false);
    let active_message_id = Signal::derive(|| Option::<String>::None);

    // ── Refs for smart scroll ───────────────────────────────────────────
    let messages_end_ref = NodeRef::<leptos::html::Div>::new();
    let messages_container_ref = NodeRef::<leptos::html::Div>::new();

    // ── Session loading effect ──────────────────────────────────────────
    // When url_session_id changes, load the session from the server.
    // Matches React's useEffect for urlSessionId (Chat.jsx lines 782-804).
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
                                let events: Vec<crate::components::chat::ThinkingEvent> =
                                    msg.thinking_events
                                        .iter()
                                        .filter_map(|v| serde_json::from_value(v.clone()).ok())
                                        .collect();

                                let token_usage: Option<crate::components::chat::TokenUsage> =
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
    // These will be properly wired to server functions in Phase 8/9.
    // For now they update local state to demonstrate the UI behavior.

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
                                            // Chat input will be wired here in Phase 8
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

                // Input area placeholder — will be wired in Phase 8
                // The ChatInput component already exists from Phase 5; it will be
                // integrated with WebSocket sending and cancel logic in Phase 8.
            </div>
        </div>
    }
}
