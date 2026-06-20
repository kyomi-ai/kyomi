// SPDX-License-Identifier: LicenseRef-Alytic-Enterprise

//! Slack Message Processor — converts Kyomi messages to Block Kit format.
//!
//! Ports Python's `slack_message_processor.py` + `slack_table_utils.py`.
//!
//! Pipeline:
//! 1. Extract and render ChartML (PNG charts, native metrics/tables)
//! 2. Replace ChartML blocks with positional markers
//! 3. Extract markdown tables → Block Kit table blocks
//! 4. Convert markdown → Slack mrkdwn (with divider markers)
//! 5. Assemble Block Kit blocks (text chunks, inline tables/charts/dividers)

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::client::SlackClient;
use crate::helpers::markdown_to_slack;
use serde_json::{json, Value};
use tracing::{info, warn};

use kyomi_agent::chartml_utils::{self, ExtractionResult};
use kyomi_agent::d3_format;
use kyomi_agent::tools::chart_data_resolver;
use kyomi_agent::tools::chart_palettes;
use kyomi_agent::tools::QueryContext;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum text length for a Slack section block.
const MAX_SECTION_LENGTH: usize = 2900;

/// Maximum charts to render as PNG per Slack message.
const MAX_CHARTS_RENDERED: usize = 2;

/// Maximum Block Kit blocks per Slack message.
const MAX_BLOCKS: usize = 50;

/// Maximum rows to show in a native table rendering.
const MAX_TABLE_ROWS: usize = 100;

/// Maximum columns for Slack Block Kit tables.
const MAX_TABLE_COLUMNS: usize = 20;

/// Maximum text length per Slack mrkdwn cell.
const MAX_CELL_LENGTH: usize = 2900;

/// Default chart render width (px).
const CHART_WIDTH: u32 = 800;

/// Default chart render height (px).
const CHART_HEIGHT: u32 = 400;

/// Chart render density (2× for retina sharpness on high-DPI displays;
/// matches the email path's EMAIL_CHART_DENSITY).
const CHART_DENSITY: u32 = 144;

/// Marker for the first valid markdown table.
const TABLE_MARKER: &str = "<<<SLACK_TABLE>>>";

/// Marker for divider blocks.
const DIVIDER_MARKER: &str = "<<<SLACK_DIVIDER>>>";

/// Regex for `<<<SLACK_CHART_N>>>` markers.
static RE_CHART_MARKER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"<<<SLACK_CHART_(\d+)>>>").expect("valid regex"));


/// Regex for markdown tables: header | separator | data rows.
static RE_MD_TABLE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?m)^(\|[^\n]+\|)\n(\|[-:\s|]+\|)\n((?:\|[^\n]+\|\n?)+)",
    )
    .expect("valid regex")
});

/// Regex for markdown table used in splitting (simpler, for finditer).
static RE_TABLE_SPLIT: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)(\|[^\n]+\|\n\|[-:\s|]+\|\n(?:\|[^\n]+\|\n)+)")
        .expect("valid regex")
});

/// Regex for headings (used in table splitting).
static RE_HEADING: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\n(#{1,6}\s+.+)\n").expect("valid regex"));

/// Regex for horizontal rules in Slack mrkdwn text.
static RE_HR: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)^\s*[-*_]{3,}\s*$").expect("valid regex")
});

/// Regex for 3+ consecutive newlines → collapse to 2.
static RE_EXCESS_NEWLINES: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\n{3,}").expect("valid regex"));

/// Regex for markdown bold: **text**
static RE_MD_BOLD: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\*\*(.+?)\*\*").expect("valid regex"));

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------


/// A parsed markdown table.
struct ParsedTable {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    alignments: Vec<String>,
    valid: bool,
    num_rows: usize,
    num_columns: usize,
    error: Option<String>,
}

/// Info about the next marker found in text.
struct MarkerInfo {
    position: usize,
    marker_type: MarkerType,
    marker_text: String,
}

enum MarkerType {
    Table,
    Chart(usize),
    Divider,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Split a message with multiple markdown tables into separate chunks.
///
/// Strategy: keep the first table in the first chunk, then split at headings
/// before subsequent tables. Returns a single chunk if 0 or 1 tables.
pub fn split_message_for_multiple_tables(message: &str) -> Vec<String> {
    let tables: Vec<_> = RE_TABLE_SPLIT.find_iter(message).collect();

    if tables.len() <= 1 {
        return vec![message.to_string()];
    }

    info!(
        table_count = tables.len(),
        "Splitting message into multiple Slack messages"
    );

    let mut chunks = Vec::new();
    let mut current_pos = 0usize;

    for (idx, table_match) in tables.iter().enumerate() {
        if idx == 0 {
            continue; // First table stays in first chunk
        }

        let table_start = table_match.start();
        let text_before_table = &message[current_pos..table_start];

        // Find last heading before this table
        let headings: Vec<_> = RE_HEADING.find_iter(text_before_table).collect();

        let split_point = if let Some(last_heading) = headings.last() {
            current_pos + last_heading.start() + 1 // +1 to keep newline before heading
        } else {
            table_start
        };

        let chunk = message[current_pos..split_point].trim_end();
        if !chunk.is_empty() {
            chunks.push(chunk.to_string());
        }
        current_pos = split_point;
    }

    // Final chunk
    let final_chunk = message[current_pos..].trim_end();
    if !final_chunk.is_empty() {
        chunks.push(final_chunk.to_string());
    }

    info!(
        chunk_count = chunks.len(),
        table_count = tables.len(),
        "Split message into chunks"
    );
    chunks
}

/// Process a message and build Slack Block Kit blocks.
///
/// This is the main entry point — orchestrates the full pipeline:
/// 1. Extract & render ChartML (PNG or native metric/table)
/// 2. Replace ChartML with markers
/// 3. Extract markdown tables
/// 4. Convert markdown → Slack mrkdwn
/// 5. Assemble blocks with inline markers
///
/// Returns `(blocks, fallback_text)`.
#[allow(clippy::too_many_arguments)]
pub async fn process_and_build_slack_blocks(
    message: &str,
    bot_token: &str,
    slack_client: &SlackClient,
    query_ctx: &QueryContext,
    footer_url: Option<&str>,
    footer_text: &str,
    header_text: Option<&str>,
    header_emoji: Option<&str>,
) -> (Vec<Value>, String) {
    // ── Stage 1: Extract and render ChartML ─────────────────────────────
    let extraction = extract_chartml_specs(message);
    let total_charts = extraction.specs.len();
    let charts_to_render = total_charts.min(MAX_CHARTS_RENDERED);

    let mut file_uploads: HashMap<usize, String> = HashMap::new(); // spec_idx -> file_id
    let mut native_blocks: HashMap<usize, Vec<Value>> = HashMap::new(); // spec_idx -> blocks
    let mut failed_indices: HashSet<usize> = HashSet::new();

    if !extraction.specs.is_empty() {
        let user_palette = chart_palettes::get_user_palette(&query_ctx.db, &query_ctx.user_id).await;

        for idx in 0..charts_to_render {
            let spec = &extraction.specs[idx];

            // Resolve chart data (execute queries → inline rows)
            let resolved_spec = match chart_data_resolver::resolve_chart_data(spec, query_ctx).await
            {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, chart_idx = idx, "Failed to resolve chart data");
                    failed_indices.insert(idx);
                    continue;
                }
            };

            let viz_type = get_visualize_type(&resolved_spec);

            // Native rendering for metric/table types
            match viz_type.as_deref() {
                Some("metric") => {
                    let blocks = render_metric_slack_blocks(&resolved_spec);
                    info!(chart_idx = idx, "Rendered metric natively");
                    native_blocks.insert(idx, blocks);
                    continue;
                }
                Some("table") => {
                    let blocks = render_table_slack_blocks(&resolved_spec);
                    info!(chart_idx = idx, "Rendered table natively");
                    native_blocks.insert(idx, blocks);
                    continue;
                }
                _ => {}
            }

            let chart_title = get_chart_title(&resolved_spec);
            info!(chart_title = %chart_title, "Rendering chart to PNG");

            // Convert resolved spec to YAML for chartml-rs
            let resolved_yaml = match serde_yaml::to_string(&resolved_spec) {
                Ok(y) => y,
                Err(e) => {
                    warn!(error = %e, chart_idx = idx, "Failed to serialize spec to YAML");
                    failed_indices.insert(idx);
                    continue;
                }
            };

            // Render via chartml-rs (Rust-native, no HTTP)
            match kyomi_agent::chartml_factory::render_chart_to_png(
                &resolved_yaml, CHART_WIDTH, CHART_HEIGHT, CHART_DENSITY, Some(&user_palette),
            ).await {
                Ok(png_bytes) => {
                    let filename = format!(
                        "{}_{}.png",
                        chart_title.replace(' ', "_"),
                        chrono::Utc::now().timestamp()
                    );
                    match slack_client
                        .upload_file_for_blocks(bot_token, &filename, &chart_title, png_bytes)
                        .await
                    {
                        Ok(upload) => {
                            file_uploads.insert(idx, upload.id);
                            info!(chart_idx = idx, "Uploaded chart to Slack");
                        }
                        Err(e) => {
                            warn!(error = %e, chart_idx = idx, "Failed to upload chart");
                            failed_indices.insert(idx);
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, chart_idx = idx, "Failed to render chart");
                    failed_indices.insert(idx);
                }
            }
        }
    }

    // Wait for Slack to process uploaded files
    if !file_uploads.is_empty() {
        info!(
            file_count = file_uploads.len(),
            "Waiting for Slack file processing"
        );
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    // ── Stage 2: Replace ChartML with markers ───────────────────────────
    let rendered_indices: HashSet<usize> = (0..charts_to_render)
        .filter(|i| !failed_indices.contains(i))
        .chain(native_blocks.keys().copied())
        .collect();

    let mut text = replace_chartml_with_markers(
        message,
        &extraction,
        &rendered_indices,
        &failed_indices,
    );

    // ── Stage 3: Extract markdown tables ────────────────────────────────
    let (text_with_markers, tables) = extract_markdown_tables(&text);
    text = text_with_markers;

    let table_block = if let Some(first_table) = tables.first() {
        if first_table.valid {
            info!(
                rows = first_table.num_rows,
                cols = first_table.num_columns,
                "Converting first table to Block Kit"
            );
            markdown_table_to_slack_block(first_table)
        } else {
            None
        }
    } else {
        None
    };

    // ── Stage 4: Markdown → Slack mrkdwn ────────────────────────────────
    let slack_text = markdown_to_slack(&text);
    // Convert horizontal rules to divider markers
    let slack_text = RE_HR
        .replace_all(&slack_text, DIVIDER_MARKER)
        .into_owned();

    // ── Stage 5: Assemble Block Kit blocks ──────────────────────────────
    let mut blocks: Vec<Value> = Vec::new();

    // Optional header (for watch alerts)
    if let Some(header) = header_text {
        let display = match header_emoji {
            Some(emoji) => format!("{emoji} {header}"),
            None => header.to_string(),
        };
        blocks.push(json!({
            "type": "header",
            "text": {"type": "plain_text", "text": display, "emoji": true},
        }));
    }

    // Walk text looking for markers, inserting text chunks and blocks
    let mut remaining = slack_text.as_str();
    let mut table_block = table_block; // take ownership for consuming

    while !remaining.is_empty() {
        if let Some(marker) = find_next_marker(remaining) {
            // Text before the marker
            let before = remaining[..marker.position].trim_end();
            if !before.is_empty() {
                chunk_text_into_sections(before, &mut blocks);
            }

            // Insert the appropriate block for this marker
            match marker.marker_type {
                MarkerType::Table => {
                    if let Some(tb) = table_block.take() {
                        blocks.push(tb);
                        info!("Added table block inline");
                    }
                }
                MarkerType::Chart(idx) => {
                    if let Some(native) = native_blocks.remove(&idx) {
                        for block in native {
                            blocks.push(block);
                        }
                        info!(chart_idx = idx, "Added native chart inline");
                    } else if let Some(file_id) = file_uploads.remove(&idx) {
                        let title = if idx < extraction.specs.len() {
                            get_chart_title(&extraction.specs[idx])
                        } else {
                            "Chart".to_string()
                        };
                        blocks.push(json!({
                            "type": "image",
                            "slack_file": {"id": file_id},
                            "alt_text": title,
                        }));
                        info!(chart_idx = idx, "Added chart image inline");
                    }
                }
                MarkerType::Divider => {
                    blocks.push(json!({"type": "divider"}));
                }
            }

            // Advance past the marker
            let after_pos = marker.position + marker.marker_text.len();
            remaining = remaining[after_pos..].trim_start();
        } else {
            // No more markers — emit remaining text
            chunk_text_into_sections(remaining, &mut blocks);
            remaining = "";
        }
    }

    // Append remaining native blocks not placed inline
    for (_idx, native_block_list) in native_blocks {
        for block in native_block_list {
            blocks.push(block);
        }
    }

    // Append remaining chart images not placed inline
    for (idx, file_id) in &file_uploads {
        let title = if *idx < extraction.specs.len() {
            get_chart_title(&extraction.specs[*idx])
        } else {
            "Chart".to_string()
        };
        blocks.push(json!({
            "type": "image",
            "slack_file": {"id": file_id},
            "alt_text": title,
        }));
    }

    // Note about additional charts beyond limit
    if total_charts > MAX_CHARTS_RENDERED {
        let additional = total_charts - MAX_CHARTS_RENDERED;
        let word = if additional == 1 { "chart" } else { "charts" };
        blocks.push(json!({
            "type": "context",
            "elements": [{"type": "mrkdwn", "text": format!("\u{1F4CA} _{additional} more {word} available in Kyomi_")}],
        }));
    }

    // Footer with link
    if let Some(url) = footer_url {
        blocks.push(json!({
            "type": "context",
            "elements": [{"type": "mrkdwn", "text": format!("<{url}|{footer_text}>")}],
        }));
    }

    // Enforce 50 block limit
    if blocks.len() > MAX_BLOCKS {
        blocks.truncate(MAX_BLOCKS - 1);
        blocks.push(json!({
            "type": "section",
            "text": {"type": "mrkdwn", "text": "_Response truncated. View full response in Kyomi._"},
        }));
    }

    // Fallback text — Python: `slack_text[:200]` (char-based, no ellipsis)
    let fallback = if slack_text.is_empty() {
        "Message from Kyomi".to_string()
    } else {
        truncate_chars(&slack_text, 200)
    };

    (blocks, fallback)
}

// ---------------------------------------------------------------------------
// ChartML extraction and replacement
// ---------------------------------------------------------------------------

/// Extract ChartML specs — delegates to the shared `chartml_utils` module.
fn extract_chartml_specs(message: &str) -> ExtractionResult {
    chartml_utils::extract_chartml_specs(message)
}

/// Replace ChartML fenced blocks with markers or placeholders.
///
/// Uses the block-to-spec mapping from extraction: each block's replacement
/// is the concatenation of its specs' replacements. Blocks with no chart
/// components are removed silently (matching Python).
fn replace_chartml_with_markers(
    message: &str,
    extraction: &ExtractionResult,
    rendered_indices: &HashSet<usize>,
    _failed_indices: &HashSet<usize>,
) -> String {
    if extraction.blocks.is_empty() {
        return message.to_string();
    }

    let mut result = message.to_string();

    // Process blocks in reverse order so byte offsets stay valid
    for block in extraction.blocks.iter().rev() {
        if block.spec_indices.is_empty() {
            // Block had no chart components — remove it silently
            result.replace_range(block.range.clone(), "");
            continue;
        }

        // Build replacement for this block by concatenating each spec's replacement
        let parts: Vec<String> = block
            .spec_indices
            .iter()
            .map(|&spec_idx| {
                let spec = &extraction.specs[spec_idx];
                if rendered_indices.contains(&spec_idx) {
                    format!("\n<<<SLACK_CHART_{spec_idx}>>>\n")
                } else {
                    let title = get_chart_title(spec);
                    format!("_[{title}] (view at Kyomi.ai)_")
                }
            })
            .collect();

        let block_replacement = parts.join("\n");
        result.replace_range(block.range.clone(), &block_replacement);
    }

    // Clean up excessive newlines
    let result = RE_EXCESS_NEWLINES.replace_all(&result, "\n\n");
    result.trim().to_string()
}

// ---------------------------------------------------------------------------
// Markdown table extraction
// ---------------------------------------------------------------------------

/// Extract markdown tables from text, replacing the first valid one with a marker.
fn extract_markdown_tables(text: &str) -> (String, Vec<ParsedTable>) {
    let matches: Vec<_> = RE_MD_TABLE.captures_iter(text).collect();

    if matches.is_empty() {
        return (text.to_string(), Vec::new());
    }

    let mut tables = Vec::new();
    let mut match_ranges = Vec::new();

    for cap in &matches {
        let Some(full_match) = cap.get(0) else { continue };
        match_ranges.push(full_match.start()..full_match.end());

        let header_row = &cap[1];
        let separator_row = &cap[2];
        let data_rows_text = &cap[3];

        // Parse headers
        let headers: Vec<String> = header_row
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim().to_string())
            .collect();

        // Parse alignments
        let alignments: Vec<String> = separator_row
            .trim_matches('|')
            .split('|')
            .map(|cell| {
                let cell = cell.trim();
                if cell.starts_with(':') && cell.ends_with(':') {
                    "center".to_string()
                } else if cell.ends_with(':') {
                    "right".to_string()
                } else {
                    "left".to_string()
                }
            })
            .collect();

        // Parse data rows
        let rows: Vec<Vec<String>> = data_rows_text
            .trim()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                line.trim_matches('|')
                    .split('|')
                    .map(|c| c.trim().to_string())
                    .collect()
            })
            .collect();

        let num_columns = headers.len();
        let num_rows = rows.len();
        let mut valid = true;
        let mut error = None;

        if num_columns > MAX_TABLE_COLUMNS {
            valid = false;
            error = Some(format!("Too many columns ({num_columns} > {MAX_TABLE_COLUMNS})"));
        } else if num_rows > MAX_TABLE_ROWS {
            valid = false;
            error = Some(format!("Too many rows ({num_rows} > {MAX_TABLE_ROWS})"));
        } else if num_columns == 0 {
            valid = false;
            error = Some("Empty table".to_string());
        }

        tables.push(ParsedTable {
            headers,
            rows,
            alignments,
            valid,
            num_rows,
            num_columns,
            error,
        });
    }

    // Replace tables in text with markers/placeholders (reverse order)
    let mut result = text.to_string();
    for (i, range) in match_ranges.iter().enumerate().rev() {
        let replacement = if i == 0 && tables[0].valid {
            format!("\n{TABLE_MARKER}\n")
        } else {
            get_table_placeholder(&tables[i])
        };
        result.replace_range(range.clone(), &replacement);
    }

    // Clean up excessive newlines
    let result = RE_EXCESS_NEWLINES.replace_all(&result, "\n\n");
    (result.trim().to_string(), tables)
}

/// Generate placeholder text for a table that won't be rendered as Block Kit.
fn get_table_placeholder(table: &ParsedTable) -> String {
    let col_preview = if table.headers.len() > 3 {
        format!("{}...", table.headers[..3].join(", "))
    } else {
        table.headers.join(", ")
    };

    let title = if table.headers.is_empty() {
        format!("Table ({} rows)", table.num_rows)
    } else {
        format!("Table: {} ({} rows)", col_preview, table.num_rows)
    };

    let error_suffix = if !table.valid {
        table
            .error
            .as_ref()
            .map(|e| format!(" - {e}"))
            .unwrap_or_default()
    } else {
        String::new()
    };

    format!("_[{title}{error_suffix}] (view at Kyomi.ai)_\n")
}

/// Build a Slack Block Kit table cell.
///
/// Slack table cells only accept `raw_text` or `rich_text` types.
/// Uses `raw_text` for plain text, `rich_text` when bold styling is needed.
fn build_table_cell(text: &str, bold: bool) -> Value {
    let text = truncate_cell_text(text);

    if bold {
        // Strip markdown bold markers — we apply bold via style
        let clean = RE_MD_BOLD.replace_all(&text, "$1");
        return json!({
            "type": "rich_text",
            "elements": [{
                "type": "rich_text_section",
                "elements": [{
                    "type": "text",
                    "text": clean.as_ref(),
                    "style": {"bold": true},
                }],
            }],
        });
    }

    // Check for markdown bold markers in data cells
    if RE_MD_BOLD.is_match(&text) {
        let mut elements: Vec<Value> = Vec::new();
        let mut pos = 0;
        for cap in RE_MD_BOLD.captures_iter(&text) {
            let full_match = cap.get(0).expect("group 0 always exists in a regex match");
            // Plain text before bold segment
            if full_match.start() > pos {
                let plain = &text[pos..full_match.start()];
                if !plain.is_empty() {
                    elements.push(json!({"type": "text", "text": plain}));
                }
            }
            // Bold segment
            elements.push(json!({
                "type": "text",
                "text": &cap[1],
                "style": {"bold": true},
            }));
            pos = full_match.end();
        }
        // Remaining plain text
        if pos < text.len() {
            let remaining = &text[pos..];
            if !remaining.is_empty() {
                elements.push(json!({"type": "text", "text": remaining}));
            }
        }
        return json!({
            "type": "rich_text",
            "elements": [{
                "type": "rich_text_section",
                "elements": elements,
            }],
        });
    }

    // Plain text
    json!({"type": "raw_text", "text": text})
}

/// Convert a parsed markdown table to a Slack Block Kit table block.
fn markdown_table_to_slack_block(table: &ParsedTable) -> Option<Value> {
    if !table.valid {
        return None;
    }

    // Column settings
    let column_settings: Vec<Value> = table
        .alignments
        .iter()
        .map(|align| {
            json!({
                "align": align,
                "is_wrapped": true,
            })
        })
        .collect();

    // Header row (bold rich_text cells)
    let header_cells: Vec<Value> = table
        .headers
        .iter()
        .map(|h| build_table_cell(h, true))
        .collect();

    let mut block_rows = vec![Value::Array(header_cells)];

    // Data rows
    for row in &table.rows {
        let mut row_cells: Vec<Value> = Vec::new();
        for (i, cell) in row.iter().enumerate() {
            if i >= table.headers.len() {
                break;
            }
            row_cells.push(build_table_cell(cell, false));
        }
        // Pad with empty cells if row is short
        while row_cells.len() < table.headers.len() {
            row_cells.push(json!({"type": "raw_text", "text": ""}));
        }
        block_rows.push(Value::Array(row_cells));
    }

    Some(json!({
        "type": "table",
        "column_settings": column_settings,
        "rows": block_rows,
    }))
}

/// Truncate cell text if it exceeds the Slack mrkdwn limit.
fn truncate_cell_text(text: &str) -> String {
    if text.chars().count() <= MAX_CELL_LENGTH {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(MAX_CELL_LENGTH - 1).collect();
        format!("{truncated}…")
    }
}

/// Truncate a string to at most `max_chars` characters (char-safe, no ellipsis).
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        text.chars().take(max_chars).collect()
    }
}

/// Find the largest byte index ≤ `byte_pos` that is a valid UTF-8 char boundary.
fn floor_char_boundary(s: &str, byte_pos: usize) -> usize {
    if byte_pos >= s.len() {
        return s.len();
    }
    let mut pos = byte_pos;
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

// ---------------------------------------------------------------------------
// Native metric/table rendering
// ---------------------------------------------------------------------------

/// Get the `visualize.type` from a ChartML spec — delegates to shared module.
fn get_visualize_type(spec: &Value) -> Option<String> {
    chartml_utils::get_visualize_type(spec)
}

/// Get the title from a ChartML spec — delegates to shared module.
fn get_chart_title(spec: &Value) -> String {
    chartml_utils::get_chart_title(spec)
}

/// Render a ChartML metric spec as Slack Block Kit blocks.
fn render_metric_slack_blocks(spec: &Value) -> Vec<Value> {
    let visualize = spec.get("visualize").cloned().unwrap_or(json!({}));
    let data = spec.get("data").cloned().unwrap_or(json!({}));
    let rows = data
        .get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    let title = get_chart_title(spec);
    let value_field = visualize.get("value").and_then(|v| v.as_str());
    let fmt = visualize.get("format").and_then(|v| v.as_str());
    let label = visualize
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or(&title);
    let compare_field = visualize.get("compareWith").and_then(|v| v.as_str());

    if rows.is_empty() {
        return vec![json!({
            "type": "section",
            "text": {"type": "mrkdwn", "text": format!("*{label}*\n_No data_")},
            "expand": true,
        })];
    }

    let row = &rows[0];

    // Extract and format value
    let (formatted_value, raw_value) = if let Some(field) = value_field {
        match row.get(field) {
            Some(raw) if !raw.is_null() => {
                (d3_format::format_d3(Some(raw), fmt), raw.as_f64())
            }
            _ => ("N/A".to_string(), None),
        }
    } else {
        ("N/A".to_string(), None)
    };

    // Build trend line
    let trend_text = if let (Some(cmp_field), Some(current_val)) = (compare_field, raw_value) {
        if let Some(compare_val) = row.get(cmp_field).and_then(|v| v.as_f64()) {
            if compare_val != 0.0 {
                let pct_change = ((current_val - compare_val) / compare_val.abs()) * 100.0;
                if pct_change > 0.0 {
                    format!("\n\u{25B2} {:.1}% vs previous", pct_change.abs())
                } else if pct_change < 0.0 {
                    format!("\n\u{25BC} {:.1}% vs previous", pct_change.abs())
                } else {
                    "\n\u{2014} No change vs previous".to_string()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let text = format!("*{label}*\n*{formatted_value}*{trend_text}");
    vec![json!({
        "type": "section",
        "text": {"type": "mrkdwn", "text": text},
        "expand": true,
    })]
}

/// Render a ChartML table spec as Slack Block Kit blocks.
fn render_table_slack_blocks(spec: &Value) -> Vec<Value> {
    let visualize = spec.get("visualize").cloned().unwrap_or(json!({}));
    let data = spec.get("data").cloned().unwrap_or(json!({}));
    let rows = data
        .get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let title = get_chart_title(spec);

    if rows.is_empty() {
        return vec![json!({
            "type": "section",
            "text": {"type": "mrkdwn", "text": format!("*{title}*\n_No data available_")},
            "expand": true,
        })];
    }

    // Determine columns from visualize.columns or first row keys
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
        return vec![json!({
            "type": "section",
            "text": {"type": "mrkdwn", "text": format!("*{title}*\n_Table data unavailable_")},
            "expand": true,
        })];
    }

    let total_rows = rows.len();
    let display_rows = &rows[..total_rows.min(MAX_TABLE_ROWS)];

    // Build column settings
    let column_settings: Vec<Value> = header_labels
        .iter()
        .map(|_| json!({"align": "left", "is_wrapped": true}))
        .collect();

    // Header row (bold rich_text cells)
    let header_cells: Vec<Value> = header_labels
        .iter()
        .map(|h| build_table_cell(h, true))
        .collect();
    let mut block_rows = vec![Value::Array(header_cells)];

    // Data rows
    for row in display_rows {
        let row_cells: Vec<Value> = columns
            .iter()
            .map(|col| {
                let val = row
                    .get(col.as_str())
                    .map(|v| {
                        if let Some(s) = v.as_str() {
                            s.to_string()
                        } else if v.is_null() {
                            "None".to_string()
                        } else {
                            v.to_string()
                        }
                    })
                    .unwrap_or_default();
                build_table_cell(&val, false)
            })
            .collect();
        block_rows.push(Value::Array(row_cells));
    }

    let mut blocks = vec![json!({
        "type": "table",
        "column_settings": column_settings,
        "rows": block_rows,
    })];

    if total_rows > MAX_TABLE_ROWS {
        blocks.push(json!({
            "type": "context",
            "elements": [{"type": "mrkdwn", "text": format!("Showing {MAX_TABLE_ROWS} of {total_rows} rows — view full table in Kyomi")}],
        }));
    }

    blocks
}

/// Extract column names from the first data row.
fn columns_from_first_row(rows: &[Value]) -> (Vec<String>, Vec<String>) {
    if let Some(first) = rows.first()
        && let Some(obj) = first.as_object() {
            let cols: Vec<String> = obj.keys().cloned().collect();
            let labels = cols.clone();
            return (cols, labels);
        }
    (Vec::new(), Vec::new())
}

// ---------------------------------------------------------------------------
// Text chunking and marker finding
// ---------------------------------------------------------------------------

/// Find the next TABLE / CHART / DIVIDER marker in text.
fn find_next_marker(text: &str) -> Option<MarkerInfo> {
    let mut earliest: Option<MarkerInfo> = None;

    // Table marker
    if let Some(pos) = text.find(TABLE_MARKER) {
        earliest = Some(MarkerInfo {
            position: pos,
            marker_type: MarkerType::Table,
            marker_text: TABLE_MARKER.to_string(),
        });
    }

    // Divider marker
    if let Some(pos) = text.find(DIVIDER_MARKER)
        && earliest.as_ref().is_none_or(|e| pos < e.position) {
            earliest = Some(MarkerInfo {
                position: pos,
                marker_type: MarkerType::Divider,
                marker_text: DIVIDER_MARKER.to_string(),
            });
        }

    // Chart markers
    if let Some(m) = RE_CHART_MARKER.find(text)
        && earliest.as_ref().is_none_or(|e| m.start() < e.position) {
            let idx: usize = RE_CHART_MARKER
                .captures(text)
                .and_then(|c| c.get(1))
                .and_then(|g| g.as_str().parse().ok())
                .unwrap_or(0);
            earliest = Some(MarkerInfo {
                position: m.start(),
                marker_type: MarkerType::Chart(idx),
                marker_text: m.as_str().to_string(),
            });
        }

    earliest
}

/// Split text into Slack section blocks, each ≤ 2900 chars.
fn chunk_text_into_sections(text: &str, blocks: &mut Vec<Value>) {
    let mut remaining = text;

    while remaining.len() > MAX_SECTION_LENGTH {
        // Find a char-boundary-safe prefix of ~MAX_SECTION_LENGTH bytes
        let safe_end = floor_char_boundary(remaining, MAX_SECTION_LENGTH);

        // Find break point: prefer newline, then space
        let break_point = remaining[..safe_end]
            .rfind('\n')
            .or_else(|| remaining[..safe_end].rfind(' '))
            .unwrap_or(safe_end);

        let chunk = remaining[..break_point].trim();
        if !chunk.is_empty() {
            blocks.push(json!({
                "type": "section",
                "text": {"type": "mrkdwn", "text": chunk},
                "expand": true,
            }));
        }
        remaining = remaining[break_point..].trim_start();
    }

    let chunk = remaining.trim();
    if !chunk.is_empty() {
        blocks.push(json!({
            "type": "section",
            "text": {"type": "mrkdwn", "text": chunk},
            "expand": true,
        }));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- split_message_for_multiple_tables --

    #[test]
    fn split_no_tables() {
        let msg = "Just some text without tables.";
        let chunks = split_message_for_multiple_tables(msg);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], msg);
    }

    #[test]
    fn split_one_table() {
        let msg = "Header\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nFooter";
        let chunks = split_message_for_multiple_tables(msg);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn split_two_tables() {
        let msg = "## Section 1\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n## Section 2\n\n| C | D |\n|---|---|\n| 3 | 4 |\n";
        let chunks = split_message_for_multiple_tables(msg);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("Section 1"));
        assert!(chunks[1].contains("Section 2"));
    }

    // -- replace_chartml_with_markers --

    #[test]
    fn replace_rendered_chart() {
        let msg = "Before\n```chartml\ntype: chart\ntitle: Revenue\n```\nAfter";
        let extraction = extract_chartml_specs(msg);
        let rendered: HashSet<usize> = [0].into();
        let failed: HashSet<usize> = HashSet::new();
        let result = replace_chartml_with_markers(msg, &extraction, &rendered, &failed);
        assert!(result.contains("<<<SLACK_CHART_0>>>"));
        assert!(!result.contains("chartml"));
    }

    #[test]
    fn replace_failed_chart() {
        let msg = "Before\n```chartml\ntype: chart\ntitle: Revenue\n```\nAfter";
        let extraction = extract_chartml_specs(msg);
        let rendered: HashSet<usize> = HashSet::new();
        let failed: HashSet<usize> = [0].into();
        let result = replace_chartml_with_markers(msg, &extraction, &rendered, &failed);
        assert!(result.contains("_[Revenue] (view at Kyomi.ai)_"));
    }

    #[test]
    fn replace_non_chart_block_removed() {
        let msg = "Before\n```chartml\ntype: source\nname: my_source\n```\nAfter";
        let extraction = extract_chartml_specs(msg);
        let rendered: HashSet<usize> = HashSet::new();
        let failed: HashSet<usize> = HashSet::new();
        let result = replace_chartml_with_markers(msg, &extraction, &rendered, &failed);
        assert!(!result.contains("chartml"));
        assert!(!result.contains("source"));
        assert!(result.contains("Before"));
        assert!(result.contains("After"));
    }

    #[test]
    fn replace_multi_spec_block() {
        let msg = "Text\n```chartml\n- type: chart\n  title: A\n- type: chart\n  title: B\n```\nEnd";
        let extraction = extract_chartml_specs(msg);
        assert_eq!(extraction.specs.len(), 2);
        let rendered: HashSet<usize> = [0].into();
        let failed: HashSet<usize> = [1].into();
        let result = replace_chartml_with_markers(msg, &extraction, &rendered, &failed);
        assert!(result.contains("<<<SLACK_CHART_0>>>"));
        assert!(result.contains("_[B] (view at Kyomi.ai)_"));
    }

    // -- extract_markdown_tables --

    #[test]
    fn extract_table_basic() {
        let text = "Before\n\n| Name | Value |\n|------|-------|\n| A | 1 |\n| B | 2 |\n\nAfter";
        let (result, tables) = extract_markdown_tables(text);
        assert_eq!(tables.len(), 1);
        assert!(tables[0].valid);
        assert_eq!(tables[0].headers, vec!["Name", "Value"]);
        assert_eq!(tables[0].num_rows, 2);
        assert!(result.contains(TABLE_MARKER));
        assert!(!result.contains("|"));
    }

    #[test]
    fn extract_no_tables() {
        let text = "No tables here";
        let (result, tables) = extract_markdown_tables(text);
        assert!(tables.is_empty());
        assert_eq!(result, text);
    }

    // -- markdown_table_to_slack_block --

    #[test]
    fn table_to_block_kit() {
        let table = ParsedTable {
            headers: vec!["Name".into(), "Value".into()],
            rows: vec![vec!["A".into(), "1".into()]],
            alignments: vec!["left".into(), "right".into()],
            valid: true,
            num_rows: 1,
            num_columns: 2,
            error: None,
        };
        let block = markdown_table_to_slack_block(&table).unwrap();
        assert_eq!(block["type"], "table");
        assert_eq!(block["column_settings"][0]["align"], "left");
        assert_eq!(block["column_settings"][1]["align"], "right");
        let rows = block["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2); // header + 1 data row
    }

    #[test]
    fn table_to_block_kit_invalid() {
        let table = ParsedTable {
            headers: Vec::new(),
            rows: Vec::new(),
            alignments: Vec::new(),
            valid: false,
            num_rows: 0,
            num_columns: 0,
            error: Some("Empty table".into()),
        };
        assert!(markdown_table_to_slack_block(&table).is_none());
    }

    // -- chunk_text_into_sections --

    #[test]
    fn chunk_short_text() {
        let mut blocks = Vec::new();
        chunk_text_into_sections("Short text", &mut blocks);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["text"]["text"], "Short text");
    }

    #[test]
    fn chunk_long_text() {
        let long_text = "A ".repeat(2000); // ~4000 chars
        let mut blocks = Vec::new();
        chunk_text_into_sections(&long_text, &mut blocks);
        assert!(blocks.len() >= 2);
        for block in &blocks {
            let text = block["text"]["text"].as_str().unwrap();
            assert!(text.len() <= MAX_SECTION_LENGTH);
        }
    }

    #[test]
    fn chunk_empty_text() {
        let mut blocks = Vec::new();
        chunk_text_into_sections("", &mut blocks);
        assert!(blocks.is_empty());
    }

    // -- find_next_marker --

    #[test]
    fn find_table_marker() {
        let text = format!("Some text {TABLE_MARKER} more text");
        let marker = find_next_marker(&text).unwrap();
        assert!(matches!(marker.marker_type, MarkerType::Table));
        assert_eq!(marker.marker_text, TABLE_MARKER);
    }

    #[test]
    fn find_chart_marker() {
        let text = "Some text <<<SLACK_CHART_0>>> more";
        let marker = find_next_marker(text).unwrap();
        assert!(matches!(marker.marker_type, MarkerType::Chart(0)));
    }

    #[test]
    fn find_divider_marker() {
        let text = format!("Some text {DIVIDER_MARKER} more");
        let marker = find_next_marker(&text).unwrap();
        assert!(matches!(marker.marker_type, MarkerType::Divider));
    }

    #[test]
    fn find_earliest_marker() {
        let text = format!("A {DIVIDER_MARKER} B {TABLE_MARKER}");
        let marker = find_next_marker(&text).unwrap();
        assert!(matches!(marker.marker_type, MarkerType::Divider));
    }

    #[test]
    fn find_no_marker() {
        assert!(find_next_marker("No markers here").is_none());
    }

    // -- Native metric rendering --

    #[test]
    fn metric_basic() {
        let spec = json!({
            "title": "Revenue",
            "visualize": {"type": "metric", "value": "revenue", "format": "$,.0f"},
            "data": {"rows": [{"revenue": 42000}]},
        });
        let blocks = render_metric_slack_blocks(&spec);
        assert_eq!(blocks.len(), 1);
        let text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(text.contains("*Revenue*"));
        assert!(text.contains("$42,000"));
    }

    #[test]
    fn metric_with_trend() {
        let spec = json!({
            "title": "Users",
            "visualize": {"type": "metric", "value": "current", "compareWith": "previous"},
            "data": {"rows": [{"current": 120, "previous": 100}]},
        });
        let blocks = render_metric_slack_blocks(&spec);
        let text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(text.contains("\u{25B2}")); // ▲
        assert!(text.contains("20.0%"));
    }

    #[test]
    fn metric_negative_trend() {
        let spec = json!({
            "title": "Revenue",
            "visualize": {"type": "metric", "value": "current", "compareWith": "previous"},
            "data": {"rows": [{"current": 80, "previous": 100}]},
        });
        let blocks = render_metric_slack_blocks(&spec);
        let text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(text.contains("\u{25BC}")); // ▼
        assert!(text.contains("20.0%"));
    }

    #[test]
    fn metric_no_data() {
        let spec = json!({
            "title": "Empty",
            "visualize": {"type": "metric", "value": "val"},
            "data": {"rows": []},
        });
        let blocks = render_metric_slack_blocks(&spec);
        let text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(text.contains("No data"));
    }

    // -- Native table rendering --

    #[test]
    fn table_rendering_basic() {
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
        let blocks = render_table_slack_blocks(&spec);
        assert!(!blocks.is_empty());
        assert_eq!(blocks[0]["type"], "table");
        let rows = blocks[0]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 3); // header + 2 data rows
    }

    #[test]
    fn table_rendering_no_data() {
        let spec = json!({
            "title": "Empty Table",
            "visualize": {"type": "table"},
            "data": {"rows": []},
        });
        let blocks = render_table_slack_blocks(&spec);
        let text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(text.contains("No data available"));
    }

    #[test]
    fn table_rendering_truncation() {
        let mut rows = Vec::new();
        for i in 0..150 {
            rows.push(json!({"id": i}));
        }
        let spec = json!({
            "title": "Big Table",
            "visualize": {"type": "table"},
            "data": {"rows": rows},
        });
        let blocks = render_table_slack_blocks(&spec);
        assert!(blocks.len() >= 2); // table + context
        let last = blocks.last().unwrap();
        assert_eq!(last["type"], "context");
        let text = last["elements"][0]["text"].as_str().unwrap();
        assert!(text.contains("Showing 100 of 150"));
    }

    // -- truncate_cell_text --

    #[test]
    fn truncate_short_cell() {
        assert_eq!(truncate_cell_text("short"), "short");
    }

    #[test]
    fn truncate_long_cell() {
        let long = "X".repeat(3000);
        let result = truncate_cell_text(&long);
        assert!(result.len() < 3000);
        assert!(result.ends_with('…'));
    }
}
