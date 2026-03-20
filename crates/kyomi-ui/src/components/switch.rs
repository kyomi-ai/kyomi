// SPDX-License-Identifier: AGPL-3.0-or-later

//! Switch component — matches `apps/frontend/src/components/ui/switch.jsx` exactly.
//!
//! A toggle switch (on/off) with a sliding thumb animation, used for
//! enable/disable toggles (e.g. Data Sources).
//!
//! React classes are copied verbatim from the `Switch` component.

use leptos::prelude::*;

/// Base classes for the switch track (the pill-shaped container).
/// From React: the `<button>` element classes.
const TRACK_BASE: &str = "peer inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border-2 border-transparent shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-50";

/// Track color when checked.
const TRACK_CHECKED: &str = "bg-primary";

/// Track color when unchecked.
const TRACK_UNCHECKED: &str = "bg-input";

/// Base classes for the thumb (the sliding circle).
/// From React: the `<span>` element classes.
const THUMB_BASE: &str = "pointer-events-none block h-4 w-4 rounded-full bg-background shadow-lg ring-0 transition-transform";

/// Thumb position when checked.
const THUMB_CHECKED: &str = "translate-x-4";

/// Thumb position when unchecked.
const THUMB_UNCHECKED: &str = "translate-x-0";

/// Switch component matching the React shadcn/ui Switch.
#[component]
pub fn Switch(
    /// Reactive checked state.
    checked: Signal<bool>,
    /// Called when the switch is toggled, with the new value.
    on_change: Callback<bool>,
    /// Whether the switch is disabled.
    #[prop(optional)]
    disabled: bool,
    /// Additional CSS classes for the track element.
    #[prop(optional, into)]
    class: String,
) -> impl IntoView {
    let track_classes = move || {
        let state_class = if checked.get() {
            TRACK_CHECKED
        } else {
            TRACK_UNCHECKED
        };
        format!("{} {} {}", TRACK_BASE, state_class, class)
    };

    let thumb_classes = move || {
        let pos_class = if checked.get() {
            THUMB_CHECKED
        } else {
            THUMB_UNCHECKED
        };
        format!("{} {}", THUMB_BASE, pos_class)
    };

    view! {
        <button
            type="button"
            role="switch"
            aria-checked=move || checked.get().to_string()
            attr:data-state=move || if checked.get() { "checked" } else { "unchecked" }
            disabled=disabled
            class=track_classes
            on:click=move |_| {
                if !disabled {
                    on_change.run(!checked.get());
                }
            }
        >
            <span class=thumb_classes />
        </button>
    }
}
