// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard copilot sidebar — conversational AI for editing dashboards.
//!
//! Thin wrapper that hosts [`CopilotChat`] inside the shared [`RightPanel`]
//! (Editorial Margin pattern, see DESIGN.md). All chrome — header, close
//! button, resize handle, mobile overlay — lives in `RightPanel`.
//!
//! Dashboard-specific wiring kept here:
//! - Subscription to the `dashboard_update` WebSocket event so the AI can push
//!   markdown edits directly into the dashboard via `on_apply_content`.
//! - Dashboard markdown context passed to the chat engine.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::chat::CopilotChat;
use crate::components::RightPanel;

// ─── Constants ──────────────────────────────────────────────────────────────

const MIN_WIDTH: f64 = 320.0;
const MAX_WIDTH: f64 = 600.0;
const DEFAULT_WIDTH: f64 = 384.0;

// ─── Main component ─────────────────────────────────────────────────────────

/// Copilot sidebar for dashboard editing.
///
/// Hosts the shared [`CopilotChat`] inside a [`RightPanel`]. Chat session
/// lifecycle is handled by `CopilotChat` via its `active` prop (which follows
/// `open`): when the panel closes, the session is torn down.
#[component]
pub fn CopilotSidebar(
    /// Dashboard ID to associate the copilot session with.
    dashboard_id: String,
    /// Current dashboard content (markdown) — injected as context with messages.
    #[prop(into)]
    dashboard_content: Signal<String>,
    /// Whether the sidebar is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Callback to close the sidebar.
    on_close: Callback<()>,
    /// Callback when the AI pushes new dashboard content via the
    /// `dashboard_update` WS event.
    on_apply_content: Callback<String>,
) -> impl IntoView {
    let _dashboard_id = StoredValue::new(dashboard_id);
    let width = RwSignal::new(DEFAULT_WIDTH);

    // Custom WS event handler: the AI can apply changes directly to the
    // dashboard by emitting a `dashboard_update` event with a `content` field.
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
        >
            <CopilotChat
                context_type="dashboard_copilot"
                context_content=dashboard_content
                context_label="Dashboard Content"
                active=Signal::derive(move || open.get())
                placeholder="Ask about your dashboard..."
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
                empty_title="Ask me anything about your dashboard!"
                empty_description="I can help you improve charts, suggest changes, or make edits directly."
                custom_ws_events=vec!["dashboard_update".to_string()]
                on_custom_ws_event=on_custom_ws
            />
        </RightPanel>
    }
}
