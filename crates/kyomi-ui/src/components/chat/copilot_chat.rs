// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reusable copilot chat component — shared chat logic for all copilot sidebars.
//!
//! Delegates all state management, WebSocket subscriptions, session lifecycle,
//! and send logic to [`ChatEngine`]. This component is now a thin rendering
//! shell that wires the engine's signals to the UI.
//!
//! Consumers wrap this component with their own sidebar chrome (header, resize
//! handle, mobile layout).

use leptos::prelude::*;

use super::agent_message_body::AgentMessageBody;
use super::chat_engine::{ChatEngine, ChatEngineConfig, SessionMode};
use super::websocket_client::WebSocketContext;
use super::ChatInput;
use crate::components::{EmptyState, Spinner};

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
    // ── Create the ChatEngine in Ephemeral mode ───────────────────────
    let engine = ChatEngine::new(ChatEngineConfig {
        session_mode: SessionMode::Ephemeral {
            context_type: context_type.clone(),
            active,
        },
        context_type: Some(context_type),
        custom_ws_events,
        on_custom_ws_event,
        context_content: Some(context_content),
        context_label: Some(context_label),
    });

    // ── Store rendering props ─────────────────────────────────────────
    let assistant_action_stored = StoredValue::new(assistant_message_action);
    let empty_icon_stored = StoredValue::new(empty_icon);
    let empty_title_stored = StoredValue::new(empty_title);
    let empty_description_stored = StoredValue::new(empty_description);
    let placeholder_stored = StoredValue::new(placeholder);

    // ── Set up scroll ─────────────────────────────────────────────────
    let message_list_ref = NodeRef::<leptos::html::Div>::new();
    engine.setup_scroll(message_list_ref);

    // ── Connection state for ChatInput ────────────────────────────────
    let ws_ctx = use_context::<WebSocketContext>();
    let connection_state = Signal::derive(move || {
        ws_ctx
            .as_ref()
            .map(|ws| ws.connection_state.get().to_string())
            .unwrap_or_else(|| "disconnected".to_string())
    });

    // ── Send callback ─────────────────────────────────────────────────
    let engine_for_send = engine.clone();
    let on_send = Callback::new(move |msg: String| {
        engine_for_send.send(msg);
    });

    // ── Cancel callback ───────────────────────────────────────────────
    let engine_for_cancel = engine.clone();
    let on_cancel = Callback::new(move |()| {
        engine_for_cancel.cancel();
    });

    // ── Read signals from engine ──────────────────────────────────────
    let messages = engine.messages();
    let chat_state = engine.chat_state().clone();
    let thinking = engine.thinking().clone();
    let chat_state_for_error = chat_state.clone();

    // ── Render ────────────────────────────────────────────────────────
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
                        let Some(title) = empty_title_stored.try_get_value() else { return ().into_any() };
                        let Some(description) = empty_description_stored.try_get_value() else { return ().into_any() };
                        if let Some(icon_fn) = empty_icon_stored.try_get_value().flatten() {
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

                // Messages — keyed <For> so each message component is stable across
                // streaming chunks. AgentThinking mounts once per message and updates
                // reactively via Signal props, preventing timer resets.
                <For
                    each=move || messages.get()
                    key=|msg| msg.message_id.clone()
                    children=move |msg| {
                        let is_user = msg.message_type == "user";
                        let content = msg.content.clone();
                        let msg_id = msg.message_id.clone();

                        // Per-message thinking derived signals — created inside the For
                        // child closure but outside any nested closures so they are
                        // stable reactive nodes tied to this message's scope.
                        let thinking_for_msg = thinking.clone();
                        let msg_thinking = Signal::derive({
                            let mid = msg_id.clone();
                            move || {
                                thinking_for_msg
                                    .state()
                                    .get()
                                    .get(&mid)
                                    .cloned()
                                    .unwrap_or_default()
                            }
                        });
                        // Copilot does not merge stored DB events — the engine only
                        // carries live ephemeral session messages, so `thinking_state`
                        // IS the source of truth. No stored/live merge needed here.
                        // msg_thinking is already Signal<ThinkingState> — pass it directly.

                        // Content must be derived reactively from `messages` keyed by
                        // `msg_id`. The <For> children closure runs only once per new
                        // message (on DiffOpAdd), so a static `content.clone()` would
                        // capture the initial empty string and never update during streaming.
                        let content_signal = Signal::derive({
                            let mid = msg_id.clone();
                            move || {
                                messages
                                    .get()
                                    .into_iter()
                                    .find(|m| m.message_id == mid)
                                    .map(|m| m.content.clone())
                                    .unwrap_or_default()
                            }
                        });

                        // Copilot uses `is_active` from thinking state as the streaming
                        // indicator — same logic as the original unkeyed map.
                        let is_streaming_sig =
                            Signal::derive(move || msg_thinking.get().is_active);

                        view! {
                            <div class=if is_user { "flex justify-end" } else { "flex justify-start" }>
                                <div class=if is_user {
                                    "bg-primary text-primary-foreground rounded-lg p-3 max-w-[85%]"
                                } else {
                                    "bg-card border border-border rounded-2xl shadow-sm p-4 max-w-[85%] overflow-hidden"
                                }>
                                    {if is_user {
                                        // User messages: plain text
                                        view! {
                                            <p class="text-sm whitespace-pre-wrap">{content}</p>
                                        }.into_any()
                                    } else {
                                        // Assistant messages: AgentMessageBody with optional action slot.
                                        // The action slot (e.g. "Apply to Dashboard") receives content
                                        // reactively — the render_fn is called inside a closure that
                                        // re-runs when content_signal changes, so it sees final content.
                                        let action_view = assistant_action_stored
                                            .try_get_value()
                                            .flatten()
                                            .map(|render_fn| {
                                                view! {
                                                    {move || render_fn(content_signal.get())}
                                                }
                                            });

                                        view! {
                                            <AgentMessageBody
                                                message_id=Signal::stored(msg_id)
                                                content=content_signal
                                                thinking_state=msg_thinking
                                                is_streaming=is_streaming_sig
                                            >
                                                {action_view}
                                            </AgentMessageBody>
                                        }.into_any()
                                    }}
                                </div>
                            </div>
                        }
                    }
                />

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
                placeholder=placeholder_stored.try_get_value().unwrap_or_default()
            />
        </div>
    }
}
