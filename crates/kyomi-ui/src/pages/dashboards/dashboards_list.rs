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

use crate::components::{
    Button, ButtonVariant, Card, CardContent, CardFooter, CardHeader, CardTitle,
    ConfirmDialog, Spinner, StyledSelect,
};
use super::collections_sidebar::CollectionsSidebar;
use crate::server_fns::collections::{
    list_collections, CollectionItem,
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

    // ── Sort ─────────────────────────────────────────────────────────────
    let (sort_signal, set_sort_signal) = signal("recent".to_string());

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

    // ── Confirm dialog derived signals ───────────────────────────────────
    let confirm_title = move || {
        deleting_dashboard
            .get()
            .map(|(_, title)| format!("Delete \"{title}\"?"))
            .unwrap_or_else(|| "Delete Dashboard?".to_string())
    };
    let confirm_message = move || {
        deleting_dashboard
            .get()
            .map(|(_, title)| {
                format!(
                    "Are you sure you want to delete \"{title}\"? This action cannot be undone."
                )
            })
            .unwrap_or_default()
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
        <div class="flex flex-col h-full bg-muted">
            // Header
            <div class="min-h-16 border-b border-border bg-card px-6 py-3 flex-shrink-0 flex flex-col sm:flex-row sm:items-center gap-3">
                <h1 class="text-2xl font-semibold text-foreground flex-shrink-0">
                    {move || {
                        if let Some(ref coll_id) = active_collection_id.get() {
                            // Find collection name
                            let name = collections_resource
                                .get()
                                .and_then(|r| r.ok())
                                .and_then(|colls| {
                                    colls.iter()
                                        .find(|c| c.collection_id == *coll_id)
                                        .map(|c| c.name.clone())
                                });
                            name.unwrap_or_else(|| "All Dashboards".to_string())
                        } else {
                            "All Dashboards".to_string()
                        }
                    }}
                </h1>

                // Search and Sort Controls
                <div class="flex-1 flex items-center gap-3 justify-start sm:justify-center">
                    // Search Input
                    <div class="relative flex-1 max-w-md search-container">
                        <svg
                            class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground"
                            fill="none"
                            stroke="currentColor"
                            viewBox="0 0 24 24"
                        >
                            <path
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                stroke-width="2"
                                d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"
                            />
                        </svg>
                        <input
                            type="text"
                            placeholder="Search dashboards..."
                            class="w-full pl-10 pr-4 py-2 text-sm border border-input rounded-lg focus:ring-2 focus:ring-primary/20 focus:border-primary bg-card text-foreground transition-colors"
                            prop:value=move || search_input.get()
                            on:input=move |ev| {
                                set_search_input.set(event_target_value(&ev));
                            }
                        />
                        // Clear button
                        <Show when=move || !search_input.get().is_empty()>
                            <button
                                class="absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
                                aria-label="Clear search"
                                on:click=move |_| set_search_input.set(String::new())
                            >
                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                                </svg>
                            </button>
                        </Show>
                    </div>

                    // Sort Dropdown
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

                // Action buttons
                <div class="flex items-center gap-3 flex-shrink-0">
                    // Collections toggle button
                    <button
                        class=move || {
                            if collections_open.get() {
                                "flex items-center gap-2 px-2 md:px-4 py-2 text-sm font-medium rounded-lg transition-colors bg-primary/10 text-primary"
                            } else {
                                "flex items-center gap-2 px-2 md:px-4 py-2 text-sm font-medium rounded-lg transition-colors bg-accent text-foreground hover:bg-accent/80"
                            }
                        }
                        aria-label="Toggle Collections"
                        on:click=move |_| set_collections_open.update(|v| *v = !*v)
                    >
                        <svg class="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10" />
                        </svg>
                        <span class="hidden sm:inline">"Collections"</span>
                    </button>

                    // Create Dashboard button
                    <button
                        class="flex items-center gap-2 px-3 md:px-4 py-2 text-sm font-medium rounded-lg transition-colors text-white bg-primary hover:bg-primary/90"
                        on:click=handle_create
                        disabled=move || creating.get()
                    >
                        <Show
                            when=move || !creating.get()
                            fallback=|| view! { <Spinner class="text-white" /> }
                        >
                            <svg class="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
                            </svg>
                        </Show>
                        <span class="hidden sm:inline whitespace-nowrap">"Create Dashboard"</span>
                    </button>
                </div>
            </div>

            // Content area
            <div class="flex flex-1 min-h-0">
                // Main Content — Dashboards Grid
                <div class="flex-1 overflow-y-auto">
                    <div class="p-4 md:p-6">
                        <Transition fallback=move || view! {
                            <div class="flex items-center justify-center py-16">
                                <Spinner class="h-8 w-8 text-muted-foreground" />
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
                                                <EmptyState
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
                title=confirm_title()
                message=confirm_message()
                confirm_text="Delete"
                on_confirm=on_confirm_delete
                on_cancel=on_cancel_delete
            />

            // Add to Collection modal
            <AddToCollectionModal
                dashboard=Signal::derive(move || add_to_collection_dashboard.get())
                collections=Signal::derive(move || get_collections())
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

/// Empty state when no dashboards exist, search returns nothing, or active
/// collection is empty.
#[component]
fn EmptyState(
    /// Whether the user has an active search query.
    has_search: Signal<bool>,
    /// Whether filtering by a collection.
    #[prop(default = false)]
    has_active_collection: bool,
    /// Callback for the "Create" button.
    on_create: Callback<leptos::ev::MouseEvent>,
) -> impl IntoView {
    view! {
        <div class="text-center py-16 bg-card rounded-2xl shadow-sm border border-border">
            <div class="max-w-md mx-auto">
                <svg
                    class="w-24 h-24 mx-auto text-muted-foreground/50 mb-6"
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                >
                    <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="1.5"
                        d="M9 17v-2m3 2v-4m3 4v-6m2 10H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
                    />
                </svg>
                <h3 class="text-xl font-semibold text-foreground mb-2">
                    {move || {
                        if has_search.get() {
                            "No matching dashboards"
                        } else if has_active_collection {
                            "No dashboards in this collection"
                        } else {
                            "No dashboards yet"
                        }
                    }}
                </h3>
                <p class="text-muted-foreground mb-6">
                    {move || {
                        if has_search.get() {
                            "No dashboards found for your search. Try a different search term.".to_string()
                        } else if has_active_collection {
                            "Add dashboards to this collection using the + icon on dashboard cards".to_string()
                        } else {
                            "Get started by creating your first markdown dashboard with embedded charts".to_string()
                        }
                    }}
                </p>
                <Show when=move || !has_search.get() && !has_active_collection>
                    <button
                        class="inline-flex items-center gap-2 px-4 py-2 text-sm font-medium rounded-lg transition-colors text-white bg-primary hover:bg-primary/90"
                        on:click=move |ev| on_create.run(ev)
                    >
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
                        </svg>
                        "Create Your First Dashboard"
                    </button>
                </Show>
            </div>
        </div>
    }
}

/// Grid of dashboard cards.
#[component]
fn DashboardGrid(
    dashboards: Vec<DashboardListItem>,
    collections: Vec<CollectionItem>,
    on_delete: Callback<(String, String)>,
    on_add_to_collection: Callback<DashboardListItem>,
    on_collection_click: Callback<String>,
) -> impl IntoView {
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

            view! {
                <DashboardCard
                    dashboard=dashboard
                    collections=dashboard_collections
                    on_delete=on_delete
                    on_add_to_collection=on_add_to_collection
                    on_collection_click=on_collection_click
                />
            }
        })
        .collect_view();

    view! {
        <div class="w-full grid gap-6 grid-cols-1 md:grid-cols-2 lg:grid-cols-3">
            {items}
        </div>
    }
}

/// A single dashboard card with hamburger menu.
#[component]
fn DashboardCard(
    dashboard: DashboardListItem,
    collections: Vec<CollectionItem>,
    on_delete: Callback<(String, String)>,
    on_add_to_collection: Callback<DashboardListItem>,
    on_collection_click: Callback<String>,
) -> impl IntoView {
    let (menu_open, set_menu_open) = signal(false);

    let view_href = format!("/dashboard/{}", dashboard.dashboard_id);
    let view_href_footer = view_href.clone();
    let edit_href = format!("/dashboard/{}/edit", dashboard.dashboard_id);
    let edit_href_menu = edit_href.clone();
    let title = dashboard.title.clone();
    let delete_id = dashboard.dashboard_id.clone();
    let delete_title = dashboard.title.clone();
    let relative_time = format_relative_time(&dashboard.updated_at);
    let view_count = dashboard.view_count;
    let summary = dashboard.summary.clone();

    let dashboard_for_add = dashboard.clone();

    // Click-outside detection for hamburger menu
    let menu_container_ref = NodeRef::<leptos::html::Div>::new();

    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::prelude::*;

        let cleanup: StoredValue<Option<SendWrapper<Box<dyn FnOnce()>>>> =
            StoredValue::new(None);

        Effect::new(move |_| {
            // Clean up any previous listener
            if let Some(teardown) = cleanup.try_update_value(|v| v.take()).flatten() {
                teardown.take()();
            }

            if menu_open.get() {
                let window = web_sys::window().expect("window");
                let container_el = menu_container_ref.get();

                let cb = Closure::<dyn Fn(web_sys::Event)>::new(move |ev: web_sys::Event| {
                    if let Some(target) = ev.target() {
                        let target_node: web_sys::Node = target.unchecked_into();
                        if let Some(ref el) = container_el {
                            let html_el: &web_sys::HtmlElement = el;
                            let node: &web_sys::Node = html_el.as_ref();
                            if !node.contains(Some(&target_node)) {
                                set_menu_open.set(false);
                            }
                        } else {
                            set_menu_open.set(false);
                        }
                    }
                });

                let _ = window.add_event_listener_with_callback_and_bool(
                    "click",
                    cb.as_ref().unchecked_ref(),
                    true,
                );

                let window_clone = window.clone();
                let cb_ref: js_sys::Function =
                    cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
                let teardown: Box<dyn FnOnce()> = Box::new(move || {
                    let _ = window_clone.remove_event_listener_with_callback_and_bool(
                        "click",
                        &cb_ref,
                        true,
                    );
                    drop(cb);
                });
                cleanup.set_value(Some(SendWrapper::new(teardown)));
            }
        });

        on_cleanup(move || {
            if let Some(teardown) = cleanup.try_update_value(|v| v.take()).flatten() {
                teardown.take()();
            }
        });
    }

    view! {
        <Card class="hover:border-primary/30 transition-colors duration-200 flex flex-col">
            <CardHeader>
                <div class="flex items-start justify-between">
                    <CardTitle class="text-xl flex-1 pr-2 line-clamp-2">
                        <a href=view_href.clone() class="hover:text-primary transition-colors">
                            {title}
                        </a>
                    </CardTitle>
                    <div class="flex gap-1">
                        // Hamburger menu (3-dot)
                        <div node_ref=menu_container_ref class="relative">
                            <button
                                class="flex-shrink-0 p-2 text-muted-foreground hover:text-foreground hover:bg-accent rounded-lg transition-colors"
                                aria-label="Dashboard actions"
                                on:click=move |_| set_menu_open.update(|v| *v = !*v)
                            >
                                // Three-dot vertical icon
                                <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 5v.01M12 12v.01M12 19v.01M12 6a1 1 0 110-2 1 1 0 010 2zm0 7a1 1 0 110-2 1 1 0 010 2zm0 7a1 1 0 110-2 1 1 0 010 2z" />
                                </svg>
                            </button>

                            // Dropdown menu
                            <Show when=move || menu_open.get()>
                                <div class="absolute right-0 top-full mt-1 z-50 min-w-[160px] rounded-md border border-border bg-popover text-popover-foreground shadow-md p-1">
                                    // Edit
                                    <a
                                        href=edit_href_menu.clone()
                                        class="flex items-center gap-2 w-full px-3 py-2 text-sm rounded-sm hover:bg-accent hover:text-accent-foreground transition-colors"
                                    >
                                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                                        </svg>
                                        "Edit"
                                    </a>
                                    // Add to Collection
                                    {
                                        let dashboard_for_add = dashboard_for_add.clone();
                                        view! {
                                            <button
                                                class="flex items-center gap-2 w-full px-3 py-2 text-sm rounded-sm hover:bg-accent hover:text-accent-foreground transition-colors"
                                                on:click=move |_| {
                                                    set_menu_open.set(false);
                                                    on_add_to_collection.run(dashboard_for_add.clone());
                                                }
                                            >
                                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
                                                </svg>
                                                "Add to Collection"
                                            </button>
                                        }
                                    }
                                    // Divider
                                    <div class="my-1 h-px bg-border" />
                                    // Delete
                                    {
                                        let delete_id = delete_id.clone();
                                        let delete_title = delete_title.clone();
                                        view! {
                                            <button
                                                class="flex items-center gap-2 w-full px-3 py-2 text-sm rounded-sm text-destructive hover:bg-destructive/10 transition-colors"
                                                on:click=move |_| {
                                                    set_menu_open.set(false);
                                                    on_delete.run((delete_id.clone(), delete_title.clone()));
                                                }
                                            >
                                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                                                </svg>
                                                "Delete"
                                            </button>
                                        }
                                    }
                                </div>
                            </Show>
                        </div>
                    </div>
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
                        view! {
                            <div
                                class="inline-flex items-center gap-1.5 px-2 py-1 rounded-full text-xs font-medium cursor-pointer hover:opacity-80 transition-opacity"
                                style=bg_color
                                on:click=move |_| on_collection_click.run(coll_id.clone())
                            >
                                <div class="w-2 h-2 rounded-full" style=dot_color.clone() />
                                {name}
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
                        <svg class="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                stroke-width="2"
                                d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z"
                            />
                        </svg>
                        <span class="whitespace-nowrap">"Updated " {relative_time}</span>
                    </div>
                    <div class="flex items-center gap-1">
                        <svg class="w-4 h-4 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
                        </svg>
                        <span class="whitespace-nowrap">{view_count}</span>
                    </div>
                </div>
            </CardContent>

            <CardFooter>
                <div class="flex gap-2 w-full">
                    <a href=view_href_footer class="flex-1">
                        <Button variant=ButtonVariant::Default class="w-full">
                            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
                            </svg>
                            "View"
                        </Button>
                    </a>
                    <a href=edit_href class="flex-1">
                        <Button variant=ButtonVariant::Outline class="w-full">
                            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                            </svg>
                            "Edit"
                        </Button>
                    </a>
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
                    <div class="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
                        <div class="bg-card rounded-2xl shadow-2xl max-w-md w-full">
                            // Header
                            <div class="flex justify-between items-center p-6 border-b border-border">
                                <h2 class="text-2xl font-bold text-foreground">"Add to Collection"</h2>
                                <button
                                    class="text-muted-foreground hover:text-foreground transition-colors"
                                    on:click=move |_| on_close.run(())
                                >
                                    <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                                    </svg>
                                </button>
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
                                                                <div class="flex items-center gap-1 px-1.5 py-0.5 rounded text-xs bg-success/10 text-success-foreground">
                                                                    <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3.055 11H5a2 2 0 012 2v1a2 2 0 002 2 2 2 0 012 2v2.945M8 3.935V5.5A2.5 2.5 0 0010.5 8h.5a2 2 0 012 2 2 2 0 104 0 2 2 0 012-2h1.064M15 20.488V18a2 2 0 012-2h3.064M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                                                                    </svg>
                                                                    "Public"
                                                                </div>
                                                            }.into_any()
                                                        } else {
                                                            view! {
                                                                <div class="flex items-center gap-1 px-1.5 py-0.5 rounded text-xs bg-muted text-muted-foreground">
                                                                    <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
                                                                    </svg>
                                                                    "Private"
                                                                </div>
                                                            }.into_any()
                                                        }}
                                                    </div>
                                                    {coll_desc.map(|desc| {
                                                        view! { <p class="text-sm text-muted-foreground">{desc}</p> }
                                                    })}
                                                </div>
                                                <svg class="w-5 h-5 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
                                                </svg>
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
