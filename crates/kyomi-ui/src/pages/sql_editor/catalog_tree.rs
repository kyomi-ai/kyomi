// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource catalog tree browser for the SQL Editor sidebar.
//!
//! Matches the React `DatasourceCatalogTree.jsx`:
//! - Hierarchical tree: project > dataset/schema > table > column
//! - Expandable/collapsible nodes with type-specific icons
//! - Click table → insert table name into editor
//! - Click column → insert column name
//! - Info button on tables → show table details
//! - Search filter with 700ms debounce
//! - Refresh button → re-index catalog
//! - Loading skeleton while fetching

use std::collections::HashSet;

use leptos::prelude::*;
use phosphor_leptos::Icon;
use crate::components::{Button, ButtonSize, ButtonVariant};
use crate::pages::sql_editor::types::{CatalogNode, CatalogNodeType};
use crate::query_cache::use_query;
use crate::server_fns::sql_editor::get_catalog_tree;

// ─── Main component ────────────────────────────────────────────────────────

/// Catalog tree browser displaying the hierarchical structure of a datasource.
///
/// Fetches the tree via `get_catalog_tree()` and renders expandable nodes.
/// Search filtering is done client-side by name substring matching.
#[component]
pub fn CatalogTree(
    /// Datasource slug to load the catalog for.
    #[prop(into)]
    datasource_slug: Signal<Option<String>>,
    /// Search query to filter visible nodes.
    #[prop(into)]
    search_query: Signal<String>,
    /// Increment to trigger a catalog reload.
    #[prop(into)]
    refresh_trigger: Signal<u32>,
    /// Callback when user clicks a table (receives full table ID).
    on_table_click: Callback<String>,
    /// Callback when user clicks a column (receives column name).
    on_column_click: Callback<String>,
    /// Callback when user clicks the info button on a table (optional).
    /// When provided, an info icon appears on hover for table rows.
    #[prop(optional)]
    on_table_info: Option<Callback<String>>,
) -> impl IntoView {
    // Store the optional callback so it can be threaded through inner components.
    let stored_table_info = StoredValue::new(on_table_info);

    // Track expanded nodes by their full name / ID.
    let (expanded_nodes, set_expanded_nodes) = signal(HashSet::<String>::new());

    // Fetch catalog tree reactively when datasource or refresh trigger changes.
    // Deps are (slug, trigger) — both Serialize. Empty slug is passed through
    // to the server fn which will return an error; the view handles the
    // "no datasource" case by checking for an empty slug before showing data.
    let catalog_data = use_query(
        "catalog",
        move || {
            (
                datasource_slug.try_get().flatten().unwrap_or_default(),
                refresh_trigger.try_get().unwrap_or(0),
            )
        },
        |(slug, _trigger)| get_catalog_tree(slug, true),
    );

    // Reset expanded nodes when datasource changes.
    Effect::new(move |_| {
        let _ = datasource_slug.try_get();
        let _ = set_expanded_nodes.try_set(HashSet::new());
    });

    let toggle_node = move |node_id: String| {
        set_expanded_nodes.update(|set| {
            if !set.remove(&node_id) {
                set.insert(node_id);
            }
        });
    };

    view! {
        {move || {
            let slug = datasource_slug.get();
            if slug.as_deref().unwrap_or("").is_empty() {
                // No datasource selected
                return view! {
                    <div class="flex flex-col items-center justify-center py-8 px-4 text-center">
                        <Icon icon=phosphor_leptos::DATABASE attr:class="w-12 h-12 text-muted-foreground mb-2" />
                        <p class="text-sm text-muted-foreground">"Select a datasource"</p>
                        <p class="text-xs text-muted-foreground mt-1">"Choose a datasource to browse its catalog"</p>
                    </div>
                }.into_any();
            }

            match catalog_data.get() {
                None => {
                    // Loading (first fetch or transition)
                    view! {
                        <div class="flex items-center justify-center py-8">
                            <crate::components::Spinner />
                        </div>
                    }.into_any()
                }
                Some(Err(e)) => {
                    // Error state
                    view! {
                        <div class="flex flex-col items-center justify-center py-8 px-4 text-center">
                            <Icon icon=phosphor_leptos::WARNING attr:class="w-12 h-12 text-error-foreground mb-2" />
                            <p class="text-sm text-error-foreground">"Failed to load catalog"</p>
                            <p class="text-xs text-muted-foreground mt-1">{e.to_string()}</p>
                        </div>
                    }.into_any()
                }
                Some(Ok(catalog)) => {
                    if catalog.tree.is_empty() {
                        // Empty catalog
                        view! {
                            <div class="flex flex-col items-center justify-center py-8 px-4 text-center">
                                <Icon icon=phosphor_leptos::TRAY attr:class="w-12 h-12 text-muted-foreground mb-2" />
                                <p class="text-sm text-muted-foreground">"No tables indexed"</p>
                                <p class="text-xs text-muted-foreground mt-1">"Index the catalog in datasource settings"</p>
                            </div>
                        }.into_any()
                    } else {
                        // Regular tree view
                        let table_count = catalog.table_count;
                        let tree = catalog.tree;
                        view! {
                            <CatalogTreeView
                                tree=tree
                                table_count=table_count
                                search_query=search_query
                                expanded_nodes=expanded_nodes
                                on_toggle=Callback::new(toggle_node)
                                on_table_click=on_table_click
                                on_column_click=on_column_click
                                on_table_info=stored_table_info.try_get_value().flatten()
                            />
                        }.into_any()
                    }
                }
            }
        }}
    }
}

// ─── Tree view component ───────────────────────────────────────────────────

/// Renders the catalog tree with table count header and recursive nodes.
/// Separated from the main component to avoid re-fetching on search changes.
#[component]
fn CatalogTreeView(
    tree: Vec<CatalogNode>,
    table_count: usize,
    #[prop(into)]
    search_query: Signal<String>,
    #[prop(into)]
    expanded_nodes: Signal<HashSet<String>>,
    on_toggle: Callback<String>,
    on_table_click: Callback<String>,
    on_column_click: Callback<String>,
    /// When Some, an info button is shown on table rows.
    on_table_info: Option<Callback<String>>,
) -> impl IntoView {
    let tree = StoredValue::new(tree);

    view! {
        <div class="text-xs">
            // Table count header
            <Show when=move || table_count != 0>
                <div class="px-2 py-1 text-xs text-muted-foreground border-b border-border">
                    {table_count}" table"{if table_count != 1 { "s" } else { "" }}" indexed"
                </div>
            </Show>

            // Tree nodes
            {move || {
                let query = search_query.get().to_lowercase();
                let filtered: Vec<_> = tree.try_get_value().unwrap_or_default()
                    .into_iter()
                    .filter(|node| query.is_empty() || node_matches_search(node, &query))
                    .collect();

                if filtered.is_empty() && !query.is_empty() {
                    // Empty search results — matches React "No tables found" state
                    view! {
                        <div class="flex flex-col items-center justify-center py-8 px-4 text-center">
                            <Icon icon=phosphor_leptos::MAGNIFYING_GLASS attr:class="w-12 h-12 text-muted-foreground mb-2" />
                            <p class="text-sm text-muted-foreground">"No tables found"</p>
                            <p class="text-xs text-muted-foreground mt-1">"Try a different search term"</p>
                        </div>
                    }.into_any()
                } else {
                    filtered
                        .into_iter()
                        .map(|node| {
                            view! {
                                <CatalogNodeView
                                    node=node
                                    depth=0
                                    search_query=search_query
                                    expanded_nodes=expanded_nodes
                                    on_toggle=on_toggle
                                    on_table_click=on_table_click
                                    on_column_click=on_column_click
                                    on_table_info=on_table_info
                                />
                            }
                        })
                        .collect_view()
                        .into_any()
                }
            }}
        </div>
    }
}

// ─── Single node component ─────────────────────────────────────────────────

/// Renders a single node in the catalog tree (recursive).
#[component]
fn CatalogNodeView(
    node: CatalogNode,
    depth: usize,
    #[prop(into)]
    search_query: Signal<String>,
    #[prop(into)]
    expanded_nodes: Signal<HashSet<String>>,
    on_toggle: Callback<String>,
    on_table_click: Callback<String>,
    on_column_click: Callback<String>,
    /// When Some, an info button is shown on table rows.
    on_table_info: Option<Callback<String>>,
) -> impl IntoView {
    let node_id = node
        .full_name
        .clone()
        .unwrap_or_else(|| node.name.clone());
    let node_id_toggle = node_id.clone();
    let node_id_arrow = node_id.clone();
    let node_name = node.name.clone();
    let node_type = node.node_type.clone();
    let children = node.children;
    let has_children = !children.is_empty();
    let is_table = matches!(node_type, CatalogNodeType::Table | CatalogNodeType::View);
    let is_column = matches!(node_type, CatalogNodeType::Column(_));

    // Extract column data type for display.
    let column_data_type = match &node_type {
        CatalogNodeType::Column(dt) => Some(dt.clone()),
        _ => None,
    };

    let margin_left = if depth > 0 {
        format!("margin-left: {}rem", depth)
    } else {
        String::new()
    };

    // Click handler: tables → insert, columns → insert, others → toggle.
    let click_name = node_name.clone();
    let click_id = node_id.clone();
    let handle_click = move |_: web_sys::MouseEvent| {
        if is_table {
            on_table_click.run(click_id.clone());
        } else if is_column {
            on_column_click.run(click_name.clone());
        } else if has_children {
            on_toggle.run(node_id_toggle.clone());
        }
    };

    // Arrow toggle click (separate from row click).
    let handle_arrow_click = move |ev: web_sys::MouseEvent| {
        ev.stop_propagation();
        on_toggle.run(node_id_arrow.clone());
    };

    let child_count = children.len();

    view! {
        <div style=margin_left>
            <div
                class="flex items-center gap-1 px-2 py-0.5 hover:bg-secondary rounded-md cursor-pointer transition-colors group"
                on:click=handle_click
            >
                // Expand/collapse arrow
                {if has_children && !is_column {
                    let nid = node_id.clone();
                    view! {
                        <Icon
                            icon=phosphor_leptos::CARET_RIGHT
                            attr:class=move || {
                                let expanded = expanded_nodes.get().contains(&nid);
                                if expanded {
                                    "w-3 h-3 flex-shrink-0 text-muted-foreground transition-transform rotate-90"
                                } else {
                                    "w-3 h-3 flex-shrink-0 text-muted-foreground transition-transform"
                                }
                            }
                            on:click=handle_arrow_click
                        />
                    }.into_any()
                } else if !is_column {
                    view! { <div class="w-3" /> }.into_any()
                } else {
                    view! { <span /> }.into_any()
                }}

                // Type-specific icon
                <NodeIcon node_type=node_type.clone() />

                // Name
                <span class=if is_table || is_column {
                    "font-mono text-xs text-card-foreground whitespace-nowrap"
                } else {
                    "text-card-foreground whitespace-nowrap"
                }>
                    {node_name.clone()}
                </span>

                // Column data type
                {column_data_type.map(|dt| view! {
                    <span class="text-muted-foreground text-xs whitespace-nowrap">{dt}</span>
                })}

                // Child count (for non-table containers)
                {(has_children && !is_table).then(|| view! {
                    <span class="text-xs text-muted-foreground flex-shrink-0 whitespace-nowrap">
                        "("{child_count}")"
                    </span>
                })}

                // Info button for tables (only when on_table_info callback is provided)
                {is_table.then_some(on_table_info).flatten().map(|table_info_cb| {
                    let info_id = node_id.clone();
                    view! {
                        <Button
                            variant=ButtonVariant::GhostMuted
                            size=ButtonSize::IconXs
                            class="opacity-0 group-hover:opacity-100 transition-opacity ml-auto flex-shrink-0"
                            aria_label="Table info"
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.stop_propagation();
                                table_info_cb.run(info_id.clone());
                            }
                        >
                            <Icon icon=phosphor_leptos::INFO size="14px" />
                        </Button>
                    }
                })}
            </div>

            // Children (rendered when expanded)
            {move || {
                let nid = node_id.clone();
                let is_expanded = expanded_nodes.get().contains(&nid);
                if has_children && is_expanded {
                    let query = search_query.get().to_lowercase();
                    children
                        .iter()
                        .filter(|child| query.is_empty() || node_matches_search(child, &query))
                        .map(|child| {
                            view! {
                                <CatalogNodeView
                                    node=child.clone()
                                    depth=depth + 1
                                    search_query=search_query
                                    expanded_nodes=expanded_nodes
                                    on_toggle=on_toggle
                                    on_table_click=on_table_click
                                    on_column_click=on_column_click
                                    on_table_info=on_table_info
                                />
                            }
                        })
                        .collect_view()
                        .into_any()
                } else {
                    ().into_any()
                }
            }}
        </div>
    }
}

// ─── Node icon component ───────────────────────────────────────────────────

/// SVG icon for a catalog node, matching the React `getIcon()` logic.
#[component]
fn NodeIcon(node_type: CatalogNodeType) -> impl IntoView {
    match node_type {
        CatalogNodeType::Project | CatalogNodeType::Database => {
            view! {
                <Icon icon=phosphor_leptos::FOLDER attr:class="w-4 h-4 flex-shrink-0 text-muted-foreground" />
            }
            .into_any()
        }
        CatalogNodeType::Dataset | CatalogNodeType::Schema => {
            view! {
                <Icon icon=phosphor_leptos::DATABASE attr:class="w-4 h-4 flex-shrink-0 text-muted-foreground" />
            }
            .into_any()
        }
        CatalogNodeType::Table | CatalogNodeType::View => {
            view! {
                <Icon icon=phosphor_leptos::TABLE attr:class="w-3 h-3 flex-shrink-0 text-primary" />
            }
            .into_any()
        }
        CatalogNodeType::Column(_) => {
            view! {
                <Icon icon=phosphor_leptos::TAG attr:class="w-3 h-3 flex-shrink-0 text-muted-foreground" />
            }
            .into_any()
        }
    }
}

// ─── Search helpers ────────────────────────────────────────────────────────

/// Check if a node or any of its descendants match the search query.
fn node_matches_search(node: &CatalogNode, query: &str) -> bool {
    if node.name.to_lowercase().contains(query) {
        return true;
    }
    if let Some(ref full_name) = node.full_name
        && full_name.to_lowercase().contains(query)
    {
        return true;
    }
    node.children
        .iter()
        .any(|child| node_matches_search(child, query))
}
