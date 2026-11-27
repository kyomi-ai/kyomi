// SPDX-License-Identifier: AGPL-3.0-or-later

//! The one place a chart-type switch rewrites a `visualize` block.
//!
//! [`switch_visualize_type`] is what every surface with a chart-type picker
//! calls (the MCP chart app, dashboards and the chart builder), so the same
//! switch gives the same spec on all of them. It drops the settings only the old
//! type had (`orientation`, `mode`, per-row `mark`), then, across the
//! chart / table / metric categories, converts the fields (a port of
//! `convertVisualizeForTypeChange()` from the JS MCP app).

use serde_json::{json, Map, Value};

/// Remove `key` and return its value, keeping the order of the other keys.
///
/// `Map::remove` would swap the last key into the gap when serde_json's
/// `preserve_order` is on (the MCP chart app turns it on), which reorders the
/// author's keys; `retain` keeps them in place.
fn take_key(map: &mut Map<String, Value>, key: &str) -> Option<Value> {
    let value = map.get(key).cloned();
    map.retain(|k, _| k != key);
    value
}

/// Classify a chart type into its category.
fn category(chart_type: &str) -> &'static str {
    match chart_type {
        "table" => "table",
        "metric" => "metric",
        _ => "chart",
    }
}

/// Switch a `visualize` map from `from_type` to `to_type`, in place.
///
/// Sets `type` to `to_type`. When the type actually changes it also
/// 1. drops what the new type does not have: `orientation` unless the new type
///    is `bar`, `mode` unless it is `bar` or `area`, and the per-row `mark`
///    overrides so the new type applies uniformly; then
/// 2. converts the fields across categories (chart / table / metric).
///
/// The marks go first so they are not carried into a table's columns. Keys that
/// stay keep their position in `viz` (it keeps whatever order the map has).
pub fn switch_visualize_type(viz: &mut Map<String, Value>, from_type: &str, to_type: &str) {
    viz.insert("type".to_string(), Value::String(to_type.to_string()));
    if from_type == to_type {
        return;
    }
    strip_properties_incompatible_with(viz, to_type);
    convert_visualize_for_type_change(viz, from_type, to_type);
}

fn strip_properties_incompatible_with(viz: &mut Map<String, Value>, new_type: &str) {
    if new_type != "bar" {
        take_key(viz, "orientation");
    }
    if new_type != "bar" && new_type != "area" {
        take_key(viz, "mode");
    }
    if let Some(rows) = viz.get_mut("rows").and_then(Value::as_array_mut) {
        for row in rows {
            if let Some(row) = row.as_object_mut() {
                take_key(row, "mark");
            }
        }
    }
}

/// Convert the `visualize` object when switching between incompatible type categories.
///
/// Operates in-place on the visualize map. Does nothing if the categories are the same.
fn convert_visualize_for_type_change(
    viz: &mut Map<String, Value>,
    from_type: &str,
    to_type: &str,
) {
    let from_cat = category(from_type);
    let to_cat = category(to_type);

    if from_cat == to_cat {
        return;
    }

    match (from_cat, to_cat) {
        ("table", "chart") => table_to_chart(viz),
        ("table", "metric") => table_to_metric(viz),
        ("metric", "chart") => metric_to_chart(viz),
        ("metric", "table") => metric_to_table(viz),
        ("chart", "table") => chart_to_table(viz),
        ("chart", "metric") => chart_to_metric(viz),
        _ => {}
    }
}

/// Extract field name from a column value (string or object with "field" key).
fn extract_field(val: &Value) -> Option<String> {
    match val {
        Value::String(s) => Some(s.clone()),
        Value::Object(obj) => obj.get("field").and_then(|f| f.as_str()).map(String::from),
        _ => None,
    }
}

/// The entries of a `rows`/`columns` value: a list is its items, and a single
/// field (`rows: revenue`) is a list of one, as ChartML allows both.
fn entries(val: Value) -> Vec<Value> {
    match val {
        Value::Array(items) => items,
        other => vec![other],
    }
}

/// Convert a column value to an object with at least a "field" key.
fn to_field_object(val: &Value) -> Option<Value> {
    match val {
        Value::String(s) => Some(json!({ "field": s })),
        Value::Object(obj) if obj.contains_key("field") => Some(val.clone()),
        _ => None,
    }
}

fn table_to_chart(viz: &mut Map<String, Value>) {
    let cols: Vec<Value> = viz
        .get("columns")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();

    // First column → x-axis (columns), rest → y-axis (rows)
    let first = cols.first().and_then(extract_field);
    let rows: Vec<Value> = cols.iter().skip(1).filter_map(|c| {
        let field = extract_field(c)?;
        let obj = c.as_object();
        // Strip table-specific "width" property
        match obj {
            Some(o) => {
                let mut clean = o.clone();
                take_key(&mut clean, "width");
                if clean.len() == 1 && clean.contains_key("field") {
                    Some(Value::String(field))
                } else {
                    Some(Value::Object(clean))
                }
            }
            None => Some(Value::String(field)),
        }
    }).collect();

    take_key(viz, "columns");
    if let Some(x) = first {
        viz.insert("columns".to_string(), Value::String(x));
    }
    viz.insert("rows".to_string(), Value::Array(rows));
}

fn table_to_metric(viz: &mut Map<String, Value>) {
    let cols: Vec<Value> = viz
        .get("columns")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();

    // Use second column (or first) as the metric value
    let value_col = cols.get(1).or(cols.first());
    if let Some(field) = value_col.and_then(extract_field) {
        viz.insert("value".to_string(), Value::String(field));
    }
    take_key(viz, "columns");
    take_key(viz, "rows");
}

fn metric_to_chart(viz: &mut Map<String, Value>) {
    let value = take_key(viz, "value");
    take_key(viz, "label");
    take_key(viz, "format");
    take_key(viz, "compareWith");
    take_key(viz, "invertTrend");
    take_key(viz, "columns");

    let rows = if let Some(v) = value {
        vec![v]
    } else {
        vec![]
    };
    viz.insert("rows".to_string(), Value::Array(rows));
}

fn metric_to_table(viz: &mut Map<String, Value>) {
    let value = take_key(viz, "value");
    let label = take_key(viz, "label");
    let format = take_key(viz, "format");
    take_key(viz, "compareWith");
    take_key(viz, "invertTrend");
    take_key(viz, "rows");

    let mut cols = Vec::new();
    if let Some(Value::String(field)) = value {
        let mut col = Map::new();
        col.insert("field".to_string(), Value::String(field));
        if let Some(Value::String(l)) = label {
            col.insert("label".to_string(), Value::String(l));
        }
        if let Some(Value::String(f)) = format {
            col.insert("format".to_string(), Value::String(f));
        }
        cols.push(Value::Object(col));
    }
    viz.insert("columns".to_string(), Value::Array(cols));
}

fn chart_to_table(viz: &mut Map<String, Value>) {
    let columns_val = take_key(viz, "columns");
    let rows_val = take_key(viz, "rows");

    let mut cols = Vec::new();

    // columns (x-axis) → first table column(s)
    if let Some(cv) = columns_val {
        for item in entries(cv) {
            if let Some(obj) = to_field_object(&item) {
                cols.push(obj);
            }
        }
    }

    // rows (y-axis) → additional table columns
    if let Some(rv) = rows_val {
        for item in entries(rv) {
            if let Some(obj) = to_field_object(&item) {
                cols.push(obj);
            }
        }
    }

    viz.insert("columns".to_string(), Value::Array(cols));
}

fn chart_to_metric(viz: &mut Map<String, Value>) {
    let rows_val = take_key(viz, "rows");
    take_key(viz, "columns");

    // Use first row field as metric value
    if let Some(first) = rows_val.map(entries).and_then(|items| items.into_iter().next())
        && let Some(field) = extract_field(&first)
    {
        viz.insert("value".to_string(), Value::String(field));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_to_table_basic() {
        let mut viz = serde_json::from_value::<Map<String, Value>>(json!({
            "type": "table",
            "columns": "date",
            "rows": ["revenue", "cost"]
        })).unwrap();

        convert_visualize_for_type_change(&mut viz, "bar", "table");

        let cols = viz.get("columns").unwrap().as_array().unwrap();
        assert_eq!(cols.len(), 3);
        assert_eq!(cols[0]["field"], "date");
        assert_eq!(cols[1]["field"], "revenue");
        assert_eq!(cols[2]["field"], "cost");
        assert!(viz.get("rows").is_none());
    }

    #[test]
    fn table_to_chart_basic() {
        let mut viz = serde_json::from_value::<Map<String, Value>>(json!({
            "type": "bar",
            "columns": [
                {"field": "date"},
                {"field": "revenue"},
                {"field": "cost"}
            ]
        })).unwrap();

        convert_visualize_for_type_change(&mut viz, "table", "bar");

        assert_eq!(viz.get("columns").unwrap(), "date");
        let rows = viz.get("rows").unwrap().as_array().unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn same_category_noop() {
        let mut viz = serde_json::from_value::<Map<String, Value>>(json!({
            "type": "line",
            "columns": "date",
            "rows": ["revenue"]
        })).unwrap();

        let original = viz.clone();
        convert_visualize_for_type_change(&mut viz, "bar", "line");
        assert_eq!(viz, original);
    }

    #[test]
    fn a_single_field_rows_is_kept_when_switching_to_table_and_metric() {
        let mut to_table = serde_json::from_value::<Map<String, Value>>(json!({
            "type": "table",
            "columns": "region",
            "rows": "revenue"
        })).unwrap();
        convert_visualize_for_type_change(&mut to_table, "bar", "table");
        let fields: Vec<&Value> = to_table["columns"].as_array().unwrap().iter().map(|c| &c["field"]).collect();
        assert_eq!(fields, ["region", "revenue"]);

        let mut to_metric = serde_json::from_value::<Map<String, Value>>(json!({
            "type": "metric",
            "columns": "region",
            "rows": "revenue"
        })).unwrap();
        convert_visualize_for_type_change(&mut to_metric, "bar", "metric");
        assert_eq!(to_metric["value"], "revenue");
    }

    const TYPES: [&str; 8] = ["bar", "line", "area", "scatter", "pie", "doughnut", "table", "metric"];

    /// A bar chart authored with every setting some type drops.
    fn bar_with_every_setting() -> Map<String, Value> {
        serde_json::from_value(json!({
            "type": "bar",
            "columns": "region",
            "orientation": "horizontal",
            "mode": "stacked",
            "rows": [
                {"field": "revenue", "mark": "line", "label": "Revenue"},
                {"field": "cost"}
            ]
        })).unwrap()
    }

    /// Every ordered pair of picker types: `type` is the new type, `orientation`
    /// survives only on bar, `mode` only on bar and area, and a row's `mark` on
    /// none (a switch to the same type changes nothing).
    #[test]
    fn every_type_pair_drops_what_the_new_type_does_not_have() {
        for from in TYPES {
            for to in TYPES {
                let mut viz = bar_with_every_setting();
                viz.insert("type".to_string(), json!(from));
                let before = viz.clone();
                switch_visualize_type(&mut viz, from, to);
                let pair = format!("{from} -> {to}");
                assert_eq!(viz["type"], to, "{pair}");
                if from == to {
                    assert_eq!(viz, before, "{pair}");
                    continue;
                }
                assert_eq!(viz.contains_key("orientation"), to == "bar", "{pair}: {viz:?}");
                assert_eq!(viz.contains_key("mode"), to == "bar" || to == "area", "{pair}: {viz:?}");
                let marks = viz
                    .values()
                    .filter_map(Value::as_array)
                    .flatten()
                    .filter(|entry| entry.get("mark").is_some())
                    .count();
                assert_eq!(marks, 0, "{pair}: {viz:?}");
            }
        }
    }

    #[test]
    fn bar_to_line_keeps_the_fields_and_drops_orientation_mode_and_marks() {
        let mut viz = bar_with_every_setting();
        switch_visualize_type(&mut viz, "bar", "line");
        assert_eq!(
            Value::Object(viz),
            json!({
                "type": "line",
                "columns": "region",
                "rows": [{"field": "revenue", "label": "Revenue"}, {"field": "cost"}]
            })
        );
    }

    #[test]
    fn bar_to_table_drops_the_settings_and_the_marks_do_not_reach_the_columns() {
        let mut viz = bar_with_every_setting();
        switch_visualize_type(&mut viz, "bar", "table");
        assert_eq!(
            Value::Object(viz),
            json!({
                "type": "table",
                "columns": [
                    {"field": "region"},
                    {"field": "revenue", "label": "Revenue"},
                    {"field": "cost"}
                ]
            })
        );
    }

    #[test]
    fn bar_to_area_keeps_the_mode_and_drops_the_orientation() {
        let mut viz = bar_with_every_setting();
        switch_visualize_type(&mut viz, "bar", "area");
        assert_eq!(viz["mode"], "stacked");
        assert!(!viz.contains_key("orientation"));
    }

    #[test]
    fn bar_to_metric_names_the_value_and_drops_the_settings() {
        let mut viz = bar_with_every_setting();
        switch_visualize_type(&mut viz, "bar", "metric");
        assert_eq!(Value::Object(viz), json!({"type": "metric", "value": "revenue"}));
    }
}
