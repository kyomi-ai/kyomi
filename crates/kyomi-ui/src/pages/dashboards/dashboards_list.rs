// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard list page — thin wrapper around shared document components.
//!
//! Uses `DocumentCardGrid`, `SearchSortBar`, and `CollectionsSidebar` for
//! the reusable UI, adding only dashboard-specific logic: create action,
//! empty state text, WebSocket subscription, and collection management.
//!
//! ## Unified list-page filter skeleton (F-010)
//!
//! Dashboards, Knowledge, and Chats all share the same list-page filter
//! skeleton:
//!
//! 1. Page header (`page-header h-16 px-4 md:px-6`) with title +
//!    primary action button.
//! 2. Toolbar row inside `bg-background px-4 md:px-6 pb-3 flex-shrink-0`
//!    containing a full-width `<SearchInput class="flex-1" />` plus an
//!    optional sort `StaticSelect` (Dashboards/Knowledge) or pinned-toggle
//!    (Chats).
//! 3. Optional chip row using identical button classes:
//!    `px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-primary text-primary-foreground`
//!    (active) /
//!    `px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-secondary text-foreground border border-border hover:bg-secondary/80`
//!    (inactive).
//!
//! Dashboards uses *dynamic* collection chips (from the user's collections),
//! Chats uses *static* scope chips (All / Mine / Shared / Slack), and
//! Knowledge omits chips entirely (no scope dimension applies in the
//! single-user knowledge model).

use std::sync::Arc;

use super::collections_sidebar::CollectionsSidebar;
use crate::components::documents::{DocumentCardGrid, DocumentCardGridSkeleton, SearchSortBar};
use crate::components::toast::{toast_error, toast_success};
use crate::components::{
    Button, ButtonSize, ButtonVariant, ConfirmDialog, EmptyState, Spinner, ToggleButton,
};
use crate::query_cache::{QueryCache, use_query};
use crate::server_fns::collections::{
    CollectionItem, list_collections, remove_dashboard_from_collection,
};
use crate::server_fns::dashboards::{DashboardListItem, create_dashboard, delete_dashboard};
use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

// ─────────────────────────────────────────────────────────────────────────────
// Main page component
// ─────────────────────────────────────────────────────────────────────────────

/// Dashboard list page with search, sort, create, delete, and collection
/// integration.
#[component]
pub fn DashboardsListPage() -> impl IntoView {
    // ── Collection sidebar integration points ───────────────────────────
    let (collections_open, set_collections_open) = signal(false);
    let (active_collection_id, set_active_collection_id) = signal(Option::<String>::None);

    // ── Search + sort signals ───────────────────────────────────────────
    let (query_signal, set_query_signal) = signal(Option::<String>::None);
    let (sort_signal, set_sort_signal) = signal("recent".to_string());

    // ── Data fetching ───────────────────────────────────────────────────
    // Dashboards come from the SyncStore (populated from IndexedDB on
    // startup and kept current by the sync engine). Client-side
    // search/sort replaces the server-side query so list page navigation
    // is instant on return visits (KYO-169).
    let query_cache = expect_context::<QueryCache>();
    let sync_store = expect_context::<crate::cache::store::SyncStore>();
    let all_dashboards = sync_store.dashboards();

    let store_initialized = sync_store.initialized();

    // Client-side search + sort derived from the in-memory store.
    let dashboards_signal = Signal::derive(move || {
        let mut items = all_dashboards.get();
        // Search filter — use try_get because query_signal is page-scoped
        // while all_dashboards is Layout-scoped (SyncStore). A sync update
        // after navigation would re-evaluate this derive with disposed page signals.
        if let Some(q) = query_signal.try_get().flatten() {
            let q_lower = q.to_lowercase();
            items.retain(|d| {
                d.title.to_lowercase().contains(&q_lower)
                    || d.summary
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&q_lower)
            });
        }
        // Sort
        let sort = sort_signal.try_get().unwrap_or_default();
        match sort.as_str() {
            "updated_at" | "recent" | "" => items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
            "popularity" => items.sort_by(|a, b| {
                b.popularity_score
                    .partial_cmp(&a.popularity_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.view_count.cmp(&a.view_count))
            }),
            "created_at" | "created" => items.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
            "title" => items.sort_by(|a, b| a.title.cmp(&b.title)),
            _ => items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
        }
        items
    });

    // Collections list, scoped to dashboards. Deps hold the doc_type so
    // each scope (dashboard / knowledge / all) has its own cache entry.
    let collections_resource = use_query(
        "collections",
        || Some("dashboard".to_string()),
        |dt: Option<String>| list_collections(dt),
    );

    // ── Delete confirmation ─────────────────────────────────────────────
    let (confirm_open, set_confirm_open) = signal(false);
    let (deleting_dashboard, set_deleting_dashboard) = signal(Option::<(String, String)>::None); // (id, title)

    let delete_action = Action::new(|dashboard_id: &String| {
        let dashboard_id = dashboard_id.clone();
        async move { delete_dashboard(dashboard_id).await }
    });

    Effect::new(move |_| {
        if let Some(result) = delete_action.value().get() {
            match result {
                Ok(()) => {
                    if let Some((id, _title)) = deleting_dashboard.try_get_untracked().flatten() {
                        sync_store.remove_dashboard(&id);
                    }
                    toast_success("Dashboard deleted");
                }
                Err(e) => toast_error(format!("Failed to delete dashboard: {e}")),
            }
        }
    });

    let on_confirm_delete = Callback::new(move |()| {
        set_confirm_open.set(false);
        if let Some((dashboard_id, _title)) = deleting_dashboard.try_get_untracked().flatten() {
            delete_action.dispatch(dashboard_id);
        }
    });

    let on_cancel_delete = Callback::new(move |()| {
        set_confirm_open.set(false);
        set_deleting_dashboard.set(None);
    });

    // ── Add to collection ───────────────────────────────────────────────
    let (add_to_collection_dashboard, set_add_to_collection_dashboard) =
        signal(Option::<DashboardListItem>::None);

    // ── Remove from collection ──────────────────────────────────────────
    let (remove_confirm_open, set_remove_confirm_open) = signal(false);
    let (removing_info, set_removing_info) = signal(Option::<(String, String, String)>::None); // (collection_id, dashboard_id, collection_name)

    let remove_action = Action::new(move |(collection_id, dashboard_id): &(String, String)| {
        let collection_id = collection_id.clone();
        let dashboard_id = dashboard_id.clone();
        async move { remove_dashboard_from_collection(collection_id, dashboard_id).await }
    });

    Effect::new(move |_| {
        if let Some(result) = remove_action.value().get() {
            if let Err(e) = result {
                leptos::logging::error!("Failed to remove from collection: {e}");
            }
            query_cache.invalidate("collections");
        }
    });

    let on_confirm_remove = Callback::new(move |()| {
        set_remove_confirm_open.set(false);
        if let Some((collection_id, dashboard_id, _)) = removing_info.try_get_untracked().flatten()
        {
            remove_action.dispatch((collection_id, dashboard_id));
        }
    });

    let on_cancel_remove = Callback::new(move |()| {
        set_remove_confirm_open.set(false);
        set_removing_info.set(None);
    });

    // ── Create new dashboard ────────────────────────────────────────────
    let navigate_create = StoredValue::new(leptos_router::hooks::use_navigate());

    let create_action = Action::new(|_: &()| async move {
        create_dashboard("Untitled Dashboard".to_string(), None).await
    });

    Effect::new(move |_| {
        if let Some(result) = create_action.value().get() {
            match result {
                Ok(dashboard_id) => {
                    let url = format!("/dashboard/{dashboard_id}/edit");
                    navigate_create.get_value()(&url, leptos_router::NavigateOptions::default());
                }
                Err(e) => {
                    leptos::logging::error!("Failed to create dashboard: {e}");
                }
            }
        }
    });

    let handle_create = move |_| {
        create_action.dispatch(());
    };

    // ── Filter dashboards by active collection ──────────────────────────
    // Returns None while the SyncStore has not yet been initialized (shows
    // the loading skeleton). Once initialized, returns the list (possibly
    // empty) filtered by the active collection.
    let filtered_dashboards = move || -> Option<Vec<DashboardListItem>> {
        if !store_initialized.get() {
            return None;
        }
        let dashboards = dashboards_signal.get();
        let active_id = active_collection_id.get();

        if let Some(ref coll_id) = active_id {
            let collection_dashboard_ids: std::collections::HashSet<String> = collections_resource
                .get()
                .and_then(|r| r.ok())
                .unwrap_or_default()
                .iter()
                .filter(|c| c.collection_id == *coll_id)
                .flat_map(|c| c.dashboards.iter().map(|d| d.dashboard_id.clone()))
                .collect();

            Some(
                dashboards
                    .into_iter()
                    .filter(|d| collection_dashboard_ids.contains(&d.dashboard_id))
                    .collect(),
            )
        } else {
            Some(dashboards)
        }
    };

    let get_collections = move || -> Vec<CollectionItem> {
        collections_resource
            .get()
            .and_then(|r| r.ok())
            .unwrap_or_default()
    };

    // Freshness handled by the sync engine writing to SyncStore on
    // sync_action WebSocket events (KYO-169).

    view! {
        <div class="flex flex-col h-full bg-background">
            // Row 1: Title + action buttons
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
                        <Icon icon=phosphor_leptos::STACK size="16px" />
                        <span class="hidden sm:inline">"Collections"</span>
                    </ToggleButton>

                    // Create Dashboard
                    <Button
                        size=ButtonSize::Sm
                        on:click=handle_create
                        disabled=Signal::derive(move || create_action.pending().get())
                    >
                        <Show
                            when=move || !create_action.pending().get()
                            fallback=|| view! { <Spinner class="text-primary-foreground" /> }
                        >
                            <Icon icon=phosphor_leptos::PLUS size="14px" />
                        </Show>
                        <span class="hidden sm:inline whitespace-nowrap">"Create Dashboard"</span>
                    </Button>
                </div>
            </div>

            // Row 2: Search + sort
            <SearchSortBar
                on_search=Callback::new(move |q| set_query_signal.set(q))
                on_sort=Callback::new(move |s| set_sort_signal.set(s))
                storage_key="kyomi_dashboards_sort"
                placeholder="Search dashboards..."
            />

            // Collection filter buttons
            <div class="bg-background px-4 md:px-6 pb-3 flex-shrink-0">
                <div class="flex items-center gap-2">
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
                                            if active_collection_id.try_get_untracked().flatten().as_deref() == Some(&coll_id) {
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
                        <Transition fallback=move || view! { <DocumentCardGridSkeleton /> }>
                            {move || {
                                let collections = get_collections();

                                filtered_dashboards().map(|dashboards| {
                                    if dashboards.is_empty() {
                                        let create_cb = Callback::new(handle_create);
                                        let has_active_collection = active_collection_id.get().is_some();
                                        view! {
                                            <DashboardsEmptyState
                                                has_search=Signal::derive(move || query_signal.get().is_some())
                                                has_active_collection=has_active_collection
                                                on_create=create_cb
                                            />
                                        }.into_any()
                                    } else {
                                        view! {
                                            <DocumentCardGrid
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
                                                    if active_collection_id.try_get_untracked().flatten().as_deref() == Some(&coll_id) {
                                                        set_active_collection_id.set(None);
                                                    } else {
                                                        set_active_collection_id.set(Some(coll_id));
                                                    }
                                                })
                                            />
                                        }.into_any()
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
                        query_cache.invalidate("collections");
                        // Dashboards list is kept current by the sync engine (KYO-169).
                    })
                    doc_type="dashboard".to_string()
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
                    query_cache.invalidate("collections");
                    // Dashboards list is kept current by the sync engine (KYO-169).
                })
            />
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sub-components (dashboard-specific)
// ─────────────────────────────────────────────────────────────────────────────

/// Dashboard chart icon for empty states.
#[component]
fn DashboardChartIcon() -> impl IntoView {
    view! {
        <Icon icon=phosphor_leptos::CHART_BAR weight=IconWeight::Duotone size="64px" />
    }
}

/// Empty state when no dashboards exist, search returns nothing, or active
/// collection is empty.
#[component]
fn DashboardsEmptyState(
    has_search: Signal<bool>,
    #[prop(default = false)] has_active_collection: bool,
    on_create: Callback<leptos::ev::MouseEvent>,
) -> impl IntoView {
    view! {
        {move || {
            if has_search.get() {
                view! {
                    <EmptyState
                        icon=Arc::new(|| view! { <DashboardChartIcon /> }.into_any())
                        title="No matching dashboards"
                        description="No dashboards found for your search. Try a different search term."
                    />
                }.into_any()
            } else if has_active_collection {
                view! {
                    <EmptyState
                        icon=Arc::new(|| view! { <DashboardChartIcon /> }.into_any())
                        title="No dashboards in this collection"
                        description="Add dashboards to this collection using the + icon on dashboard cards"
                    />
                }.into_any()
            } else {
                view! {
                    <EmptyState
                        icon=Arc::new(|| view! { <DashboardChartIcon /> }.into_any())
                        title="An empty dashboard is just a first draft."
                        description="Write the one your team will actually read — markdown with charts inline."
                        action=Arc::new(move || view! {
                            <Button on:click=move |ev| on_create.run(ev)>
                                <Icon icon=phosphor_leptos::PLUS size="14px" />
                                "Create Your First Dashboard"
                            </Button>
                        }.into_any())
                    />
                }.into_any()
            }
        }}
    }
}

/// Modal for adding a dashboard to a collection.
#[component]
fn AddToCollectionModal(
    dashboard: Signal<Option<DashboardListItem>>,
    collections: Signal<Vec<CollectionItem>>,
    on_close: Callback<()>,
    on_added: Callback<()>,
) -> impl IntoView {
    let (adding, set_adding) = signal(false);

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
                                    <Icon icon=phosphor_leptos::X size="18px" />
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
                                                        set_adding.try_set(false);
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
                                                                    <Icon icon=phosphor_leptos::GLOBE size="12px" />
                                                                    "Public"
                                                                </div>
                                                            }.into_any()
                                                        } else {
                                                            view! {
                                                                <div class="flex items-center gap-1 px-1.5 py-0.5 rounded-md text-xs bg-muted text-muted-foreground">
                                                                    <Icon icon=phosphor_leptos::LOCK size="12px" />
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
                                                    <Icon icon=phosphor_leptos::CARET_RIGHT size="20px" />
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
