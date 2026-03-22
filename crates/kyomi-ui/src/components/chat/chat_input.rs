// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat Input Component
//!
//! Auto-expanding textarea with send/stop buttons, connection status indicator,
//! credits exhausted message, and optional "Skip AI" checkbox.
//!
//! Ported from the input area in `apps/frontend/src/components/ChatInterface.jsx`
//! (lines 564-628). CSS classes are copied verbatim from the React source.

use leptos::prelude::*;

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
    #[prop(default = "Ask me anything...")]
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
            if let Some(el) = textarea_ref.get() {
                use wasm_bindgen::JsCast;
                if let Some(textarea) = el.dyn_ref::<web_sys::HtmlTextAreaElement>() {
                    web_sys::HtmlElement::style(textarea).set_property("height", "auto").ok();
                }
            }
            // Refocus after clearing
            request_animation_frame(move || {
                if let Some(el) = textarea_ref.get() {
                    el.focus().ok();
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

    // Auto-focus textarea on mount (100ms delay)
    #[cfg(target_arch = "wasm32")]
    {
        let textarea_ref = textarea_ref;
        Effect::new(move |_| {
            let textarea_ref = textarea_ref;
            gloo_timers::callback::Timeout::new(100, move || {
                if let Some(el) = textarea_ref.get() {
                    el.focus().ok();
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

    view! {
        <div class="border-t border-border flex-shrink-0 p-4 bg-card">
            {if credits_exhausted {
                view! {
                    <div class="text-center text-sm text-muted-foreground py-2">
                        "AI budget exhausted for this month. Upgrade for more capacity."
                    </div>
                }.into_any()
            } else {
                view! {
                    <>
                        // Connection status indicator
                        <Show when=move || !is_connected()>
                            <div class="mb-2 text-sm text-muted-foreground flex items-center gap-2">
                                <div class="w-2 h-2 bg-warning-foreground rounded-full animate-pulse"></div>
                                <span>{connection_status_text}</span>
                            </div>
                        </Show>

                        // Skip AI checkbox (optional)
                        <Show when=move || show_skip_ai>
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

                        // Input area with textarea and send/stop button
                        <div class="relative flex items-center">
                            <textarea
                                node_ref=textarea_ref
                                prop:value=move || input_value.get()
                                on:input=on_input
                                on:keydown=on_keydown
                                placeholder=placeholder
                                class="w-full pr-12 resize-none border border-input focus:outline-none focus:ring-2 focus:ring-ring focus:border-transparent bg-background rounded-xl px-4 py-3 shadow-sm min-h-[52px]"
                                style=format!("max-height: {}px", max_height)
                                rows="1"
                                disabled=move || !can_send.get()
                            />
                            <Show
                                when=move || show_stop_button.get()
                                fallback=move || {
                                    view! {
                                        // Send button
                                        <button
                                            on:click=move |_| do_send()
                                            disabled=move || !send_enabled()
                                            class="absolute right-2 top-2 bottom-2 my-auto p-2 bg-primary text-primary-foreground rounded-lg hover:opacity-90 disabled:bg-muted disabled:cursor-not-allowed transition-opacity flex items-center justify-center"
                                            aria-label="Send message"
                                            title=move || {
                                                if !is_connected() {
                                                    "Waiting for connection..."
                                                } else {
                                                    "Send message"
                                                }
                                            }
                                        >
                                            // Paper airplane SVG
                                            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 19l9 2-9-18-9 18 9-2zm0 0v-8" />
                                            </svg>
                                        </button>
                                    }
                                }
                            >
                                // Stop button
                                <button
                                    on:click=move |_| on_cancel.run(())
                                    disabled=move || !can_cancel.get()
                                    class="absolute right-2 top-2 bottom-2 my-auto px-3 py-2 bg-destructive hover:bg-destructive/90 disabled:bg-muted disabled:cursor-not-allowed text-white rounded-lg transition-colors flex items-center gap-1.5"
                                    aria-label="Stop generating"
                                    title=move || if can_cancel.get() { "Stop generating" } else { "Waiting for response..." }
                                >
                                    // X/stop SVG
                                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                                    </svg>
                                    <span class="text-sm font-medium">"Stop"</span>
                                </button>
                            </Show>
                        </div>
                    </>
                }.into_any()
            }}
        </div>
    }
}
