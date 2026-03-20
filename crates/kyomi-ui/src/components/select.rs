// SPDX-License-Identifier: AGPL-3.0-or-later

//! Select component — styled native `<select>` matching the Radix SelectTrigger
//! from `apps/frontend/src/components/ui/select.jsx`.
//!
//! The React version uses Radix UI for a custom popover dropdown. For the Leptos
//! migration we use a native `<select>` styled with the same classes as
//! SelectTrigger. The native dropdown works well enough and avoids reimplementing
//! Radix's popover/positioning logic.
//!
//! SelectTrigger classes:
//! `flex h-9 w-full items-center justify-between whitespace-nowrap rounded-md
//!  border border-input bg-transparent px-3 py-2 text-sm text-foreground shadow-sm
//!  ring-offset-background focus:outline-none focus:ring-1 focus:ring-ring
//!  disabled:cursor-not-allowed disabled:opacity-50`

use leptos::prelude::*;

/// Classes matching SelectTrigger from the React Select component.
pub const SELECT_CLASS: &str = "flex h-9 w-full items-center justify-between whitespace-nowrap rounded-md border border-input bg-transparent px-3 py-2 text-sm text-foreground shadow-sm ring-offset-background focus:outline-none focus:ring-1 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-50 appearance-none cursor-pointer";

/// Inline chevron SVG as background image (matches the ChevronDown icon in React SelectTrigger).
pub const CHEVRON_STYLE: &str = "background-image: url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%2364748b' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='m6 9 6 6 6-6'/%3E%3C/svg%3E\"); background-repeat: no-repeat; background-position: right 0.75rem center; padding-right: 2.5rem;";

/// A styled native select dropdown.
#[component]
pub fn StyledSelect(
    #[prop(into)] value: String,
    options: Vec<(&'static str, &'static str)>,
    on_change: impl Fn(String) + 'static + Send + Sync,
) -> impl IntoView {
    view! {
        <select
            class=SELECT_CLASS
            style=CHEVRON_STYLE
            on:change=move |ev| on_change(event_target_value(&ev))
        >
            {options.iter().map(|(val, label)| {
                let selected = value == *val;
                view! {
                    <option value=*val selected=selected>{*label}</option>
                }
            }).collect_view()}
        </select>
    }
}
