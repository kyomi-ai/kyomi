// SPDX-License-Identifier: AGPL-3.0-or-later

//! Styled select dropdown — matches shadcn/ui Select appearance.
//!
//! Uses a native `<select>` element styled to match the design system.
//! The focus state uses a subtle border change (not a thick ring) to
//! match the Radix Select behavior in the React frontend.

use leptos::prelude::*;

/// A styled native select dropdown.
///
/// Props:
/// - `value`: The currently selected value
/// - `options`: List of (value, label) pairs
/// - `on_change`: Called when the selection changes
///
/// # Example
/// ```ignore
/// <StyledSelect
///     value="chat"
///     options=vec![("chat", "Chat"), ("dashboards", "Dashboards")]
///     on_change=move |val| { /* handle change */ }
/// />
/// ```
#[component]
pub fn StyledSelect(
    #[prop(into)] value: String,
    options: Vec<(&'static str, &'static str)>,
    on_change: impl Fn(String) + 'static + Send + Sync,
) -> impl IntoView {
    view! {
        <select
            class="flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground transition-colors hover:border-ring focus:outline-none focus:border-ring appearance-none cursor-pointer"
            style="background-image: url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%2364748b' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='m6 9 6 6 6-6'/%3E%3C/svg%3E\"); background-repeat: no-repeat; background-position: right 0.75rem center; padding-right: 2.5rem;"
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
