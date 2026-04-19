// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart Builder Modal — Leptos port of
//! `apps/frontend/src/components/ChartBuilderModal.jsx`.
//!
//! Two-screen flow:
//! 1. **SQL Editor** — datasource selection, SQL query, catalog sidebar
//! 2. **Chart Config** — split pane: left editing panel (Visual / AI / YAML sub-tabs),
//!    right live preview
//!
//! ## State model — single source of truth
//!
//! The three config sub-tabs (Visual / AI / YAML) all share a single source of
//! truth: the parsed YAML AST (`serde_yaml::Value`) held in `ast`. Every tab
//! reads from and writes to this AST. The `yaml_text` signal is just a buffer
//! for the YAML editor — when a Visual-tab control mutates the AST, the handler
//! also synchronously writes the freshly-serialized YAML into `yaml_text`.
//! There is intentionally NO effect that reacts to AST changes and re-syncs
//! `yaml_text`, because that would clobber in-progress typing in the YAML tab.
//!
//! Parse errors while typing in the YAML tab surface in `yaml_parse_error`
//! without disturbing the AST or the text buffer — the Visual tab keeps
//! showing the last-known-good AST.
//!
//! React reference: `ChartBuilderModal.jsx`, `ChartVisualEditor.jsx`,
//! `ChartMLConfigEditor.jsx`, `ChartBuilderCopilotSidebar.jsx`.

use std::sync::Arc;

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};
use serde_yaml::{Mapping, Value};
use crate::components::chat::CopilotChat;
use crate::components::input::INPUT_CLASS;
use crate::components::modal::{Modal, ModalSize};
use crate::components::select::DynSelect;
use crate::components::Spinner;
use crate::pages::sql_editor::catalog_tree::CatalogTree;
use crate::pages::sql_editor::results_table::ResultsTable;
use crate::pages::sql_editor::types::QueryResult;
use crate::server_fns::datasources::{list_datasources, query_datasource_arrow};
use crate::server_fns::sql_editor::execute_sql_query;

use super::markdown_renderer::{configured_chartml, kyomi_palette, kyomi_theme};
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

/// Seed YAML for a new (blank) chart.
///
/// The renderer's `extract_query` reads `data.query` (or `data.url`); it does
/// NOT read `data.sql`. Seeding with `query:` ensures new single-chart
/// dashboards actually render once data is supplied.
const NEW_CHART_SEED: &str = r#"type: chart
version: 1
data:
  datasource: ""
  query: ""
visualize:
  type: bar
  style:
    title: "New Chart"
"#;

// ─── Series entry (Visual tab view model) ───────────────────────────────────

/// A single Y-axis series entry — used only as a view model for the Visual
/// tab's series list so `<For>` gets stable keys. The canonical form lives in
/// the AST under `visualize.rows`.
#[derive(Clone, Debug)]
struct SeriesEntry {
    /// Unique ID for stable keying in `<For>` loops.
    id: u32,
    y_field: String,
    label: String,
}

// ─── AST helpers: mapping / key manipulation ────────────────────────────────

/// Ensure `ast` is a mapping, replacing non-mapping values with an empty one.
fn ensure_root_mapping(ast: &mut Value) {
    if !ast.is_mapping() {
        *ast = Value::Mapping(Mapping::new());
    }
}

/// Ensure `parent[key]` exists and is a mapping; create an empty one if missing.
fn ensure_nested_mapping(parent: &mut Value, key: &str) {
    ensure_root_mapping(parent);
    let map = parent.as_mapping_mut().expect("ensured above");
    let k = Value::String(key.to_string());
    match map.get(&k) {
        Some(v) if v.is_mapping() => {}
        _ => {
            map.insert(k, Value::Mapping(Mapping::new()));
        }
    }
}

/// Remove `parent[key]` if `parent` is a mapping.
fn remove_key(parent: &mut Value, key: &str) {
    if let Some(map) = parent.as_mapping_mut() {
        map.remove(Value::String(key.to_string()));
    }
}

/// Get a nested string value by path (e.g. `["visualize", "type"]`).
fn get_string_at(ast: &Value, path: &[&str]) -> Option<String> {
    let mut cur = ast;
    for k in path {
        cur = cur.get(*k)?;
    }
    cur.as_str().map(String::from)
}

// ─── AST: document extraction (strip sequence wrapper) ──────────────────────

/// Extract the chart document from a top-level YAML `Value`.
///
/// On-disk chart specs are a single-element sequence (`- type: chart\n ...`),
/// but the AST we operate on internally is the bare chart mapping. This helper
/// unwraps the sequence if present, preferring the first element whose
/// `type: chart`.
fn extract_chart_doc(val: Value) -> Value {
    match val {
        Value::Sequence(mut seq) => {
            // Prefer an explicit `type: chart` entry, else fall back to first.
            let idx = seq
                .iter()
                .position(|d| d.get("type").and_then(|t| t.as_str()) == Some("chart"));
            match idx {
                Some(i) => seq.swap_remove(i),
                None => seq.into_iter().next().unwrap_or_else(|| Value::Mapping(Mapping::new())),
            }
        }
        other => other,
    }
}

/// Wrap the chart document back into a single-element sequence for saving.
fn wrap_as_sequence(doc: &Value) -> Value {
    // Ensure the top-level `type: chart` tag is present — it's required for the
    // renderer to identify the block.
    let mut doc = doc.clone();
    if doc.get("type").and_then(|v| v.as_str()) != Some("chart") {
        ensure_root_mapping(&mut doc);
        if let Some(map) = doc.as_mapping_mut() {
            map.insert(
                Value::String("type".to_string()),
                Value::String("chart".to_string()),
            );
        }
    }
    Value::Sequence(vec![doc])
}

/// Parse an incoming YAML string into the chart-document AST. Returns `None`
/// on parse error so the caller can display an inline error without disturbing
/// the current AST.
fn parse_chart_yaml(text: &str) -> Result<Value, serde_yaml::Error> {
    serde_yaml::from_str::<Value>(text).map(extract_chart_doc)
}

/// Serialize the current AST for display in the YAML editor / preview.
fn serialize_ast(ast: &Value) -> String {
    serde_yaml::to_string(&wrap_as_sequence(ast)).unwrap_or_default()
}

/// Produce the initial AST: either from existing YAML, or from the new-chart seed.
fn initial_ast(existing: Option<&str>) -> Value {
    match existing {
        Some(y) if !y.trim().is_empty() => match serde_yaml::from_str::<Value>(y) {
            Ok(v) => extract_chart_doc(v),
            Err(_) => seed_ast(),
        },
        _ => seed_ast(),
    }
}

/// Fresh AST for a new chart. Uses `data.query` (not `sql`) so the renderer
/// picks it up.
fn seed_ast() -> Value {
    serde_yaml::from_str::<Value>(NEW_CHART_SEED)
        .unwrap_or_else(|_| Value::Mapping(Mapping::new()))
}

// ─── AST field accessors — Visual tab controls ──────────────────────────────

/// Get the chart title. The canonical location is `visualize.style.title`; we
/// also accept a top-level `title` (some older exports).
fn ast_get_title(ast: &Value) -> String {
    get_string_at(ast, &["visualize", "style", "title"])
        .or_else(|| get_string_at(ast, &["title"]))
        .unwrap_or_default()
}

/// Set (or clear) the chart title, writing to `visualize.style.title`.
fn ast_set_title(ast: &mut Value, val: &str) {
    ensure_root_mapping(ast);
    // Always strip any legacy top-level title so we don't have two copies.
    remove_key(ast, "title");

    if val.is_empty() {
        // Remove from visualize.style.title; tidy up empty style mapping.
        if let Some(vis) = ast.get_mut("visualize")
            && let Some(style) = vis.get_mut("style")
        {
            remove_key(style, "title");
            if style.as_mapping().is_some_and(|m| m.is_empty()) {
                remove_key(vis, "style");
            }
        }
        return;
    }

    ensure_nested_mapping(ast, "visualize");
    let vis = ast.get_mut("visualize").expect("ensured");
    ensure_nested_mapping(vis, "style");
    let style = vis.get_mut("style").expect("ensured");
    if let Some(map) = style.as_mapping_mut() {
        map.insert(
            Value::String("title".to_string()),
            Value::String(val.to_string()),
        );
    }
}

fn ast_get_datasource(ast: &Value) -> String {
    get_string_at(ast, &["data", "datasource"]).unwrap_or_default()
}

fn ast_set_datasource(ast: &mut Value, val: &str) {
    ensure_root_mapping(ast);
    if val.is_empty() {
        if let Some(data) = ast.get_mut("data") {
            remove_key(data, "datasource");
        }
        return;
    }
    ensure_nested_mapping(ast, "data");
    let data = ast.get_mut("data").expect("ensured");
    if let Some(map) = data.as_mapping_mut() {
        map.insert(
            Value::String("datasource".to_string()),
            Value::String(val.to_string()),
        );
    }
}

/// Get the SQL query — accepts legacy `sql` as a fallback for older saves.
fn ast_get_query(ast: &Value) -> String {
    get_string_at(ast, &["data", "query"])
        .or_else(|| get_string_at(ast, &["data", "sql"]))
        .unwrap_or_default()
}

/// Set (or clear) the SQL query. Always writes to `data.query` — the renderer
/// only reads `query`/`url`. Also migrates away from any legacy `data.sql`.
fn ast_set_query(ast: &mut Value, val: &str) {
    ensure_root_mapping(ast);
    if val.is_empty() {
        if let Some(data) = ast.get_mut("data") {
            remove_key(data, "query");
            remove_key(data, "sql");
        }
        return;
    }
    ensure_nested_mapping(ast, "data");
    let data = ast.get_mut("data").expect("ensured");
    // Clear legacy `sql` so the saved YAML is canonical.
    remove_key(data, "sql");
    if let Some(map) = data.as_mapping_mut() {
        map.insert(
            Value::String("query".to_string()),
            Value::String(val.to_string()),
        );
    }
}

fn ast_get_chart_type(ast: &Value) -> String {
    get_string_at(ast, &["visualize", "type"]).unwrap_or_else(|| "bar".to_string())
}

fn ast_set_chart_type(ast: &mut Value, val: &str) {
    ensure_root_mapping(ast);
    ensure_nested_mapping(ast, "visualize");
    let vis = ast.get_mut("visualize").expect("ensured");
    if let Some(map) = vis.as_mapping_mut() {
        map.insert(
            Value::String("type".to_string()),
            Value::String(val.to_string()),
        );
    }
}

fn ast_get_x_field(ast: &Value) -> String {
    get_string_at(ast, &["visualize", "columns"]).unwrap_or_default()
}

fn ast_set_x_field(ast: &mut Value, val: &str) {
    ensure_root_mapping(ast);
    if val.is_empty() {
        if let Some(vis) = ast.get_mut("visualize") {
            remove_key(vis, "columns");
        }
        return;
    }
    ensure_nested_mapping(ast, "visualize");
    let vis = ast.get_mut("visualize").expect("ensured");
    if let Some(map) = vis.as_mapping_mut() {
        map.insert(
            Value::String("columns".to_string()),
            Value::String(val.to_string()),
        );
    }
}

fn ast_get_orientation(ast: &Value) -> Option<String> {
    get_string_at(ast, &["visualize", "orientation"])
}

fn ast_set_orientation(ast: &mut Value, val: Option<&str>) {
    ensure_root_mapping(ast);
    match val {
        None => {
            if let Some(vis) = ast.get_mut("visualize") {
                remove_key(vis, "orientation");
            }
        }
        Some(v) => {
            ensure_nested_mapping(ast, "visualize");
            let vis = ast.get_mut("visualize").expect("ensured");
            if let Some(map) = vis.as_mapping_mut() {
                map.insert(
                    Value::String("orientation".to_string()),
                    Value::String(v.to_string()),
                );
            }
        }
    }
}

fn ast_get_mode(ast: &Value) -> Option<String> {
    get_string_at(ast, &["visualize", "mode"])
}

fn ast_set_mode(ast: &mut Value, val: Option<&str>) {
    ensure_root_mapping(ast);
    match val {
        None => {
            if let Some(vis) = ast.get_mut("visualize") {
                remove_key(vis, "mode");
            }
        }
        Some(v) => {
            ensure_nested_mapping(ast, "visualize");
            let vis = ast.get_mut("visualize").expect("ensured");
            if let Some(map) = vis.as_mapping_mut() {
                map.insert(
                    Value::String("mode".to_string()),
                    Value::String(v.to_string()),
                );
            }
        }
    }
}

// ─── Series (visualize.rows) ────────────────────────────────────────────────

/// Derive the Visual-tab series view model from the AST's `visualize.rows`.
///
/// `rows` may be:
/// - absent (→ empty list)
/// - a bare string (single series by field name)
/// - a single `{field, label}` mapping
/// - a sequence of strings or `{field, label}` mappings
///
/// Unknown entry shapes are rendered as an empty-field entry so the list still
/// round-trips in length; we don't silently drop entries.
fn ast_get_series(ast: &Value) -> Vec<SeriesEntry> {
    let Some(rows_val) = ast.get("visualize").and_then(|v| v.get("rows")) else {
        return Vec::new();
    };
    match rows_val {
        Value::String(s) => vec![SeriesEntry {
            id: 0,
            y_field: s.clone(),
            label: String::new(),
        }],
        Value::Mapping(m) => {
            let field = m
                .get(Value::String("field".to_string()))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let label = m
                .get(Value::String("label".to_string()))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            vec![SeriesEntry { id: 0, y_field: field, label }]
        }
        Value::Sequence(seq) => seq
            .iter()
            .enumerate()
            .map(|(i, entry)| match entry {
                Value::String(s) => SeriesEntry {
                    id: i as u32,
                    y_field: s.clone(),
                    label: String::new(),
                },
                _ => SeriesEntry {
                    id: i as u32,
                    y_field: entry
                        .get("field")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    label: entry
                        .get("label")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                },
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Derive the Visual-tab series view-state from the AST.
///
/// If the AST has no series entries, returns a single blank row so the user
/// always has something to type into. The returned list is the authoritative
/// editable state for the Visual tab's series list — see the `visual_series`
/// signal in `ChartBuilderModal`.
fn visual_series_from_ast(ast: &Value) -> Vec<SeriesEntry> {
    let series = ast_get_series(ast);
    if series.is_empty() {
        vec![SeriesEntry {
            id: 0,
            y_field: String::new(),
            label: String::new(),
        }]
    } else {
        series
    }
}

/// Next `SeriesEntry.id` to hand out, given an existing list. Used to keep
/// `<For>` keys stable across adds/removes.
fn next_id_after(entries: &[SeriesEntry]) -> u32 {
    entries.iter().map(|s| s.id).max().map(|m| m + 1).unwrap_or(1)
}

/// Write the series list back to `visualize.rows`, preserving the compact-form
/// conventions used on-disk: a single bare-field entry collapses to a string.
fn ast_set_series(ast: &mut Value, series: &[SeriesEntry]) {
    let non_empty: Vec<&SeriesEntry> = series.iter().filter(|s| !s.y_field.is_empty()).collect();

    ensure_root_mapping(ast);

    if non_empty.is_empty() {
        if let Some(vis) = ast.get_mut("visualize") {
            remove_key(vis, "rows");
        }
        return;
    }

    ensure_nested_mapping(ast, "visualize");
    let vis = ast.get_mut("visualize").expect("ensured");
    let vis_map = vis.as_mapping_mut().expect("ensured");

    // Single entry, no label → store as bare string for parity with React.
    if non_empty.len() == 1 && non_empty[0].label.is_empty() {
        vis_map.insert(
            Value::String("rows".to_string()),
            Value::String(non_empty[0].y_field.clone()),
        );
        return;
    }

    let rows: Vec<Value> = non_empty
        .iter()
        .map(|s| {
            if s.label.is_empty() {
                Value::String(s.y_field.clone())
            } else {
                let mut map = Mapping::new();
                map.insert(
                    Value::String("field".to_string()),
                    Value::String(s.y_field.clone()),
                );
                map.insert(
                    Value::String("label".to_string()),
                    Value::String(s.label.clone()),
                );
                Value::Mapping(map)
            }
        })
        .collect();

    vis_map.insert(Value::String("rows".to_string()), Value::Sequence(rows));
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
    /// Workspace UUID — used to construct a `KyomiDatasourceProvider` registered
    /// on the preview chartml so charts with `data: { datasource, query }` render
    /// in the visual editor. The modal mounts outside the dashboard's own
    /// DashboardChartProviders, so the provider must be supplied here.
    #[prop(into, optional)]
    workspace_id: String,
    /// Callback to close the modal.
    on_close: Callback<()>,
    /// Callback with the generated/updated ChartML YAML.
    on_insert: Callback<String>,
) -> impl IntoView {
    let is_edit_mode = existing_yaml.is_some();
    let existing_yaml_stored = StoredValue::new(existing_yaml.clone());

    // Must capture before spawn_local — the Leptos owner is lost across awaits,
    // so use_theme() returns None inside async closures.
    let initial_is_dark = crate::components::theme::use_theme()
        .map(|s| s.effective.get_untracked() == "dark")
        .unwrap_or(false);

    // ── Canonical state: the chart-document AST ─────────────────────────
    // All three sub-tabs read from and write to this signal. There is NO
    // effect that syncs AST → yaml_text; instead, every AST-mutating handler
    // in the Visual/AI tabs also writes the serialized AST into yaml_text
    // synchronously.
    let initial_ast_val = initial_ast(existing_yaml.as_deref());

    // Title used in the modal header — derived once at open time from the
    // AST; the header doesn't need to track live edits.
    let initial_title = ast_get_title(&initial_ast_val);

    let ast = RwSignal::new(initial_ast_val.clone());

    // ── Text buffer for the YAML editor ─────────────────────────────────
    // Tracks what the user is typing; never clobbered by an AST write.
    let initial_yaml_text = existing_yaml
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| serialize_ast(&initial_ast_val));
    let yaml_text = RwSignal::new(initial_yaml_text);

    // Inline parse error for the YAML editor; None = clean.
    let yaml_parse_error = RwSignal::new(None::<String>);

    // ── Visual-tab series view-state ────────────────────────────────────
    // The AST is the serialization authority — `ast_set_series` drops empty
    // entries so the saved YAML stays clean — but the Visual tab needs to
    // keep blank rows around while the user is typing (otherwise
    // "+ Add Series" would do nothing visible on a chart that already has
    // a series). `visual_series` is the editable list the Visual tab's
    // `<For>` iterates; non-empty entries are propagated into the AST on
    // every edit.
    let initial_visual_series = visual_series_from_ast(&initial_ast_val);
    let (next_series_id, set_next_series_id) = signal(next_id_after(&initial_visual_series));
    let visual_series = RwSignal::new(initial_visual_series);

    // ── Reset state when the modal opens with new yaml ──────────────────
    Effect::new(move || {
        if open.get() {
            let yaml = existing_yaml_stored.get_value();
            let new_ast = initial_ast(yaml.as_deref());

            // Derive Visual-tab series state (seeded with one blank if empty)
            // and re-seed the id counter so new rows never collide with the
            // ones we just loaded.
            let new_visual_series = visual_series_from_ast(&new_ast);
            set_next_series_id.set(next_id_after(&new_visual_series));

            // Text buffer: prefer the raw existing YAML so the user sees
            // exactly what was saved (preserves formatting/comments the AST
            // might drop). For a brand-new chart, use the seed serialization.
            let text = yaml
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| serialize_ast(&new_ast));

            ast.set(new_ast);
            visual_series.set(new_visual_series);
            yaml_text.set(text);
            yaml_parse_error.set(None);
        }
    });

    // ── Datasource options from server ──────────────────────────────────
    // Shared QueryCache entry — invalidated by `datasource_update` WS events
    // (see layout.rs QueryCacheWsBridge) so the dropdown stays fresh when
    // another tab/member creates, updates, or deletes a datasource.
    let datasources_signal = crate::query_cache::use_query(
        "datasources",
        || (),
        |_: ()| list_datasources(),
    );

    // Derive DynSelect options: (slug, "name (type)")
    let datasource_options = Signal::derive(move || {
        datasources_signal
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

    // ── Derived signals for Visual-tab inputs ───────────────────────────
    // Each getter reads only the slice of the AST it needs, so an edit to
    // e.g. the title doesn't invalidate the chart-type <DynSelect>.
    let title_sig = Signal::derive(move || ast.with(ast_get_title));
    let datasource_slug_sig = Signal::derive(move || ast.with(ast_get_datasource));
    let sql_sig = Signal::derive(move || ast.with(ast_get_query));
    let chart_type_sig = Signal::derive(move || ast.with(ast_get_chart_type));
    let x_field_sig = Signal::derive(move || ast.with(ast_get_x_field));
    let orientation_sig = Signal::derive(move || ast.with(ast_get_orientation));
    let mode_sig = Signal::derive(move || ast.with(ast_get_mode));

    // ── Helper: mutate AST and refresh the YAML text buffer atomically ──
    // Used by every Visual-tab / AI-tab handler that touches the AST. Keeping
    // both writes in one place guarantees yaml_text never lags behind the AST
    // (no feedback loop, no effect needed).
    //
    // This is a plain function that takes the signals as arguments. All three
    // signals here are `Copy` (RwSignal), so handlers can just capture them by
    // value and call `mutate_ast(ast, yaml_text, yaml_parse_error, |a| …)`.
    fn mutate_ast(
        ast: RwSignal<Value>,
        yaml_text: RwSignal<String>,
        yaml_parse_error: RwSignal<Option<String>>,
        f: impl FnOnce(&mut Value),
    ) {
        ast.update(f);
        yaml_text.set(ast.with_untracked(serialize_ast));
        yaml_parse_error.set(None);
    }

    // ── Series management ───────────────────────────────────────────────
    // Adds / removes operate on `visual_series` first, then propagate the
    // non-empty subset into the AST via `ast_set_series`. The blank row
    // produced by "+ Add Series" lives only in `visual_series` until the
    // user types a field name — this is what makes the row show up in the
    // Visual tab without polluting the saved YAML with empty entries.
    let add_series = move |_: web_sys::MouseEvent| {
        let id = next_series_id.get_untracked();
        set_next_series_id.set(id + 1);
        visual_series.update(|s| {
            s.push(SeriesEntry {
                id,
                y_field: String::new(),
                label: String::new(),
            });
        });
        // No AST mutation here — the new row is empty, so `ast_set_series`
        // would drop it anyway.
    };

    let remove_series = move |id: u32| {
        // Preserve the "always at least one row" invariant: refuse to remove
        // the final row so the user always has somewhere to type.
        if visual_series.with_untracked(|s| s.len()) <= 1 {
            return;
        }
        visual_series.update(|s| s.retain(|e| e.id != id));
        mutate_ast(ast, yaml_text, yaml_parse_error, move |a| {
            ast_set_series(a, &visual_series.get_untracked());
        });
    };

    // ── Save handler ────────────────────────────────────────────────────
    let handle_insert = Callback::new(move |()| {
        let yaml =
            serde_yaml::to_string(&wrap_as_sequence(&ast.get_untracked())).unwrap_or_default();
        on_insert.run(yaml);
        on_close.run(());
    });

    // ── Footer ──────────────────────────────────────────────────────────
    let cancel_class = format!(
        "{BTN_BASE} text-foreground hover:text-foreground hover:bg-secondary {BTN_SIZE}"
    );
    let insert_class = format!("{BTN_BASE} {BTN_DEFAULT} {BTN_SIZE}");

    let cancel_class_clone = cancel_class.clone();
    let insert_class_clone = insert_class.clone();

    let insert_label = if is_edit_mode { "Update Chart" } else { "Save Chart" };

    let footer_view: ChildrenFn = Arc::new(move || {
        let cancel_class = cancel_class_clone.clone();
        let insert_class = insert_class_clone.clone();

        // Disable insert: inline charts are always saveable, but remote charts
        // (datasource selected) require SQL to be useful.
        let is_disabled = if datasource_slug_sig.get().is_empty() {
            false // inline chart — no SQL required
        } else {
            sql_sig.get().trim().is_empty() // remote chart — SQL is required
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

    // ── SQL Editor on_change — writes to AST ────────────────────────────
    let sql_on_change: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |new_val: String| {
        mutate_ast(ast, yaml_text, yaml_parse_error, move |a| {
            ast_set_query(a, &new_val);
        });
    });
    let sql_on_change = StoredValue::new(sql_on_change);

    // ── YAML editor on_change ───────────────────────────────────────────
    // Writes the incoming text to yaml_text unconditionally (so cursor state
    // survives). Attempts a parse: on success, update the AST and clear the
    // error; on failure, surface the error but leave the AST untouched.
    let yaml_on_change: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |new_val: String| {
        yaml_text.set(new_val.clone());
        match parse_chart_yaml(&new_val) {
            Ok(doc) => {
                // Re-derive Visual-tab series state from the new AST so the
                // Visual tab reflects whatever the user just wrote in YAML
                // (e.g. adding a 3rd series). Bump the id counter past the
                // loaded rows so subsequent "+ Add Series" clicks don't
                // collide with freshly-loaded entry ids.
                let new_visual_series = visual_series_from_ast(&doc);
                set_next_series_id.set(next_id_after(&new_visual_series));
                visual_series.set(new_visual_series);
                ast.set(doc);
                yaml_parse_error.set(None);
            }
            Err(e) => {
                yaml_parse_error.set(Some(e.to_string()));
            }
        }
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
    // `ChartMLRef` is `Rc<ChartML>` on WASM (!Send + !Sync). Leptos signals
    // require Send + Sync on the inner value, so we wrap in SendWrapper —
    // safe because wasm32-unknown-unknown is single-threaded and the wrapper
    // panics if accessed from a different thread (which can never happen).
    let (preview_chartml, set_preview_chartml) =
        signal(None::<send_wrapper::SendWrapper<chartml_leptos::ChartMLRef>>);
    let (preview_loading, set_preview_loading) = signal(false);
    let (preview_error, set_preview_error) = signal(None::<String>);

    // Stable ChartML instance for the inline-data preview branch. Constructed
    // ONCE per modal mount and stashed in a `StoredValue` (Copy, survives
    // across reactive closures) so the preview closure can read the same
    // `ChartMLRef` on every re-render. Without this, the closure would call
    // `configured_chartml(...)` on every render — handing `<ChartMLChart>`
    // a fresh `chartml` prop each time, which restarts its internal Resource
    // and visibly never settles. `StoredValue::new_local` allows the
    // wasm-only `Rc`-backed `ChartMLRef` (`!Send`) without a SendWrapper.
    //
    // Also register `KyomiDatasourceProvider` on this chartml's resolver so
    // preview charts with `data: { datasource, query }` work — the modal
    // mounts outside DashboardChartProviders, so ChartMLChart's context
    // fallback finds nothing. We register directly here.
    let inline_chartml: StoredValue<chartml_leptos::ChartMLRef, LocalStorage> = {
        let chartml = configured_chartml("kyomi", initial_is_dark);
        if !workspace_id.is_empty() {
            let provider: chartml_leptos::ProviderRef = std::sync::Arc::new(
                crate::chartml_provider::KyomiDatasourceProvider::new(workspace_id.clone()),
            );
            chartml.resolver().register_provider("datasource", provider);
        }
        StoredValue::new_local(chartml)
    };

    // ── Auto-fetch preview data when opening with existing datasource + SQL ──
    #[cfg(target_arch = "wasm32")]
    {
        let initial_ds = ast_get_datasource(&initial_ast_val);
        let initial_sql = ast_get_query(&initial_ast_val);
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
                                        let is_dark = initial_is_dark;
                                        let colors = kyomi_palette("kyomi", is_dark);
                                        let theme = kyomi_theme(is_dark);
                                        let mut chartml_inst = chartml_core::ChartML::new();
                                        chartml_inst.register_renderer("bar", chartml_chart_cartesian::CartesianRenderer::new());
                                        chartml_inst.register_renderer("line", chartml_chart_cartesian::CartesianRenderer::new());
                                        chartml_inst.register_renderer("area", chartml_chart_cartesian::CartesianRenderer::new());
                                        chartml_inst.register_renderer("pie", chartml_chart_pie::PieRenderer::new());
                                        chartml_inst.register_renderer("doughnut", chartml_chart_pie::PieRenderer::new());
                                        chartml_inst.register_renderer("scatter", chartml_chart_scatter::ScatterRenderer::new());
                                        chartml_inst.register_renderer("metric", chartml_chart_metric::MetricRenderer::new());
                                        chartml_inst.register_renderer("table", chartml_chart_table::TableRenderer::new());
                                        chartml_inst.register_transform(chartml_datafusion::DataFusionTransform);
                                        chartml_inst.set_default_palette(colors);
                                        chartml_inst.set_theme(theme);
                                        chartml_inst.register_source("_remote", data_table);
                                        set_preview_chartml.set(Some(send_wrapper::SendWrapper::new(
                                            chartml_leptos::ChartMLRef::new(chartml_inst),
                                        )));
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
        format!("Chart Builder: {initial_title}")
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
    /// Active border routes through `border-primary` (design token) rather than
    /// the hardcoded Tailwind palette `border-amber-600` so it tracks DESIGN.md
    /// primary color changes automatically.
    const TAB_ACTIVE: &str =
        "px-1 py-3 text-sm font-medium border-b-2 border-primary text-primary transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring focus-visible:ring-offset-1 rounded-sm";
    const TAB_INACTIVE: &str =
        "px-1 py-3 text-sm font-medium border-b-2 border-transparent text-muted-foreground hover:text-foreground hover:border-border transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring focus-visible:ring-offset-1 rounded-sm";

    /// CSS for config sub-tab pills (segmented-control pattern — the active
    /// face reads as a card raised above the muted container).
    const SUB_TAB_ACTIVE: &str =
        "px-3 py-1 text-xs font-medium rounded bg-background text-foreground shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
    const SUB_TAB_INACTIVE: &str =
        "px-3 py-1 text-xs font-medium rounded text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";

    /// CSS for modifier chip — active state.
    const CHIP_ACTIVE: &str =
        "inline-flex items-center px-2.5 py-0.5 text-xs font-medium rounded-full border transition-colors bg-primary/10 border-primary/50 text-primary";
    /// CSS for modifier chip — inactive state.
    const CHIP_INACTIVE: &str =
        "inline-flex items-center px-2.5 py-0.5 text-xs font-medium rounded-full border transition-colors bg-transparent border-border text-muted-foreground hover:border-foreground hover:text-foreground";

    // ── Preview YAML — derived from the AST ─────────────────────────────
    // The preview pane renders the current AST serialization, not the raw
    // text buffer, so invalid YAML in the editor doesn't blank the preview.
    let preview_yaml = Signal::derive(move || ast.with(serialize_ast));

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
                        on:click=move |_| set_active_tab.set("chart".to_string())
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
                                    <Icon icon=phosphor_leptos::DATABASE attr:class="w-4 h-4 text-muted-foreground flex-shrink-0" />
                                    <div class="w-full sm:w-[240px] min-w-0 sm:flex-shrink-0">
                                        <Suspense fallback=move || view! {
                                            <div class="text-sm text-muted-foreground">"Loading datasources..."</div>
                                        }>
                                            <DynSelect
                                                value=datasource_slug_sig
                                                options=datasource_options
                                                on_change=move |slug: String| {
                                                    mutate_ast(ast, yaml_text, yaml_parse_error, move |a| {
                                                        ast_set_datasource(a, &slug);
                                                    });
                                                }
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
                                        <Icon icon=phosphor_leptos::DATABASE size="14px" />
                                        "Catalog"
                                    </button>
                                </div>

                                // SQL query editor — fills remaining space
                                <div class="flex-1 min-h-0">
                                    <SqlEditorSection
                                        content=sql_sig
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
                                        disabled=move || query_running.get() || datasource_slug_sig.get().is_empty() || sql_sig.get().trim().is_empty()
                                        on:click=move |_| {
                                            let ds_slug = datasource_slug_sig.get_untracked();
                                            let query_text = sql_sig.get_untracked();
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
                                    let slug = datasource_slug_sig.get();
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
                                                    <Icon icon=phosphor_leptos::ARROWS_CLOCKWISE size="14px" />
                                                </button>
                                                <button
                                                    type="button"
                                                    class="p-1 rounded-md hover:bg-secondary text-muted-foreground hover:text-foreground transition-colors"
                                                    title="Close catalog"
                                                    on:click=move |_| set_catalog_open.set(false)
                                                >
                                                    <Icon icon=phosphor_leptos::X size="14px" />
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
                                                                    mutate_ast(ast, yaml_text, yaml_parse_error, move |a| {
                                                                        let cur = ast_get_query(a);
                                                                        let next = if cur.is_empty() {
                                                                            format!("SELECT * FROM {table_id}")
                                                                        } else {
                                                                            format!("{cur} {table_id}")
                                                                        };
                                                                        ast_set_query(a, &next);
                                                                    });
                                                                })
                                                                on_column_click=Callback::new(move |col_name: String| {
                                                                    mutate_ast(ast, yaml_text, yaml_parse_error, move |a| {
                                                                        let cur = ast_get_query(a);
                                                                        let next = if cur.is_empty() {
                                                                            col_name.clone()
                                                                        } else {
                                                                            format!("{cur} {col_name}")
                                                                        };
                                                                        ast_set_query(a, &next);
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
                                                        <Icon icon=phosphor_leptos::CLOCK size="40px" attr:class="text-muted-foreground mb-2" />
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
                                        on:click=move |_| set_config_tab.set("yaml".to_string())
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
                                                    value=chart_type_sig
                                                    options=Signal::stored(
                                                        CHART_TYPES
                                                            .iter()
                                                            .map(|(k, v)| (k.to_string(), v.to_string()))
                                                            .collect::<Vec<_>>()
                                                    )
                                                    on_change=move |ct: String| {
                                                        mutate_ast(ast, yaml_text, yaml_parse_error, move |a| {
                                                            // Clear incompatible modifiers on type change.
                                                            if ct != "bar" {
                                                                ast_set_orientation(a, None);
                                                            }
                                                            if ct != "bar" && ct != "area" {
                                                                ast_set_mode(a, None);
                                                            }
                                                            ast_set_chart_type(a, &ct);
                                                        });
                                                    }
                                                />

                                                // Modifier chips — contextual based on chart type
                                                // React: ChartVisualEditor lines 216-258
                                                {move || {
                                                    let ct = chart_type_sig.get();
                                                    (ct == "bar" || ct == "area").then(|| view! {
                                                        <div class="flex flex-wrap gap-2 mt-2">
                                                            // Horizontal chip (bar only)
                                                            {move || (chart_type_sig.get() == "bar").then(|| view! {
                                                                <button
                                                                    type="button"
                                                                    class=move || {
                                                                        if orientation_sig.get().as_deref() == Some("horizontal") { CHIP_ACTIVE } else { CHIP_INACTIVE }
                                                                    }
                                                                    on:click=move |_| {
                                                                        let is_horizontal = orientation_sig.get_untracked().as_deref() == Some("horizontal");
                                                                        mutate_ast(ast, yaml_text, yaml_parse_error, move |a| {
                                                                            if is_horizontal {
                                                                                ast_set_orientation(a, None);
                                                                            } else {
                                                                                ast_set_orientation(a, Some("horizontal"));
                                                                            }
                                                                        });
                                                                    }
                                                                >
                                                                    "Horizontal"
                                                                </button>
                                                            })}
                                                            // Grouped chip (bar only)
                                                            {move || (chart_type_sig.get() == "bar").then(|| view! {
                                                                <button
                                                                    type="button"
                                                                    class=move || {
                                                                        if mode_sig.get().as_deref() == Some("grouped") { CHIP_ACTIVE } else { CHIP_INACTIVE }
                                                                    }
                                                                    on:click=move |_| {
                                                                        let is_grouped = mode_sig.get_untracked().as_deref() == Some("grouped");
                                                                        mutate_ast(ast, yaml_text, yaml_parse_error, move |a| {
                                                                            if is_grouped {
                                                                                ast_set_mode(a, None);
                                                                            } else {
                                                                                ast_set_mode(a, Some("grouped"));
                                                                            }
                                                                        });
                                                                    }
                                                                >
                                                                    "Grouped"
                                                                </button>
                                                            })}
                                                            // Normalized chip (area only)
                                                            {move || (chart_type_sig.get() == "area").then(|| view! {
                                                                <button
                                                                    type="button"
                                                                    class=move || {
                                                                        if mode_sig.get().as_deref() == Some("normalized") { CHIP_ACTIVE } else { CHIP_INACTIVE }
                                                                    }
                                                                    on:click=move |_| {
                                                                        let is_normalized = mode_sig.get_untracked().as_deref() == Some("normalized");
                                                                        mutate_ast(ast, yaml_text, yaml_parse_error, move |a| {
                                                                            if is_normalized {
                                                                                ast_set_mode(a, None);
                                                                            } else {
                                                                                ast_set_mode(a, Some("normalized"));
                                                                            }
                                                                        });
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
                                                    prop:value=move || title_sig.get()
                                                    on:input=move |ev| {
                                                        let val = event_target_value(&ev);
                                                        mutate_ast(ast, yaml_text, yaml_parse_error, move |a| {
                                                            ast_set_title(a, &val);
                                                        });
                                                    }
                                                    placeholder="Chart title"
                                                />
                                            </div>

                                            // X Axis Field — hidden for pie/doughnut/metric
                                            {move || {
                                                let ct = chart_type_sig.get();
                                                let needs_axes = !matches!(ct.as_str(), "metric" | "pie" | "doughnut");
                                                needs_axes.then(|| view! {
                                                    <div class="space-y-2">
                                                        <label class=LABEL_CLASS>"X Axis Field"</label>
                                                        <input
                                                            type="text"
                                                            class=INPUT_CLASS
                                                            prop:value=move || x_field_sig.get()
                                                            on:input=move |ev| {
                                                                let val = event_target_value(&ev);
                                                                mutate_ast(ast, yaml_text, yaml_parse_error, move |a| {
                                                                    ast_set_x_field(a, &val);
                                                                });
                                                            }
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
                                                    // Iterate `visual_series` directly — it always
                                                    // contains at least one (possibly blank) entry
                                                    // thanks to `visual_series_from_ast` seeding,
                                                    // so no ghost-row fallback is needed.
                                                    each=move || visual_series.get()
                                                    key=|entry| entry.id
                                                    let:entry
                                                >
                                                    {
                                                        // Capture the stable entry id — not the
                                                        // position — so surviving rows after a
                                                        // remove still target the right entry.
                                                        let entry_id = entry.id;
                                                        let y_val = entry.y_field.clone();
                                                        let label_val = entry.label.clone();
                                                        let show_remove = move || visual_series.with(|s| s.len() > 1);

                                                        view! {
                                                            <div class="flex items-start gap-2">
                                                                <div class="flex-1 space-y-1">
                                                                    <input
                                                                        type="text"
                                                                        class=INPUT_CLASS
                                                                        prop:value=y_val.clone()
                                                                        on:input=move |ev| {
                                                                            let val = event_target_value(&ev);
                                                                            // Update view-state by id, then
                                                                            // propagate the non-empty subset
                                                                            // into the AST.
                                                                            visual_series.update(|s| {
                                                                                if let Some(e) = s.iter_mut().find(|e| e.id == entry_id) {
                                                                                    e.y_field = val;
                                                                                }
                                                                            });
                                                                            mutate_ast(ast, yaml_text, yaml_parse_error, move |a| {
                                                                                ast_set_series(a, &visual_series.get_untracked());
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
                                                                            visual_series.update(|s| {
                                                                                if let Some(e) = s.iter_mut().find(|e| e.id == entry_id) {
                                                                                    e.label = val;
                                                                                }
                                                                            });
                                                                            mutate_ast(ast, yaml_text, yaml_parse_error, move |a| {
                                                                                ast_set_series(a, &visual_series.get_untracked());
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
                                                                            on:click=move |_| remove_series(entry_id)
                                                                            title="Remove series"
                                                                        >
                                                                            <Icon icon=phosphor_leptos::X size="16px" />
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
                                            chart_yaml=preview_yaml
                                            on_chart_update=Callback::new(move |new_yaml: String| {
                                                // AI returns fresh YAML — replace the AST and sync the
                                                // text buffer. On parse failure, leave the AST alone and
                                                // surface the error in the YAML tab.
                                                match parse_chart_yaml(&new_yaml) {
                                                    Ok(doc) => {
                                                        // Keep the Visual tab and id counter in sync
                                                        // with the AI-produced AST, same as the YAML
                                                        // editor path.
                                                        let new_visual_series = visual_series_from_ast(&doc);
                                                        set_next_series_id.set(next_id_after(&new_visual_series));
                                                        visual_series.set(new_visual_series);
                                                        ast.set(doc);
                                                        yaml_text.set(ast.with_untracked(serialize_ast));
                                                        yaml_parse_error.set(None);
                                                    }
                                                    Err(e) => {
                                                        yaml_parse_error.set(Some(e.to_string()));
                                                    }
                                                }
                                            })
                                        />
                                    })}

                                    // ── YAML sub-tab ────────────────────
                                    {move || (config_tab.get() == "yaml").then(|| view! {
                                        <div class="flex flex-col h-full min-h-[400px]">
                                            // Inline parse error banner — shown only while the buffer
                                            // fails to parse. The AST keeps showing the last-known-good
                                            // state to the Visual tab in the meantime.
                                            {move || yaml_parse_error.get().map(|msg| view! {
                                                <div class="mx-4 mt-3 border border-error-border rounded-md bg-error p-3">
                                                    <p class="text-sm text-error-foreground font-medium">"YAML Parse Error"</p>
                                                    <p class="text-xs text-error-foreground mt-1 font-mono whitespace-pre-wrap">{msg}</p>
                                                </div>
                                            })}
                                            <div class="flex-1 min-h-0">
                                                <YamlEditorSection
                                                    content=Signal::derive(move || yaml_text.get())
                                                    on_change=yaml_on_change.get_value()
                                                />
                                            </div>
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
                                            let ds_slug = datasource_slug_sig.get_untracked();
                                            let query_text = sql_sig.get_untracked();
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
                                                                        let is_dark = initial_is_dark;
                                                                        let colors = kyomi_palette("kyomi", is_dark);
                                                                        let theme = kyomi_theme(is_dark);
                                                                        let mut chartml_inst = chartml_core::ChartML::new();
                                                                        chartml_inst.register_renderer("bar", chartml_chart_cartesian::CartesianRenderer::new());
                                                                        chartml_inst.register_renderer("line", chartml_chart_cartesian::CartesianRenderer::new());
                                                                        chartml_inst.register_renderer("area", chartml_chart_cartesian::CartesianRenderer::new());
                                                                        chartml_inst.register_renderer("pie", chartml_chart_pie::PieRenderer::new());
                                                                        chartml_inst.register_renderer("doughnut", chartml_chart_pie::PieRenderer::new());
                                                                        chartml_inst.register_renderer("scatter", chartml_chart_scatter::ScatterRenderer::new());
                                                                        chartml_inst.register_renderer("metric", chartml_chart_metric::MetricRenderer::new());
                                                                        chartml_inst.register_renderer("table", chartml_chart_table::TableRenderer::new());
                                                                        chartml_inst.register_transform(chartml_datafusion::DataFusionTransform);
                                                                        chartml_inst.set_default_palette(colors);
                                                                        chartml_inst.set_theme(theme);
                                                                        chartml_inst.register_source("_remote", data_table);
                                                                        set_preview_chartml.set(Some(send_wrapper::SendWrapper::new(
                                            chartml_leptos::ChartMLRef::new(chartml_inst),
                                        )));
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
                                                    <Icon icon=phosphor_leptos::ARROWS_CLOCKWISE size="14px" />
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
                                            // Render remote chart preview — data was fetched.
                                            // `preview_chartml` stores a SendWrapper<ChartMLRef> (Leptos
                                            // signals require Send + Sync but ChartMLRef is !Send on wasm),
                                            // so `.take()` unwraps it on the wasm main thread.
                                            let spec = rewrite_spec_for_remote(&preview_yaml.get());
                                            let chartml_inst = chartml_inst.take();
                                            view! {
                                                <ChartPreview
                                                    spec=spec
                                                    chartml=chartml_inst
                                                />
                                            }.into_any()
                                        } else if datasource_slug_sig.get().is_empty() {
                                            // Inline data chart — render directly without remote fetch.
                                            // Reuse the stable per-mount `inline_chartml` (created above
                                            // outside the reactive closure) — recreating it here would
                                            // re-mount `<ChartMLChart>` on every preview-yaml change and
                                            // the chart would never settle.
                                            let chartml_inst = inline_chartml.with_value(|c| c.clone());
                                            view! {
                                                <ChartPreview
                                                    spec=preview_yaml.get()
                                                    chartml=chartml_inst
                                                />
                                            }.into_any()
                                        } else {
                                            // Has datasource but no data fetched yet
                                            view! {
                                                <div class="flex items-center justify-center h-full">
                                                    <div class="text-center px-4">
                                                        <Icon icon=phosphor_leptos::CHART_BAR size="48px" attr:class="text-muted-foreground/30 mx-auto mb-3" />
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
    chartml: chartml_leptos::ChartMLRef,
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
        let theme = crate::pages::sql_editor::code_editor::use_editor_theme();

        view! {
            <div class="min-h-[200px] border border-input rounded-md overflow-hidden" style="height: calc(100vh - 420px);">
                <CodeEditor
                    language=Signal::stored(Language::new_static("sql"))
                    content=content
                    theme=theme
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
        let theme = crate::pages::sql_editor::code_editor::use_editor_theme();

        view! {
            <div class="h-full min-h-[400px] border-t border-border overflow-hidden">
                <CodeEditor
                    language=Signal::stored(Language::new_static("yaml"))
                    content=content
                    theme=theme
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
            empty_icon=Arc::new(|| view! { <Icon icon=phosphor_leptos::SPARKLE weight=IconWeight::Duotone size="64px" /> }.into_any())
            empty_title="Ask me anything about your chart!"
            empty_description="I can help you change chart types, adjust styling, or fix configuration issues."
            custom_ws_events=vec!["chart_update".to_string()]
            on_custom_ws_event=on_custom
        />
    }
}
