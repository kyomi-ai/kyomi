// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard panel for creating or adding charts to dashboards.
//!
//! Uses MCP server tools (via the JS bridge) to interact with the Kyomi API:
//! - `search_dashboards` — list existing dashboards
//! - `create_dashboard` — create a new dashboard with the chart
//! - `get_dashboard_info` — get existing dashboard content
//! - `modify_dashboard` — append chart to existing dashboard

use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;
use leptos::prelude::Set;

use crate::app::AppState;
use crate::mcp_interop;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Clone, Deserialize)]
struct DashboardEntry {
    dashboard_id: String,
    title: String,
    updated_at: Option<String>,
}

#[derive(Deserialize)]
struct SearchResult {
    dashboards: Option<Vec<DashboardEntry>>,
}

#[derive(Deserialize)]
struct DashboardInfo {
    content: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct MutationResult {
    url: Option<String>,
    error: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the ChartML markdown block for the current chart.
fn get_chart_markdown_block(source_spec: &serde_json::Value) -> String {
    let yaml = serde_yaml::to_string(source_spec).unwrap_or_default();
    format!("```chartml\n{yaml}```")
}

/// Format a relative date string (e.g. "2d ago", "today").
fn format_relative_date(iso_str: &str) -> String {
    let now = js_sys::Date::now();
    let date = js_sys::Date::new(&iso_str.into());
    let diff_ms = now - date.get_time();
    let diff_days = (diff_ms / 86_400_000.0) as i64;

    if diff_days == 0 {
        "today".to_string()
    } else if diff_days == 1 {
        "yesterday".to_string()
    } else if diff_days < 30 {
        format!("{diff_days}d ago")
    } else {
        let diff_months = diff_days / 30;
        if diff_months < 12 {
            format!("{diff_months}mo ago")
        } else {
            format!("{}y ago", diff_days / 365)
        }
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

#[component]
pub fn DashboardPanel() -> impl IntoView {
    let state = expect_context::<AppState>();

    let (active_tab, set_active_tab) = signal("create".to_string());
    let (dashboards, set_dashboards) = signal(None::<Vec<DashboardEntry>>);
    let (loading, set_loading) = signal(false);
    let (selected_id, set_selected_id) = signal(None::<String>);
    let (saving, set_saving) = signal(false);
    let (status_msg, set_status_msg) = signal(None::<(String, bool)>); // (message, is_error)
    let (success, set_success) = signal(None::<(String, Option<String>)>); // (message, url)

    // Load dashboards when switching to "existing" tab
    let load_dashboards = move || {
        set_loading.set(true);
        spawn_local(async move {
            let args = serde_json::json!({ "sort_by": "recent", "limit": 20 });
            match mcp_interop::call_server_tool("search_dashboards", &args).await {
                Ok(result) => {
                    let parsed: SearchResult =
                        serde_json::from_value(result).unwrap_or(SearchResult { dashboards: None });
                    set_dashboards.set(parsed.dashboards);
                }
                Err(e) => {
                    set_status_msg.set(Some((format!("Failed to load dashboards: {e}"), true)));
                    set_dashboards.set(Some(vec![]));
                }
            }
            set_loading.set(false);
        });
    };

    // Create a new dashboard
    let handle_create = move |title: String| {
        let source = state.source_spec.get_untracked();
        set_saving.set(true);
        spawn_local(async move {
            let Some(spec) = source else {
                set_status_msg.set(Some(("No chart data available".to_string(), true)));
                set_saving.set(false);
                return;
            };
            let block = get_chart_markdown_block(&spec);
            let args = serde_json::json!({
                "title": title,
                "content": block,
                "verified_no_duplicates": true,
            });
            match mcp_interop::call_server_tool("create_dashboard", &args).await {
                Ok(result) => {
                    let parsed: MutationResult =
                        serde_json::from_value(result).unwrap_or(MutationResult { url: None, error: None });
                    if let Some(err) = parsed.error {
                        set_status_msg.set(Some((format!("Failed: {err}"), true)));
                        set_saving.set(false);
                    } else {
                        set_success.set(Some(("Dashboard created!".to_string(), parsed.url)));
                    }
                }
                Err(e) => {
                    set_status_msg.set(Some((format!("Failed to create dashboard: {e}"), true)));
                    set_saving.set(false);
                }
            }
        });
    };

    // Add chart to an existing dashboard
    let handle_add_to_existing = move |dashboard_id: String| {
        let source = state.source_spec.get_untracked();
        set_saving.set(true);
        spawn_local(async move {
            let Some(spec) = source else {
                set_status_msg.set(Some(("No chart data available".to_string(), true)));
                set_saving.set(false);
                return;
            };

            // Get existing content
            let info_args = serde_json::json!({ "dashboard_id": dashboard_id });
            let info = match mcp_interop::call_server_tool("get_dashboard_info", &info_args).await {
                Ok(r) => serde_json::from_value::<DashboardInfo>(r)
                    .unwrap_or(DashboardInfo { content: None, error: None }),
                Err(e) => {
                    set_status_msg.set(Some((format!("Failed to load dashboard: {e}"), true)));
                    set_saving.set(false);
                    return;
                }
            };

            if let Some(err) = info.error {
                set_status_msg.set(Some((format!("Failed: {err}"), true)));
                set_saving.set(false);
                return;
            }

            let block = get_chart_markdown_block(&spec);
            let existing = info.content.unwrap_or_default();
            let new_content = if existing.is_empty() {
                block
            } else {
                format!("{existing}\n\n{block}")
            };

            let modify_args = serde_json::json!({
                "dashboard_id": dashboard_id,
                "content": new_content,
                "change_summary": "Added chart from MCP",
            });
            match mcp_interop::call_server_tool("modify_dashboard", &modify_args).await {
                Ok(result) => {
                    let parsed: MutationResult =
                        serde_json::from_value(result).unwrap_or(MutationResult { url: None, error: None });
                    if let Some(err) = parsed.error {
                        set_status_msg.set(Some((format!("Failed: {err}"), true)));
                        set_saving.set(false);
                    } else {
                        set_success.set(Some(("Chart added to dashboard!".to_string(), parsed.url)));
                    }
                }
                Err(e) => {
                    set_status_msg.set(Some((format!("Failed: {e}"), true)));
                    set_saving.set(false);
                }
            }
        });
    };

    view! {
        <div class="dashboard-panel">
            // Success state replaces the entire panel
            {move || success.get().map(|(msg, url)| view! {
                <div class="dashboard-success">
                    <div class="dashboard-success-message">{msg}</div>
                    {url.map(|u| {
                        let url_for_click = u.clone();
                        view! {
                            <button
                                class="dashboard-btn-primary"
                                on:click=move |_| mcp_interop::open_link(&url_for_click)
                            >
                                "Open Dashboard"
                            </button>
                        }
                    })}
                </div>
            })}

            // Normal panel (hidden when success is shown)
            {move || success.get().is_none().then(|| {
                let tab = active_tab.get();
                view! {
                    // Tab bar
                    <div class="dashboard-tabs">
                        <button
                            class=move || if active_tab.get() == "create" { "dashboard-tab active" } else { "dashboard-tab" }
                            on:click=move |_| set_active_tab.set("create".to_string())
                        >
                            "Create New"
                        </button>
                        <button
                            class=move || if active_tab.get() == "existing" { "dashboard-tab active" } else { "dashboard-tab" }
                            on:click=move |_| {
                                set_active_tab.set("existing".to_string());
                                if dashboards.get_untracked().is_none() && !loading.get_untracked() {
                                    load_dashboards();
                                }
                            }
                        >
                            "Add to Existing"
                        </button>
                    </div>

                    // Tab content
                    {if tab == "create" {
                        view! { <CreateTab saving=saving on_create=handle_create /> }.into_any()
                    } else {
                        view! {
                            <ExistingTab
                                dashboards=dashboards
                                loading=loading
                                saving=saving
                                selected_id=selected_id
                                set_selected_id=set_selected_id
                                on_add=handle_add_to_existing
                            />
                        }.into_any()
                    }}

                    // Status message
                    {move || status_msg.get().map(|(msg, is_error)| {
                        let class = if is_error { "dashboard-status error" } else { "dashboard-status" };
                        view! { <div class=class>{msg}</div> }
                    })}
                }
            })}
        </div>
    }
}

#[component]
fn CreateTab(
    saving: ReadSignal<bool>,
    on_create: impl Fn(String) + 'static + Clone + Send,
) -> impl IntoView {
    let state = expect_context::<AppState>();
    let (title_value, set_title_value) = signal(String::new());

    // Pre-fill from chart title
    let initial_title = state
        .source_spec
        .get_untracked()
        .as_ref()
        .and_then(|s| s.get("title").and_then(|t| t.as_str()).map(String::from))
        .unwrap_or_default();
    set_title_value.set(initial_title);

    let can_save = move || !saving.get() && !title_value.get().trim().is_empty();

    view! {
        <div class="dashboard-form">
            <label class="dashboard-field-label">"Dashboard Title"</label>
            <input
                type="text"
                class="dashboard-input"
                placeholder="Enter dashboard title..."
                prop:value=move || title_value.get()
                on:input=move |ev| {
                    set_title_value.set(event_target_value(&ev));
                }
            />
            <div class="dashboard-btn-row">
                <button
                    class="dashboard-btn-primary"
                    disabled=move || !can_save()
                    on:click={
                        let on_create = on_create.clone();
                        move |_| {
                            let title = title_value.get_untracked().trim().to_string();
                            if !title.is_empty() {
                                on_create(title);
                            }
                        }
                    }
                >
                    {move || if saving.get() { "Saving..." } else { "Create Dashboard" }}
                </button>
            </div>
        </div>
    }
}

#[component]
fn ExistingTab(
    dashboards: ReadSignal<Option<Vec<DashboardEntry>>>,
    loading: ReadSignal<bool>,
    saving: ReadSignal<bool>,
    selected_id: ReadSignal<Option<String>>,
    set_selected_id: WriteSignal<Option<String>>,
    on_add: impl Fn(String) + 'static + Clone + Send,
) -> impl IntoView {
    view! {
        <div class="dashboard-form">
            {move || {
                if loading.get() {
                    view! { <div class="dashboard-loading">"Loading dashboards..."</div> }.into_any()
                } else {
                    let dash_list = dashboards.get();
                    let items = dash_list.unwrap_or_default();
                    if items.is_empty() {
                        view! {
                            <div class="dashboard-empty">
                                "No dashboards yet. Create one using the \"Create New\" tab."
                            </div>
                        }.into_any()
                    } else {
                            view! {
                                <div class="dashboard-list">
                                    {items.into_iter().map(|dash| {
                                        let id = dash.dashboard_id.clone();
                                        let id_for_click = id.clone();
                                        let id_for_class = id.clone();
                                        view! {
                                            <button
                                                class=move || {
                                                    if selected_id.get().as_deref() == Some(&id_for_class) {
                                                        "dashboard-item selected"
                                                    } else {
                                                        "dashboard-item"
                                                    }
                                                }
                                                on:click=move |_| set_selected_id.set(Some(id_for_click.clone()))
                                            >
                                                <span class="dashboard-item-title">{dash.title}</span>
                                                {dash.updated_at.map(|dt| view! {
                                                    <span class="dashboard-item-date">
                                                        {format_relative_date(&dt)}
                                                    </span>
                                                })}
                                            </button>
                                        }
                                    }).collect_view()}
                                </div>
                                <div class="dashboard-btn-row">
                                    <button
                                        class="dashboard-btn-primary"
                                        disabled=move || saving.get() || selected_id.get().is_none()
                                        on:click={
                                            let on_add = on_add.clone();
                                            move |_| {
                                                if let Some(id) = selected_id.get_untracked() {
                                                    on_add(id);
                                                }
                                            }
                                        }
                                    >
                                        {move || if saving.get() { "Saving..." } else { "Add to Dashboard" }}
                                    </button>
                                </div>
                            }.into_any()
                    }
                }
            }}
        </div>
    }
}
