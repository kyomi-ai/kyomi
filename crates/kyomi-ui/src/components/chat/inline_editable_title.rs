// SPDX-License-Identifier: AGPL-3.0-or-later

//! Inline Editable Title Component
//!
//! Click-to-edit title with save/cancel support. Shows a pencil icon on hover
//! in display mode, and save/cancel buttons in edit mode.
//!
//! Ported from `apps/frontend/src/components/InlineEditableTitle.jsx`.
//! CSS classes are copied verbatim from the React source.

use leptos::prelude::*;
use phosphor_leptos::Icon;
use crate::components::button::{Button, ButtonSize, ButtonVariant};

/// Inline editable title component.
///
/// Displays a title as text that can be clicked to enter edit mode.
/// In edit mode, shows an input with save/cancel buttons.
///
/// Matches React's `InlineEditableTitle` component exactly.
#[component]
pub fn InlineEditableTitle(
    /// The current title value (reactive).
    #[prop(into)]
    value: Signal<String>,
    /// Called with the new title when the user saves.
    #[prop(into)]
    on_save: Callback<String>,
    /// Placeholder text shown when the value is empty.
    #[prop(default = "Untitled")]
    placeholder: &'static str,
) -> impl IntoView {
    let (is_editing, set_is_editing) = signal(false);
    let (edit_value, set_edit_value) = signal(String::new());
    let input_ref = NodeRef::<leptos::html::Input>::new();
    // Guard flag: prevents blur handler from saving after Escape cancels.
    // When Escape is pressed, `handle_cancel` sets this to true and hides
    // the input, which triggers blur. The blur handler checks this flag
    // and skips saving if it was a cancel-initiated blur.
    let (cancelled, set_cancelled) = signal(false);

    // When entering edit mode, copy current value to edit buffer
    let start_editing = move |_| {
        set_edit_value.set(value.get());
        set_is_editing.set(true);
    };

    // Focus and select input when entering edit mode
    #[cfg(target_arch = "wasm32")]
    {
        let input_ref = input_ref;
        Effect::new(move |_| {
            if is_editing.get() {
                let input_ref = input_ref;
                // Small delay to ensure DOM is updated
                gloo_timers::callback::Timeout::new(10, move || {
                    if let Some(guard) = input_ref.try_read_untracked() {
                        if let Some(el) = guard.as_ref() {
                            el.focus().ok();
                            el.select();
                        }
                    }
                })
                .forget();
            }
        });
    }

    // Save handler — only saves if value changed and non-empty
    let handle_save = move || {
        let trimmed = edit_value.get().trim().to_string();
        if !trimmed.is_empty() && trimmed != value.get_untracked() {
            on_save.run(trimmed);
        }
        set_is_editing.set(false);
    };

    // Cancel handler — reverts to original value
    let handle_cancel = move || {
        set_cancelled.set(true);
        set_edit_value.set(value.get_untracked());
        set_is_editing.set(false);
    };

    // Keydown handler — Enter to save, Escape to cancel
    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        match ev.key().as_str() {
            "Enter" => {
                ev.prevent_default();
                handle_save();
            }
            "Escape" => {
                ev.prevent_default();
                handle_cancel();
            }
            _ => {}
        }
    };

    view! {
        <Show
            when=move || is_editing.get()
            fallback=move || {
                let placeholder = placeholder;
                view! {
                    // Display mode — clickable title with pencil icon on hover
                    <button
                        on:click=start_editing
                        class="flex items-center gap-2 group hover:bg-secondary/50 rounded-md px-2 py-1 transition-colors min-w-0"
                    >
                        <span class="text-base font-semibold text-foreground truncate">
                            {move || {
                                let v = value.get();
                                if v.is_empty() { placeholder.to_string() } else { v }
                            }}
                        </span>
                        <Icon icon=phosphor_leptos::PENCIL_SIMPLE attr:class="h-4 w-4 text-muted-foreground opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0" size="16px" />
                    </button>
                }
            }
        >
            // Edit mode — input with save/cancel buttons
            <div class="flex items-center gap-2">
                <input
                    node_ref=input_ref
                    type="text"
                    prop:value=move || edit_value.get()
                    on:input=move |ev| {
                        #[cfg(target_arch = "wasm32")]
                        {
                            use wasm_bindgen::JsCast;
                            if let Some(target) = ev.target() {
                                if let Some(input) = target.dyn_ref::<web_sys::HtmlInputElement>() {
                                    set_edit_value.set(input.value());
                                }
                            }
                        }
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            let _ = ev;
                        }
                    }
                    on:keydown=on_keydown
                    on:blur=move |_| {
                        // Only save on blur if still in editing mode (not already saved/cancelled)
                        if is_editing.get_untracked() && !cancelled.get_untracked() {
                            handle_save();
                        }
                        set_cancelled.set(false);
                    }
                    placeholder=placeholder
                    class="text-base font-semibold px-2 py-1 border-0 border-b-2 border-b-transparent bg-transparent focus:outline-none focus:border-b-ring text-foreground min-w-0 flex-1 transition-colors"
                />
                // Save button (check icon)
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Icon
                    attr:class="h-7 w-7 flex-shrink-0"
                    on:click=move |_| handle_save()
                >
                    <Icon icon=phosphor_leptos::CHECK size="16px" />
                </Button>
                // Cancel button (X icon)
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Icon
                    attr:class="h-7 w-7 flex-shrink-0"
                    on:click=move |_| handle_cancel()
                >
                    <Icon icon=phosphor_leptos::X size="16px" />
                </Button>
            </div>
        </Show>
    }
}
