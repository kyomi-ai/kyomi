// SPDX-License-Identifier: AGPL-3.0-or-later

//! ChartML extension for the kode WYSIWYG editor.
//!
//! Renders `chartml` fenced code blocks as live interactive charts with
//! tooltips and animations — identical to the dashboard viewer. The kode
//! tree editor uses `<For>` keyed rendering, so chart components persist
//! across editor re-renders as long as their content doesn't change.

use chartml_leptos::ChartMLChart;
use kode_leptos::extension::Extension;
use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;

use crate::chartml_provider::configured_chartml;
use crate::components::dashboard::chart_header_bar::ChartHeaderBar;
use crate::components::dashboard::markdown_renderer::{
    apply_spec_overrides, chart_col_span_class, extract_chart_mode, extract_chart_orientation,
    extract_chart_type, extract_col_span, split_chartml_block,
};

/// Kode extension that renders `chartml` code blocks as live charts.
///
/// Stores the palette name and a reactive `is_dark` memo rather than a
/// pre-built `ChartMLRef`. Each rendered chart block creates its own
/// `ChartMLRef` inside a reactive closure, so charts re-mount with the
/// correct palette when the system theme changes.
pub struct ChartMLExtension {
    palette: String,
    is_dark: send_wrapper::SendWrapper<Memo<bool>>,
}

impl ChartMLExtension {
    /// Create the extension with the named palette and a reactive dark-mode memo.
    ///
    /// Charts are rendered lazily inside reactive closures that call
    /// [`configured_chartml`] — the shared factory that registers all 9 Kyomi
    /// renderers, the DataFusion transform, the palette, the Kyomi editorial
    /// theme, and tracing-based resolver hooks.
    ///
    /// # Arguments
    ///
    /// * `palette_name` — Kyomi palette name (e.g. `"kyomi"`).
    /// * `is_dark` — reactive memo that tracks whether the UI is in dark mode.
    pub fn new(palette_name: &str, is_dark: Memo<bool>) -> Self {
        Self {
            palette: palette_name.to_string(),
            is_dark: send_wrapper::SendWrapper::new(is_dark),
        }
    }
}

impl Extension for ChartMLExtension {
    fn name(&self) -> &str {
        "chartml"
    }

    fn code_block_languages(&self) -> &[&str] {
        &["chartml"]
    }

    fn capture_context(&self) -> Option<Box<dyn Fn()>> {
        use chartml_leptos::ProviderRef;

        let provider = leptos::prelude::use_context::<ProviderRef>()?;

        Some(Box::new(move || {
            leptos::prelude::provide_context(provider.clone());
        }))
    }

    fn render_code_block(
        &self,
        language: &str,
        content: &str,
        _block_start: usize,
        _block_end: usize,
    ) -> Option<AnyView> {
        if language != "chartml" {
            return None;
        }

        // A chartml block may hold a single YAML mapping (one chart) or a
        // YAML sequence with `- type: chart` items (N charts). Split here so
        // the WYSIWYG editor mirrors the viewer: every item becomes its own
        // rendered chart with its own header/edit-chrome, rather than only
        // the first item being visible. See KYO-107.
        let yamls = split_chartml_block(content.trim());
        if yamls.is_empty() {
            return None;
        }

        let palette = self.palette.clone();
        let is_dark = *self.is_dark;

        // The full block content is used by the edit-request listener in the
        // dashboard editor to locate which fence in the source was clicked
        // (each block's content is used as a fingerprint to find its index).
        let full_block_content = content.to_string();

        let views: Vec<AnyView> = yamls
            .into_iter()
            .enumerate()
            .map(|(array_index, item_yaml)| {
                let col_span = serde_yaml::from_str::<serde_json::Value>(&item_yaml)
                    .ok()
                    .as_ref()
                    .map(extract_col_span)
                    .unwrap_or(12);
                let col_class = chart_col_span_class(col_span);
                let palette_clone = palette.clone();
                let block_content = full_block_content.clone();
                let chart_view = move || {
                    let chartml = configured_chartml(&palette_clone, is_dark.get());
                    render_one_chart(
                        item_yaml.clone(),
                        array_index,
                        block_content.clone(),
                        chartml,
                    )
                };
                view! { <div class=col_class>{chart_view}</div> }.into_any()
            })
            .collect();

        Some(
            view! {
                // dashboard-content wrapper triggers chart container CSS (border,
                // bg, radius) on each .chart-card child. Grid wrapping lets items
                // with `layout.colSpan` share rows instead of stacking full-width.
                <div class="dashboard-content not-prose grid grid-cols-12 gap-4">
                    {views}
                </div>
            }
            .into_any(),
        )
    }
}

/// Render a single chart view for the WYSIWYG editor. Owns its own
/// override/refresh signals so per-chart header controls (type/orientation/mode
/// selectors, refresh button) act independently from sibling charts in the
/// same fenced block.
///
/// - `item_yaml` — the YAML for THIS chart (already split out of a sequence
///   block by [`split_chartml_block`] if applicable).
/// - `array_index` — index of this item within its block (0 for mappings).
/// - `block_content` — the full block's YAML (unsplit) used by the edit
///   listener to disambiguate which fence in the source was clicked.
/// - `chartml` — the configured ChartML renderer passed down from the
///   extension instance.
fn render_one_chart(
    item_yaml: String,
    array_index: usize,
    block_content: String,
    chartml: chartml_leptos::ChartMLRef,
) -> AnyView {
    let yaml = item_yaml;
    // Parse initial chart metadata from YAML
    let parsed_spec: Option<serde_json::Value> = serde_yaml::from_str(&yaml).ok();
    let initial_chart_type = parsed_spec.as_ref().and_then(extract_chart_type);
    let initial_orientation = parsed_spec.as_ref().and_then(extract_chart_orientation);
    let initial_mode = parsed_spec.as_ref().and_then(extract_chart_mode);

    // Override signals — same pattern as ChartBlock in markdown_renderer
    let (type_override, set_type_override) = signal(None::<String>);
    let (orientation_override, set_orientation_override) = signal(None::<Option<String>>);
    let (mode_override, set_mode_override) = signal(None::<Option<String>>);

    let initial_type_stored = StoredValue::new(initial_chart_type.clone());
    let initial_orient_stored = StoredValue::new(initial_orientation.clone());
    let initial_mode_stored = StoredValue::new(initial_mode.clone());

    // Derived current values for the header bar display
    let current_chart_type = Memo::new(move |_| {
        type_override
            .get()
            .or_else(|| initial_type_stored.get_value())
    });
    let current_orientation = Memo::new(move |_| match orientation_override.get() {
        Some(o) => o,
        None => initial_orient_stored.get_value(),
    });
    let current_mode = Memo::new(move |_| match mode_override.get() {
        Some(m) => m,
        None => initial_mode_stored.get_value(),
    });

    // Derive effective YAML spec with overrides applied
    let yaml_for_spec = yaml.clone();
    let effective_spec = Memo::new(move |_| {
        let t_ovr = type_override.get();
        let o_ovr = orientation_override.get();
        let m_ovr = mode_override.get();

        if t_ovr.is_none() && o_ovr.is_none() && m_ovr.is_none() {
            return yaml_for_spec.clone();
        }

        apply_spec_overrides(
            &yaml_for_spec,
            t_ovr.as_deref(),
            o_ovr.as_ref().map(|o| o.as_deref()),
            m_ovr.as_ref().map(|m| m.as_deref()),
        )
    });

    // Callbacks for the header bar
    let on_type_change = Callback::new(move |t: String| {
        set_type_override.set(Some(t));
    });
    let on_orientation_change = Callback::new(move |o: Option<String>| {
        set_orientation_override.set(Some(o));
    });
    let on_mode_change = Callback::new(move |m: Option<String>| {
        set_mode_override.set(Some(m));
    });

    let yaml_for_info = yaml.clone();
    let on_info = Callback::new(move |()| {
        dispatch_chart_info_event(&yaml_for_info);
    });

    // Edit dispatch carries enough info for the listener to locate this
    // chart in the editor source:
    //   - `yaml` — the per-item yaml to populate the chart builder
    //   - `block_content` — the full block's content (fingerprint used to
    //     find `block_index` by matching against the source's ```chartml fences)
    //   - `array_index` — which item within the block was clicked
    let yaml_for_edit = yaml.clone();
    let block_content_for_edit = block_content.clone();
    let on_edit = Callback::new(move |()| {
        dispatch_chart_edit_event(&yaml_for_edit, &block_content_for_edit, array_index);
    });

    // Last refreshed tracking — set_last_refreshed used in cfg(wasm32) blocks
    let (last_refreshed, set_last_refreshed) = signal(None::<f64>);
    let (is_refreshing, _set_is_refreshing) = signal(false);

    // Set initial timestamp and build refresh callback
    #[cfg(target_arch = "wasm32")]
    {
        set_last_refreshed.set(Some(js_sys::Date::now()));
    }
    let on_refresh = Callback::new(move |()| {
        // Inline charts don't fetch remote data — just update the timestamp
        set_last_refreshed.set(Some(
            #[cfg(target_arch = "wasm32")]
            {
                js_sys::Date::now()
            },
            #[cfg(not(target_arch = "wasm32"))]
            {
                0.0
            },
        ));
    });

    // Store callbacks for use inside the reactive header closure
    let on_type_stored = StoredValue::new(on_type_change);
    let on_orient_stored = StoredValue::new(on_orientation_change);
    let on_mode_stored = StoredValue::new(on_mode_change);
    let on_info_stored = StoredValue::new(on_info);
    let on_edit_stored = StoredValue::new(on_edit);
    let on_refresh_stored = StoredValue::new(on_refresh);

    view! {
        <div class="chart-card">
            // Reactive header bar — re-renders when type/orientation/mode change
            {move || {
                let ct = current_chart_type.get();
                let co = current_orientation.get();
                let cm = current_mode.get();
                let type_cb = on_type_stored.get_value();
                let orient_cb = on_orient_stored.get_value();
                let mode_cb = on_mode_stored.get_value();
                let info_cb = on_info_stored.get_value();
                let edit_cb = on_edit_stored.get_value();
                let refresh_cb = on_refresh_stored.get_value();
                let last_sig = Signal::derive(move || last_refreshed.get());
                let refreshing_sig = Signal::derive(move || is_refreshing.get());
                view! {
                    <ChartHeaderBar
                        chart_type=ct.unwrap_or_default()
                        chart_orientation=co.unwrap_or_default()
                        chart_mode=cm.unwrap_or_default()
                        show_type_selector=true
                        show_refresh=true
                        show_info=true
                        show_edit=true
                        on_type_change=type_cb
                        on_orientation_change=orient_cb
                        on_mode_change=mode_cb
                        on_info=info_cb
                        on_edit=edit_cb
                        on_refresh=refresh_cb
                        last_updated=last_sig
                        is_refreshing=refreshing_sig
                    />
                }
            }}
            <ChartMLChart
                spec=Signal::derive(move || effective_spec.get())
                chartml=chartml
            />
        </div>
    }
    .into_any()
}

/// Dispatch a `chart-info-request` CustomEvent carrying raw YAML in `detail`.
/// The dashboard editor's listener reads `ev.detail()` as a string and opens
/// the chart info modal. No structured payload is needed — info is read-only.
fn dispatch_chart_info_event(yaml: &str) {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let detail = wasm_bindgen::JsValue::from_str(yaml);
        let init = web_sys::CustomEventInit::new();
        init.set_detail(&detail);
        if let Ok(event) =
            web_sys::CustomEvent::new_with_event_init_dict("chart-info-request", &init)
        {
            let _ = window.dispatch_event(&event);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = yaml;
    }
}

/// Dispatch a `chart-edit-request` CustomEvent carrying a structured JSON
/// payload with enough information for the dashboard editor's listener to
/// locate the exact chart in the source and open the chart builder with the
/// correct per-item YAML.
///
/// Payload shape (JSON-stringified in `detail`):
/// ```json
/// { "yaml": "...", "block_content": "...", "array_index": 0 }
/// ```
/// - `yaml`: the per-item YAML to populate the chart builder
/// - `block_content`: the full block's YAML (fingerprint for the listener to
///   find `block_index` by matching against ```chartml fences in the source)
/// - `array_index`: which item within the block was clicked (0 for mapping blocks)
fn dispatch_chart_edit_event(yaml: &str, block_content: &str, array_index: usize) {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let payload = serde_json::json!({
            "yaml": yaml,
            "block_content": block_content,
            "array_index": array_index,
        });
        let json = payload.to_string();
        let detail = wasm_bindgen::JsValue::from_str(&json);
        let init = web_sys::CustomEventInit::new();
        init.set_detail(&detail);
        if let Ok(event) =
            web_sys::CustomEvent::new_with_event_init_dict("chart-edit-request", &init)
        {
            let _ = window.dispatch_event(&event);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (yaml, block_content, array_index);
    }
}
