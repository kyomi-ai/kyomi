// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared `KyomiChart` component — single source of truth for rendering one
//! ChartML chart with its header chrome (type selector, refresh, action menu).
//!
//! The markdown-renderer path delegates here via `MarkdownRenderer` in
//! `markdown_renderer.rs` (Phase 3 migration complete).  The WYSIWYG
//! extension path (`render_one_chart` in `chartml_extension.rs`) will
//! delegate here once Phase 4 lands.
//!
//! # Responsibilities
//!
//! - YAML parsing for initial `chart_type`, `chart_orientation`, `chart_mode`,
//!   `chart_height_px` (per-type default fallback via
//!   [`default_chart_height_for_type`]).
//! - Override signals (`chart_type_override`, `chart_orientation_override`,
//!   `chart_mode_override`) for the header bar selectors.
//! - Effective-YAML [`Memo`] — applies overrides then substitutes
//!   `{{param}}` placeholders from the `parameters` signal (parity fix #9).
//! - Per-chart [`RwSignal<u32>`] refresh trigger; folded with the optional
//!   context [`RefreshAllSignal`] (parity fix #17).
//! - `last_refreshed` timestamp driven by spec / refresh / parameter
//!   input ticks, not by first render alone (parity fix #18).
//! - [`ChartHeaderBar`] with `show_*` flags derived from `Option::is_some()`
//!   on each action callback (parity fix #13).
//! - [`ChartMLChart`] with `min-height` reservation derived from
//!   `chart_height_px` (parity fix #11 / #23 / #29).
//!
//! # What is NOT here
//!
//! `colSpan` layout wrapping is the caller's responsibility. The component
//! emits a single `<div class="chart-card">` with no outer grid wrapper.

use std::collections::HashMap;

use chartml_leptos::ChartMLChart;
use leptos::prelude::*;

use crate::chartml_provider::RefreshAllSignal;
use crate::components::dashboard::chart_header_bar::ChartHeaderBar;
use crate::components::dashboard::markdown_renderer::{
    apply_spec_overrides, default_chart_height_for_type, extract_chart_height, extract_chart_mode,
    extract_chart_orientation, extract_chart_type, substitute_params,
};

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Render a single ChartML chart with its header chrome.
///
/// Owns its own override/refresh signals so per-chart controls (type selector,
/// orientation/mode chips, refresh button) act independently from sibling
/// charts in the same fenced block.
///
/// The `colSpan` layout wrapper is the caller's responsibility — this
/// component only emits `<div class="chart-card">…</div>`.
///
/// # Parity guarantees
///
/// This component is the shared implementation that resolves the divergences
/// found during the chartml parity audit:
///
/// | Audit item | Behaviour |
/// | --- | --- |
/// | #9  | `parameters` signal substituted into effective YAML |
/// | #11 | `min-height` wrapper derived from `chart_height_px` |
/// | #13 | `show_*` flags reflect callback `Option::is_some()` |
/// | #17 | `RefreshAllSignal` context folded into per-chart refresh trigger |
/// | #18 | `last_refreshed` driven by spec/refresh/param input ticks |
/// | #23 | Same as #11 — loading placeholder sits inside reserved space |
/// | #29 | `default_chart_height_for_type` used for per-type height reservation |
#[component]
pub fn KyomiChart(
    /// Per-chart YAML spec (already split out of a sequence block by the caller).
    #[prop(into)]
    yaml: String,
    /// Zero-based index of the chartml fenced block within the document.
    block_index: usize,
    /// Zero-based index of this chart within its block (0 for single-chart blocks).
    array_index: usize,
    /// Dashboard parameter values for `{{param}}` substitution.
    #[prop(into)]
    parameters: Signal<HashMap<String, String>>,
    /// Pre-configured [`chartml_leptos::ChartMLRef`] — built by the caller via
    /// [`crate::chartml_provider::configured_chartml`] so palette, theme, and
    /// renderers are resolved once at the block level rather than per-chart.
    #[prop(into)]
    chartml: chartml_leptos::ChartMLRef,
    /// "Edit" action callback — receives `(block_index, array_index)`.
    /// `None` hides the edit button in the header bar.
    on_edit_chart: Option<Callback<(usize, usize)>>,
    /// "Delete" action callback — receives `(block_index, array_index)`.
    /// `None` hides the delete button in the header bar.
    on_delete_chart: Option<Callback<(usize, usize)>>,
    /// "Save to dashboard" callback — receives chart YAML wrapped in a
    /// ```` ```chartml ```` fence.  `None` hides the save button.
    on_save_to_dashboard: Option<Callback<String>>,
    /// "Chart info" callback — receives raw chart YAML.
    /// `None` hides the info button in the header bar.
    on_chart_info: Option<Callback<String>>,
    /// "Ask about this chart" callback — receives chart YAML wrapped in a
    /// ```` ```chartml ```` fence.  `None` hides the ask button.
    on_ask_about_chart: Option<Callback<String>>,
    /// Callback invoked with the new chart type string when the user changes
    /// the type via the header bar dropdown. Used by the WYSIWYG editor to
    /// persist type changes to the dashboard source. `None` in the viewer.
    #[prop(optional)]
    on_type_change_persist: Option<Callback<String>>,
) -> impl IntoView {
    // ------------------------------------------------------------------
    // 1. Parse YAML for initial chrome metadata
    // ------------------------------------------------------------------
    // Errors are non-fatal — the YAML still flows to ChartMLChart which
    // surfaces parse failures to the user with full context.
    let parsed_spec: Option<serde_json::Value> = serde_yaml::from_str(&yaml).ok();

    let initial_chart_type = parsed_spec.as_ref().and_then(extract_chart_type);
    let initial_orientation = parsed_spec.as_ref().and_then(extract_chart_orientation);
    let initial_mode = parsed_spec.as_ref().and_then(extract_chart_mode);

    // Reserve the rendered chart's height on the outer wrapper so the
    // ChartMLChart loading placeholder doesn't cause a layout shift when data
    // arrives. Falls back to chartml's per-type default (150px for metric
    // cards, 400px for cartesian / pie / scatter / table) when the spec omits
    // an explicit height. (Parity fix #11 / #23 / #29.)
    let chart_height_px = parsed_spec
        .as_ref()
        .and_then(extract_chart_height)
        .unwrap_or_else(|| default_chart_height_for_type(initial_chart_type.as_deref()));

    // ------------------------------------------------------------------
    // 2. Override signals
    // ------------------------------------------------------------------
    let (type_override, set_type_override) = signal(None::<String>);
    let (orientation_override, set_orientation_override) = signal(None::<Option<String>>);
    let (mode_override, set_mode_override) = signal(None::<Option<String>>);

    // ------------------------------------------------------------------
    // 3. Per-chart refresh signal + dashboard-wide RefreshAllSignal fold
    // ------------------------------------------------------------------
    // Per-chart "Refresh" button bumps `local_refresh`. The optional
    // dashboard-wide `RefreshAllSignal` from Leptos context (provided by the
    // dashboard viewer; absent in the editor / chart-builder preview) is
    // folded into `combined_refresh` by addition so any bump on either
    // source produces a distinct value. (Parity fix #16 / #17.)
    let local_refresh = RwSignal::new(0_u32);

    let refresh_all = use_context::<RefreshAllSignal>();

    let combined_refresh = Signal::derive(move || {
        let l = local_refresh.try_get().unwrap_or(0);
        let g = refresh_all.and_then(|s| s.try_get()).unwrap_or(0);
        l.wrapping_add(g)
    });

    // ------------------------------------------------------------------
    // 4. Derived current type / orientation / mode for the header bar
    // ------------------------------------------------------------------
    let initial_type_stored = StoredValue::new(initial_chart_type.clone());
    let initial_orient_stored = StoredValue::new(initial_orientation.clone());
    let initial_mode_stored = StoredValue::new(initial_mode.clone());

    let current_chart_type = Memo::new(move |_| {
        type_override
            .try_get()
            .flatten()
            .or_else(|| initial_type_stored.try_get_value().flatten())
    });
    let current_orientation = Memo::new(move |_| match orientation_override.try_get().flatten() {
        Some(o) => o,
        None => initial_orient_stored.try_get_value().flatten(),
    });
    let current_mode = Memo::new(move |_| match mode_override.try_get().flatten() {
        Some(m) => m,
        None => initial_mode_stored.try_get_value().flatten(),
    });

    // ------------------------------------------------------------------
    // 5. Effective-YAML Memo — overrides + parameter substitution
    // ------------------------------------------------------------------
    // Re-runs whenever any override signal or the parameters signal changes.
    // Parameter substitution (parity fix #9) happens here so `{{region}}`
    // placeholders reach the datasource as resolved values, matching the
    // dashboard viewer's behavior.
    let yaml_for_spec = yaml.clone();
    let effective_yaml = Memo::new(move |_| {
        let t_ovr = type_override.try_get().flatten();
        let o_ovr = orientation_override.try_get().flatten();
        let m_ovr = mode_override.try_get().flatten();

        let with_overrides = if t_ovr.is_none() && o_ovr.is_none() && m_ovr.is_none() {
            yaml_for_spec.clone()
        } else {
            apply_spec_overrides(
                &yaml_for_spec,
                t_ovr.as_deref(),
                o_ovr.as_ref().map(|o| o.as_deref()),
                m_ovr.as_ref().map(|m| m.as_deref()),
            )
        };

        let params = parameters.try_get().unwrap_or_default();
        if params.is_empty() {
            with_overrides
        } else {
            substitute_params(&with_overrides, &params)
        }
    });

    let spec_signal = Signal::derive(move || effective_yaml.try_get().unwrap_or_default());

    // ------------------------------------------------------------------
    // 6. Last-refreshed timestamp (parity fix #18)
    // ------------------------------------------------------------------
    // Driven by `ChartMLChart`'s `on_refreshed` callback after each
    // fetch + transform + render completes. On cache hits this reflects
    // when the data was originally fetched from the server, not "now".
    let (last_refreshed, set_last_refreshed) = signal(None::<f64>);
    let on_refreshed_cb = Callback::new(move |ms: f64| {
        set_last_refreshed.try_set(Some(ms));
    });

    // ------------------------------------------------------------------
    // 7. Determine which header-bar features to show (parity fix #13)
    // ------------------------------------------------------------------
    let has_edit = on_edit_chart.is_some();
    let has_delete = on_delete_chart.is_some();
    let has_save = on_save_to_dashboard.is_some();
    let has_info = on_chart_info.is_some();
    let has_ask = on_ask_about_chart.is_some();

    // ------------------------------------------------------------------
    // 8. Store callbacks for use inside reactive closures
    // ------------------------------------------------------------------
    let yaml_for_save = yaml.clone();
    let yaml_for_info = yaml.clone();
    let yaml_for_ask = yaml.clone();

    let edit_cb = StoredValue::new(on_edit_chart);
    let delete_cb = StoredValue::new(on_delete_chart);
    let save_cb = StoredValue::new(on_save_to_dashboard);
    let info_cb = StoredValue::new(on_chart_info);
    let ask_cb = StoredValue::new(on_ask_about_chart);
    let yaml_for_save_stored = StoredValue::new(yaml_for_save);
    let yaml_for_info_stored = StoredValue::new(yaml_for_info);
    let yaml_for_ask_stored = StoredValue::new(yaml_for_ask);

    // ------------------------------------------------------------------
    // 9. Build header-bar callbacks
    // ------------------------------------------------------------------
    let on_type_change_cb = Callback::new(move |t: String| {
        set_type_override.set(Some(t.clone()));
        if let Some(cb) = on_type_change_persist {
            cb.run(t);
        }
    });
    let on_orientation_change_cb = Callback::new(move |o: Option<String>| {
        set_orientation_override.set(Some(o));
    });
    let on_mode_change_cb = Callback::new(move |m: Option<String>| {
        set_mode_override.set(Some(m));
    });
    // Per-chart refresh — bumps local_refresh which feeds through
    // combined_refresh into ChartMLChart's refresh_trigger. The chartml
    // component owns the actual cache invalidation + re-fetch; this side
    // just owns the trigger counter.
    let on_refresh_cb = Callback::new(move |()| {
        local_refresh.update(|c| *c += 1);
    });

    let on_edit_cb = {
        let bi = block_index;
        let ai = array_index;
        Callback::new(move |()| {
            if let Some(cb) = edit_cb.try_get_value().flatten() {
                cb.run((bi, ai));
            }
        })
    };
    let on_delete_cb = {
        let bi = block_index;
        let ai = array_index;
        Callback::new(move |()| {
            if let Some(cb) = delete_cb.try_get_value().flatten() {
                cb.run((bi, ai));
            }
        })
    };
    let on_save_cb = Callback::new(move |()| {
        if let Some(cb) = save_cb.try_get_value().flatten() {
            let Some(chart_yaml) = yaml_for_save_stored.try_get_value() else { return };
            let chart_md = format!("```chartml\n{}\n```", chart_yaml);
            cb.run(chart_md);
        }
    });
    let on_info_cb = Callback::new(move |()| {
        if let Some(cb) = info_cb.try_get_value().flatten() {
            let Some(chart_yaml) = yaml_for_info_stored.try_get_value() else { return };
            cb.run(chart_yaml);
        }
    });
    let on_ask_cb = Callback::new(move |()| {
        if let Some(cb) = ask_cb.try_get_value().flatten() {
            let Some(chart_yaml) = yaml_for_ask_stored.try_get_value() else { return };
            let chart_md = format!("```chartml\n{}\n```", chart_yaml);
            cb.run(chart_md);
        }
    });

    // Store all callbacks in `StoredValue` so they can be moved into the
    // reactive header closure without borrow-checker trouble.
    let on_type_change_stored = StoredValue::new(on_type_change_cb);
    let on_orientation_change_stored = StoredValue::new(on_orientation_change_cb);
    let on_mode_change_stored = StoredValue::new(on_mode_change_cb);
    let on_refresh_stored = StoredValue::new(on_refresh_cb);
    let on_edit_stored = StoredValue::new(on_edit_cb);
    let on_delete_stored = StoredValue::new(on_delete_cb);
    let on_save_stored = StoredValue::new(on_save_cb);
    let on_info_stored = StoredValue::new(on_info_cb);
    let on_ask_stored = StoredValue::new(on_ask_cb);

    // ------------------------------------------------------------------
    // 10. View — chart-card wrapper, header bar, chart (with height reserve)
    // ------------------------------------------------------------------
    view! {
        <div class="chart-card">
            // Header bar re-renders when type/orientation/mode override signals
            // change. All other callbacks are stable `StoredValue`s.
            {move || {
                let ct = current_chart_type.try_get().flatten();
                let co = current_orientation.try_get().unwrap_or_default();
                let cm = current_mode.try_get().unwrap_or_default();

                let Some(type_cb) = on_type_change_stored.try_get_value() else { return view! { <div/> }.into_any(); };
                let Some(orient_cb) = on_orientation_change_stored.try_get_value() else { return view! { <div/> }.into_any(); };
                let Some(mode_cb) = on_mode_change_stored.try_get_value() else { return view! { <div/> }.into_any(); };
                let Some(refresh_cb) = on_refresh_stored.try_get_value() else { return view! { <div/> }.into_any(); };
                let Some(edit_action) = on_edit_stored.try_get_value() else { return view! { <div/> }.into_any(); };
                let Some(delete_action) = on_delete_stored.try_get_value() else { return view! { <div/> }.into_any(); };
                let Some(save_action) = on_save_stored.try_get_value() else { return view! { <div/> }.into_any(); };
                let Some(info_action) = on_info_stored.try_get_value() else { return view! { <div/> }.into_any(); };
                let Some(ask_action) = on_ask_stored.try_get_value() else { return view! { <div/> }.into_any(); };
                let last_sig = Signal::derive(move || last_refreshed.try_get().flatten());
                let refreshing_sig = Signal::derive(|| false);
                view! {
                    <ChartHeaderBar
                        chart_type=ct.unwrap_or_default()
                        chart_orientation=co.unwrap_or_default()
                        chart_mode=cm.unwrap_or_default()
                        show_type_selector=true
                        show_refresh=true
                        show_edit=has_edit
                        show_delete=has_delete
                        show_save_to_dashboard=has_save
                        show_info=has_info
                        show_ask_about=has_ask
                        on_type_change=type_cb
                        on_orientation_change=orient_cb
                        on_mode_change=mode_cb
                        on_refresh=refresh_cb
                        on_edit=edit_action
                        on_delete=delete_action
                        on_save_to_dashboard=save_action
                        on_info=info_action
                        on_ask_about=ask_action
                        last_updated=last_sig
                        is_refreshing=refreshing_sig
                    />
                }.into_any()
            }}
            // `min-height` reserves the rendered chart's vertical space so the
            // ChartMLChart loading placeholder doesn't cause a layout shift when
            // data arrives. (Parity fix #11 / #23 / #29.)
            <div class="w-full flex flex-col" style=format!("min-height: {}px", chart_height_px)>
                <ChartMLChart
                    spec=spec_signal
                    chartml=chartml
                    refresh_trigger=combined_refresh
                    on_refreshed=on_refreshed_cb
                />
            </div>
        </div>
    }
}
