// SPDX-License-Identifier: AGPL-3.0-or-later

//! Schema-aware completion provider for ChartML YAML editing.
//!
//! Parses YAML context around the cursor position to determine the schema path,
//! then returns valid keys or values from the ChartML schema for that path.

use std::sync::Arc;

use kode_leptos::{
    CompletionContext, CompletionItem, CompletionKind, CompletionProviderConfig,
    FenceTracker,
};

/// Build a [`CompletionProviderConfig`] for ChartML YAML (pure YAML editors).
pub fn chartml_completion_provider() -> CompletionProviderConfig {
    CompletionProviderConfig {
        provider: Arc::new(|ctx| Box::pin(async move { provide_completions(ctx) })),
        trigger_characters: vec![':', ' '],
        activate_on_typing: true,
        render_item: None,
    }
}

/// Build a [`CompletionProviderConfig`] for ChartML inside Markdown documents.
///
/// Uses [`FenceTracker`] to detect whether the cursor is inside a `chartml` or
/// `yaml` fenced code block. If so, extracts the block content and delegates to
/// the same schema-aware completion logic. Returns nothing when the cursor is
/// outside a YAML fence.
pub fn chartml_markdown_completion_provider() -> CompletionProviderConfig {
    CompletionProviderConfig {
        provider: Arc::new(|ctx| {
            Box::pin(async move { provide_markdown_completions(ctx) })
        }),
        trigger_characters: vec![':', ' '],
        activate_on_typing: true,
        render_item: None,
    }
}

/// Completion provider for Markdown mode: only activates inside chartml/yaml fences.
fn provide_markdown_completions(ctx: CompletionContext) -> Vec<CompletionItem> {
    let lines: Vec<&str> = ctx.text.lines().collect();

    // Use FenceTracker to find what language the cursor line is in,
    // and locate the fence boundaries.
    let mut tracker = FenceTracker::new();
    let mut fence_start: Option<usize> = None;
    let mut cursor_in_yaml = false;

    for (i, &line) in lines.iter().enumerate() {
        let lang = tracker.process_line(line);

        if i == ctx.cursor.line {
            if lang.name() == "yaml" {
                cursor_in_yaml = true;
            }
            break;
        }

        // Track the most recent fence opening — when we see a Markdown line
        // followed by YAML, the fence content starts on the next line.
        match lang.name() {
            "markdown" => {
                // If the next line is YAML content, this was the fence opener.
                // We'll set fence_start when we first see YAML.
                fence_start = None;
            }
            "yaml" if fence_start.is_none() => {
                fence_start = Some(i);
            }
            _ => {}
        }
    }

    if !cursor_in_yaml {
        return vec![];
    }

    let block_start = fence_start.unwrap_or(ctx.cursor.line);

    // Find the fence end (or use end of document)
    let block_end = lines[(block_start + 1)..]
        .iter()
        .enumerate()
        .find(|&(offset, line)| {
            let abs_line = block_start + 1 + offset;
            let trimmed = line.trim_start();
            abs_line > ctx.cursor.line
                && (trimmed.starts_with("```") || trimmed.starts_with("~~~"))
        })
        .map_or(lines.len(), |(offset, _)| block_start + 1 + offset);

    // Extract the YAML block content and build a virtual context
    let yaml_lines: Vec<&str> = lines[block_start..block_end].to_vec();
    let yaml_text = yaml_lines.join("\n");
    let adjusted_line = ctx.cursor.line - block_start;

    let virtual_ctx = CompletionContext {
        text: yaml_text,
        cursor: kode_leptos::Position::new(adjusted_line, ctx.cursor.col),
        version: ctx.version,
        trigger: ctx.trigger,
    };

    provide_completions(virtual_ctx)
}

// ── Schema knowledge ────────────────────────────────────────────────────

/// A schema node describes what's valid at a given YAML path.
enum SchemaNode {
    /// Object with known property names (and optional descriptions).
    Object(&'static [(&'static str, &'static str)]),
    /// Enum of allowed string values.
    Enum(&'static [&'static str]),
}

/// Look up what the schema says is valid at `path`.
///
/// `path` is a list of YAML keys from root to cursor, e.g. `["visualize", "type"]`.
fn schema_lookup(path: &[&str]) -> Option<SchemaNode> {
    match path {
        // ── Root level (document type detection) ──
        // When the document already has a `type` key we know the component kind.
        // An empty path means we're at root level — offer top-level keys for a Chart
        // (the most common component type being edited).
        [] => Some(SchemaNode::Object(CHART_KEYS)),

        // ── Component type value ──
        ["type"] => Some(SchemaNode::Enum(&[
            "chart", "source", "params", "style", "config",
        ])),

        // ── Chart keys ──
        ["version"] => Some(SchemaNode::Enum(&["1"])),

        // ── Data section ──
        ["data"] => Some(SchemaNode::Object(DATA_KEYS)),
        ["data", "provider"] => Some(SchemaNode::Enum(PROVIDERS)),
        ["data", "cache"] => Some(SchemaNode::Object(CACHE_KEYS)),
        ["data", "cache", "autoRefresh"] => Some(SchemaNode::Enum(BOOLEANS)),

        // ── Visualize section ──
        ["visualize"] => Some(SchemaNode::Object(VISUALIZE_KEYS)),
        ["visualize", "type"] => Some(SchemaNode::Enum(CHART_TYPES)),
        ["visualize", "mode"] => Some(SchemaNode::Enum(CHART_MODES)),
        ["visualize", "orientation"] => Some(SchemaNode::Enum(ORIENTATIONS)),

        // Visualize > rows (object form)
        ["visualize", "rows"] => Some(SchemaNode::Object(ROWS_OBJECT_KEYS)),
        ["visualize", "rows", "mark"] => Some(SchemaNode::Enum(MARK_TYPES)),
        ["visualize", "rows", "axis"] => Some(SchemaNode::Enum(Y_AXES)),
        ["visualize", "rows", "lineStyle"] => Some(SchemaNode::Enum(LINE_STYLES)),
        ["visualize", "rows", "dataLabels"] => Some(SchemaNode::Object(DATA_LABELS_KEYS)),
        ["visualize", "rows", "dataLabels", "show"] => Some(SchemaNode::Enum(BOOLEANS)),
        ["visualize", "rows", "dataLabels", "position"] => {
            Some(SchemaNode::Enum(LABEL_POSITIONS))
        }

        // Visualize > columns (object form)
        ["visualize", "columns"] => Some(SchemaNode::Object(COLUMNS_OBJECT_KEYS)),
        ["visualize", "columns", "align"] => Some(SchemaNode::Enum(ALIGNMENTS)),

        // Visualize > marks
        ["visualize", "marks"] => Some(SchemaNode::Object(MARKS_KEYS)),
        ["visualize", "marks", "color"] => Some(SchemaNode::Object(FIELD_LABEL_KEYS)),
        ["visualize", "marks", "size"] => Some(SchemaNode::Object(FIELD_LABEL_KEYS)),
        ["visualize", "marks", "shape"] => Some(SchemaNode::Object(FIELD_LABEL_KEYS)),
        ["visualize", "marks", "text"] => Some(SchemaNode::Object(FIELD_LABEL_FORMAT_KEYS)),

        // Visualize > axes
        ["visualize", "axes"] => Some(SchemaNode::Object(AXES_KEYS)),
        ["visualize", "axes", "columns" | "rows" | "left" | "right" | "x"] => {
            Some(SchemaNode::Object(AXIS_KEYS))
        }

        // Visualize > annotations (array item)
        ["visualize", "annotations"] => Some(SchemaNode::Object(ANNOTATION_KEYS)),
        ["visualize", "annotations", "type"] => {
            Some(SchemaNode::Enum(&["line", "band"]))
        }
        ["visualize", "annotations", "axis"] => {
            Some(SchemaNode::Enum(&["left", "right", "x"]))
        }
        ["visualize", "annotations", "orientation"] => Some(SchemaNode::Enum(ORIENTATIONS)),
        ["visualize", "annotations", "labelPosition"] => {
            Some(SchemaNode::Enum(&["start", "center", "end"]))
        }

        // Visualize > style (inline)
        ["visualize", "style"] => Some(SchemaNode::Object(STYLE_KEYS)),
        ["visualize", "style", "grid"] => Some(SchemaNode::Object(GRID_KEYS)),
        ["visualize", "style", "grid", "x" | "y"] => Some(SchemaNode::Enum(BOOLEANS)),
        ["visualize", "style", "showDots"] => Some(SchemaNode::Enum(BOOLEANS)),
        ["visualize", "style", "legend"] => Some(SchemaNode::Object(LEGEND_KEYS)),
        ["visualize", "style", "legend", "position"] => Some(SchemaNode::Enum(LEGEND_POSITIONS)),
        ["visualize", "style", "legend", "orientation"] => Some(SchemaNode::Enum(ORIENTATIONS)),
        ["visualize", "style", "fonts"] => {
            Some(SchemaNode::Object(&[
                ("title", "Title font settings"),
                ("axis", "Axis font settings"),
                ("dataLabel", "Data label font settings"),
            ]))
        }
        ["visualize", "style", "fonts", "title"] => {
            Some(SchemaNode::Object(FONT_TITLE_KEYS))
        }
        ["visualize", "style", "fonts", "axis"] => {
            Some(SchemaNode::Object(FONT_AXIS_KEYS))
        }
        ["visualize", "style", "fonts", "dataLabel"] => {
            Some(SchemaNode::Object(FONT_DATA_LABEL_KEYS))
        }
        ["visualize", "style", "pageSize"] => {
            Some(SchemaNode::Enum(&["10", "25", "50", "100", "200"]))
        }

        // ── Transform section ──
        ["transform"] => Some(SchemaNode::Object(TRANSFORM_KEYS)),
        ["transform", "aggregate"] => Some(SchemaNode::Object(AGGREGATE_KEYS)),
        ["transform", "aggregate", "dimensions"] => Some(SchemaNode::Object(DIMENSION_OBJ_KEYS)),
        ["transform", "aggregate", "dimensions", "type"] => {
            Some(SchemaNode::Enum(&["string", "number", "date"]))
        }
        ["transform", "aggregate", "measures"] => Some(SchemaNode::Object(MEASURE_KEYS)),
        ["transform", "aggregate", "measures", "aggregation"] => {
            Some(SchemaNode::Enum(AGGREGATIONS))
        }
        ["transform", "aggregate", "filters"] => Some(SchemaNode::Object(FILTER_KEYS)),
        ["transform", "aggregate", "filters", "combinator"] => {
            Some(SchemaNode::Enum(&["and", "or"]))
        }
        ["transform", "aggregate", "filters", "rules"] => Some(SchemaNode::Object(FILTER_RULE_KEYS)),
        ["transform", "aggregate", "filters", "rules", "operator"] => {
            Some(SchemaNode::Enum(FILTER_OPERATORS))
        }
        ["transform", "aggregate", "sort"] => {
            Some(SchemaNode::Object(&[
                ("field", "Field name to sort by"),
                ("direction", "Sort direction"),
            ]))
        }
        ["transform", "aggregate", "sort", "direction"] => {
            Some(SchemaNode::Enum(&["asc", "desc"]))
        }
        ["transform", "forecast"] => Some(SchemaNode::Object(FORECAST_KEYS)),
        ["transform", "forecast", "model"] => {
            Some(SchemaNode::Enum(&["auto", "ets", "linear", "exponential", "logistic"]))
        }

        // ── Layout (appears on chart and params) ──
        ["layout"] => Some(SchemaNode::Object(LAYOUT_KEYS)),

        // ── Params section ──
        ["params"] => Some(SchemaNode::Object(PARAM_KEYS)),
        ["params", "type"] => Some(SchemaNode::Enum(PARAM_TYPES)),

        // ── Style component (top-level) ──
        ["style"] | ["style", ..] if path.len() == 1 => {
            // When `style` is a string reference this doesn't apply,
            // but when it's an inline object at chart level:
            Some(SchemaNode::Object(STYLE_KEYS))
        }

        // ── Source component ──
        ["provider"] => Some(SchemaNode::Enum(PROVIDERS)),
        ["cache"] => Some(SchemaNode::Object(CACHE_KEYS)),
        ["cache", "autoRefresh"] => Some(SchemaNode::Enum(BOOLEANS)),

        _ => None,
    }
}

// ── Static schema data ──────────────────────────────────────────────────

const BOOLEANS: &[&str] = &["true", "false"];

const CHART_KEYS: &[(&str, &str)] = &[
    ("type", "Component type (chart, source, params, style, config)"),
    ("version", "ChartML version (1)"),
    ("title", "Chart title"),
    ("data", "Data source"),
    ("visualize", "Visualization specification"),
    ("transform", "Data transformation pipeline"),
    ("params", "Chart-level parameters"),
    ("layout", "Layout options (colSpan)"),
    ("style", "Reference to a named style component"),
];

const DATA_KEYS: &[(&str, &str)] = &[
    ("datasource", "Datasource slug"),
    ("provider", "Provider type (bigquery, postgres, ...)"),
    ("query", "SQL query string"),
    ("cache", "Cache configuration"),
    ("rows", "Inline data rows (with provider: inline)"),
    ("url", "HTTP endpoint URL (with provider: http)"),
];

const PROVIDERS: &[&str] = &[
    "bigquery", "clickhouse", "postgres", "mysql", "snowflake",
    "databricks", "redshift", "sqlserver", "synapse", "duckdb",
    "inline", "http",
];

const CACHE_KEYS: &[(&str, &str)] = &[
    ("ttl", "Time to live (e.g., '30s', '5m', '6h', '1d')"),
    ("autoRefresh", "Enable automatic refresh based on TTL"),
];

const CHART_TYPES: &[&str] = &[
    "bar", "line", "area", "pie", "doughnut", "scatter", "table", "metric",
];

const CHART_MODES: &[&str] = &["grouped", "stacked", "normalized"];

const ORIENTATIONS: &[&str] = &["vertical", "horizontal"];

const VISUALIZE_KEYS: &[(&str, &str)] = &[
    ("type", "Chart type (bar, line, area, pie, ...)"),
    ("mode", "Chart mode (grouped, stacked, normalized)"),
    ("orientation", "Bar chart orientation"),
    ("rows", "Y-axis field(s)"),
    ("columns", "X-axis field(s) or table columns"),
    ("marks", "Visual encoding channels (color, size, shape, text)"),
    ("axes", "Axis configuration"),
    ("annotations", "Reference lines, bands, and markers"),
    ("style", "Inline style overrides"),
    ("value", "Value field (metric charts)"),
    ("label", "Label text (metric charts)"),
    ("format", "Number format (metric charts)"),
    ("compareWith", "Comparison field (metric charts)"),
    ("invertTrend", "Invert trend colors (metric charts)"),
];

const ROWS_OBJECT_KEYS: &[(&str, &str)] = &[
    ("field", "Field name from data"),
    ("label", "Display label"),
    ("mark", "Visual mark type override"),
    ("color", "Color override (hex or CSS)"),
    ("axis", "Which Y-axis (left or right)"),
    ("format", "Number/date format string"),
    ("lineStyle", "Line dash pattern"),
    ("dataLabels", "Data label configuration"),
];

const MARK_TYPES: &[&str] = &["bar", "line", "area", "dot", "range"];

const Y_AXES: &[&str] = &["left", "right"];

const LINE_STYLES: &[&str] = &["solid", "dashed", "dotted"];

const DATA_LABELS_KEYS: &[(&str, &str)] = &[
    ("show", "Whether to show data labels"),
    ("position", "Label position (top, center, bottom)"),
    ("format", "Number format for labels"),
    ("color", "Label text color"),
    ("fontSize", "Font size in pixels"),
];

const LABEL_POSITIONS: &[&str] = &["top", "center", "bottom"];

const COLUMNS_OBJECT_KEYS: &[(&str, &str)] = &[
    ("field", "Field name from data"),
    ("label", "Display label"),
    ("width", "Column width in pixels or 'auto'"),
    ("format", "Number/date format string"),
    ("align", "Text alignment (left, center, right)"),
];

const ALIGNMENTS: &[&str] = &["left", "center", "right"];

const MARKS_KEYS: &[(&str, &str)] = &[
    ("color", "Color grouping field"),
    ("size", "Size encoding field"),
    ("shape", "Shape encoding field"),
    ("text", "Text encoding field"),
];

const FIELD_LABEL_KEYS: &[(&str, &str)] = &[
    ("field", "Field name"),
    ("label", "Display label"),
];

const FIELD_LABEL_FORMAT_KEYS: &[(&str, &str)] = &[
    ("field", "Field name"),
    ("format", "Number/date format string"),
];

const AXES_KEYS: &[(&str, &str)] = &[
    ("columns", "Category axis config"),
    ("rows", "Measure axis config"),
    ("left", "Left/Y-axis config"),
    ("right", "Right/secondary Y-axis config"),
    ("x", "Bottom/X-axis config"),
];

const AXIS_KEYS: &[(&str, &str)] = &[
    ("label", "Axis label text"),
    ("format", "Number/date format string"),
    ("min", "Minimum axis value"),
    ("max", "Maximum axis value"),
];

const ANNOTATION_KEYS: &[(&str, &str)] = &[
    ("type", "Annotation type (line or band)"),
    ("axis", "Which axis to attach to"),
    ("value", "Value for line annotation"),
    ("from", "Start value for band"),
    ("to", "End value for band"),
    ("orientation", "Line/band orientation"),
    ("label", "Annotation label"),
    ("labelPosition", "Label position (start, center, end)"),
    ("color", "Color (hex or CSS)"),
    ("strokeWidth", "Line width"),
    ("strokeColor", "Stroke color (for bands)"),
    ("dashArray", "Dash pattern (e.g., '5,5')"),
    ("opacity", "Opacity (0-1)"),
];

const STYLE_KEYS: &[(&str, &str)] = &[
    ("height", "Chart height in pixels"),
    ("pageSize", "Default rows per page (table charts)"),
    ("colors", "Color palette"),
    ("grid", "Grid line configuration"),
    ("showDots", "Show dots on line charts"),
    ("strokeWidth", "Line thickness"),
    ("fonts", "Font configuration"),
    ("legend", "Legend configuration"),
];

const GRID_KEYS: &[(&str, &str)] = &[
    ("x", "Show vertical grid lines"),
    ("y", "Show horizontal grid lines"),
    ("color", "Grid line color"),
    ("opacity", "Grid line opacity (0-1)"),
    ("dashArray", "Dash pattern for grid lines"),
];

const LEGEND_KEYS: &[(&str, &str)] = &[
    ("position", "Legend position"),
    ("orientation", "Legend orientation"),
];

const LEGEND_POSITIONS: &[&str] = &["top", "right", "bottom", "left", "none"];

const FONT_TITLE_KEYS: &[(&str, &str)] = &[
    ("family", "Font family"),
    ("size", "Font size"),
    ("weight", "Font weight"),
    ("color", "Font color"),
];

const FONT_AXIS_KEYS: &[(&str, &str)] = &[
    ("family", "Font family"),
    ("size", "Font size"),
    ("color", "Font color"),
];

const FONT_DATA_LABEL_KEYS: &[(&str, &str)] = &[
    ("family", "Font family"),
    ("size", "Font size"),
    ("weight", "Font weight"),
];

const TRANSFORM_KEYS: &[(&str, &str)] = &[
    ("sql", "DuckDB SQL for transforming/joining sources"),
    ("aggregate", "Declarative aggregation"),
    ("forecast", "Time series forecasting"),
];

const AGGREGATE_KEYS: &[(&str, &str)] = &[
    ("dimensions", "Group-by columns"),
    ("measures", "Aggregation measures"),
    ("filters", "Filter conditions"),
    ("sort", "Sort order"),
    ("limit", "Maximum rows to return"),
];

const DIMENSION_OBJ_KEYS: &[(&str, &str)] = &[
    ("column", "Column expression or name"),
    ("name", "Output field name"),
    ("type", "Data type for casting"),
];

const MEASURE_KEYS: &[(&str, &str)] = &[
    ("column", "Column to aggregate"),
    ("aggregation", "Aggregation function"),
    ("expression", "SQL expression for calculated measure"),
    ("name", "Output column name"),
];

const AGGREGATIONS: &[&str] = &[
    "sum", "count", "avg", "min", "max", "countDistinct",
    "median", "stddev", "variance",
    "percentile25", "percentile50", "percentile75",
    "percentile90", "percentile95", "percentile99",
];

const FILTER_KEYS: &[(&str, &str)] = &[
    ("combinator", "Logical operator (and/or)"),
    ("rules", "Array of filter rules"),
];

const FILTER_RULE_KEYS: &[(&str, &str)] = &[
    ("field", "Field name to filter on"),
    ("operator", "Comparison operator"),
    ("value", "Filter value"),
];

const FILTER_OPERATORS: &[&str] = &[
    "=", "!=", ">", ">=", "<", "<=",
    "in", "notIn", "contains", "startsWith", "endsWith", "between",
    "isNull", "isNotNull",
];

const FORECAST_KEYS: &[(&str, &str)] = &[
    ("timestamp", "Timestamp column"),
    ("value", "Value column"),
    ("horizon", "Number of future periods"),
    ("confidence_level", "Confidence interval (0-1)"),
    ("model", "Forecasting model"),
    ("group_by", "Group-by columns for per-group forecasts"),
];

const LAYOUT_KEYS: &[(&str, &str)] = &[
    ("colSpan", "Grid column span (1-12)"),
];

const PARAM_KEYS: &[(&str, &str)] = &[
    ("id", "Parameter identifier"),
    ("type", "Parameter type"),
    ("label", "Display label"),
    ("default", "Default value"),
    ("options", "Available options (select/multiselect)"),
    ("placeholder", "Placeholder text"),
    ("layout", "Layout options"),
];

const PARAM_TYPES: &[&str] = &["multiselect", "select", "daterange", "number", "text"];

// ── YAML context parsing ────────────────────────────────────────────────

/// Determine whether the cursor is in key position or value position,
/// and build the schema path from root to cursor.
///
/// Returns `(path, in_value_position)`:
/// - `path`: list of YAML keys from root to the current context
/// - `in_value_position`: true if cursor is after `:` on the current line
fn parse_yaml_context(text: &str, line: usize, col: usize) -> (Vec<String>, bool) {
    let lines: Vec<&str> = text.lines().collect();
    if line >= lines.len() {
        return (vec![], false);
    }

    let current_line = lines[line];
    let trimmed = current_line.trim_start();

    // Skip comment lines and YAML document markers
    if trimmed.starts_with('#') || trimmed == "---" || trimmed == "..." {
        return (vec![], false);
    }

    let current_indent = current_line.len() - trimmed.len();

    // Determine if we're in key or value position on the current line
    let (current_key, in_value) = parse_line_key_value(current_line, col);

    // Build the path by walking backwards through parent keys
    let mut path = Vec::new();

    // If we have a key on the current line, add it
    if let Some(key) = &current_key {
        path.push(key.clone());
    }

    // Walk backwards to find parent keys by indentation
    let target_indent = current_indent;
    if target_indent > 0 || current_key.is_some() {
        let search_indent = if current_key.is_some() {
            current_indent
        } else {
            // We're on a blank or value-only line; find the parent at same or lower indent
            current_indent + 1
        };

        let mut i = line;
        let mut looking_for = search_indent;

        while i > 0 && looking_for > 0 {
            i -= 1;
            let prev = lines[i];
            let prev_trimmed = prev.trim_start();

            // Skip blanks, comments, array items without keys
            if prev_trimmed.is_empty()
                || prev_trimmed.starts_with('#')
                || prev_trimmed == "---"
            {
                continue;
            }

            let prev_indent = prev.len() - prev_trimmed.len();

            if prev_indent < looking_for {
                // This is a parent key
                if let Some(key) = extract_key(prev_trimmed) {
                    // Skip array markers: treat "- key: val" as same indent level
                    let effective_key = key.trim_start_matches("- ").to_string();
                    if !effective_key.is_empty() {
                        path.push(effective_key);
                    }
                }
                looking_for = prev_indent;
                if looking_for == 0 {
                    break;
                }
            }
        }
    }

    path.reverse();
    (path, in_value)
}

/// Parse the current line to extract the key name and whether `col` is in value position.
fn parse_line_key_value(line: &str, col: usize) -> (Option<String>, bool) {
    let trimmed = line.trim_start();

    // Handle array item lines: `- key: value` or just `- value`
    let (effective, dash_offset) = trimmed
        .strip_prefix("- ")
        .map_or((trimmed, 0), |rest| (rest, 2));

    if let Some(colon_pos) = effective.find(':') {
        let key = effective[..colon_pos].trim().to_string();
        // Find where the colon is in the original line
        let indent = line.len() - trimmed.len();
        let abs_colon = indent + dash_offset + colon_pos;

        if col > abs_colon {
            // Cursor is after the colon — value position
            (Some(key), true)
        } else {
            // Cursor is before/at the colon — key position (typing a key)
            (None, false)
        }
    } else if effective.is_empty() || !effective.contains(':') {
        // No colon on this line — we're typing a new key
        (None, false)
    } else {
        (None, false)
    }
}

/// Extract the key name from a trimmed YAML line.
fn extract_key(trimmed: &str) -> Option<String> {
    let effective = trimmed.strip_prefix("- ").unwrap_or(trimmed);
    effective
        .find(':')
        .map(|pos| effective[..pos].trim().to_string())
}

// ── Provider logic ──────────────────────────────────────────────────────

fn provide_completions(ctx: CompletionContext) -> Vec<CompletionItem> {
    let (path, in_value) = parse_yaml_context(&ctx.text, ctx.cursor.line, ctx.cursor.col);

    // Build the lookup path as &str references
    let path_refs: Vec<&str> = path.iter().map(|s| s.as_str()).collect();

    if in_value {
        // We're in value position — look up valid values for this key
        match schema_lookup(&path_refs) {
            Some(SchemaNode::Enum(values)) => values
                .iter()
                .enumerate()
                .map(|(i, &v)| CompletionItem {
                    label: v.to_string(),
                    insert_text: None,
                    detail: None,
                    sort_order: i as i32,
                    kind: CompletionKind::Keyword,
                })
                .collect(),
            Some(SchemaNode::Object(_)) => {
                // Value position but the schema says this is an object —
                // no inline value completions (user needs to go to next line).
                vec![]
            }
            None => vec![],
        }
    } else {
        // We're in key position — look up valid child keys for the parent
        let parent_path = if path_refs.is_empty() {
            // At root level
            &path_refs[..]
        } else {
            // The last element is the partial key being typed — look up the parent
            &path_refs[..path_refs.len().saturating_sub(1)]
        };

        // Collect keys already present at the same indentation level to filter them out
        let existing_keys = collect_sibling_keys(&ctx.text, ctx.cursor.line);

        match schema_lookup(parent_path) {
            Some(SchemaNode::Object(keys)) => keys
                .iter()
                .filter(|(k, _)| !existing_keys.contains(&k.to_string()))
                .enumerate()
                .map(|(i, &(key, desc))| CompletionItem {
                    label: key.to_string(),
                    insert_text: Some(format!("{key}: ")),
                    detail: Some(desc.to_string()),
                    sort_order: i as i32,
                    kind: CompletionKind::Property,
                })
                .collect(),
            _ => vec![],
        }
    }
}

/// Collect the key names of sibling lines (same indentation level) around `line`.
fn collect_sibling_keys(text: &str, cursor_line: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    if cursor_line >= lines.len() {
        return vec![];
    }

    let current = lines[cursor_line];
    let trimmed = current.trim_start();
    let indent = current.len() - trimmed.len();

    let mut keys = Vec::new();

    // Scan backwards
    for i in (0..cursor_line).rev() {
        let l = lines[i];
        let lt = l.trim_start();
        let li = l.len() - lt.len();
        if lt.is_empty() || lt.starts_with('#') {
            continue;
        }
        if li < indent {
            break; // hit a parent — stop
        }
        if li == indent && let Some(k) = extract_key(lt) {
            keys.push(k);
        }
    }

    // Scan forwards (skip current line)
    for l in &lines[(cursor_line + 1)..] {
        let lt = l.trim_start();
        let li = l.len() - lt.len();
        if lt.is_empty() || lt.starts_with('#') {
            continue;
        }
        if li < indent {
            break;
        }
        if li == indent && let Some(k) = extract_key(lt) {
            keys.push(k);
        }
    }

    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_level_keys() {
        let yaml = "ty";
        let items = provide_completions(CompletionContext {
            text: yaml.to_string(),
            cursor: kode_leptos::Position::new(0, 2),
            version: 0,
            trigger: kode_leptos::CompletionTrigger::Typing,
        });
        // Should offer chart root keys (type, version, title, data, visualize, ...)
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"type"), "should suggest 'type', got: {labels:?}");
        assert!(labels.contains(&"visualize"), "should suggest 'visualize'");
    }

    #[test]
    fn type_value_completions() {
        let yaml = "type: ";
        let items = provide_completions(CompletionContext {
            text: yaml.to_string(),
            cursor: kode_leptos::Position::new(0, 6),
            version: 0,
            trigger: kode_leptos::CompletionTrigger::TriggerCharacter(' '),
        });
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"chart"), "should suggest 'chart', got: {labels:?}");
        assert!(labels.contains(&"source"), "should suggest 'source'");
    }

    #[test]
    fn visualize_type_values() {
        let yaml = "type: chart\nversion: 1\ndata: my_source\nvisualize:\n  type: ";
        let items = provide_completions(CompletionContext {
            text: yaml.to_string(),
            cursor: kode_leptos::Position::new(4, 8),
            version: 0,
            trigger: kode_leptos::CompletionTrigger::TriggerCharacter(' '),
        });
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"bar"), "should suggest 'bar', got: {labels:?}");
        assert!(labels.contains(&"line"), "should suggest 'line'");
        assert!(labels.contains(&"table"), "should suggest 'table'");
    }

    #[test]
    fn visualize_child_keys() {
        let yaml = "visualize:\n  type: bar\n  ";
        let items = provide_completions(CompletionContext {
            text: yaml.to_string(),
            cursor: kode_leptos::Position::new(2, 2),
            version: 0,
            trigger: kode_leptos::CompletionTrigger::Typing,
        });
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // 'type' is already present as a sibling — should be filtered out
        assert!(!labels.contains(&"type"), "'type' should be filtered out, got: {labels:?}");
        assert!(labels.contains(&"rows"), "should suggest 'rows'");
        assert!(labels.contains(&"columns"), "should suggest 'columns'");
    }

    #[test]
    fn nested_path_detection() {
        let yaml = "visualize:\n  axes:\n    left:\n      label: ";
        let (path, in_value) = parse_yaml_context(yaml, 3, 13);
        assert_eq!(path, vec!["visualize", "axes", "left", "label"]);
        assert!(in_value);
    }

    #[test]
    fn aggregate_measures_aggregation_values() {
        let yaml = "transform:\n  aggregate:\n    measures:\n      - aggregation: ";
        let items = provide_completions(CompletionContext {
            text: yaml.to_string(),
            cursor: kode_leptos::Position::new(3, 22),
            version: 0,
            trigger: kode_leptos::CompletionTrigger::TriggerCharacter(' '),
        });
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"sum"), "should suggest 'sum', got: {labels:?}");
        assert!(labels.contains(&"avg"), "should suggest 'avg'");
        assert!(labels.contains(&"countDistinct"), "should suggest 'countDistinct'");
    }

    #[test]
    fn data_provider_values() {
        let yaml = "data:\n  provider: ";
        let items = provide_completions(CompletionContext {
            text: yaml.to_string(),
            cursor: kode_leptos::Position::new(1, 12),
            version: 0,
            trigger: kode_leptos::CompletionTrigger::TriggerCharacter(' '),
        });
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"bigquery"), "should suggest 'bigquery', got: {labels:?}");
        assert!(labels.contains(&"postgres"), "should suggest 'postgres'");
    }

    #[test]
    fn existing_keys_filtered_at_root() {
        let yaml = "type: chart\nversion: 1\n\n";
        let items = provide_completions(CompletionContext {
            text: yaml.to_string(),
            cursor: kode_leptos::Position::new(2, 0),
            version: 0,
            trigger: kode_leptos::CompletionTrigger::Typing,
        });
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(!labels.contains(&"type"), "'type' already exists");
        assert!(!labels.contains(&"version"), "'version' already exists");
        assert!(labels.contains(&"data"), "should suggest 'data'");
    }

    // ── Markdown fence tests ────────────────────────────────────────

    #[test]
    fn markdown_completions_inside_chartml_fence() {
        let md = "# Dashboard\n\nSome text\n\n```chartml\ntype: chart\nvisualize:\n  type: \n```\n\nMore text";
        let items = provide_markdown_completions(CompletionContext {
            text: md.to_string(),
            // Line 7 = "  type: " (inside the fence)
            cursor: kode_leptos::Position::new(7, 8),
            version: 0,
            trigger: kode_leptos::CompletionTrigger::TriggerCharacter(' '),
        });
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"bar"), "should suggest chart types inside chartml fence, got: {labels:?}");
        assert!(labels.contains(&"line"));
    }

    #[test]
    fn markdown_completions_outside_fence_returns_empty() {
        let md = "# Dashboard\n\nSome text\n\n```chartml\ntype: chart\n```\n\nMore text";
        let items = provide_markdown_completions(CompletionContext {
            text: md.to_string(),
            // Line 2 = "Some text" (outside any fence)
            cursor: kode_leptos::Position::new(2, 5),
            version: 0,
            trigger: kode_leptos::CompletionTrigger::Typing,
        });
        assert!(items.is_empty(), "should return nothing outside chartml fence");
    }

    #[test]
    fn markdown_completions_in_non_chartml_fence_returns_empty() {
        let md = "```sql\nSELECT *\n```";
        let items = provide_markdown_completions(CompletionContext {
            text: md.to_string(),
            // Line 1 = "SELECT *" (inside sql fence)
            cursor: kode_leptos::Position::new(1, 4),
            version: 0,
            trigger: kode_leptos::CompletionTrigger::Typing,
        });
        assert!(items.is_empty(), "should return nothing inside sql fence");
    }

    #[test]
    fn markdown_root_keys_inside_fence() {
        let md = "```chartml\n\n```";
        let items = provide_markdown_completions(CompletionContext {
            text: md.to_string(),
            // Line 1 = "" (blank line inside fence)
            cursor: kode_leptos::Position::new(1, 0),
            version: 0,
            trigger: kode_leptos::CompletionTrigger::Typing,
        });
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"type"), "should suggest root keys, got: {labels:?}");
        assert!(labels.contains(&"visualize"));
    }
}
