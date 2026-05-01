// SPDX-License-Identifier: AGPL-3.0-or-later

//! Query history sidebar panel for the SQL Editor.
//!
//! Matches the React `QueryHistorySidebar.jsx`:
//! - Lists query history with search and "saved only" filter
//! - 50 items per page with infinite scroll (IntersectionObserver)
//! - Click query → load into editor
//! - Star/unstar → toggle `is_saved` via server function
//! - Delete → remove via server function
//! - Relative timestamp formatting ("2h ago", "yesterday")
//! - Error queries shown with red indicator

use leptos::prelude::*;
#[cfg(feature = "hydrate")]
use wasm_bindgen::prelude::*;

use phosphor_leptos::Icon;
use crate::components::{Button, ButtonSize, ButtonVariant, Checkbox, ConfirmDialog};
use crate::pages::sql_editor::types::QueryHistoryEntry;
use crate::server_fns::sql_editor::{
    delete_query_history, list_query_history, update_query_history,
};

// ─── Constants ──────────────────────────────────────────────────────────────

const ITEMS_PER_PAGE: u32 = 50;

// ─── Main component ────────────────────────────────────────────────────────

/// Query history list with infinite scroll, search, and save/delete actions.
#[component]
pub fn QueryHistory(
    /// Debounced search query from the parent sidebar.
    #[prop(into)]
    search_query: Signal<String>,
    /// Increment to trigger a history reload (e.g., after running a query).
    #[prop(into)]
    refresh_trigger: Signal<u32>,
    /// Callback when user clicks a query (query_text, datasource_slug).
    on_query_select: Callback<(String, Option<String>)>,
    /// Callback to clear the search input from inside the component.
    on_search_change: Callback<String>,
) -> impl IntoView {
    // ── Local state ─────────────────────────────────────────────────────
    let (show_saved_only, set_show_saved_only) = signal(false);
    let (entries, set_entries) = signal(Vec::<QueryHistoryEntry>::new());
    let (loading, set_loading) = signal(true);
    let (loading_more, set_loading_more) = signal(false);
    let (has_more, set_has_more) = signal(true);
    let (error, set_error) = signal(Option::<String>::None);
    let (offset, set_offset) = signal(0u32);

    // Sentinel element for infinite scroll.
    let sentinel_ref = NodeRef::<leptos::html::Div>::new();

    // ── Load query history ──────────────────────────────────────────────
    let load_history = move |reset: bool| {
        let search = search_query.get_untracked();
        let saved_only = show_saved_only.get_untracked();
        let current_offset = if reset { 0 } else { offset.get_untracked() };

        if reset {
            set_loading.set(true);
            set_has_more.set(true);
        } else {
            set_loading_more.set(true);
        }
        set_error.set(None);

        let search_opt = if search.is_empty() {
            None
        } else {
            Some(search.clone())
        };

        leptos::task::spawn_local(async move {
            match list_query_history(
                search_opt,
                Some(saved_only),
                ITEMS_PER_PAGE,
                current_offset,
            )
            .await
            {
                Ok(new_data) => {
                    let count = new_data.len() as u32;
                    if reset {
                        set_entries.try_set(new_data);
                        set_offset.try_set(count);
                    } else {
                        set_entries.try_update(|list| list.extend(new_data));
                        set_offset.try_update(|o| *o += count);
                    }
                    set_has_more.try_set(count == ITEMS_PER_PAGE);
                }
                Err(e) => {
                    set_error.try_set(Some(e.to_string()));
                }
            }
            set_loading.try_set(false);
            set_loading_more.try_set(false);
        });
    };

    // ── Reload when search, saved-only filter, or refresh trigger changes ──
    Effect::new(move |_| {
        // Subscribe to all three signals.
        let _ = search_query.get();
        let _ = show_saved_only.get();
        let _ = refresh_trigger.get();
        load_history(true);
    });

    // ── Infinite scroll via IntersectionObserver ────────────────────────
    #[cfg(feature = "hydrate")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::closure::Closure;

        Effect::new(move |_| {
            let Some(el) = sentinel_ref.get() else {
                return;
            };
            let el: web_sys::Element = el.into();

            let has_more_val = has_more.get();
            let loading_more_val = loading_more.get();
            let loading_val = loading.get();

            if !has_more_val || loading_more_val || loading_val {
                return;
            }

            let callback = Closure::<dyn Fn(js_sys::Array)>::new(move |entries: js_sys::Array| {
                if let Some(entry) = entries.get(0).dyn_ref::<web_sys::IntersectionObserverEntry>()
                    && entry.is_intersecting()
                        && has_more.get_untracked()
                        && !loading_more.get_untracked()
                        && !loading.get_untracked()
                    {
                        load_history(false);
                    }
            });

            let options = web_sys::IntersectionObserverInit::new();
            options.set_threshold(&wasm_bindgen::JsValue::from_f64(0.1));

            let observer =
                web_sys::IntersectionObserver::new_with_options(
                    callback.as_ref().unchecked_ref(),
                    &options,
                )
                .ok();

            if let Some(ref obs) = observer {
                obs.observe(&el);
            }

            // Wrap non-Send JS types in SendWrapper for on_cleanup.
            let observer = observer.map(SendWrapper::new);
            let callback = SendWrapper::new(callback);

            on_cleanup(move || {
                if let Some(obs) = observer {
                    obs.disconnect();
                }
                drop(callback);
            });
        });
    }

    // ── Toggle saved status ─────────────────────────────────────────────
    let handle_toggle_saved = move |query_id: String, currently_saved: bool| {
        // Optimistic UI update.
        set_entries.update(|list| {
            if let Some(entry) = list.iter_mut().find(|e| e.id == query_id) {
                entry.is_saved = !currently_saved;
            }
        });

        leptos::task::spawn_local(async move {
            if let Err(_e) = update_query_history(query_id.clone(), Some(!currently_saved)).await {
                // Revert on failure.
                set_entries.try_update(|list| {
                    if let Some(entry) = list.iter_mut().find(|e| e.id == query_id) {
                        entry.is_saved = currently_saved;
                    }
                });
            }
        });
    };

    // ── Delete query (with confirmation dialog) ────────────────────────
    let (delete_dialog_open, set_delete_dialog_open) = signal(false);
    let (pending_delete_id, set_pending_delete_id) = signal(Option::<String>::None);

    let handle_delete = move |query_id: String| {
        set_pending_delete_id.set(Some(query_id));
        set_delete_dialog_open.set(true);
    };

    let on_delete_confirm = Callback::new(move |()| {
        set_delete_dialog_open.set(false);
        if let Some(query_id) = pending_delete_id.get_untracked() {
            set_pending_delete_id.set(None);
            // Remove from local state immediately.
            set_entries.update(|list| {
                list.retain(|e| e.id != query_id);
            });

            leptos::task::spawn_local(async move {
                if let Err(_e) = delete_query_history(query_id).await {
                    // On failure, reload the list to restore correct state.
                    load_history(true);
                }
            });
        }
    });

    let on_delete_cancel = Callback::new(move |()| {
        set_delete_dialog_open.set(false);
        set_pending_delete_id.set(None);
    });

    // ── Render ──────────────────────────────────────────────────────────

    view! {
        <div class="flex flex-col h-full">
            // Delete confirmation dialog
            <ConfirmDialog
                open=Signal::from(delete_dialog_open)
                title="Delete Query?"
                message="Delete this query from history?"
                confirm_text="Delete"
                on_confirm=on_delete_confirm
                on_cancel=on_delete_cancel
            />

            // "Saved only" filter toggle
            <div class="px-3 py-2 border-b border-border bg-muted">
                <label class="flex items-center gap-2 text-xs cursor-pointer">
                    <Checkbox
                        checked=Signal::derive(move || show_saved_only.get())
                        on_change=Callback::new(move |val: bool| set_show_saved_only.set(val))
                    />
                    <span class="text-foreground">"Saved only"</span>
                </label>
            </div>

            // Content area
            {move || {
                if loading.get() {
                    // Loading state
                    view! {
                        <div class="flex-1 flex items-center justify-center py-8">
                            <crate::components::Spinner />
                        </div>
                    }.into_any()
                } else if let Some(err) = error.get() {
                    // Error state
                    view! {
                        <div class="flex-1 p-3">
                            <div class="rounded-md border border-error bg-error/10 p-3">
                                <p class="text-xs text-error-foreground">{err}</p>
                            </div>
                        </div>
                    }.into_any()
                } else if entries.get().is_empty() {
                    // Empty state
                    let sq = search_query.get();
                    let so = show_saved_only.get();
                    view! {
                        <div class="flex-1 flex items-center justify-center p-4">
                            <div class="text-center">
                                <div class="text-xs text-muted-foreground">
                                    {if !sq.is_empty() {
                                        "No queries found"
                                    } else if so {
                                        "No saved queries yet"
                                    } else {
                                        "No query history yet"
                                    }}
                                </div>
                                {(!sq.is_empty()).then(|| {
                                    view! {
                                        <button
                                            class="mt-2 text-xs text-primary transition-colors hover:text-info-foreground"
                                            on:click=move |_| on_search_change.run(String::new())
                                        >
                                            "Clear search"
                                        </button>
                                    }
                                })}
                                {(so && sq.is_empty()).then(|| {
                                    view! {
                                        <button
                                            class="mt-2 text-xs text-primary transition-colors hover:text-info-foreground"
                                            on:click=move |_| set_show_saved_only.set(false)
                                        >
                                            "Show all queries"
                                        </button>
                                    }
                                })}
                            </div>
                        </div>
                    }.into_any()
                } else {
                    // Query list
                    view! {
                        <div class="flex-1 overflow-auto">
                            <For
                                each=move || entries.get()
                                key=|entry| entry.id.clone()
                                children=move |entry| {
                                    let query_text = entry.query_text.clone();
                                    let query_text_select = entry.query_text.clone();
                                    let datasource = entry.datasource.clone();
                                    let entry_id = entry.id.clone();
                                    let entry_id_save = entry.id.clone();
                                    let entry_id_delete = entry.id.clone();
                                    let status = entry.status.clone();
                                    let error_msg = entry.error_message.clone();
                                    let created_at = entry.created_at.clone();
                                    let exec_time = entry.execution_time_ms;
                                    let row_count = entry.row_count;

                                    let preview = get_query_preview(&query_text);
                                    let timestamp = format_relative_time(&created_at);
                                    let exec_time_str = exec_time.and_then(format_execution_time);
                                    let is_error = status == "error";

                                    // Read is_saved reactively from the entries signal so optimistic
                                    // updates are reflected immediately.
                                    let is_saved = {
                                        let eid = entry_id.clone();
                                        Memo::new(move |_| {
                                            entries
                                                .get()
                                                .iter()
                                                .find(|e| e.id == eid)
                                                .map(|e| e.is_saved)
                                                .unwrap_or(false)
                                        })
                                    };

                                    view! {
                                        <div
                                            class="px-3 py-2 border-b border-border cursor-pointer transition-colors hover:bg-secondary"
                                            on:click=move |_| {
                                                on_query_select.run((query_text_select.clone(), datasource.clone()));
                                            }
                                        >
                                            // Query preview + action buttons
                                            <div class="flex items-start justify-between gap-2 mb-1">
                                                <div class="flex-1 min-w-0">
                                                    <div class="text-xs font-mono text-foreground truncate">
                                                        {preview}
                                                    </div>
                                                </div>
                                                <div class="flex items-center gap-1 flex-shrink-0">
                                                    // Star button
                                                    <Button
                                                        variant=ButtonVariant::GhostMuted
                                                        size=ButtonSize::IconXs
                                                        aria_label="Toggle saved"
                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                            ev.stop_propagation();
                                                            handle_toggle_saved(entry_id_save.clone(), is_saved.get());
                                                        }
                                                    >
                                                        <svg
                                                            class="w-3 h-3"
                                                            fill=move || if is_saved.get() { "currentColor" } else { "none" }
                                                            stroke="currentColor"
                                                            viewBox="0 0 24 24"
                                                        >
                                                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                                                        </svg>
                                                    </Button>
                                                    // Delete button
                                                    <Button
                                                        variant=ButtonVariant::GhostDestructive
                                                        size=ButtonSize::IconXs
                                                        aria_label="Delete query"
                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                            ev.stop_propagation();
                                                            handle_delete(entry_id_delete.clone());
                                                        }
                                                    >
                                                        <Icon icon=phosphor_leptos::TRASH attr:class="w-3 h-3" />
                                                    </Button>
                                                </div>
                                            </div>

                                            // Metadata row
                                            <div class="flex items-center gap-2 text-xs text-muted-foreground">
                                                // Timestamp
                                                <span>{timestamp}</span>

                                                // Status indicator
                                                {if is_error {
                                                    let err_display = error_msg.unwrap_or_else(|| "Error".to_string());
                                                    view! {
                                                        <span class="text-error-foreground" title=err_display>
                                                            {"\u{2717}"}
                                                        </span>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <span class="text-success-foreground">{"\u{2713}"}</span>
                                                    }.into_any()
                                                }}

                                                // Execution time
                                                {exec_time_str.map(|s| view! {
                                                    <span>{s}</span>
                                                })}

                                                // Row count
                                                {row_count.map(|rc| view! {
                                                    <span>{format_row_count(rc)}" rows"</span>
                                                })}
                                            </div>
                                        </div>
                                    }
                                }
                            />

                            // Loading more indicator
                            <Show when=move || loading_more.get()>
                                <div class="flex items-center justify-center py-4">
                                    <crate::components::Spinner />
                                    <span class="ml-2 text-xs text-muted-foreground">"Loading more..."</span>
                                </div>
                            </Show>

                            // Infinite scroll sentinel — always in DOM so NodeRef is stable
                            // and the IntersectionObserver doesn't need recreation.
                            <div
                                node_ref=sentinel_ref
                                style="height: 1px"
                                style:visibility=move || {
                                    if has_more.get() && !loading_more.get() { "visible" } else { "hidden" }
                                }
                            />

                            // End of list indicator
                            <Show when=move || !has_more.get() && !entries.get().is_empty()>
                                <div class="py-3 text-center text-xs text-muted-foreground">
                                    "End of query history"
                                </div>
                            </Show>
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

// ─── Formatting helpers ────────────────────────────────────────────────────

/// Get the first line of a query, truncated to 60 characters.
fn get_query_preview(query_text: &str) -> String {
    let first_line = query_text.lines().next().unwrap_or("").trim();
    let char_count = first_line.chars().count();
    if char_count > 60 {
        let truncate_at = first_line
            .char_indices()
            .nth(60)
            .map(|(i, _)| i)
            .unwrap_or(first_line.len());
        format!("{}...", &first_line[..truncate_at])
    } else {
        first_line.to_string()
    }
}

/// Format a timestamp as relative time ("Just now", "5m ago", "2h ago", "Yesterday", "3d ago").
///
/// Uses `chrono` for parsing and `js_sys::Date::now()` (WASM) or `SystemTime` (SSR)
/// for the current time.
fn format_relative_time(iso_string: &str) -> String {
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(iso_string) else {
        // Fallback: try parsing without timezone
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(iso_string, "%Y-%m-%dT%H:%M:%S") {
            let dt = naive.and_utc();
            return format_duration_ago(chrono::Utc::now() - dt);
        }
        return iso_string.to_string();
    };

    let now = chrono::Utc::now();
    let duration = now - parsed.to_utc();
    format_duration_ago(duration)
}

fn format_duration_ago(duration: chrono::TimeDelta) -> String {
    let minutes = duration.num_minutes();
    let hours = duration.num_hours();
    let days = duration.num_days();

    if minutes < 1 {
        "Just now".to_string()
    } else if minutes < 60 {
        format!("{minutes}m ago")
    } else if hours < 24 {
        format!("{hours}h ago")
    } else if days == 1 {
        "Yesterday".to_string()
    } else if days < 7 {
        format!("{days}d ago")
    } else {
        // Fall back to a short date format.
        let now = chrono::Utc::now();
        let date = now - duration;
        date.format("%b %d, %Y").to_string()
    }
}

/// Format execution time in ms to a human-readable string.
fn format_execution_time(ms: i32) -> Option<String> {
    if ms <= 0 {
        return None;
    }
    if ms < 1000 {
        Some(format!("{ms}ms"))
    } else {
        Some(format!("{:.1}s", ms as f64 / 1000.0))
    }
}

/// Format row count with thousand separators.
fn format_row_count(count: i32) -> String {
    if count < 1000 {
        count.to_string()
    } else if count < 1_000_000 {
        format!(
            "{},{:03}",
            count / 1000,
            count % 1000
        )
    } else {
        format!(
            "{},{:03},{:03}",
            count / 1_000_000,
            (count % 1_000_000) / 1000,
            count % 1000
        )
    }
}
