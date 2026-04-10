// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reusable copilot chat component — shared chat logic for all copilot sidebars.
//!
//! Extracts the common chat interface from dashboard copilot, chart builder copilot,
//! and watch agent sidebar into a single reusable component. Provides session management,
//! WebSocket streaming, message rendering with markdown, thinking animations, and input
//! with send/stop buttons.
//!
//! Consumers wrap this component with their own sidebar chrome (header, resize handle,
//! mobile layout).

use std::collections::HashMap;

use leptos::prelude::*;

use super::websocket_client::WebSocketContext;
use super::{AgentThinking, ChatInput, ChatStateMachine, ThinkingState};
#[cfg(feature = "hydrate")]
use super::{ThinkingEvent, TokenUsage, process_thinking_event};
use crate::components::dashboard::MarkdownRenderer;
use crate::components::{EmptyState, Spinner};
use crate::server_fns::copilot::{
    create_copilot_session, delete_copilot_session, send_copilot_message,
};

// ─── Chat message type ─────────────────────────────────────────────────────

/// A single chat message in the copilot conversation.
#[derive(Clone, Debug)]
struct CopilotMessage {
    /// Unique message ID (from WebSocket events for assistant, generated for user).
    message_id: String,
    /// "user" or "assistant"
    role: String,
    /// The message content.
    content: String,
}

// ─── Main component ────────────────────────────────────────────────────────

/// Reusable copilot chat interface.
///
/// Provides the full chat experience for copilot sidebars: session lifecycle,
/// WebSocket streaming, message rendering with markdown, thinking animations,
/// and input area with send/stop buttons.
///
/// Does NOT include any sidebar chrome — consumers wrap this component with
/// their own header, resize handle, and mobile layout.
#[component]
pub fn CopilotChat(
    /// Context type: "dashboard_copilot", "chart_builder_copilot", "watch_copilot"
    #[prop(into)]
    context_type: String,

    /// Content signal sent as context with messages (dashboard markdown, chart YAML, etc.)
    #[prop(into)]
    context_content: Signal<String>,

    /// Label for context prefix: "Dashboard Content", "Chart Content", etc.
    #[prop(into)]
    context_label: String,

    /// Session lifecycle control.
    /// Some(signal) = create when true, delete when false (dashboard/watch pattern)
    /// None = create immediately on mount (chart builder pattern)
    #[prop(into, optional)]
    active: Option<Signal<bool>>,

    /// Input placeholder text
    #[prop(into, optional, default = "Ask a question...".into())]
    placeholder: String,

    /// Empty state: icon render function
    #[prop(optional)]
    empty_icon: Option<ChildrenFn>,

    /// Empty state: title
    #[prop(into, optional, default = "Ask me anything!".into())]
    empty_title: String,

    /// Empty state: description
    #[prop(into, optional, default = "".into())]
    empty_description: String,

    /// Custom WS event names to subscribe to (e.g., ["dashboard_update"])
    #[prop(into, optional)]
    custom_ws_events: Vec<String>,

    /// Handler for custom WS events. Receives (event_name, data_value).
    #[prop(optional)]
    on_custom_ws_event: Option<Callback<(String, serde_json::Value)>>,

    /// Optional per-assistant-message action slot (e.g., "Apply to Dashboard" button).
    /// Receives the message content string, returns a view.
    #[prop(optional)]
    assistant_message_action:
        Option<std::sync::Arc<dyn Fn(String) -> AnyView + Send + Sync>>,
) -> impl IntoView {
    // Store props in StoredValues for use inside closures.
    let context_type_stored = StoredValue::new(context_type);
    let context_label_stored = StoredValue::new(context_label);
    let assistant_action_stored = StoredValue::new(assistant_message_action);
    let empty_icon_stored = StoredValue::new(empty_icon);
    let empty_title_stored = StoredValue::new(empty_title);
    let empty_description_stored = StoredValue::new(empty_description);

    let placeholder_stored = StoredValue::new(placeholder);

    // ── Chat state machine ─────────────────────────────────────────────
    let chat_state = ChatStateMachine::new();
    let chat_state_for_session = chat_state.clone();
    let chat_state_for_send = chat_state.clone();
    let chat_state_for_cancel = chat_state.clone();

    // ── Session state ──────────────────────────────────────────────────
    let (session_id, set_session_id) = signal(Option::<String>::None);

    // ── Chat messages ──────────────────────────────────────────────────
    let (messages, set_messages) = signal(Vec::<CopilotMessage>::new());

    // Track whether the first message has been sent (for context prefix).
    let (has_sent_first, set_has_sent_first) = signal(false);

    // Monotonic counter for user message IDs.
    let (user_msg_counter, set_user_msg_counter) = signal(0u32);

    // ── Thinking state (per message_id) ────────────────────────────────
    let (thinking_map, set_thinking_map) = signal(HashMap::<String, ThinkingState>::new());

    // ── WebSocket context ──────────────────────────────────────────────
    let ws_ctx = use_context::<WebSocketContext>();

    // Connection state signal for ChatInput.
    let ws_ctx_for_connection = ws_ctx.clone();
    let connection_state = Signal::derive(move || {
        ws_ctx_for_connection
            .as_ref()
            .map(|ws| ws.connection_state.get().to_string())
            .unwrap_or_else(|| "disconnected".to_string())
    });

    #[cfg(debug_assertions)]
    {
        if ws_ctx.is_none() {
            leptos::logging::warn!(
                "CopilotChat: WebSocketContext not found — WebSocket features will be disabled"
            );
        }
    }

    // ── Session lifecycle ──────────────────────────────────────────────
    Effect::new(move || {
        let should_be_active = active.is_none_or(|s| s.get());

        if should_be_active && session_id.get_untracked().is_none() {
            // Reset all state for fresh session.
            set_messages.set(Vec::new());
            chat_state_for_session.reset();
            set_thinking_map.set(HashMap::new());
            set_has_sent_first.set(false);

            let ctx_type = context_type_stored.get_value();
            let chat_state_err = chat_state_for_session.clone();
            leptos::task::spawn_local(async move {
                match create_copilot_session(ctx_type).await {
                    Ok(sid) => set_session_id.set(Some(sid)),
                    Err(e) => {
                        chat_state_err.set_error(&format!("Failed to start copilot: {e}"));
                    }
                }
            });
        } else if !should_be_active && let Some(sid) = session_id.get_untracked() {
            set_session_id.set(None);
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

    // ── WebSocket subscriptions ────────────────────────────────────────
    // Matches React ChatInterface.jsx lines 207-381 with configurable contextType.
    // Filters all events by context_type and session_id.
    #[cfg(target_arch = "wasm32")]
    {
        let chat_state_ws = chat_state.clone();
        let custom_events = custom_ws_events;
        let on_custom_event = on_custom_ws_event;

        Effect::new(move |_| {
            let Some(ws) = ws_ctx.as_ref().cloned() else {
                return;
            };

            let ctx_type_ws = StoredValue::new(context_type_stored.get_value());

            // Helper: check if event belongs to this copilot instance.
            // Uses StoredValue for ctx_type so the closure is Copy and can be
            // captured by multiple ws.subscribe callbacks on WASM.
            let should_handle =
                move |event_context_type: Option<&str>,
                      msg_session_id: Option<&str>|
                      -> bool {
                    let ctx_type = ctx_type_ws.get_value();
                    if event_context_type != Some(ctx_type.as_str()) {
                        return false;
                    }
                    let current_sid = session_id.get_untracked();
                    let Some(sid) = &current_sid else {
                        return false;
                    };
                    if let Some(msg_sid) = msg_session_id {
                        if msg_sid != sid.as_str() {
                            return false;
                        }
                    }
                    true
                };

            // ── agent_thinking ──────────────────────────────────────
            let chat_state_thinking = chat_state_ws.clone();
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
                set_messages.update(|msgs| {
                    if !msgs.iter().any(|m| m.message_id == msg_message_id) {
                        msgs.push(CopilotMessage {
                            message_id: msg_message_id.clone(),
                            role: "assistant".to_string(),
                            content: String::new(),
                        });
                    }
                });

                // Transition to streaming state.
                chat_state_thinking.start_streaming(&msg_message_id);

                // Update thinking state.
                set_thinking_map.update(|map| {
                    let current = map.get(&msg_message_id).cloned().unwrap_or_default();

                    if current.cancelled {
                        return;
                    }

                    let updated_events =
                        process_thinking_event(&current.events, thinking_event);
                    map.insert(
                        msg_message_id,
                        ThinkingState {
                            events: updated_events,
                            is_active: true,
                            cancelled: false,
                            token_usage: token_usage.or(current.token_usage),
                        },
                    );
                });
            });

            // ── chat_stream ─────────────────────────────────────────
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

                set_messages.update(|msgs| {
                    if let Some(existing) = msgs
                        .iter_mut()
                        .find(|m| m.message_id == msg_message_id && m.role == "assistant")
                    {
                        existing.content.push_str(&content);
                    } else {
                        msgs.push(CopilotMessage {
                            message_id: msg_message_id,
                            role: "assistant".to_string(),
                            content,
                        });
                    }
                });
            });

            // ── chat_complete ───────────────────────────────────────
            let chat_state_complete = chat_state_ws.clone();
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

                let full_content = msg
                    .data
                    .as_ref()
                    .and_then(|d| d.get("content"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                // Update message with full content.
                set_messages.update(|msgs| {
                    for m in msgs.iter_mut() {
                        if m.message_id == msg_message_id && m.role == "assistant" {
                            if let Some(ref content) = full_content {
                                m.content = content.clone();
                            }
                        }
                    }
                });

                // Stop thinking animation.
                set_thinking_map.update(|map| {
                    if let Some(entry) = map.get_mut(&msg_message_id) {
                        if !entry.cancelled {
                            entry.is_active = false;
                        }
                    }
                });

                chat_state_complete.complete();
            });

            // ── token_usage_update ──────────────────────────────────
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

                // Filter by session_id.
                let current_sid = session_id.get_untracked();
                if let Some(sid) = &current_sid {
                    if msg.session_id.as_deref() != Some(sid.as_str()) {
                        return;
                    }
                }

                set_thinking_map.update(|map| {
                    let current = map.get(&msg_message_id).cloned().unwrap_or_default();
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

            // ── error ───────────────────────────────────────────────
            let chat_state_error = chat_state_ws.clone();
            let ctx_type_error = context_type_stored.get_value();
            let unsub_error = ws.subscribe("error", move |msg| {
                let event_context_type = msg
                    .data
                    .as_ref()
                    .and_then(|d| d.get("context_type"))
                    .and_then(|v| v.as_str());

                if event_context_type != Some(ctx_type_error.as_str()) {
                    return;
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

            // ── request_cancelled ───────────────────────────────────
            let chat_state_cancelled = chat_state_ws.clone();
            let unsub_request_cancelled = ws.subscribe("request_cancelled", move |msg| {
                let msg_message_id = match &msg.message_id {
                    Some(m) => m.clone(),
                    None => return,
                };

                set_messages.update(|msgs| {
                    for m in msgs.iter_mut() {
                        if m.message_id == msg_message_id && m.role == "assistant" {
                            m.content = "_Request cancelled by user._".to_string();
                        }
                    }
                });

                set_thinking_map.update(|map| {
                    if let Some(entry) = map.get_mut(&msg_message_id) {
                        entry.is_active = false;
                        entry.cancelled = true;
                    }
                });

                chat_state_cancelled.confirm_cancelled();
            });

            // ── Custom WS event subscriptions ───────────────────────
            let mut custom_unsubs: Vec<send_wrapper::SendWrapper<Box<dyn FnOnce()>>> =
                Vec::new();

            for event_name in &custom_events {
                let event_name_clone = event_name.clone();
                let ctx_type_custom = context_type_stored.get_value();
                let on_custom_event = on_custom_event;

                let unsub = ws.subscribe(event_name, move |msg| {
                    let event_context_type = msg
                        .data
                        .as_ref()
                        .and_then(|d| d.get("context_type"))
                        .and_then(|v| v.as_str());

                    if event_context_type != Some(ctx_type_custom.as_str()) {
                        return;
                    }

                    // Filter by session_id.
                    let current_sid = session_id.get_untracked();
                    if let Some(sid) = &current_sid {
                        if msg.session_id.as_deref() != Some(sid.as_str()) {
                            return;
                        }
                    }

                    if let Some(data) = msg.data {
                        if let Some(cb) = on_custom_event {
                            cb.run((event_name_clone.clone(), data));
                        }
                    }
                });
                custom_unsubs.push(send_wrapper::SendWrapper::new(unsub));
            }

            // ── Cleanup: unsubscribe all on component unmount ───────
            let unsub_agent_thinking = send_wrapper::SendWrapper::new(unsub_agent_thinking);
            let unsub_chat_stream = send_wrapper::SendWrapper::new(unsub_chat_stream);
            let unsub_chat_complete = send_wrapper::SendWrapper::new(unsub_chat_complete);
            let unsub_token_usage = send_wrapper::SendWrapper::new(unsub_token_usage);
            let unsub_error = send_wrapper::SendWrapper::new(unsub_error);
            let unsub_request_cancelled =
                send_wrapper::SendWrapper::new(unsub_request_cancelled);

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
        });
    }

    // Suppress unused variable warnings on SSR where the wasm32 block is excluded.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (&custom_ws_events, &on_custom_ws_event);
    }

    // ── Send message handler ───────────────────────────────────────────
    let on_send = Callback::new(move |msg: String| {
        let sid = match session_id.get_untracked() {
            Some(sid) => sid,
            None => return,
        };

        // Generate user message ID.
        let counter = user_msg_counter.get_untracked();
        set_user_msg_counter.set(counter + 1);
        let user_msg_id = format!("user_{counter}");

        // Add user message optimistically.
        set_messages.update(|msgs| {
            msgs.push(CopilotMessage {
                message_id: user_msg_id,
                role: "user".to_string(),
                content: msg.clone(),
            });
        });

        // Build context prefix.
        let is_first = !has_sent_first.get_untracked();
        let content = context_content.get_untracked();
        let label = context_label_stored.get_value();
        let content_opt = if content.is_empty() {
            None
        } else if is_first {
            Some(format!("[{label}]\n{content}"))
        } else {
            Some(format!("[{label} has been updated]\n{content}"))
        };
        set_has_sent_first.set(true);

        let ctx_type = context_type_stored.get_value();
        chat_state_for_send.start_sending(&sid);

        let chat_state_err = chat_state_for_send.clone();
        leptos::task::spawn_local(async move {
            if let Err(e) = send_copilot_message(sid, msg, ctx_type, content_opt).await {
                chat_state_err.set_error(&format!("Failed to send: {e}"));
            }
        });
    });

    // ── Cancel handler ─────────────────────────────────────────────────
    let on_cancel = Callback::new(move |()| {
        chat_state_for_cancel.request_cancel();
    });

    // ── Scroll to bottom on new messages ───────────────────────────────
    let message_list_ref = NodeRef::<leptos::html::Div>::new();

    Effect::new(move || {
        // Track message changes.
        let _ = messages.get();

        #[cfg(feature = "hydrate")]
        if let Some(guard) = message_list_ref.try_read_untracked() {
            if let Some(el) = guard.as_ref() {
                let scroll_height = el.scroll_height() as f64;
                let scroll_top = el.scroll_top() as f64;
                let client_height = el.client_height() as f64;
                let distance_from_bottom = scroll_height - scroll_top - client_height;

                // Only auto-scroll if within 100px of bottom.
                if distance_from_bottom < 100.0 {
                    el.set_scroll_top(scroll_height as i32);
                }
            }
        }
    });

    // ── Render ──────────────────────────────────────────────────────────
    let chat_state_for_error = chat_state.clone();

    view! {
        <div class="flex flex-col flex-1 min-w-0 min-h-0">
            // Error banner
            {move || chat_state_for_error.error().get().map(|err| view! {
                <div class="p-4 text-center">
                    <p class="text-error-foreground mb-2">{err}</p>
                </div>
            })}

            // Message list
            <div class="flex-1 overflow-y-auto p-4 space-y-4" node_ref=message_list_ref>
                // Empty state
                <Show when=move || messages.get().is_empty() && chat_state.can_send.get()>
                    {move || {
                        // EmptyState's `icon` is `Option<ChildrenFn>` via `#[prop(optional)]`,
                        // so we must branch: pass the icon prop only when Some.
                        let title = empty_title_stored.get_value();
                        let description = empty_description_stored.get_value();
                        if let Some(icon_fn) = empty_icon_stored.get_value() {
                            view! {
                                <EmptyState
                                    icon=icon_fn
                                    title=title
                                    description=description
                                    class="border-0 bg-transparent"
                                />
                            }.into_any()
                        } else {
                            view! {
                                <EmptyState
                                    title=title
                                    description=description
                                    class="border-0 bg-transparent"
                                />
                            }.into_any()
                        }
                    }}
                </Show>

                // Messages
                {move || {
                    let msgs = messages.get();
                    let thinking = thinking_map.get();

                    msgs.iter().map(|msg| {
                        let is_user = msg.role == "user";
                        let content = msg.content.clone();
                        let msg_id = msg.message_id.clone();

                        // Get thinking state for this message (assistant only).
                        let thinking_state = if !is_user {
                            thinking.get(&msg_id).cloned()
                        } else {
                            None
                        };
                        let has_thinking = thinking_state.as_ref().is_some_and(|t| !t.events.is_empty());
                        let thinking_events = thinking_state.as_ref().map(|t| t.events.clone()).unwrap_or_default();
                        let thinking_active = thinking_state.as_ref().is_some_and(|t| t.is_active);
                        let thinking_token_usage = thinking_state.as_ref().and_then(|t| t.token_usage.clone());

                        // Clone content for the action slot and markdown renderer.
                        let content_for_action = content.clone();
                        let content_for_render = content.clone();

                        // Derive streaming signal from thinking state for MarkdownRenderer.
                        let is_streaming_for_md = thinking_state.as_ref().is_some_and(|t| t.is_active);

                        view! {
                            <div class=if is_user { "flex justify-end" } else { "flex justify-start" }>
                                <div class=if is_user {
                                    "bg-primary text-primary-foreground rounded-lg p-3 max-w-[85%]"
                                } else {
                                    "bg-card border border-border rounded-2xl shadow-sm p-4 max-w-[85%] overflow-hidden"
                                }>
                                    // Agent thinking panel — shown for assistant messages with thinking events
                                    {has_thinking.then(move || {
                                        if let Some(tu) = thinking_token_usage.clone() {
                                            view! {
                                                <div class="mb-2">
                                                    <AgentThinking
                                                        thinking_events=thinking_events.clone()
                                                        is_active=thinking_active
                                                        token_usage=tu
                                                    />
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <div class="mb-2">
                                                    <AgentThinking
                                                        thinking_events=thinking_events.clone()
                                                        is_active=thinking_active
                                                    />
                                                </div>
                                            }.into_any()
                                        }
                                    })}

                                    // Message content
                                    {if is_user {
                                        // User messages: plain text
                                        view! {
                                            <p class="text-sm whitespace-pre-wrap">{content}</p>
                                        }.into_any()
                                    } else if !content_for_render.is_empty() {
                                        // Assistant messages: markdown rendering
                                        let content_signal = Signal::derive({
                                            let c = content_for_render.clone();
                                            move || c.clone()
                                        });
                                        let is_streaming_signal = Signal::derive(move || is_streaming_for_md);

                                        view! {
                                            <MarkdownRenderer content=content_signal is_streaming=is_streaming_signal />
                                        }.into_any()
                                    } else {
                                        // Empty assistant message (thinking in progress)
                                        ().into_any()
                                    }}

                                    // Optional per-assistant-message action slot
                                    {(!is_user).then(move || {
                                        let action = assistant_action_stored.get_value();
                                        action.map(|render_fn| {
                                            render_fn(content_for_action.clone())
                                        })
                                    })}
                                </div>
                            </div>
                        }
                    }).collect_view()
                }}

                // Loading spinner — shown when waiting for first thinking event
                <Show when=move || chat_state.is_sending.get()>
                    <div class="flex justify-start">
                        <div class="bg-card border border-border rounded-2xl p-3 flex items-center gap-2 shadow-sm">
                            <Spinner class="text-muted-foreground" />
                            <span class="text-sm text-muted-foreground">"Thinking..."</span>
                        </div>
                    </div>
                </Show>
            </div>

            // Input area — using ChatInput
            <ChatInput
                on_send=on_send
                on_cancel=on_cancel
                can_send=chat_state.can_send
                show_stop_button=chat_state.show_stop_button
                can_cancel=chat_state.can_cancel
                connection_state=connection_state
                placeholder=placeholder_stored.get_value()
            />
        </div>
    }
}
