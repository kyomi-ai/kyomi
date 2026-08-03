// SPDX-License-Identifier: AGPL-3.0-or-later

//! Table info modal — displays table metadata (columns, types, descriptions,
//! row count) fetched from the datasource table cache.

use leptos::prelude::*;
use phosphor_leptos::Icon;

use crate::components::modal::{Modal, ModalSize};
use crate::components::Skeleton;
use crate::server_fns::sql_editor::{get_table_info, TableInfoResponse};

// ─── Component ───────────────────────────────────────────────────────────────

/// Modal that displays detailed table metadata: columns, types, modes, and
/// descriptions. Fetches data via `get_table_info` when opened.
#[component]
pub fn TableInfoModal(
    /// Whether the modal is visible.
    #[prop(into)]
    show: Signal<bool>,
    /// Called when the modal is closed.
    on_close: Callback<()>,
    /// The fully qualified table ID (e.g. "project.dataset.table").
    #[prop(into)]
    table_id: Signal<Option<String>>,
    /// The datasource slug (needed for the server function).
    #[prop(into)]
    datasource_slug: Signal<Option<String>>,
) -> impl IntoView {
    // Fetch table info when modal is open and both IDs are present.
    let table_info = Resource::new(
        move || (show.get(), table_id.get(), datasource_slug.get()),
        move |(visible, tid, slug)| async move {
            if !visible {
                return None;
            }
            let tid = tid?;
            let slug = slug?;
            Some(get_table_info(slug, tid).await)
        },
    );

    view! {
        <Modal
            show=show
            on_close=on_close
            title="Table Info"
            size=ModalSize::Lg
        >
            <Transition fallback=move || view! { <TableInfoSkeleton /> }>
                {move || Suspend::new(async move {
                    let result = table_info.await;
                    match result {
                        None => {
                            // Modal not visible or missing params — empty
                            view! { <div /> }.into_any()
                        }
                        Some(Err(e)) => {
                            view! {
                                <div class="flex flex-col items-center justify-center py-8 text-center">
                                    <Icon icon=phosphor_leptos::WARNING attr:class="w-12 h-12 text-error-foreground mb-2" />
                                    <p class="text-sm text-error-foreground">"Failed to load table info"</p>
                                    <p class="text-xs text-muted-foreground mt-1">{e.to_string()}</p>
                                </div>
                            }.into_any()
                        }
                        Some(Ok(info)) => {
                            view! {
                                <div class="animate-fade-in">
                                    <TableInfoContent info=info />
                                </div>
                            }.into_any()
                        }
                    }
                })}
            </Transition>
        </Modal>
    }
}

// ─── Loading skeleton ──────────────────────────────────────────────────────

/// Fallback shown while `get_table_info` is in flight — mirrors
/// `TableInfoContent`'s layout (heading, refreshed timestamp, stats row,
/// then a header-plus-rows column table) so the modal doesn't resize when
/// real data replaces it (KYO-233).
#[component]
fn TableInfoSkeleton() -> impl IntoView {
    view! {
        <div class="space-y-4">
            // Table name heading + qualified path
            <div>
                <Skeleton class="h-5 w-40 mb-1.5" />
                <Skeleton class="h-3 w-56" />
            </div>

            // Last refreshed timestamp
            <Skeleton class="h-3 w-48" />

            // Stats row (Type / Rows / Columns)
            <div class="flex gap-4">
                <Skeleton class="h-4 w-16" />
                <Skeleton class="h-4 w-20" />
                <Skeleton class="h-4 w-20" />
            </div>

            // Columns table — header + rows
            <div class="overflow-hidden border border-border rounded-md">
                <div class="flex items-center gap-6 bg-muted px-3 py-2">
                    <Skeleton class="h-3 w-16" />
                    <Skeleton class="h-3 w-16" />
                    <Skeleton class="h-3 w-14" />
                    <Skeleton class="h-3 w-28" />
                </div>
                {(0..6).map(|_| view! {
                    <div class="flex items-center gap-6 px-3 py-2 border-t border-border">
                        <Skeleton class="h-3 w-20" />
                        <Skeleton class="h-3 w-14" />
                        <Skeleton class="h-3 w-12" />
                        <Skeleton class="h-3 w-36" />
                    </div>
                }).collect_view()}
            </div>
        </div>
    }
}

// ─── Inner content ───────────────────────────────────────────────────────────

/// Renders the table info content once data is loaded.
#[component]
fn TableInfoContent(info: TableInfoResponse) -> impl IntoView {
    let metadata = &info.table_metadata;

    // Extract table-level fields.
    let full_id = info.table_id.clone();
    let table_name = full_id
        .rsplit('.')
        .next()
        .unwrap_or(&full_id)
        .to_string();
    let table_type = metadata
        .get("table_type")
        .and_then(|v| v.as_str())
        .unwrap_or("TABLE")
        .to_string();
    let row_count = metadata
        .get("row_count")
        .and_then(|v| v.as_u64());

    // Format the refreshed-at timestamp for display.
    let refreshed_at = info.structure_refreshed_at.as_deref().map(|ts| {
        // Try to parse and format nicely; fall back to raw string.
        chrono::DateTime::parse_from_rfc3339(ts)
            .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_else(|_| ts.to_string())
    });

    // Extract columns array from metadata.
    let columns: Vec<serde_json::Value> = metadata
        .get("columns")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Build a lookup map from column_descriptions (AI-generated descriptions).
    let col_descriptions = info.column_descriptions.clone();

    view! {
        <div class="space-y-4">
            // Table name heading + qualified path
            <div>
                <h3 class="text-lg font-semibold font-mono text-foreground">{table_name}</h3>
                <p class="text-xs text-muted-foreground font-mono mt-0.5">{full_id}</p>
            </div>

            // Last refreshed timestamp
            <div class="text-xs text-muted-foreground">
                {refreshed_at.map(|ts| view! {
                    <p class="flex items-center gap-1">
                        <Icon icon=phosphor_leptos::CLOCK size="12px" />
                        "Last refreshed: "{ts}
                    </p>
                })}
            </div>

            // Stats row
            <div class="flex gap-4 text-sm">
                <div>
                    <span class="text-muted-foreground">"Type: "</span>
                    <span class="font-medium text-foreground">{table_type}</span>
                </div>
                {row_count.map(|count| view! {
                    <div>
                        <span class="text-muted-foreground">"Rows: "</span>
                        <span class="font-mono font-medium text-foreground">{format_row_count(count)}</span>
                    </div>
                })}
                <div>
                    <span class="text-muted-foreground">"Columns: "</span>
                    <span class="font-mono font-medium text-foreground">{columns.len().to_string()}</span>
                </div>
            </div>

            // Columns table
            {if columns.is_empty() {
                view! {
                    <p class="text-sm text-muted-foreground py-4">"No column metadata available."</p>
                }.into_any()
            } else {
                let rows: Vec<ColumnRow> = columns
                    .iter()
                    .map(|col| {
                        let name = col.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let col_type = col.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let mode = col.get("mode").and_then(|v| v.as_str()).unwrap_or("").to_string();

                        // Merge descriptions: column_descriptions map wins if non-empty.
                        let inline_desc = col
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let ai_desc = col_descriptions
                            .as_ref()
                            .and_then(|m| m.get(&name))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let description = if !ai_desc.is_empty() {
                            ai_desc
                        } else {
                            inline_desc
                        };

                        ColumnRow { name, col_type, mode, description }
                    })
                    .collect();

                view! {
                    <div class="overflow-auto max-h-[50vh] border border-border rounded-md">
                        <table class="w-full text-sm">
                            <thead class="sticky top-0 bg-muted">
                                <tr>
                                    <th class="text-left px-3 py-2 text-xs text-muted-foreground font-medium uppercase tracking-wider">"Name"</th>
                                    <th class="text-left px-3 py-2 text-xs text-muted-foreground font-medium uppercase tracking-wider">"Type"</th>
                                    <th class="text-left px-3 py-2 text-xs text-muted-foreground font-medium uppercase tracking-wider">"Mode"</th>
                                    <th class="text-left px-3 py-2 text-xs text-muted-foreground font-medium uppercase tracking-wider">"Description"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {rows.into_iter().map(|row| {
                                    let mode_class = match row.mode.as_str() {
                                        "REQUIRED" => "text-xs px-1.5 py-0.5 rounded bg-warning/10 text-warning-foreground",
                                        "REPEATED" => "text-xs px-1.5 py-0.5 rounded bg-info/10 text-info-foreground",
                                        _ => "text-xs px-1.5 py-0.5 rounded bg-muted text-muted-foreground",
                                    };
                                    view! {
                                        <tr class="border-b border-border hover:bg-muted/50 transition-colors">
                                            <td class="px-3 py-2 font-mono text-foreground whitespace-nowrap">{row.name}</td>
                                            <td class="px-3 py-2 font-mono text-muted-foreground whitespace-nowrap">{row.col_type}</td>
                                            <td class="px-3 py-2 whitespace-nowrap">
                                                {(!row.mode.is_empty()).then(|| view! {
                                                    <span class=mode_class>{row.mode}</span>
                                                })}
                                            </td>
                                            <td class="px-3 py-2 text-muted-foreground max-w-xs truncate">{row.description}</td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Intermediate struct for rendering column rows.
struct ColumnRow {
    name: String,
    col_type: String,
    mode: String,
    description: String,
}

/// Format a row count with thousands separators (e.g. 1,234,567).
fn format_row_count(count: u64) -> String {
    let s = count.to_string();
    let bytes = s.as_bytes();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(b as char);
    }
    result
}
