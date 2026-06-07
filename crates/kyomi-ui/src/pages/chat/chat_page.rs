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

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};
use leptos_router::components::Outlet;
use leptos_router::hooks::{use_location, use_navigate};

use super::chat_message::ChatMessage;
use crate::components::chat::websocket_client::{ConnectionState, WebSocketContext};
use crate::components::chat::ChatInput;
use crate::components::chat::{
    ChatEngine, ChatEngineConfig, InlineEditableTitle, SessionMode, ThinkingEvent, TokenUsage,
};
use crate::components::dashboard::{ChartInfoModal, SaveDashboardModal};
use crate::components::button::{Button, ButtonLink, ButtonSize, ButtonVariant, ToggleButton};
use crate::components::{ConfirmDialog, EmptyState, Skeleton};
use std::sync::Arc;
#[cfg(target_arch = "wasm32")]
use crate::server_fns::chat::get_chart_context;
use crate::server_fns::chat::{
    get_session_messages, mark_session_read, send_chat_message, share_session,
    toggle_message_pin, unshare_session, update_message_content, update_session_title,
    ChatMessageItem, SessionDetail,
};
use crate::server_fns::context::UserContext;
use crate::chartml_provider::provide_chart_context;
use crate::cache::store::SyncStore;

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

use crate::utils::time::get_time_context;

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

/// Format cost for the chat footer — always 2 decimal places.
fn format_footer_cost(cost: f64) -> String {
    format!("${:.2}", cost)
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

    let is_on_chat_route = Memo::new(move |_| {
        location.pathname.get().starts_with("/chat")
    });

    // ── User context (for ownership checks, multi_user_enabled, personal mode) ──
    // Provided by the parent Layout as a LocalResource — one fetch per session,
    // no skeleton flash on navigation into /chat. LocalResource is required
    // here because the resource is read in Signal::derive closures which run
    // outside Suspense.
    let user_ctx_resource =
        expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

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

    // Must be at the component top level (not in an Effect) so provide_context
    // registers in this owner scope, visible to descendant ChartBlock components.
    {
        let workspace_id = user_ctx_resource
            .get()
            .and_then(|res| res.ok())
            .and_then(|ctx| ctx.workspace_id)
            .unwrap_or_else(|| "default".to_string());
        provide_chart_context(&workspace_id);
    }

    // ── Query parameters (for chart exploration and watch creation) ──────
    // The location is used on WASM for query parameter parsing (Phase 11).
    let _location = use_location();

    // ── Page-specific state signals ────────────────────────────────────
    let (current_session_id, set_current_session_id) = signal(Option::<String>::None);
    let (session_title, set_session_title) = signal(String::new());
    let (session_metadata, set_session_metadata) = signal(SessionDetail::default());
    let (is_loading, set_is_loading) = signal(false);
    let (show_pinned_only, set_show_pinned_only) = signal(false);
    let (current_greeting, set_current_greeting) = signal(String::new());

    // Phase 9 — shared conversation: skip AI response checkbox
    // RwSignal so it can be passed to ChatInput's skip_ai prop.
    let skip_ai_response: RwSignal<bool> = RwSignal::new(false);

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

    // ── ChatEngine — unified state container ────────────────────────────
    // Owns messages, thinking state, chat state machine, and the 6 standard
    // WS subscriptions (agent_thinking, chat_stream, chat_complete,
    // token_usage_update, error, request_cancelled).
    let engine = ChatEngine::new(ChatEngineConfig {
        session_mode: SessionMode::External {
            session_id: Signal::derive(move || current_session_id.get()),
        },
        context_type: None, // Main chat doesn't filter by context_type
        custom_ws_events: vec![],
        on_custom_ws_event: None,
        context_content: None,
        context_label: None,
    });

    // Convenience aliases for engine-owned state used throughout the component.
    let messages = engine.messages();
    let chat_state = engine.chat_state().clone();

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

    // ── Workspace settings (default model, token usage, etc.) ──────────
    // Provided by the layout-level SyncStore. Used to pass the workspace
    // default model to send_chat_message and to display the active model name.
    let sync_store = expect_context::<SyncStore>();
    let workspace_default_model = Signal::derive(move || {
        sync_store
            .workspace_settings()
            .get()
            .and_then(|ws| ws.default_model)
    });

    let show_token_usage = Signal::derive(move || {
        sync_store
            .workspace_settings()
            .get()
            .map(|ws| ws.show_token_usage)
            .unwrap_or(false)
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

    // ── Tier 2 cache: warm messages from IndexedDB before server responds ─
    // (KYO-215) On WASM, whenever url_session_id changes to a known session,
    // we immediately read any cached messages from IndexedDB and populate the
    // engine.  This means returning visitors see their previous messages
    // instantly — no skeleton — while the server fetch runs in background.
    //
    // Rule: use spawn_local inside a wasm32 block, never Resource::new, to
    // avoid desyncing Leptos serialized resource IDs (SSR_HYDRATION_GUIDE.md).
    #[cfg(target_arch = "wasm32")]
    {
        let engine_for_cache_read = engine.clone();
        let user_ctx_for_cache = user_ctx_resource;
        Effect::new(move |prev_sid: Option<Option<String>>| {
            let session_id = url_session_id.get();

            // Only run when we navigate TO a session (not away from one).
            let Some(ref sid) = session_id else {
                return session_id;
            };

            // Skip if the session ID hasn't changed since last run.
            if prev_sid.as_ref().and_then(|p| p.as_ref()) == Some(sid) {
                return session_id;
            }

            let sid = sid.clone();
            let engine_inner = engine_for_cache_read.clone();
            let ws_id = user_ctx_for_cache
                .get()
                .and_then(|r| r.ok())
                .and_then(|ctx| ctx.workspace_id)
                .unwrap_or_else(|| "default".to_string());

            leptos::task::spawn_local(async move {
                let Ok(db) =
                    crate::cache::db::init_cache_db(&ws_id).await
                else {
                    return;
                };

                let Ok(entries) = crate::cache::db::read_all(
                    &db,
                    kyomi_types::sync::entity_types::CHAT_MESSAGES,
                    &ws_id,
                )
                .await
                else {
                    return;
                };

                let Some((_id, json, _ts)) =
                    entries.iter().find(|(id, _, _)| id == &sid)
                else {
                    return;
                };

                let Ok(cached_messages) =
                    serde_json::from_str::<Vec<ChatMessageItem>>(json)
                else {
                    return;
                };

                // Only hydrate if the resource hasn't already resolved with
                // fresh data (checked untracked to avoid reactive dependency).
                // Use try_ variants: the user may have navigated away while
                // the IndexedDB read was in flight.
                if !cached_messages.is_empty() && is_loading.try_get_untracked().unwrap_or(false) {
                    engine_inner.try_set_messages(cached_messages);
                    set_is_loading.try_set(false);
                }
            });

            session_id
        });
    }

    // ── Tier 2 cache: write messages to IndexedDB after server load ───────
    // (KYO-215) After the resource resolves with fresh server data, persist
    // the messages so the next visit can load from cache.
    #[cfg(target_arch = "wasm32")]
    {
        let user_ctx_for_cache_write = user_ctx_resource;
        Effect::new(move |_| {
            let Some(Some((ref sid, Ok(ref response)))) = session_messages_resource.get() else {
                return;
            };

            let sid = sid.clone();
            let messages_to_cache = response.messages.clone();
            // Read untracked — workspace_id doesn't change within a session, and
            // tracking it here would re-run the write effect on every user-ctx
            // change unrelated to the messages themselves.
            let ws_id = user_ctx_for_cache_write
                .get_untracked()
                .and_then(|r| r.ok())
                .and_then(|ctx| ctx.workspace_id)
                .unwrap_or_else(|| "default".to_string());

            leptos::task::spawn_local(async move {
                let Ok(db) = crate::cache::db::init_cache_db(&ws_id).await else {
                    return;
                };

                let Ok(json) = serde_json::to_string(&messages_to_cache) else {
                    return;
                };

                // Use current timestamp as updated_at for the cache entry.
                let updated_at = {
                    let ts = js_sys::Date::now() as u64;
                    format!("{ts}")
                };

                if let Err(e) = crate::cache::db::upsert(
                    &db,
                    kyomi_types::sync::entity_types::CHAT_MESSAGES,
                    &sid,
                    &ws_id,
                    &json,
                    &updated_at,
                )
                .await
                {
                    tracing::warn!(
                        session_id = %sid,
                        "chat_messages cache write failed: {e}"
                    );
                }
            });
        });
    }

    // ── State management effect: clear / reset on URL change ────────────
    // Handles the two navigation cases:
    //   1. Navigating TO a session — set is_loading, clear messages
    //   2. Navigating AWAY from all sessions — clear all state (new chat)
    let engine_for_load = engine.clone();
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
                    engine_for_load.chat_state().state().get_untracked(),
                    crate::components::chat::ChatState::Sending
                    | crate::components::chat::ChatState::Streaming
                    | crate::components::chat::ChatState::Cancelling
                );
                if !is_just_created && !actively_chatting {
                    engine_for_load.chat_state().reset();
                }
            }
            // URL has no session ID but we have a current session — clear state (new chat)
            (None, Some(_)) => {
                if !location.pathname.get_untracked().starts_with("/chat") {
                    return;
                }
                just_created_session.set(None);
                set_current_session_id.set(None);
                engine_for_load.reset();
                set_session_title.set(String::new());
                set_session_metadata.set(SessionDetail::default());
                set_current_greeting.set(generate_greeting(&user_display_name.get()));
            }
            _ => {}
        }
    });

    // ── Resource sync effect: apply loaded session data to signals ───────
    // Fires when session_messages_resource resolves. Populates messages,
    // session metadata, thinking state, and clears is_loading.
    let engine_for_resource = engine.clone();
    Effect::new(move |_| {
        match session_messages_resource.get() {
            Some(Some((sid, Ok(response)))) => {
                // Set session metadata
                if let Some(ref title) = response.session.title {
                    set_session_title.set(title.clone());
                }
                let is_shared = response.session.shared;
                set_session_metadata.set(response.session);

                // Populate thinking state from stored events via the engine's
                // ThinkingManager. Replay each message's events, then mark complete.
                engine_for_resource.thinking().clear_all();
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

                    // Replay events through ThinkingManager for dedup/sorting
                    for event in events {
                        engine_for_resource.thinking().handle_thinking_event(
                            &msg.message_id,
                            event,
                            None, // token_usage set separately below
                        );
                    }

                    // Set token usage if present (works even when no events exist)
                    if let Some(usage) = token_usage {
                        engine_for_resource.thinking().update_token_usage(
                            &msg.message_id,
                            usage,
                        );
                    }

                    // Mark as complete (history events are not active)
                    engine_for_resource.thinking().complete_thinking(&msg.message_id);
                }

                // M16 — Capture last message ID before the move
                let last_msg_id = response.messages.last().map(|m| m.message_id.clone());
                engine_for_resource.set_messages(response.messages);
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
                engine_for_resource.set_messages(Vec::new());
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
    // near the bottom (within 100px). Delegated to the engine which
    // matches React's scrollToBottom with isNearBottom check.
    engine.setup_scroll(messages_container_ref);

    // ── Page-specific WebSocket subscriptions ─────────────────────────────
    // The 6 standard WS events (agent_thinking, chat_stream, chat_complete,
    // token_usage_update, error, request_cancelled) are handled by the engine.
    // Here we subscribe only to the 3 page-specific events:
    //   - session_created: navigates to new session URL
    //   - title_update: updates session title
    //   - shared_chat_message: handles messages from other users
    #[cfg(target_arch = "wasm32")]
    {
        let chat_state_ws = chat_state.clone();
        let navigate_ws = navigate.clone();

        let ws_ctx_for_effect = ws_ctx.clone();
        let engine_for_ws = engine.clone();
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
                if let Some(msg_sid) = &msg.session_id
                    && just_created_session.get_untracked().as_deref() == Some(msg_sid.as_str()) {
                        return;
                    }

                // Only process if we don't have a session ID yet AND we're in SENDING state
                if current_sid.is_none()
                    && state == crate::components::chat::ChatState::Sending
                    && let (Some(session_id_ref), Some(data)) =
                        (msg.session_id.as_ref(), msg.data.as_ref())
                {
                    let session_id = session_id_ref.clone();

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
                if let (Some(sid), Some(data)) = (&msg.session_id, &msg.data)
                    && let Some(title) = data.get("title").and_then(|v| v.as_str()) {
                        let current_sid = current_session_id.get_untracked();
                        if current_sid.as_deref() == Some(sid.as_str()) {
                            set_session_title.set(title.to_string());
                        }
                    }
            });

            // ── error (page-specific addition) ─────────────────────────
            // The engine handles state transition for errors; we also clear
            // the page-level is_loading flag as a safety net.
            let unsub_error = ws.subscribe("error", move |_msg| {
                set_is_loading.set(false);
            });

            // ── shared_chat_message ────────────────────────────────────
            // Phase 9 — Handle messages from other users in shared conversations.
            // Matches React: Chat.jsx lines 695-744.
            let engine_for_shared = engine_for_ws.clone();
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

                let mut msgs = engine_for_shared.messages().get_untracked();

                // Dedup by client_msg_id (optimistic message from this user)
                let mut deduped = false;
                if let Some(ref cid) = client_msg_id
                    && let Some(existing) = msgs.iter_mut().find(|m| m.message_id == *cid) {
                        existing.message_id = message_id.clone();
                        deduped = true;
                    }

                if !deduped {
                    // Dedup by message_id
                    if msgs.iter().any(|m| m.message_id == message_id) {
                        deduped = true;
                    }
                }

                if !deduped {
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
                }

                engine_for_shared.set_messages(msgs);

                // Mark session as read (fire-and-forget)
                let sid = msg_session_id;
                leptos::task::spawn_local(async move {
                    let _ = mark_session_read(sid, None).await;
                });
            });

            // ── Cleanup: unsubscribe page-specific events on unmount ────
            // The 6 standard events are cleaned up by the engine.
            let unsub_session_created = send_wrapper::SendWrapper::new(unsub_session_created);
            let unsub_title_update = send_wrapper::SendWrapper::new(unsub_title_update);
            let unsub_error = send_wrapper::SendWrapper::new(unsub_error);
            let unsub_shared_chat_message = send_wrapper::SendWrapper::new(unsub_shared_chat_message);
            on_cleanup(move || {
                unsub_session_created.take()();
                unsub_title_update.take()();
                unsub_error.take()();
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
        let location_search_chart = _location.search;
        let navigate_chart = navigate.clone();
        let engine_for_chart = engine.clone();
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
            let navigate_inner = navigate_chart.clone();
            let engine_inner = engine_for_chart.clone();
            // Use try_ variants throughout: the user may have navigated away
            // while the async KV fetch was in flight.
            leptos::task::spawn_local(async move {
                match get_chart_context(chart_id).await {
                    Ok(Some(ctx)) => {
                        // Store chart context for prepending to first user message
                        set_chart_context.try_set(Some(ctx.chart_markdown.clone()));

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
                            timestamp: chrono::Utc::now().to_rfc3339(),
                            pinned: false,
                            sent_by: None,
                            thinking_events: Vec::new(),
                            token_usage: None,
                        };

                        engine_inner.try_set_messages(vec![initial_message]);
                        set_session_title.try_set(
                            if ctx.title.is_empty() {
                                "Chart Exploration".to_string()
                            } else {
                                format!("Exploring: {}", ctx.title)
                            },
                        );
                        set_current_greeting.try_set(String::new());
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
                            timestamp: chrono::Utc::now().to_rfc3339(),
                            pinned: false,
                            sent_by: None,
                            thinking_events: Vec::new(),
                            token_usage: None,
                        };

                        engine_inner.try_set_messages(vec![initial_message]);
                        set_current_greeting.try_set(String::new());
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
                            timestamp: chrono::Utc::now().to_rfc3339(),
                            pinned: false,
                            sent_by: None,
                            thinking_events: Vec::new(),
                            token_usage: None,
                        };

                        engine_inner.try_set_messages(vec![initial_message]);
                        set_current_greeting.try_set(String::new());
                    }
                }

                // Clear the query parameter to prevent re-triggering on refresh
                if let Some(window) = web_sys::window()
                    && let Ok(pathname) = window.location().pathname() {
                        navigate_inner(&pathname, leptos_router::NavigateOptions {
                            replace: true,
                            ..Default::default()
                        });
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
        let location_search = _location.search;
        let navigate_explore = navigate.clone();
        let engine_for_explore = engine.clone();
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
                        timestamp: chrono::Utc::now().to_rfc3339(),
                        pinned: false,
                        sent_by: None,
                        thinking_events: Vec::new(),
                        token_usage: None,
                    };

                    engine_for_explore.set_messages(vec![initial_message]);
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
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    pinned: false,
                    sent_by: None,
                    thinking_events: Vec::new(),
                    token_usage: None,
                };

                engine_for_explore.set_messages(vec![initial_message]);
                set_session_title.set("Setting up a Watch".to_string());
                set_current_greeting.set(String::new());
            }

            // M14 — Clear query params after processing so browser refresh
            // doesn't re-trigger the context effect.
            if (has_explore_chart || has_create_watch)
                && let Some(window) = web_sys::window()
                    && let Ok(location) = window.location().pathname() {
                        navigate_explore(&location, leptos_router::NavigateOptions {
                            replace: true,
                            ..Default::default()
                        });
                    }
        });
    }

    // M16 — mark-session-read is now called inside the session loading
    // spawn_local (above) right after messages are loaded, rather than in a
    // separate Effect that over-fires on metadata/session_id changes.

    // ── Callbacks ───────────────────────────────────────────────────────

    // Phase 9 — Toggle pin: update local state + call server function
    // Matches React: Chat.jsx lines 1345-1359
    let engine_for_pin = engine.clone();
    let on_toggle_pin = Callback::new(move |message_id: String| {
        // Optimistically toggle local state
        let mut msgs = engine_for_pin.messages().get_untracked();
        if let Some(msg) = msgs.iter_mut().find(|m| m.message_id == message_id) {
            msg.pinned = !msg.pinned;
        }
        engine_for_pin.set_messages(msgs);

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
    let engine_for_update = engine.clone();
    let on_message_update = Callback::new(move |(message_id, new_content): (String, String)| {
        // Update local state immediately
        let mut msgs = engine_for_update.messages().get_untracked();
        if let Some(msg) = msgs.iter_mut().find(|m| m.message_id == message_id) {
            msg.content = new_content.clone();
        }
        engine_for_update.set_messages(msgs);

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
                        // try_update: the user may have navigated away while the
                        // server call was in flight.
                        set_session_metadata.try_update(|m| m.shared = true);
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
                        // try_update: the user may have navigated away while the
                        // server call was in flight.
                        set_session_metadata.try_update(|m| m.shared = false);
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
    let engine_for_send = engine.clone();
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

        // Add optimistic user message via engine (returns generated message_id)
        let user_message_id = engine_for_send.add_user_message(&input_text);

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
        let engine_inner = engine_for_send.clone();
        // C4 — Keep optimistic message ID for updating after server response
        let optimistic_id = user_message_id.clone();
        // M9 — Pass client_msg_id for shared conversation deduplication
        let client_msg_id = Some(user_message_id);
        // Capture the workspace default model at dispatch time so the value
        // reflects the model configured when the user clicked Send, not a
        // potentially-stale async read. Passing None falls back to the server's
        // own resolution chain (LLM_MODEL env var → built-in default).
        let dispatch_model = workspace_default_model.get_untracked();
        // All signal writes inside this async block use try_ variants: the user
        // may navigate away (or the copilot sidebar may close) while send_chat_message
        // is awaiting a server response, disposing the page-scoped signals.
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
                dispatch_model,
                client_msg_id,
            )
            .await
            {
                Ok(response) => {
                    // C4 — Update optimistic user message ID to server-assigned ID.
                    // React does: response.user_message_id → update optimistic msg.
                    if !response.user_message_id.is_empty() {
                        let server_id = response.user_message_id.clone();
                        if let Some(mut msgs) = engine_inner.messages().try_get_untracked() {
                            if let Some(msg) = msgs.iter_mut().find(|m| m.message_id == optimistic_id) {
                                msg.message_id = server_id;
                            }
                            engine_inner.try_set_messages(msgs);
                        }
                    }

                    // Phase 9 — If skip_ai was enabled, reset state and return
                    // (no AI response expected). Matches React: Chat.jsx lines 1147-1151.
                    if response.skip_ai {
                        // Guard: reset() and set() use RwSignal::set() internally;
                        // check the state signal is still alive before calling.
                        if chat_state_inner.state().try_get_untracked().is_some() {
                            chat_state_inner.reset();
                        }
                        skip_ai_response.try_set(false);
                        // Still need to update session_id if new
                        if session_id.is_none() {
                            // M8 — Mark as just-created to skip redundant reload.
                            // Do NOT set current_session_id here — setting it before navigate
                            // causes a race: effects flush with url_session_id=None but
                            // current_session_id=Some(sid), triggering the (None, Some(_)) branch
                            // which resets chat_state and hides the stop button.
                            // The URL change Effect sets current_session_id after navigate completes.
                            just_created_session.try_set(Some(response.session_id.clone()));
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
                        just_created_session.try_set(Some(response.session_id.clone()));

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
                        timestamp: chrono::Utc::now().to_rfc3339(),
                        pinned: false,
                        sent_by: None,
                        thinking_events: Vec::new(),
                        token_usage: None,
                    };
                    if let Some(mut msgs) = engine_inner.messages().try_get_untracked() {
                        msgs.push(error_msg);
                        engine_inner.try_set_messages(msgs);
                    }

                    // Guard: set_error() uses RwSignal::set() internally;
                    // check the state signal is still alive before calling.
                    if chat_state_inner.state().try_get_untracked().is_some() {
                        chat_state_inner.set_error(&error_text);
                    }
                }
            }
        });
    });

    // ── Cancel handler (Task 8.4) ───────────────────────────────────────
    // Matches React's handleCancel() in ChatInterface.jsx (lines 471-480).
    // Delegated to engine.cancel() which handles state transition and WS send.
    let engine_for_cancel = engine.clone();
    let on_cancel = Callback::new(move |_: ()| {
        engine_for_cancel.cancel();
    });

    // ── Derived signals for ChatInput ───────────────────────────────────
    let can_send_signal = chat_state.can_send;
    let show_stop_button_signal = chat_state.show_stop_button;

    // Extract thinking state ReadSignal (Copy) for use in the view template.
    // Must be extracted here rather than inside the view to avoid moving `engine`
    // into non-FnMut closures.
    let thinking_state_signal = engine.thinking().state();

    // Cumulative cost across all messages
    let cumulative_cost = Signal::derive(move || {
        thinking_state_signal
            .get()
            .values()
            .filter_map(|ts| ts.token_usage.as_ref())
            .map(|tu| tu.cost)
            .sum::<f64>()
    });

    // Latest context usage from the most recent assistant message
    let latest_context = Signal::derive(move || {
        let msgs = messages.get();
        let thinking_map = thinking_state_signal.get();
        msgs.iter()
            .rev()
            .filter(|m| m.message_type == "assistant")
            .find_map(|m| {
                thinking_map
                    .get(&m.message_id)
                    .and_then(|ts| ts.token_usage.clone())
            })
    });

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

    let is_self_hosted = Signal::derive(move || {
        user_ctx_resource
            .get()
            .and_then(|r| r.ok())
            .map(|ctx| ctx.is_self_hosted)
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
                    <div class="flex flex-col items-center justify-center h-full w-full p-8 bg-background">
                        <div class="max-w-md w-full bg-card border border-border rounded-lg p-8 shadow text-center">
                            <div class="text-muted-foreground mx-auto mb-6 flex items-center justify-center">
                                <Icon icon=phosphor_leptos::CHAT_CIRCLE_TEXT weight=IconWeight::Duotone size="64px" />
                            </div>
                            <h2 class="text-xl font-semibold text-foreground mb-3">
                                "Chat requires an AI provider"
                            </h2>
                            <p class="text-muted-foreground mb-6">
                                "Use Kyomi from Claude Code via MCP, or add your own API key in Settings."
                            </p>
                            <div class="flex flex-col gap-3">
                                <ButtonLink href="/settings/profile">"Open Settings"</ButtonLink>
                                <ButtonLink href="/setup" variant=ButtonVariant::Secondary>"Learn about MCP"</ButtonLink>
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
                    // Uses the shared EmptyState component to stay structurally
                    // identical to /dashboards and /knowledge.
                    view! {
                        <div class="flex flex-col h-full bg-background overflow-y-auto">
                            <div class="p-4 md:p-6 w-full">
                                <EmptyState
                                    icon=Arc::new(|| view! {
                                        <Icon icon=phosphor_leptos::DATABASE weight=IconWeight::Duotone size="64px" />
                                    }.into_any())
                                    title="Ask a question to get started."
                                    description="Connect a warehouse and Kyomi will turn natural-language questions into charts you can save to a dashboard."
                                    action=Arc::new(|| view! {
                                        <ButtonLink href="/settings/datasources">"Connect Data Source"</ButtonLink>
                                    }.into_any())
                                />
                            </div>
                        </div>
                    }
                }
            >
                <div class="flex flex-col h-full bg-background overflow-x-hidden" style="flex-direction: column;">
                    <div class="flex-1 flex flex-col overflow-hidden">
                        // Chat Header — only rendered when a session is active.
                        // The empty/no-session state is bare: the EmptyState below
                        // already carries its own heading and the top bar would
                        // duplicate that affordance. See KYO-21.
                        <Show when=move || !messages.get().is_empty()>
                            <div class="page-header h-16 px-4 md:px-6 flex-shrink-0 z-20 flex items-center justify-between gap-4">
                                    // Left side: back button + title + badges
                                    <div class="flex items-center gap-2 min-w-0 flex-1 overflow-hidden">
                                        // Back to chat list — DESIGN.md: detail pages must have back nav
                                        <ButtonLink
                                            href="/chats"
                                            variant=ButtonVariant::Ghost
                                            size=ButtonSize::Icon
                                            class="flex-shrink-0 text-muted-foreground hover:text-foreground"
                                            aria_label="Back to chats"
                                        >
                                            <Icon icon=phosphor_leptos::CARET_LEFT size="18px" />
                                        </ButtonLink>
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
                                                            <Button
                                                                variant=ButtonVariant::Secondary
                                                                size=ButtonSize::Sm
                                                                disabled=true
                                                                aria_label="Slack channel conversations are always shared with your team"
                                                            >
                                                                <Icon icon=phosphor_leptos::LOCK size="14px" />
                                                                <span class="hidden sm:inline">"Make Private"</span>
                                                            </Button>
                                                        }.into_any()
                                                    } else {
                                                        view! {
                                                            <Button
                                                                variant=ButtonVariant::Secondary
                                                                size=ButtonSize::Sm
                                                                aria_label="Make this conversation private"
                                                                on:click=handle_unshare
                                                            >
                                                                <Icon icon=phosphor_leptos::LOCK size="14px" />
                                                                <span class="hidden sm:inline">"Make Private"</span>
                                                            </Button>
                                                        }.into_any()
                                                    }
                                                } else {
                                                    view! {
                                                        <Button
                                                            variant=ButtonVariant::Secondary
                                                            size=ButtonSize::Sm
                                                            aria_label="Share this conversation with your workspace"
                                                            on:click=handle_share
                                                        >
                                                            <Icon icon=phosphor_leptos::SHARE_NETWORK size="14px" />
                                                            <span class="hidden sm:inline">"Share"</span>
                                                        </Button>
                                                    }.into_any()
                                                }
                                            }}
                                        </Show>

                                        // Pinned messages filter button
                                        // Matches React: Chat.jsx lines 1588-1609
                                        <Show when=move || current_session_id.get().is_some() && has_pinned.get()>
                                            <ToggleButton
                                                variant=Signal::derive(move || if show_pinned_only.get() { ButtonVariant::Active } else { ButtonVariant::Secondary })
                                                size=ButtonSize::Sm
                                                aria_label=MaybeProp::derive(move || Some(if show_pinned_only.get() { "Show all messages".to_string() } else { "Show only pinned messages".to_string() }))
                                                on:click=move |_| set_show_pinned_only.update(|v| *v = !*v)
                                            >
                                                <Icon icon=phosphor_leptos::STAR attr:class=move || if show_pinned_only.get() { "fill-current" } else { "" } size="14px" />
                                                <span>{move || if show_pinned_only.get() { "Pinned Only" } else { "Pinned" }}</span>
                                            </ToggleButton>
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
                                } else if msgs.is_empty() && is_on_chat_route.get() {
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
                                                    <h1 class="text-3xl md:text-4xl font-display font-normal text-foreground mb-8">
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
                                                            thinking_state_signal.get()
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
                            // Chat input area — wired to send_message and cancel handlers.
                            // Skip AI checkbox is built into ChatInput (show_skip_ai + skip_ai props).
                            <ChatInput
                                on_send=on_send
                                on_cancel=on_cancel
                                can_send=can_send_signal
                                show_stop_button=show_stop_button_signal
                                connection_state=connection_state_signal
                                credits_exhausted=credits_exhausted.get()
                                show_skip_ai=current_session_id.get().is_some()
                                skip_ai=skip_ai_response
                            />
                        </Show>

                        // Persistent footer: context usage + cumulative cost.
                        // Shown when token usage display is enabled, messages exist,
                        // and there is data to show.
                        // Footer: fixed height to prevent layout shift.
                        // Shows context % and cost when available.
                        <Show when=move || show_token_usage.get() && !messages.get().is_empty()>
                            <div class="flex-shrink-0 px-4 md:px-6 h-7 flex items-center justify-end bg-background">
                                <span class="text-xs text-muted-foreground font-mono">
                                    {move || {
                                        let mut parts = Vec::new();
                                        if let Some(tu) = latest_context.get()
                                            && tu.context_tokens > 0
                                            && tu.context_window > 0
                                        {
                                            let pct = (tu.context_tokens as f64
                                                / tu.context_window as f64
                                                * 100.0)
                                                .min(100.0);
                                            parts.push(format!("{:.0}%", pct));
                                        }
                                        // Only show cost in self-hosted mode.
                                        if is_self_hosted.get() {
                                            let cost = cumulative_cost.get();
                                            if cost > 0.0 {
                                                parts.push(format_footer_cost(cost));
                                            }
                                        }
                                        parts.join(" \u{00B7} ")
                                    }}
                                </span>
                            </div>
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
        <SaveDashboardModal
            open=Signal::derive(move || dashboard_modal_open.get())
            chart_yaml=dashboard_modal_content
            on_close=Callback::new(move |()| set_dashboard_modal_open.set(false))
            on_saved=Callback::new(move |_dashboard_id: String| {
                set_dashboard_modal_open.set(false);
            })
        />

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
