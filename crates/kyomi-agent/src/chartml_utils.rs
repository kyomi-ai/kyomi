// SPDX-License-Identifier: AGPL-3.0-or-later

//! ChartML extraction utilities — shared between Slack, PDF export, and email.
//!
//! Extracts ChartML specs from ` ```chartml ``` ` fenced blocks,
//! provides helpers for chart title and visualize type lookup.

use std::sync::LazyLock;

use serde_json::Value;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Regex
// ---------------------------------------------------------------------------

/// Regex to capture ` ```chartml ` fenced blocks.
///
/// The closing fence may or may not be preceded by a newline — the chart tool
/// emits `format!("```chartml\n{yaml}```")` without a trailing newline, while
/// the LLM often adds one.  `\s*` before the closing fence handles both.
pub static RE_CHARTML: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?s)```chartml\s*\n([\s\S]*?)\s*```").expect("valid regex")
});

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of extracting ChartML from a message.
///
/// A single fenced ` ```chartml ``` ` block can contain:
/// - A single YAML dict (one component)
/// - A YAML list of multiple components
///
/// Only `type: "chart"` components are extracted (matching Python).
/// Non-chart types (`source`, `params`, `style`, `config`) are skipped.
pub struct ExtractionResult {
    /// All extracted chart specs (flat list, filtered to `type: "chart"` only).
    pub specs: Vec<Value>,
    /// Fenced blocks with their byte ranges and mapping to spec indices.
    pub blocks: Vec<ExtractedBlock>,
}

/// A fenced ` ```chartml ``` ` block with its byte range and associated spec indices.
pub struct ExtractedBlock {
    /// Byte range of the full ` ```chartml ... ``` ` block in the source text.
    pub range: std::ops::Range<usize>,
    /// Indices into `ExtractionResult::specs` for chart components in this block.
    pub spec_indices: Vec<usize>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Extract ChartML specs from ` ```chartml ` fenced blocks.
///
/// Handles both single-component (YAML dict) and multi-component (YAML list)
/// blocks. Only extracts components with `type: "chart"` — non-chart types
/// (`source`, `params`, `style`, `config`) are skipped silently.
///
/// This matches Python's `extract_chartml_from_response()` behavior.
pub fn extract_chartml_specs(message: &str) -> ExtractionResult {
    let mut specs: Vec<Value> = Vec::new();
    let mut blocks: Vec<ExtractedBlock> = Vec::new();

    for cap in RE_CHARTML.captures_iter(message) {
        let Some(full_match) = cap.get(0) else {
            continue;
        };
        let yaml_content = &cap[1];

        let parsed = match serde_yaml::from_str::<Value>(yaml_content) {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "Failed to parse ChartML YAML");
                blocks.push(ExtractedBlock {
                    range: full_match.start()..full_match.end(),
                    spec_indices: Vec::new(),
                });
                continue;
            }
        };

        // Handle both single component (dict) and component array (list)
        let components: Vec<Value> = match &parsed {
            Value::Object(_) => vec![parsed],
            Value::Array(arr) => arr.clone(),
            _ => {
                warn!("ChartML block is neither dict nor list");
                blocks.push(ExtractedBlock {
                    range: full_match.start()..full_match.end(),
                    spec_indices: Vec::new(),
                });
                continue;
            }
        };

        let mut block_spec_indices = Vec::new();

        for component in components {
            if !component.is_object() {
                warn!("ChartML component is not a dict, skipping");
                continue;
            }

            // Only extract chart components (matching Python's filter)
            match component.get("type").and_then(|v| v.as_str()) {
                Some("chart") => {
                    let idx = specs.len();
                    specs.push(component);
                    block_spec_indices.push(idx);
                }
                Some(other_type) => {
                    // Non-chart types (source, params, style, config) are skipped
                    info!(component_type = %other_type, "Skipping non-chart ChartML component");
                }
                None => {
                    warn!("ChartML component missing 'type' field, skipping");
                }
            }
        }

        blocks.push(ExtractedBlock {
            range: full_match.start()..full_match.end(),
            spec_indices: block_spec_indices,
        });
    }

    info!(
        specs = specs.len(),
        blocks = blocks.len(),
        "Extracted ChartML specs from message"
    );

    ExtractionResult { specs, blocks }
}

/// Get the `visualize.type` from a ChartML spec.
pub fn get_visualize_type(spec: &Value) -> Option<String> {
    spec.get("visualize")?
        .get("type")?
        .as_str()
        .map(String::from)
}

/// Get the title from a ChartML spec.
pub fn get_chart_title(spec: &Value) -> String {
    spec.get("title")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| "Chart".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_chartml_basic() {
        let msg =
            "Text\n```chartml\ntype: chart\ntitle: Revenue\nvisualize:\n  type: bar\n```\nMore text";
        let result = extract_chartml_specs(msg);
        assert_eq!(result.specs.len(), 1);
        assert_eq!(result.blocks.len(), 1);
        assert_eq!(
            result.specs[0].get("title").unwrap().as_str().unwrap(),
            "Revenue"
        );
    }

    #[test]
    fn extract_chartml_multiple() {
        let msg =
            "```chartml\ntype: chart\ntitle: A\n```\nMiddle\n```chartml\ntype: chart\ntitle: B\n```";
        let result = extract_chartml_specs(msg);
        assert_eq!(result.specs.len(), 2);
        assert_eq!(result.blocks.len(), 2);
    }

    #[test]
    fn extract_chartml_none() {
        let result = extract_chartml_specs("No charts here");
        assert!(result.specs.is_empty());
        assert!(result.blocks.is_empty());
    }

    #[test]
    fn extract_chartml_invalid_yaml_skipped() {
        let msg = "```chartml\n[invalid yaml: {\n```";
        let result = extract_chartml_specs(msg);
        assert!(result.specs.is_empty());
        assert_eq!(result.blocks.len(), 1);
        assert!(result.blocks[0].spec_indices.is_empty());
    }

    #[test]
    fn extract_chartml_filters_non_chart_types() {
        let msg = "```chartml\ntype: source\nname: my_source\n```";
        let result = extract_chartml_specs(msg);
        assert!(result.specs.is_empty());
        assert_eq!(result.blocks.len(), 1);
        assert!(result.blocks[0].spec_indices.is_empty());
    }

    #[test]
    fn extract_chartml_filters_missing_type() {
        let msg = "```chartml\ntitle: NoType\nvisualize:\n  type: bar\n```";
        let result = extract_chartml_specs(msg);
        assert!(result.specs.is_empty());
    }

    #[test]
    fn extract_chartml_yaml_list() {
        let msg =
            "```chartml\n- type: chart\n  title: Chart A\n- type: chart\n  title: Chart B\n```";
        let result = extract_chartml_specs(msg);
        assert_eq!(result.specs.len(), 2);
        assert_eq!(result.blocks.len(), 1);
        assert_eq!(result.blocks[0].spec_indices, vec![0, 1]);
    }

    #[test]
    fn extract_chartml_yaml_list_mixed_types() {
        let msg = "```chartml\n- type: style\n  palette: blues\n- type: chart\n  title: Revenue\n- type: config\n  key: val\n```";
        let result = extract_chartml_specs(msg);
        assert_eq!(result.specs.len(), 1);
        assert_eq!(
            result.specs[0].get("title").unwrap().as_str().unwrap(),
            "Revenue"
        );
        assert_eq!(result.blocks[0].spec_indices, vec![0]);
    }

    #[test]
    fn extract_chartml_yaml_list_no_charts() {
        let msg = "```chartml\n- type: source\n  name: src\n- type: params\n  key: val\n```";
        let result = extract_chartml_specs(msg);
        assert!(result.specs.is_empty());
        assert_eq!(result.blocks.len(), 1);
        assert!(result.blocks[0].spec_indices.is_empty());
    }

    #[test]
    fn chart_title_from_top_level() {
        assert_eq!(get_chart_title(&json!({"title": "My Chart"})), "My Chart");
    }

    #[test]
    fn chart_title_fallback() {
        assert_eq!(get_chart_title(&json!({})), "Chart");
    }

    #[test]
    fn visualize_type_extracted() {
        assert_eq!(
            get_visualize_type(&json!({"visualize": {"type": "bar"}})),
            Some("bar".to_string())
        );
    }

    #[test]
    fn visualize_type_none() {
        assert_eq!(get_visualize_type(&json!({})), None);
    }
}
