// SPDX-License-Identifier: AGPL-3.0-or-later

//! Checkbox component — matches `apps/frontend/src/components/ui/checkbox.jsx`.
//!
//! A checkbox toggle with a checkmark SVG indicator, used for
//! terms acceptance, boolean preferences, select-all toggles, etc.
//!
//! Supports three visual states matching the React source:
//! - **Unchecked**: empty box
//! - **Checked**: box with check icon
//! - **Indeterminate**: box with minus icon (partial selection)
//!
//! React classes are copied verbatim from the `Checkbox` component.

use leptos::prelude::*;

/// Base classes for the checkbox button.
/// From React: the `<button>` element classes.
const CHECKBOX_BASE: &str = "peer inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm border border-input shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-50";

/// Classes applied when checked or indeterminate.
/// From React: `(isChecked || isIndeterminate) ? "bg-primary border-primary text-primary-foreground" : ...`
const CHECKBOX_ACTIVE: &str = "bg-primary border-primary text-primary-foreground";

/// Classes applied when unchecked (and not indeterminate).
const CHECKBOX_UNCHECKED: &str = "bg-background";

/// Checkbox component matching the React shadcn/ui Checkbox.
#[component]
pub fn Checkbox(
    /// Whether the checkbox is checked.
    #[prop(into)]
    checked: Signal<bool>,
    /// Called when the checked state changes, with the new value.
    on_change: Callback<bool>,
    /// Whether the checkbox shows an indeterminate (minus) icon.
    /// Only takes effect when `checked` is false, matching the React behavior:
    /// `const isIndeterminate = indeterminate && !isChecked;`
    #[prop(into, optional)]
    indeterminate: Option<Signal<bool>>,
    /// Additional CSS classes.
    #[prop(into, optional)]
    class: String,
    /// Whether the checkbox is disabled.
    #[prop(optional)]
    disabled: bool,
) -> impl IntoView {
    // Matches React: `const isIndeterminate = indeterminate && !isChecked;`
    let is_indeterminate = move || {
        indeterminate
            .map(|sig| sig.get() && !checked.get())
            .unwrap_or(false)
    };

    let button_classes = move || {
        let is_checked = checked.get();
        let state_class = if is_checked || is_indeterminate() {
            CHECKBOX_ACTIVE
        } else {
            CHECKBOX_UNCHECKED
        };
        format!("{} {} {}", CHECKBOX_BASE, state_class, class)
    };

    // Matches React: `aria-checked={isIndeterminate ? "mixed" : isChecked}`
    let aria_checked = move || {
        if is_indeterminate() {
            "mixed".to_string()
        } else {
            checked.get().to_string()
        }
    };

    // Matches React: `data-state={isIndeterminate ? "indeterminate" : isChecked ? "checked" : "unchecked"}`
    let data_state = move || {
        if is_indeterminate() {
            "indeterminate"
        } else if checked.get() {
            "checked"
        } else {
            "unchecked"
        }
    };

    view! {
        <button
            type="button"
            role="checkbox"
            aria-checked=aria_checked
            attr:data-state=data_state
            class=button_classes
            disabled=disabled
            on:click=move |_| {
                if !disabled {
                    on_change.run(!checked.get());
                }
            }
        >
            // Check icon — shown when checked
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
            // Minus icon — shown when indeterminate (and not checked)
            // Matches React: `{isIndeterminate && <Minus className="h-3 w-3" strokeWidth={3} />}`
            <Show when=move || is_indeterminate()>
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
                    <path d="M5 12h14" />
                </svg>
            </Show>
        </button>
    }
}
