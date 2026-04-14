// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared document card grid — reusable across dashboards and knowledge pages.
//!
//! Extracted from `dashboards_list.rs` to enable both pages to share a single
//! implementation. Bug fixes here fix both pages.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Button, ButtonLink, ButtonSize, ButtonVariant, Card, CardContent, CardFooter, CardHeader,
    CardTitle, Skeleton,
};
use crate::server_fns::collections::CollectionItem;
use crate::server_fns::dashboards::DashboardListItem;

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
pub fn format_relative_time(rfc3339: &str) -> String {
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
// Loading skeleton
// ─────────────────────────────────────────────────────────────────────────────

/// Loading skeleton grid for document cards.
#[component]
pub fn DocumentCardGridSkeleton() -> impl IntoView {
    view! {
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
    let view_href_footer = view_href.clone();
    let edit_href = format!("{}/{}/edit", base_path, dashboard.dashboard_id);
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
                    <div class="flex gap-1">
                        // Add to Collection — only when collections are available and callback provided
                        {if has_available_collections && on_add_to_collection.is_some() {
                            let dashboard_for_add = dashboard_for_add.clone();
                            let cb = on_add_to_collection.unwrap();
                            Some(view! {
                                <Button
                                    variant=ButtonVariant::Ghost
                                    size=ButtonSize::Icon
                                    aria_label="Add to collection"
                                    class="flex-shrink-0"
                                    on:click=move |_| cb.run(dashboard_for_add.clone())
                                >
                                    <Icon icon=icondata_lu::LuPlus width="14" height="14" />
                                </Button>
                            })
                        } else {
                            None
                        }}
                        // Delete
                        {
                            let delete_id = delete_id.clone();
                            let delete_title = delete_title.clone();
                            view! {
                                <Button
                                    variant=ButtonVariant::Ghost
                                    size=ButtonSize::Icon
                                    aria_label="Delete"
                                    class="flex-shrink-0"
                                    on:click=move |_| on_delete.run((delete_id.clone(), delete_title.clone()))
                                >
                                    <Icon icon=icondata_lu::LuTrash2 width="14" height="14" attr:class="text-destructive" />
                                </Button>
                            }
                        }
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
                                on:click=move |_| {
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
                                                ev.stop_propagation();
                                                cb.run((
                                                    coll_id_for_remove.clone(),
                                                    dash_id_for_remove.clone(),
                                                    coll_name_for_remove.clone(),
                                                ));
                                            }
                                        >
                                            <Icon icon=icondata_lu::LuX width="12" height="12" />
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
                    <ButtonLink href=view_href_footer variant=ButtonVariant::Default class="w-full flex-1">
                        <Icon icon=icondata_lu::LuEye width="14" height="14" />
                        "View"
                    </ButtonLink>
                    <ButtonLink href=edit_href variant=ButtonVariant::Outline class="w-full flex-1">
                        <Icon icon=icondata_lu::LuPencil width="14" height="14" />
                        "Edit"
                    </ButtonLink>
                </div>
            </CardFooter>
        </Card>
    }
}
