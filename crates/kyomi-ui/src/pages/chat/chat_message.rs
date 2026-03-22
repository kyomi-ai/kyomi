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

use std::collections::HashMap;

use leptos::prelude::*;

use crate::components::chat::{AgentThinking, ThinkingState};
use crate::components::dashboard::MarkdownRenderer;
use crate::server_fns::chat::{ChatMessageItem, SessionDetail};

// ─── Relative time formatting ───────────────────────────────────────────────

/// Format an RFC 3339 timestamp as a human-readable relative time string.
///
/// Matches React's `formatRelativeTime()` from `lib/formatters.js`.
fn format_relative_time(rfc3339: &str) -> String {
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(rfc3339) else {
        return rfc3339.to_string();
    };

    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(parsed);

    let seconds = duration.num_seconds();
    if seconds < 60 {
        return "just now".to_string();
    }

    let minutes = duration.num_minutes();
    if minutes < 60 {
        return format!("{minutes}m ago");
    }

    let hours = duration.num_hours();
    if hours < 24 {
        return format!("{hours}h ago");
    }

    let days = duration.num_days();
    if days < 30 {
        return format!("{days}d ago");
    }

    // For older dates, show "Mar 15" or "Mar 15, 2025"
    let parsed_utc = parsed.with_timezone(&chrono::Utc);
    if parsed_utc.format("%Y").to_string() == now.format("%Y").to_string() {
        parsed_utc.format("%b %-d").to_string()
    } else {
        parsed_utc.format("%b %-d, %Y").to_string()
    }
}

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
#[allow(unused_variables)]
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
    /// Callback when message content is updated — receives (message_id, new_content).
    on_message_update: Callback<(String, String)>,
) -> impl IntoView {
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
    let message_pinned = message.pinned;
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
                    {sender_name_user}" \u{00B7} "{format_relative_time(&message_timestamp_user)}
                </div>
            </div>
        }
        .into_any()
    } else if is_user_message {
        // ── User message (someone else's in shared conversation) — left-aligned
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
                    {sender_name_user}" \u{00B7} "{format_relative_time(&message_timestamp_user)}
                </div>
            </div>
        }
        .into_any()
    } else {
        // ── Assistant message — full width ───────────────────────────────

        // Determine whether to show the AgentThinking panel.
        // Matches React logic: show if we have thinking data OR this is the active streaming message.
        let should_show_thinking = move || {
            let ts = thinking_state.get();
            let has_thinking_data = !ts.events.is_empty();
            let is_active_message = is_streaming.get()
                && active_message_id.get().as_deref() == Some(&message_id_for_thinking);
            has_thinking_data || is_active_message
        };

        // Build thinking events: merge stored events with live state.
        // Live state takes priority (it may have more recent events).
        let thinking_events_for_component = {
            let stored = stored_thinking_events.clone();
            move || {
                let live = thinking_state.get();
                if !live.events.is_empty() {
                    live.events.clone()
                } else {
                    stored.clone()
                }
            }
        };

        let thinking_is_active = move || thinking_state.get().is_active;

        let thinking_token_usage = {
            let stored = stored_token_usage.clone();
            move || {
                let live = thinking_state.get();
                live.token_usage.clone().or_else(|| stored.clone())
            }
        };

        // Content signal for MarkdownRenderer
        let content_signal = Signal::derive({
            let content = message_content.clone();
            move || content.clone()
        });

        view! {
            <div class="flex flex-col items-start">
                <div
                    id=format!("message-{}", message.message_id)
                    class="w-full px-6 py-4 bg-card border border-border rounded-2xl shadow-sm overflow-hidden"
                >
                    // Agent thinking header (expandable panel)
                    <Show when=should_show_thinking>
                        {
                            let events_fn = thinking_events_for_component.clone();
                            let active_fn = thinking_is_active;
                            let token_fn = thinking_token_usage.clone();
                            move || {
                                let events = events_fn();
                                let is_active = active_fn();
                                match token_fn() {
                                    Some(tu) => view! {
                                        <AgentThinking
                                            thinking_events=events
                                            is_active=is_active
                                            variant="header-bar"
                                            token_usage=tu
                                        />
                                    }.into_any(),
                                    None => view! {
                                        <AgentThinking
                                            thinking_events=events
                                            is_active=is_active
                                            variant="header-bar"
                                        />
                                    }.into_any(),
                                }
                            }
                        }
                    </Show>

                    // Message content — rendered as markdown with ChartML support
                    <Show when={
                        let content = message_content.clone();
                        move || !content.is_empty()
                    }>
                        <MarkdownRenderer
                            content=content_signal
                            parameters=Signal::derive(|| HashMap::new())
                        />
                    </Show>
                </div>

                // Footer: sender + timestamp + action buttons
                <Show when={
                    let content = message.content.clone();
                    move || !content.is_empty()
                }>
                    <div class="w-full flex items-center justify-between gap-2 mt-1 px-1">
                        // Sender name + timestamp
                        <div class="text-xs text-muted-foreground">
                            {sender_name_footer.clone()}" \u{00B7} "{format_relative_time(&message_timestamp)}
                        </div>

                        // Action buttons: pin + save to dashboard
                        <div class="flex items-center gap-3">
                            // Pin/unpin button (star icon)
                            <button
                                on:click={
                                    let mid = message_id_for_pin.clone();
                                    move |_| on_toggle_pin.run(mid.clone())
                                }
                                class=move || format!(
                                    "text-xs transition-colors flex items-center gap-1 {}",
                                    if message_pinned {
                                        "text-primary hover:text-primary/80"
                                    } else {
                                        "text-muted-foreground hover:text-foreground"
                                    }
                                )
                                aria-label=if message_pinned { "Unpin message" } else { "Pin message" }
                            >
                                <svg class="w-4 h-4" fill=if message_pinned { "currentColor" } else { "none" } stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                                </svg>
                            </button>

                            // Save to Dashboard button
                            <button
                                on:click={
                                    let content = message_content_for_save.clone();
                                    move |_| on_open_dashboard_modal.run(content.clone())
                                }
                                class="text-xs text-muted-foreground hover:text-primary transition-colors flex items-center gap-1"
                                aria-label="Save to Dashboard"
                            >
                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 7H5a2 2 0 00-2 2v9a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-3m-1 4l-3 3m0 0l-3-3m3 3V4" />
                                </svg>
                                <span>"Save to Dashboard"</span>
                            </button>
                        </div>
                    </div>
                </Show>
            </div>
        }
        .into_any()
    }
}
