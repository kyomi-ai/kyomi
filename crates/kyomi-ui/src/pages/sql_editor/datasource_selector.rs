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
use leptos_icons::Icon;

use crate::components::{DynSelect, Spinner};
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
                <Spinner class="text-muted-foreground" />
                <span>"Loading datasources..."</span>
            </div>
        </Show>

        <Show when=move || has_error.get()>
            <div class="px-3 py-2 text-sm text-error-foreground">
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
                    <Icon icon=icondata_lu::LuSettings width="12" height="12" />
                    <span>"Connect in Settings"</span>
                </a>
            </div>
        </Show>

        <Show when=move || !is_loading.get() && !has_error.get() && !accessible_datasources.get().is_empty()>
            <div class="w-[140px] sm:w-[240px]">
                <DynSelect
                    value=Signal::derive(move || selection.slug.get().unwrap_or_default())
                    options=Signal::derive(move || {
                        accessible_datasources
                            .get()
                            .iter()
                            .map(|ds| {
                                (
                                    ds.slug.clone(),
                                    format!("{} ({})", ds.name, ds.type_display_name),
                                )
                            })
                            .collect::<Vec<(String, String)>>()
                    })
                    on_change=move |new_slug: String| {
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
                />
            </div>
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
