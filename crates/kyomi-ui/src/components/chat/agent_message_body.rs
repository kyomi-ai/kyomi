// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared assistant-message body component.
//!
//! Renders the content of a single assistant message: the optional agent-thinking
//! panel, the markdown body, and an optional action slot (e.g. "Apply to
//! Dashboard"). Extracted so the main chat window and the dashboard copilot share
//! identical rendering logic without code duplication.
//!
//! The key property this component provides is **stability**: it accepts reactive
//! `Signal<T>` props so its internal `AgentThinking` and `MarkdownRenderer`
//! sub-components update in-place instead of being torn down and recreated on
//! every streaming chunk. Callers must ensure they render this component inside a
//! keyed `<For>` (or equivalent stable-identity mechanism) so the component
//! itself is never needlessly re-mounted.

use leptos::prelude::*;

use super::agent_thinking::AgentThinking;
use super::thinking::ThinkingState;
use crate::components::dashboard::MarkdownRenderer;

/// Shared body for an assistant chat message.
///
/// Renders:
/// 1. Agent thinking panel (when events exist or `force_show_thinking` is set)
/// 2. Markdown content (when non-empty)
/// 3. Optional action slot via `children`
#[component]
pub fn AgentMessageBody(
    /// Message ID — forwarded to `AgentThinking` for on-demand full-text fetch.
    #[prop(into)]
    message_id: Signal<String>,
    /// Assistant message content (markdown source).
    #[prop(into)]
    content: Signal<String>,
    /// Per-message thinking state (events, is_active, token_usage).
    #[prop(into)]
    thinking_state: Signal<ThinkingState>,
    /// Whether this message is actively streaming (drives `MarkdownRenderer`).
    #[prop(into)]
    is_streaming: Signal<bool>,
    /// Force-show the thinking panel even when no events have arrived yet.
    ///
    /// The main chat window sets this for the currently-active streaming message
    /// (the `AgentThinking` timer must start before the first event lands).
    /// The copilot leaves it `false` — the panel appears as soon as events arrive.
    #[prop(into, default = Signal::stored(false))]
    force_show_thinking: Signal<bool>,
    /// Forwarded to `MarkdownRenderer` — fires when a chart's info icon is clicked.
    /// Main chat passes this; copilot does not.
    #[prop(optional)]
    on_chart_info: Option<Callback<String>>,
    /// Optional action slot rendered below the message content.
    ///
    /// Copilot passes an "Apply to Dashboard" button here; main chat does not.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    // Derived signals from thinking_state — stable references so sub-components
    // receive individually-keyed reactive reads rather than the whole struct.
    let thinking_events = Signal::derive(move || thinking_state.get().events.clone());
    let thinking_is_active = Signal::derive(move || thinking_state.get().is_active);
    let thinking_token_usage = Signal::derive(move || thinking_state.get().token_usage.clone());

    let should_show_thinking =
        move || !thinking_events.get().is_empty() || force_show_thinking.get();

    view! {
        // Agent thinking panel
        <Show when=should_show_thinking>
            <AgentThinking
                thinking_events=thinking_events
                is_active=thinking_is_active
                token_usage=thinking_token_usage
                message_id=message_id
            />
        </Show>

        // Markdown content
        <Show when=move || !content.get().is_empty()>
            {if let Some(cb) = on_chart_info {
                view! {
                    <MarkdownRenderer
                        content=content
                        is_streaming=is_streaming
                        on_chart_info=cb
                        class="prose-kyomi-chat"
                    />
                }.into_any()
            } else {
                view! {
                    <MarkdownRenderer
                        content=content
                        is_streaming=is_streaming
                        class="prose-kyomi-chat"
                    />
                }.into_any()
            }}
        </Show>

        // Optional action slot (e.g. "Apply to Dashboard" in copilot)
        {children.map(|c| c())}
    }
}
