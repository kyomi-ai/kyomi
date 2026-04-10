// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart Builder Modal — Leptos port of
//! `apps/frontend/src/components/ChartBuilderModal.jsx`.
//!
//! Two-screen flow:
//! 1. **SQL Editor** — datasource selection, SQL query, catalog sidebar
//! 2. **Chart Config** — split pane: left editing panel (Visual / AI / YAML sub-tabs),
//!    right live preview
//!
//! React reference: `ChartBuilderModal.jsx`, `ChartVisualEditor.jsx`,
//! `ChartMLConfigEditor.jsx`, `ChartBuilderCopilotSidebar.jsx`.

use std::sync::Arc;

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::chat::CopilotChat;
use crate::components::input::INPUT_CLASS;
use crate::components::modal::{Modal, ModalSize};
use crate::components::select::DynSelect;
use crate::components::Spinner;
use crate::pages::sql_editor::catalog_tree::CatalogTree;
use crate::pages::sql_editor::results_table::ResultsTable;
use crate::pages::sql_editor::types::QueryResult;
use crate::server_fns::datasources::{list_datasources, query_datasource_arrow, DatasourceInfo};
use crate::server_fns::sql_editor::execute_sql_query;

use super::markdown_renderer::{configured_chartml, kyomi_palette};
use super::shared::{BTN_BASE, BTN_DEFAULT, BTN_SIZE};

/// Label classes — matches the design system (uppercase tracking-wide like React).
const LABEL_CLASS: &str =
    "text-xs font-medium text-muted-foreground uppercase tracking-wide";

/// Chart type options matching the React `CHART_TYPE_OPTIONS` in
/// `ChartVisualEditor.jsx` and the ChartML spec.
const CHART_TYPES: &[(&str, &str)] = &[
    ("bar", "Bar"),
    ("line", "Line"),
    ("area", "Area"),
    ("scatter", "Scatter"),
    ("pie", "Pie"),
    ("doughnut", "Doughnut"),
    ("table", "Table"),
    ("metric", "Metric"),
];

// ─── Series entry ───────────────────────────────────────────────────────────

/// A single Y-axis series entry.
#[derive(Clone, Debug)]
struct SeriesEntry {
    /// Unique ID for stable keying in `<For>` loops.
    id: u32,
    y_field: String,
    label: String,
}

// ─── YAML parsing (for editing existing charts) ─────────────────────────────

/// Parsed chart configuration extracted from existing ChartML YAML.
#[derive(Clone, Debug, Default)]
struct ParsedChart {
    title: String,
    datasource_slug: String,
    sql: String,
    chart_type: String,
    x_field: String,
    orientation: Option<String>,
    mode: Option<String>,
    series: Vec<SeriesEntry>,
}

/// Attempt to parse a ChartML YAML string into a `ParsedChart`.
/// Falls back to defaults for any missing fields.
fn parse_existing_yaml(yaml: &str) -> ParsedChart {
    let mut chart = ParsedChart::default();

    // Parse YAML — handle both single doc and list (ChartML wraps in a list)
    let docs: Vec<serde_yaml::Value> = match serde_yaml::from_str::<serde_yaml::Value>(yaml) {
        Ok(serde_yaml::Value::Sequence(seq)) => seq,
        Ok(val) => vec![val],
        Err(_) => return chart,
    };

    // Take the first chart-type document
    let doc = docs
        .iter()
        .find(|d| {
            d.get("type")
                .and_then(|t| t.as_str()) == Some("chart")
        })
        .or(docs.first());

    let Some(doc) = doc else {
        return chart;
    };

    // Top-level title (React stores it here, not under visualize.style)
    if let Some(title) = doc.get("title").and_then(|v| v.as_str()) {
        chart.title = title.to_string();
    }

    if let Some(data) = doc.get("data") {
        if let Some(ds) = data.get("datasource").and_then(|v| v.as_str()) {
            chart.datasource_slug = ds.to_string();
        }
        // Support both "sql" and "query" field names
        let sql_val = data
            .get("sql")
            .or_else(|| data.get("query"))
            .and_then(|v| v.as_str());
        if let Some(sql) = sql_val {
            chart.sql = sql.to_string();
        }
    }

    if let Some(vis) = doc.get("visualize") {
        if let Some(ct) = vis.get("type").and_then(|v| v.as_str()) {
            chart.chart_type = ct.to_string();
        }
        if let Some(x) = vis.get("columns").and_then(|v| v.as_str()) {
            chart.x_field = x.to_string();
        }
        if let Some(o) = vis.get("orientation").and_then(|v| v.as_str()) {
            chart.orientation = Some(o.to_string());
        }
        if let Some(m) = vis.get("mode").and_then(|v| v.as_str()) {
            chart.mode = Some(m.to_string());
        }

        // Title lives under visualize.style.title
        if let Some(style) = vis.get("style")
            && let Some(title) = style.get("title").and_then(|v| v.as_str())
        {
            chart.title = title.to_string();
        }

        // Parse rows — can be a bare string or a sequence of {field, label} objects
        if let Some(rows_val) = vis.get("rows") {
            match rows_val {
                serde_yaml::Value::String(s) => {
                    // Single row as bare string
                    chart.series.push(SeriesEntry {
                        id: 0,
                        y_field: s.clone(),
                        label: String::new(),
                    });
                }
                serde_yaml::Value::Sequence(rows_seq) => {
                    for (i, entry) in rows_seq.iter().enumerate() {
                        match entry {
                            serde_yaml::Value::String(s) => {
                                chart.series.push(SeriesEntry {
                                    id: i as u32,
                                    y_field: s.clone(),
                                    label: String::new(),
                                });
                            }
                            _ => {
                                let field = entry
                                    .get("field")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string();
                                let label = entry
                                    .get("label")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default()
                                    .to_string();
                                chart.series.push(SeriesEntry {
                                    id: i as u32,
                                    y_field: field,
                                    label,
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    chart
}

// ─── Chart form state snapshot ──────────────────────────────────────────────

/// Snapshot of the form state for YAML generation/patching.
/// Groups the parameters that `patch_yaml` and `build_yaml` share.
struct ChartFormState<'a> {
    title: &'a str,
    datasource_slug: &'a str,
    sql: &'a str,
    chart_type: &'a str,
    x_field: &'a str,
    orientation: Option<&'a str>,
    mode: Option<&'a str>,
    series: &'a [SeriesEntry],
}

// ─── YAML patching ──────────────────────────────────────────────────────────

/// Patch an existing ChartML YAML string with form-state changes.
///
/// Preserves all fields not modeled by the form (inline data, provider,
/// cache, etc.) — only updates the fields the form controls.
/// Falls back to `build_yaml` if the original YAML is empty or unparseable.
fn patch_yaml(original: &str, f: &ChartFormState<'_>) -> String {
    if original.trim().is_empty() {
        return build_yaml(f);
    }

    let mut val: serde_yaml::Value = match serde_yaml::from_str(original) {
        Ok(v) => v,
        Err(_) => return build_yaml(f),
    };

    // Get the chart document — handle both list and single-doc format
    let doc = if let Some(seq) = val.as_sequence_mut() {
        seq.iter_mut().find(|d| {
            d.get("type").and_then(|t| t.as_str()) == Some("chart")
        })
    } else {
        Some(&mut val)
    };

    let Some(doc) = doc else {
        return build_yaml(f);
    };

    // Patch top-level title
    if !f.title.is_empty() {
        doc["title"] = serde_yaml::Value::String(f.title.to_string());
    }

    // Patch data section — only if datasource/sql are non-empty (remote chart).
    // Don't overwrite inline data with empty datasource/sql.
    if !f.datasource_slug.is_empty() {
        if doc.get("data").is_none() {
            doc["data"] = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
        }
        doc["data"]["datasource"] = serde_yaml::Value::String(f.datasource_slug.to_string());
        if !f.sql.is_empty() {
            doc["data"]["sql"] = serde_yaml::Value::String(f.sql.to_string());
        }
    }

    // Patch visualize section
    if doc.get("visualize").is_none() {
        doc["visualize"] = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }

    doc["visualize"]["type"] = serde_yaml::Value::String(f.chart_type.to_string());

    // Orientation
    if let Some(orient) = f.orientation {
        doc["visualize"]["orientation"] = serde_yaml::Value::String(orient.to_string());
    } else if let Some(vis) = doc.get_mut("visualize").and_then(|v| v.as_mapping_mut()) {
        vis.remove(serde_yaml::Value::String("orientation".to_string()));
    }

    // Mode
    if let Some(m) = f.mode {
        doc["visualize"]["mode"] = serde_yaml::Value::String(m.to_string());
    } else if let Some(vis) = doc.get_mut("visualize").and_then(|v| v.as_mapping_mut()) {
        vis.remove(serde_yaml::Value::String("mode".to_string()));
    }

    // Columns
    let needs_axes = !matches!(f.chart_type, "metric" | "pie" | "doughnut");
    if needs_axes && !f.x_field.is_empty() {
        doc["visualize"]["columns"] = serde_yaml::Value::String(f.x_field.to_string());
    }

    // Rows (series)
    let non_empty_series: Vec<&SeriesEntry> =
        f.series.iter().filter(|s| !s.y_field.is_empty()).collect();
    if !non_empty_series.is_empty() {
        let rows: Vec<serde_yaml::Value> = non_empty_series.iter().map(|s| {
            if s.label.is_empty() {
                serde_yaml::Value::String(s.y_field.clone())
            } else {
                let mut map = serde_yaml::Mapping::new();
                map.insert(
                    serde_yaml::Value::String("field".to_string()),
                    serde_yaml::Value::String(s.y_field.clone()),
                );
                map.insert(
                    serde_yaml::Value::String("label".to_string()),
                    serde_yaml::Value::String(s.label.clone()),
                );
                serde_yaml::Value::Mapping(map)
            }
        }).collect();

        if rows.len() == 1 && matches!(&rows[0], serde_yaml::Value::String(_)) {
            doc["visualize"]["rows"] = rows.into_iter().next().unwrap();
        } else {
            doc["visualize"]["rows"] = serde_yaml::Value::Sequence(rows);
        }
    }

    serde_yaml::to_string(&val).unwrap_or_else(|_| original.to_string())
}

// ─── YAML generation (new charts only) ──────────────────────────────────────

/// Build a ChartML YAML string from the form state.
/// Used only for NEW charts — existing charts use `patch_yaml` to preserve data.
fn build_yaml(f: &ChartFormState<'_>) -> String {
    // Indent SQL lines for YAML block scalar
    let sql_indented = f.sql
        .lines()
        .map(|line| format!("      {line}"))
        .collect::<Vec<_>>()
        .join("\n");

    let datasource_slug = f.datasource_slug;
    let chart_type = f.chart_type;

    let mut yaml = format!(
        r#"- type: chart
  version: 1
  data:
    datasource: "{datasource_slug}"
    sql: |
{sql_indented}
  visualize:
    type: {chart_type}"#
    );

    if let Some(orient) = f.orientation {
        yaml.push_str(&format!("\n    orientation: {orient}"));
    }

    if let Some(m) = f.mode {
        yaml.push_str(&format!("\n    mode: {m}"));
    }

    let needs_axes = !matches!(f.chart_type, "metric" | "pie" | "doughnut");
    if needs_axes && !f.x_field.is_empty() {
        yaml.push_str(&format!("\n    columns: {}", f.x_field));
    }

    let non_empty_series: Vec<&SeriesEntry> =
        f.series.iter().filter(|s| !s.y_field.is_empty()).collect();
    if !non_empty_series.is_empty() {
        if non_empty_series.len() == 1 && non_empty_series[0].label.is_empty() {
            yaml.push_str(&format!("\n    rows: {}", non_empty_series[0].y_field));
        } else {
            yaml.push_str("\n    rows:");
            for entry in &non_empty_series {
                yaml.push_str(&format!("\n      - field: {}", entry.y_field));
                if !entry.label.is_empty() {
                    yaml.push_str(&format!("\n        label: \"{}\"", entry.label));
                }
            }
        }
    }

    if !f.title.is_empty() {
        yaml.push_str(&format!("\n    style:\n      title: \"{}\"", f.title));
    }

    yaml.push('\n');
    yaml
}

// ─── Component ──────────────────────────────────────────────────────────────

/// Chart Builder Modal — create or edit a ChartML chart.
///
/// React reference: `apps/frontend/src/components/ChartBuilderModal.jsx`
///
/// Features:
/// - SQL Editor tab with datasource selector, catalog sidebar, query execution
/// - Chart Config tab with 50/50 split pane:
///   - Left: Visual / AI / YAML sub-tabs
///   - Right: live chart preview
/// - Chart type modifier chips (Horizontal, Grouped, Normalized)
#[component]
pub fn ChartBuilderModal(
    /// Whether the modal is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Existing chart YAML to edit (None for new chart).
    #[prop(optional)]
    existing_yaml: Option<String>,
    /// Callback to close the modal.
    on_close: Callback<()>,
    /// Callback with the generated/updated ChartML YAML.
    on_insert: Callback<String>,
) -> impl IntoView {
    let is_edit_mode = existing_yaml.is_some();
    let existing_yaml_stored = StoredValue::new(existing_yaml.clone());

    // The original YAML — used as the base for patching in edit mode.
    // New charts start with an empty string (build_yaml generates from scratch).
    let (original_yaml, set_original_yaml) = signal(
        existing_yaml.clone().unwrap_or_default()
    );

    // Parse existing YAML or use defaults
    let initial = existing_yaml
        .as_ref()
        .map(|y| parse_existing_yaml(y))
        .unwrap_or_default();

    // ── Unique ID counter for series entries ─────────────────────────────
    let (next_series_id, set_next_series_id) = signal(
        initial.series.iter().map(|s| s.id).max().map(|m| m + 1).unwrap_or(1)
    );

    // ── Form state ──────────────────────────────────────────────────────

    let (title, set_title) = signal(if initial.title.is_empty() {
        "New Chart".to_string()
    } else {
        initial.title.clone()
    });

    let (datasource_slug, set_datasource_slug) = signal(initial.datasource_slug.clone());

    let (sql, set_sql) = signal(initial.sql.clone());

    let (chart_type, set_chart_type) = signal(if initial.chart_type.is_empty() {
        "bar".to_string()
    } else {
        initial.chart_type.clone()
    });

    let (x_field, set_x_field) = signal(initial.x_field.clone());

    let (orientation, set_orientation) = signal(initial.orientation.clone());
    let (mode, set_mode) = signal(initial.mode.clone());

    let initial_series = if initial.series.is_empty() {
        vec![SeriesEntry {
            id: 0,
            y_field: String::new(),
            label: String::new(),
        }]
    } else {
        initial.series.clone()
    };
    let (series, set_series) = signal(initial_series);

    // ── YAML editor state (separate from form, synced on sub-tab switch) ──
    let (yaml_text, set_yaml_text) = signal(String::new());

    // ── Reset form state when modal opens with different yaml ────────────
    Effect::new(move || {
        if open.get() {
            let yaml = existing_yaml_stored.get_value();

            // Store original YAML for patching
            set_original_yaml.set(yaml.clone().unwrap_or_default());

            let parsed = yaml
                .as_ref()
                .map(|y| parse_existing_yaml(y))
                .unwrap_or_default();

            set_title.set(if parsed.title.is_empty() {
                "New Chart".to_string()
            } else {
                parsed.title.clone()
            });
            set_datasource_slug.set(parsed.datasource_slug.clone());
            set_sql.set(parsed.sql.clone());
            set_chart_type.set(if parsed.chart_type.is_empty() {
                "bar".to_string()
            } else {
                parsed.chart_type.clone()
            });
            set_x_field.set(parsed.x_field.clone());
            set_orientation.set(parsed.orientation.clone());
            set_mode.set(parsed.mode.clone());

            if parsed.series.is_empty() {
                set_next_series_id.set(1);
                set_series.set(vec![SeriesEntry {
                    id: 0,
                    y_field: String::new(),
                    label: String::new(),
                }]);
            } else {
                set_next_series_id.set(
                    parsed.series.iter().map(|s| s.id).max().unwrap_or(0) + 1
                );
                set_series.set(parsed.series);
            }

            // Sync YAML text — use the original YAML directly for edit mode
            // so the user sees the actual chart spec, not a reconstruction
            set_yaml_text.set(yaml.unwrap_or_default());
        }
    });

    // ── Datasource options from server ──────────────────────────────────
    let datasources_resource = Resource::new(
        move || open.get(),
        move |is_open| async move {
            if !is_open {
                return Ok(Vec::<DatasourceInfo>::new());
            }
            list_datasources().await
        },
    );

    // Derive DynSelect options: (slug, "name (type)")
    let datasource_options = Signal::derive(move || {
        datasources_resource
            .get()
            .and_then(|res| res.ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|ds| ds.active)
            .map(|ds| {
                let label = format!("{} ({})", ds.name, ds.type_display_name);
                (ds.slug, label)
            })
            .collect::<Vec<(String, String)>>()
    });

    // ── Series management ───────────────────────────────────────────────

    let add_series = move |_: web_sys::MouseEvent| {
        let id = next_series_id.get_untracked();
        set_next_series_id.set(id + 1);
        set_series.update(|s| {
            s.push(SeriesEntry {
                id,
                y_field: String::new(),
                label: String::new(),
            });
        });
    };

    let remove_series = move |idx: usize| {
        set_series.update(|s| {
            if s.len() > 1 {
                s.remove(idx);
            }
        });
    };

    // ── Derived YAML from form state (for preview and YAML tab) ─────────
    // In edit mode, patches the original YAML to preserve inline data etc.
    // In create mode, builds from scratch.
    let current_yaml = Memo::new(move |_| {
        let t = title.get();
        let ds = datasource_slug.get();
        let s = sql.get();
        let ct = chart_type.get();
        let xf = x_field.get();
        let o = orientation.get();
        let m = mode.get();
        let sr = series.get();
        let form = ChartFormState {
            title: &t,
            datasource_slug: &ds,
            sql: &s,
            chart_type: &ct,
            x_field: &xf,
            orientation: o.as_deref(),
            mode: m.as_deref(),
            series: &sr,
        };
        patch_yaml(&original_yaml.get(), &form)
    });

    // ── Insert / Update handler ─────────────────────────────────────────

    let handle_insert = Callback::new(move |()| {
        let yaml = current_yaml.get_untracked();
        on_insert.run(yaml);
        on_close.run(());
    });

    // ── Footer ──────────────────────────────────────────────────────────

    // React: Cancel = ghost style, Save = primary style
    let cancel_class = format!("{BTN_BASE} text-foreground hover:text-foreground hover:bg-secondary {BTN_SIZE}");
    let insert_class = format!("{BTN_BASE} {BTN_DEFAULT} {BTN_SIZE}");

    let cancel_class_clone = cancel_class.clone();
    let insert_class_clone = insert_class.clone();

    let insert_label = if is_edit_mode {
        "Update Chart"
    } else {
        "Save Chart"
    };

    let footer_view: ChildrenFn = Arc::new(move || {
        let cancel_class = cancel_class_clone.clone();
        let insert_class = insert_class_clone.clone();

        // Disable insert: inline charts are always saveable, but remote charts
        // (datasource selected) require SQL to be useful.
        let is_disabled = if datasource_slug.get().is_empty() {
            false // inline chart — no SQL required
        } else {
            sql.get().trim().is_empty() // remote chart — SQL is required
        };

        view! {
            <button
                class=cancel_class
                on:click=move |_| on_close.run(())
            >
                "Cancel"
            </button>
            <button
                class=insert_class
                on:click=move |_| handle_insert.run(())
                disabled=is_disabled
            >
                {insert_label}
            </button>
        }
        .into_any()
    });

    // ── SQL Editor sub-component ────────────────────────────────────────
    // Rendered conditionally — kode-leptos requires wasm32.

    let sql_on_change: Arc<dyn Fn(String) + Send + Sync> =
        Arc::new(move |new_val: String| {
            set_sql.set(new_val);
        });
    let sql_on_change = StoredValue::new(sql_on_change);

    // ── YAML editor on_change ────────────────────────────────────────────
    // ── Shared: apply parsed YAML back into form signals ───────────────
    // Used by both the YAML editor on_change and the AI copilot on_chart_update.
    let apply_parsed = move |parsed: ParsedChart, yaml_str: &str| {
        if !parsed.chart_type.is_empty() {
            set_chart_type.set(parsed.chart_type);
        }
        if !parsed.title.is_empty() || yaml_str.contains("title:") {
            set_title.set(parsed.title);
        }
        set_x_field.set(parsed.x_field);
        set_orientation.set(parsed.orientation);
        set_mode.set(parsed.mode);
        if !parsed.datasource_slug.is_empty() {
            set_datasource_slug.set(parsed.datasource_slug);
        }
        if !parsed.sql.is_empty() {
            set_sql.set(parsed.sql);
        }
        if !parsed.series.is_empty() {
            set_next_series_id.set(
                parsed.series.iter().map(|s| s.id).max().unwrap_or(0) + 1
            );
            set_series.set(parsed.series);
        }
    };

    let yaml_on_change: Arc<dyn Fn(String) + Send + Sync> =
        Arc::new(move |new_val: String| {
            set_yaml_text.set(new_val.clone());
            // Update the base YAML so future form changes patch from this version
            set_original_yaml.set(new_val.clone());
            let parsed = parse_existing_yaml(&new_val);
            apply_parsed(parsed, &new_val);
        });
    let yaml_on_change = StoredValue::new(yaml_on_change);

    // ── Catalog sidebar state ──────────────────────────────────────────
    let (catalog_open, set_catalog_open) = signal(false);
    let (catalog_tab, set_catalog_tab) = signal("catalog".to_string());
    let (catalog_search, set_catalog_search) = signal(String::new());
    let (catalog_refresh_trigger, _set_catalog_refresh_trigger) = signal(0u32);

    // ── Query execution state ─────────────────────────────────────────
    let (query_running, set_query_running) = signal(false);
    let (query_result, set_query_result) = signal(None::<QueryResult>);
    let (query_error, set_query_error) = signal(None::<String>);

    // ── Preview state — remote data fetching ──────────────────────────
    let (preview_chartml, set_preview_chartml) =
        signal(None::<Arc<chartml_core::ChartML>>);
    let (preview_loading, set_preview_loading) = signal(false);
    let (preview_error, set_preview_error) = signal(None::<String>);

    // ── Auto-fetch preview data when opening with existing datasource + SQL ──
    #[cfg(target_arch = "wasm32")]
    {
        let initial_ds = initial.datasource_slug.clone();
        let initial_sql = initial.sql.clone();
        if !initial_ds.is_empty() && !initial_sql.trim().is_empty() {
            set_preview_loading.set(true);
            leptos::task::spawn_local(async move {
                match query_datasource_arrow(initial_ds, initial_sql, None).await {
                    Ok(query_result) => {
                        use base64::Engine;
                        match base64::engine::general_purpose::STANDARD
                            .decode(&query_result.ipc_base64)
                        {
                            Ok(ipc_bytes) => {
                                match chartml_core::data::DataTable::from_ipc_bytes(&ipc_bytes) {
                                    Ok(data_table) => {
                                        let colors = kyomi_palette("balanced");
                                        let mut chartml_inst = chartml_core::ChartML::new();
                                        chartml_inst.register_renderer("bar", chartml_chart_cartesian::CartesianRenderer::new());
                                        chartml_inst.register_renderer("line", chartml_chart_cartesian::CartesianRenderer::new());
                                        chartml_inst.register_renderer("area", chartml_chart_cartesian::CartesianRenderer::new());
                                        chartml_inst.register_renderer("pie", chartml_chart_pie::PieRenderer::new());
                                        chartml_inst.register_renderer("doughnut", chartml_chart_pie::PieRenderer::new());
                                        chartml_inst.register_renderer("scatter", chartml_chart_scatter::ScatterRenderer::new());
                                        chartml_inst.register_renderer("metric", chartml_chart_metric::MetricRenderer::new());
                                        chartml_inst.register_transform(chartml_datafusion::DataFusionTransform);
                                        chartml_inst.set_default_palette(colors);
                                        chartml_inst.register_source("_remote", data_table);
                                        set_preview_chartml.set(Some(Arc::new(chartml_inst)));
                                    }
                                    Err(e) => set_preview_error.set(Some(format!("Arrow decode error: {e}"))),
                                }
                            }
                            Err(e) => set_preview_error.set(Some(format!("Base64 decode error: {e}"))),
                        }
                        set_preview_loading.set(false);
                    }
                    Err(e) => {
                        set_preview_error.set(Some(format!("Query error: {e}")));
                        set_preview_loading.set(false);
                    }
                }
            });
        }
    }

    // ── View ────────────────────────────────────────────────────────────

    let modal_title = if is_edit_mode {
        format!("Chart Builder: {}", initial.title)
    } else {
        "Chart Builder: New Chart".to_string()
    };

    // ── Tab state ──────────────────────────────────────────────────────
    let (active_tab, set_active_tab) = signal(
        if is_edit_mode { "chart".to_string() } else { "sql".to_string() }
    );

    // ── Chart Config sub-tab state (Visual / AI / YAML) ────────────────
    let (config_tab, set_config_tab) = signal("visual".to_string());

    /// CSS classes for underlined tab buttons — matches React's border-b-2 style.
    const TAB_ACTIVE: &str =
        "px-1 py-3 text-sm font-medium border-b-2 border-amber-600 text-primary transition-colors";
    const TAB_INACTIVE: &str =
        "px-1 py-3 text-sm font-medium border-b-2 border-transparent text-muted-foreground hover:text-foreground hover:border-border transition-colors";

    /// CSS for config sub-tab pills (matches React's pill-style bg-background toggle).
    const SUB_TAB_ACTIVE: &str =
        "px-3 py-1 text-xs font-medium rounded bg-background text-foreground shadow-sm transition-colors";
    const SUB_TAB_INACTIVE: &str =
        "px-3 py-1 text-xs font-medium rounded text-muted-foreground hover:text-foreground transition-colors";

    /// CSS for modifier chip — active state.
    const CHIP_ACTIVE: &str =
        "inline-flex items-center px-2.5 py-0.5 text-xs font-medium rounded-full border transition-colors bg-primary/10 border-primary/50 text-primary";
    /// CSS for modifier chip — inactive state.
    const CHIP_INACTIVE: &str =
        "inline-flex items-center px-2.5 py-0.5 text-xs font-medium rounded-full border transition-colors bg-transparent border-border text-muted-foreground hover:border-foreground hover:text-foreground";

    view! {
        <Modal
            show=open
            on_close=on_close
            title=modal_title
            size=ModalSize::Full
            footer=footer_view
        >
            // Full-height flex column so content fills the modal.
            <div class="flex flex-col -m-4 sm:-m-6">
                // ── Tab bar — underlined style matching React ────────────
                // React: border-b border-border px-6 flex gap-8
                <div class="border-b border-border px-6 flex gap-8 flex-shrink-0">
                    <button
                        type="button"
                        class=move || {
                            if active_tab.get() == "sql" { TAB_ACTIVE } else { TAB_INACTIVE }
                        }
                        on:click=move |_| set_active_tab.set("sql".to_string())
                    >
                        "SQL Editor"
                    </button>
                    <button
                        type="button"
                        class=move || {
                            if active_tab.get() == "chart" { TAB_ACTIVE } else { TAB_INACTIVE }
                        }
                        on:click=move |_| {
                            // Sync YAML text from current form state when switching to chart tab
                            set_yaml_text.set(current_yaml.get_untracked());
                            set_active_tab.set("chart".to_string());
                        }
                    >
                        "Chart Config"
                    </button>
                </div>

                // ── SQL Editor tab ──────────────────────────────────────
                // React: px-6 py-4 flex-1 min-h-0 flex flex-col gap-4
                {move || {
                    (active_tab.get() == "sql").then(|| view! {
                        // Horizontal layout: editor area + optional catalog sidebar
                        // min-h-[70vh] gives the SQL code editor real vertical space.
                        <div class="flex flex-1 min-h-[70vh]">
                            // Main editor column
                            <div class="px-6 py-4 flex-1 min-h-0 min-w-0 flex flex-col gap-4">
                                // Datasource selector row with catalog toggle
                                <div class="flex items-center gap-3 flex-shrink-0">
                                    // Database icon before dropdown
                                    <Icon icon=icondata_lu::LuDatabase attr:class="w-4 h-4 text-muted-foreground flex-shrink-0" />
                                    <div class="w-full sm:w-[240px] min-w-0 sm:flex-shrink-0">
                                        <Suspense fallback=move || view! {
                                            <div class="text-sm text-muted-foreground">"Loading datasources..."</div>
                                        }>
                                            <DynSelect
                                                value=Signal::derive(move || datasource_slug.get())
                                                options=datasource_options
                                                on_change=move |slug: String| set_datasource_slug.set(slug)
                                                placeholder="Select a datasource..."
                                            />
                                        </Suspense>
                                    </div>
                                    // Catalog toggle button (pill-style)
                                    <button
                                        type="button"
                                        class=move || {
                                            if catalog_open.get() {
                                                "flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-full bg-primary/10 text-primary border border-primary/30 transition-colors flex-shrink-0"
                                            } else {
                                                "flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded-full border border-input text-muted-foreground hover:text-foreground hover:bg-secondary transition-colors flex-shrink-0"
                                            }
                                        }
                                        on:click=move |_| set_catalog_open.update(|v| *v = !*v)
                                    >
                                        <Icon icon=icondata_lu::LuDatabase width="14" height="14" />
                                        "Catalog"
                                    </button>
                                </div>

                                // SQL query editor — fills remaining space
                                <div class="flex-1 min-h-0">
                                    <SqlEditorSection
                                        content=sql.into()
                                        on_change=sql_on_change.get_value()
                                    />
                                </div>

                                // Run Query button
                                <div class="flex items-center justify-between flex-shrink-0">
                                    // Query status (row count / error indicator)
                                    <div class="text-xs text-muted-foreground">
                                        {move || {
                                            if query_running.get() {
                                                view! { <span class="flex items-center gap-1.5"><Spinner class="text-primary".to_string() />" Running..."</span> }.into_any()
                                            } else if let Some(ref err) = query_error.get() {
                                                view! { <span class="text-error-foreground" title=err.clone()>"Query failed"</span> }.into_any()
                                            } else if let Some(ref result) = query_result.get() {
                                                let total = result.total_rows.unwrap_or(result.row_count);
                                                view! { <span>{format!("{total} rows")}</span> }.into_any()
                                            } else {
                                                view! { <span></span> }.into_any()
                                            }
                                        }}
                                    </div>
                                    <button
                                        type="button"
                                        class=format!("{BTN_BASE} {BTN_DEFAULT} {BTN_SIZE}")
                                        disabled=move || query_running.get() || datasource_slug.get().is_empty() || sql.get().trim().is_empty()
                                        on:click=move |_| {
                                            let ds_slug = datasource_slug.get_untracked();
                                            let query_text = sql.get_untracked();
                                            if ds_slug.is_empty() || query_text.trim().is_empty() {
                                                return;
                                            }
                                            set_query_running.set(true);
                                            set_query_error.set(None);
                                            set_query_result.set(None);

                                            leptos::task::spawn_local(async move {
                                                match execute_sql_query(ds_slug, query_text, 50, 1).await {
                                                    Ok(result) => {
                                                        set_query_result.set(Some(result));
                                                    }
                                                    Err(e) => {
                                                        set_query_error.set(Some(e.to_string()));
                                                    }
                                                }
                                                set_query_running.set(false);
                                            });
                                        }
                                    >
                                        "Run Query"
                                    </button>
                                </div>

                                // Query error display
                                {move || {
                                    let err = query_error.get();
                                    err.map(|msg| view! {
                                        <div class="flex-shrink-0 max-h-[200px] overflow-auto border border-error-border rounded-md bg-error p-3">
                                            <p class="text-sm text-error-foreground font-medium">"Query Error"</p>
                                            <p class="text-sm text-error-foreground mt-1">{msg}</p>
                                        </div>
                                    })
                                }}

                                // Query results table
                                {move || {
                                    let result = query_result.get();
                                    result.map(|r| view! {
                                        <div class="flex-shrink-0 max-h-[250px] overflow-auto border border-border rounded-md">
                                            <ResultsTable
                                                result=r
                                                current_page=1
                                                page_size=50
                                                on_page_change=Callback::new(|_| {})
                                                on_page_size_change=Callback::new(|_| {})
                                            />
                                        </div>
                                    })
                                }}
                            </div>

                            // Catalog sidebar (280px, shown when catalog_open is true)
                            {move || catalog_open.get().then(|| {
                                let catalog_slug_signal = Signal::derive(move || {
                                    let slug = datasource_slug.get();
                                    if slug.is_empty() { None } else { Some(slug) }
                                });

                                view! {
                                    <div class="w-72 flex-shrink-0 border-l border-border bg-muted/30 flex flex-col min-h-0">
                                        // Sidebar header: pill toggle + refresh + close
                                        <div class="px-3 py-3 border-b border-border flex-shrink-0 flex items-center justify-between">
                                            <div class="flex items-center gap-1 rounded-lg bg-muted p-0.5">
                                                <button
                                                    type="button"
                                                    class=move || {
                                                        if catalog_tab.get() == "catalog" {
                                                            "flex-1 text-xs font-medium px-2.5 py-1 rounded-md bg-background text-foreground shadow-sm transition-colors"
                                                        } else {
                                                            "flex-1 text-xs font-medium px-2.5 py-1 rounded-md text-muted-foreground hover:text-foreground transition-colors"
                                                        }
                                                    }
                                                    on:click=move |_| set_catalog_tab.set("catalog".to_string())
                                                >
                                                    "Catalog"
                                                </button>
                                                <button
                                                    type="button"
                                                    class=move || {
                                                        if catalog_tab.get() == "history" {
                                                            "flex-1 text-xs font-medium px-2.5 py-1 rounded-md bg-background text-foreground shadow-sm transition-colors"
                                                        } else {
                                                            "flex-1 text-xs font-medium px-2.5 py-1 rounded-md text-muted-foreground hover:text-foreground transition-colors"
                                                        }
                                                    }
                                                    on:click=move |_| set_catalog_tab.set("history".to_string())
                                                >
                                                    "History"
                                                </button>
                                            </div>
                                            // Refresh + Close buttons
                                            <div class="flex items-center gap-1">
                                                <button
                                                    type="button"
                                                    class="p-1 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors"
                                                    title="Refresh catalog"
                                                >
                                                    <Icon icon=icondata_lu::LuRefreshCw width="14" height="14" />
                                                </button>
                                                <button
                                                    type="button"
                                                    class="p-1 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors"
                                                    title="Close catalog"
                                                    on:click=move |_| set_catalog_open.set(false)
                                                >
                                                    <Icon icon=icondata_lu::LuX width="14" height="14" />
                                                </button>
                                            </div>
                                        </div>

                                        // Sidebar body
                                        {move || {
                                            if catalog_tab.get() == "catalog" {
                                                view! {
                                                    <div class="flex flex-col flex-1 min-h-0">
                                                        // Search input
                                                        <div class="px-3 py-2 flex-shrink-0">
                                                            <input
                                                                type="text"
                                                                class=INPUT_CLASS
                                                                placeholder="Search tables..."
                                                                prop:value=move || catalog_search.get()
                                                                on:input=move |ev| set_catalog_search.set(event_target_value(&ev))
                                                            />
                                                        </div>
                                                        // Catalog tree
                                                        <div class="flex-1 overflow-y-auto min-h-0">
                                                            <CatalogTree
                                                                datasource_slug=catalog_slug_signal
                                                                search_query=Signal::derive(move || catalog_search.get())
                                                                refresh_trigger=Signal::derive(move || catalog_refresh_trigger.get())
                                                                on_table_click=Callback::new(move |table_id: String| {
                                                                    // Insert table name into SQL — append at cursor or end
                                                                    set_sql.update(|s| {
                                                                        if s.is_empty() {
                                                                            *s = format!("SELECT * FROM {table_id}");
                                                                        } else {
                                                                            s.push(' ');
                                                                            s.push_str(&table_id);
                                                                        }
                                                                    });
                                                                })
                                                                on_column_click=Callback::new(move |col_name: String| {
                                                                    set_sql.update(|s| {
                                                                        if !s.is_empty() {
                                                                            s.push(' ');
                                                                        }
                                                                        s.push_str(&col_name);
                                                                    });
                                                                })
                                                            />
                                                        </div>
                                                    </div>
                                                }.into_any()
                                            } else {
                                                // History tab placeholder
                                                view! {
                                                    <div class="flex flex-col items-center justify-center py-8 px-4 text-center">
                                                        <Icon icon=icondata_lu::LuClock width="40" height="40" attr:class="text-muted-foreground mb-2" />
                                                        <p class="text-sm text-muted-foreground">"Query history"</p>
                                                        <p class="text-xs text-muted-foreground mt-1">"Coming soon"</p>
                                                    </div>
                                                }.into_any()
                                            }
                                        }}
                                    </div>
                                }
                            })}
                        </div>
                    })
                }}

                // ── Chart Config tab — 50/50 split pane ────────────────
                // React: flex-1 min-h-0 flex → left 50% editing, right 50% preview
                {move || {
                    (active_tab.get() == "chart").then(|| view! {
                        <div class="flex flex-1 min-h-[70vh]">
                            // ── Left: Editing Panel (50%) ──────────────
                            <div class="w-1/2 border-r border-border flex flex-col min-h-0">
                                // Config sub-tab bar (Visual / AI / YAML)
                                <div class="border-b border-border px-4 py-2 bg-muted flex items-center gap-1 flex-shrink-0">
                                    <button
                                        type="button"
                                        class=move || if config_tab.get() == "visual" { SUB_TAB_ACTIVE } else { SUB_TAB_INACTIVE }
                                        on:click=move |_| set_config_tab.set("visual".to_string())
                                    >
                                        "Visual"
                                    </button>
                                    <button
                                        type="button"
                                        class=move || if config_tab.get() == "ai" { SUB_TAB_ACTIVE } else { SUB_TAB_INACTIVE }
                                        on:click=move |_| set_config_tab.set("ai".to_string())
                                    >
                                        "AI"
                                    </button>
                                    <button
                                        type="button"
                                        class=move || if config_tab.get() == "yaml" { SUB_TAB_ACTIVE } else { SUB_TAB_INACTIVE }
                                        on:click=move |_| {
                                            // Sync YAML text from form state when switching to YAML tab
                                            set_yaml_text.set(current_yaml.get_untracked());
                                            set_config_tab.set("yaml".to_string());
                                        }
                                    >
                                        "YAML"
                                    </button>
                                </div>

                                // Sub-tab content
                                <div class="flex-1 min-h-0 overflow-auto">
                                    // ── Visual sub-tab ──────────────────
                                    {move || (config_tab.get() == "visual").then(|| view! {
                                        <div class="p-4 space-y-6">
                                            // Chart Type
                                            <div class="space-y-2">
                                                <label class=LABEL_CLASS>"Chart Type"</label>
                                                <DynSelect
                                                    value=Signal::derive(move || chart_type.get())
                                                    options=Signal::stored(
                                                        CHART_TYPES
                                                            .iter()
                                                            .map(|(k, v)| (k.to_string(), v.to_string()))
                                                            .collect::<Vec<_>>()
                                                    )
                                                    on_change=move |ct: String| {
                                                        // Clear incompatible modifiers on type change
                                                        if ct != "bar" {
                                                            set_orientation.set(None);
                                                        }
                                                        if ct != "bar" && ct != "area" {
                                                            set_mode.set(None);
                                                        }
                                                        set_chart_type.set(ct);
                                                    }
                                                />

                                                // Modifier chips — contextual based on chart type
                                                // React: ChartVisualEditor lines 216-258
                                                {move || {
                                                    let ct = chart_type.get();
                                                    (ct == "bar" || ct == "area").then(|| view! {
                                                        <div class="flex flex-wrap gap-2 mt-2">
                                                            // Horizontal chip (bar only)
                                                            {move || (chart_type.get() == "bar").then(|| view! {
                                                                <button
                                                                    type="button"
                                                                    class=move || {
                                                                        if orientation.get().as_deref() == Some("horizontal") { CHIP_ACTIVE } else { CHIP_INACTIVE }
                                                                    }
                                                                    on:click=move |_| {
                                                                        if orientation.get_untracked().as_deref() == Some("horizontal") {
                                                                            set_orientation.set(None);
                                                                        } else {
                                                                            set_orientation.set(Some("horizontal".to_string()));
                                                                        }
                                                                    }
                                                                >
                                                                    "Horizontal"
                                                                </button>
                                                            })}
                                                            // Grouped chip (bar only)
                                                            {move || (chart_type.get() == "bar").then(|| view! {
                                                                <button
                                                                    type="button"
                                                                    class=move || {
                                                                        if mode.get().as_deref() == Some("grouped") { CHIP_ACTIVE } else { CHIP_INACTIVE }
                                                                    }
                                                                    on:click=move |_| {
                                                                        if mode.get_untracked().as_deref() == Some("grouped") {
                                                                            set_mode.set(None);
                                                                        } else {
                                                                            set_mode.set(Some("grouped".to_string()));
                                                                        }
                                                                    }
                                                                >
                                                                    "Grouped"
                                                                </button>
                                                            })}
                                                            // Normalized chip (area only)
                                                            {move || (chart_type.get() == "area").then(|| view! {
                                                                <button
                                                                    type="button"
                                                                    class=move || {
                                                                        if mode.get().as_deref() == Some("normalized") { CHIP_ACTIVE } else { CHIP_INACTIVE }
                                                                    }
                                                                    on:click=move |_| {
                                                                        if mode.get_untracked().as_deref() == Some("normalized") {
                                                                            set_mode.set(None);
                                                                        } else {
                                                                            set_mode.set(Some("normalized".to_string()));
                                                                        }
                                                                    }
                                                                >
                                                                    "Normalized"
                                                                </button>
                                                            })}
                                                        </div>
                                                    })
                                                }}
                                            </div>

                                            // Title
                                            <div class="space-y-2">
                                                <label class=LABEL_CLASS>"Title"</label>
                                                <input
                                                    type="text"
                                                    class=INPUT_CLASS
                                                    prop:value=move || title.get()
                                                    on:input=move |ev| set_title.set(event_target_value(&ev))
                                                    placeholder="Chart title"
                                                />
                                            </div>

                                            // X Axis Field — hidden for pie/doughnut/metric
                                            {move || {
                                                let ct = chart_type.get();
                                                let needs_axes = !matches!(ct.as_str(), "metric" | "pie" | "doughnut");
                                                needs_axes.then(|| view! {
                                                    <div class="space-y-2">
                                                        <label class=LABEL_CLASS>"X Axis Field"</label>
                                                        <input
                                                            type="text"
                                                            class=INPUT_CLASS
                                                            prop:value=move || x_field.get()
                                                            on:input=move |ev| set_x_field.set(event_target_value(&ev))
                                                            placeholder="e.g. date, category, name..."
                                                        />
                                                    </div>
                                                })
                                            }}

                                            // Series (Y Axis)
                                            <div class="space-y-3">
                                                <div class="flex items-center justify-between">
                                                    <label class=LABEL_CLASS>"Series (Y Axis)"</label>
                                                    <button
                                                        type="button"
                                                        class="text-sm text-primary hover:text-primary/80 font-medium transition-colors px-2 py-1.5 -mr-2 rounded-md hover:bg-primary/5"
                                                        on:click=add_series
                                                    >
                                                        "+ Add Series"
                                                    </button>
                                                </div>

                                                <For
                                                    each=move || {
                                                        let s = series.get();
                                                        s.into_iter().enumerate().collect::<Vec<_>>()
                                                    }
                                                    key=|(_, entry)| entry.id
                                                    let:item
                                                >
                                                    {
                                                        let (idx, entry) = item;
                                                        let y_val = entry.y_field.clone();
                                                        let label_val = entry.label.clone();
                                                        let show_remove = move || series.get().len() > 1;

                                                        view! {
                                                            <div class="flex items-start gap-2">
                                                                <div class="flex-1 space-y-1">
                                                                    <input
                                                                        type="text"
                                                                        class=INPUT_CLASS
                                                                        prop:value=y_val.clone()
                                                                        on:input=move |ev| {
                                                                            let val = event_target_value(&ev);
                                                                            set_series.update(|s| {
                                                                                if let Some(entry) = s.get_mut(idx) {
                                                                                    entry.y_field = val;
                                                                                }
                                                                            });
                                                                        }
                                                                        placeholder="Y field name (e.g. revenue, count)"
                                                                    />
                                                                    <input
                                                                        type="text"
                                                                        class=INPUT_CLASS
                                                                        prop:value=label_val.clone()
                                                                        on:input=move |ev| {
                                                                            let val = event_target_value(&ev);
                                                                            set_series.update(|s| {
                                                                                if let Some(entry) = s.get_mut(idx) {
                                                                                    entry.label = val;
                                                                                }
                                                                            });
                                                                        }
                                                                        placeholder="Label (optional)"
                                                                    />
                                                                </div>
                                                                {move || show_remove().then(|| {
                                                                    view! {
                                                                        <button
                                                                            type="button"
                                                                            class="mt-1 p-2 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors"
                                                                            on:click=move |_| remove_series(idx)
                                                                            title="Remove series"
                                                                        >
                                                                            <Icon icon=icondata_lu::LuX width="16" height="16" />
                                                                        </button>
                                                                    }
                                                                })}
                                                            </div>
                                                        }
                                                    }
                                                </For>
                                            </div>
                                        </div>
                                    })}

                                    // ── AI sub-tab ─────────────────────
                                    {move || (config_tab.get() == "ai").then(|| view! {
                                        <ChartCopilot
                                            chart_yaml=Signal::derive(move || current_yaml.get())
                                            on_chart_update=Callback::new(move |new_yaml: String| {
                                                let parsed = parse_existing_yaml(&new_yaml);
                                                apply_parsed(parsed, &new_yaml);
                                                set_original_yaml.set(new_yaml.clone());
                                                set_yaml_text.set(new_yaml);
                                            })
                                        />
                                    })}

                                    // ── YAML sub-tab ────────────────────
                                    {move || (config_tab.get() == "yaml").then(|| view! {
                                        <div class="h-full min-h-[400px]">
                                            <YamlEditorSection
                                                content=Signal::derive(move || yaml_text.get())
                                                on_change=yaml_on_change.get_value()
                                            />
                                        </div>
                                    })}
                                </div>
                            </div>

                            // ── Right: Always-visible Preview (50%) ────
                            <div class="w-1/2 flex flex-col min-h-0">
                                <div class="border-b border-border px-4 py-2 bg-muted flex items-center justify-between flex-shrink-0">
                                    <h3 class="text-xs font-medium text-foreground">"Preview"</h3>
                                    // Refresh preview button
                                    <button
                                        type="button"
                                        class="p-1 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors"
                                        title="Refresh preview"
                                        disabled=move || preview_loading.get()
                                        on:click=move |_| {
                                            let ds_slug = datasource_slug.get_untracked();
                                            let query_text = sql.get_untracked();
                                            if ds_slug.is_empty() || query_text.trim().is_empty() {
                                                return;
                                            }
                                            set_preview_loading.set(true);
                                            set_preview_error.set(None);

                                            leptos::task::spawn_local(async move {
                                                match query_datasource_arrow(ds_slug, query_text, None).await {
                                                    Ok(query_result) => {
                                                        use base64::Engine;
                                                        match base64::engine::general_purpose::STANDARD
                                                            .decode(&query_result.ipc_base64) {
                                                            Ok(ipc_bytes) => {
                                                                match chartml_core::data::DataTable::from_ipc_bytes(&ipc_bytes) {
                                                                    Ok(data_table) => {
                                                                        let colors = kyomi_palette("balanced");
                                                                        let mut chartml_inst = chartml_core::ChartML::new();
                                                                        chartml_inst.register_renderer("bar", chartml_chart_cartesian::CartesianRenderer::new());
                                                                        chartml_inst.register_renderer("line", chartml_chart_cartesian::CartesianRenderer::new());
                                                                        chartml_inst.register_renderer("area", chartml_chart_cartesian::CartesianRenderer::new());
                                                                        chartml_inst.register_renderer("pie", chartml_chart_pie::PieRenderer::new());
                                                                        chartml_inst.register_renderer("doughnut", chartml_chart_pie::PieRenderer::new());
                                                                        chartml_inst.register_renderer("scatter", chartml_chart_scatter::ScatterRenderer::new());
                                                                        chartml_inst.register_renderer("metric", chartml_chart_metric::MetricRenderer::new());
                                                                        chartml_inst.register_transform(chartml_datafusion::DataFusionTransform);
                                                                        chartml_inst.set_default_palette(colors);
                                                                        chartml_inst.register_source("_remote", data_table);
                                                                        set_preview_chartml.set(Some(Arc::new(chartml_inst)));
                                                                    }
                                                                    Err(e) => set_preview_error.set(Some(format!("Arrow decode error: {e}"))),
                                                                }
                                                            }
                                                            Err(e) => set_preview_error.set(Some(format!("Base64 decode error: {e}"))),
                                                        }
                                                        set_preview_loading.set(false);
                                                    }
                                                    Err(e) => {
                                                        set_preview_error.set(Some(format!("Query error: {e}")));
                                                        set_preview_loading.set(false);
                                                    }
                                                }
                                            });
                                        }
                                    >
                                        {move || {
                                            if preview_loading.get() {
                                                view! { <Spinner class="text-primary w-3.5 h-3.5".to_string() /> }.into_any()
                                            } else {
                                                view! {
                                                    <Icon icon=icondata_lu::LuRefreshCw width="14" height="14" />
                                                }.into_any()
                                            }
                                        }}
                                    </button>
                                </div>

                                <div class="flex-1 min-h-0 overflow-auto p-4">
                                    // Preview content
                                    {move || {
                                        if preview_loading.get() {
                                            view! {
                                                <div class="flex items-center justify-center h-full">
                                                    <div class="flex flex-col items-center gap-2">
                                                        <Spinner class="text-primary".to_string() />
                                                        <p class="text-sm text-muted-foreground">"Loading preview..."</p>
                                                    </div>
                                                </div>
                                            }.into_any()
                                        } else if let Some(ref err) = preview_error.get() {
                                            view! {
                                                <div class="flex items-center justify-center h-full">
                                                    <div class="text-center px-4">
                                                        <p class="text-sm text-error-foreground mb-1">"Preview Error"</p>
                                                        <p class="text-xs text-muted-foreground">{err.clone()}</p>
                                                    </div>
                                                </div>
                                            }.into_any()
                                        } else if let Some(chartml_inst) = preview_chartml.get() {
                                            // Render remote chart preview — data was fetched
                                            let preview_yaml = current_yaml.get();
                                            let preview_spec = rewrite_spec_for_remote(&preview_yaml);
                                            view! {
                                                <ChartPreview
                                                    spec=preview_spec
                                                    chartml=chartml_inst
                                                />
                                            }.into_any()
                                        } else if datasource_slug.get().is_empty() {
                                            // Inline data chart — render directly without remote fetch.
                                            // ChartML with DataFusionTransform handles inline data natively.
                                            let chartml_inst = configured_chartml("balanced");
                                            let preview_yaml = current_yaml.get();
                                            view! {
                                                <ChartPreview
                                                    spec=preview_yaml
                                                    chartml=chartml_inst
                                                />
                                            }.into_any()
                                        } else {
                                            // Has datasource but no data fetched yet
                                            view! {
                                                <div class="flex items-center justify-center h-full">
                                                    <div class="text-center px-4">
                                                        <Icon icon=icondata_lu::LuChartBar width="48" height="48" attr:class="text-muted-foreground/30 mx-auto mb-3" />
                                                        <p class="text-sm text-muted-foreground mb-1">"No preview available"</p>
                                                        <p class="text-xs text-muted-foreground">"Run your SQL query first, then click the refresh button to see a preview."</p>
                                                    </div>
                                                </div>
                                            }.into_any()
                                        }
                                    }}
                                </div>
                            </div>
                        </div>
                    })
                }}
            </div>
        </Modal>
    }
}

// ─── Helper: rewrite YAML spec to use _remote data source ──────────────────

/// Rewrites a ChartML YAML spec to reference the named `_remote` source
/// instead of a datasource + SQL query. This is the same pattern used by
/// `ChartBlock` in `markdown_renderer.rs`.
fn rewrite_spec_for_remote(yaml: &str) -> String {
    match serde_yaml::from_str::<serde_json::Value>(yaml) {
        Ok(mut val) => {
            let doc = if let Some(arr) = val.as_array_mut() {
                arr.first_mut()
            } else {
                Some(&mut val)
            };
            if let Some(doc) = doc
                && let Some(obj) = doc.as_object_mut()
            {
                obj.insert(
                    "data".to_string(),
                    serde_json::Value::String("_remote".to_string()),
                );
            }
            serde_yaml::to_string(&val).unwrap_or_else(|_| yaml.to_string())
        }
        Err(_) => yaml.to_string(),
    }
}

// ─── Chart Preview component ──────────────────────────────────────────────

/// Renders a live chart preview using `ChartMLChart`.
#[component]
fn ChartPreview(
    #[prop(into)] spec: String,
    chartml: Arc<chartml_core::ChartML>,
) -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        use chartml_leptos::ChartMLChart;

        view! {
            <ChartMLChart
                spec=Signal::stored(spec)
                chartml=chartml
            />
        }
        .into_any()
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = spec;
        let _ = chartml;
        view! {
            <div class="flex items-center justify-center h-full text-muted-foreground text-sm">
                "Chart preview loading..."
            </div>
        }
        .into_any()
    }
}

// ─── SQL Editor Section ─────────────────────────────────────────────────────

/// Renders either the kode-leptos CodeEditor (on wasm32) or a plain textarea
/// placeholder (during SSR).
#[component]
fn SqlEditorSection(
    content: Signal<String>,
    on_change: Arc<dyn Fn(String) + Send + Sync>,
) -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        use kode_leptos::{CodeEditor, Language};

        view! {
            <div class="min-h-[200px] border border-input rounded-md overflow-hidden" style="height: calc(100vh - 420px);">
                <CodeEditor
                    language=Signal::stored(Language::Sql)
                    content=content
                    on_change=on_change
                />
            </div>
        }
        .into_any()
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = content;
        let _ = on_change;

        view! {
            <div class="h-full min-h-[200px] bg-muted rounded-md p-4 flex items-center justify-center text-muted-foreground text-sm">
                "Loading SQL editor..."
            </div>
        }
        .into_any()
    }
}

// ─── YAML Editor Section ────────────────────────────────────────────────────

/// Renders a kode-leptos CodeEditor in YAML mode for the YAML sub-tab.
#[component]
fn YamlEditorSection(
    content: Signal<String>,
    on_change: Arc<dyn Fn(String) + Send + Sync>,
) -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        use kode_leptos::{CodeEditor, Language};

        view! {
            <div class="h-full min-h-[400px] border-t border-border overflow-hidden">
                <CodeEditor
                    language=Signal::stored(Language::Yaml)
                    content=content
                    on_change=on_change
                />
            </div>
        }
        .into_any()
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = content;
        let _ = on_change;

        view! {
            <div class="h-full min-h-[400px] bg-muted rounded-md p-4 flex items-center justify-center text-muted-foreground text-sm">
                "Loading YAML editor..."
            </div>
        }
        .into_any()
    }
}

// ─── Chart Copilot ──────────────────────────────────────────────────────────

/// Chart Builder Copilot — inline AI chat for editing charts.
///
/// Delegates to the shared [`CopilotChat`] component with chart-builder-specific
/// configuration: `context_type = "chart_builder_copilot"`, chart YAML context,
/// and a `chart_update` custom WS event handler that applies AI-generated specs.
///
/// Rendered inline as tab content (no sidebar chrome).
#[component]
fn ChartCopilot(
    /// Current chart YAML — sent as context with each message.
    #[prop(into)]
    chart_yaml: Signal<String>,
    /// Callback when the AI updates the chart spec.
    on_chart_update: Callback<String>,
) -> impl IntoView {
    let on_custom = Callback::new(move |(_event_name, data): (String, serde_json::Value)| {
        if let Some(content) = data.get("content").and_then(|v| v.as_str()) {
            on_chart_update.run(content.to_string());
        }
    });

    view! {
        <CopilotChat
            context_type="chart_builder_copilot"
            context_content=chart_yaml
            context_label="Chart Content"
            placeholder="Ask about your chart..."
            empty_icon=Arc::new(|| view! { <Icon icon=icondata_lu::LuSparkles width="48" height="48" /> }.into_any())
            empty_title="Ask me anything about your chart!"
            empty_description="I can help you change chart types, adjust styling, or fix configuration issues."
            custom_ws_events=vec!["chart_update".to_string()]
            on_custom_ws_event=on_custom
        />
    }
}

