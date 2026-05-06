// SPDX-License-Identifier: AGPL-3.0-or-later

//! Root application component and shared state.

use std::rc::Rc;

use leptos::prelude::*;
use chartml_core::ChartML;
use chartml_chart_cartesian::CartesianRenderer;
use chartml_chart_pie::PieRenderer;
use chartml_chart_scatter::ScatterRenderer;
use chartml_chart_metric::MetricRenderer;
use chartml_leptos::ChartMLChart;

use leptos::prelude::Callback;
use kyomi_ui_components::dashboard::ChartHeaderBar;
use crate::info_panel::InfoPanel;
use crate::dashboard_panel::DashboardPanel;
use crate::type_convert::convert_visualize_for_type_change;

const KYOMI_LOGO_SVG: &str = r#"<svg width="18" height="18" viewBox="0 0 50 50" xmlns="http://www.w3.org/2000/svg"><g transform="translate(25, 25)"><g fill="currentColor"><polygon points="0,-22 3.5,-9 0,-5.5 -3.5,-9"/><polygon points="15.5,-15.5 9,-3.5 5.5,-5.5 9,-9"/><polygon points="22,0 9,3.5 5.5,0 9,-3.5"/><polygon points="15.5,15.5 3.5,9 0,5.5 3.5,9"/><polygon points="0,22 -3.5,9 0,5.5 3.5,9"/><polygon points="-15.5,15.5 -9,3.5 -5.5,5.5 -9,9"/><polygon points="-22,0 -9,-3.5 -5.5,0 -9,3.5"/><polygon points="-15.5,-15.5 -3.5,-9 0,-5.5 -3.5,-9"/></g><circle cx="0" cy="0" r="4.5" fill="currentColor"/></g></svg>"#;

// ---------------------------------------------------------------------------
// Shared application state
// ---------------------------------------------------------------------------

/// Reactive state shared across the entire MCP chart app.
#[derive(Clone, Copy)]
pub struct AppState {
    /// Current chart spec (possibly modified by type/orientation/mode switches).
    pub spec: RwSignal<Option<serde_json::Value>>,
    /// Original spec from server (with datasource + query for info panel).
    pub source_spec: RwSignal<Option<serde_json::Value>>,
    /// Color palette from server.
    pub palette: RwSignal<Option<Vec<String>>>,
    /// Chart context ID for "Continue in Kyomi" link.
    pub chart_context_id: RwSignal<Option<String>>,
    /// Kyomi app URL for deep links.
    pub app_url: RwSignal<Option<String>>,
    /// Info panel visibility toggle.
    pub info_panel_open: RwSignal<bool>,
    /// Dashboard panel visibility toggle.
    pub dashboard_panel_open: RwSignal<bool>,
    /// Current theme ("light" or "dark").
    pub theme: RwSignal<String>,
    /// Error message to display.
    pub error: RwSignal<Option<String>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            spec: RwSignal::new(None),
            source_spec: RwSignal::new(None),
            palette: RwSignal::new(None),
            chart_context_id: RwSignal::new(None),
            app_url: RwSignal::new(None),
            info_panel_open: RwSignal::new(false),
            dashboard_panel_open: RwSignal::new(false),
            theme: RwSignal::new("light".to_string()),
            error: RwSignal::new(None),
        }
    }
}

// ---------------------------------------------------------------------------
// ChartML setup
// ---------------------------------------------------------------------------

fn setup_chartml() -> Rc<ChartML> {
    let mut c = ChartML::new();
    c.register_renderer("bar", CartesianRenderer::new());
    c.register_renderer("line", CartesianRenderer::new());
    c.register_renderer("area", CartesianRenderer::new());
    c.register_renderer("pie", PieRenderer::new());
    c.register_renderer("doughnut", PieRenderer::new());
    c.register_renderer("scatter", ScatterRenderer::new());
    c.register_renderer("metric", MetricRenderer::new());
    Rc::new(c)
}

// ---------------------------------------------------------------------------
// Chart type helpers
// ---------------------------------------------------------------------------

const SWITCHABLE_TYPES: &[&str] = &[
    "bar", "line", "area", "scatter", "pie", "doughnut", "table", "metric",
];

fn is_switchable(chart_type: &str) -> bool {
    SWITCHABLE_TYPES.contains(&chart_type)
}

fn get_chart_type(spec: &serde_json::Value) -> Option<String> {
    spec.get("visualize")
        .and_then(|v| v.get("type"))
        .and_then(|t| t.as_str())
        .map(String::from)
}

fn get_chart_orientation(spec: &serde_json::Value) -> Option<String> {
    spec.get("visualize")
        .and_then(|v| v.get("orientation"))
        .and_then(|t| t.as_str())
        .map(String::from)
}

fn get_chart_mode(spec: &serde_json::Value) -> Option<String> {
    spec.get("visualize")
        .and_then(|v| v.get("mode"))
        .and_then(|t| t.as_str())
        .map(String::from)
}

// ---------------------------------------------------------------------------
// Root component
// ---------------------------------------------------------------------------

#[component]
pub fn App() -> impl IntoView {
    let state = AppState::new();
    provide_context(state);

    // Initialize MCP transport and connect to host
    leptos::task::spawn_local(async move {
        if let Err(e) = crate::mcp_interop::initialize(state).await {
            web_sys::console::error_1(&format!("MCP connect failed: {e}").into());
            state.error.set(Some(format!("Failed to connect to host: {e}")));
        }
    });

    // Derive YAML string from spec for ChartMLChart
    let spec_yaml = Memo::new(move |_| {
        state.spec.get().map(|v| {
            serde_yaml::to_string(&v).unwrap_or_default()
        }).unwrap_or_default()
    });

    // Derive chart type info for header
    let chart_type = Memo::new(move |_| {
        state.spec.get().and_then(|s| get_chart_type(&s))
    });
    let chart_orientation = Memo::new(move |_| {
        state.spec.get().and_then(|s| get_chart_orientation(&s))
    });
    let chart_mode = Memo::new(move |_| {
        state.spec.get().and_then(|s| get_chart_mode(&s))
    });

    // ChartML instance — created once, palette is pre-injected into specs by server
    let chartml = StoredValue::new_local(setup_chartml());

    // -- Header event callbacks --

    let on_type_change = Callback::new(move |new_type: String| {
        state.spec.update(|spec_opt| {
            if let Some(spec) = spec_opt {
                let previous_type = get_chart_type(spec).unwrap_or_default();
                if let Some(viz) = spec.get_mut("visualize").and_then(|v| v.as_object_mut()) {
                    viz.insert("type".to_string(), serde_json::Value::String(new_type.clone()));
                    if new_type != "bar" {
                        viz.remove("orientation");
                    }
                    if new_type != "bar" && new_type != "area" {
                        viz.remove("mode");
                    }
                    convert_visualize_for_type_change(viz, &previous_type, &new_type);
                }
            }
        });
    });

    let on_orientation_change = Callback::new(move |orientation: Option<String>| {
        state.spec.update(|spec_opt| {
            if let Some(spec) = spec_opt {
                if let Some(viz) = spec.get_mut("visualize").and_then(|v| v.as_object_mut()) {
                    match orientation {
                        Some(o) => { viz.insert("orientation".to_string(), serde_json::Value::String(o)); }
                        None => { viz.remove("orientation"); }
                    }
                }
            }
        });
    });

    let on_mode_change = Callback::new(move |mode: Option<String>| {
        state.spec.update(|spec_opt| {
            if let Some(spec) = spec_opt {
                if let Some(viz) = spec.get_mut("visualize").and_then(|v| v.as_object_mut()) {
                    match mode {
                        Some(m) => { viz.insert("mode".to_string(), serde_json::Value::String(m)); }
                        None => { viz.remove("mode"); }
                    }
                }
            }
        });
    });

    let on_info_toggle = Callback::new(move |()| {
        state.info_panel_open.update(|v| *v = !*v);
        state.dashboard_panel_open.set(false);
    });

    let on_dashboard_toggle = Callback::new(move |()| {
        state.dashboard_panel_open.update(|v| *v = !*v);
        state.info_panel_open.set(false);
    });

    let on_ask_about = Callback::new(move |()| {
        let ctx_id = state.chart_context_id.get_untracked();
        let url = state.app_url.get_untracked();
        if let (Some(ctx), Some(base)) = (ctx_id, url) {
            crate::mcp_interop::open_link(&format!("{base}/chat?chart={ctx}"));
        }
    });

    view! {
        {move || state.error.get().map(|err| view! {
            <div style="color: #dc2626; padding: 20px; text-align: center; background: #fef2f2; border: 1px solid #fecaca; border-radius: 8px; margin: 20px;">
                {format!("Chart rendering failed: {err}")}
            </div>
        })}

        {move || state.spec.get().map(|_| {
            let ct = chart_type.get();
            let co = chart_orientation.get();
            let cm = chart_mode.get();
            let show_type_selector = ct.as_deref().map(is_switchable).unwrap_or(false);
            let show_ask_about = state.chart_context_id.get().is_some() && state.app_url.get().is_some();

            let logo_slot: leptos::prelude::Children = Box::new(move || view! {
                <button
                    class="kyomi-logo-link"
                    aria-label="Open Kyomi"
                    on:click=move |_| crate::mcp_interop::open_link("https://kyomi.ai")
                    inner_html=KYOMI_LOGO_SVG
                />
            }.into_any());

            view! {
                <ChartHeaderBar
                    chart_type=ct.unwrap_or_default()
                    chart_orientation=co.unwrap_or_default()
                    chart_mode=cm.unwrap_or_default()
                    show_type_selector=show_type_selector
                    show_info=true
                    show_save_to_dashboard=true
                    show_ask_about=show_ask_about
                    on_type_change=on_type_change
                    on_orientation_change=on_orientation_change
                    on_mode_change=on_mode_change
                    on_info=on_info_toggle
                    on_save_to_dashboard=on_dashboard_toggle
                    on_ask_about=on_ask_about
                    before=logo_slot
                />

                {move || state.info_panel_open.get().then(|| view! {
                    <InfoPanel />
                })}

                {move || state.dashboard_panel_open.get().then(|| view! {
                    <DashboardPanel />
                })}

                <ChartMLChart
                    spec=Signal::derive(move || spec_yaml.get())
                    chartml=chartml.get_value()
                />
            }
        })}
    }
}
