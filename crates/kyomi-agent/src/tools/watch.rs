// SPDX-License-Identifier: AGPL-3.0-or-later

//! Watch tools — create, preview, update, search, delete, get info, and trigger watches.

use async_trait::async_trait;

use kyomi_auth::websocket::helpers as ws_helpers;

use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

// ---------------------------------------------------------------------------
// CreateWatchTool
// ---------------------------------------------------------------------------

/// Create a new watch for automated data monitoring.
pub struct CreateWatchTool;

#[async_trait]
impl AgentTool for CreateWatchTool {
    fn name(&self) -> &str {
        "create_watch"
    }

    fn description(&self) -> &str {
        "Create a Kyomi Watch - automated data monitoring that runs on a schedule.\n\n\
         **Two Modes (REQUIRED - you must choose one):**\n\n\
         1. **Alert Mode** (mode=\"alert\") - Conditional monitoring\n\
         2. **Report Mode** (mode=\"report\") - Scheduled reports\n\n\
         **Workflow:**\n\
         1. Use search_watches to check for duplicates first\n\
         2. Explore data schema\n\
         3. Set verified_no_duplicates=true after checking\n\n\
         **Schedule:** Cron expression in UTC (5 fields: min hour day month weekday)\n\n\
         NOTE: Premium feature (Pro/Team plans)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Short descriptive name for the watch (e.g., 'Daily Sales Monitor')"
                },
                "prompt": {
                    "type": "string",
                    "description": "The monitoring instruction - what to check and when to alert."
                },
                "schedule": {
                    "type": "string",
                    "description": "Cron expression in UTC (5 fields: minute hour day-of-month month day-of-week). Examples: '0 9 * * *' (daily 9am UTC), '0 15 * * 1-5' (weekdays 3pm UTC)."
                },
                "mode": {
                    "type": "string",
                    "enum": ["alert", "report"],
                    "description": "REQUIRED: Watch mode. 'alert' for conditional monitoring, 'report' for scheduled summaries."
                },
                "verified_no_duplicates": {
                    "type": "boolean",
                    "description": "Set to true after checking for duplicates with search_watches. Must be true to create."
                },
                "queries": {
                    "type": "array",
                    "description": "Optional reference queries for the monitoring agent.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "comment": { "type": "string" },
                            "sql": { "type": "string" },
                            "datasource": { "type": "string" }
                        },
                        "required": ["comment", "sql"]
                    }
                },
                "datasource_hints": {
                    "type": "object",
                    "description": "Optional hints about which datasources to query.",
                    "properties": {
                        "datasources": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    }
                },
                "alert_emails": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Email addresses to send alerts to."
                },
                "alert_emails_enabled": {
                    "type": "boolean",
                    "description": "Enable email alerts. Default: false.",
                    "default": false
                }
            },
            "required": ["name", "prompt", "schedule", "mode", "verified_no_duplicates"]
        })
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
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'name'".into())
            })?;
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'prompt'".into())
            })?;
        let schedule = args
            .get("schedule")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'schedule'".into())
            })?;
        let mode = args
            .get("mode")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'mode'".into())
            })?;
        let verified = args
            .get("verified_no_duplicates")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !verified {
            return Ok(serde_json::json!({
                "error": "You must confirm that you've searched for existing watches \
                          before creating a new one. Use the search_watches tool first \
                          to check for duplicates, then set verified_no_duplicates=true."
            })
            .to_string());
        }

        let queries = args.get("queries").cloned();

        // Validate each reference query's SQL via dry-run before saving.
        if let Some(serde_json::Value::Array(ref query_list)) = queries {
            let query_ctx = ctx.query_context();
            let mut validation_errors: Vec<serde_json::Value> = Vec::new();

            for (i, q) in query_list.iter().enumerate() {
                let sql = match q.get("sql").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => continue, // no SQL to validate
                };
                let datasource_slug = match q.get("datasource").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => continue, // no datasource — can't validate
                };
                let comment = q.get("comment").and_then(|v| v.as_str()).unwrap_or("");

                if let Err(err) = super::query_utils::dry_run_datasource_query(
                    &query_ctx,
                    datasource_slug,
                    sql,
                )
                .await
                {
                    validation_errors.push(serde_json::json!({
                        "query_index": i,
                        "comment": comment,
                        "datasource": datasource_slug,
                        "error": err,
                    }));
                }
            }

            if !validation_errors.is_empty() {
                return Ok(serde_json::json!({
                    "success": false,
                    "error": "One or more reference queries have invalid SQL. Fix the SQL and try again.",
                    "validation_errors": validation_errors,
                })
                .to_string());
            }
        }

        let datasource_hints = args.get("datasource_hints").cloned();
        let alert_emails_list = args
            .get("alert_emails")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| e.as_str())
                    .collect::<Vec<&str>>()
                    .join(",")
            });
        let alert_emails_enabled = args
            .get("alert_emails_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let watch = match kyomi_auth::watch_service::create_watch(
            &ctx.db,
            &ctx.workspace_id,
            &ctx.user_id,
            name,
            prompt,
            schedule,
            mode,
            queries.as_ref(),
            datasource_hints.as_ref(),
            alert_emails_list.as_deref(),
            alert_emails_enabled,
        )
        .await
        {
            Ok(w) => w,
            Err(kyomi_core::Error::Forbidden(msg)) => {
                return Ok(serde_json::json!({
                    "success": false,
                    "error": msg,
                    "upgrade_required": true,
                })
                .to_string());
            }
            Err(kyomi_core::Error::Conflict(msg)) => {
                return Ok(serde_json::json!({
                    "success": false,
                    "error": msg,
                })
                .to_string());
            }
            Err(kyomi_core::Error::BadRequest(msg)) => {
                return Ok(serde_json::json!({
                    "success": false,
                    "error": msg,
                })
                .to_string());
            }
            Err(e) => return Err(e),
        };

        // Watches are strictly private to their creator (KYO-177/KYO-180) —
        // route the live-sync broadcast to the owner only. `send_to_user` fans
        // out to all of the owner's own connections, so same-user multi-tab
        // sync still works; it is only *other* workspace members who no
        // longer receive the event.
        ws_helpers::broadcast_watch_sync(
            &ctx.db,
            &ctx.ws_manager,
            &watch.watch_id,
            &ctx.workspace_id,
            kyomi_types::sync::SyncActionType::Insert,
            &watch.created_by,
        )
        .await;

        let schedule_description =
            kyomi_auth::watch_service::describe_cron(&watch.schedule);
        let next_run_display = watch
            .next_run_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_default();

        Ok(serde_json::json!({
            "success": true,
            "watch_id": watch.watch_id,
            "name": watch.name,
            "schedule": watch.schedule,
            "schedule_description": schedule_description,
            "next_run_at": next_run_display,
            "mode": watch.mode,
            "message": format!(
                "Created watch '{}'. Next run: {}.",
                watch.name, schedule_description
            ),
        })
        .to_string())
    }
}

// ---------------------------------------------------------------------------
// PreviewWatchTool
// ---------------------------------------------------------------------------

/// Preview a watch configuration without creating or updating it.
pub struct PreviewWatchTool;

#[async_trait]
impl AgentTool for PreviewWatchTool {
    fn name(&self) -> &str {
        "preview_watch"
    }

    fn description(&self) -> &str {
        "Preview a watch configuration before creating or updating it. \
         Validates the cron schedule and returns a human-readable summary. \
         This tool is only available in copilot mode."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Short descriptive name for the watch"
                },
                "prompt": {
                    "type": "string",
                    "description": "The monitoring instruction"
                },
                "schedule": {
                    "type": "string",
                    "description": "Cron expression in UTC (5 fields)"
                },
                "watch_id": {
                    "type": "string",
                    "description": "For updates only: the existing watch ID"
                }
            },
            "required": ["name", "prompt", "schedule"]
        })
    }

    fn is_copilot_only(&self) -> bool {
        true
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
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'name'".into())
            })?;
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'prompt'".into())
            })?;
        let schedule = args
            .get("schedule")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'schedule'".into())
            })?;
        let watch_id = args.get("watch_id").and_then(|v| v.as_str());

        // Validate cron schedule
        if let Err(e) = kyomi_auth::watch_service::parse_schedule(schedule) {
            return Ok(serde_json::json!({
                "success": false,
                "error": e.to_string(),
            })
            .to_string());
        }

        let schedule_description =
            kyomi_auth::watch_service::describe_cron(schedule.trim());
        let next_run_at = kyomi_auth::watch_service::calculate_next_run(schedule.trim())
            .map(|t| t.to_rfc3339())
            .unwrap_or_default();

        let mut result = serde_json::json!({
            "success": true,
            "name": name.trim(),
            "prompt": prompt.trim(),
            "schedule": schedule.trim(),
            "schedule_description": schedule_description,
            "next_run_at": next_run_at,
        });

        if let Some(wid) = watch_id {
            result["watch_id"] = serde_json::json!(wid);
        }

        Ok(result.to_string())
    }
}

// ---------------------------------------------------------------------------
// UpdateWatchTool
// ---------------------------------------------------------------------------

/// Update an existing watch's configuration.
pub struct UpdateWatchTool;

#[async_trait]
impl AgentTool for UpdateWatchTool {
    fn name(&self) -> &str {
        "update_watch"
    }

    fn description(&self) -> &str {
        "Update an existing Kyomi Watch. You can change name, prompt, schedule, \
         mode, enabled status, Slack channel, or email alert settings. Only include \
         parameters you want to change."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "watch_id": {
                    "type": "string",
                    "description": "The ID of the watch to update"
                },
                "name": {
                    "type": "string",
                    "description": "New name for the watch (optional)"
                },
                "prompt": {
                    "type": "string",
                    "description": "New monitoring instruction (optional)"
                },
                "schedule": {
                    "type": "string",
                    "description": "New cron schedule (optional)"
                },
                "mode": {
                    "type": "string",
                    "enum": ["alert", "report"],
                    "description": "Change watch mode (optional)"
                },
                "enabled": {
                    "type": "boolean",
                    "description": "Enable or disable the watch (optional)"
                },
                "alert_emails": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Email addresses for alerts"
                },
                "alert_emails_enabled": {
                    "type": "boolean",
                    "description": "Enable or disable email alerts"
                },
                "queries": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "comment": { "type": "string" },
                            "sql": { "type": "string" },
                            "datasource": { "type": "string" }
                        },
                        "required": ["comment", "sql"]
                    },
                    "description": "Optional reference queries for the monitoring agent."
                }
            },
            "required": ["watch_id"]
        })
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
        let watch_id = args
            .get("watch_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'watch_id'".into(),
                )
            })?;

        let alert_emails_str = args
            .get("alert_emails")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| e.as_str())
                    .collect::<Vec<&str>>()
                    .join(",")
            });

        let updates = kyomi_auth::watch_service::WatchUpdate {
            name: args
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            prompt: args
                .get("prompt")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            schedule: args
                .get("schedule")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            mode: args
                .get("mode")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            enabled: args.get("enabled").and_then(|v| v.as_bool()),
            alert_emails: alert_emails_str,
            alert_emails_enabled: args
                .get("alert_emails_enabled")
                .and_then(|v| v.as_bool()),
            queries: args
                .get("queries")
                .and_then(|v| v.as_array())
                .map(|arr| serde_json::Value::Array(arr.clone())),
            datasource_hints: None,
        };

        let watch = match kyomi_auth::watch_service::update_watch(
            &ctx.db,
            watch_id,
            &ctx.workspace_id,
            &ctx.user_id,
            &updates,
        )
        .await
        {
            Ok(w) => w,
            Err(kyomi_core::Error::NotFound(msg)) => {
                return Ok(serde_json::json!({ "error": msg }).to_string());
            }
            Err(kyomi_core::Error::BadRequest(msg)) => {
                return Ok(serde_json::json!({
                    "success": false,
                    "error": msg,
                })
                .to_string());
            }
            Err(e) => return Err(e),
        };

        // Watches are strictly private to their creator (KYO-177/KYO-180) —
        // route the live-sync broadcast to the owner only. `send_to_user` fans
        // out to all of the owner's own connections, so same-user multi-tab
        // sync still works; it is only *other* workspace members who no
        // longer receive the event.
        ws_helpers::broadcast_watch_sync(
            &ctx.db,
            &ctx.ws_manager,
            &watch.watch_id,
            &ctx.workspace_id,
            kyomi_types::sync::SyncActionType::Update,
            &watch.created_by,
        )
        .await;

        let schedule_description =
            kyomi_auth::watch_service::describe_cron(&watch.schedule);
        let next_run_display = watch
            .next_run_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_default();

        let message = if watch.enabled {
            format!("Updated watch '{}'. Next run: {}.", watch.name, schedule_description)
        } else {
            format!("Updated watch '{}'. Watch is disabled.", watch.name)
        };

        Ok(serde_json::json!({
            "success": true,
            "watch_id": watch.watch_id,
            "name": watch.name,
            "schedule": watch.schedule,
            "schedule_description": schedule_description,
            "enabled": watch.enabled,
            "next_run_at": next_run_display,
            "mode": watch.mode,
            "message": message,
        })
        .to_string())
    }
}

// ---------------------------------------------------------------------------
// SearchWatchesTool
// ---------------------------------------------------------------------------

/// Search for existing watches in the workspace.
pub struct SearchWatchesTool;

#[async_trait]
impl AgentTool for SearchWatchesTool {
    fn name(&self) -> &str {
        "search_watches"
    }

    fn description(&self) -> &str {
        "Search for existing watches to avoid duplicates or find watches to modify. \
         Leave the query empty to list all watches. Returns matching watches with \
         full details (name, prompt, schedule, mode, queries, status, last execution)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search term to match against watch names and prompts. Leave empty to list all."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10)",
                    "default": 10
                },
                "timezone_offset": {
                    "type": "string",
                    "description": "User's timezone offset in ISO format (e.g., '+11:00', '-05:00') for schedule display."
                }
            },
            "required": []
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
        let query = args.get("query").and_then(|v| v.as_str());
        let limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(10);
        let _timezone_offset = args
            .get("timezone_offset")
            .and_then(|v| v.as_str());

        let results = kyomi_auth::watch_service::search_watches(
            &ctx.db,
            &ctx.workspace_id,
            query,
            limit,
        )
        .await?;

        // Get total workspace watch count
        let all_watches =
            kyomi_auth::watch_service::list_watches(&ctx.db, &ctx.workspace_id).await?;
        let total_workspace_watches = all_watches.len();

        let watches: Vec<serde_json::Value> = results
            .iter()
            .map(|w| {
                let schedule_display =
                    kyomi_auth::watch_service::describe_cron(&w.schedule);
                let prompt_truncated = if w.prompt.len() > 200 {
                    format!("{}...", &w.prompt[..200])
                } else {
                    w.prompt.clone()
                };

                serde_json::json!({
                    "watch_id": w.watch_id,
                    "name": w.name,
                    "prompt": prompt_truncated,
                    "schedule": w.schedule,
                    "schedule_display": schedule_display,
                    "mode": w.mode,
                    "enabled": w.enabled,
                    "queries": w.queries,
                    "last_run_at": w.last_run_at.map(|t| t.to_rfc3339()),
                    "last_run_status": w.last_run_status,
                    "next_run_at": w.next_run_at.map(|t| t.to_rfc3339()),
                    "created_at": w.created_at.to_rfc3339(),
                })
            })
            .collect();

        let count = watches.len();

        Ok(serde_json::json!({
            "watches": watches,
            "count": count,
            "total_workspace_watches": total_workspace_watches,
        })
        .to_string())
    }
}

// ---------------------------------------------------------------------------
// DeleteWatchTool
// ---------------------------------------------------------------------------

/// Permanently delete a watch.
pub struct DeleteWatchTool;

#[async_trait]
impl AgentTool for DeleteWatchTool {
    fn name(&self) -> &str {
        "delete_watch"
    }

    fn description(&self) -> &str {
        "Permanently delete a watch. This action cannot be undone. \
         Use update_watch to temporarily disable a watch instead."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "watch_id": {
                    "type": "string",
                    "description": "The ID of the watch to delete"
                }
            },
            "required": ["watch_id"]
        })
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            destructive_hint: Some(true),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let watch_id = args
            .get("watch_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'watch_id'".into(),
                )
            })?;

        match kyomi_auth::watch_service::delete_watch(
            &ctx.db,
            watch_id,
            &ctx.workspace_id,
            &ctx.user_id,
        )
        .await
        {
            Ok(created_by) => {
                // Watches are strictly private to their creator (KYO-177/KYO-180) —
                // route the live-sync broadcast to the owner only. `send_to_user`
                // fans out to all of the owner's own connections, so same-user
                // multi-tab sync still works; it is only *other* workspace
                // members who no longer receive the event.
                ws_helpers::broadcast_watch_sync(
                    &ctx.db,
                    &ctx.ws_manager,
                    watch_id,
                    &ctx.workspace_id,
                    kyomi_types::sync::SyncActionType::Delete,
                    &created_by,
                )
                .await;

                Ok(serde_json::json!({
                    "success": true,
                    "watch_id": watch_id,
                    "message": format!("Deleted watch '{watch_id}'"),
                })
                .to_string())
            }
            Err(kyomi_core::Error::NotFound(msg)) => {
                Ok(serde_json::json!({ "error": msg }).to_string())
            }
            Err(e) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// GetWatchInfoTool
// ---------------------------------------------------------------------------

/// Get detailed information about a watch including recent execution history.
pub struct GetWatchInfoTool;

#[async_trait]
impl AgentTool for GetWatchInfoTool {
    fn name(&self) -> &str {
        "get_watch_info"
    }

    fn description(&self) -> &str {
        "Get detailed information for a specific watch including recent execution \
         history. Use this to view configuration, debug watch behavior, check why \
         a watch didn't alert, or verify it is running on schedule."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "watch_id": {
                    "type": "string",
                    "description": "The watch ID to retrieve (e.g., 'watch-abc123')"
                }
            },
            "required": ["watch_id"]
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
        let watch_id = args
            .get("watch_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'watch_id'".into(),
                )
            })?;

        let watch = kyomi_auth::watch_service::get_watch(
            &ctx.db,
            watch_id,
            &ctx.workspace_id,
            &ctx.user_id,
        )
        .await?;

        let watch = match watch {
            Some(w) => w,
            None => {
                return Ok(serde_json::json!({
                    "error": format!("Watch not found: {watch_id}")
                })
                .to_string());
            }
        };

        let executions = kyomi_auth::watch_service::get_executions(
            &ctx.db,
            watch_id,
            &ctx.workspace_id,
            &ctx.user_id,
            5,
        )
        .await?;

        let schedule_description =
            kyomi_auth::watch_service::describe_cron(&watch.schedule);

        let execution_list: Vec<serde_json::Value> = executions
            .iter()
            .map(|exec| {
                // Extract alert_title from execution_trace if present
                let alert_title = exec
                    .execution_trace
                    .as_ref()
                    .and_then(|trace| trace.get("title"))
                    .and_then(|t| t.as_str())
                    .or_else(|| {
                        exec.execution_trace
                            .as_ref()
                            .and_then(|trace| trace.get("alert_title"))
                            .and_then(|t| t.as_str())
                    });

                // Truncate agent_response to 500 chars
                let agent_response = exec.agent_response.as_deref().map(|r| {
                    if r.len() > 500 {
                        format!("{}...", &r[..500])
                    } else {
                        r.to_string()
                    }
                });

                serde_json::json!({
                    "timestamp": exec.started_at.to_rfc3339(),
                    "status": exec.status,
                    "alert_triggered": exec.alert_triggered,
                    "alert_title": alert_title,
                    "agent_response": agent_response,
                    "error_message": exec.error_message,
                })
            })
            .collect();

        let execution_count = execution_list.len();

        Ok(serde_json::json!({
            "watch": {
                "watch_id": watch.watch_id,
                "name": watch.name,
                "prompt": watch.prompt,
                "schedule": watch.schedule,
                "schedule_description": schedule_description,
                "mode": watch.mode,
                "enabled": watch.enabled,
                "last_run_at": watch.last_run_at.map(|t| t.to_rfc3339()),
                "last_run_status": watch.last_run_status,
                "next_run_at": watch.next_run_at.map(|t| t.to_rfc3339()),
                "queries": watch.queries,
                "alert_emails": watch.alert_emails,
                "alert_emails_enabled": watch.alert_emails_enabled,
                "created_at": watch.created_at.to_rfc3339(),
                "updated_at": watch.updated_at.to_rfc3339(),
            },
            "recent_executions": execution_list,
            "execution_count": execution_count,
        })
        .to_string())
    }
}

// ---------------------------------------------------------------------------
// TriggerWatchTool
// ---------------------------------------------------------------------------

/// Manually trigger a watch to run immediately.
pub struct TriggerWatchTool;

#[async_trait]
impl AgentTool for TriggerWatchTool {
    fn name(&self) -> &str {
        "trigger_watch"
    }

    fn description(&self) -> &str {
        "Manually trigger a watch to run immediately. The watch must be enabled. \
         Use get_watch_info afterwards to see the results."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "watch_id": {
                    "type": "string",
                    "description": "The ID of the watch to trigger"
                }
            },
            "required": ["watch_id"]
        })
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
        let watch_id = args
            .get("watch_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Missing required parameter 'watch_id'".into(),
                )
            })?;

        // Check watch exists and is enabled
        let watch = kyomi_auth::watch_service::get_watch(
            &ctx.db,
            watch_id,
            &ctx.workspace_id,
            &ctx.user_id,
        )
        .await?;

        let watch = match watch {
            Some(w) => w,
            None => {
                return Ok(serde_json::json!({
                    "error": format!("Watch not found: {watch_id}")
                })
                .to_string());
            }
        };

        if !watch.enabled {
            return Ok(serde_json::json!({
                "error": format!(
                    "Watch '{}' is disabled. Enable it first with update_watch.",
                    watch.name
                )
            })
            .to_string());
        }

        // Check rate limiting
        let (can_run, reason) = kyomi_auth::watch_service::can_run_watch_now(
            &ctx.db,
            watch_id,
            &ctx.workspace_id,
            &ctx.user_id,
        )
        .await?;

        if !can_run {
            return Ok(serde_json::json!({
                "error": reason,
            })
            .to_string());
        }

        // Resolve embedding service (required for watch execution).
        let embedding = ctx.embedding.get().map_err(|e| {
            kyomi_core::Error::ServiceUnavailable(format!("Embedding model not ready: {e}"))
        })?.clone();

        // Spawn background execution — same as the HTTP trigger endpoint.
        // `execute_watch` creates the execution record internally.
        let bg_watch_id = watch.watch_id.clone();
        let bg_db = ctx.db.clone();
        let bg_kv = ctx.kv.clone();
        let bg_encryption_key = ctx.encryption_key.clone();
        let bg_ws_manager = ctx.ws_manager.clone();
        let bg_config = ctx.config.clone();
        let bg_connect = ctx.connect_registry.clone();
        let bg_platforms = ctx.platforms.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::watch_execution::execute_watch(
                &bg_db,
                &bg_kv,
                &bg_encryption_key,
                &embedding,
                &bg_ws_manager,
                &bg_config,
                bg_connect,
                &bg_platforms,
                &bg_watch_id,
            )
            .await
            {
                tracing::error!(watch_id = %bg_watch_id, error = %e, "Watch execution failed");
            }
        });

        tracing::info!(
            watch_id = %watch.watch_id,
            "Watch manually triggered"
        );

        Ok(serde_json::json!({
            "success": true,
            "watch_id": watch.watch_id,
            "name": watch.name,
            "message": format!(
                "Watch '{}' has been triggered. Results will appear in your alerts.",
                watch.name
            ),
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

    // ── Live broadcast routing (KYO-180) ────────────────────────────────
    //
    // KYO-177 made watches strictly private to their creator and fixed the
    // UI server-fn path (`crates/kyomi-ui/src/server_fns/watches.rs`) to
    // route live-sync broadcasts to the owner only via
    // `broadcast_watch_sync` / `send_to_user`. The AI-agent tool path here
    // was missed and kept calling the old `send_watch_update` helper, which
    // broadcasts to every connected workspace member — leaking a private
    // watch's content to bystanders whenever a watch is created, updated,
    // or deleted through chat instead of the Watches page. These tests
    // exercise the real tool `execute()` path end-to-end (real sqlite DB,
    // real `WebSocketManager`) and assert the non-owner receives nothing.

    mod broadcast_routing {
        use std::sync::Arc;

        use sqlx::sqlite::SqlitePoolOptions;

        use kyomi_auth::websocket::WebSocketManager;
        use kyomi_core::DbPool;

        use super::*;

        async fn test_pool() -> DbPool {
            let _ = kyomi_core::constants::load_with_fallback();

            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .expect("connect in-memory sqlite");

            sqlx::query("PRAGMA foreign_keys=ON")
                .execute(&pool)
                .await
                .expect("enable foreign keys");

            sqlx::migrate!("../../apps/server/migrations-sqlite")
                .run(&pool)
                .await
                .expect("run sqlite migrations");

            DbPool::Sqlite(pool)
        }

        /// Seed two users ("user-a", "user-b") in one workspace ("ws-1",
        /// owned by user-a). Mirrors the fixture in
        /// `kyomi_auth::watch_service::privacy_tests`.
        async fn seed_workspace_with_two_users(pool: &DbPool) {
            let sq = match pool {
                DbPool::Sqlite(sq) => sq,
                _ => unreachable!(),
            };

            sqlx::query("INSERT INTO users (user_id, email) VALUES ('user-a', 'a@test.local')")
                .execute(sq)
                .await
                .expect("insert user-a");
            sqlx::query("INSERT INTO users (user_id, email) VALUES ('user-b', 'b@test.local')")
                .execute(sq)
                .await
                .expect("insert user-b");

            sqlx::query(
                "INSERT INTO workspaces (workspace_id, name, owner_user_id) \
                 VALUES ('ws-1', 'Shared Workspace', 'user-a')",
            )
            .execute(sq)
            .await
            .expect("insert workspace");

            sqlx::query(
                "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
                 VALUES ('ws-1', 'user-a', 'workspace_admin', 1)",
            )
            .execute(sq)
            .await
            .expect("insert workspace_users user-a");
            sqlx::query(
                "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
                 VALUES ('ws-1', 'user-b', 'user', 1)",
            )
            .execute(sq)
            .await
            .expect("insert workspace_users user-b");
        }

        /// Build a `ToolContext` for user-a acting in ws-1, wired to the
        /// given (real) `WebSocketManager` and (real) sqlite pool so the
        /// tool's broadcast call routes through actual connections.
        fn build_ctx(pool: DbPool, ws_manager: &WebSocketManager) -> ToolContext {
            ToolContext {
                db: pool,
                kv: kyomi_core::kv_store_memory::InMemoryKVStore::new_pool(),
                user_id: "user-a".to_string(),
                workspace_id: "ws-1".to_string(),
                encryption_key: Arc::new([0u8; 32]),
                embedding: kyomi_embed::LazyEmbedding::new(),
                ws_manager: ws_manager.clone(),
                config: Arc::new(kyomi_core::Config::test_config()),
                session_id: None,
                supports_mcp_apps: false,
                workspace_roles: Vec::new(),
                connect_registry: None,
                platforms: Arc::new(kyomi_core::platform::PlatformRegistry::new()),
                user_display_name: "User A".to_string(),
            }
        }

        #[tokio::test]
        async fn create_watch_tool_broadcasts_to_owner_only() {
            let pool = test_pool().await;
            seed_workspace_with_two_users(&pool).await;

            let manager = WebSocketManager::new(None, pool.clone());
            let (_conn_a, mut rx_a) = manager.connect("user-a").expect("connect user-a");
            let (_conn_b, mut rx_b) = manager.connect("user-b").expect("connect user-b");
            rx_a.try_recv().expect("heartbeat for user-a");
            rx_b.try_recv().expect("heartbeat for user-b");

            let ctx = build_ctx(pool, &manager);

            let args = serde_json::json!({
                "name": "Agent-created watch",
                "prompt": "Check if revenue drops more than 10 percent",
                "schedule": "0 9 * * *",
                "mode": "alert",
                "verified_no_duplicates": true,
            });

            let result = CreateWatchTool
                .execute(args, &ctx)
                .await
                .expect("create_watch tool execution");
            let parsed: serde_json::Value =
                serde_json::from_str(&result).expect("tool result is JSON");
            assert_eq!(parsed["success"], serde_json::json!(true), "{result}");

            let msg_a = rx_a
                .try_recv()
                .expect("owner (user-a) should receive the sync_action broadcast");
            assert!(msg_a.contains("sync_action"), "message should be a SyncAction: {msg_a}");

            let result_b = rx_b.try_recv();
            assert!(
                result_b.is_err(),
                "non-owner (user-b) must NOT receive the watch broadcast, got: {result_b:?}"
            );
        }

        #[tokio::test]
        async fn update_watch_tool_broadcasts_to_owner_only() {
            let pool = test_pool().await;
            seed_workspace_with_two_users(&pool).await;

            let watch = kyomi_auth::watch_service::create_watch(
                &pool,
                "ws-1",
                "user-a",
                "Pre-existing watch",
                "Check if revenue drops more than 10 percent",
                "0 9 * * *",
                "alert",
                None,
                None,
                None,
                false,
            )
            .await
            .expect("seed watch");

            let manager = WebSocketManager::new(None, pool.clone());
            let (_conn_a, mut rx_a) = manager.connect("user-a").expect("connect user-a");
            let (_conn_b, mut rx_b) = manager.connect("user-b").expect("connect user-b");
            rx_a.try_recv().expect("heartbeat for user-a");
            rx_b.try_recv().expect("heartbeat for user-b");

            let ctx = build_ctx(pool, &manager);

            let args = serde_json::json!({
                "watch_id": watch.watch_id,
                "name": "Renamed via agent",
            });

            let result = UpdateWatchTool
                .execute(args, &ctx)
                .await
                .expect("update_watch tool execution");
            let parsed: serde_json::Value =
                serde_json::from_str(&result).expect("tool result is JSON");
            assert_eq!(parsed["success"], serde_json::json!(true), "{result}");

            let msg_a = rx_a
                .try_recv()
                .expect("owner (user-a) should receive the sync_action broadcast");
            assert!(msg_a.contains("sync_action"), "message should be a SyncAction: {msg_a}");

            let result_b = rx_b.try_recv();
            assert!(
                result_b.is_err(),
                "non-owner (user-b) must NOT receive the watch broadcast, got: {result_b:?}"
            );
        }

        #[tokio::test]
        async fn delete_watch_tool_broadcasts_to_owner_only() {
            let pool = test_pool().await;
            seed_workspace_with_two_users(&pool).await;

            let watch = kyomi_auth::watch_service::create_watch(
                &pool,
                "ws-1",
                "user-a",
                "Watch to delete",
                "Check if revenue drops more than 10 percent",
                "0 9 * * *",
                "alert",
                None,
                None,
                None,
                false,
            )
            .await
            .expect("seed watch");

            let manager = WebSocketManager::new(None, pool.clone());
            let (_conn_a, mut rx_a) = manager.connect("user-a").expect("connect user-a");
            let (_conn_b, mut rx_b) = manager.connect("user-b").expect("connect user-b");
            rx_a.try_recv().expect("heartbeat for user-a");
            rx_b.try_recv().expect("heartbeat for user-b");

            let ctx = build_ctx(pool, &manager);

            let args = serde_json::json!({ "watch_id": watch.watch_id });

            let result = DeleteWatchTool
                .execute(args, &ctx)
                .await
                .expect("delete_watch tool execution");
            let parsed: serde_json::Value =
                serde_json::from_str(&result).expect("tool result is JSON");
            assert_eq!(parsed["success"], serde_json::json!(true), "{result}");

            let msg_a = rx_a
                .try_recv()
                .expect("owner (user-a) should receive the sync_action broadcast");
            assert!(msg_a.contains("sync_action"), "message should be a SyncAction: {msg_a}");

            let result_b = rx_b.try_recv();
            assert!(
                result_b.is_err(),
                "non-owner (user-b) must NOT receive the watch broadcast, got: {result_b:?}"
            );
        }
    }

    // -- CreateWatchTool ----------------------------------------------------

    #[test]
    fn create_watch_name() {
        assert_eq!(CreateWatchTool.name(), "create_watch");
    }

    #[test]
    fn create_watch_description_not_empty() {
        assert!(!CreateWatchTool.description().is_empty());
    }

    #[test]
    fn create_watch_schema_has_required_fields() {
        let schema = CreateWatchTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("name")));
        assert!(required.contains(&serde_json::json!("prompt")));
        assert!(required.contains(&serde_json::json!("schedule")));
        assert!(required.contains(&serde_json::json!("mode")));
        assert!(required.contains(&serde_json::json!("verified_no_duplicates")));
        assert_eq!(required.len(), 5);
    }

    #[test]
    fn create_watch_schema_has_expected_properties() {
        let schema = CreateWatchTool.parameters_schema();
        let props = schema["properties"].as_object().expect("properties is object");
        assert!(props.contains_key("name"));
        assert!(props.contains_key("prompt"));
        assert!(props.contains_key("schedule"));
        assert!(props.contains_key("mode"));
        assert!(props.contains_key("verified_no_duplicates"));
        assert!(props.contains_key("queries"));
        assert!(props.contains_key("datasource_hints"));
        assert!(props.contains_key("alert_emails"));
        assert!(props.contains_key("alert_emails_enabled"));
    }

    #[test]
    fn create_watch_annotations_not_read_only() {
        let ann = CreateWatchTool.annotations().expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(false));
        assert!(ann.destructive_hint.is_none());
    }

    #[test]
    fn create_watch_not_copilot_only() {
        assert!(!CreateWatchTool.is_copilot_only());
    }

    // -- PreviewWatchTool ---------------------------------------------------

    #[test]
    fn preview_watch_name() {
        assert_eq!(PreviewWatchTool.name(), "preview_watch");
    }

    #[test]
    fn preview_watch_description_not_empty() {
        assert!(!PreviewWatchTool.description().is_empty());
    }

    #[test]
    fn preview_watch_schema_has_required_fields() {
        let schema = PreviewWatchTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("name")));
        assert!(required.contains(&serde_json::json!("prompt")));
        assert!(required.contains(&serde_json::json!("schedule")));
        assert_eq!(required.len(), 3);
    }

    #[test]
    fn preview_watch_schema_has_optional_watch_id() {
        let schema = PreviewWatchTool.parameters_schema();
        let props = schema["properties"].as_object().expect("properties is object");
        assert!(props.contains_key("watch_id"));
    }

    #[test]
    fn preview_watch_is_copilot_only() {
        assert!(PreviewWatchTool.is_copilot_only());
    }

    #[test]
    fn preview_watch_annotations_read_only() {
        let ann = PreviewWatchTool.annotations().expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(true));
        assert!(ann.destructive_hint.is_none());
    }

    // -- UpdateWatchTool ----------------------------------------------------

    #[test]
    fn update_watch_name() {
        assert_eq!(UpdateWatchTool.name(), "update_watch");
    }

    #[test]
    fn update_watch_description_not_empty() {
        assert!(!UpdateWatchTool.description().is_empty());
    }

    #[test]
    fn update_watch_schema_requires_watch_id() {
        let schema = UpdateWatchTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("watch_id")));
        assert_eq!(required.len(), 1);
    }

    #[test]
    fn update_watch_schema_has_optional_fields() {
        let schema = UpdateWatchTool.parameters_schema();
        let props = schema["properties"].as_object().expect("properties is object");
        assert!(props.contains_key("name"));
        assert!(props.contains_key("prompt"));
        assert!(props.contains_key("schedule"));
        assert!(props.contains_key("mode"));
        assert!(props.contains_key("enabled"));
        assert!(props.contains_key("alert_emails"));
        assert!(props.contains_key("alert_emails_enabled"));
        assert!(props.contains_key("queries"));
    }

    #[test]
    fn update_watch_annotations_not_read_only() {
        let ann = UpdateWatchTool.annotations().expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(false));
        assert!(ann.destructive_hint.is_none());
    }

    #[test]
    fn update_watch_not_copilot_only() {
        assert!(!UpdateWatchTool.is_copilot_only());
    }

    // -- SearchWatchesTool --------------------------------------------------

    #[test]
    fn search_watches_name() {
        assert_eq!(SearchWatchesTool.name(), "search_watches");
    }

    #[test]
    fn search_watches_description_not_empty() {
        assert!(!SearchWatchesTool.description().is_empty());
    }

    #[test]
    fn search_watches_schema_has_no_required_fields() {
        let schema = SearchWatchesTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.is_empty());
    }

    #[test]
    fn search_watches_schema_has_expected_properties() {
        let schema = SearchWatchesTool.parameters_schema();
        let props = schema["properties"].as_object().expect("properties is object");
        assert!(props.contains_key("query"));
        assert!(props.contains_key("limit"));
        assert!(props.contains_key("timezone_offset"));
    }

    #[test]
    fn search_watches_annotations_read_only() {
        let ann = SearchWatchesTool.annotations().expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(true));
        assert!(ann.destructive_hint.is_none());
    }

    #[test]
    fn search_watches_not_copilot_only() {
        assert!(!SearchWatchesTool.is_copilot_only());
    }

    // -- DeleteWatchTool ----------------------------------------------------

    #[test]
    fn delete_watch_name() {
        assert_eq!(DeleteWatchTool.name(), "delete_watch");
    }

    #[test]
    fn delete_watch_description_not_empty() {
        assert!(!DeleteWatchTool.description().is_empty());
    }

    #[test]
    fn delete_watch_schema_requires_watch_id() {
        let schema = DeleteWatchTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("watch_id")));
        assert_eq!(required.len(), 1);
    }

    #[test]
    fn delete_watch_annotations_destructive() {
        let ann = DeleteWatchTool.annotations().expect("has annotations");
        assert_eq!(ann.destructive_hint, Some(true));
        assert!(ann.read_only_hint.is_none());
    }

    #[test]
    fn delete_watch_not_copilot_only() {
        assert!(!DeleteWatchTool.is_copilot_only());
    }

    // -- GetWatchInfoTool ---------------------------------------------------

    #[test]
    fn get_watch_info_name() {
        assert_eq!(GetWatchInfoTool.name(), "get_watch_info");
    }

    #[test]
    fn get_watch_info_description_not_empty() {
        assert!(!GetWatchInfoTool.description().is_empty());
    }

    #[test]
    fn get_watch_info_schema_requires_watch_id() {
        let schema = GetWatchInfoTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("watch_id")));
        assert_eq!(required.len(), 1);
    }

    #[test]
    fn get_watch_info_annotations_read_only() {
        let ann = GetWatchInfoTool.annotations().expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(true));
        assert!(ann.destructive_hint.is_none());
    }

    #[test]
    fn get_watch_info_not_copilot_only() {
        assert!(!GetWatchInfoTool.is_copilot_only());
    }

    // -- TriggerWatchTool ---------------------------------------------------

    #[test]
    fn trigger_watch_name() {
        assert_eq!(TriggerWatchTool.name(), "trigger_watch");
    }

    #[test]
    fn trigger_watch_description_not_empty() {
        assert!(!TriggerWatchTool.description().is_empty());
    }

    #[test]
    fn trigger_watch_schema_requires_watch_id() {
        let schema = TriggerWatchTool.parameters_schema();
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("watch_id")));
        assert_eq!(required.len(), 1);
    }

    #[test]
    fn trigger_watch_annotations_not_read_only() {
        let ann = TriggerWatchTool.annotations().expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(false));
        assert!(ann.destructive_hint.is_none());
    }

    #[test]
    fn trigger_watch_not_copilot_only() {
        assert!(!TriggerWatchTool.is_copilot_only());
    }

    // -- Cross-tool: is_copilot_only ----------------------------------------

    #[test]
    fn only_preview_watch_is_copilot_only() {
        assert!(!CreateWatchTool.is_copilot_only());
        assert!(PreviewWatchTool.is_copilot_only());
        assert!(!UpdateWatchTool.is_copilot_only());
        assert!(!SearchWatchesTool.is_copilot_only());
        assert!(!DeleteWatchTool.is_copilot_only());
        assert!(!GetWatchInfoTool.is_copilot_only());
        assert!(!TriggerWatchTool.is_copilot_only());
    }

    // -- Cross-tool: annotations summary ------------------------------------

    #[test]
    fn read_only_tools_have_read_only_annotation() {
        // These tools should be read-only
        for tool in &[
            PreviewWatchTool.annotations(),
            SearchWatchesTool.annotations(),
            GetWatchInfoTool.annotations(),
        ] {
            let ann = tool.as_ref().expect("has annotations");
            assert_eq!(ann.read_only_hint, Some(true));
        }
    }

    #[test]
    fn mutating_tools_have_not_read_only_annotation() {
        // These tools mutate data
        for tool in &[
            CreateWatchTool.annotations(),
            UpdateWatchTool.annotations(),
            TriggerWatchTool.annotations(),
        ] {
            let ann = tool.as_ref().expect("has annotations");
            assert_eq!(ann.read_only_hint, Some(false));
        }
    }

    #[test]
    fn only_delete_has_destructive_annotation() {
        let ann = DeleteWatchTool.annotations().expect("has annotations");
        assert_eq!(ann.destructive_hint, Some(true));

        // All other tools should not have destructive_hint set
        for tool in &[
            CreateWatchTool.annotations(),
            PreviewWatchTool.annotations(),
            UpdateWatchTool.annotations(),
            SearchWatchesTool.annotations(),
            GetWatchInfoTool.annotations(),
            TriggerWatchTool.annotations(),
        ] {
            let ann = tool.as_ref().expect("has annotations");
            assert!(ann.destructive_hint.is_none());
        }
    }
}
