// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tool for validating ChartML YAML blocks before the agent includes them in a response.

use async_trait::async_trait;
use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

pub struct ValidateChartmlTool;

#[async_trait]
impl AgentTool for ValidateChartmlTool {
    fn name(&self) -> &str {
        "validate_chartml"
    }

    fn description(&self) -> &str {
        "Validate ChartML YAML blocks before including them in your response. \
         Call this tool with the full chartml YAML content (without the ```chartml fences). \
         The tool checks YAML structure, required keys, and validates SQL queries \
         against the datasource via dry-run."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "blocks": {
                    "type": "array",
                    "description": "Array of ChartML YAML strings to validate (without ```chartml fences)",
                    "items": {
                        "type": "string"
                    }
                }
            },
            "required": ["blocks"]
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
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let blocks = args
            .get("blocks")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'blocks' (array)".into())
            })?;

        if blocks.is_empty() {
            return Ok(serde_json::json!({
                "valid": true,
                "blocks_checked": 0
            })
            .to_string());
        }

        let mut errors: Vec<serde_json::Value> = Vec::new();
        let query_ctx = ctx.query_context();

        for (i, block_value) in blocks.iter().enumerate() {
            let block_num = i + 1;
            let yaml_str = block_value.as_str().ok_or_else(|| {
                kyomi_core::Error::BadRequest(format!(
                    "Block {block_num}: expected string, got {block_value}"
                ))
            })?;

            let parsed: Result<serde_yaml::Value, _> = serde_yaml::from_str(yaml_str);
            match parsed {
                Ok(value) => {
                    let mapping = value.as_mapping();
                    let data_key = serde_yaml::Value::String("data".to_string());
                    let visualize_key = serde_yaml::Value::String("visualize".to_string());
                    let has_data = mapping
                        .map(|m| m.contains_key(&data_key))
                        .unwrap_or(false);
                    let has_visualize = mapping
                        .map(|m| m.contains_key(&visualize_key))
                        .unwrap_or(false);

                    if !has_data {
                        errors.push(serde_json::json!({
                            "block": block_num,
                            "type": "missing_key",
                            "message": "missing required key 'data'"
                        }));
                    }
                    if !has_visualize {
                        errors.push(serde_json::json!({
                            "block": block_num,
                            "type": "missing_key",
                            "message": "missing required key 'visualize'"
                        }));
                    }

                    if has_data {
                        let query = value
                            .get("data")
                            .and_then(|d| d.get("query"))
                            .and_then(|v| v.as_str());
                        let datasource = value
                            .get("data")
                            .and_then(|d| d.get("datasource"))
                            .and_then(|v| v.as_str());

                        if let (Some(sql), Some(slug)) = (query, datasource) {
                            match crate::tools::query_utils::dry_run_datasource_query(
                                &query_ctx, slug, sql,
                            )
                            .await
                            {
                                Ok(()) => {}
                                Err(e) => {
                                    if !e.starts_with("Failed to resolve")
                                        && !e.starts_with("Failed to create")
                                    {
                                        errors.push(serde_json::json!({
                                            "block": block_num,
                                            "type": "sql_error",
                                            "message": format!("SQL error: {e}")
                                        }));
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    errors.push(serde_json::json!({
                        "block": block_num,
                        "type": "yaml_parse",
                        "message": format!("invalid YAML: {e}")
                    }));
                }
            }
        }

        if errors.is_empty() {
            Ok(serde_json::json!({
                "valid": true,
                "blocks_checked": blocks.len()
            })
            .to_string())
        } else {
            Ok(serde_json::json!({
                "valid": false,
                "errors": errors
            })
            .to_string())
        }
    }
}
