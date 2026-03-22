// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart Builder Modal — a simplified Leptos port of
//! `apps/frontend/src/components/ChartBuilderModal.jsx`.
//!
//! Provides a step-by-step flow for creating/editing charts:
//! 1. Select a datasource
//! 2. Write a SQL query
//! 3. Configure chart type, axes, and series
//! 4. Generate ChartML YAML and insert into the dashboard
//!
//! Uses the project's `Modal`, `DynSelect`, and kode-leptos `CodeEditor`
//! components to match existing patterns.

use std::sync::Arc;

use leptos::prelude::*;

use crate::components::input::INPUT_CLASS;
use crate::components::modal::{Modal, ModalSize};
use crate::components::select::DynSelect;
use crate::server_fns::datasources::{list_datasources, DatasourceInfo};

// ─── Constants ──────────────────────────────────────────────────────────────

/// Button base classes — same as `save_dashboard_modal.rs`.
const BTN_BASE: &str = "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0";

/// Default (primary) button variant classes.
const BTN_DEFAULT: &str =
    "bg-primary text-primary-foreground shadow hover:bg-primary/90";

/// Outline button variant classes.
const BTN_OUTLINE: &str = "border border-input bg-background text-foreground shadow-sm hover:bg-accent hover:text-accent-foreground";

/// Default button size classes.
const BTN_SIZE: &str = "h-9 px-4 py-2";

/// Label classes — matches the project's design system.
const LABEL_CLASS: &str = "text-sm font-medium text-foreground";

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
                .and_then(|t| t.as_str())
                .map_or(false, |t| t == "chart")
        })
        .or(docs.first());

    let Some(doc) = doc else {
        return chart;
    };

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
        if let Some(x) = vis.get("x").and_then(|v| v.as_str()) {
            chart.x_field = x.to_string();
        }
        if let Some(serde_yaml::Value::Sequence(series_seq)) = vis.get("series") {
            for entry in series_seq {
                let y = entry
                    .get("y")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let label = entry
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                chart.series.push(SeriesEntry { y_field: y, label });
            }
        }
    }

    chart
}

// ─── YAML generation ────────────────────────────────────────────────────────

/// Build a ChartML YAML string from the form state.
fn build_yaml(
    title: &str,
    datasource_slug: &str,
    sql: &str,
    chart_type: &str,
    x_field: &str,
    series: &[SeriesEntry],
) -> String {
    // Indent SQL lines for YAML block scalar
    let sql_indented = sql
        .lines()
        .map(|line| format!("      {line}"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut yaml = format!(
        r#"- type: chart
  version: 1
  title: "{title}"
  data:
    datasource: "{datasource_slug}"
    sql: |
{sql_indented}
  visualize:
    type: {chart_type}"#
    );

    // x axis — only for chart types that use it
    let needs_axes = !matches!(chart_type, "metric" | "pie" | "doughnut");
    if needs_axes && !x_field.is_empty() {
        yaml.push_str(&format!("\n    x: {x_field}"));
    }

    // Series
    let has_series = series.iter().any(|s| !s.y_field.is_empty());
    if has_series {
        yaml.push_str("\n    series:");
        for entry in series {
            if entry.y_field.is_empty() {
                continue;
            }
            yaml.push_str(&format!("\n      - y: {}", entry.y_field));
            if !entry.label.is_empty() {
                yaml.push_str(&format!("\n        label: \"{}\"", entry.label));
            }
        }
    }

    yaml.push('\n');
    yaml
}

// ─── Component ──────────────────────────────────────────────────────────────

/// Chart Builder Modal — create or edit a ChartML chart.
///
/// React reference: `apps/frontend/src/components/ChartBuilderModal.jsx`
///
/// This is a simplified version that covers the core flow:
/// datasource selection, SQL query, chart type/axis configuration,
/// and YAML generation. The React version has additional features
/// (Monaco editor, AI copilot, live preview) that will be added
/// incrementally.
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

    // Parse existing YAML or use defaults
    let initial = existing_yaml
        .as_ref()
        .map(|y| parse_existing_yaml(y))
        .unwrap_or_default();

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

    let initial_series = if initial.series.is_empty() {
        vec![SeriesEntry {
            y_field: String::new(),
            label: String::new(),
        }]
    } else {
        initial.series.clone()
    };
    let (series, set_series) = signal(initial_series);

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
        set_series.update(|s| {
            s.push(SeriesEntry {
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

    // ── Insert / Update handler ─────────────────────────────────────────

    let handle_insert = Callback::new(move |()| {
        let yaml = build_yaml(
            &title.get_untracked(),
            &datasource_slug.get_untracked(),
            &sql.get_untracked(),
            &chart_type.get_untracked(),
            &x_field.get_untracked(),
            &series.get_untracked(),
        );
        on_insert.run(yaml);
        on_close.run(());
    });

    // ── Footer ──────────────────────────────────────────────────────────

    let cancel_class = format!("{BTN_BASE} {BTN_OUTLINE} {BTN_SIZE}");
    let insert_class = format!("{BTN_BASE} {BTN_DEFAULT} {BTN_SIZE}");

    let cancel_class_clone = cancel_class.clone();
    let insert_class_clone = insert_class.clone();

    let insert_label = if is_edit_mode {
        "Update Chart"
    } else {
        "Insert Chart"
    };

    let footer_view: ChildrenFn = Arc::new(move || {
        let cancel_class = cancel_class_clone.clone();
        let insert_class = insert_class_clone.clone();

        // Disable insert when datasource or SQL is empty
        let is_disabled =
            datasource_slug.get().is_empty() || sql.get().trim().is_empty();

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

    // ── View ────────────────────────────────────────────────────────────

    let modal_title = if is_edit_mode {
        "Edit Chart"
    } else {
        "Chart Builder"
    };

    view! {
        <Modal
            show=open
            on_close=on_close
            title=modal_title
            size=ModalSize::Lg
            footer=footer_view
        >
            <div class="space-y-6">
                // ── Section 1: Title ────────────────────────────────────
                <div class="space-y-2">
                    <label class=LABEL_CLASS>"Chart Title"</label>
                    <input
                        type="text"
                        class=INPUT_CLASS
                        prop:value=move || title.get()
                        on:input=move |ev| set_title.set(event_target_value(&ev))
                        placeholder="Enter chart title..."
                    />
                </div>

                // ── Section 2: Datasource ───────────────────────────────
                <div class="space-y-2">
                    <label class=LABEL_CLASS>"Datasource"</label>
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

                // ── Section 3: SQL Query ────────────────────────────────
                <div class="space-y-2">
                    <label class=LABEL_CLASS>"SQL Query"</label>
                    <SqlEditorSection
                        content=sql.into()
                        on_change=sql_on_change.get_value()
                    />
                </div>

                // ── Section 4: Chart Type ───────────────────────────────
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
                        on_change=move |ct: String| set_chart_type.set(ct)
                    />
                </div>

                // ── Section 5: Axis Configuration ───────────────────────
                // Only show axes for types that use them
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

                // ── Section 6: Y Axis / Series ──────────────────────────
                <div class="space-y-3">
                    <div class="flex items-center justify-between">
                        <label class=LABEL_CLASS>"Series (Y Axis)"</label>
                        <button
                            type="button"
                            class="text-xs text-primary hover:text-primary/80 font-medium transition-colors"
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
                        key=|(idx, _)| *idx
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
                                                class="mt-1.5 p-1.5 rounded hover:bg-accent text-muted-foreground hover:text-foreground transition-colors"
                                                on:click=move |_| remove_series(idx)
                                                title="Remove series"
                                            >
                                                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                                                </svg>
                                            </button>
                                        }
                                    })}
                                </div>
                            }
                        }
                    </For>
                </div>
            </div>
        </Modal>
    }
}

// ─── SQL Editor Section ─────────────────────────────────────────────────────

/// Renders either the kode-leptos CodeEditor (on wasm32) or a plain textarea
/// placeholder (during SSR).
///
/// Follows the same `#[cfg(target_arch = "wasm32")]` gating pattern as
/// `DashboardCodeEditor` in `pages/dashboards/dashboard_editor.rs`.
#[component]
fn SqlEditorSection(
    content: Signal<String>,
    on_change: Arc<dyn Fn(String) + Send + Sync>,
) -> impl IntoView {
    #[cfg(target_arch = "wasm32")]
    {
        use kode_leptos::{CodeEditor, Language};

        view! {
            <div class="h-48 border border-input rounded-md overflow-hidden">
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
            <div class="h-48 bg-muted rounded-md p-4 flex items-center justify-center text-muted-foreground text-sm">
                "Loading SQL editor..."
            </div>
        }
        .into_any()
    }
}
