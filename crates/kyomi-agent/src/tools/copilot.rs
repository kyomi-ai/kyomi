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
// Shared validation-failure result
// ---------------------------------------------------------------------------

/// Build the structured tool result for a copilot input-validation failure.
///
/// Uses [`kyomi_core::Error::user_message`] rather than `Display` so the
/// variant tag (`bad request: `) never reaches the retry guidance shown to
/// the model or the text rendered to the user (KYO-389, same species as
/// KYO-380's `watch_execution_error_message`).
///
/// No `tracing` call here deliberately: unlike KYO-380, these are user-input
/// validation failures (the model supplied bad ChartML / a bad cron
/// expression / a bad mode), not system faults. They're already surfaced to
/// both the model and the user via this result, and the model retries.
/// Logging every rejected model draft would be noise, not signal.
fn validation_failure_result(headline: &str, e: &kyomi_core::Error) -> String {
    let error_message = e.user_message();
    serde_json::json!({
        "success": false,
        "validation_failed": true,
        "error_count": 1,
        "errors": [error_message],
        "message": format!("{headline}\n{error_message}"),
    })
    .to_string()
}

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
            return Ok(validation_failure_result(
                "ChartML validation failed. Fix these issues and try again:",
                &e,
            ));
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
// UpdateWatchCopilotTool
// ---------------------------------------------------------------------------

/// Push watch configuration drafts to the frontend via WebSocket.
///
/// Mirrors [`UpdateChartCopilotTool`] for the watch copilot: the agent calls
/// this tool with the fields it wants to change and the backend validates the
/// cron expression and mode, then broadcasts a [`MessageType::WatchUpdate`]
/// message that the watch form auto-applies as a draft. The user saves the
/// watch themselves — this tool does NOT persist to the database.
///
/// Distinct from the real `update_watch` tool ([`crate::tools::watch::UpdateWatchTool`]),
/// which writes directly to Postgres and is the one MCP and chat agents use.
pub struct UpdateWatchCopilotTool;

#[async_trait]
impl AgentTool for UpdateWatchCopilotTool {
    fn name(&self) -> &str {
        "update_watch_draft"
    }

    fn description(&self) -> &str {
        "Draft an update to the watch the user is editing in the modal. \
         Provide only the fields you want to change, plus a brief summary. \
         This tool pushes the draft into the user's open watch form — it \
         does NOT save the watch to the database. The user will click Save \
         in the modal when they're ready. The cron schedule is validated \
         before sending — fix any errors and retry."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Short, descriptive watch name"
                },
                "prompt": {
                    "type": "string",
                    "description": "Monitoring instruction for the watch agent"
                },
                "schedule": {
                    "type": "string",
                    "description": "Cron expression in UTC (5 fields: minute hour day-of-month month day-of-week)"
                },
                "mode": {
                    "type": "string",
                    "enum": ["alert", "report"],
                    "description": "'alert' = conditional monitoring, 'report' = scheduled summary every run"
                },
                "slack_channel_id": {
                    "type": "string",
                    "description": "Optional Slack channel ID for delivery"
                },
                "alert_emails": {
                    "type": "string",
                    "description": "Comma-separated list of email addresses for alerts"
                },
                "alert_emails_enabled": {
                    "type": "boolean",
                    "description": "Whether email alerts are enabled"
                },
                "queries": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "comment": {
                                "type": "string",
                                "description": "Why this query matters for the watch"
                            },
                            "sql": {
                                "type": "string",
                                "description": "SQL query text"
                            },
                            "datasource": {
                                "type": "string",
                                "description": "Datasource slug the query targets"
                            }
                        },
                        "required": ["comment", "sql", "datasource"]
                    },
                    "description": "Reference queries the monitoring agent can use"
                },
                "summary": {
                    "type": "string",
                    "description": "Brief explanation of what was changed"
                }
            },
            "required": ["summary"]
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
        let summary = args
            .get("summary")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'summary'".into(),
                )
            })?;

        // Validate cron expression if provided. Mirrors the ChartML validator
        // pattern in UpdateChartCopilotTool: on failure, return a structured
        // error result so the LLM can correct and retry rather than erroring
        // out the turn.
        if let Some(schedule) = args.get("schedule").and_then(|v| v.as_str())
            && let Err(e) = kyomi_auth::watch_service::parse_schedule(schedule)
        {
            return Ok(validation_failure_result(
                "Watch schedule validation failed. Fix the cron expression and try again:",
                &e,
            ));
        }

        // Validate mode if provided.
        if let Some(mode) = args.get("mode").and_then(|v| v.as_str())
            && let Err(e) = kyomi_auth::watch_service::validate_watch_mode(mode)
        {
            return Ok(validation_failure_result(
                "Watch mode validation failed. Fix the mode and try again:",
                &e,
            ));
        }

        // Build the data payload with every provided field. Summary and
        // context_type are always present; everything else is optional and
        // only forwarded when supplied so the frontend can merge partial
        // updates cleanly.
        let mut data = serde_json::Map::new();
        for field in [
            "name",
            "prompt",
            "schedule",
            "mode",
            "slack_channel_id",
            "alert_emails",
            "alert_emails_enabled",
            "queries",
        ] {
            if let Some(v) = args.get(field) {
                data.insert(field.to_string(), v.clone());
            }
        }
        data.insert("summary".to_string(), serde_json::Value::String(summary.to_string()));
        data.insert(
            "context_type".to_string(),
            serde_json::Value::String("watch_copilot".to_string()),
        );

        let mut msg = WebSocketMessage::new(MessageType::WatchUpdate)
            .with_data(serde_json::Value::Object(data));

        if let Some(ref sid) = ctx.session_id {
            msg = msg.with_session(sid);
        }

        ctx.ws_manager.send_to_user(&ctx.user_id, msg).await;

        Ok(serde_json::json!({
            "success": true,
            "message": "Watch configuration sent to user",
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

    // -- UpdateWatchCopilotTool ----------------------------------------------

    #[test]
    fn update_watch_copilot_name() {
        assert_eq!(UpdateWatchCopilotTool.name(), "update_watch_draft");
    }

    #[test]
    fn update_watch_copilot_description_not_empty() {
        assert!(!UpdateWatchCopilotTool.description().is_empty());
    }

    #[test]
    fn update_watch_copilot_is_copilot_only() {
        assert!(UpdateWatchCopilotTool.is_copilot_only());
    }

    #[test]
    fn update_watch_copilot_schema_requires_summary() {
        let schema = UpdateWatchCopilotTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("summary")));
        assert_eq!(required.len(), 1);
    }

    #[test]
    fn update_watch_copilot_schema_has_expected_optional_fields() {
        let schema = UpdateWatchCopilotTool.parameters_schema();
        let props = schema["properties"].as_object().expect("properties");
        for field in [
            "name",
            "prompt",
            "schedule",
            "mode",
            "slack_channel_id",
            "alert_emails",
            "alert_emails_enabled",
            "queries",
            "summary",
        ] {
            assert!(
                props.contains_key(field),
                "schema missing property '{field}'"
            );
        }
    }

    #[test]
    fn update_watch_copilot_annotations_not_read_only() {
        let ann = UpdateWatchCopilotTool
            .annotations()
            .expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(false));
        assert!(ann.destructive_hint.is_none());
    }

    // -- validation_failure_result (KYO-389) ---------------------------------
    //
    // The three copilot tool paths above all fail validation through
    // `kyomi_core::Error::BadRequest`, whose `Display` is `"bad request: {0}"`.
    // These tests drive `validation_failure_result` — the exact function each
    // of the three sites calls, with the exact headline literal each site
    // uses — through a real `BadRequest` and confirm the variant tag never
    // reaches either the `errors` array or the `message` field the model and
    // user see.

    #[test]
    fn validation_failure_result_chartml_strips_tag_and_keeps_detail() {
        let e = kyomi_core::Error::BadRequest(
            "line 4: unknown chart type 'foo-bar'".to_string(),
        );
        let result = validation_failure_result(
            "ChartML validation failed. Fix these issues and try again:",
            &e,
        );
        let json: serde_json::Value = serde_json::from_str(&result).expect("valid json");

        assert_eq!(
            json["errors"][0].as_str().unwrap(),
            "line 4: unknown chart type 'foo-bar'"
        );
        assert_eq!(
            json["message"].as_str().unwrap(),
            "ChartML validation failed. Fix these issues and try again:\n\
             line 4: unknown chart type 'foo-bar'"
        );
    }

    #[test]
    fn validation_failure_result_watch_schedule_strips_tag_and_keeps_detail() {
        let e = kyomi_core::Error::BadRequest(
            "cron expression must have 5 fields, got 3".to_string(),
        );
        let result = validation_failure_result(
            "Watch schedule validation failed. Fix the cron expression and try again:",
            &e,
        );
        let json: serde_json::Value = serde_json::from_str(&result).expect("valid json");

        assert_eq!(
            json["errors"][0].as_str().unwrap(),
            "cron expression must have 5 fields, got 3"
        );
        assert_eq!(
            json["message"].as_str().unwrap(),
            "Watch schedule validation failed. Fix the cron expression and try again:\n\
             cron expression must have 5 fields, got 3"
        );
    }

    #[test]
    fn validation_failure_result_watch_mode_strips_tag_and_keeps_detail() {
        let e = kyomi_core::Error::BadRequest(
            "mode must be 'alert' or 'report', got 'summary'".to_string(),
        );
        let result = validation_failure_result(
            "Watch mode validation failed. Fix the mode and try again:",
            &e,
        );
        let json: serde_json::Value = serde_json::from_str(&result).expect("valid json");

        assert_eq!(
            json["errors"][0].as_str().unwrap(),
            "mode must be 'alert' or 'report', got 'summary'"
        );
        assert_eq!(
            json["message"].as_str().unwrap(),
            "Watch mode validation failed. Fix the mode and try again:\n\
             mode must be 'alert' or 'report', got 'summary'"
        );
    }
}
