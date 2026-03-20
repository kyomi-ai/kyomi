// SPDX-License-Identifier: AGPL-3.0-or-later

//! Copilot tools — push dashboard and chart updates via WebSocket.
//!
//! These tools are used by dashboard and chart copilots to send real-time
//! content updates to the frontend. They are only available in copilot mode
//! (`is_copilot_only() -> true`).

use async_trait::async_trait;
use kyomi_core::{MessageType, WebSocketMessage};

use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

// ---------------------------------------------------------------------------
// UpdateDashboardCopilotTool
// ---------------------------------------------------------------------------

/// Push dashboard content updates to the frontend via WebSocket.
pub struct UpdateDashboardCopilotTool;

#[async_trait]
impl AgentTool for UpdateDashboardCopilotTool {
    fn name(&self) -> &str {
        "update_dashboard"
    }

    fn description(&self) -> &str {
        "Update the dashboard content. Use this to apply changes to the \
         dashboard the user is editing. You MUST provide the COMPLETE updated \
         markdown content and a brief summary of changes."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The complete updated dashboard markdown content"
                },
                "summary": {
                    "type": "string",
                    "description": "Brief explanation of what was changed"
                }
            },
            "required": ["content", "summary"]
        })
    }

    fn is_copilot_only(&self) -> bool {
        true
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            read_only_hint: Some(false),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'content'".into(),
                )
            })?;
        let summary = args
            .get("summary")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'summary'".into(),
                )
            })?;

        let mut msg = WebSocketMessage::new(MessageType::DashboardUpdate)
            .with_data(serde_json::json!({
                "content": content,
                "summary": summary,
                "context_type": "dashboard_copilot",
            }));

        if let Some(ref sid) = ctx.session_id {
            msg = msg.with_session(sid);
        }

        ctx.ws_manager.send_to_user(&ctx.user_id, msg).await;

        Ok(serde_json::json!({
            "success": true,
            "message": "Dashboard content sent to user",
        })
        .to_string())
    }
}

// ---------------------------------------------------------------------------
// UpdateChartCopilotTool
// ---------------------------------------------------------------------------

/// Push chart content updates to the frontend via WebSocket.
///
/// Validates ChartML content before sending. If validation fails, returns
/// errors so the LLM can fix and retry.
pub struct UpdateChartCopilotTool;

#[async_trait]
impl AgentTool for UpdateChartCopilotTool {
    fn name(&self) -> &str {
        "update_chart"
    }

    fn description(&self) -> &str {
        "Update the chart configuration. Use this to apply changes to the \
         chart the user is editing. You MUST provide the COMPLETE updated \
         ChartML YAML and a brief summary of changes. ChartML is validated \
         before sending — fix any errors and retry."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The complete updated ChartML YAML content"
                },
                "summary": {
                    "type": "string",
                    "description": "Brief explanation of what was changed"
                }
            },
            "required": ["content", "summary"]
        })
    }

    fn is_copilot_only(&self) -> bool {
        true
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            read_only_hint: Some(false),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'content'".into(),
                )
            })?;
        let summary = args
            .get("summary")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'summary'".into(),
                )
            })?;

        // Validate ChartML content — wrap in a fenced block for the validator
        let fenced = format!("```chartml\n{content}\n```");
        if let Err(e) = kyomi_auth::dashboard_service::validate_dashboard_content(&fenced) {
            let error_message = e.to_string();
            return Ok(serde_json::json!({
                "success": false,
                "validation_failed": true,
                "error_count": 1,
                "errors": [error_message],
                "message": format!(
                    "ChartML validation failed. Fix these issues and try again:\n{error_message}"
                ),
            })
            .to_string());
        }

        let mut msg = WebSocketMessage::new(MessageType::ChartUpdate)
            .with_data(serde_json::json!({
                "content": content,
                "summary": summary,
                "context_type": "chart_builder_copilot",
            }));

        if let Some(ref sid) = ctx.session_id {
            msg = msg.with_session(sid);
        }

        ctx.ws_manager.send_to_user(&ctx.user_id, msg).await;

        Ok(serde_json::json!({
            "success": true,
            "message": "Chart content sent to user",
        })
        .to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- UpdateDashboardCopilotTool ------------------------------------------

    #[test]
    fn update_dashboard_copilot_name() {
        assert_eq!(UpdateDashboardCopilotTool.name(), "update_dashboard");
    }

    #[test]
    fn update_dashboard_copilot_description_not_empty() {
        assert!(!UpdateDashboardCopilotTool.description().is_empty());
    }

    #[test]
    fn update_dashboard_copilot_is_copilot_only() {
        assert!(UpdateDashboardCopilotTool.is_copilot_only());
    }

    #[test]
    fn update_dashboard_copilot_schema_requires_content_and_summary() {
        let schema = UpdateDashboardCopilotTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("content")));
        assert!(required.contains(&serde_json::json!("summary")));
        assert_eq!(required.len(), 2);
    }

    #[test]
    fn update_dashboard_copilot_annotations_not_read_only() {
        let ann = UpdateDashboardCopilotTool
            .annotations()
            .expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(false));
        assert!(ann.destructive_hint.is_none());
    }

    // -- UpdateChartCopilotTool ----------------------------------------------

    #[test]
    fn update_chart_copilot_name() {
        assert_eq!(UpdateChartCopilotTool.name(), "update_chart");
    }

    #[test]
    fn update_chart_copilot_description_not_empty() {
        assert!(!UpdateChartCopilotTool.description().is_empty());
    }

    #[test]
    fn update_chart_copilot_is_copilot_only() {
        assert!(UpdateChartCopilotTool.is_copilot_only());
    }

    #[test]
    fn update_chart_copilot_schema_requires_content_and_summary() {
        let schema = UpdateChartCopilotTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("content")));
        assert!(required.contains(&serde_json::json!("summary")));
        assert_eq!(required.len(), 2);
    }

    #[test]
    fn update_chart_copilot_annotations_not_read_only() {
        let ann = UpdateChartCopilotTool
            .annotations()
            .expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(false));
        assert!(ann.destructive_hint.is_none());
    }
}
