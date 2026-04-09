// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard list page — matches `apps/frontend/src/pages/DashboardsList.jsx`.
//!
//! Displays a searchable, sortable grid of dashboard cards with collection
//! badges, hamburger menus, and empty states. Uses server functions for data
//! fetching and mutations (create, delete).
//!
//! The collections sidebar itself is implemented separately (Task 8). This
//! module exposes `collections_open` and `active_collection_id` signals as
//! integration points.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Button, ButtonLink, ButtonSize, ButtonVariant, Card, CardContent, CardFooter,
    CardHeader, CardTitle, ConfirmDialog, EmptyState, SearchInput, Skeleton,
    Spinner, StyledSelect, ToggleButton,
};
use super::collections_sidebar::CollectionsSidebar;
use crate::server_fns::collections::{
    list_collections, remove_dashboard_from_collection, CollectionItem,
};
use crate::server_fns::dashboards::{
    create_dashboard, delete_dashboard, list_dashboards, DashboardListItem,
};

// ─────────────────────────────────────────────────────────────────────────────
// Relative time helper
// ─────────────────────────────────────────────────────────────────────────────

/// Converts an RFC 3339 timestamp string into a compact relative time string.
///
/// Matches the React frontend's display format:
/// - "just now" (< 60s)
/// - "5m ago" (< 60m)
/// - "2h ago" (< 24h)
/// - "3d ago" (< 30d)
/// - "Mar 15" (>= 30d, same year)
/// - "Mar 15, 2025" (different year)
fn format_relative_time(rfc3339: &str) -> String {
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(rfc3339) else {
        return rfc3339.to_string();
    };

    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(parsed);

    let seconds = duration.num_seconds();
    if seconds < 60 {
        return "just now".to_string();
    }

    let minutes = duration.num_minutes();
    if minutes < 60 {
        return format!("{minutes}m ago");
    }

    let hours = duration.num_hours();
    if hours < 24 {
        return format!("{hours}h ago");
    }

    let days = duration.num_days();
    if days < 30 {
        return format!("{days}d ago");
    }

    // For older dates, show "Mar 15" or "Mar 15, 2025"
    let parsed_utc = parsed.with_timezone(&chrono::Utc);
    if parsed_utc.format("%Y").to_string() == now.format("%Y").to_string() {
        parsed_utc.format("%b %-d").to_string()
    } else {
        parsed_utc.format("%b %-d, %Y").to_string()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main page component
// ─────────────────────────────────────────────────────────────────────────────

/// Dashboard list page with search, sort, create, delete, and collection
/// integration.
#[component]
pub fn DashboardsListPage() -> impl IntoView {
    // ── Collection sidebar integration points (Task 8) ──────────────────
    let (collections_open, set_collections_open) = signal(false);
    let (active_collection_id, set_active_collection_id) = signal(Option::<String>::None);

    // ── Search (with 600ms debounce) ────────────────────────────────────
    let (search_input, set_search_input) = signal(String::new());
    let (query_signal, set_query_signal) = signal(Option::<String>::None);

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
                set_query_signal.set(q);
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
            set_query_signal.set(q);
        });
    }

    // ── Sort (persisted in localStorage) ───────────────────────────────
    let initial_sort = {
        #[cfg(target_arch = "wasm32")]
        {
            web_sys::window()
                .and_then(|w| w.local_storage().ok().flatten())
                .and_then(|s| s.get_item("kyomi_dashboards_sort").ok().flatten())
                .unwrap_or_else(|| "recent".to_string())
        }
        #[cfg(not(target_arch = "wasm32"))]
        { "recent".to_string() }
    };
    let (sort_signal, set_sort_signal) = signal(initial_sort);

    // Persist sort preference on change
    #[cfg(target_arch = "wasm32")]
    {
        Effect::new(move |_| {
            let val = sort_signal.get();
            if let Some(storage) = web_sys::window()
                .and_then(|w| w.local_storage().ok().flatten())
            {
                let _ = storage.set_item("kyomi_dashboards_sort", &val);
            }
        });
    }

    // ── Data fetching ────────────────────────────────────────────────────
    let dashboards_resource = Resource::new(
        move || (query_signal.get(), sort_signal.get()),
        move |(query, sort)| list_dashboards(query, Some(sort), None),
    );

    // Fetch collections for badge display and filtering
    let collections_resource = Resource::new(
        || (),
        |_| list_collections(),
    );

    // ── Delete confirmation ──────────────────────────────────────────────
    let (confirm_open, set_confirm_open) = signal(false);
    let (deleting_dashboard, set_deleting_dashboard) =
        signal(Option::<(String, String)>::None); // (id, title)

    let on_confirm_delete = Callback::new(move |()| {
        set_confirm_open.set(false);
        if let Some((dashboard_id, _title)) = deleting_dashboard.get_untracked() {
            leptos::task::spawn_local(async move {
                if let Err(e) = delete_dashboard(dashboard_id).await {
                    leptos::logging::error!("Failed to delete dashboard: {e}");
                }
                dashboards_resource.refetch();
            });
        }
    });

    let on_cancel_delete = Callback::new(move |()| {
        set_confirm_open.set(false);
        set_deleting_dashboard.set(None);
    });

    // ── Add to collection ────────────────────────────────────────────────
    let (add_to_collection_dashboard, set_add_to_collection_dashboard) =
        signal(Option::<DashboardListItem>::None);

    // ── Remove from collection ──────────────────────────────────────────
    let (remove_confirm_open, set_remove_confirm_open) = signal(false);
    let (removing_info, set_removing_info) =
        signal(Option::<(String, String, String)>::None); // (collection_id, dashboard_id, collection_name)

    let on_confirm_remove = Callback::new(move |()| {
        set_remove_confirm_open.set(false);
        if let Some((collection_id, dashboard_id, _)) = removing_info.get_untracked() {
            leptos::task::spawn_local(async move {
                if let Err(e) = remove_dashboard_from_collection(collection_id, dashboard_id).await {
                    leptos::logging::error!("Failed to remove from collection: {e}");
                }
                collections_resource.refetch();
            });
        }
    });

    let on_cancel_remove = Callback::new(move |()| {
        set_remove_confirm_open.set(false);
        set_removing_info.set(None);
    });

    // ── Create new dashboard ─────────────────────────────────────────────
    let (creating, set_creating) = signal(false);

    let handle_create = move |_| {
        set_creating.set(true);
        leptos::task::spawn_local(async move {
            match create_dashboard("Untitled Dashboard".to_string(), None).await {
                Ok(dashboard_id) => {
                    let url = format!("/dashboard/{dashboard_id}/edit");
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().set_href(&url);
                    }
                }
                Err(e) => {
                    leptos::logging::error!("Failed to create dashboard: {e}");
                    set_creating.set(false);
                }
            }
        });
    };

    // ── Filter dashboards by active collection ───────────────────────────
    let filtered_dashboards = move || -> Option<Result<Vec<DashboardListItem>, ServerFnError>> {
        let result = dashboards_resource.get()?;
        let active_id = active_collection_id.get();

        match result {
            Err(e) => Some(Err(e)),
            Ok(dashboards) => {
                if let Some(ref coll_id) = active_id {
                    // Get dashboard IDs in the active collection
                    let collection_dashboard_ids: std::collections::HashSet<String> =
                        collections_resource
                            .get()
                            .and_then(|r| r.ok())
                            .unwrap_or_default()
                            .iter()
                            .filter(|c| c.collection_id == *coll_id)
                            .flat_map(|c| c.dashboards.iter().map(|d| d.dashboard_id.clone()))
                            .collect();

                    let filtered: Vec<DashboardListItem> = dashboards
                        .into_iter()
                        .filter(|d| collection_dashboard_ids.contains(&d.dashboard_id))
                        .collect();
                    Some(Ok(filtered))
                } else {
                    Some(Ok(dashboards))
                }
            }
        }
    };

    // Get collections for a given dashboard
    let get_collections = move || -> Vec<CollectionItem> {
        collections_resource
            .get()
            .and_then(|r| r.ok())
            .unwrap_or_default()
    };

    // ── WebSocket subscription: dashboard_update ─────────────────────────
    // When any dashboard is created or deleted (by another user or agent),
    // refetch the list so the UI stays in sync in real-time.
    #[cfg(target_arch = "wasm32")]
    {
        use crate::components::chat::websocket_client::WebSocketContext;
        let ws_ctx = use_context::<WebSocketContext>();

        let ws_ctx_for_effect = ws_ctx.clone();
        Effect::new(move |_| {
            let Some(ws) = ws_ctx_for_effect.as_ref().cloned() else {
                return;
            };

            let unsub = ws.subscribe("dashboard_update", move |msg| {
                let action = msg
                    .data
                    .as_ref()
                    .and_then(|d| d.get("action"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                match action {
                    "created" | "deleted" => {
                        dashboards_resource.refetch();
                    }
                    _ => {}
                }
            });

            let unsub = send_wrapper::SendWrapper::new(unsub);
            on_cleanup(move || {
                unsub.take()();
            });
        });
    }

    view! {
        <div class="flex flex-col h-full bg-background">
            // Row 1: Title + action buttons (matches chats page pattern)
            <div class="page-header h-16 px-4 md:px-6 flex-shrink-0 flex items-center justify-between">
                <h1 class="text-3xl font-display text-foreground">"Dashboards"</h1>

                <div class="flex items-center gap-2">
                    // Collections sidebar toggle
                    <ToggleButton
                        variant=Signal::derive(move || {
                            if collections_open.get() {
                                ButtonVariant::Active
                            } else {
                                ButtonVariant::Secondary
                            }
                        })
                        size=ButtonSize::Sm
                        aria_label=MaybeProp::from(Some("Manage Collections".to_string()))
                        on:click=move |_| set_collections_open.update(|v| *v = !*v)
                    >
                        <svg class="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10" />
                        </svg>
                        <span class="hidden sm:inline">"Collections"</span>
                    </ToggleButton>

                    // Create Dashboard
                    <Button
                        size=ButtonSize::Sm
                        on:click=handle_create
                        disabled=Signal::derive(move || creating.get())
                    >
                        <Show
                            when=move || !creating.get()
                            fallback=|| view! { <Spinner class="text-primary-foreground" /> }
                        >
                            <Icon icon=icondata_lu::LuPlus width="14" height="14" />
                        </Show>
                        <span class="hidden sm:inline whitespace-nowrap">"Create Dashboard"</span>
                    </Button>
                </div>
            </div>

            // Row 2: Search + sort (full-width, matches chats page pattern)
            <div class="bg-background px-4 md:px-6 py-3 flex-shrink-0">
                <div class="flex items-center gap-3">
                    <SearchInput
                        value=Signal::derive(move || search_input.get())
                        on_input=Callback::new(move |val: String| set_search_input.set(val))
                        placeholder="Search dashboards..."
                        class="flex-1"
                    />
                    <div class="w-40">
                        <StyledSelect
                            value=sort_signal.get_untracked()
                            options=vec![
                                ("recent", "Recently Updated"),
                                ("popularity", "Most Popular"),
                                ("created", "Newest First"),
                            ]
                            on_change=move |val: String| set_sort_signal.set(val)
                        />
                    </div>
                </div>

                // Row 3: Collection filter buttons
                <div class="flex items-center gap-2 mt-3">
                    // "All" filter
                    <button
                        on:click=move |_| set_active_collection_id.set(None)
                        class=move || {
                            if active_collection_id.get().is_none() {
                                "px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-primary text-primary-foreground"
                            } else {
                                "px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-secondary text-foreground border border-border hover:bg-secondary/80"
                            }
                        }
                    >
                        "All"
                    </button>

                    // One button per collection
                    <Suspense fallback=|| ()>
                        {move || {
                            let colls = collections_resource.get()
                                .and_then(|r| r.ok())
                                .unwrap_or_default();

                            colls.into_iter().map(|coll| {
                                let coll_id = coll.collection_id.clone();
                                let coll_id_check = coll_id.clone();
                                let color = coll.color.clone().unwrap_or_else(|| "#d97706".to_string());
                                let dot_style = format!("background-color: {color};");
                                let name = coll.name.clone();

                                view! {
                                    <button
                                        on:click=move |_| {
                                            if active_collection_id.get_untracked().as_deref() == Some(&coll_id) {
                                                set_active_collection_id.set(None);
                                            } else {
                                                set_active_collection_id.set(Some(coll_id.clone()));
                                            }
                                        }
                                        class=move || {
                                            if active_collection_id.get().as_deref() == Some(&coll_id_check) {
                                                "px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-primary text-primary-foreground"
                                            } else {
                                                "px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-secondary text-foreground border border-border hover:bg-secondary/80"
                                            }
                                        }
                                    >
                                        <div class="w-2 h-2 rounded-full flex-shrink-0" style=dot_style.clone() />
                                        {name}
                                    </button>
                                }
                            }).collect_view()
                        }}
                    </Suspense>
                </div>
            </div>

            // Content area
            <div class="flex flex-1 min-h-0">
                // Main Content — Dashboards Grid
                <div class="flex-1 overflow-y-auto @container">
                    <div class="p-4 md:p-6">
                        <Transition fallback=move || view! {
                            <div class="w-full grid gap-6 grid-cols-1 @xl:grid-cols-2 @4xl:grid-cols-3">
                                {(0..6).map(|_| view! {
                                    <Card class="bg-muted">
                                        <CardHeader>
                                            <Skeleton class="h-5 w-3/4" />
                                        </CardHeader>
                                        <CardContent>
                                            <Skeleton class="h-4 w-full mb-2" />
                                            <Skeleton class="h-4 w-2/3 mb-4" />
                                            <Skeleton class="h-3 w-1/3" />
                                        </CardContent>
                                        <CardFooter>
                                            <div class="flex gap-2 w-full">
                                                <Skeleton class="h-10 flex-1" />
                                                <Skeleton class="h-10 flex-1" />
                                            </div>
                                        </CardFooter>
                                    </Card>
                                }).collect_view()}
                            </div>
                        }>
                            {move || {
                                let collections = get_collections();

                                filtered_dashboards().map(|result| {
                                    match result {
                                        Err(e) => {
                                            view! {
                                                <div class="text-center py-16 text-destructive">
                                                    <p>"Failed to load dashboards: " {e.to_string()}</p>
                                                </div>
                                            }.into_any()
                                        }
                                        Ok(dashboards) if dashboards.is_empty() => {
                                            let create_cb = Callback::new(handle_create);
                                            let has_active_collection = active_collection_id.get().is_some();
                                            view! {
                                                <DashboardsEmptyState
                                                    has_search=Signal::derive(move || query_signal.get().is_some())
                                                    has_active_collection=has_active_collection
                                                    on_create=create_cb
                                                />
                                            }.into_any()
                                        }
                                        Ok(dashboards) => {
                                            view! {
                                                <DashboardGrid
                                                    dashboards=dashboards
                                                    collections=collections
                                                    on_delete=Callback::new(move |(id, title): (String, String)| {
                                                        set_deleting_dashboard.set(Some((id, title)));
                                                        set_confirm_open.set(true);
                                                    })
                                                    on_add_to_collection=Callback::new(move |dashboard: DashboardListItem| {
                                                        set_add_to_collection_dashboard.set(Some(dashboard));
                                                    })
                                                    on_remove_from_collection=Callback::new(move |(coll_id, dash_id, coll_name): (String, String, String)| {
                                                        set_removing_info.set(Some((coll_id, dash_id, coll_name)));
                                                        set_remove_confirm_open.set(true);
                                                    })
                                                    on_collection_click=Callback::new(move |coll_id: String| {
                                                        if active_collection_id.get_untracked().as_deref() == Some(&coll_id) {
                                                            set_active_collection_id.set(None);
                                                        } else {
                                                            set_active_collection_id.set(Some(coll_id));
                                                        }
                                                    })
                                                />
                                            }.into_any()
                                        }
                                    }
                                })
                            }}
                        </Transition>
                    </div>
                </div>

                // Right sidebar — collections
                <CollectionsSidebar
                    open=Signal::derive(move || collections_open.get())
                    set_open=set_collections_open
                    active_collection_id=Signal::derive(move || active_collection_id.get())
                    set_active_collection_id=set_active_collection_id
                    on_collections_changed=Callback::new(move |()| {
                        collections_resource.refetch();
                        dashboards_resource.refetch();
                    })
                />
            </div>

            // Confirm dialog for delete
            <ConfirmDialog
                open=Signal::derive(move || confirm_open.get())
                title=Signal::derive(move || "Delete Dashboard?".to_string())
                message=Signal::derive(move || {
                    deleting_dashboard.get()
                        .map(|(_, title)| format!("Are you sure you want to delete \"{title}\"? This action cannot be undone."))
                        .unwrap_or_default()
                })
                confirm_text="Delete"
                on_confirm=on_confirm_delete
                on_cancel=on_cancel_delete
            />

            // Confirm dialog for remove from collection
            <ConfirmDialog
                open=Signal::derive(move || remove_confirm_open.get())
                title=Signal::derive(move || "Remove from Collection?".to_string())
                message=Signal::derive(move || {
                    removing_info.get()
                        .map(|(_, _, name)| format!("Remove this dashboard from \"{name}\"?"))
                        .unwrap_or_default()
                })
                confirm_text="Remove"
                on_confirm=on_confirm_remove
                on_cancel=on_cancel_remove
            />

            // Add to Collection modal
            <AddToCollectionModal
                dashboard=Signal::derive(move || add_to_collection_dashboard.get())
                collections=Signal::derive(get_collections)
                on_close=Callback::new(move |()| {
                    set_add_to_collection_dashboard.set(None);
                })
                on_added=Callback::new(move |()| {
                    set_add_to_collection_dashboard.set(None);
                    collections_resource.refetch();
                })
            />
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sub-components
// ─────────────────────────────────────────────────────────────────────────────

/// Dashboard chart icon for empty states.
#[component]
fn DashboardChartIcon() -> impl IntoView {
    view! {
        <Icon icon=icondata_lu::LuFileChartColumn attr:class="w-12 h-12" />
    }
}

/// Empty state when no dashboards exist, search returns nothing, or active
/// collection is empty. Delegates to the shared `EmptyState` component.
#[component]
fn DashboardsEmptyState(
    /// Whether the user has an active search query.
    has_search: Signal<bool>,
    /// Whether filtering by a collection.
    #[prop(default = false)]
    has_active_collection: bool,
    /// Callback for the "Create" button.
    on_create: Callback<leptos::ev::MouseEvent>,
) -> impl IntoView {
    view! {
        {move || {
            if has_search.get() {
                view! {
                    <EmptyState
                        icon=std::sync::Arc::new(|| view! { <DashboardChartIcon /> }.into_any())
                        title="No matching dashboards"
                        description="No dashboards found for your search. Try a different search term."
                    />
                }.into_any()
            } else if has_active_collection {
                view! {
                    <EmptyState
                        icon=std::sync::Arc::new(|| view! { <DashboardChartIcon /> }.into_any())
                        title="No dashboards in this collection"
                        description="Add dashboards to this collection using the + icon on dashboard cards"
                    />
                }.into_any()
            } else {
                view! {
                    <EmptyState
                        icon=std::sync::Arc::new(|| view! { <DashboardChartIcon /> }.into_any())
                        title="No dashboards yet"
                        description="Get started by creating your first markdown dashboard with embedded charts"
                        action=std::sync::Arc::new(move || view! {
                            <Button on:click=move |ev| on_create.run(ev)>
                                <Icon icon=icondata_lu::LuPlus width="14" height="14" />
                                "Create Your First Dashboard"
                            </Button>
                        }.into_any())
                    />
                }.into_any()
            }
        }}
    }
}

/// Grid of dashboard cards.
#[component]
fn DashboardGrid(
    dashboards: Vec<DashboardListItem>,
    collections: Vec<CollectionItem>,
    on_delete: Callback<(String, String)>,
    on_add_to_collection: Callback<DashboardListItem>,
    on_remove_from_collection: Callback<(String, String, String)>,
    on_collection_click: Callback<String>,
) -> impl IntoView {
    let total_collection_count = collections.len();
    let collections = std::sync::Arc::new(collections);
    let items = dashboards
        .into_iter()
        .map(|dashboard| {
            // Find which collections this dashboard belongs to
            let dashboard_collections: Vec<CollectionItem> = collections
                .iter()
                .filter(|c| {
                    c.dashboards
                        .iter()
                        .any(|d| d.dashboard_id == dashboard.dashboard_id)
                })
                .cloned()
                .collect();

            // Show + icon only when there are collections the dashboard is NOT yet in
            let has_available_collections =
                dashboard_collections.len() < total_collection_count;

            view! {
                <DashboardCard
                    dashboard=dashboard
                    collections=dashboard_collections
                    has_available_collections=has_available_collections
                    on_delete=on_delete
                    on_add_to_collection=on_add_to_collection
                    on_remove_from_collection=on_remove_from_collection
                    on_collection_click=on_collection_click
                />
            }
        })
        .collect_view();

    view! {
        <div class="w-full grid gap-6 grid-cols-1 @xl:grid-cols-2 @4xl:grid-cols-3">
            {items}
        </div>
    }
}

/// A single dashboard card with action icons.
#[component]
fn DashboardCard(
    dashboard: DashboardListItem,
    collections: Vec<CollectionItem>,
    /// Whether there are collections the dashboard is NOT yet in.
    #[prop(default = false)]
    has_available_collections: bool,
    on_delete: Callback<(String, String)>,
    on_add_to_collection: Callback<DashboardListItem>,
    on_remove_from_collection: Callback<(String, String, String)>,
    on_collection_click: Callback<String>,
) -> impl IntoView {
    let view_href = format!("/dashboard/{}", dashboard.dashboard_id);
    let view_href_footer = view_href.clone();
    let edit_href = format!("/dashboard/{}/edit", dashboard.dashboard_id);
    let title = dashboard.title.clone();
    let delete_id = dashboard.dashboard_id.clone();
    let delete_title = dashboard.title.clone();
    let relative_time = format_relative_time(&dashboard.updated_at);
    let view_count = dashboard.view_count;
    let summary = dashboard.summary.clone();

    let dashboard_id_for_badges = dashboard.dashboard_id.clone();
    let dashboard_for_add = dashboard.clone();

    view! {
        <Card class="hover:border-primary/30 transition-colors duration-200 flex flex-col">
            <CardHeader>
                <div class="flex items-center justify-between">
                    <CardTitle class="text-xl flex-1 pr-2 line-clamp-2">
                        <a href=view_href.clone() class="hover:text-primary transition-colors">
                            {title}
                        </a>
                    </CardTitle>
                    // Add to Collection — only when collections are available
                    {if has_available_collections {
                        let dashboard_for_add = dashboard_for_add.clone();
                        Some(view! {
                            <Button
                                variant=ButtonVariant::GhostMuted
                                size=ButtonSize::IconSm
                                aria_label="Add to collection"
                                on:click=move |_| on_add_to_collection.run(dashboard_for_add.clone())
                            >
                                <Icon icon=icondata_lu::LuPlus width="14" height="14" />
                            </Button>
                        })
                    } else {
                        None
                    }}
                </div>
            </CardHeader>

            <CardContent class="flex-1 flex flex-col">
                // AI-generated summary / content preview
                {summary.map(|text| {
                    view! {
                        <p class="text-sm text-muted-foreground mb-3 line-clamp-4">
                            {text}
                        </p>
                    }
                })}

                // Collection Badges
                {if !collections.is_empty() {
                    let badges = collections.iter().map(|collection| {
                        let color = collection.color.clone().unwrap_or_else(|| "#d97706".to_string());
                        let bg_color = format!("background-color: {color}20; color: {color};");
                        let dot_color = format!("background-color: {color};");
                        let name = collection.name.clone();
                        let coll_id = collection.collection_id.clone();
                        let coll_id_for_click = coll_id.clone();
                        let coll_id_for_remove = coll_id.clone();
                        let coll_name_for_remove = collection.name.clone();
                        let dash_id_for_remove = dashboard_id_for_badges.clone();
                        view! {
                            <div
                                class="group relative inline-flex items-center gap-1.5 px-2 py-1 rounded-full text-xs font-medium cursor-pointer hover:opacity-80 transition-opacity"
                                style=bg_color
                                on:click=move |_| on_collection_click.run(coll_id_for_click.clone())
                            >
                                <div class="w-2 h-2 rounded-full" style=dot_color.clone() />
                                {name}
                                <button
                                    class="ml-1 hover:bg-foreground/10 rounded-full p-0.5"
                                    aria-label="Remove from collection"
                                    on:click=move |ev: leptos::ev::MouseEvent| {
                                        ev.stop_propagation();
                                        on_remove_from_collection.run((
                                            coll_id_for_remove.clone(),
                                            dash_id_for_remove.clone(),
                                            coll_name_for_remove.clone(),
                                        ));
                                    }
                                >
                                    <Icon icon=icondata_lu::LuX width="12" height="12" />
                                </button>
                            </div>
                        }
                    }).collect_view();

                    Some(view! {
                        <div class="flex flex-wrap gap-2 mb-4">
                            {badges}
                        </div>
                    })
                } else {
                    None
                }}

                // Metadata: time + view count
                <div class="flex flex-wrap items-center gap-2 text-xs text-muted-foreground mt-auto">
                    <div class="flex items-center gap-1">
                        <Icon icon=icondata_lu::LuClock width="14" height="14" />
                        <span class="whitespace-nowrap">"Updated " {relative_time}</span>
                    </div>
                    <div class="flex items-center gap-1">
                        <Icon icon=icondata_lu::LuEye width="14" height="14" />
                        <span class="whitespace-nowrap">{view_count}</span>
                    </div>
                </div>
            </CardContent>

            <CardFooter>
                <div class="flex gap-2 w-full">
                    <ButtonLink href=view_href_footer variant=ButtonVariant::Default class="flex-1">
                        <Icon icon=icondata_lu::LuEye width="14" height="14" />
                        "View"
                    </ButtonLink>
                    <ButtonLink href=edit_href variant=ButtonVariant::Outline class="flex-1">
                        <Icon icon=icondata_lu::LuPencil width="14" height="14" />
                        "Edit"
                    </ButtonLink>
                    {
                        let delete_id = delete_id.clone();
                        let delete_title = delete_title.clone();
                        view! {
                            <Button
                                variant=ButtonVariant::GhostDestructive
                                size=ButtonSize::IconSm
                                aria_label="Delete dashboard"
                                on:click=move |_| on_delete.run((delete_id.clone(), delete_title.clone()))
                            >
                                <Icon icon=icondata_lu::LuTrash2 width="14" height="14" />
                            </Button>
                        }
                    }
                </div>
            </CardFooter>
        </Card>
    }
}

/// Modal for adding a dashboard to a collection.
///
/// Matches the "Add to Collection" modal from React DashboardsList.jsx.
#[component]
fn AddToCollectionModal(
    /// The dashboard to add (None = modal closed).
    dashboard: Signal<Option<DashboardListItem>>,
    /// All collections in the workspace.
    collections: Signal<Vec<CollectionItem>>,
    /// Called when modal is closed without action.
    on_close: Callback<()>,
    /// Called after successfully adding to a collection.
    on_added: Callback<()>,
) -> impl IntoView {
    let (adding, set_adding) = signal(false);

    // Available collections = those that don't already contain this dashboard
    let available_collections = move || -> Vec<CollectionItem> {
        let Some(ref db) = dashboard.get() else {
            return vec![];
        };
        collections
            .get()
            .into_iter()
            .filter(|c| {
                !c.dashboards
                    .iter()
                    .any(|d| d.dashboard_id == db.dashboard_id)
            })
            .collect()
    };

    view! {
        <Show when=move || dashboard.get().is_some()>
            {move || {
                let db = dashboard.get().expect("checked in Show::when");
                let db_title = db.title.clone();
                let db_id = db.dashboard_id.clone();
                let avail = available_collections();

                view! {
                    <div class="fixed inset-0 bg-[var(--color-overlay)] flex items-center justify-center z-50 p-4">
                        <div class="bg-card rounded-lg shadow-xl max-w-md w-full">
                            // Header
                            <div class="flex justify-between items-center p-6 border-b border-border">
                                <h2 class="text-lg font-semibold text-foreground">"Add to Collection"</h2>
                                <Button
                                    variant=ButtonVariant::Secondary
                                    size=ButtonSize::Icon
                                    on:click=move |_| on_close.run(())
                                    aria_label="Close"
                                >
                                    <Icon icon=icondata_lu::LuX width="18" height="18" />
                                </Button>
                            </div>

                            // Body
                            <div class="p-6">
                                <p class="text-sm text-muted-foreground mb-4">
                                    "Add "
                                    <span class="font-semibold">{db_title}</span>
                                    " to:"
                                </p>

                                {if avail.is_empty() {
                                    view! {
                                        <div class="text-center py-8">
                                            <p class="text-muted-foreground">"This dashboard is in all collections"</p>
                                        </div>
                                    }.into_any()
                                } else {
                                    let items = avail.into_iter().map(|collection| {
                                        let coll_id = collection.collection_id.clone();
                                        let coll_name = collection.name.clone();
                                        let coll_desc = collection.description.clone();
                                        let coll_color = collection.color.clone().unwrap_or_else(|| "#d97706".to_string());
                                        let dot_style = format!("background-color: {coll_color};");
                                        let is_public = collection.is_public;
                                        let db_id = db_id.clone();

                                        view! {
                                            <button
                                                class="w-full flex items-center gap-3 p-4 rounded-lg border border-border hover:bg-muted transition-colors disabled:opacity-50"
                                                disabled=move || adding.get()
                                                on:click=move |_| {
                                                    set_adding.set(true);
                                                    let coll_id = coll_id.clone();
                                                    let db_id = db_id.clone();
                                                    leptos::task::spawn_local(async move {
                                                        let result = crate::server_fns::collections::add_dashboard_to_collection(
                                                            coll_id, db_id,
                                                        ).await;
                                                        set_adding.set(false);
                                                        match result {
                                                            Ok(()) => on_added.run(()),
                                                            Err(e) => {
                                                                leptos::logging::error!("Failed to add to collection: {e}");
                                                            }
                                                        }
                                                    });
                                                }
                                            >
                                                <div class="w-4 h-4 rounded-full flex-shrink-0" style=dot_style />
                                                <div class="flex-1 text-left">
                                                    <div class="flex items-center gap-2 mb-1">
                                                        <h3 class="font-medium text-foreground">{coll_name}</h3>
                                                        {if is_public {
                                                            view! {
                                                                <div class="flex items-center gap-1 px-1.5 py-0.5 rounded-md text-xs bg-success/10 text-success-foreground">
                                                                    <Icon icon=icondata_lu::LuGlobe width="12" height="12" />
                                                                    "Public"
                                                                </div>
                                                            }.into_any()
                                                        } else {
                                                            view! {
                                                                <div class="flex items-center gap-1 px-1.5 py-0.5 rounded-md text-xs bg-muted text-muted-foreground">
                                                                    <Icon icon=icondata_lu::LuLock width="12" height="12" />
                                                                    "Private"
                                                                </div>
                                                            }.into_any()
                                                        }}
                                                    </div>
                                                    {coll_desc.map(|desc| {
                                                        view! { <p class="text-sm text-muted-foreground">{desc}</p> }
                                                    })}
                                                </div>
                                                <div class="text-muted-foreground">
                                                    <Icon icon=icondata_lu::LuChevronRight width="20" height="20" />
                                                </div>
                                            </button>
                                        }
                                    }).collect_view();

                                    view! {
                                        <div class="space-y-2 max-h-96 overflow-y-auto">
                                            {items}
                                        </div>
                                    }.into_any()
                                }}
                            </div>
                        </div>
                    </div>
                }
            }}
        </Show>
    }
}
