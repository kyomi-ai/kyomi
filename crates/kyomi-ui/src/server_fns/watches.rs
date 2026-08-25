// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for watch CRUD, executions, and alert management.
//!
//! These call directly into `kyomi_auth::watch_service` — the internal REST
//! routes that predated this module were deleted wholesale in the
//! React→Leptos migration (KYO-73, #181).

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::types::{AlertsPage, WatchExecutionItem, WatchListItem};

/// Shared config for creating or updating a watch.
/// Used by [`create_watch`] and [`update_watch`] to keep argument count under clippy's limit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WatchConfig {
    pub name: String,
    pub prompt: String,
    pub schedule: String,
    pub mode: Option<String>,
    pub queries: Option<String>,
    pub slack_channel_id: Option<String>,
    pub slack_channel_name: Option<String>,
    pub alert_emails: Option<String>,
    pub alert_emails_enabled: Option<bool>,
}
#[cfg(feature = "ssr")]
use crate::types::AlertItem;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers (server-only)
// ─────────────────────────────────────────────────────────────────────────────

/// Convert a `Watch` model to a `WatchListItem`, resolving alert channel info.
#[cfg(feature = "ssr")]
async fn watch_to_item(
    db: &kyomi_core::DbPool,
    watch: &kyomi_core::models::Watch,
) -> WatchListItem {
    // Resolve Slack alert channel from the platform-abstracted table
    let (slack_channel_id, slack_channel_name) =
        match kyomi_core::platform::get_watch_alert_channels(db, &watch.watch_id).await {
            Ok(channels) => {
                if let Some(ch) = channels.iter().find(|c| c.channel_type == "slack") {
                    (Some(ch.channel_id.clone()), ch.channel_name.clone())
                } else {
                    (None, None)
                }
            }
            Err(e) => {
                tracing::warn!(
                    watch_id = %watch.watch_id,
                    error = %e,
                    "Failed to load alert channels"
                );
                (None, None)
            }
        };

    WatchListItem {
        watch_id: watch.watch_id.clone(),
        name: watch.name.clone(),
        prompt: watch.prompt.clone(),
        schedule: watch.schedule.clone(),
        mode: watch.mode.to_string(),
        enabled: watch.enabled,
        last_run_at: watch.last_run_at.map(|dt| dt.to_rfc3339()),
        last_run_status: watch.last_run_status.map(|s| s.to_string()),
        next_run_at: watch.next_run_at.map(|dt| dt.to_rfc3339()),
        created_at: watch.created_at.to_rfc3339(),
        created_by: watch.created_by.clone(),
        alert_emails: watch.alert_emails.clone(),
        alert_emails_enabled: watch.alert_emails_enabled,
        queries: watch.queries.clone(),
        slack_channel_id,
        slack_channel_name,
    }
}

/// Convert a `WatchExecution` model to a `WatchExecutionItem`.
#[cfg(feature = "ssr")]
fn execution_to_item(
    execution: &kyomi_core::models::WatchExecution,
    include_trace: bool,
) -> WatchExecutionItem {
    WatchExecutionItem {
        id: execution.id,
        watch_id: execution.watch_id.clone(),
        watch_name: execution.watch_name.clone(),
        mode: Some(
            execution
                .mode
                .unwrap_or(kyomi_core::WatchMode::Alert)
                .to_string(),
        ),
        started_at: execution.started_at.to_rfc3339(),
        completed_at: execution.completed_at.map(|dt| dt.to_rfc3339()),
        status: execution.status.to_string(),
        agent_response: execution.agent_response.clone(),
        error_message: execution.error_message.clone(),
        input_tokens: execution.input_tokens,
        output_tokens: execution.output_tokens,
        alert_triggered: execution.alert_triggered,
        notification_id: execution.notification_id.clone(),
        execution_trace: if include_trace {
            execution.execution_trace.clone()
        } else {
            None
        },
        read_at: execution.read_at.map(|dt| dt.to_rfc3339()),
        deleted_at: execution.deleted_at.map(|dt| dt.to_rfc3339()),
        deleted_by: execution.deleted_by.clone(),
    }
}

/// Convert a `WatchExecution` model to an `AlertItem`.
///
/// Includes execution_trace to match the REST API's `get_alerts_history` behavior.
#[cfg(feature = "ssr")]
fn execution_to_alert(execution: &kyomi_core::models::WatchExecution) -> AlertItem {
    AlertItem {
        id: execution.id,
        watch_id: execution.watch_id.clone(),
        watch_name: execution.watch_name.clone(),
        mode: Some(
            execution
                .mode
                .unwrap_or(kyomi_core::WatchMode::Alert)
                .to_string(),
        ),
        started_at: execution.started_at.to_rfc3339(),
        completed_at: execution.completed_at.map(|dt| dt.to_rfc3339()),
        status: execution.status.to_string(),
        agent_response: execution.agent_response.clone(),
        error_message: execution.error_message.clone(),
        input_tokens: execution.input_tokens,
        output_tokens: execution.output_tokens,
        alert_triggered: execution.alert_triggered,
        notification_id: execution.notification_id.clone(),
        execution_trace: execution.execution_trace.clone(),
        read_at: execution.read_at.map(|dt| dt.to_rfc3339()),
        deleted_at: execution.deleted_at.map(|dt| dt.to_rfc3339()),
        deleted_by: execution.deleted_by.clone(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Watch CRUD
// ─────────────────────────────────────────────────────────────────────────────

/// List all watches in the current workspace.
#[server(prefix = "/leptos-api")]
pub async fn list_watches() -> Result<Vec<WatchListItem>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let watches = kyomi_auth::watch_service::list_watches(ac.db(), &ac.ws_id, &ac.auth.user_id)
        .await
        .into_sfn()?;

    let mut items = Vec::with_capacity(watches.len());
    for w in &watches {
        items.push(watch_to_item(ac.db(), w).await);
    }

    Ok(items)
}

/// Create a new watch.
#[server(prefix = "/leptos-api")]
pub async fn create_watch(config: WatchConfig) -> Result<WatchListItem, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let WatchConfig {
        name,
        prompt,
        schedule,
        mode,
        queries,
        slack_channel_id,
        slack_channel_name,
        alert_emails,
        alert_emails_enabled,
    } = config;

    let mode = mode.unwrap_or_else(|| "alert".to_string());

    // Parse queries JSON string into Value
    let queries_value: Option<serde_json::Value> = queries
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| ServerFnError::new(format!("Invalid queries JSON: {e}")))?;

    let watch = kyomi_auth::watch_service::create_watch(
        ac.db(),
        &ac.ws_id,
        &ac.auth.user_id,
        name.trim(),
        prompt.trim(),
        &schedule,
        &mode,
        queries_value.as_ref(),
        None, // datasource_hints: not exposed in Leptos UI
        alert_emails.as_deref(),
        alert_emails_enabled.unwrap_or(false),
    )
    .await
    .into_sfn()?;

    // Set the Slack alert channel if provided
    if let Some(ref channel_id) = slack_channel_id
        && !channel_id.is_empty()
    {
        kyomi_core::platform::set_watch_alert_channel(
            ac.db(),
            &watch.watch_id,
            "slack",
            channel_id,
            slack_channel_name.as_deref(),
        )
        .await
        .map_err(|e| {
            tracing::error!(
                watch_id = %watch.watch_id,
                "Failed to set alert channel: {e}"
            );
            ServerFnError::new("Failed to set alert channel")
        })?;
    }

    let item = watch_to_item(ac.db(), &watch).await;

    if let Some(ws_manager) = &ac.ctx.ws_manager {
        kyomi_auth::websocket::helpers::broadcast_watch_sync(
            ac.db(), ws_manager, &watch.watch_id, &ac.ws_id,
            kyomi_types::sync::SyncActionType::Insert, &watch.created_by,
        ).await;
    }

    Ok(item)
}

/// Update a watch with partial fields.
#[server(prefix = "/leptos-api")]
pub async fn update_watch(
    watch_id: String,
    config: WatchConfig,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let WatchConfig {
        name: name_val,
        prompt: prompt_val,
        schedule: schedule_val,
        mode,
        queries,
        slack_channel_id,
        slack_channel_name,
        alert_emails,
        alert_emails_enabled,
    } = config;

    // Wrap in Option for the update path (all fields optional)
    let name = Some(name_val);
    let prompt = Some(prompt_val);
    let schedule = Some(schedule_val);

    // Parse queries JSON string into Value
    let queries_value: Option<serde_json::Value> = queries
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| ServerFnError::new(format!("Invalid queries JSON: {e}")))?;

    // Process alert_emails: empty/whitespace means clear
    let alert_emails = alert_emails.map(|s| {
        let trimmed = s.trim().to_string();
        if trimmed.is_empty() {
            String::new()
        } else {
            trimmed
        }
    });

    let updates = kyomi_auth::watch_service::WatchUpdate {
        name: name.map(|s| s.trim().to_string()),
        prompt: prompt.map(|s| s.trim().to_string()),
        schedule,
        mode,
        enabled: None, // Use toggle_watch() instead — enabled is not exposed via update_watch
        alert_emails,
        alert_emails_enabled,
        queries: queries_value,
        datasource_hints: None,
    };

    // Check if there are actual updates
    let has_updates = updates.name.is_some()
        || updates.prompt.is_some()
        || updates.schedule.is_some()
        || updates.mode.is_some()
        || updates.alert_emails.is_some()
        || updates.alert_emails_enabled.is_some()
        || updates.queries.is_some();

    let has_slack_update = slack_channel_id.is_some();

    if !has_updates && !has_slack_update {
        return Err(ServerFnError::new("No updates provided"));
    }

    // Track the owner so the live-sync broadcast below can route privately —
    // watches have no sharing model, so this always goes to `created_by`.
    let created_by = if has_updates {
        let updated = kyomi_auth::watch_service::update_watch(
            ac.db(),
            &watch_id,
            &ac.ws_id,
            &ac.auth.user_id,
            &updates,
        )
        .await
        .into_sfn()?;
        updated.created_by
    } else {
        // Slack-only update path — no field update call was made, so fetch
        // the current watch to learn its owner.
        kyomi_auth::watch_service::get_watch(ac.db(), &watch_id, &ac.ws_id, &ac.auth.user_id)
            .await
            .into_sfn()?
            .ok_or_else(|| ServerFnError::new("Watch not found"))?
            .created_by
    };

    // Update the Slack alert channel if provided
    if let Some(ref channel_id) = slack_channel_id {
        if channel_id.is_empty() {
            kyomi_core::platform::remove_watch_alert_channel(ac.db(), &watch_id, "slack")
                .await
                .map_err(|e| {
                    tracing::error!(watch_id = %watch_id, "Failed to remove alert channel: {e}");
                    ServerFnError::new("Failed to remove alert channel")
                })?;
        } else {
            kyomi_core::platform::set_watch_alert_channel(
                ac.db(),
                &watch_id,
                "slack",
                channel_id,
                slack_channel_name.as_deref(),
            )
            .await
            .map_err(|e| {
                tracing::error!(watch_id = %watch_id, "Failed to set alert channel: {e}");
                ServerFnError::new("Failed to set alert channel")
            })?;
        }
    }

    if let Some(ws_manager) = &ac.ctx.ws_manager {
        kyomi_auth::websocket::helpers::broadcast_watch_sync(
            ac.db(), ws_manager, &watch_id, &ac.ws_id,
            kyomi_types::sync::SyncActionType::Update, &created_by,
        ).await;
    }

    Ok(())
}

/// Delete a watch.
#[server(prefix = "/leptos-api")]
pub async fn delete_watch(watch_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let created_by =
        kyomi_auth::watch_service::delete_watch(ac.db(), &watch_id, &ac.ws_id, &ac.auth.user_id)
            .await
            .into_sfn()?;

    if let Some(ws_manager) = &ac.ctx.ws_manager {
        kyomi_auth::websocket::helpers::broadcast_watch_sync(
            ac.db(), ws_manager, &watch_id, &ac.ws_id,
            kyomi_types::sync::SyncActionType::Delete, &created_by,
        ).await;
    }

    Ok(())
}

/// Toggle a watch's enabled state.
#[server(prefix = "/leptos-api")]
pub async fn toggle_watch(watch_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let watch = kyomi_auth::watch_service::get_watch(ac.db(), &watch_id, &ac.ws_id, &ac.auth.user_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Watch not found"))?;

    kyomi_auth::watch_service::toggle_watch(
        ac.db(),
        &watch_id,
        &ac.ws_id,
        &ac.auth.user_id,
        !watch.enabled,
    )
    .await
    .into_sfn()?;

    if let Some(ws_manager) = &ac.ctx.ws_manager {
        kyomi_auth::websocket::helpers::broadcast_watch_sync(
            ac.db(), ws_manager, &watch_id, &ac.ws_id,
            kyomi_types::sync::SyncActionType::Update, &watch.created_by,
        ).await;
    }

    Ok(())
}

/// Manually trigger a watch run.
///
/// Checks rate limits and concurrency, then spawns background execution.
#[server(prefix = "/leptos-api")]
pub async fn run_watch_now(watch_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Rate limit and concurrency check
    let (can_run, reason) = kyomi_auth::watch_service::can_run_watch_now(
        ac.db(),
        &watch_id,
        &ac.ws_id,
        &ac.auth.user_id,
    )
    .await
    .into_sfn()?;

    if !can_run {
        return Err(ServerFnError::new(reason));
    }

    // Spawn background execution — require all necessary services
    let bg_db = ac.ctx.db.clone();
    let bg_kv = ac.kv()?;
    let bg_encryption_key = ac.encryption_key()?;
    let bg_embedding = ac
        .ctx
        .embedding
        .wait_ready()
        .await
        // user_message() (KYO-448) — Display would leak the variant tag.
        .map_err(|e| ServerFnError::new(format!("Embedding model not ready: {}", e.user_message())))?
        .clone();
    let bg_ws_manager = ac
        .ctx
        .ws_manager
        .clone()
        .ok_or_else(|| ServerFnError::new("WebSocket manager not available"))?;
    let bg_config = ac.ctx.config.clone();
    let bg_connect = ac.ctx.connect_registry.clone();
    let bg_platforms = ac
        .ctx
        .platforms
        .clone()
        .ok_or_else(|| ServerFnError::new("Platform registry not available"))?;
    let bg_watch_id = watch_id.clone();

    tokio::spawn(async move {
        if let Err(e) = kyomi_agent::watch_execution::execute_watch(
            &bg_db,
            &bg_kv,
            &bg_encryption_key,
            &bg_embedding,
            &bg_ws_manager,
            &bg_config,
            bg_connect,
            &bg_platforms,
            &bg_watch_id,
        )
        .await
        {
            tracing::error!(
                watch_id = %bg_watch_id,
                error = %e,
                "Background watch execution failed"
            );
        }
    });

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Executions
// ─────────────────────────────────────────────────────────────────────────────

/// Get execution history for a watch.
#[server(prefix = "/leptos-api")]
pub async fn get_watch_executions(
    watch_id: String,
    limit: Option<i64>,
) -> Result<Vec<WatchExecutionItem>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let limit = limit.unwrap_or(20).clamp(1, 100) as u32;

    let executions = kyomi_auth::watch_service::get_executions(
        ac.db(),
        &watch_id,
        &ac.ws_id,
        &ac.auth.user_id,
        limit,
    )
    .await
    .into_sfn()?;

    Ok(executions.iter().map(|e| execution_to_item(e, false)).collect())
}

/// Get a specific execution by ID.
#[server(prefix = "/leptos-api")]
pub async fn get_watch_execution(
    watch_id: String,
    execution_id: i32,
) -> Result<WatchExecutionItem, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let execution = kyomi_auth::watch_service::get_execution_by_id(
        ac.db(),
        &watch_id,
        execution_id,
        &ac.ws_id,
        &ac.auth.user_id,
    )
    .await
    .into_sfn()?
    .ok_or_else(|| ServerFnError::new("Execution not found"))?;

    Ok(execution_to_item(&execution, true))
}

// ─────────────────────────────────────────────────────────────────────────────
// Alerts
// ─────────────────────────────────────────────────────────────────────────────

/// Get alerts history (paginated).
#[server(prefix = "/leptos-api")]
pub async fn get_alerts(
    watch_id: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    include_deleted: Option<bool>,
) -> Result<AlertsPage, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let limit = limit.unwrap_or(50).clamp(1, 100);
    let offset = offset.unwrap_or(0).max(0);
    let include_deleted = include_deleted.unwrap_or(false);

    let (executions, total) = kyomi_auth::watch_service::get_alerts_history(
        ac.db(),
        &ac.ws_id,
        watch_id.as_deref(),
        limit,
        offset,
        include_deleted,
        &ac.auth.user_id,
    )
    .await
    .into_sfn()?;

    Ok(AlertsPage {
        alerts: executions.iter().map(execution_to_alert).collect(),
        total,
    })
}

/// Get unread alerts count for the sidebar badge.
#[server(prefix = "/leptos-api")]
pub async fn get_unread_alerts_count() -> Result<i64, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    kyomi_auth::watch_service::get_unread_alerts_count(ac.db(), &ac.ws_id, &ac.auth.user_id)
        .await
        .into_sfn()
}

/// Mark an alert as read.
#[server(prefix = "/leptos-api")]
pub async fn mark_alert_read(execution_id: i32) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    kyomi_auth::watch_service::mark_alert_read(ac.db(), execution_id, &ac.ws_id, &ac.auth.user_id)
        .await
        .into_sfn()
}

/// Mark an alert as unread.
#[server(prefix = "/leptos-api")]
pub async fn mark_alert_unread(execution_id: i32) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    kyomi_auth::watch_service::mark_alert_unread(ac.db(), execution_id, &ac.ws_id, &ac.auth.user_id)
        .await
        .into_sfn()
}

/// Soft-delete an alert.
#[server(prefix = "/leptos-api")]
pub async fn delete_alert(execution_id: i32) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    kyomi_auth::watch_service::delete_alert(ac.db(), execution_id, &ac.ws_id, &ac.auth.user_id)
        .await
        .into_sfn()
}

/// Restore a soft-deleted alert.
#[server(prefix = "/leptos-api")]
pub async fn restore_alert(execution_id: i32) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    kyomi_auth::watch_service::restore_alert(ac.db(), execution_id, &ac.ws_id, &ac.auth.user_id)
        .await
        .into_sfn()
}

/// Bulk soft-delete alerts.
#[server(prefix = "/leptos-api")]
pub async fn bulk_delete_alerts(execution_ids: Vec<i32>) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    if execution_ids.is_empty() {
        return Err(ServerFnError::new("execution_ids must not be empty"));
    }

    let unique_ids: Vec<i32> = execution_ids
        .into_iter()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    if unique_ids.len() > 100 {
        return Err(ServerFnError::new(
            "Cannot process more than 100 alerts at once",
        ));
    }

    kyomi_auth::watch_service::bulk_delete_alerts(ac.db(), &unique_ids, &ac.ws_id, &ac.auth.user_id)
        .await
        .into_sfn()?;

    Ok(())
}

/// Bulk mark alerts as read.
#[server(prefix = "/leptos-api")]
pub async fn bulk_mark_alerts_read(execution_ids: Vec<i32>) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    if execution_ids.is_empty() {
        return Err(ServerFnError::new("execution_ids must not be empty"));
    }

    let unique_ids: Vec<i32> = execution_ids
        .into_iter()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    if unique_ids.len() > 100 {
        return Err(ServerFnError::new(
            "Cannot process more than 100 alerts at once",
        ));
    }

    kyomi_auth::watch_service::bulk_mark_alerts_read(
        ac.db(),
        &unique_ids,
        &ac.ws_id,
        &ac.auth.user_id,
    )
    .await
    .into_sfn()?;

    Ok(())
}

/// Bulk mark alerts as unread.
#[server(prefix = "/leptos-api")]
pub async fn bulk_mark_alerts_unread(execution_ids: Vec<i32>) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    if execution_ids.is_empty() {
        return Err(ServerFnError::new("execution_ids must not be empty"));
    }

    let unique_ids: Vec<i32> = execution_ids
        .into_iter()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    if unique_ids.len() > 100 {
        return Err(ServerFnError::new(
            "Cannot process more than 100 alerts at once",
        ));
    }

    kyomi_auth::watch_service::bulk_mark_alerts_unread(
        ac.db(),
        &unique_ids,
        &ac.ws_id,
        &ac.auth.user_id,
    )
    .await
    .into_sfn()?;

    Ok(())
}

/// Create a chat session from an alert, continuing the conversation.
///
/// Returns the new session_id.
#[server(prefix = "/leptos-api")]
pub async fn continue_alert_in_chat(execution_id: i32) -> Result<String, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let encryption_key = ac.encryption_key()?;

    kyomi_auth::watch_service::create_chat_session_from_alert(
        ac.db(),
        &encryption_key,
        &ac.auth.user_id,
        &ac.ws_id,
        execution_id,
    )
    .await
    .into_sfn()
}

/// Get thinking events for a specific execution.
///
/// Returns a JSON array of thinking events directly, since Leptos components
/// consume it as-is. The now-deleted REST route this replaced
/// (`GET /watches/{watch_id}/executions/{execution_id}/thinking-events`) wrapped
/// the same data in an envelope object (`{ execution_id, session_id, events }`).
#[server(prefix = "/leptos-api")]
pub async fn get_thinking_events(
    watch_id: String,
    execution_id: i32,
) -> Result<serde_json::Value, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let execution = kyomi_auth::watch_service::get_execution_by_id(
        ac.db(),
        &watch_id,
        execution_id,
        &ac.ws_id,
        &ac.auth.user_id,
    )
    .await
    .into_sfn()?
    .ok_or_else(|| ServerFnError::new("Execution not found"))?;

    let mut thinking_events = serde_json::Value::Array(Vec::new());

    // Load thinking events from session messages
    if let Some(ref session_id) = execution.session_id {
        let encryption_key = ac.encryption_key()?;

        match kyomi_auth::chat_service::get_session_messages(
            ac.db(),
            &encryption_key,
            session_id,
            1000,
        )
        .await
        {
            Ok(messages) => {
                for msg in &messages {
                    if msg.message_type == "assistant" && !msg.thinking_events.is_empty() {
                        thinking_events =
                            serde_json::Value::Array(msg.thinking_events.clone());
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to load thinking events from session: {e}");
            }
        }
    }

    // Fallback for old executions that stored events in execution_trace
    if thinking_events
        .as_array()
        .map(|a| a.is_empty())
        .unwrap_or(true)
        && let Some(events) = execution
            .execution_trace
            .as_ref()
            .and_then(|t| t.get("events"))
            .filter(|e| e.is_array())
    {
        thinking_events = events.clone();
    }

    Ok(thinking_events)
}

// SSR-only import — placed at bottom to match `dashboards.rs` convention.
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};
