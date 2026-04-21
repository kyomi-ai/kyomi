// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for watch CRUD, executions, and alert management.
//!
//! These replace the REST API calls for managing watches, viewing executions,
//! and handling alerts. Each function calls the same service-layer code as
//! the existing REST routes in `apps/server/src/routes/watches.rs`.

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
///
/// Mirrors `watch_to_response` in `apps/server/src/routes/watches.rs`.
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
///
/// Mirrors `execution_to_response` in `apps/server/src/routes/watches.rs`.
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
///
/// Mirrors `GET /watches/` in `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn list_watches() -> Result<Vec<WatchListItem>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let watches = kyomi_auth::watch_service::list_watches(&ctx.db, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let mut items = Vec::with_capacity(watches.len());
    for w in &watches {
        items.push(watch_to_item(&ctx.db, w).await);
    }

    Ok(items)
}

/// Create a new watch.
///
/// Mirrors `POST /watches/` in `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn create_watch(config: WatchConfig) -> Result<WatchListItem, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

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
        &ctx.db,
        ws_id,
        &auth.user_id,
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
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Set the Slack alert channel if provided
    if let Some(ref channel_id) = slack_channel_id
        && !channel_id.is_empty()
    {
        kyomi_core::platform::set_watch_alert_channel(
            &ctx.db,
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

    Ok(watch_to_item(&ctx.db, &watch).await)
}

/// Get a single watch by ID.
///
/// Mirrors `GET /watches/{watch_id}` in `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_watch(watch_id: String) -> Result<WatchListItem, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let watch = kyomi_auth::watch_service::get_watch(&ctx.db, &watch_id, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Watch not found"))?;

    Ok(watch_to_item(&ctx.db, &watch).await)
}

/// Update a watch with partial fields.
///
/// Mirrors `PATCH /watches/{watch_id}` in `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn update_watch(
    watch_id: String,
    config: WatchConfig,
) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

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

    if has_updates {
        kyomi_auth::watch_service::update_watch(&ctx.db, &watch_id, ws_id, &updates)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    }

    // Update the Slack alert channel if provided
    if let Some(ref channel_id) = slack_channel_id {
        if channel_id.is_empty() {
            kyomi_core::platform::remove_watch_alert_channel(&ctx.db, &watch_id, "slack")
                .await
                .map_err(|e| {
                    tracing::error!(watch_id = %watch_id, "Failed to remove alert channel: {e}");
                    ServerFnError::new("Failed to remove alert channel")
                })?;
        } else {
            kyomi_core::platform::set_watch_alert_channel(
                &ctx.db,
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

    Ok(())
}

/// Delete a watch.
///
/// Mirrors `DELETE /watches/{watch_id}` in `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn delete_watch(watch_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    kyomi_auth::watch_service::delete_watch(&ctx.db, &watch_id, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

/// Toggle a watch's enabled state.
///
/// Mirrors `POST /watches/{watch_id}/toggle` in `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn toggle_watch(watch_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let watch = kyomi_auth::watch_service::get_watch(&ctx.db, &watch_id, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Watch not found"))?;

    kyomi_auth::watch_service::toggle_watch(&ctx.db, &watch_id, ws_id, !watch.enabled)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

/// Manually trigger a watch run.
///
/// Checks rate limits and concurrency, then spawns background execution.
///
/// Mirrors `POST /watches/{watch_id}/run` in `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn run_watch_now(watch_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Rate limit and concurrency check
    let (can_run, reason) =
        kyomi_auth::watch_service::can_run_watch_now(&ctx.db, &watch_id, ws_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    if !can_run {
        return Err(ServerFnError::new(reason));
    }

    // Spawn background execution — require all necessary services
    let bg_db = ctx.db.clone();
    let bg_kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;
    let bg_encryption_key = ctx
        .encryption_key
        .clone()
        .ok_or_else(|| ServerFnError::new("Encryption key not available"))?;
    let bg_embedding = ctx
        .embedding
        .wait_ready()
        .await
        .map_err(|e| ServerFnError::new(format!("Embedding model not ready: {e}")))?
        .clone();
    let bg_ws_manager = ctx
        .ws_manager
        .clone()
        .ok_or_else(|| ServerFnError::new("WebSocket manager not available"))?;
    let bg_config = ctx.config.clone();
    let bg_connect = ctx.connect_registry.clone();
    let bg_platforms = ctx
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
///
/// Mirrors `GET /watches/{watch_id}/executions` in `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_watch_executions(
    watch_id: String,
    limit: Option<i64>,
) -> Result<Vec<WatchExecutionItem>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let limit = limit.unwrap_or(20).clamp(1, 100) as u32;

    let executions =
        kyomi_auth::watch_service::get_executions(&ctx.db, &watch_id, ws_id, limit)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(executions.iter().map(|e| execution_to_item(e, false)).collect())
}

/// Get a specific execution by ID.
///
/// Mirrors `GET /watches/{watch_id}/executions/{execution_id}` in
/// `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_watch_execution(
    watch_id: String,
    execution_id: i32,
) -> Result<WatchExecutionItem, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let execution =
        kyomi_auth::watch_service::get_execution_by_id(&ctx.db, &watch_id, execution_id, ws_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .ok_or_else(|| ServerFnError::new("Execution not found"))?;

    Ok(execution_to_item(&execution, true))
}

// ─────────────────────────────────────────────────────────────────────────────
// Alerts
// ─────────────────────────────────────────────────────────────────────────────

/// Get alerts history (paginated).
///
/// Mirrors `GET /watches/alerts` in `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_alerts(
    watch_id: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    include_deleted: Option<bool>,
) -> Result<AlertsPage, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let limit = limit.unwrap_or(50).clamp(1, 100);
    let offset = offset.unwrap_or(0).max(0);
    let include_deleted = include_deleted.unwrap_or(false);

    let (executions, total) = kyomi_auth::watch_service::get_alerts_history(
        &ctx.db,
        ws_id,
        watch_id.as_deref(),
        limit,
        offset,
        include_deleted,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(AlertsPage {
        alerts: executions.iter().map(execution_to_alert).collect(),
        total,
    })
}

/// Get unread alerts count for the sidebar badge.
///
/// Mirrors `GET /watches/alerts/count` in `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_unread_alerts_count() -> Result<i64, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    kyomi_auth::watch_service::get_unread_alerts_count(&ctx.db, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Mark an alert as read.
///
/// Mirrors `POST /watches/alerts/{execution_id}/read` in
/// `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn mark_alert_read(execution_id: i32) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    kyomi_auth::watch_service::mark_alert_read(&ctx.db, execution_id, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Mark an alert as unread.
///
/// Mirrors `POST /watches/alerts/{execution_id}/unread` in
/// `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn mark_alert_unread(execution_id: i32) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    kyomi_auth::watch_service::mark_alert_unread(&ctx.db, execution_id, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Soft-delete an alert.
///
/// Mirrors `POST /watches/alerts/{execution_id}/delete` in
/// `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn delete_alert(execution_id: i32) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    kyomi_auth::watch_service::delete_alert(&ctx.db, execution_id, ws_id, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Restore a soft-deleted alert.
///
/// Mirrors `POST /watches/alerts/{execution_id}/restore` in
/// `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn restore_alert(execution_id: i32) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    kyomi_auth::watch_service::restore_alert(&ctx.db, execution_id, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Bulk soft-delete alerts.
///
/// Mirrors `POST /watches/alerts/bulk-delete` in
/// `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn bulk_delete_alerts(execution_ids: Vec<i32>) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

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

    kyomi_auth::watch_service::bulk_delete_alerts(&ctx.db, &unique_ids, ws_id, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

/// Bulk mark alerts as read.
///
/// Mirrors `POST /watches/alerts/bulk-read` in
/// `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn bulk_mark_alerts_read(execution_ids: Vec<i32>) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

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

    kyomi_auth::watch_service::bulk_mark_alerts_read(&ctx.db, &unique_ids, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

/// Bulk mark alerts as unread.
///
/// Mirrors `POST /watches/alerts/bulk-unread` in
/// `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn bulk_mark_alerts_unread(execution_ids: Vec<i32>) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

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

    kyomi_auth::watch_service::bulk_mark_alerts_unread(&ctx.db, &unique_ids, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

/// Create a chat session from an alert, continuing the conversation.
///
/// Returns the new session_id.
///
/// Mirrors `POST /watches/alerts/{execution_id}/continue-chat` in
/// `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
// lint-allow: server-fn-callouts=pre-existing orchestration drift tracked in KYO-124
pub async fn continue_alert_in_chat(execution_id: i32) -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let encryption_key = ctx
        .encryption_key
        .clone()
        .ok_or_else(|| ServerFnError::new("Encryption key not available"))?;

    // Get the execution with full trace (works even if watch is deleted)
    let execution =
        kyomi_auth::watch_service::get_execution_by_id_only(&ctx.db, execution_id, ws_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .ok_or_else(|| ServerFnError::new("Alert not found"))?;

    if !execution.alert_triggered {
        return Err(ServerFnError::new(
            "This execution did not trigger an alert",
        ));
    }

    // Extract watch context from execution trace
    let watch_name = execution
        .watch_name
        .as_deref()
        .unwrap_or("Deleted Watch");

    let mut watch_prompt: Option<String> = None;
    let mut alert_title: Option<String> = None;

    if let Some(obj) = execution
        .execution_trace
        .as_ref()
        .and_then(|t| t.as_object())
    {
        watch_prompt = obj
            .get("watch_prompt")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        alert_title = obj
            .get("alert_title")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    // Fallback: try fetching from live watch if trace doesn't have prompt
    if let (None, Some(wid)) = (&watch_prompt, &execution.watch_id) {
        let watch = kyomi_auth::watch_service::get_watch(&ctx.db, wid, ws_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        watch_prompt = watch.map(|w| w.prompt);
    }

    let watch_prompt = watch_prompt.unwrap_or_else(|| "(Watch has been deleted)".to_string());

    // Load thinking events from execution session messages
    let mut thinking_events: Option<serde_json::Value> = None;

    if let Some(ref session_id) = execution.session_id {
        match kyomi_auth::chat_service::get_session_messages(
            &ctx.db,
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
                            Some(serde_json::Value::Array(msg.thinking_events.clone()));
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to load thinking events from session: {e}");
            }
        }
    }

    // Load agent_state from execution trace
    let mut agent_state: Option<serde_json::Value> = None;
    if let Some(obj) = execution
        .execution_trace
        .as_ref()
        .and_then(|t| t.as_object())
    {
        agent_state = obj.get("agent_state").cloned();

        // Fallback for old executions that stored events in execution_trace
        if thinking_events.is_none()
            && let Some(events) = obj.get("events").filter(|e| e.is_array())
        {
            thinking_events = Some(events.clone());
        }
    }

    // Create a new chat session
    let session_id = uuid::Uuid::new_v4().to_string();
    let title = format!(
        "Alert: {}",
        alert_title.as_deref().unwrap_or(watch_name)
    );

    kyomi_auth::chat_service::create_session_with_id(
        &ctx.db,
        &auth.user_id,
        ws_id,
        &session_id,
        Some(&title),
        "chat",
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Add the "user" message representing what was monitored
    let user_message = format!("Monitor: {watch_name}\n\n{watch_prompt}");
    kyomi_auth::chat_service::add_message(
        &ctx.db,
        &encryption_key,
        &session_id,
        "user",
        &user_message,
        None,
        None,
        None,
        Some(&auth.user_id),
        None,
        None,
        None,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Add the "assistant" message with the alert response and thinking events
    let metadata = thinking_events
        .as_ref()
        .map(|events| serde_json::json!({ "thinking_events": events }));

    kyomi_auth::chat_service::add_message(
        &ctx.db,
        &encryption_key,
        &session_id,
        "assistant",
        execution.agent_response.as_deref().unwrap_or(""),
        metadata.as_ref(),
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Save the full agent state from the watch execution
    if let Some(mut state_val) = agent_state {
        // Remove the watch-specific system prompt from the agent state
        if let Some(messages) = state_val.get_mut("messages").and_then(|m| m.as_array_mut()) {
            let is_system = messages
                .first()
                .and_then(|f| f.get("role"))
                .and_then(|r| r.as_str())
                == Some("system");

            if is_system {
                messages.remove(0);

                // Adjust compaction index after system prompt removal
                if let Some(idx_val) = state_val
                    .get("messages_since_compaction_index")
                    .and_then(|v| v.as_i64())
                    .filter(|&v| v > 0)
                {
                    state_val["messages_since_compaction_index"] =
                        serde_json::json!(std::cmp::max(0, idx_val - 1));
                }
            }
        }

        // Update timestamp to current time
        state_val["timestamp"] =
            serde_json::json!(chrono::Utc::now().to_rfc3339());

        let config = serde_json::json!({ "agent_state": state_val });
        if let Err(e) = kyomi_auth::chat_service::update_session(
            &ctx.db,
            &session_id,
            None,
            None,
            Some(&config),
        )
        .await
        {
            tracing::error!(
                "Failed to save agent_state for session {session_id}: {e}"
            );
        }
    } else {
        // Fallback for old executions without agent_state
        let fallback_state = serde_json::json!({
            "version": "2.0",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "messages": [
                {"role": "user", "content": user_message},
                {"role": "assistant", "content": execution.agent_response.as_deref().unwrap_or("")},
            ],
            "global_iteration": 1,
            "compacted_summary": null,
            "messages_since_compaction_index": 0,
            "last_input_tokens": 0,
            "config": {
                "max_iterations": 25,
                "temperature": 0.1,
            }
        });

        let config = serde_json::json!({ "agent_state": fallback_state });
        if let Err(e) = kyomi_auth::chat_service::update_session(
            &ctx.db,
            &session_id,
            None,
            None,
            Some(&config),
        )
        .await
        {
            tracing::error!(
                "Failed to save fallback agent_state for session {session_id}: {e}"
            );
        }
    }

    Ok(session_id)
}

/// Get the most recent execution for a watch.
///
/// Mirrors `GET /watches/{watch_id}/last-execution` in
/// `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_last_execution(
    watch_id: String,
) -> Result<Option<WatchExecutionItem>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Verify watch exists and belongs to workspace
    kyomi_auth::watch_service::get_watch(&ctx.db, &watch_id, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Watch not found"))?;

    let executions =
        kyomi_auth::watch_service::get_executions(&ctx.db, &watch_id, ws_id, 1)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(executions.first().map(|e| execution_to_item(e, true)))
}

/// Get thinking events for a specific execution.
///
/// Returns a JSON array of thinking events directly (not wrapped in an envelope
/// object like the REST route). Unlike the REST equivalent which returns
/// `{ execution_id, session_id, events }`, this returns the events array directly
/// since Leptos components consume it directly.
///
/// Based on `GET /watches/{watch_id}/executions/{execution_id}/thinking-events` in
/// `apps/server/src/routes/watches.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_thinking_events(
    watch_id: String,
    execution_id: i32,
) -> Result<serde_json::Value, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let execution =
        kyomi_auth::watch_service::get_execution_by_id(&ctx.db, &watch_id, execution_id, ws_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .ok_or_else(|| ServerFnError::new("Execution not found"))?;

    let mut thinking_events = serde_json::Value::Array(Vec::new());

    // Load thinking events from session messages
    if let Some(ref session_id) = execution.session_id {
        let encryption_key = ctx
            .encryption_key
            .clone()
            .ok_or_else(|| ServerFnError::new("Encryption key not available"))?;

        match kyomi_auth::chat_service::get_session_messages(
            &ctx.db,
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
use super::{extract_auth, extract_context, workspace_id};
