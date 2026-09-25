// SPDX-License-Identifier: AGPL-3.0-or-later

//! Copilot sidebar — conversational AI for editing dashboards and knowledge documents.
//!
//! Thin wrapper that hosts [`CopilotChat`] inside the shared [`RightPanel`]
//! (Editorial Margin pattern, see DESIGN.md). All chrome — header, close
//! button, resize handle, mobile overlay — lives in `RightPanel`.
//!
//! Context-aware wiring kept here:
//! - `dashboard_id` is threaded through as `CopilotChat::document_id`, so
//!   the copilot's tool calls are scoped server-side to this document
//!   (`ToolContext::document_id`, KYO-536) — the copilot writes the
//!   document directly (`modify_dashboard`/`edit_knowledge_file`/
//!   `write_knowledge_file`) and the write is saved immediately, not a
//!   draft. There is no more bespoke `dashboard_update` WebSocket bridge:
//!   the editor reflects a copilot write the same way it reflects any
//!   other save.
//! - Context content (markdown) passed to the chat engine.
//! - `context_name` prop drives all UI copy (placeholder, empty state) and the
//!   backend context type so the agent receives the correct system prompt.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::RightPanel;
use crate::components::chat::{BeforeSendHook, CopilotChat};
use crate::components::chat::websocket_client::{ConnectionState, WebSocketContext};
use crate::server_fns::copilot::{list_dashboard_copilot_receipts, undo_copilot_change};

#[derive(Clone, PartialEq)]
struct SavedChangeReceipt {
    id: String,
    version: i64,
    summary: String,
    undone: bool,
}

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
    /// Dashboard/knowledge document ID this copilot session is scoped to.
    /// Threaded through to `ToolContext::document_id` so the copilot's
    /// write tools can never target a different document (KYO-536).
    #[prop(into)]
    dashboard_id: String,
    /// Current content (markdown) — injected as context with messages.
    #[prop(into)]
    dashboard_content: Signal<String>,
    /// Whether the sidebar is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Callback to close the sidebar.
    on_close: Callback<()>,
    /// Invoked immediately before every copilot message is sent, to commit
    /// the caller's editor buffer first — the copilot always edits the
    /// *saved* document (KYO-536). See `BeforeSendHook`.
    before_send: BeforeSendHook,
    /// Context name that drives UI copy and the backend agent prompt.
    /// Use `"dashboard"` (default) for dashboards, `"document"` for knowledge docs.
    #[prop(into, default = "dashboard".to_string())]
    context_name: String,
    /// Viewer-only context sampled with each message.
    #[prop(into, optional)]
    view_context: Option<Signal<String>>,
    /// Owner status comes from the server; failed access checks are read-only.
    #[prop(into, optional)]
    read_only: Option<Signal<bool>>,
    /// A historical preview is always question-only.
    #[prop(into, optional)]
    historical_preview: Option<Signal<bool>>,
    /// Access check error, if the viewer could not determine edit permission.
    #[prop(into, optional)]
    access_error: Option<Signal<Option<String>>>,
    #[prop(optional)] on_retry_access: Option<Callback<()>>,
    /// Refetch the document after an acknowledged write or a successful undo.
    #[prop(optional)]
    on_saved_change: Option<Callback<()>>,
    /// Opens the version history for the current document.
    #[prop(optional)]
    on_open_history: Option<Callback<()>>,
    /// Visible filters shown above the conversation in viewer mode.
    #[prop(into, optional)]
    filter_summary: Option<Signal<String>>,
    /// A failed dashboard reread blocks further edits until retry succeeds.
    #[prop(into, optional)]
    refresh_error: Option<Signal<Option<String>>>,
    #[prop(optional)]
    on_retry_refresh: Option<Callback<()>>,
    /// Bumped when the dashboard changes or a reread is requested.
    #[prop(into, optional)]
    receipt_refresh: Option<Signal<u32>>,
) -> impl IntoView {
    let width = RwSignal::new(DEFAULT_WIDTH);
    let receipts = RwSignal::new(Vec::<SavedChangeReceipt>::new());
    let undo_error = RwSignal::new(None::<String>);
    let receipt_load_error = RwSignal::new(None::<String>);
    let receipt_retry = RwSignal::new(0_u32);
    let websocket = use_context::<WebSocketContext>();
    Effect::new(move |was_connected: Option<bool>| {
        let connected = websocket.as_ref().is_some_and(|ws| ws.connection_state.get() == ConnectionState::Connected);
        if connected && was_connected == Some(false) && open.get() {
            receipt_retry.update(|generation| *generation += 1);
        }
        connected
    });
    let dashboard_id_for_list = dashboard_id.clone();
    let receipt_list = Resource::new(
        move || {
            (
                on_saved_change.is_some() && open.get() && read_only.is_some_and(|mode| !mode.get()),
                receipt_retry.get(),
                receipt_refresh.map(|signal| signal.get()).unwrap_or_default(),
            )
        },
        move |(should_load, _, _)| {
            let id = dashboard_id_for_list.clone();
            async move {
                if should_load { Some(list_dashboard_copilot_receipts(id).await) } else { None }
            }
        },
    );
    Effect::new(move |_| {
        let Some(result) = receipt_list.get() else { return };
        match result {
            Some(Ok(persisted)) => {
                receipt_load_error.set(None);
                let mut newly_found = false;
                receipts.update(|items| {
                    for receipt in persisted.into_iter().rev() {
                        if items.iter().any(|item| item.id == receipt.receipt_id) { continue; }
                        newly_found = true;
                        items.insert(0, SavedChangeReceipt {
                            id: receipt.receipt_id,
                            version: i64::from(receipt.version_number),
                            summary: receipt.change_summary,
                            undone: false,
                        });
                    }
                });
                if newly_found && let Some(callback) = on_saved_change {
                    callback.run(());
                }
            }
            Some(Err(error)) => receipt_load_error.set(Some(format!("Could not load saved changes: {error}"))),
            None => {}
        }
    });
    let undo_action = Action::new(|id: &String| {
        let id = id.clone();
        async move {
            let result = undo_copilot_change(id.clone()).await;
            (id, result)
        }
    });
    Effect::new(move |_| {
        let Some((id, result)) = undo_action.value().get() else { return };
        match result {
            Ok(()) => {
                receipts.update(|items| {
                    if let Some(item) = items.iter_mut().find(|item| item.id == id) {
                        item.undone = true;
                    }
                });
                if let Some(callback) = on_saved_change { callback.run(()); }
            }
            Err(error) => undo_error.set(Some(error.to_string())),
        }
    });
    let dashboard_id_for_receipt = dashboard_id.clone();
    let on_receipt = Callback::new(move |(event, data): (String, serde_json::Value)| {
        if on_saved_change.is_none()
            || event != "copilot_mutation_receipt"
            || data.get("dashboard_id").and_then(|value| value.as_str())
                != Some(dashboard_id_for_receipt.as_str())
        {
            return;
        }
        let Some(id) = data.get("receipt_id").and_then(|value| value.as_str()) else {
            return;
        };
        let Some(version) = data.get("version_number").and_then(|value| value.as_i64()) else {
            return;
        };
        let summary = data
            .get("change_summary")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("Dashboard updated")
            .to_string();
        receipts.try_update(|items| {
            if items.iter().any(|item| item.id == id) {
                return;
            }
            items.insert(
                0,
                SavedChangeReceipt {
                    id: id.to_string(),
                    version,
                    summary,
                    undone: false,
                },
            );
        });
        undo_error.try_set(None);
        if let Some(callback) = on_saved_change {
            callback.try_run(());
        }
    });

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
    let placeholder = if read_only.is_some() {
        Signal::derive(move || {
            if historical_preview.is_some_and(|preview| preview.try_get().unwrap_or(false)) {
                "Ask about this historical version...".to_string()
            } else if read_only.is_some_and(|mode| mode.try_get().unwrap_or(true)) {
                "Ask about this data…".to_string()
            } else {
                "Ask about this data or request a dashboard change…".to_string()
            }
        })
    } else {
        Signal::stored(placeholder.to_string())
    };

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
            {move || access_error.and_then(|error| error.get()).map(|message| view! {
                <div class="mx-4 mt-4 rounded-md border border-error/30 bg-error/5 px-3 py-2 text-sm text-error-foreground">
                    <span>{message}</span>
                    {on_retry_access.map(|retry| view! {
                        <button class="ml-2 underline" on:click=move |_| retry.run(())>"Retry"</button>
                    })}
                </div>
            })}
            {move || refresh_error.and_then(|error| error.get()).map(|message| view! {
                <div class="mx-4 mt-4 rounded-md border border-error/30 bg-error/5 px-3 py-2 text-sm text-error-foreground" role="alert">
                    <span>{message}</span>
                    {on_retry_refresh.map(|retry| view! {
                        <button class="ml-2 underline" on:click=move |_| retry.run(())>"Retry refresh"</button>
                    })}
                </div>
            })}
            {move || filter_summary.map(|summary| summary.get()).filter(|summary| !summary.is_empty()).map(|summary| view! {
                <div class="mx-4 mt-3 text-xs text-muted-foreground">{summary}</div>
            })}
            {move || {
                let is_preview = historical_preview.is_some_and(|preview| preview.get());
                let is_read_only = read_only.is_some_and(|read_only| read_only.get());
                (is_preview || is_read_only).then(|| view! {
                    <div class="mx-4 mt-4 rounded-md border border-border bg-muted/50 px-3 py-2 text-xs text-muted-foreground">
                        {if is_preview {
                            "Viewing a historical version. Copilot can answer questions but cannot save changes."
                        } else {
                            "You can ask questions about this document. Only its owner can save changes with Copilot."
                        }}
                    </div>
                })
            }}
            <Show when=move || on_saved_change.is_some() && !receipts.get().is_empty()>
                <div class="mx-4 mt-4 space-y-2" aria-live="polite">
                    <For
                        each=move || receipts.get()
                        key=|receipt| receipt.id.clone()
                        children=move |receipt| {
                            let receipt_id = receipt.id.clone();
                            let undo_id = receipt_id.clone();
                            let version = receipt.version;
                            let summary = receipt.summary.clone();
                            view! {
                                <div class="rounded-md border border-success/30 bg-success/5 px-3 py-2 text-xs">
                                    <div class="font-medium text-foreground">{move || {
                                        if receipts.get().iter().any(|item| item.id == receipt_id && item.undone) {
                                            format!("Undone change · restored version {version}")
                                        } else {
                                            format!("Saved change · Undo restores version {version}")
                                        }
                                    }}</div>
                                    <div class="mt-1 text-muted-foreground">{summary}</div>
                                    <div class="mt-2 flex items-center gap-3">
                                        <button class="font-medium text-primary hover:underline"
                                            disabled=move || historical_preview.is_some_and(|preview| preview.get()) || undo_action.pending().get() || receipts.get().iter().any(|item| item.id == undo_id && item.undone)
                                            on:click={
                                                let id = receipt.id.clone();
                                                move |_| {
                                                    if undo_action.pending().get_untracked() { return; }
                                                    undo_error.set(None);
                                                    undo_action.dispatch(id.clone());
                                                }
                                            }
                                        >"Undo"</button>
                                        {on_open_history.map(|open_history| view! {
                                            <button class="font-medium text-primary hover:underline" on:click=move |_| open_history.run(())>"History"</button>
                                        })}
                                    </div>
                                </div>
                            }
                        }
                    />
                    {move || undo_error.get().map(|error| view! {
                        <p class="text-xs text-error-foreground" role="alert">{error}</p>
                    })}
                </div>
            </Show>
            {move || receipt_load_error.get().map(|error| view! {
                <div class="mx-4 mt-3 text-xs text-error-foreground" role="alert">
                    {error}
                    <button class="ml-2 underline" on:click=move |_| receipt_retry.update(|count| *count += 1)>"Retry"</button>
                </div>
            })}
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
                document_id=dashboard_id.clone()
                view_context=view_context.unwrap_or_else(|| Signal::stored(String::new()))
                historical_preview=historical_preview.unwrap_or_else(|| Signal::stored(false))
                custom_ws_events=if on_saved_change.is_some() { vec!["copilot_mutation_receipt".to_string()] } else { vec![] }
                on_custom_ws_event=on_receipt
                before_send=before_send.clone()
            />
        </RightPanel>
    }
}
