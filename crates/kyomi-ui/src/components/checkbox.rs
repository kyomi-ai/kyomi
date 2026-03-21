// SPDX-License-Identifier: AGPL-3.0-or-later

//! Checkbox component — matches `apps/frontend/src/components/ui/checkbox.jsx`.
//!
//! A checkbox toggle with a checkmark SVG indicator, used for
//! terms acceptance, boolean preferences, etc.
//!
//! Note: The React source also supports an `indeterminate` state with a
//! `<Minus>` icon. This is intentionally omitted as it is not needed for
//! current use cases (terms acceptance).
//!
//! React classes are copied verbatim from the `Checkbox` component.

use leptos::prelude::*;

/// Base classes for the checkbox button.
/// From React: the `<button>` element classes.
const CHECKBOX_BASE: &str = "peer inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm border border-input shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-50";

/// Classes applied when checked.
const CHECKBOX_CHECKED: &str = "bg-primary border-primary text-primary-foreground";

/// Classes applied when unchecked.
const CHECKBOX_UNCHECKED: &str = "bg-background";

/// Checkbox component matching the React shadcn/ui Checkbox.
#[component]
pub fn Checkbox(
    /// Whether the checkbox is checked.
    #[prop(into)]
    checked: Signal<bool>,
    /// Called when the checked state changes, with the new value.
    on_change: Callback<bool>,
    /// Additional CSS classes.
    #[prop(into, optional)]
    class: String,
    /// Whether the checkbox is disabled.
    #[prop(optional)]
    disabled: bool,
) -> impl IntoView {
    let button_classes = move || {
        let state_class = if checked.get() {
            CHECKBOX_CHECKED
        } else {
            CHECKBOX_UNCHECKED
        };
        format!("{} {} {}", CHECKBOX_BASE, state_class, class)
    };

    view! {
        <button
            type="button"
            role="checkbox"
            aria-checked=move || checked.get().to_string()
            attr:data-state=move || if checked.get() { "checked" } else { "unchecked" }
            class=button_classes
            disabled=disabled
            on:click=move |_| {
                if !disabled {
                    on_change.run(!checked.get());
                }
            }
        >
            <Show when=move || checked.get()>
                <svg
                    xmlns="http://www.w3.org/2000/svg"
                    width="24"
                    height="24"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="3"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    class="h-3 w-3"
                >
                    <path d="M20 6 9 17l-5-5" />
                </svg>
            </Show>
        </button>
    }
}
