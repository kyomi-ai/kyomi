// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared document card grid — reusable across dashboards and knowledge pages.
//!
//! Extracted from `dashboards_list.rs` to enable both pages to share a single
//! implementation. Bug fixes here fix both pages.

use leptos::prelude::*;
use phosphor_leptos::Icon;
use crate::components::{
    Button, ButtonSize, ButtonVariant, Card, CardContent, Skeleton,
};
use crate::components::popover::{Placement, Popover};
use crate::server_fns::collections::CollectionItem;
use crate::server_fns::dashboards::DashboardListItem;

// ─────────────────────────────────────────────────────────────────────────────
// Relative time helper
// ─────────────────────────────────────────────────────────────────────────────

/// Converts a timestamp string into a compact relative time string.
///
/// Accepts RFC 3339 (`2026-06-05T09:40:53Z`) and Postgres format
/// (`2026-06-05 09:40:53.348324+00`). Returns `"Updated recently"` if the
/// timestamp cannot be parsed.
///
/// Matches the React frontend's display format:
/// - "just now" (< 60s)
/// - "5m ago" (< 60m)
/// - "2h ago" (< 24h)
/// - "3d ago" (< 30d)
/// - "Mar 15" (>= 30d, same year)
/// - "Mar 15, 2025" (different year)
pub fn format_relative_time(timestamp: &str) -> String {
    let Some(parsed) = crate::utils::time::parse_timestamp(timestamp) else {
        return "Updated recently".to_string();
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
// Loading skeleton
// ─────────────────────────────────────────────────────────────────────────────

/// Loading skeleton grid for document cards.
#[component]
pub fn DocumentCardGridSkeleton() -> impl IntoView {
    view! {
        <div class="w-full grid gap-6 grid-cols-1 @xl:grid-cols-2 @4xl:grid-cols-3">
            {(0..6).map(|_| view! {
                <Card class="bg-muted">
                    <CardContent class="pt-6">
                        <Skeleton class="h-5 w-3/4 mb-3" />
                        <Skeleton class="h-4 w-full mb-1" />
                        <Skeleton class="h-4 w-2/3 mb-4" />
                        <Skeleton class="h-3 w-1/3" />
                    </CardContent>
                </Card>
            }).collect_view()}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Document card grid
// ─────────────────────────────────────────────────────────────────────────────

/// Grid of document cards with collection badges and action buttons.
///
/// Shared between dashboards and knowledge pages. The `on_add_to_collection`,
/// `on_remove_from_collection`, and `on_collection_click` callbacks are
/// optional to support pages that don't use collection management.
#[component]
pub fn DocumentCardGrid(
    /// Documents to display.
    dashboards: Vec<DashboardListItem>,
    /// All collections (for badge display).
    collections: Vec<CollectionItem>,
    /// Callback for delete: (dashboard_id, title).
    on_delete: Callback<(String, String)>,
    /// Callback to add a document to a collection (None = feature disabled).
    #[prop(optional)]
    on_add_to_collection: Option<Callback<DashboardListItem>>,
    /// Callback to remove a document from a collection: (collection_id, dashboard_id, collection_name).
    #[prop(optional)]
    on_remove_from_collection: Option<Callback<(String, String, String)>>,
    /// Callback when a collection badge is clicked.
    #[prop(optional)]
    on_collection_click: Option<Callback<String>>,
    /// Base URL path for view/edit links. Defaults to `/dashboard` so existing
    /// callers are unaffected; the knowledge page passes `/knowledge` so its
    /// cards link to `/knowledge/:id` instead.
    #[prop(default = "/dashboard")]
    base_path: &'static str,
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
                <DocumentCard
                    dashboard=dashboard
                    collections=dashboard_collections
                    has_available_collections=has_available_collections
                    on_delete=on_delete
                    on_add_to_collection=on_add_to_collection
                    on_remove_from_collection=on_remove_from_collection
                    on_collection_click=on_collection_click
                    base_path=base_path
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

// ─────────────────────────────────────────────────────────────────────────────
// Single document card
// ─────────────────────────────────────────────────────────────────────────────

/// A single document card with title, summary, collection badges, and actions.
#[component]
fn DocumentCard(
    dashboard: DashboardListItem,
    collections: Vec<CollectionItem>,
    /// Whether there are collections the dashboard is NOT yet in.
    #[prop(default = false)]
    has_available_collections: bool,
    on_delete: Callback<(String, String)>,
    on_add_to_collection: Option<Callback<DashboardListItem>>,
    on_remove_from_collection: Option<Callback<(String, String, String)>>,
    on_collection_click: Option<Callback<String>>,
    #[prop(default = "/dashboard")]
    base_path: &'static str,
) -> impl IntoView {
    let view_href = format!("{}/{}", base_path, dashboard.dashboard_id);
    let edit_href = format!("{}/{}/edit", base_path, dashboard.dashboard_id);
    let title = dashboard.title.clone();
    let delete_id = dashboard.dashboard_id.clone();
    let delete_title = dashboard.title.clone();
    let relative_time = format_relative_time(&dashboard.updated_at);
    let view_count = dashboard.view_count;
    let summary = dashboard.summary.clone();

    let dashboard_id_for_badges = dashboard.dashboard_id.clone();
    let is_publicly_shared = dashboard.is_publicly_shared;

    // Kebab menu state — must be set up before entering view! macro
    let kebab_trigger_ref = NodeRef::<leptos::html::Div>::new();
    let (kebab_open, set_kebab_open) = signal(false);

    // Store values that are used inside Popover's ChildrenFn closure.
    // ChildrenFn requires Fn (not FnOnce), so non-Copy values must be accessed
    // via StoredValue (which is Copy) rather than moved directly into the closure.
    let edit_href_stored = StoredValue::new(edit_href);
    let navigate = StoredValue::new(leptos_router::hooks::use_navigate());
    let delete_id_stored = StoredValue::new(delete_id);
    let delete_title_stored = StoredValue::new(delete_title);
    let dashboard_for_add_stored = StoredValue::new(dashboard);

    view! {
        <a
            href=view_href
            class="block group"
        >
            <Card class="relative hover:border-primary/30 hover:shadow-md hover:-translate-y-0.5 transition-all duration-200 flex flex-col h-full">
                <CardContent class="flex-1 flex flex-col pt-6">
                    // Title
                    <h3 class="font-display text-xl line-clamp-2 group-hover:text-primary transition-colors mb-2">
                        {title}
                    </h3>

                    // Visibility badge — lock for private, globe for public
                    <div class="flex items-center gap-1 mb-2">
                        {if is_publicly_shared {
                            view! {
                                <span class="inline-flex items-center gap-1 text-xs px-1.5 py-0.5 rounded-full bg-success/10 text-success-foreground">
                                    <Icon icon=phosphor_leptos::GLOBE_SIMPLE size="12px" />
                                    "Public"
                                </span>
                            }.into_any()
                        } else {
                            view! {
                                <span class="inline-flex items-center gap-1 text-xs px-1.5 py-0.5 rounded-full bg-muted text-muted-foreground">
                                    <Icon icon=phosphor_leptos::LOCK_SIMPLE size="12px" />
                                    "Private"
                                </span>
                            }.into_any()
                        }}
                    </div>

                    // Summary or empty-state placeholder
                    {match summary {
                        Some(text) => view! {
                            <p class="text-sm text-muted-foreground line-clamp-3 mb-3">
                                {text}
                            </p>
                        }.into_any(),
                        None => view! {
                            <p class="text-sm text-muted-foreground italic line-clamp-3 mb-3">
                                "No content yet"
                            </p>
                        }.into_any(),
                    }}

                    // Collection Badges
                    {if !collections.is_empty() {
                        let badges = collections.iter().map(|collection| {
                            let color = collection.color.clone().unwrap_or_else(|| "#D97706".to_string());
                            let bg_color = format!("background-color: {color}20; color: {color};");
                            let dot_color = format!("background-color: {color};");
                            let name = collection.name.clone();
                            let coll_id = collection.collection_id.clone();
                            let coll_id_for_click = coll_id.clone();
                            let coll_id_for_remove = coll_id.clone();
                            let coll_name_for_remove = collection.name.clone();
                            let dash_id_for_remove = dashboard_id_for_badges.clone();

                            let on_badge_click = on_collection_click;
                            let on_badge_remove = on_remove_from_collection;

                            view! {
                                <div
                                    class="group relative inline-flex items-center gap-1.5 px-2 py-1 rounded-full text-xs font-medium cursor-pointer hover:opacity-80 transition-opacity"
                                    style=bg_color
                                    on:click=move |ev: leptos::ev::MouseEvent| {
                                        ev.prevent_default();
                                        ev.stop_propagation();
                                        if let Some(cb) = on_badge_click {
                                            cb.run(coll_id_for_click.clone());
                                        }
                                    }
                                >
                                    <div class="w-2 h-2 rounded-full" style=dot_color.clone() />
                                    {name}
                                    {if let Some(cb) = on_badge_remove {
                                        let coll_id_for_remove = coll_id_for_remove.clone();
                                        let dash_id_for_remove = dash_id_for_remove.clone();
                                        let coll_name_for_remove = coll_name_for_remove.clone();
                                        Some(view! {
                                            <button
                                                class="ml-1 hover:bg-foreground/10 rounded-full p-0.5"
                                                aria-label="Remove from collection"
                                                on:click=move |ev: leptos::ev::MouseEvent| {
                                                    ev.prevent_default();
                                                    ev.stop_propagation();
                                                    cb.run((
                                                        coll_id_for_remove.clone(),
                                                        dash_id_for_remove.clone(),
                                                        coll_name_for_remove.clone(),
                                                    ));
                                                }
                                            >
                                                <Icon icon=phosphor_leptos::X size="12px" />
                                            </button>
                                        })
                                    } else {
                                        None
                                    }}
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

                    // Metadata footer: relative time · view count
                    <div class="text-xs text-muted-foreground mt-auto">
                        "Updated "
                        {relative_time}
                        {" \u{00b7} "}
                        {view_count}
                        " views"
                    </div>
                </CardContent>

                // Kebab menu trigger — positioned absolute top-right
                <div
                    node_ref=kebab_trigger_ref
                    class="absolute top-5 right-4 opacity-0 group-hover:opacity-100 [@media(hover:none)]:opacity-100 transition-opacity z-10"
                >
                    <Button
                        variant=ButtonVariant::GhostMuted
                        size=ButtonSize::IconSm
                        aria_label="Actions"
                        on:click=move |ev: leptos::ev::MouseEvent| {
                            ev.prevent_default();
                            ev.stop_propagation();
                            set_kebab_open.update(|v| *v = !*v);
                        }
                    >
                        <Icon icon=phosphor_leptos::DOTS_THREE_VERTICAL size="14px" />
                    </Button>
                </div>
                <Popover
                    trigger_ref=kebab_trigger_ref
                    open=Signal::from(kebab_open)
                    on_close=Callback::new(move |_| set_kebab_open.set(false))
                    placement=Placement::BOTTOM_END
                    class="min-w-[10rem] rounded-md border border-border bg-popover text-popover-foreground shadow-lg p-1"
                >
                    // Edit — client-side navigation
                    <button
                        class="menu-item"
                        on:click=move |ev: leptos::ev::MouseEvent| {
                            ev.prevent_default();
                            ev.stop_propagation();
                            set_kebab_open.set(false);
                            let Some(href) = edit_href_stored.try_get_value() else { return };
                            let Some(nav) = navigate.try_get_value() else { return };
                            nav(&href, Default::default());
                        }
                    >
                        <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="14px" />
                        "Edit"
                    </button>

                    // Add to Collection — only when callback provided and collections available
                    {if has_available_collections {
                        on_add_to_collection.map(|cb| {
                            view! {
                                <button
                                    class="menu-item"
                                    on:click=move |ev: leptos::ev::MouseEvent| {
                                        ev.prevent_default();
                                        ev.stop_propagation();
                                        set_kebab_open.set(false);
                                        if let Some(val) = dashboard_for_add_stored.try_get_value() { cb.run(val); }
                                    }
                                >
                                    <Icon icon=phosphor_leptos::PLUS size="14px" />
                                    "Add to Collection"
                                </button>
                            }
                        })
                    } else {
                        None
                    }}

                    // Divider
                    <div class="border-t border-border my-1" />

                    // Delete — destructive
                    <button
                        class="menu-item text-destructive hover:bg-destructive/10"
                        on:click=move |ev: leptos::ev::MouseEvent| {
                            ev.prevent_default();
                            ev.stop_propagation();
                            set_kebab_open.set(false);
                            if let (Some(id), Some(title)) = (delete_id_stored.try_get_value(), delete_title_stored.try_get_value()) { on_delete.run((id, title)); }
                        }
                    >
                        <Icon icon=phosphor_leptos::TRASH size="14px" />
                        "Delete"
                    </button>
                </Popover>
            </Card>
        </a>
    }
}
