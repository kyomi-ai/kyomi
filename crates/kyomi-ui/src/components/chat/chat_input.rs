// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat Input Component
//!
//! Auto-expanding textarea with send/stop buttons, connection status indicator,
//! credits exhausted message, and optional "Skip AI" checkbox.
//!
//! Ported from the input area in `apps/frontend/src/components/ChatInterface.jsx`
//! (lines 564-628). CSS classes are copied verbatim from the React source.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::alert::{Alert, AlertDescription, AlertVariant};
use crate::components::checkbox::Checkbox;

/// Chat input component with auto-expanding textarea, send/stop buttons, and
/// connection status.
///
/// Matches the input area from React's `ChatInterface` component exactly
/// (layout, classes, behavior).
#[component]
pub fn ChatInput(
    /// Called with the message text when the user sends a message.
    #[prop(into)]
    on_send: Callback<String>,
    /// Called when the user clicks the stop button.
    #[prop(into)]
    on_cancel: Callback<()>,
    /// Whether the user can send a new message (idle state).
    #[prop(into)]
    can_send: Signal<bool>,
    /// Whether to show the stop button instead of the send button.
    #[prop(into)]
    show_stop_button: Signal<bool>,
    /// Whether the stop/cancel action is available (maps to React's `canCancel`).
    #[prop(into, default = Signal::derive(|| true))]
    can_cancel: Signal<bool>,
    /// WebSocket connection state string ("connected", "connecting", "reconnecting", "disconnected").
    #[prop(into)]
    connection_state: Signal<String>,
    /// Placeholder text for the textarea.
    #[prop(default = "Ask me anything about your data \u{2728}")]
    placeholder: &'static str,
    /// Whether to show the "Skip AI response" checkbox.
    #[prop(default = false)]
    show_skip_ai: bool,
    /// Signal for the skip AI checkbox state (required when `show_skip_ai` is true).
    #[prop(into, optional)]
    skip_ai: Option<RwSignal<bool>>,
    /// Whether the user's AI credits are exhausted.
    #[prop(default = false)]
    credits_exhausted: bool,
    /// Whether to render in inline mode (centered with greeting, no border/bg).
    /// When true, the outer wrapper is unstyled. When false (default), the wrapper
    /// has `bg-background` for the bottom-pinned layout.
    #[prop(default = false)]
    inline: bool,
    /// Maximum height for the auto-expanding textarea in pixels.
    #[prop(default = 200)]
    max_height: u32,
) -> impl IntoView {
    let (input_value, set_input_value) = signal(String::new());
    let textarea_ref = NodeRef::<leptos::html::Textarea>::new();

    // Derived: is the connection active?
    let is_connected = move || connection_state.get() == "connected";

    // Derived: can the send button be clicked?
    let send_enabled = move || can_send.get() && !input_value.get().trim().is_empty() && is_connected();

    // Send the current message
    let do_send = move || {
        let text = input_value.get();
        if text.trim().is_empty() || !can_send.get() {
            return;
        }

        on_send.run(text);
        set_input_value.set(String::new());

        // Reset textarea height and refocus
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(guard) = textarea_ref.try_read_untracked() {
                if let Some(el) = guard.as_ref() {
                    use wasm_bindgen::JsCast;
                    if let Some(textarea) = el.dyn_ref::<web_sys::HtmlTextAreaElement>() {
                        web_sys::HtmlElement::style(textarea).set_property("height", "auto").ok();
                    }
                }
            }
            // Refocus after clearing — use try_read_untracked so we don't panic if
            // ChatInput was unmounted (e.g., Show condition changed) before the
            // animation frame fires.
            request_animation_frame(move || {
                if let Some(guard) = textarea_ref.try_read_untracked() {
                    if let Some(el) = guard.as_ref() {
                        el.focus().ok();
                    }
                }
            });
        }
    };

    // Handle textarea input — auto-expand height
    let on_input = move |ev: web_sys::Event| {
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;
            if let Some(target) = ev.target() {
                if let Some(textarea) = target.dyn_ref::<web_sys::HtmlTextAreaElement>() {
                    set_input_value.set(textarea.value());
                    // Auto-expand: reset height then set to scrollHeight (capped)
                    web_sys::HtmlElement::style(textarea).set_property("height", "auto").ok();
                    let scroll_height = textarea.scroll_height();
                    let capped = scroll_height.min(max_height as i32);
                    web_sys::HtmlElement::style(textarea)
                        .set_property("height", &format!("{}px", capped))
                        .ok();
                }
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (max_height, &ev);
        }
    };

    // Handle keydown — Enter to send, Shift+Enter for newline
    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() {
            ev.prevent_default();
            do_send();
        }
    };

    // Auto-focus textarea on mount (100ms delay).
    // Uses try_read_untracked (safe, returns None when disposed) instead of .get()
    // (panics when disposed) to handle the case where ChatInput is unmounted before
    // the timeout fires — e.g., when a Show condition changes within 100ms of mount.
    #[cfg(target_arch = "wasm32")]
    {
        let textarea_ref = textarea_ref;
        Effect::new(move |_| {
            let textarea_ref = textarea_ref;
            gloo_timers::callback::Timeout::new(100, move || {
                if let Some(guard) = textarea_ref.try_read_untracked() {
                    if let Some(el) = guard.as_ref() {
                        el.focus().ok();
                    }
                }
            })
            .forget();
        });
    }

    // Connection status text
    let connection_status_text = move || {
        let state = connection_state.get();
        match state.as_str() {
            "connecting" => "Connecting...",
            "reconnecting" => "Reconnecting...",
            _ => "Disconnected",
        }
    };

    // Effective placeholder: different when credits are exhausted.
    // Matches React: Chat.jsx line 1668 — placeholder changes when creditsExhausted.
    let effective_placeholder = if credits_exhausted {
        "AI features disabled - upgrade to continue"
    } else {
        placeholder
    };

    view! {
        <div class=if inline { "" } else { "flex-shrink-0 p-4 bg-background" }>
            // Credits exhausted warning banner — shown above the (disabled) textarea.
            // Matches React: Chat.jsx lines 1651-1657 and 1721-1727.
            {if credits_exhausted {
                view! {
                    <Alert variant=AlertVariant::Warning class="mb-4">
                        <AlertDescription class="text-center">
                            "AI budget exhausted for this month. Upgrade for more capacity."
                        </AlertDescription>
                    </Alert>
                }.into_any()
            } else {
                ().into_any()
            }}

            // Connection status indicator (only when not credits-exhausted and not connected)
            <Show when=move || !credits_exhausted && !is_connected()>
                <div class="mb-2 text-sm text-muted-foreground flex items-center gap-2">
                    <div class="w-2 h-2 bg-warning-foreground rounded-full animate-pulse"></div>
                    <span>{connection_status_text}</span>
                </div>
            </Show>

            // Skip AI checkbox (optional, hidden when credits exhausted)
            <Show when=move || show_skip_ai && !credits_exhausted>
                {move || {
                    skip_ai.map(|skip_signal| {
                        let checked = Signal::derive(move || skip_signal.get());
                        view! {
                            <div class="mb-2 flex items-center gap-2">
                                <Checkbox
                                    checked=checked
                                    on_change=Callback::new(move |v: bool| skip_signal.set(v))
                                />
                                <label class="text-sm text-muted-foreground cursor-pointer">
                                    "Skip AI response"
                                </label>
                            </div>
                        }
                    })
                }}
            </Show>

            // Input area with textarea and send/stop button.
            // The textarea is always rendered — disabled when credits_exhausted.
            // Matches React: Chat.jsx lines 1658-1686 (textarea always present, just disabled).
            <div class="relative flex items-center">
                <textarea
                    node_ref=textarea_ref
                    prop:value=move || input_value.get()
                    on:input=on_input
                    on:keydown=on_keydown
                    placeholder=effective_placeholder
                    class="w-full pr-12 resize-none overflow-hidden border border-input focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring bg-card rounded-xl px-4 py-3 shadow-sm text-foreground"
                    style=format!("max-height: {}px", max_height)
                    rows="1"
                    disabled=move || credits_exhausted || !can_send.get()
                />
                // Send/Stop buttons — use CSS display instead of <Show> to avoid
                // DOM unmount/remount issues during URL transitions. Both buttons
                // stay in the DOM; only one is visible at a time.
                <button
                    on:click=move |_| do_send()
                    disabled=move || credits_exhausted || !send_enabled()
                    class="absolute right-2 top-2 bottom-2 my-auto p-2 bg-primary text-primary-foreground rounded-lg hover:opacity-90 disabled:bg-muted disabled:cursor-not-allowed transition-opacity flex items-center justify-center"
                    style=move || if show_stop_button.get() { "display: none" } else { "" }
                    aria-label="Send message"
                    title=move || {
                        if credits_exhausted {
                            "AI budget exhausted"
                        } else if !is_connected() {
                            "Waiting for connection..."
                        } else {
                            "Send message"
                        }
                    }
                >
                    <Icon icon=icondata_lu::LuSend width="16" height="16" />
                </button>
                // Stop button
                <button
                    on:click=move |_| on_cancel.run(())
                    disabled=move || !can_cancel.get()
                    class="absolute right-2 top-2 bottom-2 my-auto px-3 py-2 bg-destructive hover:bg-destructive/90 disabled:bg-muted disabled:cursor-not-allowed text-destructive-foreground rounded-lg transition-colors flex items-center gap-1.5"
                    style=move || if show_stop_button.get() { "" } else { "display: none" }
                    aria-label="Stop generating"
                    title=move || if can_cancel.get() { "Stop generating" } else { "Waiting for response..." }
                >
                    <Icon icon=icondata_lu::LuSquare width="16" height="16" />
                    <span class="text-sm font-medium">"Stop"</span>
                </button>
            </div>
        </div>
    }
}
