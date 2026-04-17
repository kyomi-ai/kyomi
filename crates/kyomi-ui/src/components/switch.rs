// SPDX-License-Identifier: AGPL-3.0-or-later

//! Switch component — matches `apps/frontend/src/components/ui/switch.jsx` exactly.
//!
//! A toggle switch (on/off) with a sliding thumb animation, used for
//! enable/disable toggles (e.g. Data Sources).
//!
//! React classes are copied verbatim from the `Switch` component.
//!
//! ## Optional built-in label
//!
//! Pass `label="…"` to render a clickable label text alongside the switch —
//! clicking the label text toggles the switch (matching native
//! `<input type="checkbox"> + <label>` UX). When no `label` is provided, the
//! component renders a bare `<button role="switch">` with zero additional DOM
//! so existing callsites keep their exact markup.

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

/// Default classes for the built-in label text (used when `label_class` is empty).
const LABEL_DEFAULT_CLASS: &str = "text-sm text-muted-foreground";

/// Wrapper classes when a built-in label is rendered — matches the typical
/// `flex items-center gap-2` sibling pairing that callsites previously wrote
/// around a bare `<Switch>` + `<Label>` pair.
const LABEL_WRAPPER_CLASS: &str = "inline-flex items-center gap-2 cursor-pointer";

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
    /// Optional label text rendered next to the switch. When `Some`, the
    /// component wraps the button in a `<label>` and adds a clickable
    /// `<span>` containing this text — clicking the text toggles the switch.
    /// When `None`, renders as a bare `<button>` with no wrapper (zero DOM
    /// change from the no-label shape).
    #[prop(optional, into)]
    label: Option<String>,
    /// CSS classes for the label text `<span>`. When empty, falls back to
    /// `"text-sm text-muted-foreground"`. When non-empty, the caller's value
    /// fully replaces the default (no merge) — mirroring how other Kyomi
    /// components treat optional class props.
    #[prop(optional, into)]
    label_class: String,
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

    // Clicking the label text must toggle the switch. The `<label>` wrapper
    // does NOT auto-forward clicks to a nested `<button role="switch">` (that
    // auto-forwarding only applies to native form inputs like
    // `<input type="checkbox">`), so we wire this explicitly on the text
    // `<span>`. We intentionally do NOT put `on:click` on the `<label>`
    // wrapper — doing so would double-fire when the button itself is clicked
    // (button click bubbles to the label).
    let toggle_from_label = move |_| {
        if !disabled {
            on_change.run(!checked.get());
        }
    };

    let resolved_label_class = if label_class.is_empty() {
        LABEL_DEFAULT_CLASS.to_string()
    } else {
        label_class
    };

    match label {
        // With label: wrap in `<label>` so a click anywhere in the wrapper
        // targets the underlying button (default label->button behavior),
        // and the text span gets its own toggle handler.
        Some(text) => view! {
            <label class=LABEL_WRAPPER_CLASS>
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
                <span class=resolved_label_class on:click=toggle_from_label>{text}</span>
            </label>
        }
        .into_any(),
        // Without label: zero-wrapper bare button, preserving the original
        // DOM shape for every existing callsite that hasn't migrated.
        None => view! {
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
        .into_any(),
    }
}
