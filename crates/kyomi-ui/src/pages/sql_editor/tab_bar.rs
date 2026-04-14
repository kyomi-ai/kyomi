// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tab bar component — horizontal scrollable bar of result tabs.
//!
//! Mirrors the React `TabBar.jsx` + `ResultTab.jsx` components.
//! Each tab shows a colored circle, row count, execution time, status icon,
//! pin button, and close button. Double-click restores query + datasource.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{Button, ButtonSize, ButtonVariant, Spinner};
use super::state::SqlEditorState;
use super::types::{QueryStatus, ResultTab};

// ─────────────────────────────────────────────────────────────────────────────
// Tab colors — balanced chart palette
// ─────────────────────────────────────────────────────────────────────────────

const TAB_COLORS: [&str; 8] = [
    "#4e79a7", "#f28e2b", "#e15759", "#76b7b2",
    "#59a14f", "#edc948", "#b07aa1", "#ff9da7",
];

fn tab_color(color_index: u8) -> &'static str {
    TAB_COLORS[(color_index % 8) as usize]
}

// ─────────────────────────────────────────────────────────────────────────────
// Formatting helpers (match React's formatTime / formatRows)
// ─────────────────────────────────────────────────────────────────────────────

fn format_time(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

fn format_rows(count: usize) -> String {
    if count < 1_000 {
        format!("{count}")
    } else if count < 1_000_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    }
}

/// Build the info string for a tab (e.g. "1.2K rows · 350ms").
fn tab_info(tab: &ResultTab) -> Option<String> {
    if tab.status == QueryStatus::Running {
        return Some("Running...".to_string());
    }

    if let Some(ref result) = tab.result {
        let row_str = result
            .total_rows
            .or(Some(result.row_count))
            .map(|c| format!("{} rows", format_rows(c)));
        let time_str = result.execution_time.map(format_time);
        let parts: Vec<String> = [row_str, time_str].into_iter().flatten().collect();
        if !parts.is_empty() {
            return Some(parts.join(" \u{00b7} "));
        }
    }

    if tab.status == QueryStatus::Error || tab.error.is_some() {
        return Some("Error".to_string());
    }

    None
}

// ─────────────────────────────────────────────────────────────────────────────
// TabBar component
// ─────────────────────────────────────────────────────────────────────────────

/// Horizontal tab bar with scrollable tabs and optional new-tab button.
///
/// Mirrors `TabBar.jsx` — each tab is rendered inline (no separate component
/// needed since Leptos handles fine-grained reactivity via signals).
///
/// # Callbacks
/// - `on_restore_query` — called on double-click with `(query, datasource_slug)`.
#[component]
pub fn TabBar(
    /// Called when a tab is double-clicked to restore its query + datasource.
    /// Arguments: `(query_text, datasource_slug)`. `None` disables the feature.
    on_restore_query: Option<Callback<(String, Option<String>)>>,
    /// Optional actions rendered on the right side of the tab bar (e.g. "Create Chart" button).
    #[prop(optional)]
    header_actions: Option<impl IntoView + 'static>,
) -> impl IntoView {
    let state = SqlEditorState::use_state();
    let tabs = state.tabs;
    let active_tab_id = state.active_tab_id;

    view! {
        <div class="flex items-center bg-muted border-b border-border" role="tablist" aria-label="Query result tabs">
            // Scrollable tab area
            <div class="flex-1 flex overflow-x-auto overflow-y-hidden scrollbar-thin min-w-0">
                {move || {
                    let current_tabs = tabs.get();
                    let current_active = active_tab_id.get();

                    if current_tabs.is_empty() {
                        view! {
                            <div class="px-4 py-2 text-xs text-muted-foreground italic">
                                "No results yet. Run a query to see results."
                            </div>
                        }
                        .into_any()
                    } else {
                        current_tabs
                            .into_iter()
                            .map(|tab| {
                                let is_active = current_active.as_deref() == Some(&tab.id);
                                let tab_id = tab.id.clone();
                                view! {
                                    <SingleTab
                                        tab=tab
                                        is_active=is_active
                                        on_click={
                                            let tab_id = tab_id.clone();
                                            move || {
                                                state.set_active_tab(Some(tab_id.clone()));
                                            }
                                        }
                                        on_close={
                                            let tab_id = tab_id.clone();
                                            move || {
                                                state.remove_tab(&tab_id);
                                            }
                                        }
                                        on_toggle_pin={
                                            let tab_id = tab_id.clone();
                                            move || {
                                                state.toggle_pin(&tab_id);
                                            }
                                        }
                                        on_restore_query=on_restore_query
                                    />
                                }
                            })
                            .collect_view()
                            .into_any()
                    }
                }}
            </div>
            // Header actions area (e.g. Create Chart button)
            {header_actions.map(|actions| {
                view! {
                    <div class="flex items-center gap-2 px-2 flex-shrink-0">
                        {actions}
                    </div>
                }
            })}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SingleTab component — one tab within the bar
// ─────────────────────────────────────────────────────────────────────────────

/// A single tab in the tab bar.
///
/// Mirrors `ResultTab.jsx` — colored circle, info text, status icon,
/// pin/close buttons, active indicator.
#[component]
fn SingleTab(
    /// The tab data.
    tab: ResultTab,
    /// Whether this tab is the currently active one.
    is_active: bool,
    /// Called when the tab is clicked (single click).
    on_click: impl Fn() + Send + Sync + 'static,
    /// Called when the close button is clicked.
    on_close: impl Fn() + Send + Sync + 'static,
    /// Called when the pin button is clicked.
    on_toggle_pin: impl Fn() + Send + Sync + 'static,
    /// Called when double-clicked to restore query. `None` disables the feature.
    on_restore_query: Option<Callback<(String, Option<String>)>>,
) -> impl IntoView {
    let color = tab_color(tab.color_index);
    let info = tab_info(&tab);
    let is_running = tab.status == QueryStatus::Running;
    let has_error = (tab.status == QueryStatus::Error || tab.error.is_some())
        && tab.status != QueryStatus::Running;
    let pinned = tab.pinned;
    let query = tab.query.clone();
    let ds_slug = tab.datasource_slug.clone();
    let tooltip_query = tab.query.clone();

    // Container class varies by active state
    let container_class = if is_active {
        "flex items-center gap-2 px-3 py-2 border-r border-border cursor-pointer transition-all relative group min-w-0 bg-card"
    } else {
        "flex items-center gap-2 px-3 py-2 border-r border-border cursor-pointer transition-all relative group min-w-0 bg-muted hover:bg-secondary"
    };

    // Circle opacity varies by active state
    let circle_opacity = if is_active { "1" } else { "0.6" };

    // Info text class varies by active state
    let info_class = if is_active {
        "text-xs font-medium select-none whitespace-nowrap text-foreground"
    } else {
        "text-xs font-medium select-none whitespace-nowrap text-muted-foreground"
    };

    // Pin button visibility class
    let pin_visibility_class = if pinned || is_active {
        "opacity-100"
    } else {
        "opacity-0 group-hover:opacity-100"
    };

    // Close button visibility class
    let close_visibility_class = if is_active {
        "opacity-100"
    } else {
        "opacity-0 group-hover:opacity-100"
    };

    view! {
        <div
            class=container_class
            role="tab"
            aria-selected=if is_active { "true" } else { "false" }
            tabindex=if is_active { "0" } else { "-1" }
            on:click=move |_| on_click()
            on:dblclick=move |ev: web_sys::MouseEvent| {
                ev.stop_propagation();
                if let Some(ref cb) = on_restore_query {
                    cb.run((query.clone(), ds_slug.clone()));
                }
            }
            title=tooltip_query
        >
            // Colored circle indicator
            <div
                class="w-3 h-3 rounded-full flex-shrink-0 transition-opacity"
                style:background-color=color
                style:opacity=circle_opacity
            />

            // Info display (rows · time)
            {info.map(|text| {
                view! {
                    <span class=info_class>{text}</span>
                }
            })}

            // Status icon: spinner for running
            {is_running.then(|| {
                view! {
                    <Spinner size="h-3 w-3" class="flex-shrink-0" />
                }
            })}

            // Status icon: info circle for error
            {has_error.then(|| {
                view! {
                    <Icon icon=icondata_lu::LuInfo attr:class="w-3 h-3 text-info-foreground flex-shrink-0" />
                }
            })}

            // Pin button
            <Button
                variant=ButtonVariant::GhostMuted
                size=ButtonSize::IconXs
                class=pin_visibility_class
                aria_label=if pinned {
                    "Unpin tab (will auto-close when limit reached)".to_string()
                } else {
                    "Pin tab (keep permanently)".to_string()
                }
                on:click=move |ev: web_sys::MouseEvent| {
                    ev.stop_propagation();
                    on_toggle_pin();
                }
            >
                {if pinned {
                    view! {
                        <Icon icon=icondata_lu::LuPin attr:class="w-3.5 h-3.5" />
                    }.into_any()
                } else {
                    view! {
                        <Icon icon=icondata_lu::LuBookmark attr:class="w-3.5 h-3.5" />
                    }.into_any()
                }}
            </Button>

            // Close button
            <Button
                variant=ButtonVariant::GhostMuted
                size=ButtonSize::IconXs
                class=close_visibility_class
                aria_label="Close tab".to_string()
                on:click=move |ev: web_sys::MouseEvent| {
                    ev.stop_propagation();
                    on_close();
                }
            >
                <Icon icon=icondata_lu::LuX attr:class="w-3.5 h-3.5" />
            </Button>

            // Active tab indicator (colored bottom border)
            {is_active.then(|| {
                view! {
                    <div
                        class="absolute bottom-0 left-0 right-0 h-0.5"
                        style:background-color=color
                    />
                }
            })}
        </div>
    }
}
