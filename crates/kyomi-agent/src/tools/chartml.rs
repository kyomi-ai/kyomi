// SPDX-License-Identifier: AGPL-3.0-or-later

//! ChartML specification tool — retrieve ChartML docs for visualization creation.

use async_trait::async_trait;

use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

/// A lookup key exposed through the `section` parameter, paired with the exact
/// heading it selects in `SPECIFICATION.md`.
///
/// The level is part of the identity: the specification has both a
/// `### Grid Layout` subsection and a `## Grid Layout` section.
struct Section {
    name: &'static str,
    level: usize,
    heading: &'static str,
}

/// Every section the tool can return. Keep this table as the single source of
/// truth: the parameter schema and the lookup both derive from it.
const SECTIONS: &[Section] = &[
    Section {
        name: "overview",
        level: 2,
        heading: "Overview",
    },
    Section {
        name: "source",
        level: 2,
        heading: "Component 1: Source",
    },
    Section {
        name: "params",
        level: 2,
        heading: "Component 2: Params",
    },
    Section {
        name: "chart",
        level: 2,
        heading: "Component 3: Chart",
    },
    Section {
        name: "data",
        level: 3,
        heading: "Data Layer",
    },
    Section {
        name: "transform",
        level: 3,
        heading: "Transform Pipeline (Optional)",
    },
    Section {
        name: "visualize",
        level: 3,
        heading: "Visualize Layer",
    },
    Section {
        name: "annotations",
        level: 3,
        heading: "Annotations (Reference Lines & Bands)",
    },
    Section {
        name: "style",
        level: 2,
        heading: "Component 4: Style",
    },
    Section {
        name: "config",
        level: 2,
        heading: "Component 5: Config",
    },
    Section {
        name: "variables",
        level: 2,
        heading: "Variable References",
    },
    Section {
        name: "formatting",
        level: 2,
        heading: "Number and Date Formatting",
    },
    Section {
        name: "layout",
        level: 2,
        heading: "Grid Layout",
    },
    Section {
        name: "dashboard-example",
        level: 2,
        heading: "Complete Dashboard Example",
    },
];

/// Resolve a `section` parameter value to its heading level and text in the
/// specification.
fn section_heading(section_name: &str) -> Option<(usize, &'static str)> {
    SECTIONS
        .iter()
        .find(|section| section.name == section_name)
        .map(|section| (section.level, section.heading))
}

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
        let sections: Vec<&str> = SECTIONS.iter().map(|section| section.name).collect();

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
                    "enum": sections
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

/// Extract a named section from markdown content.
fn extract_section(content: &str, section_name: &str) -> Option<String> {
    let (level, heading) = section_heading(section_name)?;
    extract_heading(content, level, heading)
}

/// Extract a section from markdown content by heading.
///
/// The section runs from the whole heading at `heading_level` whose text
/// matches `heading_text` up to the next heading of the same or higher level,
/// so a subsection never swallows its parent's remaining siblings. Headings
/// inside fenced code blocks are ignored: the specification contains YAML
/// comments such as `# OR` that would otherwise look like top-level headings.
fn extract_heading(content: &str, heading_level: usize, heading_text: &str) -> Option<String> {
    let mut fence = None;
    let mut section: Option<(usize, usize)> = None;
    let mut offset = 0;

    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();

        let line = line.trim_end_matches(['\r', '\n']);
        if is_inside_code_fence(line, &mut fence) {
            continue;
        }

        let Some((level, text)) = parse_heading(line) else {
            continue;
        };

        if let Some((start, section_level)) = section {
            if level <= section_level {
                return Some(content[start..line_start].trim_end().to_string());
            }
        } else if level == heading_level && text == heading_text {
            section = Some((line_start, level));
        }
    }

    section.map(|(start, _)| content[start..].trim_end().to_string())
}

/// Parse an ATX Markdown heading into its level and trimmed text.
fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let line = line.get(indent..).filter(|_| indent <= 3)?;

    let level = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }

    let text = line.get(level..)?.strip_prefix(' ')?.trim();
    if text.is_empty() {
        return None;
    }

    Some((level, text))
}

/// Update the fenced-code-block state for one line and report whether that
/// line belongs to a code block (a fence delimiter or its contents).
///
/// Follows the CommonMark rules that matter here: a fence may be indented by
/// at most three spaces, and a backtick fence's info string may not contain a
/// backtick. A line indented four or more spaces is an indented code block, so
/// it is content inside a fence but never opens or closes one.
fn is_inside_code_fence(line: &str, fence: &mut Option<(char, usize)>) -> bool {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let Some(trimmed) = line.get(indent..).filter(|_| indent <= 3) else {
        return fence.is_some();
    };

    let marker = trimmed.chars().next();
    let run = marker
        .map(|marker| trimmed.chars().take_while(|c| *c == marker).count())
        .unwrap_or(0);
    let looks_like_fence = run >= 3 && matches!(marker, Some('`' | '~'));
    let backtick_in_info = marker == Some('`') && trimmed[run..].contains('`');

    match fence {
        Some((open_marker, open_run)) => {
            if looks_like_fence
                && marker == Some(*open_marker)
                && run >= *open_run
                && trimmed[run..].trim().is_empty()
            {
                *fence = None;
            }
            true
        }
        None => {
            if looks_like_fence
                && !backtick_in_info
                && let Some(marker) = marker
            {
                *fence = Some((marker, run));
                return true;
            }
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent oracle for the real specification: parameter name, heading
    /// level, heading text, total extracted line count and last extracted line.
    ///
    /// Deliberately not derived from `SECTIONS`: a wrong mapping, a dropped
    /// section or a silently truncated body must fail here.
    const EXPECTED_SECTIONS: &[(&str, usize, &str, usize, &str)] = &[
        ("overview", 2, "Overview", 57, "---"),
        ("source", 2, "Component 1: Source", 114, "---"),
        ("params", 2, "Component 2: Params", 164, "---"),
        ("chart", 2, "Component 3: Chart", 850, "---"),
        (
            "data",
            3,
            "Data Layer",
            74,
            "- Each value follows the same format as single-source specs (datasource+query, provider+query, inline, or string reference)",
        ),
        (
            "transform",
            3,
            "Transform Pipeline (Optional)",
            344,
            "- The `aggregate` stage operates on a single table; for joins, use the `sql` stage first",
        ),
        ("visualize", 3, "Visualize Layer", 162, "```"),
        (
            "annotations",
            3,
            "Annotations (Reference Lines & Bands)",
            43,
            "See EXAMPLES.md for complete annotation examples with goals, targets, and event markers.",
        ),
        ("style", 2, "Component 4: Style", 163, "---"),
        ("config", 2, "Component 5: Config", 131, "---"),
        ("variables", 2, "Variable References", 30, "---"),
        ("formatting", 2, "Number and Date Formatting", 46, "---"),
        ("layout", 2, "Grid Layout", 14, "---"),
        (
            "dashboard-example",
            2,
            "Complete Dashboard Example",
            100,
            "**End of Specification**",
        ),
    ];

    #[test]
    fn every_section_resolves_in_the_real_specification() {
        for &(name, level, heading, line_count, last_line) in EXPECTED_SECTIONS {
            let content = extract_section(crate::prompt::CHARTML_SPECIFICATION, name)
                .unwrap_or_else(|| panic!("section '{name}' not found"));

            let (actual_level, actual_heading) =
                parse_heading(content.lines().next().expect("section is empty"))
                    .expect("section does not start with a heading");
            assert_eq!(
                (actual_level, actual_heading),
                (level, heading),
                "section '{name}' starts at the wrong heading"
            );

            let body = content.split_once('\n').map(|(_, body)| body).unwrap_or("");
            assert!(!body.trim().is_empty(), "section '{name}' has no body");
            assert_eq!(
                content.lines().count(),
                line_count,
                "section '{name}' has the wrong length"
            );
            assert_eq!(
                content.lines().last(),
                Some(last_line),
                "section '{name}' ends at the wrong line"
            );
        }
    }

    #[test]
    fn real_sections_stop_at_the_next_heading_of_the_same_or_higher_level() {
        for section in SECTIONS {
            let content = extract_section(crate::prompt::CHARTML_SPECIFICATION, section.name)
                .unwrap_or_else(|| panic!("section '{}' not found", section.name));
            let mut lines = content.lines();
            let (level, _) = parse_heading(lines.next().expect("section is empty"))
                .expect("section does not start with a heading");

            let mut fence = None;
            for line in lines {
                if is_inside_code_fence(line, &mut fence) {
                    continue;
                }
                if let Some((next_level, next_heading)) = parse_heading(line) {
                    assert!(
                        next_level > level,
                        "section '{}' contains heading '{next_heading}' at level {next_level}",
                        section.name
                    );
                }
            }
        }
    }

    #[test]
    fn section_parameter_matches_the_lookup_table() {
        let schema = GetChartMLSpecTool.parameters_schema();
        let enum_values: Vec<&str> = schema["properties"]["section"]["enum"]
            .as_array()
            .expect("section enum is an array")
            .iter()
            .map(|value| value.as_str().expect("section value is a string"))
            .collect();
        let names: Vec<&str> = SECTIONS.iter().map(|section| section.name).collect();
        assert_eq!(enum_values, names);

        let lookup: Vec<(&str, usize, &str)> = SECTIONS
            .iter()
            .map(|section| (section.name, section.level, section.heading))
            .collect();
        let expected: Vec<(&str, usize, &str)> = EXPECTED_SECTIONS
            .iter()
            .map(|(name, level, heading, ..)| (*name, *level, *heading))
            .collect();
        assert_eq!(lookup, expected);
    }

    #[test]
    fn parse_heading_allows_up_to_three_leading_spaces() {
        assert_eq!(parse_heading("   ## Overview"), Some((2, "Overview")));
        assert_eq!(parse_heading("    ## Overview"), None);
    }

    #[test]
    fn extract_heading_stops_at_the_next_heading_of_the_same_level() {
        let content = "## Mark\nmark body\n### Mark Details\ndetail body\n## Next\nnext body";
        let result = extract_heading(content, 2, "Mark").expect("section should be found");

        assert!(result.starts_with("## Mark"));
        assert!(result.contains("mark body"));
        assert!(result.contains("### Mark Details"));
        assert!(result.contains("detail body"));
        assert!(!result.contains("next body"));
    }

    #[test]
    fn extract_heading_stops_at_the_next_heading_of_a_higher_level() {
        let content = "### Child\nchild body\n#### Grandchild\ndeep body\n## Parent\nparent body";
        let result = extract_heading(content, 3, "Child").expect("section should be found");

        assert!(result.starts_with("### Child"));
        assert!(result.contains("deep body"));
        assert!(!result.contains("parent body"));
    }

    #[test]
    fn extract_heading_matches_whole_headings_only() {
        let content =
            "## Data Layer\nlayer body\n### Datasource Selection\nwrong body\n## Next\nnext body";

        assert!(extract_heading(content, 2, "Data").is_none());
        assert_eq!(
            extract_heading(content, 2, "Data Layer").as_deref(),
            Some("## Data Layer\nlayer body\n### Datasource Selection\nwrong body")
        );
    }

    #[test]
    fn extract_heading_distinguishes_same_text_at_different_levels() {
        let content = "### Grid Layout\nparam body\n## Grid Layout\nlayout body";

        let result = extract_heading(content, 2, "Grid Layout").expect("section should be found");
        assert!(result.starts_with("## Grid Layout"));
        assert!(result.contains("layout body"));
        assert!(!result.contains("param body"));

        let result = extract_heading(content, 3, "Grid Layout").expect("section should be found");
        assert_eq!(result, "### Grid Layout\nparam body");
    }

    #[test]
    fn extract_heading_ignores_headings_inside_code_fences() {
        let content =
            "## Example\nintro\n```markdown\n# Not A Heading\n```\noutro\n## Next\nnext body";
        let result = extract_heading(content, 2, "Example").expect("section should be found");

        assert!(result.starts_with("## Example"));
        assert!(result.contains("# Not A Heading"));
        assert!(result.contains("outro"));
        assert!(!result.contains("next body"));
    }

    #[test]
    fn extract_heading_does_not_open_a_fence_for_invalid_fence_lines() {
        for fence in ["    ```", "```foo`bar"] {
            let content = format!("## Overview\nbefore\n{fence}\n## Component 1: Source\nsource");
            let result = extract_heading(&content, 2, "Component 1: Source")
                .expect("heading after the invalid fence line should be found");

            assert_eq!(
                result, "## Component 1: Source\nsource",
                "fence line: {fence}"
            );
        }
    }

    #[test]
    fn extract_heading_recognizes_three_space_indented_fences() {
        let content = "## Overview\nbefore\n   ```\n# Not A Heading\n   ```\n## Next\nnext body";
        let result = extract_heading(content, 2, "Overview").expect("section should be found");

        assert!(result.contains("# Not A Heading"));
        assert!(!result.contains("next body"));
    }

    #[test]
    fn extract_heading_treats_an_indented_fence_marker_inside_a_fence_as_content() {
        let content = "## Overview\nbefore\n```chartml\n    ```\n## Component 1: Source\nsource";
        let result = extract_heading(content, 2, "Overview").expect("section should be found");

        assert!(result.contains("## Component 1: Source"));
        assert!(result.contains("source"));
    }

    #[test]
    fn extract_section_rejects_unknown_names() {
        assert!(extract_section("## Overview\ntext", "not-a-section").is_none());
    }
}
