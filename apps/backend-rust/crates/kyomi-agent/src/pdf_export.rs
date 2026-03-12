// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dashboard PDF export service.
//!
//! Generates PDF exports of dashboards by:
//! 1. Extracting ChartML specs from dashboard markdown
//! 2. Resolving chart data (executing queries)
//! 3. Rendering charts to PNG via chart-renderer service
//! 4. Building styled HTML with embedded chart images
//! 5. Converting HTML to PDF via chart-renderer's WeasyPrint endpoint
//!
//! Ports Python's `dashboard_pdf_export_service.py`.

use std::collections::HashMap;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::{json, Value};
use tracing::{error, info, warn};

use crate::chartml_utils;
use crate::d3_format;
use crate::tools::chart_data_resolver;
use crate::tools::chart_renderer::ChartRendererClient;
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
    chart_renderer_url: &str,
    user_palette: &[String],
    parameter_values: Option<&Value>,
) -> Result<Vec<u8>, String> {
    // 1. Extract ChartML specs from dashboard content
    let extraction = chartml_utils::extract_chartml_specs(content);
    info!(
        specs = extraction.specs.len(),
        "PDF export: found charts in dashboard '{}'", title
    );

    let mut chart_images: HashMap<usize, String> = HashMap::new(); // idx -> base64 PNG
    let mut html_rendered: HashMap<usize, String> = HashMap::new(); // idx -> native HTML

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

        // 3. Separate metric/table (native HTML) from chart specs
        let mut chart_render_specs: Vec<(usize, Value)> = Vec::new();

        for (idx, resolved) in &resolved_specs {
            match chartml_utils::get_visualize_type(resolved).as_deref() {
                Some("metric") => {
                    html_rendered.insert(*idx, render_metric_html(resolved));
                }
                Some("table") => {
                    html_rendered.insert(*idx, render_table_html(resolved));
                }
                _ => {
                    chart_render_specs.push((*idx, resolved.clone()));
                }
            }
        }

        // 4. Render non-metric/non-table charts via chart-renderer
        if !chart_render_specs.is_empty() && !chart_renderer_url.is_empty() {
            match ChartRendererClient::new(chart_renderer_url) {
                Ok(renderer) if renderer.health_check().await => {
                    for (idx, resolved_spec) in &chart_render_specs {
                        // Use the spec's intended height if set
                        let spec_height = resolved_spec
                            .get("visualize")
                            .and_then(|v| v.get("style"))
                            .and_then(|v| v.get("height"))
                            .and_then(|v| v.as_u64())
                            .map(|h| h as u32);
                        let chart_height = spec_height.unwrap_or(PDF_CHART_HEIGHT);

                        match renderer
                            .render_chart(
                                resolved_spec,
                                PDF_CHART_WIDTH,
                                chart_height,
                                Some(user_palette),
                                Some(PDF_CHART_DENSITY),
                            )
                            .await
                        {
                            Ok(png_bytes) => {
                                chart_images.insert(*idx, BASE64.encode(&png_bytes));
                            }
                            Err(e) => {
                                let chart_title =
                                    chartml_utils::get_chart_title(resolved_spec);
                                error!(
                                    error = %e, chart_idx = idx,
                                    "PDF export: failed to render chart '{}'", chart_title
                                );
                            }
                        }
                    }
                }
                Ok(_) => {
                    warn!("PDF export: chart renderer is not healthy, charts will show placeholders");
                }
                Err(e) => {
                    warn!(error = %e, "PDF export: failed to create chart renderer client");
                }
            }
        }
    }

    // 5. Replace ChartML blocks with rendered content
    let processed_content =
        replace_chartml_with_images(content, &extraction, &chart_images, &html_rendered);

    // 6. Convert processed markdown to HTML
    let html_body = markdown_to_html(&processed_content);

    // 7. Wrap in full HTML document with PDF CSS
    let full_html = build_pdf_html(&html_body);

    // 8. Convert HTML to PDF via chart-renderer
    let renderer = ChartRendererClient::new(chart_renderer_url)?;
    let pdf_bytes = renderer.html_to_pdf(&full_html).await?;

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
// Metric rendering
// ---------------------------------------------------------------------------

/// Render a ChartML metric spec as a styled HTML card for PDF export.
fn render_metric_html(spec: &Value) -> String {
    let visualize = spec.get("visualize").cloned().unwrap_or(json!({}));
    let data = spec.get("data").cloned().unwrap_or(json!({}));
    let rows = data
        .get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    let title = chartml_utils::get_chart_title(spec);
    let value_field = visualize.get("value").and_then(|v| v.as_str());
    let fmt = visualize.get("format").and_then(|v| v.as_str());
    let label = visualize
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or(&title);
    let compare_field = visualize.get("compareWith").and_then(|v| v.as_str());
    let invert_trend = visualize
        .get("invertTrend")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if rows.is_empty() {
        return format!(
            "<div class=\"chart-container\">\
             <div class=\"pdf-metric\">\
             <div class=\"pdf-metric-label\">{}</div>\
             <div class=\"pdf-metric-value\">No data</div>\
             </div></div>",
            escape_html(label)
        );
    }

    let row = &rows[0];

    // Extract and format value
    let (formatted_value, raw_value) = if let Some(field) = value_field {
        match row.get(field) {
            Some(raw) if !raw.is_null() => (d3_format::format_d3(Some(raw), fmt), raw.as_f64()),
            _ => ("N/A".to_string(), None),
        }
    } else {
        ("N/A".to_string(), None)
    };

    // Build comparison/trend section
    let mut trend_html = String::new();
    if let (Some(cmp_field), Some(current_val)) = (compare_field, raw_value) {
        if let Some(compare_val) = row.get(cmp_field).and_then(|v| v.as_f64()) {
            if compare_val != 0.0 {
                let pct_change = ((current_val - compare_val) / compare_val.abs()) * 100.0;

                if pct_change == 0.0 {
                    trend_html =
                        "<div class=\"pdf-metric-trend\">\u{2014} No change vs previous</div>"
                            .to_string();
                } else {
                    let direction = if pct_change > 0.0 { "up" } else { "down" };
                    let arrow = if direction == "up" {
                        "\u{25B2}"
                    } else {
                        "\u{25BC}"
                    };
                    let css_class = if invert_trend {
                        if direction == "up" {
                            "pdf-metric-trend-down"
                        } else {
                            "pdf-metric-trend-up"
                        }
                    } else if direction == "up" {
                        "pdf-metric-trend-up"
                    } else {
                        "pdf-metric-trend-down"
                    };
                    trend_html = format!(
                        "<div class=\"pdf-metric-trend {css_class}\">\
                         {arrow} {:.1}% vs previous</div>",
                        pct_change.abs()
                    );
                }
            }
        }
    }

    format!(
        "<div class=\"chart-container\">\
         <div class=\"pdf-metric\">\
         <div class=\"pdf-metric-label\">{}</div>\
         <div class=\"pdf-metric-value\">{}</div>\
         {}\
         </div></div>",
        escape_html(label),
        escape_html(&formatted_value),
        trend_html
    )
}

// ---------------------------------------------------------------------------
// Table rendering
// ---------------------------------------------------------------------------

/// Render a ChartML table spec as an HTML table for PDF export.
fn render_table_html(spec: &Value) -> String {
    let visualize = spec.get("visualize").cloned().unwrap_or(json!({}));
    let data = spec.get("data").cloned().unwrap_or(json!({}));
    let rows = data
        .get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let title = chartml_utils::get_chart_title(spec);

    if rows.is_empty() {
        return format!(
            "<div class=\"chart-container\">\
             <div class=\"pdf-metric\">\
             <div class=\"pdf-metric-label\">{}</div>\
             <div class=\"pdf-metric-value\">No data available</div>\
             </div></div>",
            escape_html(&title)
        );
    }

    // Determine columns: use spec columns or auto-detect from first row
    let (columns, header_labels) = if let Some(columns_spec) = visualize.get("columns") {
        if let Some(arr) = columns_spec.as_array() {
            let mut cols = Vec::new();
            let mut labels = Vec::new();
            for col in arr {
                if let Some(obj) = col.as_object() {
                    let field = obj
                        .get("field")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let label = obj
                        .get("label")
                        .and_then(|v| v.as_str())
                        .unwrap_or(field);
                    cols.push(field.to_string());
                    labels.push(label.to_string());
                } else if let Some(s) = col.as_str() {
                    cols.push(s.to_string());
                    labels.push(s.to_string());
                }
            }
            (cols, labels)
        } else if let Some(s) = columns_spec.as_str() {
            (vec![s.to_string()], vec![s.to_string()])
        } else {
            columns_from_first_row(&rows)
        }
    } else {
        columns_from_first_row(&rows)
    };

    if columns.is_empty() {
        return format!(
            "<div class=\"chart-container\">\
             <div class=\"chart-placeholder\">Table unavailable: {}</div>\
             </div>",
            escape_html(&title)
        );
    }

    let total_rows = rows.len();
    let display_rows = &rows[..total_rows.min(PDF_TABLE_MAX_ROWS)];

    // Build header
    let header_cells: String = header_labels
        .iter()
        .map(|h| format!("<th>{}</th>", escape_html(h)))
        .collect();

    // Build body rows
    let body_rows: String = display_rows
        .iter()
        .map(|row| {
            if let Some(obj) = row.as_object() {
                let cells: String = columns
                    .iter()
                    .map(|col| {
                        let val = obj
                            .get(col.as_str())
                            .map(|v| {
                                if let Some(s) = v.as_str() {
                                    s.to_string()
                                } else if v.is_null() {
                                    String::new()
                                } else {
                                    v.to_string()
                                }
                            })
                            .unwrap_or_default();
                        format!("<td>{}</td>", escape_html(&val))
                    })
                    .collect();
                format!("<tr>{cells}</tr>")
            } else {
                format!("<tr><td>{}</td></tr>", escape_html(&row.to_string()))
            }
        })
        .collect();

    // Build footer for truncation
    let footer = if total_rows > PDF_TABLE_MAX_ROWS {
        format!(
            "<tr><td colspan=\"{}\" style=\"text-align: center; \
             font-style: italic; color: #6b7280; padding: 8pt;\">\
             Showing {} of {} rows</td></tr>",
            columns.len(),
            PDF_TABLE_MAX_ROWS,
            total_rows
        )
    } else {
        String::new()
    };

    let title_html = if title != "Chart" {
        format!(
            "<div class=\"chart-native-title\">{}</div>",
            escape_html(&title)
        )
    } else {
        String::new()
    };

    format!(
        "<div class=\"chart-container\">\
         {title_html}\
         <table class=\"pdf-table\">\
         <thead><tr>{header_cells}</tr></thead>\
         <tbody>{body_rows}{footer}</tbody>\
         </table></div>"
    )
}

/// Extract column names from the first data row.
fn columns_from_first_row(rows: &[Value]) -> (Vec<String>, Vec<String>) {
    if let Some(first) = rows.first() {
        if let Some(obj) = first.as_object() {
            let cols: Vec<String> = obj.keys().cloned().collect();
            let labels = cols.clone();
            return (cols, labels);
        }
    }
    (Vec::new(), Vec::new())
}

// ---------------------------------------------------------------------------
// ChartML block replacement
// ---------------------------------------------------------------------------

/// Replace ChartML code blocks with rendered images/HTML or placeholders.
fn replace_chartml_with_images(
    content: &str,
    extraction: &chartml_utils::ExtractionResult,
    chart_images: &HashMap<usize, String>,
    html_rendered: &HashMap<usize, String>,
) -> String {
    if extraction.blocks.is_empty() {
        return content.to_string();
    }

    let mut result = content.to_string();

    // Process blocks in reverse order so byte offsets stay valid
    for block in extraction.blocks.iter().rev() {
        if block.spec_indices.is_empty() {
            // Block had no chart components — remove it silently
            result.replace_range(block.range.clone(), "");
            continue;
        }

        // Build replacement for each spec in this block
        let parts: Vec<String> = block
            .spec_indices
            .iter()
            .map(|&spec_idx| {
                // Native HTML rendering (metric, table)
                if let Some(html) = html_rendered.get(&spec_idx) {
                    return html.clone();
                }
                // PNG chart image
                let chart_title = if spec_idx < extraction.specs.len() {
                    chartml_utils::get_chart_title(&extraction.specs[spec_idx])
                } else {
                    "Chart".to_string()
                };
                if let Some(b64) = chart_images.get(&spec_idx) {
                    format!(
                        "<div class=\"chart-container\">\
                         <img src=\"data:image/png;base64,{b64}\" \
                         alt=\"{chart_title}\" class=\"chart-image\" />\
                         </div>"
                    )
                } else {
                    format!(
                        "<div class=\"chart-placeholder\">\
                         Chart unavailable: {chart_title}\
                         </div>"
                    )
                }
            })
            .collect();

        let block_replacement = parts.join("\n");
        result.replace_range(block.range.clone(), &block_replacement);
    }

    result
}

// ---------------------------------------------------------------------------
// Markdown → HTML
// ---------------------------------------------------------------------------

/// Convert markdown text to HTML.
///
/// Handles headers, bold, italic, lists, tables, code, links, and horizontal rules.
/// Passes through HTML tags (for img/div tags we already inserted).
fn markdown_to_html(markdown_text: &str) -> String {
    let lines: Vec<&str> = markdown_text.split('\n').collect();
    let mut html_lines: Vec<String> = Vec::new();
    let mut in_ul = false;
    let mut in_ol = false;
    let mut in_table = false;

    // Regex patterns (compiled once)
    let re_header = regex::Regex::new(r"^(#{1,6})\s+(.*)").expect("valid regex");
    let re_table_sep =
        regex::Regex::new(r"^\|[\s\-:]+\|$").expect("valid regex");
    let re_ul = regex::Regex::new(r"^[-*+]\s+(.*)").expect("valid regex");
    let re_ol = regex::Regex::new(r"^\d+\.\s+(.*)").expect("valid regex");
    let re_hr = regex::Regex::new(r"^[-*_]{3,}$").expect("valid regex");

    for line in &lines {
        let stripped = line.trim();

        // Empty lines — close open lists/tables
        if stripped.is_empty() {
            if in_ul {
                html_lines.push("</ul>".into());
                in_ul = false;
            }
            if in_ol {
                html_lines.push("</ol>".into());
                in_ol = false;
            }
            if in_table {
                html_lines.push("</tbody></table>".into());
                in_table = false;
            }
            html_lines.push(String::new());
            continue;
        }

        // HTML passthrough (for img/div tags we already inserted)
        if stripped.starts_with("<div") || stripped.starts_with("<img") {
            if in_ul {
                html_lines.push("</ul>".into());
                in_ul = false;
            }
            if in_ol {
                html_lines.push("</ol>".into());
                in_ol = false;
            }
            html_lines.push(stripped.to_string());
            continue;
        }

        // Headers
        if let Some(caps) = re_header.captures(stripped) {
            if in_ul {
                html_lines.push("</ul>".into());
                in_ul = false;
            }
            if in_ol {
                html_lines.push("</ol>".into());
                in_ol = false;
            }
            let level = caps[1].len();
            let text = inline_formatting(&caps[2]);
            html_lines.push(format!("<h{level}>{text}</h{level}>"));
            continue;
        }

        // Table rows
        if stripped.contains('|') && stripped.starts_with('|') {
            // Table separator row
            let no_spaces = stripped.replace(' ', "");
            if re_table_sep.is_match(&no_spaces) {
                continue;
            }

            let cells: Vec<&str> = stripped
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim())
                .collect();

            if !in_table {
                html_lines.push("<table class=\"pdf-table\">".into());
                in_table = true;
                // First row is header
                let cells_html: String = cells
                    .iter()
                    .map(|c| format!("<th>{}</th>", inline_formatting(c)))
                    .collect();
                html_lines.push(format!("<thead><tr>{cells_html}</tr></thead><tbody>"));
                continue;
            }

            let cells_html: String = cells
                .iter()
                .map(|c| format!("<td>{}</td>", inline_formatting(c)))
                .collect();
            html_lines.push(format!("<tr>{cells_html}</tr>"));
            continue;
        }

        // Close table if we're no longer in one
        if in_table && !stripped.starts_with('|') {
            html_lines.push("</tbody></table>".into());
            in_table = false;
        }

        // Unordered list
        if let Some(caps) = re_ul.captures(stripped) {
            if !in_ul {
                html_lines.push("<ul>".into());
                in_ul = true;
            }
            html_lines.push(format!("<li>{}</li>", inline_formatting(&caps[1])));
            continue;
        }

        // Ordered list
        if let Some(caps) = re_ol.captures(stripped) {
            if !in_ol {
                html_lines.push("<ol>".into());
                in_ol = true;
            }
            html_lines.push(format!("<li>{}</li>", inline_formatting(&caps[1])));
            continue;
        }

        // Close lists if non-list line
        if in_ul {
            html_lines.push("</ul>".into());
            in_ul = false;
        }
        if in_ol {
            html_lines.push("</ol>".into());
            in_ol = false;
        }

        // Horizontal rule
        if re_hr.is_match(stripped) {
            html_lines.push("<hr />".into());
            continue;
        }

        // Regular paragraph
        html_lines.push(format!("<p>{}</p>", inline_formatting(stripped)));
    }

    // Close any remaining open tags
    if in_ul {
        html_lines.push("</ul>".into());
    }
    if in_ol {
        html_lines.push("</ol>".into());
    }
    if in_table {
        html_lines.push("</tbody></table>".into());
    }

    html_lines.join("\n")
}

/// Apply inline markdown formatting (bold, italic, code, links).
fn inline_formatting(text: &str) -> String {
    let re_bold_star = regex::Regex::new(r"\*\*(.+?)\*\*").expect("valid regex");
    let re_bold_under = regex::Regex::new(r"__(.+?)__").expect("valid regex");
    let re_italic_star = regex::Regex::new(r"\*(.+?)\*").expect("valid regex");
    // Rust's regex crate doesn't support look-around. Use a capturing group for the
    // preceding non-word char (or start of string) and restore it in the replacement.
    let re_italic_under =
        regex::Regex::new(r"(?:^|(\W))_(.+?)_(?:\W|$)").expect("valid regex");
    let re_code = regex::Regex::new(r"`(.+?)`").expect("valid regex");
    let re_link = regex::Regex::new(r"\[(.+?)\]\((.+?)\)").expect("valid regex");

    let text = re_bold_star.replace_all(text, "<strong>$1</strong>");
    let text = re_bold_under.replace_all(&text, "<strong>$1</strong>");
    let text = re_italic_star.replace_all(&text, "<em>$1</em>");
    let text = re_italic_under.replace_all(&text, "$1<em>$2</em>");
    let text = re_code.replace_all(&text, "<code>$1</code>");
    let text = re_link.replace_all(&text, "<a href=\"$2\">$1</a>");

    text.into_owned()
}

// ---------------------------------------------------------------------------
// HTML template
// ---------------------------------------------------------------------------

/// Wrap body HTML in a full HTML document with PDF-optimized CSS.
fn build_pdf_html(body_html: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
@page {{
    size: A4;
    margin: 2cm 1.5cm;
    @bottom-left {{
        content: counter(page) " / " counter(pages);
        font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
        font-size: 8pt;
        color: #9ca3af;
    }}
    @bottom-right {{
        content: "Generated by kyomi.ai";
        font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
        font-size: 8pt;
        color: #9ca3af;
    }}
}}

body {{
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    font-size: 11pt;
    line-height: 1.6;
    color: #1f2937;
    max-width: 100%;
}}

h1 {{
    font-size: 22pt;
    font-weight: 700;
    color: #111827;
    margin: 0 0 8pt 0;
    padding-bottom: 6pt;
    border-bottom: 2px solid #e5e7eb;
}}

h2 {{
    font-size: 16pt;
    font-weight: 600;
    color: #1f2937;
    margin: 18pt 0 8pt 0;
}}

h3 {{
    font-size: 13pt;
    font-weight: 600;
    color: #374151;
    margin: 14pt 0 6pt 0;
}}

h4, h5, h6 {{
    font-size: 11pt;
    font-weight: 600;
    color: #4b5563;
    margin: 12pt 0 4pt 0;
}}

p {{
    margin: 6pt 0;
}}

ul, ol {{
    margin: 6pt 0;
    padding-left: 20pt;
}}

li {{
    margin: 3pt 0;
}}

strong {{
    font-weight: 600;
}}

code {{
    font-family: "SFMono-Regular", Consolas, "Liberation Mono", Menlo, monospace;
    font-size: 9.5pt;
    background: #f3f4f6;
    padding: 1pt 4pt;
    border-radius: 3pt;
}}

a {{
    color: #2563eb;
    text-decoration: none;
}}

hr {{
    border: none;
    border-top: 1px solid #e5e7eb;
    margin: 12pt 0;
}}

.chart-container {{
    margin: 16pt 0;
    text-align: center;
    page-break-inside: avoid;
}}

.chart-native-title {{
    font-size: 12pt;
    font-weight: 600;
    color: #1f2937;
    text-align: left;
    margin: 0 0 8pt 0;
}}

.chart-image {{
    max-width: 100%;
    height: auto;
    border-radius: 6pt;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.1);
}}

.chart-placeholder {{
    margin: 16pt 0;
    padding: 24pt;
    background: #f9fafb;
    border: 1px solid #e5e7eb;
    border-radius: 6pt;
    text-align: center;
    color: #6b7280;
    font-style: italic;
    page-break-inside: avoid;
}}

.pdf-table {{
    width: 100%;
    border-collapse: collapse;
    margin: 12pt 0;
    font-size: 10pt;
    page-break-inside: avoid;
}}

.pdf-table th {{
    background: #f9fafb;
    font-weight: 600;
    text-align: left;
    padding: 6pt 8pt;
    border: 1px solid #e5e7eb;
}}

.pdf-table td {{
    padding: 5pt 8pt;
    border: 1px solid #e5e7eb;
}}

.pdf-table tr:nth-child(even) td {{
    background: #f9fafb;
}}

.pdf-metric {{
    margin: 12pt 0;
    padding: 16pt 20pt;
    background: #f9fafb;
    border: 1px solid #e5e7eb;
    border-radius: 8pt;
    text-align: center;
    page-break-inside: avoid;
}}

.pdf-metric-value {{
    font-size: 28pt;
    font-weight: 700;
    color: #111827;
    margin: 4pt 0;
}}

.pdf-metric-label {{
    font-size: 11pt;
    color: #6b7280;
    margin: 0 0 4pt 0;
}}

.pdf-metric-trend {{
    font-size: 10pt;
    margin-top: 6pt;
}}

.pdf-metric-trend-up {{
    color: #059669;
}}

.pdf-metric-trend-down {{
    color: #dc2626;
}}

</style>
</head>
<body>
{body_html}
</body>
</html>"#
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Escape HTML special characters.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- escape_html --

    #[test]
    fn escape_html_special_chars() {
        assert_eq!(escape_html("<div>&\"test\"</div>"), "&lt;div&gt;&amp;&quot;test&quot;&lt;/div&gt;");
    }

    #[test]
    fn escape_html_no_special_chars() {
        assert_eq!(escape_html("hello world"), "hello world");
    }

    // -- inline_formatting --

    #[test]
    fn inline_bold() {
        assert_eq!(inline_formatting("**bold**"), "<strong>bold</strong>");
    }

    #[test]
    fn inline_italic() {
        assert_eq!(inline_formatting("*italic*"), "<em>italic</em>");
    }

    #[test]
    fn inline_code() {
        assert_eq!(inline_formatting("`code`"), "<code>code</code>");
    }

    #[test]
    fn inline_link() {
        assert_eq!(
            inline_formatting("[text](http://example.com)"),
            "<a href=\"http://example.com\">text</a>"
        );
    }

    // -- markdown_to_html --

    #[test]
    fn markdown_headers() {
        let md = "# H1\n## H2\n### H3";
        let html = markdown_to_html(md);
        assert!(html.contains("<h1>H1</h1>"));
        assert!(html.contains("<h2>H2</h2>"));
        assert!(html.contains("<h3>H3</h3>"));
    }

    #[test]
    fn markdown_unordered_list() {
        let md = "- Item 1\n- Item 2";
        let html = markdown_to_html(md);
        assert!(html.contains("<ul>"));
        assert!(html.contains("<li>Item 1</li>"));
        assert!(html.contains("<li>Item 2</li>"));
        assert!(html.contains("</ul>"));
    }

    #[test]
    fn markdown_ordered_list() {
        let md = "1. First\n2. Second";
        let html = markdown_to_html(md);
        assert!(html.contains("<ol>"));
        assert!(html.contains("<li>First</li>"));
        assert!(html.contains("<li>Second</li>"));
        assert!(html.contains("</ol>"));
    }

    #[test]
    fn markdown_paragraph() {
        let html = markdown_to_html("Hello world");
        assert!(html.contains("<p>Hello world</p>"));
    }

    #[test]
    fn markdown_hr() {
        let html = markdown_to_html("---");
        assert!(html.contains("<hr />"));
    }

    #[test]
    fn markdown_html_passthrough() {
        let html = markdown_to_html("<div class=\"chart-container\">test</div>");
        assert!(html.contains("<div class=\"chart-container\">test</div>"));
    }

    #[test]
    fn markdown_table() {
        let md = "| Name | Value |\n|------|-------|\n| A | 1 |";
        let html = markdown_to_html(md);
        assert!(html.contains("<table class=\"pdf-table\">"));
        assert!(html.contains("<th>Name</th>"));
        assert!(html.contains("<td>A</td>"));
    }

    // -- render_metric_html --

    #[test]
    fn metric_html_basic() {
        let spec = json!({
            "title": "Revenue",
            "visualize": {"type": "metric", "value": "revenue", "format": "$,.0f"},
            "data": {"rows": [{"revenue": 42000}]},
        });
        let html = render_metric_html(&spec);
        assert!(html.contains("Revenue"));
        assert!(html.contains("$42,000"));
        assert!(html.contains("pdf-metric"));
    }

    #[test]
    fn metric_html_no_data() {
        let spec = json!({
            "title": "Empty",
            "visualize": {"type": "metric", "value": "val"},
            "data": {"rows": []},
        });
        let html = render_metric_html(&spec);
        assert!(html.contains("No data"));
    }

    #[test]
    fn metric_html_with_trend() {
        let spec = json!({
            "title": "Users",
            "visualize": {"type": "metric", "value": "current", "compareWith": "previous"},
            "data": {"rows": [{"current": 120, "previous": 100}]},
        });
        let html = render_metric_html(&spec);
        assert!(html.contains("\u{25B2}")); // ▲
        assert!(html.contains("20.0%"));
        assert!(html.contains("pdf-metric-trend-up"));
    }

    #[test]
    fn metric_html_inverted_trend() {
        let spec = json!({
            "title": "Costs",
            "visualize": {
                "type": "metric", "value": "current",
                "compareWith": "previous", "invertTrend": true
            },
            "data": {"rows": [{"current": 120, "previous": 100}]},
        });
        let html = render_metric_html(&spec);
        // Up is bad when inverted → should use down class
        assert!(html.contains("pdf-metric-trend-down"));
    }

    // -- render_table_html --

    #[test]
    fn table_html_basic() {
        let spec = json!({
            "title": "Sales",
            "visualize": {
                "type": "table",
                "columns": [
                    {"field": "region", "label": "Region"},
                    {"field": "revenue", "label": "Revenue"},
                ],
            },
            "data": {"rows": [
                {"region": "North", "revenue": 100},
                {"region": "South", "revenue": 200},
            ]},
        });
        let html = render_table_html(&spec);
        assert!(html.contains("pdf-table"));
        assert!(html.contains("<th>Region</th>"));
        assert!(html.contains("<td>North</td>"));
    }

    #[test]
    fn table_html_no_data() {
        let spec = json!({
            "title": "Empty",
            "visualize": {"type": "table"},
            "data": {"rows": []},
        });
        let html = render_table_html(&spec);
        assert!(html.contains("No data available"));
    }

    #[test]
    fn table_html_truncation() {
        let rows: Vec<Value> = (0..60).map(|i| json!({"id": i})).collect();
        let spec = json!({
            "title": "Big",
            "visualize": {"type": "table"},
            "data": {"rows": rows},
        });
        let html = render_table_html(&spec);
        assert!(html.contains("Showing 50 of 60 rows"));
    }

    // -- replace_chartml_with_images --

    #[test]
    fn replace_with_chart_image() {
        let content = "Before\n```chartml\ntype: chart\ntitle: Revenue\n```\nAfter";
        let extraction = chartml_utils::extract_chartml_specs(content);
        let mut images = HashMap::new();
        images.insert(0, "AAAA".to_string());
        let result = replace_chartml_with_images(content, &extraction, &images, &HashMap::new());
        assert!(result.contains("data:image/png;base64,AAAA"));
        assert!(!result.contains("chartml"));
    }

    #[test]
    fn replace_with_placeholder() {
        let content = "Before\n```chartml\ntype: chart\ntitle: Revenue\n```\nAfter";
        let extraction = chartml_utils::extract_chartml_specs(content);
        let result =
            replace_chartml_with_images(content, &extraction, &HashMap::new(), &HashMap::new());
        assert!(result.contains("Chart unavailable: Revenue"));
    }

    #[test]
    fn replace_with_native_html() {
        let content = "Before\n```chartml\ntype: chart\ntitle: Revenue\n```\nAfter";
        let extraction = chartml_utils::extract_chartml_specs(content);
        let mut html_rendered = HashMap::new();
        html_rendered.insert(0, "<div class=\"pdf-metric\">Custom</div>".to_string());
        let result =
            replace_chartml_with_images(content, &extraction, &HashMap::new(), &html_rendered);
        assert!(result.contains("Custom"));
    }

    // -- build_pdf_html --

    #[test]
    fn pdf_html_structure() {
        let html = build_pdf_html("<p>Test</p>");
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("@page"));
        assert!(html.contains("<p>Test</p>"));
        assert!(html.contains("kyomi.ai"));
    }

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
