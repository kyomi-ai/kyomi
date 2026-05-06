// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared search + sort bar — reusable across dashboards and knowledge pages.
//!
//! Extracted from `dashboards_list.rs`. Provides a `SearchInput` with 600ms
//! debounce and a sort dropdown with localStorage persistence.

use leptos::prelude::*;

use crate::components::{SearchInput, StyledSelect};

// ─────────────────────────────────────────────────────────────────────────────
// Search + Sort bar
// ─────────────────────────────────────────────────────────────────────────────

/// Combined search input (600ms debounce) and sort dropdown bar.
///
/// The sort preference is persisted to `localStorage` under the given
/// `storage_key` (e.g. `"kyomi_dashboards_sort"`).
#[component]
pub fn SearchSortBar(
    /// Callback fired after debounce with the search query (None = cleared).
    on_search: Callback<Option<String>>,
    /// Callback fired when sort changes.
    on_sort: Callback<String>,
    /// localStorage key for sort preference persistence.
    #[prop(into)]
    storage_key: String,
    /// Placeholder text for the search input.
    #[prop(default = "Search...")]
    placeholder: &'static str,
    /// Sort options: Vec of (value, label) pairs.
    #[prop(default = default_sort_options())]
    sort_options: Vec<(&'static str, &'static str)>,
) -> impl IntoView {
    // ── Search (with 600ms debounce) ────────────────────────────────────
    let (search_input, set_search_input) = signal(String::new());

    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;

        let timeout_handle: StoredValue<Option<SendWrapper<gloo_timers::callback::Timeout>>> =
            StoredValue::new(None);

        Effect::new(move |_| {
            let value = search_input.get();

            // Cancel any pending timeout
            timeout_handle.update_value(|h| {
                drop(h.take());
            });

            let handle = gloo_timers::callback::Timeout::new(600, move || {
                let q = if value.is_empty() { None } else { Some(value) };
                on_search.run(q);
            });

            timeout_handle.set_value(Some(SendWrapper::new(handle)));
        });

        on_cleanup(move || {
            timeout_handle.update_value(|h| {
                drop(h.take());
            });
        });
    }

    // On SSR, set query directly (no debounce needed)
    #[cfg(not(target_arch = "wasm32"))]
    {
        Effect::new(move |_| {
            let value = search_input.get();
            let q = if value.is_empty() { None } else { Some(value) };
            on_search.run(q);
        });
    }

    // ── Sort (persisted in localStorage) ────────────────────────────────
    let storage_key_for_init = storage_key.clone();
    let initial_sort = {
        #[cfg(target_arch = "wasm32")]
        {
            web_sys::window()
                .and_then(|w| w.local_storage().ok().flatten())
                .and_then(|s| s.get_item(&storage_key_for_init).ok().flatten())
                .unwrap_or_else(|| "recent".to_string())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = &storage_key_for_init;
            "recent".to_string()
        }
    };
    let (sort_signal, set_sort_signal) = signal(initial_sort);

    // Persist sort preference and notify parent on change
    #[cfg(target_arch = "wasm32")]
    {
        let storage_key_for_effect = storage_key.clone();
        Effect::new(move |_| {
            let val = sort_signal.get();
            if let Some(storage) = web_sys::window()
                .and_then(|w| w.local_storage().ok().flatten())
            {
                let _ = storage.set_item(&storage_key_for_effect, &val);
            }
            on_sort.run(val);
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        Effect::new(move |_| {
            let val = sort_signal.get();
            on_sort.run(val);
        });
    }

    view! {
        <div class="bg-background px-4 md:px-6 py-3 flex-shrink-0">
            <div class="flex items-center gap-3">
                <SearchInput
                    value=Signal::derive(move || search_input.get())
                    on_input=Callback::new(move |val: String| set_search_input.set(val))
                    placeholder=placeholder
                    class="flex-1"
                />
                <div class="w-40">
                    <StyledSelect
                        value=sort_signal.get_untracked()
                        options=sort_options
                        on_change=move |val: String| set_sort_signal.set(val)
                    />
                </div>
            </div>
        </div>
    }
}

/// Default sort options used by both dashboards and knowledge pages.
fn default_sort_options() -> Vec<(&'static str, &'static str)> {
    vec![
        ("recent", "Recently Updated"),
        ("popularity", "Most Popular"),
        ("created", "Newest First"),
    ]
}
