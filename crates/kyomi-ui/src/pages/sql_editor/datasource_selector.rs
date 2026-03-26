// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource selector dropdown for the SQL Editor header.
//!
//! Mirrors `apps/frontend/src/components/DatasourceSelector.jsx`.
//!
//! - Lists all active, accessible datasources from `list_datasources()`
//! - Shows datasource name + type
//! - Persists selected datasource slug to localStorage
//! - Empty state: "No datasources available" with link to settings

use leptos::prelude::*;

use crate::server_fns::datasources::{list_datasources, DatasourceInfo};

/// localStorage key for persisting the last selected datasource slug.
const LOCALSTORAGE_KEY: &str = "kyomi:sqlEditor:lastDatasourceSlug";

/// Reactive state for the currently selected datasource.
///
/// Provided as a Leptos context at the SQL Editor page level so that
/// the execution handler, sidebar, and other components can all read
/// the current datasource without prop-drilling.
#[derive(Clone, Copy)]
pub struct DatasourceSelection {
    /// The slug of the currently selected datasource, or `None`.
    pub slug: RwSignal<Option<String>>,
    /// The type of the currently selected datasource (e.g. "bigquery").
    pub datasource_type: RwSignal<Option<String>>,
}

impl DatasourceSelection {
    /// Create a new `DatasourceSelection`, restoring from localStorage if available.
    pub fn provide() -> Self {
        let initial_slug = read_localstorage(LOCALSTORAGE_KEY);
        let selection = Self {
            slug: RwSignal::new(initial_slug),
            datasource_type: RwSignal::new(None),
        };
        provide_context(selection);
        selection
    }

    /// Retrieve from Leptos context.
    pub fn use_selection() -> Self {
        use_context::<Self>()
            .expect("DatasourceSelection not provided — call DatasourceSelection::provide() first")
    }

    /// Update the selection and persist to localStorage.
    pub fn select(&self, slug: Option<String>, ds_type: Option<String>) {
        self.slug.set(slug.clone());
        self.datasource_type.set(ds_type);
        write_localstorage(LOCALSTORAGE_KEY, slug.as_deref());
    }
}

/// Datasource selector dropdown component.
///
/// Fetches datasources on mount, auto-selects the persisted slug (or first
/// available), and calls back on change.
#[component]
pub fn DatasourceSelector() -> impl IntoView {
    let selection = DatasourceSelection::use_selection();

    // Fetch datasources as a resource.
    let datasources_resource = Resource::new(|| (), |_| async move {
        list_datasources().await
    });

    // Once datasources load, filter to accessible ones and auto-select.
    let accessible_datasources: Memo<Vec<DatasourceInfo>> = Memo::new(move |_| {
        let ds_list = match datasources_resource.get() {
            Some(Ok(list)) => list,
            _ => return Vec::new(),
        };

        // Filter: can_enable AND user_enabled (matches React logic).
        ds_list
            .into_iter()
            .filter(|ds| ds.can_enable && ds.user_enabled)
            .collect()
    });

    // Auto-select on first load.
    Effect::new(move |_| {
        let ds = accessible_datasources.get();
        if ds.is_empty() {
            return;
        }

        let current_slug = selection.slug.get_untracked();

        // Check if the current slug is still valid.
        if let Some(ref slug) = current_slug
            && let Some(found) = ds.iter().find(|d| &d.slug == slug)
        {
            // Current selection is valid — just ensure type is set.
            selection
                .datasource_type
                .set(Some(found.datasource_type.clone()));
            return;
        }

        // No valid selection — auto-select first.
        let first = &ds[0];
        selection.select(Some(first.slug.clone()), Some(first.datasource_type.clone()));
    });

    // Loading state.
    let is_loading = Memo::new(move |_| datasources_resource.get().is_none());

    // Error state.
    let has_error = Memo::new(move |_| {
        matches!(datasources_resource.get(), Some(Err(_)))
    });

    view! {
        <Show when=move || is_loading.get()>
            <div class="flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground">
                <svg
                    class="h-4 w-4 animate-spin text-muted-foreground"
                    fill="none"
                    viewBox="0 0 24 24"
                >
                    <circle
                        class="opacity-25"
                        cx="12"
                        cy="12"
                        r="10"
                        stroke="currentColor"
                        stroke-width="4"
                    ></circle>
                    <path
                        class="opacity-75"
                        fill="currentColor"
                        d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"
                    ></path>
                </svg>
                <span>"Loading datasources..."</span>
            </div>
        </Show>

        <Show when=move || has_error.get()>
            <div class="px-3 py-2 text-sm text-destructive">
                "Error loading datasources"
            </div>
        </Show>

        <Show when=move || !is_loading.get() && !has_error.get() && accessible_datasources.get().is_empty()>
            <div class="flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground">
                <span>"No datasources available."</span>
                <a
                    href="/settings"
                    class="inline-flex items-center gap-1 text-primary hover:underline"
                >
                    <svg
                        class="h-3 w-3"
                        fill="none"
                        stroke="currentColor"
                        viewBox="0 0 24 24"
                    >
                        <path
                            stroke-linecap="round"
                            stroke-linejoin="round"
                            stroke-width="2"
                            d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"
                        />
                        <path
                            stroke-linecap="round"
                            stroke-linejoin="round"
                            stroke-width="2"
                            d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"
                        />
                    </svg>
                    <span>"Connect in Settings"</span>
                </a>
            </div>
        </Show>

        <Show when=move || !is_loading.get() && !has_error.get() && !accessible_datasources.get().is_empty()>
            <select
                class="w-[140px] sm:w-[240px] h-9 rounded-md border border-input bg-background px-3 py-1 text-sm shadow-sm focus:outline-none focus:ring-1 focus:ring-ring"
                on:change=move |ev| {
                    let new_slug = event_target_value(&ev);
                    if new_slug.is_empty() {
                        selection.select(None, None);
                    } else {
                        let ds_type = accessible_datasources
                            .get_untracked()
                            .iter()
                            .find(|d| d.slug == new_slug)
                            .map(|d| d.datasource_type.clone());
                        selection.select(Some(new_slug), ds_type);
                    }
                }
                prop:value=move || selection.slug.get().unwrap_or_default()
            >
                <For
                    each=move || accessible_datasources.get()
                    key=|ds| ds.slug.clone()
                    let:ds
                >
                    {
                        let slug = ds.slug.clone();
                        let label = format!("{} ({})", ds.name, ds.type_display_name);
                        view! {
                            <option value={slug}>
                                {label}
                            </option>
                        }
                    }
                </For>
            </select>
        </Show>
    }
}

// ─── localStorage helpers ───────────────────────────────────────────────────

/// Read a string value from localStorage (WASM only).
fn read_localstorage(key: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.local_storage().ok())
            .flatten()
            .and_then(|s| s.get_item(key).ok())
            .flatten()
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        None
    }
}

/// Write a string value to localStorage (WASM only).
fn write_localstorage(key: &str, value: Option<&str>) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(storage) = web_sys::window()
            .and_then(|w| w.local_storage().ok())
            .flatten()
        {
            match value {
                Some(v) => {
                    let _ = storage.set_item(key, v);
                }
                None => {
                    let _ = storage.remove_item(key);
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (key, value);
    }
}
