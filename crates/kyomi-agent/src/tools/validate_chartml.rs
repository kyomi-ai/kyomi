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
         The tool validates the complete authoritative JSON Schema and every SQL source \
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

        let blocks: Vec<&str> = blocks.iter().map(|b| b.as_str().ok_or_else(|| kyomi_core::Error::BadRequest("Expected ChartML string".into()))).collect::<kyomi_core::Result<_>>()?;
        let errors = crate::tools::query_utils::validate_chartml_complete(&ctx.query_context(), &blocks).await;

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

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn tool_schema_and_sql_failures_have_stable_locations() {
        let ctx = crate::test_support::build_ctx(crate::test_support::test_pool().await);
        let input = "- type: config\n  version: 1\n- type: chart\n  version: 1\n  data: {provider: inline, rows: []}\n  visualize: {type: invalid}";
        let result: serde_json::Value = serde_json::from_str(&ValidateChartmlTool.execute(serde_json::json!({"blocks": [input]}), &ctx).await.unwrap()).unwrap();
        assert_eq!(result["valid"], false);
        assert!(result["errors"].as_array().unwrap().iter().any(|e| e["block"] == 1 && e["component"] == 2 && e["instance_path"] == "/1/visualize/type" && e["stage"] == "schema"));
        let result: serde_json::Value = serde_json::from_str(&ValidateChartmlTool.execute(serde_json::json!({"blocks": ["type: chart\nversion: 1\ndata: {datasource: absent, query: SELECT 1}\nvisualize: {type: table}"]}), &ctx).await.unwrap()).unwrap();
        assert_eq!(result["valid"], false);
        assert_eq!(result["errors"][0]["stage"], "sql_datasource");
        let result: serde_json::Value = serde_json::from_str(&ValidateChartmlTool.execute(serde_json::json!({"blocks": ["type: config\nversion: 1"]}), &ctx).await.unwrap()).unwrap();
        assert_eq!(result["valid"], true);
    }
}
