// SPDX-License-Identifier: AGPL-3.0-or-later

//! Copilot sidebar — conversational AI for editing dashboards and knowledge documents.
//!
//! Thin wrapper that hosts [`CopilotChat`] inside the shared [`RightPanel`]
//! (Editorial Margin pattern, see DESIGN.md). All chrome — header, close
//! button, resize handle, mobile overlay — lives in `RightPanel`.
//!
//! Context-aware wiring kept here:
//! - Subscription to the `dashboard_update` WebSocket event so the AI can push
//!   markdown edits directly into the document via `on_apply_content`.
//! - Context content (markdown) passed to the chat engine.
//! - `context_name` prop drives all UI copy (placeholder, empty state) and the
//!   backend context type so the agent receives the correct system prompt.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::chat::CopilotChat;
use crate::components::RightPanel;

// ─── Constants ──────────────────────────────────────────────────────────────

const MIN_WIDTH: f64 = 320.0;
const MAX_WIDTH: f64 = 600.0;
const DEFAULT_WIDTH: f64 = 384.0;

// ─── Main component ─────────────────────────────────────────────────────────

/// Copilot sidebar for dashboard or knowledge document editing.
///
/// Hosts the shared [`CopilotChat`] inside a [`RightPanel`]. Chat session
/// lifecycle is handled by `CopilotChat` via its `active` prop (which follows
/// `open`): when the panel closes, the session is torn down.
///
/// The `context_name` prop (default: `"dashboard"`) controls all context-
/// sensitive copy and the backend agent system prompt:
/// - `"dashboard"` → dashboard-centric placeholder + `dashboard_copilot` session type
/// - `"document"` → document-centric placeholder + `knowledge_copilot` session type
#[component]
pub fn CopilotSidebar(
    /// Dashboard ID to associate the copilot session with.
    dashboard_id: String,
    /// Current content (markdown) — injected as context with messages.
    #[prop(into)]
    dashboard_content: Signal<String>,
    /// Whether the sidebar is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Callback to close the sidebar.
    on_close: Callback<()>,
    /// Callback when the AI pushes new content via the `dashboard_update` WS event.
    on_apply_content: Callback<String>,
    /// Context name that drives UI copy and the backend agent prompt.
    /// Use `"dashboard"` (default) for dashboards, `"document"` for knowledge docs.
    #[prop(into, default = "dashboard".to_string())]
    context_name: String,
) -> impl IntoView {
    let _dashboard_id = StoredValue::new(dashboard_id);
    let width = RwSignal::new(DEFAULT_WIDTH);

    // Derive context-sensitive strings from `context_name` at construction time.
    // These are static per mount — `context_name` is not expected to change after render.
    let (context_type, context_label, placeholder, empty_title, empty_description) =
        match context_name.as_str() {
            "document" => (
                "knowledge_copilot",
                "Document Content",
                "Ask about your document...",
                "Ask me anything about your document!",
                "I can help you improve content, add context, or make edits directly.",
            ),
            // "dashboard" and any unrecognised value
            _ => (
                "dashboard_copilot",
                "Dashboard Content",
                "Ask about your dashboard...",
                "Ask me anything about your dashboard!",
                "I can help you improve charts, suggest changes, or make edits directly.",
            ),
        };

    // Custom WS event handler: the AI can apply changes directly to the
    // document by emitting a `dashboard_update` event with a `content` field.
    let on_custom_ws = Callback::new(move |(_event_name, data): (String, serde_json::Value)| {
        if let Some(content) = data.get("content").and_then(|v| v.as_str()) {
            on_apply_content.run(content.to_string());
        }
    });

    view! {
        <RightPanel
            open=open
            on_close=on_close
            width=width
            min_width=MIN_WIDTH
            max_width=MAX_WIDTH
            title="Copilot".to_string()
            close_label="Close copilot".to_string()
            flex_body=true
        >
            <CopilotChat
                context_type=context_type
                context_content=dashboard_content
                context_label=context_label
                active=Signal::derive(move || open.get())
                placeholder=placeholder
                empty_icon=std::sync::Arc::new(|| {
                    view! {
                        <Icon
                            icon=phosphor_leptos::SPARKLE
                            weight=IconWeight::Duotone
                            size="64px"
                        />
                    }
                    .into_any()
                })
                empty_title=empty_title
                empty_description=empty_description
                custom_ws_events=vec!["dashboard_update".to_string()]
                on_custom_ws_event=on_custom_ws
            />
        </RightPanel>
    }
}
