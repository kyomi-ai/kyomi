// SPDX-License-Identifier: AGPL-3.0-or-later

//! ChartML specification tool — retrieve ChartML docs for visualization creation.

use async_trait::async_trait;

use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

// ---------------------------------------------------------------------------
// GetChartMLSpecTool
// ---------------------------------------------------------------------------

/// Get ChartML specification documentation for creating visualizations.
pub struct GetChartMLSpecTool;

#[async_trait]
impl AgentTool for GetChartMLSpecTool {
    fn name(&self) -> &str {
        "get_chartml_spec"
    }

    fn description(&self) -> &str {
        "Get ChartML specification documentation for creating visualizations."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["essential", "full"],
                    "default": "essential"
                },
                "section": {
                    "type": "string",
                    "enum": ["data", "transform", "visualize", "marks", "axes", "layout", "format"]
                }
            }
        })
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let mode = args
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("essential");
        let section = args.get("section").and_then(|v| v.as_str());

        if mode == "essential" {
            Ok(serde_json::json!({
                "success": true,
                "mode": "essential",
                "content": crate::prompt::CHARTML_QUICK_REFERENCE,
            })
            .to_string())
        } else {
            // Full mode: return SPECIFICATION.md and optionally extract section
            let content = crate::prompt::CHARTML_SPECIFICATION;
            if let Some(section_name) = section {
                let section_content = extract_section(content, section_name);
                Ok(serde_json::json!({
                    "success": true,
                    "mode": "full",
                    "section": section_name,
                    "content": section_content.unwrap_or_else(|| {
                        format!("Section '{section_name}' not found in specification.")
                    }),
                })
                .to_string())
            } else {
                Ok(serde_json::json!({
                    "success": true,
                    "mode": "full",
                    "content": content,
                    "note": "Full specification returned. For specific sections, use the 'section' parameter.",
                })
                .to_string())
            }
        }
    }
}

/// Extract a section from markdown content by heading.
///
/// Searches for `## <section_name>` (case-insensitive first character),
/// and returns content from that heading until the next `## ` heading.
fn extract_section(content: &str, section_name: &str) -> Option<String> {
    let patterns = [
        format!("## {section_name}"),
        format!("## {}", capitalize_first(section_name)),
    ];

    for pattern in &patterns {
        if let Some(start_idx) = content.find(pattern.as_str()) {
            let section_content = &content[start_idx..];
            // Find the next ## heading to delimit the section
            if let Some(end_offset) = section_content[pattern.len()..].find("\n## ") {
                return Some(section_content[..pattern.len() + end_offset].to_string());
            }
            return Some(section_content.to_string());
        }
    }
    None
}

/// Capitalize the first character of a string.
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_section_finds_heading() {
        let content = "## data\nData section content\n## transform\nTransform content";
        let result = extract_section(content, "data");
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.starts_with("## data"));
        assert!(text.contains("Data section content"));
        assert!(!text.contains("Transform content"));
    }

    #[test]
    fn extract_section_capitalized() {
        let content = "## Data\nData section\n## Transform\nTransform section";
        let result = extract_section(content, "data");
        assert!(result.is_some());
        assert!(result.unwrap().starts_with("## Data"));
    }

    #[test]
    fn extract_section_last_section() {
        let content = "## visualize\nVisualize content\nMore content";
        let result = extract_section(content, "visualize");
        assert!(result.is_some());
        assert!(result.unwrap().contains("More content"));
    }

    #[test]
    fn extract_section_not_found() {
        let content = "## data\nSome content";
        let result = extract_section(content, "nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn capitalize_first_basic() {
        assert_eq!(capitalize_first("data"), "Data");
        assert_eq!(capitalize_first(""), "");
        assert_eq!(capitalize_first("a"), "A");
    }
}
