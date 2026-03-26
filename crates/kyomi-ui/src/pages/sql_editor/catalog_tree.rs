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

use crate::pages::sql_editor::types::{CatalogNode, CatalogNodeType};
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
) -> impl IntoView {
    // Track expanded nodes by their full name / ID.
    let (expanded_nodes, set_expanded_nodes) = signal(HashSet::<String>::new());

    // Fetch catalog tree reactively when datasource or refresh trigger changes.
    let catalog_resource = Resource::new(
        move || {
            (
                datasource_slug.get(),
                refresh_trigger.get(),
            )
        },
        move |(slug, _trigger)| async move {
            let Some(slug) = slug else {
                return Err("no-datasource".to_string());
            };
            get_catalog_tree(slug, true)
                .await
                .map_err(|e| e.to_string())
        },
    );

    // Reset expanded nodes when datasource changes.
    Effect::new(move |_| {
        let _ = datasource_slug.get();
        set_expanded_nodes.set(HashSet::new());
    });

    let toggle_node = move |node_id: String| {
        set_expanded_nodes.update(|set| {
            if !set.remove(&node_id) {
                set.insert(node_id);
            }
        });
    };

    view! {
        <Transition fallback=move || view! {
            <div class="flex items-center justify-center py-8">
                <crate::components::Spinner />
            </div>
        }>
            {move || Suspend::new(async move {
                let result = catalog_resource.await;
                match result {
                    Err(ref e) if e == "no-datasource" => {
                        // No datasource selected
                        view! {
                            <div class="flex flex-col items-center justify-center py-8 px-4 text-center">
                                <svg class="w-12 h-12 text-muted-foreground mb-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4" />
                                </svg>
                                <p class="text-sm text-muted-foreground">"Select a datasource"</p>
                                <p class="text-xs text-muted-foreground mt-1">"Choose a datasource to browse its catalog"</p>
                            </div>
                        }.into_any()
                    }
                    Err(e) => {
                        // Error state
                        view! {
                            <div class="flex flex-col items-center justify-center py-8 px-4 text-center">
                                <svg class="w-12 h-12 text-error-foreground mb-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
                                </svg>
                                <p class="text-sm text-error-foreground">"Failed to load catalog"</p>
                                <p class="text-xs text-muted-foreground mt-1">{e}</p>
                            </div>
                        }.into_any()
                    }
                    Ok(catalog) => {
                        if catalog.tree.is_empty() {
                            // Empty catalog
                            view! {
                                <div class="flex flex-col items-center justify-center py-8 px-4 text-center">
                                    <svg class="w-12 h-12 text-muted-foreground mb-2" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M20 13V6a2 2 0 00-2-2H6a2 2 0 00-2 2v7m16 0v5a2 2 0 01-2 2H6a2 2 0 01-2-2v-5m16 0h-2.586a1 1 0 00-.707.293l-2.414 2.414a1 1 0 01-.707.293h-3.172a1 1 0 01-.707-.293l-2.414-2.414A1 1 0 006.586 13H4" />
                                    </svg>
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
                                />
                            }.into_any()
                        }
                    }
                }
            })}
        </Transition>
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
                tree.get_value()
                    .into_iter()
                    .filter_map(|node| {
                        if query.is_empty() || node_matches_search(&node, &query) {
                            Some(view! {
                                <CatalogNodeView
                                    node=node
                                    depth=0
                                    search_query=search_query
                                    expanded_nodes=expanded_nodes
                                    on_toggle=on_toggle
                                    on_table_click=on_table_click
                                    on_column_click=on_column_click
                                />
                            })
                        } else {
                            None
                        }
                    })
                    .collect_view()
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
                class="flex items-center gap-1 px-2 py-0.5 hover:bg-accent rounded cursor-pointer transition-colors group"
                on:click=handle_click
            >
                // Expand/collapse arrow
                {if has_children && !is_column {
                    let nid = node_id.clone();
                    view! {
                        <svg
                            class=move || {
                                let expanded = expanded_nodes.get().contains(&nid);
                                if expanded {
                                    "w-3 h-3 flex-shrink-0 text-muted-foreground transition-transform rotate-90"
                                } else {
                                    "w-3 h-3 flex-shrink-0 text-muted-foreground transition-transform"
                                }
                            }
                            fill="none"
                            stroke="currentColor"
                            viewBox="0 0 24 24"
                            on:click=handle_arrow_click
                        >
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
                        </svg>
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

                // TODO: Info button for tables — render when table-detail modal is implemented.
                // Accept an `on_table_info: Option<Callback<String>>` prop and conditionally
                // render the info icon button here.
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
                <svg class="w-4 h-4 flex-shrink-0 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" />
                </svg>
            }
            .into_any()
        }
        CatalogNodeType::Dataset | CatalogNodeType::Schema => {
            view! {
                <svg class="w-4 h-4 flex-shrink-0 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4m0 5c0 2.21-3.582 4-8 4s-8-1.79-8-4" />
                </svg>
            }
            .into_any()
        }
        CatalogNodeType::Table | CatalogNodeType::View => {
            view! {
                <svg class="w-3 h-3 flex-shrink-0 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 10h18M3 14h18m-9-4v8m-7 0h14a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z" />
                </svg>
            }
            .into_any()
        }
        CatalogNodeType::Column(_) => {
            view! {
                <svg class="w-3 h-3 flex-shrink-0 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A1.994 1.994 0 013 12V7a4 4 0 014-4z" />
                </svg>
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
    if let Some(ref full_name) = node.full_name {
        if full_name.to_lowercase().contains(query) {
            return true;
        }
    }
    node.children
        .iter()
        .any(|child| node_matches_search(child, query))
}
