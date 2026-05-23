// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reusable copilot chat component — shared chat logic for all copilot sidebars.
//!
//! Delegates all state management, WebSocket subscriptions, session lifecycle,
//! and send logic to [`ChatEngine`]. This component is now a thin rendering
//! shell that wires the engine's signals to the UI.
//!
//! Consumers wrap this component with their own sidebar chrome (header, resize
//! handle, mobile layout).

#[cfg(target_arch = "wasm32")]
use std::collections::HashMap;

use leptos::prelude::*;

use super::chat_engine::{ChatEngine, ChatEngineConfig, SessionMode};
use super::websocket_client::WebSocketContext;
use super::{AgentThinking, ChatInput};
use crate::components::dashboard::MarkdownRenderer;
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

    // Per-message thinking start times — keyed by message ID.
    // Set once when thinking first becomes active for a message, so the
    // elapsed timer in AgentThinking does not reset on each tool call or
    // event update that causes the parent closure to re-run.
    // Must be declared before view! so the HashMap generic syntax does not
    // confuse the Leptos RSX parser (which sees `<` as an HTML tag opener).
    // WASM-only: on SSR the signal is never read (start times are always None).
    #[cfg(target_arch = "wasm32")]
    let thinking_start_times: RwSignal<HashMap<String, f64>> =
        RwSignal::new(HashMap::new());

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
                    let thinking_state_map = thinking.state().get();

                    msgs.iter().map(|msg| {
                        let is_user = msg.message_type == "user";
                        let content = msg.content.clone();
                        let msg_id = msg.message_id.clone();

                        // Get thinking state for this message (assistant only).
                        let ts = if !is_user {
                            thinking_state_map.get(&msg_id).cloned()
                        } else {
                            None
                        };
                        let has_thinking = ts.as_ref().is_some_and(|t| !t.events.is_empty());
                        let thinking_events = ts.as_ref().map(|t| t.events.clone()).unwrap_or_default();
                        let thinking_active = ts.as_ref().is_some_and(|t| t.is_active);
                        let thinking_token_usage = ts.as_ref().and_then(|t| t.token_usage.clone());

                        // Capture the start time for this message's thinking session.
                        // On WASM: record the timestamp the first time thinking becomes active
                        // for this message_id; return None when not active.
                        // On SSR (non-wasm32): always None — no timer runs server-side.
                        #[cfg(target_arch = "wasm32")]
                        let start_time_for_msg = if thinking_active {
                            let mut captured: Option<f64> = None;
                            thinking_start_times.update_untracked(|times| {
                                captured = Some(*times.entry(msg_id.clone()).or_insert_with(js_sys::Date::now));
                            });
                            captured
                        } else {
                            None
                        };
                        #[cfg(not(target_arch = "wasm32"))]
                        let start_time_for_msg: Option<f64> = None;

                        // Clone content for the action slot and markdown renderer.
                        let content_for_action = content.clone();
                        let content_for_render = content.clone();

                        // Derive streaming signal from thinking state for MarkdownRenderer.
                        let is_streaming_for_md = ts.as_ref().is_some_and(|t| t.is_active);

                        view! {
                            <div class=if is_user { "flex justify-end" } else { "flex justify-start" }>
                                <div class=if is_user {
                                    "bg-primary text-primary-foreground rounded-lg p-3 max-w-[85%]"
                                } else {
                                    "bg-card border border-border rounded-2xl shadow-sm p-4 max-w-[85%] overflow-hidden"
                                }>
                                    // Agent thinking panel — shown for assistant messages with thinking events
                                    {has_thinking.then(move || {
                                        match (thinking_token_usage.clone(), start_time_for_msg) {
                                            (Some(tu), Some(start)) => view! {
                                                <div class="mb-2">
                                                    <AgentThinking
                                                        thinking_events=thinking_events.clone()
                                                        is_active=thinking_active
                                                        token_usage=tu
                                                        start_time_ms=start
                                                    />
                                                </div>
                                            }.into_any(),
                                            (Some(tu), None) => view! {
                                                <div class="mb-2">
                                                    <AgentThinking
                                                        thinking_events=thinking_events.clone()
                                                        is_active=thinking_active
                                                        token_usage=tu
                                                    />
                                                </div>
                                            }.into_any(),
                                            (None, Some(start)) => view! {
                                                <div class="mb-2">
                                                    <AgentThinking
                                                        thinking_events=thinking_events.clone()
                                                        is_active=thinking_active
                                                        start_time_ms=start
                                                    />
                                                </div>
                                            }.into_any(),
                                            (None, None) => view! {
                                                <div class="mb-2">
                                                    <AgentThinking
                                                        thinking_events=thinking_events.clone()
                                                        is_active=thinking_active
                                                    />
                                                </div>
                                            }.into_any(),
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
                                            <MarkdownRenderer content=content_signal is_streaming=is_streaming_signal class="prose-kyomi-chat" />
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
