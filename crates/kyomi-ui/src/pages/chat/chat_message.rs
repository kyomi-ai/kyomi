// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat message bubble component.
//!
//! Renders a single chat message — user messages right-aligned with primary
//! background, assistant messages full-width with card background. Includes
//! agent thinking header, markdown rendering, pin/save actions, and sender
//! attribution with relative timestamps.
//!
//! Ported from `apps/frontend/src/pages/Chat.jsx` `ChatMessage` component
//! (lines 35-203). CSS classes are copied verbatim from the React source.

use leptos::prelude::*;
use phosphor_leptos::Icon;
use crate::components::chat::{AgentThinking, ThinkingState};
use crate::components::dashboard::MarkdownRenderer;
use crate::components::Tooltip;
use crate::server_fns::chat::{ChatMessageItem, SessionDetail};

use super::format_relative_time;

// ─── Chat Message Component ─────────────────────────────────────────────────

/// A single chat message bubble.
///
/// Matches the React `ChatMessage` memo component exactly:
/// - User messages: right-aligned, primary background, sender + timestamp below
/// - Assistant messages: full-width, card background, thinking header, markdown
///   content, footer with sender + timestamp + pin/save actions
///
/// Props match the React component's destructured props.
// Props current_session_id, session_metadata, and on_message_update are part
// of the component API but will be wired in Phases 9-10 (session header,
// markdown renderer chat extensions). Suppress warnings until then.
#[component]
pub fn ChatMessage(
    /// The message data to render.
    message: ChatMessageItem,
    /// Thinking state for this message (events, isActive, tokenUsage).
    thinking_state: Signal<ThinkingState>,
    /// Whether any message is currently streaming.
    is_streaming: Signal<bool>,
    /// The message ID currently being streamed to (if any).
    active_message_id: Signal<Option<String>>,
    /// Current session ID (for MarkdownRenderer context).
    current_session_id: Signal<Option<String>>,
    /// Session metadata (shared status, owner info).
    session_metadata: Signal<SessionDetail>,
    /// Current user's ID for determining "is mine" alignment.
    current_user_id: String,
    /// Callback when pin/unpin is toggled — receives message_id.
    on_toggle_pin: Callback<String>,
    /// Callback to open save-to-dashboard modal — receives message content.
    on_open_dashboard_modal: Callback<String>,
    /// Callback to show chart info modal — receives the chart YAML spec.
    on_show_chart_info: Callback<String>,
    /// Callback when message content is updated — receives (message_id, new_content).
    on_message_update: Callback<(String, String)>,
    /// Reactive pin state — derived from the messages signal so updates propagate.
    is_pinned: Signal<bool>,
) -> impl IntoView {
    // These props are part of the API but will be wired in later phases.
    let _ = (&current_session_id, &session_metadata, &on_message_update);

    // ── Determine alignment and sender ──────────────────────────────────

    // In shared conversations, check sent_by.user_id.
    // In private conversations, sent_by is None, so all user messages are "mine".
    // Matches React: message.sender === 'user' && (!message.sent_by || message.sent_by.user_id === currentUser?.user_id)
    let is_user_message = message.message_type == "user";
    let is_my_message = is_user_message
        && match &message.sent_by {
            Some(sent_by) => sent_by.user_id == current_user_id,
            None => true,
        };

    // Sender name logic — matches React exactly:
    // For assistant messages: always "Kyomi"
    // For user messages: use sent_by.display_name if available, otherwise "You"
    let sender_name = if message.message_type == "assistant" {
        "Kyomi".to_string()
    } else {
        message
            .sent_by
            .as_ref()
            .map(|sb| sb.display_name.clone())
            .unwrap_or_else(|| "You".to_string())
    };

    // Clone message fields we need in closures
    let message_id_for_pin = message.message_id.clone();
    let message_id_for_thinking = message.message_id.clone();
    let message_content = message.content.clone();
    let message_content_for_save = message.content.clone();
    let message_timestamp = message.timestamp.clone();
    let message_timestamp_user = message.timestamp.clone();
    let sender_name_footer = sender_name.clone();
    let sender_name_user = sender_name.clone();

    // ── Parse stored thinking events from message ───────────────────────

    // Messages loaded from the database have thinking_events as JSON values.
    // Parse them into ThinkingEvent structs for the AgentThinking component.
    let stored_thinking_events: Vec<crate::components::chat::ThinkingEvent> = message
        .thinking_events
        .iter()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();

    let stored_token_usage: Option<crate::components::chat::TokenUsage> = message
        .token_usage
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    // ── Render ──────────────────────────────────────────────────────────

    if is_my_message {
        // ── User message (mine) — right-aligned ─────────────────────────
        let ts_display = format_relative_time(&message_timestamp_user);
        view! {
            <div class="flex flex-col items-end">
                <div
                    id=format!("message-{}", message.message_id)
                    class="max-w-sm sm:max-w-md lg:max-w-lg xl:max-w-2xl px-4 py-3 text-primary-foreground bg-primary rounded-2xl shadow-sm"
                >
                    <div class="text-sm">
                        {message_content}
                    </div>
                </div>
                // Sender name + timestamp below user message
                <div class="text-xs text-muted-foreground mt-1 px-1 text-right">
                    {sender_name_user}" \u{00B7} "{ts_display}
                </div>
            </div>
        }
        .into_any()
    } else if is_user_message {
        // ── User message (someone else's in shared conversation) — left-aligned
        let ts_display = format_relative_time(&message_timestamp_user);
        view! {
            <div class="flex flex-col items-start">
                <div
                    id=format!("message-{}", message.message_id)
                    class="max-w-sm sm:max-w-md lg:max-w-lg xl:max-w-2xl px-4 py-3 text-primary-foreground bg-primary rounded-2xl shadow-sm"
                >
                    <div class="text-sm">
                        {message_content}
                    </div>
                </div>
                // Sender name + timestamp below user message
                <div class="text-xs text-muted-foreground mt-1 px-1 text-left">
                    {sender_name_user}" \u{00B7} "{ts_display}
                </div>
            </div>
        }
        .into_any()
    } else {
        // ── Assistant message — full width ───────────────────────────────

        // Determine whether to show the AgentThinking panel.
        // Matches React logic: show if we have thinking data OR this is the active streaming message.
        let message_id_for_show_thinking = message_id_for_thinking.clone();
        let should_show_thinking = move || {
            let ts = thinking_state.get();
            let has_thinking_data = !ts.events.is_empty();
            let is_active_message = is_streaming.get()
                && active_message_id.get().as_deref() == Some(&message_id_for_show_thinking);
            has_thinking_data || is_active_message
        };

        // Build thinking events as a reactive Signal — live state takes priority
        // (it may have more recent events). Created outside <Show> so no .get()
        // calls happen inside the Show children block.
        let thinking_events_signal = Signal::derive({
            let stored = stored_thinking_events.clone();
            move || {
                let live = thinking_state.get();
                if !live.events.is_empty() {
                    live.events.clone()
                } else {
                    stored.clone()
                }
            }
        });

        let thinking_is_active_signal = Signal::derive(move || thinking_state.get().is_active);

        let thinking_token_usage = Signal::derive({
            let stored = stored_token_usage.clone();
            move || {
                let live = thinking_state.get();
                live.token_usage.clone().or_else(|| stored.clone())
            }
        });

        // Content signal for MarkdownRenderer.
        let content_signal = Signal::derive({
            let content = message_content.clone();
            move || content.clone()
        });

        // Streaming signal for this specific message.
        let msg_id_for_streaming = message.message_id.clone();
        let is_streaming_this_msg = Signal::derive(move || {
            is_streaming.get() && active_message_id.get().as_deref() == Some(&msg_id_for_streaming)
        });

        let ts_display_assistant = StoredValue::new(format_relative_time(&message_timestamp));
        let sender_name_footer = StoredValue::new(sender_name_footer);
        // Store String values so they can be accessed by Fn closures (not FnOnce).
        let message_id_for_pin = StoredValue::new(message_id_for_pin);
        let message_content_for_save = StoredValue::new(message_content_for_save);

        // Clones needed inside closures.
        let message_content_for_footer = message_content.clone();
        let message_content_for_show = message_content.clone();

        view! {
            <div class="flex flex-col items-start">
                <div
                    id=format!("message-{}", message.message_id)
                    class="w-full px-6 py-4 bg-card border border-border rounded-2xl shadow-sm overflow-hidden"
                >
                    <Show when=should_show_thinking>
                        <AgentThinking
                            thinking_events=thinking_events_signal
                            is_active=thinking_is_active_signal
                            token_usage=thinking_token_usage
                            message_id=Signal::stored(message_id_for_thinking.clone())
                        />
                    </Show>

                    <Show when=move || !message_content_for_show.is_empty()>
                        <MarkdownRenderer
                            content=content_signal
                            is_streaming=is_streaming_this_msg
                            on_chart_info=on_show_chart_info
                            class="prose-kyomi-chat"
                        />
                    </Show>
                </div>

                // Footer: sender · timestamp on left, pin + save-to-dashboard on right.
                // Only shown when message has content. Matches React Chat.jsx lines 119-161.
                <Show when=move || !message_content_for_footer.is_empty()>
                    <div class="w-full flex items-center justify-between gap-2 mt-1 px-1">
                        <div class="text-xs text-muted-foreground">
                            {sender_name_footer.get_value()}" \u{00B7} "{ts_display_assistant.get_value()}
                        </div>
                        <div class="flex items-center gap-3">
                            <Tooltip content="Pin / Unpin message">
                                <button
                                    class=move || {
                                        let base = "text-xs transition-colors flex items-center gap-1 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded";
                                        if is_pinned.get() {
                                            format!("{base} text-primary hover:text-primary/80")
                                        } else {
                                            format!("{base} text-muted-foreground hover:text-foreground")
                                        }
                                    }
                                    aria-label=move || if is_pinned.get() { "Unpin message" } else { "Pin message" }
                                    on:click=move |_| on_toggle_pin.run(message_id_for_pin.get_value())
                                >
                                    <Icon icon=phosphor_leptos::STAR attr:class=move || if is_pinned.get() { "fill-current" } else { "" } size="16px" />
                                </button>
                            </Tooltip>
                            <Tooltip content="Save to Dashboard">
                                <button
                                    class="text-xs text-muted-foreground hover:text-primary transition-colors flex items-center gap-1 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded"
                                    aria-label="Save to Dashboard"
                                    on:click=move |_| on_open_dashboard_modal.run(message_content_for_save.get_value())
                                >
                                    <Icon icon=phosphor_leptos::DOWNLOAD_SIMPLE size="16px" />
                                    <span>"Save to Dashboard"</span>
                                </button>
                            </Tooltip>
                        </div>
                    </div>
                </Show>
            </div>
        }
        .into_any()
    }
}
