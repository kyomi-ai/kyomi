// SPDX-License-Identifier: AGPL-3.0-or-later

//! InboxPage — displays alert history for all watches.
//!
//! A focused view that renders `AlertsHistory` directly, without the
//! Watches/Alerts tab switching. Supports the `?alert=N` query param
//! for deep-linking to a specific alert.

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_query_map};

use crate::components::watches::AlertsHistory;

/// InboxPage — shows the full alerts history feed.
///
/// Mirrors the Alerts tab from WatchesPage but without the surrounding
/// tab chrome — Inbox is a first-class page in the sidebar.
#[component]
pub fn InboxPage() -> impl IntoView {
    let navigate = use_navigate();
    let query = use_query_map();

    // Deep-link alert ID from ?alert=N query parameter.
    // Same pattern as WatchesPage.
    let expanded_alert_id = Memo::new(move |_| {
        query.get().get("alert").and_then(|v| v.parse::<i32>().ok())
    });

    // Navigate to the chat session when "Continue in chat" is clicked.
    // Same callback pattern as WatchesPage.
    let on_continue_chat = {
        let navigate = navigate.clone();
        Callback::new(move |session_id: String| {
            navigate(&format!("/chat/{session_id}"), Default::default());
        })
    };

    view! {
        <div class="h-full flex flex-col bg-background">
            // Row 1: Page header — List Page Pattern from DESIGN.md
            <div class="page-header h-16 px-4 md:px-6 flex-shrink-0 flex items-center justify-between">
                <h1 class="text-3xl font-display text-foreground">"Inbox"</h1>
            </div>

            // Content area — flex-1, overflow-auto, matches watches alerts tab
            <div class="flex-1 overflow-auto p-4 md:p-6">
                <AlertsHistory
                    on_continue_chat=on_continue_chat
                    expanded_alert_id=expanded_alert_id.get()
                />
            </div>
        </div>
    }
}
