// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard copilot sidebar — conversational AI for editing dashboards.
//!
//! Matches every feature of the React `DashboardCopilotSidebar.jsx`:
//! - Desktop: resizable inline sidebar (320-600px) on the right, with drag handle
//! - Mobile: slide-in panel with backdrop overlay
//! - Session management: create on open, delete on close
//! - Chat interface with user/assistant messages
//! - "Apply to Dashboard" action for AI responses with suggested content
//!
//! ## Limitations
//!
//! Full copilot functionality (streaming AI responses) requires the WebSocket
//! client (`utils/websocket.rs`) to handle copilot-specific event types
//! (`chat_stream`, `chat_complete`). Until that is implemented, the copilot
//! stores messages via the server function but AI responses are not yet
//! delivered back to the UI.

use leptos::prelude::*;
#[cfg(feature = "hydrate")]
use wasm_bindgen::prelude::*;

use crate::components::Spinner;
use crate::server_fns::copilot::{
    create_copilot_session, delete_copilot_session, send_copilot_message,
};

use super::shared::use_is_mobile;

// ─── Constants ──────────────────────────────────────────────────────────────

const MIN_WIDTH: f64 = 320.0;
const MAX_WIDTH: f64 = 600.0;
const DEFAULT_WIDTH: f64 = 384.0;

// ─── SVG Icons ──────────────────────────────────────────────────────────────

/// Chat bubble icon (Heroicons outline) — matches `ChatBubbleLeftRightIcon`.
#[component]
fn ChatBubbleIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M20.25 8.511c.884.284 1.5 1.128 1.5 2.097v4.286c0 1.136-.847 2.1-1.98 2.193-.34.027-.68.052-1.02.072v3.091l-3-3c-1.354 0-2.694-.055-4.02-.163a2.115 2.115 0 0 1-.825-.242m9.345-8.334a2.126 2.126 0 0 0-.476-.095 48.64 48.64 0 0 0-8.048 0c-1.131.094-1.976 1.057-1.976 2.192v4.286c0 .837.46 1.58 1.155 1.951m9.345-8.334V6.637c0-1.621-1.152-3.026-2.76-3.235A48.455 48.455 0 0 0 11.25 3c-2.115 0-4.198.137-6.24.402-1.608.209-2.76 1.614-2.76 3.235v6.226c0 1.621 1.152 3.026 2.76 3.235.577.075 1.157.14 1.74.194V21l4.155-4.155" />
        </svg>
    }
}

/// X icon (Heroicons outline) — matches `XMarkIcon`.
#[component]
fn XIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor">
            <path stroke-linecap="round" stroke-linejoin="round" d="M6 18 18 6M6 6l12 12" />
        </svg>
    }
}

/// Paper airplane icon (Heroicons solid) — send button.
#[component]
fn SendIcon(#[prop(into, optional)] class: String) -> impl IntoView {
    view! {
        <svg class=class xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor">
            <path d="M3.478 2.404a.75.75 0 0 0-.926.941l2.432 7.905H13.5a.75.75 0 0 1 0 1.5H4.984l-2.432 7.905a.75.75 0 0 0 .926.94 60.519 60.519 0 0 0 18.445-8.986.75.75 0 0 0 0-1.218A60.517 60.517 0 0 0 3.478 2.404Z" />
        </svg>
    }
}

// ─── Chat message types ─────────────────────────────────────────────────────

/// A single chat message in the copilot conversation.
#[derive(Clone, Debug)]
struct ChatMessage {
    /// "user" or "assistant"
    role: String,
    /// The message content.
    content: String,
    /// Optional suggested content from the AI (for "Apply to Dashboard").
    suggested_content: Option<String>,
}

// ─── Main component ─────────────────────────────────────────────────────────

/// Copilot sidebar for dashboard editing.
///
/// Matches every feature of the React `DashboardCopilotSidebar`:
/// - Session lifecycle (create on open, delete on close)
/// - Chat interface with user/assistant messages
/// - Resizable desktop sidebar, mobile slide-in panel
/// - "Apply to Dashboard" action for AI suggestions
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
    /// Callback when the user clicks "Apply to Dashboard" on an AI response.
    on_apply_content: Callback<String>,
) -> impl IntoView {
    let dashboard_id = StoredValue::new(dashboard_id);
    let is_mobile = use_is_mobile();

    // ── Panel width (desktop resize) ────────────────────────────────────
    let (panel_width, set_panel_width) = signal(DEFAULT_WIDTH);
    let (is_resizing, set_is_resizing) = signal(false);

    // ── Session state ───────────────────────────────────────────────────
    let (session_id, set_session_id) = signal(Option::<String>::None);

    // ── Chat state ──────────────────────────────────────────────────────
    let (messages, set_messages) = signal(Vec::<ChatMessage>::new());
    let (is_loading, set_is_loading) = signal(false);
    let (input_value, set_input_value) = signal(String::new());
    let (error, set_error) = signal(Option::<String>::None);

    // Track whether the first message has been sent (for context prefix).
    let (has_sent_first, set_has_sent_first) = signal(false);

    // ── Create session when sidebar opens ───────────────────────────────
    Effect::new(move || {
        if open.get() {
            // Reset state for fresh session.
            set_messages.set(Vec::new());
            set_error.set(None);
            set_input_value.set(String::new());
            set_has_sent_first.set(false);

            let did = dashboard_id.get_value();
            leptos::task::spawn_local(async move {
                match create_copilot_session(did).await {
                    Ok(sid) => {
                        set_session_id.set(Some(sid));
                    }
                    Err(e) => {
                        set_error.set(Some(format!("Failed to start copilot: {e}")));
                    }
                }
            });
        }
    });

    // ── Cleanup session on component unmount ────────────────────────────
    on_cleanup(move || {
        let sid = session_id.get_untracked();
        if let Some(sid) = sid {
            leptos::task::spawn_local(async move {
                let _ = delete_copilot_session(sid).await;
            });
        }
    });

    // ── Handle close with cleanup ───────────────────────────────────────
    let handle_close = move || {
        let sid = session_id.get_untracked();
        set_session_id.set(None);

        if let Some(sid) = sid {
            leptos::task::spawn_local(async move {
                let _ = delete_copilot_session(sid).await;
            });
        }

        on_close.run(());
    };

    // ── Send message handler ────────────────────────────────────────────
    let handle_send = move || {
        let msg = input_value.get_untracked().trim().to_string();
        if msg.is_empty() {
            return;
        }

        let sid = match session_id.get_untracked() {
            Some(sid) => sid,
            None => return,
        };

        // Add user message to the list.
        set_messages.update(|msgs| {
            msgs.push(ChatMessage {
                role: "user".to_string(),
                content: msg.clone(),
                suggested_content: None,
            });
        });

        set_input_value.set(String::new());
        set_is_loading.set(true);
        set_error.set(None);

        // Build context: first message uses "[Dashboard Content]" prefix,
        // subsequent messages use "[Dashboard has been updated]" prefix.
        let is_first = !has_sent_first.get_untracked();
        let content = dashboard_content.get_untracked();
        let content_opt = if content.is_empty() {
            None
        } else if is_first {
            Some(format!("[Dashboard Content]\n{content}"))
        } else {
            Some(format!("[Dashboard has been updated]\n{content}"))
        };

        set_has_sent_first.set(true);

        leptos::task::spawn_local(async move {
            match send_copilot_message(sid, msg, content_opt).await {
                Ok(response) => {
                    // If we got actual content back, add it as an assistant message.
                    if !response.message.is_empty() {
                        set_messages.update(|msgs| {
                            msgs.push(ChatMessage {
                                role: "assistant".to_string(),
                                content: response.message,
                                suggested_content: response.suggested_content,
                            });
                        });
                    }
                    // NOTE: When `response.status == "processing"`, the actual AI
                    // response will arrive via WebSocket streaming events. Full
                    // streaming integration requires extending the WebSocket client
                    // to handle `chat_stream` / `chat_complete` event types.
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to send message: {e}")));
                }
            }
            set_is_loading.set(false);
        });
    };

    // ── Scroll to bottom on new messages ────────────────────────────────
    let message_list_ref = NodeRef::<leptos::html::Div>::new();

    Effect::new(move || {
        // Track message changes.
        let _ = messages.get();

        #[cfg(feature = "hydrate")]
        if let Some(el) = message_list_ref.get() {
            // Use requestAnimationFrame to scroll after DOM updates.
            let el: web_sys::HtmlElement = el.into();
            let scroll_height = el.scroll_height();
            el.set_scroll_top(scroll_height);
        }
    });

    // ── Resize drag handling (desktop) ──────────────────────────────────
    // Pattern copied from `history_panel.rs`.

    #[allow(unused_variables)]
    let handle_resize_start = move |ev: web_sys::MouseEvent| {
        ev.prevent_default();
        let start_x = ev.client_x() as f64;
        let start_w = panel_width.get_untracked();
        set_is_resizing.set(true);

        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::closure::Closure;

            let Some(window) = web_sys::window() else {
                return;
            };
            let Some(document) = window.document() else {
                return;
            };

            let move_handler =
                Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |ev: web_sys::MouseEvent| {
                    // Moving left increases width (panel is on the right).
                    let diff = start_x - ev.client_x() as f64;
                    let new_width = (start_w + diff).clamp(MIN_WIDTH, MAX_WIDTH);
                    set_panel_width.set(new_width);
                });

            let move_ref = move_handler
                .as_ref()
                .unchecked_ref::<js_sys::Function>()
                .clone();
            let document_clone = document.clone();
            let move_fn_clone = move_ref.clone();

            let up_handler = Closure::<dyn FnMut()>::once(move || {
                set_is_resizing.set(false);
                let _ = document_clone
                    .remove_event_listener_with_callback("mousemove", &move_fn_clone);
                if let Some(body) = document_clone.body() {
                    let _ = body.style().set_property("cursor", "");
                    let _ = body.style().set_property("user-select", "");
                }
            });

            let _ = document
                .add_event_listener_with_callback("mousemove", move_ref.unchecked_ref());
            let _ = document
                .add_event_listener_with_callback("mouseup", up_handler.as_ref().unchecked_ref());

            move_handler.forget();
            up_handler.forget();

            if let Some(body) = document.body() {
                let _ = body.style().set_property("cursor", "col-resize");
                let _ = body.style().set_property("user-select", "none");
            }
        }
    };

    // ── Panel content builder ───────────────────────────────────────────
    // Both mobile and desktop layouts share this inner content.
    let panel_content = move || {
        let current_error = error.get();
        let handle_send_clone = handle_send.clone();
        let handle_close_clone = handle_close.clone();

        view! {
            <div class="flex flex-col flex-1 min-w-0 h-full">
                // Header
                // React: `flex items-center justify-between px-4 py-3 border-b border-border bg-muted flex-shrink-0`
                <div class="flex items-center justify-between px-4 py-3 border-b border-border bg-muted flex-shrink-0">
                    <div class="flex items-center gap-2">
                        <ChatBubbleIcon class="w-5 h-5 text-primary" />
                        <span class="font-medium text-foreground">"Dashboard Copilot"</span>
                    </div>
                    <button
                        class="p-1 text-muted-foreground hover:text-foreground rounded-md hover:bg-accent"
                        aria-label="Close copilot"
                        on:click=move |_| handle_close_clone()
                    >
                        <XIcon class="w-5 h-5" />
                    </button>
                </div>

                // Error banner
                {current_error.map(|err| view! {
                    <div class="p-4 text-center">
                        <p class="text-error-foreground mb-2">{err}</p>
                    </div>
                })}

                // Message list
                // React: `flex-1 overflow-y-auto p-4 space-y-4`
                <div
                    class="flex-1 overflow-y-auto p-4 space-y-4"
                    node_ref=message_list_ref
                >
                    // Empty state
                    <Show when=move || messages.get().is_empty() && !is_loading.get()>
                        <div class="flex flex-col items-center justify-center h-full text-center px-4">
                            <ChatBubbleIcon class="w-12 h-12 text-muted-foreground/50 mb-3" />
                            <p class="text-muted-foreground text-sm font-medium">
                                "Ask me anything about your dashboard!"
                            </p>
                            <p class="text-muted-foreground/70 text-xs mt-1">
                                "I can help you improve charts, suggest changes, or make edits directly."
                            </p>
                        </div>
                    </Show>

                    // Messages
                    {move || messages.get().iter().enumerate().map(|(_idx, msg)| {
                        let is_user = msg.role == "user";
                        let content = msg.content.clone();
                        let suggested = msg.suggested_content.clone();
                        let has_suggested = suggested.is_some();
                        let full_message = msg.content.clone();

                        view! {
                            <div class=if is_user {
                                "flex justify-end"
                            } else {
                                "flex justify-start"
                            }>
                                <div class=if is_user {
                                    "bg-primary text-primary-foreground rounded-lg p-3 max-w-[85%]"
                                } else {
                                    "bg-muted rounded-lg p-3 max-w-[85%]"
                                }>
                                    <p class="text-sm whitespace-pre-wrap">{content}</p>

                                    // "Apply to Dashboard" button — only shown for assistant
                                    // messages that have suggested_content.
                                    {(!is_user && has_suggested).then(|| {
                                        let apply_content = suggested
                                            .unwrap_or_else(|| full_message.clone());
                                        view! {
                                            <button
                                                class="mt-2 text-xs text-primary hover:text-primary/80 font-medium"
                                                on:click=move |_| {
                                                    on_apply_content.run(apply_content.clone());
                                                }
                                            >
                                                "Apply to Dashboard"
                                            </button>
                                        }
                                    })}
                                </div>
                            </div>
                        }
                    }).collect_view()}

                    // Loading indicator
                    <Show when=move || is_loading.get()>
                        <div class="flex justify-start">
                            <div class="bg-muted rounded-lg p-3 flex items-center gap-2">
                                <Spinner class="text-muted-foreground" />
                                <span class="text-sm text-muted-foreground">"Thinking..."</span>
                            </div>
                        </div>
                    </Show>
                </div>

                // Input area
                // React: `border-t border-border p-4`
                <div class="border-t border-border p-4">
                    <div class="flex items-end gap-2">
                        <textarea
                            class="flex-1 resize-none rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring min-h-[40px] max-h-[120px]"
                            placeholder="Ask about your dashboard..."
                            rows="1"
                            prop:value=move || input_value.get()
                            on:input=move |ev| {
                                set_input_value.set(event_target_value(&ev));
                            }
                            on:keydown=move |ev| {
                                // Submit on Enter (without Shift for newline).
                                if ev.key() == "Enter" && !ev.shift_key() {
                                    ev.prevent_default();
                                    handle_send_clone();
                                }
                            }
                        />
                        <button
                            class="inline-flex items-center justify-center rounded-md h-10 w-10 bg-primary text-primary-foreground hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                            disabled=move || {
                                is_loading.get() || input_value.get().trim().is_empty()
                            }
                            on:click=move |_| handle_send()
                            aria-label="Send message"
                        >
                            <SendIcon class="w-4 h-4" />
                        </button>
                    </div>
                </div>
            </div>
        }
    };

    // ── Render ───────────────────────────────────────────────────────────

    view! {
        <Show when=move || open.get()>
            {move || {
                if is_mobile.get() {
                    // Mobile: Slide-in panel with backdrop
                    // React: `fixed top-32 left-0 right-0 bottom-0 bg-black/50 z-40`
                    // React: `fixed top-32 right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-card flex flex-col shadow-xl`
                    let handle_close_backdrop = handle_close.clone();
                    view! {
                        <div>
                            <div
                                class="fixed top-32 left-0 right-0 bottom-0 bg-black/50 z-40"
                                on:click=move |_| handle_close_backdrop()
                            />
                            <div class="fixed top-32 right-0 bottom-0 w-80 max-w-[85vw] z-50 bg-card flex flex-col shadow-xl">
                                {panel_content()}
                            </div>
                        </div>
                    }.into_any()
                } else {
                    // Desktop: Resizable inline sidebar
                    // React: `border-l border-border bg-card flex h-full overflow-hidden`
                    let width_style = move || format!("width: {}px", panel_width.get());

                    // Apply `select-none` during resize to prevent text selection.
                    let outer_class = move || {
                        if is_resizing.get() {
                            "border-l border-border bg-card flex h-full overflow-hidden select-none"
                        } else {
                            "border-l border-border bg-card flex h-full overflow-hidden"
                        }
                    };

                    view! {
                        <div
                            class=outer_class
                            style=width_style
                        >
                            // Resize Handle
                            // React: `flex items-center justify-center cursor-col-resize select-none px-1 -mr-2 relative z-10`
                            <div
                                class="flex items-center justify-center cursor-col-resize select-none px-1 -mr-2 relative z-10"
                                on:mousedown=handle_resize_start.clone()
                                aria-label="Drag to resize"
                            >
                                // React: `w-1 h-12 bg-border hover:bg-muted-foreground/50 rounded transition-colors`
                                <div class="w-1 h-12 bg-border hover:bg-muted-foreground/50 rounded transition-colors" />
                            </div>

                            {panel_content()}
                        </div>
                    }.into_any()
                }
            }}
        </Show>
    }
}
