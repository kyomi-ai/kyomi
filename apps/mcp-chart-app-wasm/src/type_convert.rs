// SPDX-License-Identifier: AGPL-3.0-or-later

//! Port of `convertVisualizeForTypeChange()` from the JS MCP app.
//!
//! When a user switches chart type across categories (chart ↔ table ↔ metric),
//! the visualize structure must be converted to match the target type's schema.

use serde_json::{json, Map, Value};

/// Classify a chart type into its category.
fn category(chart_type: &str) -> &'static str {
    match chart_type {
        "table" => "table",
        "metric" => "metric",
        _ => "chart",
    }
}

/// Convert the `visualize` object when switching between incompatible type categories.
///
/// Operates in-place on the visualize map. Does nothing if the categories are the same.
pub fn convert_visualize_for_type_change(
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
                clean.remove("width");
                if clean.len() == 1 && clean.contains_key("field") {
                    Some(Value::String(field))
                } else {
                    Some(Value::Object(clean))
                }
            }
            None => Some(Value::String(field)),
        }
    }).collect();

    viz.remove("columns");
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
    viz.remove("columns");
    viz.remove("rows");
}

fn metric_to_chart(viz: &mut Map<String, Value>) {
    let value = viz.remove("value");
    viz.remove("label");
    viz.remove("format");
    viz.remove("compareWith");
    viz.remove("invertTrend");
    viz.remove("columns");

    let rows = if let Some(v) = value {
        vec![v]
    } else {
        vec![]
    };
    viz.insert("rows".to_string(), Value::Array(rows));
}

fn metric_to_table(viz: &mut Map<String, Value>) {
    let value = viz.remove("value");
    let label = viz.remove("label");
    let format = viz.remove("format");
    viz.remove("compareWith");
    viz.remove("invertTrend");
    viz.remove("rows");

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
    let columns_val = viz.remove("columns");
    let rows_val = viz.remove("rows");

    let mut cols = Vec::new();

    // columns (x-axis) → first table column(s)
    if let Some(cv) = columns_val {
        match cv {
            Value::Array(arr) => {
                for item in arr {
                    if let Some(obj) = to_field_object(&item) {
                        cols.push(obj);
                    }
                }
            }
            _ => {
                if let Some(obj) = to_field_object(&cv) {
                    cols.push(obj);
                }
            }
        }
    }

    // rows (y-axis) → additional table columns
    if let Some(Value::Array(arr)) = rows_val {
        for item in arr {
            if let Some(obj) = to_field_object(&item) {
                cols.push(obj);
            }
        }
    }

    viz.insert("columns".to_string(), Value::Array(cols));
}

fn chart_to_metric(viz: &mut Map<String, Value>) {
    let rows_val = viz.remove("rows");
    viz.remove("columns");

    // Use first row field as metric value
    if let Some(Value::Array(arr)) = rows_val {
        if let Some(first) = arr.first() {
            if let Some(field) = extract_field(first) {
                viz.insert("value".to_string(), Value::String(field));
            }
        }
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
}
