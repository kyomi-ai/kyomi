// SPDX-License-Identifier: AGPL-3.0-or-later

//! SearchInput component — reusable search bar with icon and clear button.
//!
//! Used on list pages (chats, dashboards) for consistent search UX.

use leptos::prelude::*;
use leptos_icons::Icon;

/// Reusable search input with leading search icon and trailing clear button.
///
/// Matches the pattern used in chats and dashboards list pages.
#[component]
pub fn SearchInput(
    /// Current input value (two-way binding via `value` + `on_input`).
    #[prop(into)]
    value: Signal<String>,
    /// Called on every input change with the new value.
    on_input: Callback<String>,
    /// Placeholder text. Default: "Search..."
    #[prop(default = "Search...")]
    placeholder: &'static str,
    /// Optional: show a spinner instead of the search icon (e.g., while searching).
    #[prop(optional, into)]
    searching: MaybeProp<bool>,
    /// Additional CSS classes on the outer container (for width control).
    #[prop(into, optional)]
    class: String,
) -> impl IntoView {
    let is_searching = move || searching.get().unwrap_or(false);

    view! {
        <div class=format!("relative {class}")>
            // Leading icon — spinner when searching, search icon otherwise
            <Show
                when=is_searching
                fallback=|| view! {
                    <span class="absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground">
                        <Icon icon=icondata_lu::LuSearch width="16" height="16" />
                    </span>
                }
            >
                <span class="absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground">
                    <crate::components::Spinner />
                </span>
            </Show>

            <input
                type="text"
                placeholder=placeholder
                class="w-full pl-10 pr-10 py-2 text-sm border border-input rounded-lg bg-card text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring transition-colors"
                prop:value=move || value.get()
                on:input=move |ev| {
                    on_input.run(event_target_value(&ev));
                }
            />

            // Clear button
            <Show when=move || !value.get().is_empty() && !is_searching()>
                <button
                    class="absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground transition-colors hover:text-foreground"
                    aria-label="Clear search"
                    on:click=move |_| on_input.run(String::new())
                >
                    <Icon icon=icondata_lu::LuX width="16" height="16" />
                </button>
            </Show>
        </div>
    }
}
