// SPDX-License-Identifier: AGPL-3.0-or-later

//! ChartML parser — extracts ChartML components from markdown documents.
//!
//! Ported from `apps/frontend/src/lib/markdownChartMLParser.js`.
//! Finds all ` ```chartml ` fenced code blocks in markdown, parses their
//! YAML/JSON content, and categorizes components by type.

use regex::Regex;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of parsing a dashboard's markdown content for ChartML components.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ParsedDashboard {
    /// Named data sources defined in the dashboard.
    pub sources: Vec<SourceDef>,
    /// Individual chart specifications (arrays are flattened).
    pub charts: Vec<ChartSpec>,
    /// Parameter definitions (dashboard-level and chart-level).
    pub params: Vec<ParamGroup>,
    /// Named style definitions.
    pub styles: Vec<StyleDef>,
    /// Dashboard-level configuration (at most one per document).
    pub config: Option<serde_json::Value>,
}

/// A named data source definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceDef {
    pub name: String,
    pub definition: serde_json::Value,
}

/// A single chart specification with position tracking.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChartSpec {
    /// The full chart spec as parsed YAML/JSON.
    pub spec: serde_json::Value,
    /// Index of the chartml block in the document (0-based).
    pub block_index: usize,
    /// Index within an array block (0 for non-array charts).
    pub array_index: usize,
    /// Scoping key for chart-level parameters: `"chart_{block}_{array}"`.
    pub scope_key: String,
}

/// A group of parameter definitions from a single params block.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParamGroup {
    /// Individual parameter definitions.
    pub params: Vec<ParamDef>,
    /// Index of the chartml block containing these params.
    pub block_index: usize,
}

/// A single parameter definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParamDef {
    pub id: String,
    /// Type: `"select"`, `"multiselect"`, `"daterange"`, `"number"`, `"text"`.
    pub param_type: String,
    pub label: Option<String>,
    pub default: Option<serde_json::Value>,
    pub options: Option<Vec<serde_json::Value>>,
    pub placeholder: Option<String>,
    pub layout: Option<ParamLayout>,
}

/// Layout hints for a parameter control.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParamLayout {
    pub col_span: Option<i32>,
}

/// A named style definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StyleDef {
    pub name: String,
    pub definition: serde_json::Value,
}

/// A parameter with its resolved scope key, produced by [`extract_scoped_params`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScopedParam {
    pub param: ParamDef,
    /// Empty string for dashboard-level params, or
    /// `"chart_{block}_{array}.{param_id}"` for chart-level params.
    pub scope_key: String,
}

// ---------------------------------------------------------------------------
// Block extraction
// ---------------------------------------------------------------------------

/// Extract all chartml fenced code blocks from markdown.
///
/// Returns `Vec<(block_content, block_index)>` where `block_index` is the
/// sequential position of the chartml block among all chartml blocks (0-based).
fn extract_chartml_blocks(content: &str) -> Vec<(String, usize)> {
    // Matches ```chartml followed by optional whitespace, then a newline,
    // then the block body (non-greedy), then closing ```.
    // This mirrors the JS regex: /```chartml\s*\n([\s\S]*?)```/g
    let re = Regex::new(r"(?s)```chartml\s*\n(.*?)```").expect("valid regex");

    re.captures_iter(content)
        .enumerate()
        .map(|(idx, cap)| {
            let body = cap.get(1).map_or("", |m| m.as_str()).trim().to_string();
            (body, idx)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// YAML parsing helper
// ---------------------------------------------------------------------------

/// Parse content as YAML, falling back to JSON.
/// Mirrors the JS `parseYAML` function.
fn parse_yaml(content: &str) -> Result<serde_json::Value, String> {
    // Try YAML first (YAML is a superset of JSON).
    match serde_yaml::from_str::<serde_json::Value>(content) {
        Ok(val) => Ok(val),
        Err(yaml_err) => {
            // Fallback: try to parse as JSON.
            serde_json::from_str::<serde_json::Value>(content)
                .map_err(|_| format!("Failed to parse YAML: {yaml_err}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Core parser
// ---------------------------------------------------------------------------

/// Parse markdown content and extract all ChartML components.
///
/// Finds ` ```chartml ` fenced code blocks, parses their YAML content,
/// and categorizes components by type (source, params, chart, style, config).
///
/// Chart blocks containing arrays are flattened — each array element
/// becomes a separate [`ChartSpec`] with its own `array_index`.
pub fn parse_markdown_chartml(content: &str) -> ParsedDashboard {
    let mut result = ParsedDashboard::default();

    let blocks = extract_chartml_blocks(content);

    for (body, block_index) in blocks {
        // Parse the YAML/JSON content; skip malformed blocks gracefully.
        let parsed = match parse_yaml(&body) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // The JS parser handles both single objects and arrays at the top
        // level: `const components = Array.isArray(parsed) ? parsed : [parsed];`
        let components: Vec<serde_json::Value> = match parsed {
            serde_json::Value::Array(arr) => arr,
            other => vec![other],
        };

        // Track how many chart-type components we've seen inside this block
        // so we can assign array_index correctly.
        let mut chart_array_index: usize = 0;

        for component in components {
            let obj = match component.as_object() {
                Some(o) => o,
                None => continue,
            };

            // Must have a `type` field.
            let comp_type = match obj.get("type").and_then(|v| v.as_str()) {
                Some(t) => t.to_string(),
                None => continue,
            };

            match comp_type.as_str() {
                "source" => {
                    let name = match obj.get("name").and_then(|v| v.as_str()) {
                        Some(n) => n.to_string(),
                        None => continue, // JS throws, we skip gracefully.
                    };

                    // Store definition without name, type, version fields.
                    let mut def = component.clone();
                    if let Some(map) = def.as_object_mut() {
                        map.remove("name");
                        map.remove("type");
                        map.remove("version");
                    }

                    result.sources.push(SourceDef {
                        name,
                        definition: def,
                    });
                }

                "params" => {
                    let params_arr = match obj.get("params").and_then(|v| v.as_array()) {
                        Some(a) => a,
                        None => continue, // JS throws, we skip.
                    };

                    let mut param_defs = Vec::new();
                    for p in params_arr {
                        let p_obj = match p.as_object() {
                            Some(o) => o,
                            None => continue,
                        };

                        let id = match p_obj.get("id").and_then(|v| v.as_str()) {
                            Some(s) => s.to_string(),
                            None => continue,
                        };

                        let param_type = match p_obj.get("type").and_then(|v| v.as_str()) {
                            Some(s) => s.to_string(),
                            None => continue,
                        };

                        let label = p_obj.get("label").and_then(|v| v.as_str()).map(String::from);
                        let default = p_obj.get("default").cloned();
                        let options = p_obj
                            .get("options")
                            .and_then(|v| v.as_array())
                            .cloned();
                        let placeholder =
                            p_obj.get("placeholder").and_then(|v| v.as_str()).map(String::from);
                        let layout = p_obj.get("layout").and_then(|v| {
                            let col_span = v
                                .get("col_span")
                                .or_else(|| v.get("colSpan"))
                                .and_then(|c| c.as_i64())
                                .map(|c| c as i32);
                            if col_span.is_some() {
                                Some(ParamLayout { col_span })
                            } else {
                                None
                            }
                        });

                        param_defs.push(ParamDef {
                            id,
                            param_type,
                            label,
                            default,
                            options,
                            placeholder,
                            layout,
                        });
                    }

                    result.params.push(ParamGroup {
                        params: param_defs,
                        block_index,
                    });
                }

                "style" => {
                    let name = match obj.get("name").and_then(|v| v.as_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };

                    // Store definition without name, type, version fields.
                    let mut def = component.clone();
                    if let Some(map) = def.as_object_mut() {
                        map.remove("name");
                        map.remove("type");
                        map.remove("version");
                    }

                    result.styles.push(StyleDef {
                        name,
                        definition: def,
                    });
                }

                "config" => {
                    // Only one config per document; later ones silently
                    // overwrite (matches JS behaviour).
                    let mut cfg = component.clone();
                    if let Some(map) = cfg.as_object_mut() {
                        map.remove("type");
                        map.remove("version");
                    }
                    result.config = Some(cfg);
                }

                "chart" => {
                    let scope_key = format!("chart_{block_index}_{chart_array_index}");
                    result.charts.push(ChartSpec {
                        spec: component.clone(),
                        block_index,
                        array_index: chart_array_index,
                        scope_key,
                    });
                    chart_array_index += 1;
                }

                _ => {
                    // Unknown component type — skip silently (matches JS).
                }
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Scoped parameter extraction
// ---------------------------------------------------------------------------

/// Extract parameters from a parsed dashboard with proper scoping.
///
/// Dashboard-level params (those not associated with a specific chart) get
/// `scope_key = ""` (empty string).
///
/// Chart-level params get `scope_key = "chart_{block}_{array}.{param_id}"`.
///
/// The current implementation treats all params blocks as dashboard-level since
/// the JS parser does not associate params with specific charts at parse time.
/// Chart-level scoping can be layered on by matching `block_index` proximity.
pub fn extract_scoped_params(parsed: &ParsedDashboard) -> Vec<ScopedParam> {
    let mut scoped = Vec::new();

    for group in &parsed.params {
        for param in &group.params {
            // Dashboard-level: scope_key is empty.
            scoped.push(ScopedParam {
                param: param.clone(),
                scope_key: String::new(),
            });
        }
    }

    scoped
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // 1. Empty content
    #[test]
    fn empty_content_returns_empty_dashboard() {
        let result = parse_markdown_chartml("");
        assert!(result.sources.is_empty());
        assert!(result.charts.is_empty());
        assert!(result.params.is_empty());
        assert!(result.styles.is_empty());
        assert!(result.config.is_none());
    }

    // 2. Plain markdown with no chartml blocks
    #[test]
    fn plain_markdown_no_chartml() {
        let md = "# Hello World\n\nSome text.\n\n```sql\nSELECT 1;\n```\n";
        let result = parse_markdown_chartml(md);
        assert!(result.charts.is_empty());
        assert!(result.sources.is_empty());
    }

    // 3. Single chart block
    #[test]
    fn single_chart_block() {
        let md = r#"# Dashboard

```chartml
type: chart
version: 1
title: Revenue
data:
  source: main
visualize:
  type: bar
```
"#;
        let result = parse_markdown_chartml(md);
        assert_eq!(result.charts.len(), 1);

        let chart = &result.charts[0];
        assert_eq!(chart.block_index, 0);
        assert_eq!(chart.array_index, 0);
        assert_eq!(chart.scope_key, "chart_0_0");
        assert_eq!(chart.spec["title"], "Revenue");
        assert_eq!(chart.spec["type"], "chart");
    }

    // 4. Array chart block (3 charts in one block)
    #[test]
    fn array_chart_block() {
        let md = r#"```chartml
- type: chart
  version: 1
  title: Chart A
  data:
    source: main
  visualize:
    type: bar
- type: chart
  version: 1
  title: Chart B
  data:
    source: main
  visualize:
    type: line
- type: chart
  version: 1
  title: Chart C
  data:
    source: main
  visualize:
    type: pie
```
"#;
        let result = parse_markdown_chartml(md);
        assert_eq!(result.charts.len(), 3);

        assert_eq!(result.charts[0].block_index, 0);
        assert_eq!(result.charts[0].array_index, 0);
        assert_eq!(result.charts[0].scope_key, "chart_0_0");
        assert_eq!(result.charts[0].spec["title"], "Chart A");

        assert_eq!(result.charts[1].block_index, 0);
        assert_eq!(result.charts[1].array_index, 1);
        assert_eq!(result.charts[1].scope_key, "chart_0_1");
        assert_eq!(result.charts[1].spec["title"], "Chart B");

        assert_eq!(result.charts[2].block_index, 0);
        assert_eq!(result.charts[2].array_index, 2);
        assert_eq!(result.charts[2].scope_key, "chart_0_2");
        assert_eq!(result.charts[2].spec["title"], "Chart C");
    }

    // 5. Multiple chartml blocks → correct block_index progression
    #[test]
    fn multiple_blocks_correct_indices() {
        let md = r#"```chartml
type: chart
version: 1
title: First
data:
  source: a
visualize:
  type: bar
```

Some text in between.

```chartml
type: chart
version: 1
title: Second
data:
  source: b
visualize:
  type: line
```

```chartml
type: chart
version: 1
title: Third
data:
  source: c
visualize:
  type: pie
```
"#;
        let result = parse_markdown_chartml(md);
        assert_eq!(result.charts.len(), 3);

        assert_eq!(result.charts[0].block_index, 0);
        assert_eq!(result.charts[0].spec["title"], "First");

        assert_eq!(result.charts[1].block_index, 1);
        assert_eq!(result.charts[1].spec["title"], "Second");

        assert_eq!(result.charts[2].block_index, 2);
        assert_eq!(result.charts[2].spec["title"], "Third");
    }

    // 6. Mixed types: source + params + chart + style + config
    #[test]
    fn mixed_component_types() {
        let md = r##"```chartml
type: source
version: 1
name: revenue_data
query: SELECT date, amount FROM revenue
datasource: main_db
```

```chartml
type: params
version: 1
params:
  - id: date_range
    type: daterange
    label: Date Range
    default: last_30_days
  - id: region
    type: select
    label: Region
    options:
      - US
      - EU
      - APAC
```

```chartml
type: style
version: 1
name: dark_theme
colors:
  - "#1a1a2e"
  - "#16213e"
background: "#0f0f23"
```

```chartml
type: config
version: 1
title: Revenue Dashboard
refresh_interval: 300
style: dark_theme
```

```chartml
type: chart
version: 1
title: Revenue Over Time
data:
  source: revenue_data
visualize:
  type: line
style: dark_theme
```
"##;
        let result = parse_markdown_chartml(md);

        // Source
        assert_eq!(result.sources.len(), 1);
        assert_eq!(result.sources[0].name, "revenue_data");
        assert_eq!(result.sources[0].definition["query"], "SELECT date, amount FROM revenue");
        // name, type, version should be stripped from definition
        assert!(result.sources[0].definition.get("name").is_none());
        assert!(result.sources[0].definition.get("type").is_none());
        assert!(result.sources[0].definition.get("version").is_none());

        // Params
        assert_eq!(result.params.len(), 1);
        assert_eq!(result.params[0].params.len(), 2);
        assert_eq!(result.params[0].params[0].id, "date_range");
        assert_eq!(result.params[0].params[0].param_type, "daterange");
        assert_eq!(result.params[0].params[0].label.as_deref(), Some("Date Range"));
        assert_eq!(result.params[0].params[1].id, "region");
        assert_eq!(result.params[0].params[1].param_type, "select");
        assert_eq!(result.params[0].params[1].options.as_ref().unwrap().len(), 3);
        assert_eq!(result.params[0].block_index, 1);

        // Style
        assert_eq!(result.styles.len(), 1);
        assert_eq!(result.styles[0].name, "dark_theme");
        assert!(result.styles[0].definition.get("name").is_none());
        assert!(result.styles[0].definition.get("type").is_none());

        // Config
        assert!(result.config.is_some());
        let cfg = result.config.as_ref().unwrap();
        assert_eq!(cfg["title"], "Revenue Dashboard");
        assert_eq!(cfg["refresh_interval"], 300);
        assert!(cfg.get("type").is_none());
        assert!(cfg.get("version").is_none());

        // Chart
        assert_eq!(result.charts.len(), 1);
        assert_eq!(result.charts[0].block_index, 4);
        assert_eq!(result.charts[0].spec["title"], "Revenue Over Time");
    }

    // 7. Parameter extraction with scope keys
    #[test]
    fn scoped_params_extraction() {
        let md = r#"```chartml
type: params
version: 1
params:
  - id: date_range
    type: daterange
    label: Date Range
  - id: metric
    type: select
    label: Metric
    options:
      - revenue
      - users
```
"#;
        let parsed = parse_markdown_chartml(md);
        let scoped = extract_scoped_params(&parsed);

        assert_eq!(scoped.len(), 2);
        // Dashboard-level params have empty scope_key.
        assert_eq!(scoped[0].scope_key, "");
        assert_eq!(scoped[0].param.id, "date_range");
        assert_eq!(scoped[1].scope_key, "");
        assert_eq!(scoped[1].param.id, "metric");
    }

    // 8. Nested YAML with complex chart specs
    #[test]
    fn nested_yaml_complex_chart() {
        let md = r#"```chartml
type: chart
version: 1
title: Complex Chart
data:
  source: analytics
  transforms:
    - type: filter
      field: status
      values:
        - active
        - pending
    - type: aggregate
      groupBy:
        - region
        - product
      measures:
        - field: revenue
          fn: sum
        - field: orders
          fn: count
visualize:
  type: bar
  x: region
  y: revenue
  color: product
  options:
    stacked: true
    legend:
      position: bottom
```
"#;
        let result = parse_markdown_chartml(md);
        assert_eq!(result.charts.len(), 1);

        let spec = &result.charts[0].spec;
        let transforms = spec["data"]["transforms"].as_array().unwrap();
        assert_eq!(transforms.len(), 2);
        assert_eq!(transforms[0]["type"], "filter");
        assert_eq!(transforms[1]["measures"].as_array().unwrap().len(), 2);
        assert_eq!(spec["visualize"]["options"]["stacked"], true);
        assert_eq!(spec["visualize"]["options"]["legend"]["position"], "bottom");
    }

    // 9. Non-chartml code blocks are ignored
    #[test]
    fn non_chartml_blocks_ignored() {
        let md = r#"# Code Examples

```sql
SELECT * FROM users;
```

```python
print("hello")
```

```javascript
console.log("hi");
```

```chartml
type: chart
version: 1
title: Only Chart
data:
  source: main
visualize:
  type: bar
```

```yaml
key: value
```
"#;
        let result = parse_markdown_chartml(md);
        assert_eq!(result.charts.len(), 1);
        assert_eq!(result.charts[0].spec["title"], "Only Chart");
        // Block index is 0 because it's the first *chartml* block.
        assert_eq!(result.charts[0].block_index, 0);
    }

    // 10. Malformed YAML in a chartml block is skipped gracefully
    #[test]
    fn malformed_yaml_skipped() {
        let md = r#"```chartml
this is not valid yaml: [
  unclosed bracket
```

```chartml
type: chart
version: 1
title: Valid Chart
data:
  source: main
visualize:
  type: bar
```
"#;
        let result = parse_markdown_chartml(md);
        // The first malformed block is skipped; the second is parsed.
        assert_eq!(result.charts.len(), 1);
        assert_eq!(result.charts[0].spec["title"], "Valid Chart");
        // Block index is 1 because the malformed block still counts.
        assert_eq!(result.charts[0].block_index, 1);
    }

    // Additional: component without type field is skipped
    #[test]
    fn component_without_type_skipped() {
        let md = r#"```chartml
name: orphan
query: SELECT 1
```
"#;
        let result = parse_markdown_chartml(md);
        assert!(result.charts.is_empty());
        assert!(result.sources.is_empty());
    }

    // Additional: source without name is skipped
    #[test]
    fn source_without_name_skipped() {
        let md = r#"```chartml
type: source
version: 1
query: SELECT 1
```
"#;
        let result = parse_markdown_chartml(md);
        assert!(result.sources.is_empty());
    }

    // Additional: params without params array is skipped
    #[test]
    fn params_without_array_skipped() {
        let md = r#"```chartml
type: params
version: 1
```
"#;
        let result = parse_markdown_chartml(md);
        assert!(result.params.is_empty());
    }

    // Additional: multiple configs — last one wins
    #[test]
    fn multiple_configs_last_wins() {
        let md = r#"```chartml
type: config
version: 1
title: First Config
```

```chartml
type: config
version: 1
title: Second Config
```
"#;
        let result = parse_markdown_chartml(md);
        assert!(result.config.is_some());
        assert_eq!(result.config.as_ref().unwrap()["title"], "Second Config");
    }

    // Additional: array block with mixed types
    #[test]
    fn array_block_mixed_types() {
        let md = r#"```chartml
- type: source
  version: 1
  name: src1
  query: SELECT 1
- type: chart
  version: 1
  title: Chart In Array
  data:
    source: src1
  visualize:
    type: bar
```
"#;
        let result = parse_markdown_chartml(md);
        assert_eq!(result.sources.len(), 1);
        assert_eq!(result.sources[0].name, "src1");
        assert_eq!(result.charts.len(), 1);
        assert_eq!(result.charts[0].spec["title"], "Chart In Array");
        // Both come from block 0.
        assert_eq!(result.charts[0].block_index, 0);
    }

    // Additional: JSON content in chartml block
    #[test]
    fn json_content_in_chartml_block() {
        let md = "```chartml\n{\"type\": \"chart\", \"version\": 1, \"title\": \"JSON Chart\", \"data\": {\"source\": \"x\"}, \"visualize\": {\"type\": \"bar\"}}\n```\n";
        let result = parse_markdown_chartml(md);
        assert_eq!(result.charts.len(), 1);
        assert_eq!(result.charts[0].spec["title"], "JSON Chart");
    }

    // Additional: extract_chartml_blocks helper
    #[test]
    fn extract_blocks_basic() {
        let md = "text\n```chartml\nfoo: bar\n```\nmore\n```chartml\nbaz: qux\n```\n";
        let blocks = extract_chartml_blocks(md);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].0, "foo: bar");
        assert_eq!(blocks[0].1, 0);
        assert_eq!(blocks[1].0, "baz: qux");
        assert_eq!(blocks[1].1, 1);
    }

    // Additional: param with layout
    #[test]
    fn param_with_layout() {
        let md = r#"```chartml
type: params
version: 1
params:
  - id: wide_param
    type: select
    label: Wide Select
    layout:
      col_span: 2
    options:
      - a
      - b
```
"#;
        let result = parse_markdown_chartml(md);
        assert_eq!(result.params.len(), 1);
        let param = &result.params[0].params[0];
        assert!(param.layout.is_some());
        assert_eq!(param.layout.as_ref().unwrap().col_span, Some(2));
    }
}
