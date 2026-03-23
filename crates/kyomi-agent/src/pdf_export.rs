// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard PDF export service.
//!
//! Generates PDF exports of dashboards by:
//! 1. Extracting ChartML specs from dashboard markdown
//! 2. Resolving chart data (executing queries)
//! 3. Rendering charts to PNG via chartml-rs (Rust-native)
//! 4. Converting markdown + chart images to Typst markup
//! 5. Compiling Typst to PDF (pure Rust, no external deps)

use std::collections::HashMap;

use serde_json::{json, Value};
use tracing::{error, info};

use crate::chartml_factory;
use crate::chartml_utils;
use crate::d3_format;
use crate::markdown_to_typst;
use crate::pdf_typst;
use crate::tools::chart_data_resolver;
use crate::tools::QueryContext;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Chart width (px) optimized for A4 landscape within margins.
const PDF_CHART_WIDTH: u32 = 700;

/// Default chart height (px).
const PDF_CHART_HEIGHT: u32 = 400;

/// Render at 2x density (144 DPI) for crisp text/lines in PDF.
const PDF_CHART_DENSITY: u32 = 144;

/// Maximum rows in a PDF table.
const PDF_TABLE_MAX_ROWS: usize = 50;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Generate a PDF export of a dashboard.
///
/// Orchestrates the full pipeline: extract → resolve → render → assemble → PDF.
///
/// Returns raw PDF bytes.
pub async fn generate_dashboard_pdf(
    content: &str,
    title: &str,
    ctx: &QueryContext,
    _chart_renderer_url: &str,
    user_palette: &[String],
    parameter_values: Option<&Value>,
) -> Result<Vec<u8>, String> {
    // 1. Extract ChartML specs from dashboard content
    let extraction = chartml_utils::extract_chartml_specs(content);
    info!(
        specs = extraction.specs.len(),
        "PDF export: found charts in dashboard '{}'", title
    );

    let mut chart_png_bytes: HashMap<String, Vec<u8>> = HashMap::new(); // filename -> PNG bytes
    let mut chart_image_refs: HashMap<usize, String> = HashMap::new(); // spec_idx -> filename
    let mut typst_rendered: HashMap<usize, String> = HashMap::new(); // spec_idx -> Typst markup

    if !extraction.specs.is_empty() {
        // 2. Resolve data for all charts sequentially
        let mut resolved_specs: Vec<(usize, Value)> = Vec::new();

        for (idx, spec) in extraction.specs.iter().enumerate() {
            // Apply parameter values to the spec if provided
            let spec_to_resolve = if let Some(params) = parameter_values {
                apply_parameters(spec, params)
            } else {
                spec.clone()
            };

            match chart_data_resolver::resolve_chart_data(&spec_to_resolve, ctx).await {
                Ok(resolved) => resolved_specs.push((idx, resolved)),
                Err(e) => {
                    let chart_title = chartml_utils::get_chart_title(spec);
                    error!(
                        error = %e, chart_idx = idx,
                        "PDF export: failed to resolve data for chart '{}'", chart_title
                    );
                }
            }
        }

        // 3. Separate metric/table (native Typst) from chart specs
        let mut chart_render_specs: Vec<(usize, Value)> = Vec::new();

        for (idx, resolved) in &resolved_specs {
            match chartml_utils::get_visualize_type(resolved).as_deref() {
                Some("metric") => {
                    typst_rendered.insert(*idx, render_metric_typst_markup(resolved));
                }
                Some("table") => {
                    typst_rendered.insert(*idx, render_table_typst_markup(resolved));
                }
                _ => {
                    chart_render_specs.push((*idx, resolved.clone()));
                }
            }
        }

        // 4. Render non-metric/non-table charts via chartml-rs (Rust-native)
        for (idx, resolved_spec) in &chart_render_specs {
            let spec_height = resolved_spec
                .get("visualize")
                .and_then(|v| v.get("style"))
                .and_then(|v| v.get("height"))
                .and_then(|v| v.as_u64())
                .map(|h| h as u32);
            let chart_height = spec_height.unwrap_or(PDF_CHART_HEIGHT);

            let yaml = match serde_yaml::to_string(resolved_spec) {
                Ok(y) => y,
                Err(e) => {
                    error!(error = %e, chart_idx = idx, "PDF export: failed to serialize spec to YAML");
                    continue;
                }
            };

            match chartml_factory::render_chart_to_png(
                &yaml,
                PDF_CHART_WIDTH,
                chart_height,
                PDF_CHART_DENSITY,
                Some(user_palette),
            )
            .await
            {
                Ok(png_bytes) => {
                    let filename = format!("chart_{idx}.png");
                    chart_image_refs.insert(*idx, filename.clone());
                    chart_png_bytes.insert(filename, png_bytes);
                }
                Err(e) => {
                    let chart_title = chartml_utils::get_chart_title(resolved_spec);
                    error!(
                        error = %e, chart_idx = idx,
                        "PDF export: failed to render chart '{}'", chart_title
                    );
                }
            }
        }
    }

    // 5. Replace ChartML blocks with Typst markers
    let processed_content = replace_chartml_with_typst(
        content,
        &extraction,
        &chart_image_refs,
        &typst_rendered,
    );

    // 6. Convert processed markdown to Typst markup
    let typst_body = markdown_to_typst::markdown_to_typst(&processed_content);

    // 7. Wrap in full Typst document with page setup
    let typst_doc = pdf_typst::wrap_document(title, &typst_body);

    // 8. Compile Typst → PDF (pure Rust, no external deps)
    let pdf_bytes = pdf_typst::generate_pdf(&typst_doc, &chart_png_bytes)?;

    info!(
        bytes = pdf_bytes.len(),
        "PDF export: generated PDF for dashboard '{}'", title
    );

    Ok(pdf_bytes)
}

// ---------------------------------------------------------------------------
// Parameter application
// ---------------------------------------------------------------------------

/// Apply parameter values to a ChartML spec.
///
/// Replaces `{{param_name}}` placeholders in query strings.
fn apply_parameters(spec: &Value, params: &Value) -> Value {
    let mut spec = spec.clone();

    let params_obj = match params.as_object() {
        Some(obj) => obj,
        None => return spec,
    };

    // Replace in data.query
    if let Some(query) = spec
        .get_mut("data")
        .and_then(|d| d.get_mut("query"))
        .and_then(|q| q.as_str())
        .map(String::from)
    {
        let mut replaced = query;
        for (key, val) in params_obj {
            let placeholder = format!("{{{{{key}}}}}");
            let replacement = match val.as_str() {
                Some(s) => s.to_string(),
                None => val.to_string(),
            };
            replaced = replaced.replace(&placeholder, &replacement);
        }
        spec["data"]["query"] = Value::String(replaced);
    }

    // Replace in named source queries
    if let Some(data) = spec.get("data").and_then(|d| d.as_object()).cloned() {
        for (source_name, source_val) in &data {
            if let Some(query) = source_val.get("query").and_then(|q| q.as_str()) {
                let mut replaced = query.to_string();
                for (key, val) in params_obj {
                    let placeholder = format!("{{{{{key}}}}}");
                    let replacement = match val.as_str() {
                        Some(s) => s.to_string(),
                        None => val.to_string(),
                    };
                    replaced = replaced.replace(&placeholder, &replacement);
                }
                spec["data"][source_name]["query"] = Value::String(replaced);
            }
        }
    }

    spec
}

// ---------------------------------------------------------------------------
// ChartML block replacement
// ---------------------------------------------------------------------------

/// Replace ChartML code blocks with Typst image references or rendered Typst markup.
fn replace_chartml_with_typst(
    content: &str,
    extraction: &chartml_utils::ExtractionResult,
    chart_image_refs: &HashMap<usize, String>,
    typst_rendered: &HashMap<usize, String>,
) -> String {
    if extraction.blocks.is_empty() {
        return content.to_string();
    }

    let mut result = content.to_string();

    for block in extraction.blocks.iter().rev() {
        if block.spec_indices.is_empty() {
            result.replace_range(block.range.clone(), "");
            continue;
        }

        let parts: Vec<String> = block
            .spec_indices
            .iter()
            .map(|&spec_idx| {
                // Native Typst rendering (metric, table)
                if let Some(typst) = typst_rendered.get(&spec_idx) {
                    return typst.clone();
                }
                // PNG chart image reference
                if let Some(filename) = chart_image_refs.get(&spec_idx) {
                    format!(
                        "#align(center)[#image(\"{filename}\", width: 100%)]",
                    )
                } else {
                    let chart_title = if spec_idx < extraction.specs.len() {
                        chartml_utils::get_chart_title(&extraction.specs[spec_idx])
                    } else {
                        "Chart".to_string()
                    };
                    let escaped = pdf_typst::typst_escape(&chart_title);
                    format!(
                        r##"#block(fill: rgb("#f9fafb"), stroke: rgb("#e5e7eb"), radius: 6pt, inset: 24pt, width: 100%)[
  #align(center)[#text(fill: rgb("#6b7280"), style: "italic")[Chart unavailable: {escaped}]]
]"##
                    )
                }
            })
            .collect();

        let block_replacement = parts.join("\n");
        result.replace_range(block.range.clone(), &block_replacement);
    }

    result
}

/// Render a metric spec as Typst markup for PDF export.
fn render_metric_typst_markup(spec: &Value) -> String {
    let visualize = spec.get("visualize").cloned().unwrap_or(json!({}));
    let data = spec.get("data").cloned().unwrap_or(json!({}));
    let rows = data
        .get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    let title = chartml_utils::get_chart_title(spec);

    if rows.is_empty() {
        return markdown_to_typst::render_metric_typst(&title, "No data", None);
    }

    let first_row = &rows[0];
    let value_field = visualize
        .get("value")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let raw_value = first_row.get(value_field).cloned().unwrap_or(json!(null));
    let format_str = visualize
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let formatted = if format_str.is_empty() {
        match &raw_value {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    } else {
        match raw_value.as_f64() {
            Some(n) => d3_format::format_d3(Some(&json!(n)), Some(format_str)),
            None => raw_value.to_string(),
        }
    };

    // Trend calculation
    let trend = if let Some(compare_field) = visualize.get("compareWith").and_then(|v| v.as_str()) {
        let current = raw_value.as_f64().unwrap_or(0.0);
        let previous = first_row
            .get(compare_field)
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        if previous != 0.0 {
            let pct_change = ((current - previous) / previous.abs()) * 100.0;
            let invert = visualize
                .get("invertTrend")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let is_positive = if invert {
                pct_change <= 0.0
            } else {
                pct_change >= 0.0
            };
            Some((format!("{:.1}%", pct_change.abs()), is_positive))
        } else {
            None
        }
    } else {
        None
    };

    let trend_ref = trend
        .as_ref()
        .map(|(pct, positive)| (pct.as_str(), *positive));

    markdown_to_typst::render_metric_typst(&title, &formatted, trend_ref)
}

/// Render a table spec as Typst markup for PDF export.
fn render_table_typst_markup(spec: &Value) -> String {
    let visualize = spec.get("visualize").cloned().unwrap_or(json!({}));
    let data = spec.get("data").cloned().unwrap_or(json!({}));
    let rows = data
        .get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    let title = chartml_utils::get_chart_title(spec);

    if rows.is_empty() {
        return markdown_to_typst::render_metric_typst(&title, "No data available", None);
    }

    // Get columns from spec or auto-detect from first row
    let columns: Vec<(String, String)> = if let Some(cols) = visualize.get("columns").and_then(|c| c.as_array()) {
        cols.iter()
            .map(|col| {
                let field = col
                    .get("field")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let label = col
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&field)
                    .to_string();
                (field, label)
            })
            .collect()
    } else {
        // Auto-detect columns from first row's keys
        match rows[0].as_object() {
            Some(obj) => obj.keys().map(|k| (k.clone(), k.clone())).collect(),
            None => vec![],
        }
    };

    let total_rows = rows.len();
    let display_rows = rows.iter().take(PDF_TABLE_MAX_ROWS);

    let headers: Vec<String> = columns.iter().map(|(_, label): &(String, String)| label.clone()).collect();
    let row_data: Vec<Vec<String>> = display_rows
        .map(|row| {
            columns
                .iter()
                .map(|(field, _)| {
                    match row.get(field) {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Number(n)) => n.to_string(),
                        Some(Value::Bool(b)) => b.to_string(),
                        Some(Value::Null) | None => String::new(),
                        Some(other) => other.to_string(),
                    }
                })
                .collect()
        })
        .collect();

    markdown_to_typst::render_data_table_typst(
        &title,
        &headers,
        &row_data,
        total_rows,
        PDF_TABLE_MAX_ROWS,
    )
}


// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- apply_parameters --

    #[test]
    fn apply_params_to_query() {
        let spec = json!({
            "type": "chart",
            "data": {
                "datasource": "prod",
                "query": "SELECT * FROM orders WHERE region = '{{region}}'"
            }
        });
        let params = json!({"region": "North"});
        let result = apply_parameters(&spec, &params);
        assert_eq!(
            result["data"]["query"].as_str().unwrap(),
            "SELECT * FROM orders WHERE region = 'North'"
        );
    }
}
