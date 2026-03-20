// SPDX-License-Identifier: AGPL-3.0-or-later

//! Watch REST endpoints.
//!
//! Wire-compatible with Python's `routers/watches.py`.
//! All business logic is delegated to `kyomi_auth::watch_service`.
//! Route handlers are thin wrappers: extract auth, check capability, call service, return JSON.
//!
//! ## Endpoints
//!
//! - `GET    /`                                — list_watches
//! - `POST   /`                                — create_watch
//! - `GET    /alerts`                          — get_alerts_history
//! - `POST   /alerts/{execution_id}/read`      — mark_alert_read
//! - `POST   /alerts/{execution_id}/unread`    — mark_alert_unread
//! - `POST   /alerts/{execution_id}/delete`    — delete_alert
//! - `POST   /alerts/{execution_id}/restore`   — restore_alert
//! - `POST   /alerts/bulk-delete`              — bulk_delete_alerts
//! - `POST   /alerts/bulk-read`                — bulk_mark_alerts_read
//! - `POST   /alerts/bulk-unread`              — bulk_mark_alerts_unread
//! - `POST   /alerts/{execution_id}/continue-chat` — continue_alert_in_chat
//! - `GET    /alerts/count`                    — get_unread_alerts_count
//! - `GET    /{watch_id}`                      — get_watch
//! - `PATCH  /{watch_id}`                      — update_watch
//! - `DELETE /{watch_id}`                      — delete_watch
//! - `POST   /{watch_id}/toggle`               — toggle_watch
//! - `POST   /{watch_id}/run`                  — run_watch_now
//! - `GET    /{watch_id}/executions`            — get_executions
//! - `GET    /{watch_id}/executions/{execution_id}` — get_execution
//! - `GET    /{watch_id}/last-execution`        — get_last_execution
//! - `GET    /{watch_id}/executions/{execution_id}/thinking-events` — get_thinking_events

use std::collections::HashSet;

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use kyomi_auth::{chat_service, middleware::AuthUser, watch_service};
use kyomi_core::capability;

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the `/watches` router with all watch management endpoints.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Static paths FIRST (before /{watch_id} captures them)
        .route("/alerts/count", get(get_unread_alerts_count))
        .route("/alerts/bulk-delete", post(bulk_delete_alerts))
        .route("/alerts/bulk-read", post(bulk_mark_alerts_read))
        .route("/alerts/bulk-unread", post(bulk_mark_alerts_unread))
        .route("/alerts/{execution_id}/read", post(mark_alert_read))
        .route("/alerts/{execution_id}/unread", post(mark_alert_unread))
        .route("/alerts/{execution_id}/delete", post(delete_alert))
        .route("/alerts/{execution_id}/restore", post(restore_alert))
        .route(
            "/alerts/{execution_id}/continue-chat",
            post(continue_alert_in_chat),
        )
        .route("/alerts", get(get_alerts_history))
        // Dynamic path handlers
        .route("/", get(list_watches).post(create_watch))
        .route(
            "/{watch_id}",
            get(get_watch).patch(update_watch).delete(delete_watch),
        )
        .route("/{watch_id}/toggle", post(toggle_watch))
        .route("/{watch_id}/run", post(run_watch_now))
        .route("/{watch_id}/executions", get(get_executions))
        .route(
            "/{watch_id}/executions/{execution_id}",
            get(get_execution),
        )
        .route("/{watch_id}/last-execution", get(get_last_execution))
        .route(
            "/{watch_id}/executions/{execution_id}/thinking-events",
            get(get_thinking_events),
        )
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Extract workspace_id from user, or return 400.
fn get_workspace_id(user: &AuthUser) -> Result<&str, kyomi_core::Error> {
    user.workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("Workspace context required".into()))
}

/// Check if the workspace has the `kyomi_watch` capability. Returns 403 on failure.
/// Self-hosted mode bypasses tier checks — all features are available.
fn check_watch_capability(state: &AppState, user: &AuthUser) -> Result<(), kyomi_core::Error> {
    if state.config.self_hosted {
        return Ok(());
    }
    if !capability::has_capability(user.workspace.subscription_tier, "kyomi_watch") {
        return Err(kyomi_core::Error::Forbidden(
            "Kyomi Watch requires Pro or Team plan. Please upgrade to access this feature.".into(),
        ));
    }
    Ok(())
}

/// Convert a `Watch` model to a `WatchResponse`, including alert channel info.
async fn watch_to_response(
    db: &kyomi_core::db::DbPool,
    watch: &kyomi_core::models::Watch,
) -> WatchResponse {
    // Look up the Slack alert channel from the platform-abstracted table.
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
                tracing::warn!(watch_id = %watch.watch_id, error = %e, "Failed to load alert channels");
                (None, None)
            }
        };

    WatchResponse {
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

/// Convert a `WatchExecution` model to an `ExecutionResponse`.
fn execution_to_response(
    execution: &kyomi_core::models::WatchExecution,
    include_trace: bool,
) -> ExecutionResponse {
    ExecutionResponse {
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

// ===========================================================================
// Request / Response Types
// ===========================================================================

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct CreateWatchRequest {
    name: String,
    prompt: String,
    schedule: String,
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default)]
    datasource_hints: Option<Value>,
    #[serde(default)]
    queries: Option<Vec<Value>>,
    #[serde(default)]
    slack_channel_id: Option<String>,
    #[serde(default)]
    slack_channel_name: Option<String>,
}

fn default_mode() -> String {
    "alert".to_string()
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct UpdateWatchRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    schedule: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    datasource_hints: Option<Value>,
    #[serde(default)]
    queries: Option<Vec<Value>>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    alert_emails: Option<String>,
    #[serde(default)]
    alert_emails_enabled: Option<bool>,
    /// Slack channel for alert delivery. `None` = no change, `Some("")` = remove, `Some("C...")` = set.
    #[serde(default)]
    slack_channel_id: Option<String>,
    #[serde(default)]
    slack_channel_name: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct WatchResponse {
    watch_id: String,
    name: String,
    prompt: String,
    schedule: String,
    mode: String,
    enabled: bool,
    last_run_at: Option<String>,
    last_run_status: Option<String>,
    next_run_at: Option<String>,
    created_at: String,
    created_by: String,
    alert_emails: Option<String>,
    alert_emails_enabled: bool,
    queries: Option<Value>,
    slack_channel_id: Option<String>,
    slack_channel_name: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct ExecutionResponse {
    id: i32,
    watch_id: Option<String>,
    watch_name: Option<String>,
    mode: Option<String>,
    started_at: String,
    completed_at: Option<String>,
    status: String,
    agent_response: Option<String>,
    error_message: Option<String>,
    input_tokens: i32,
    output_tokens: i32,
    alert_triggered: bool,
    notification_id: Option<String>,
    execution_trace: Option<Value>,
    read_at: Option<String>,
    deleted_at: Option<String>,
    deleted_by: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct AlertHistoryResponse {
    alerts: Vec<ExecutionResponse>,
    total: i64,
    limit: i64,
    offset: i64,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct BulkAlertRequest {
    execution_ids: Vec<i32>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct BulkAlertResponse {
    updated_count: u64,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct ContinueChatResponse {
    session_id: String,
}

// -- Query params --

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct AlertsHistoryParams {
    #[serde(default)]
    watch_id: Option<String>,
    #[serde(default = "default_alerts_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    include_deleted: bool,
}

fn default_alerts_limit() -> i64 {
    50
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct ExecutionsParams {
    #[serde(default = "default_executions_limit")]
    limit: u32,
}

fn default_executions_limit() -> u32 {
    20
}

// ===========================================================================
// Endpoint Handlers
// ===========================================================================

// ---------------------------------------------------------------------------
// GET / — List watches
// ---------------------------------------------------------------------------

async fn list_watches(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<WatchResponse>>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    let watches = watch_service::list_watches(&state.db, workspace_id).await?;

    let mut response = Vec::with_capacity(watches.len());
    for w in &watches {
        response.push(watch_to_response(&state.db, w).await);
    }

    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// POST / — Create watch
// ---------------------------------------------------------------------------

async fn create_watch(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<CreateWatchRequest>,
) -> Result<Json<WatchResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    // Validate name length
    let name = request.name.trim();
    if name.len() < 3 {
        return Err(kyomi_core::Error::BadRequest(
            "Watch name must be at least 3 characters".into(),
        ));
    }

    // Validate prompt length
    let prompt = request.prompt.trim();
    if prompt.len() < 10 {
        return Err(kyomi_core::Error::BadRequest(
            "Watch prompt must be at least 10 characters".into(),
        ));
    }

    // Convert queries Vec<Value> to a single Value for the service
    let queries_value = request.queries.map(serde_json::Value::Array);

    let watch = watch_service::create_watch(
        &state.db,
        workspace_id,
        &user.user_id,
        name,
        prompt,
        &request.schedule,
        &request.mode,
        queries_value.as_ref(),
        request.datasource_hints.as_ref(),
        None, // alert_emails: not set on create
        false, // alert_emails_enabled: default false
    )
    .await
    .map_err(|e| match &e {
        kyomi_core::Error::Conflict(_)
        | kyomi_core::Error::BadRequest(_)
        | kyomi_core::Error::Forbidden(_) => e,
        _ => {
            tracing::error!("Failed to create watch: {e}");
            kyomi_core::Error::Internal("Failed to create watch".into())
        }
    })?;

    // Set the Slack alert channel if provided.
    if let Some(ref channel_id) = request.slack_channel_id {
        kyomi_core::platform::set_watch_alert_channel(
            &state.db,
            &watch.watch_id,
            "slack",
            channel_id,
            request.slack_channel_name.as_deref(),
        )
        .await
        .map_err(|e| {
            tracing::error!(watch_id = %watch.watch_id, "Failed to set alert channel: {e}");
            kyomi_core::Error::Internal("Failed to set alert channel".into())
        })?;
    }

    tracing::info!(
        watch_id = %watch.watch_id,
        name = %watch.name,
        "Created watch"
    );

    Ok(Json(watch_to_response(&state.db, &watch).await))
}

// ---------------------------------------------------------------------------
// GET /alerts — Get alerts history
// ---------------------------------------------------------------------------

async fn get_alerts_history(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<AlertsHistoryParams>,
) -> Result<Json<AlertHistoryResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    let limit = params.limit.clamp(1, 100);
    let offset = params.offset.max(0);

    let (executions, total) = watch_service::get_alerts_history(
        &state.db,
        workspace_id,
        params.watch_id.as_deref(),
        limit,
        offset,
        params.include_deleted,
    )
    .await?;

    let alerts: Vec<ExecutionResponse> = executions
        .iter()
        .map(|e| execution_to_response(e, true))
        .collect();

    Ok(Json(AlertHistoryResponse {
        alerts,
        total,
        limit: params.limit, // Return the original requested limit, matching Python
        offset,
    }))
}

// ---------------------------------------------------------------------------
// POST /alerts/{execution_id}/read — Mark alert as read
// ---------------------------------------------------------------------------

async fn mark_alert_read(
    State(state): State<AppState>,
    user: AuthUser,
    Path(execution_id): Path<i32>,
) -> Result<Json<ExecutionResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    watch_service::mark_alert_read(&state.db, execution_id, workspace_id).await?;

    // Re-fetch the execution to return updated state
    let execution =
        watch_service::get_execution_by_id_only(&state.db, execution_id, workspace_id).await?;

    match execution {
        Some(exec) => Ok(Json(execution_to_response(&exec, false))),
        None => Err(kyomi_core::Error::NotFound("Alert not found".into())),
    }
}

// ---------------------------------------------------------------------------
// POST /alerts/{execution_id}/unread — Mark alert as unread
// ---------------------------------------------------------------------------

async fn mark_alert_unread(
    State(state): State<AppState>,
    user: AuthUser,
    Path(execution_id): Path<i32>,
) -> Result<Json<ExecutionResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    watch_service::mark_alert_unread(&state.db, execution_id, workspace_id).await?;

    // Re-fetch the execution to return updated state
    let execution =
        watch_service::get_execution_by_id_only(&state.db, execution_id, workspace_id).await?;

    match execution {
        Some(exec) => Ok(Json(execution_to_response(&exec, false))),
        None => Err(kyomi_core::Error::NotFound("Alert not found".into())),
    }
}

// ---------------------------------------------------------------------------
// POST /alerts/{execution_id}/delete — Soft-delete alert
// ---------------------------------------------------------------------------

async fn delete_alert(
    State(state): State<AppState>,
    user: AuthUser,
    Path(execution_id): Path<i32>,
) -> Result<Json<ExecutionResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    watch_service::delete_alert(&state.db, execution_id, workspace_id, &user.user_id).await?;

    // Re-fetch the execution to return updated state
    let execution =
        watch_service::get_execution_by_id_only(&state.db, execution_id, workspace_id).await?;

    match execution {
        Some(exec) => Ok(Json(execution_to_response(&exec, false))),
        None => Err(kyomi_core::Error::NotFound("Alert not found".into())),
    }
}

// ---------------------------------------------------------------------------
// POST /alerts/{execution_id}/restore — Restore soft-deleted alert
// ---------------------------------------------------------------------------

async fn restore_alert(
    State(state): State<AppState>,
    user: AuthUser,
    Path(execution_id): Path<i32>,
) -> Result<Json<ExecutionResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    watch_service::restore_alert(&state.db, execution_id, workspace_id).await?;

    // Re-fetch the execution to return updated state
    let execution =
        watch_service::get_execution_by_id_only(&state.db, execution_id, workspace_id).await?;

    match execution {
        Some(exec) => Ok(Json(execution_to_response(&exec, false))),
        None => Err(kyomi_core::Error::NotFound("Alert not found".into())),
    }
}

// ---------------------------------------------------------------------------
// POST /alerts/bulk-delete — Bulk soft-delete alerts
// ---------------------------------------------------------------------------

async fn bulk_delete_alerts(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<BulkAlertRequest>,
) -> Result<Json<BulkAlertResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    if request.execution_ids.is_empty() {
        return Err(kyomi_core::Error::BadRequest(
            "execution_ids must not be empty".into(),
        ));
    }

    let unique_ids: Vec<i32> = request
        .execution_ids
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    if unique_ids.len() > 100 {
        return Err(kyomi_core::Error::BadRequest(
            "Cannot process more than 100 alerts at once".into(),
        ));
    }

    let count =
        watch_service::bulk_delete_alerts(&state.db, &unique_ids, workspace_id, &user.user_id)
            .await?;

    Ok(Json(BulkAlertResponse {
        updated_count: count,
    }))
}

// ---------------------------------------------------------------------------
// POST /alerts/bulk-read — Bulk mark alerts as read
// ---------------------------------------------------------------------------

async fn bulk_mark_alerts_read(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<BulkAlertRequest>,
) -> Result<Json<BulkAlertResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    if request.execution_ids.is_empty() {
        return Err(kyomi_core::Error::BadRequest(
            "execution_ids must not be empty".into(),
        ));
    }

    let unique_ids: Vec<i32> = request
        .execution_ids
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    if unique_ids.len() > 100 {
        return Err(kyomi_core::Error::BadRequest(
            "Cannot process more than 100 alerts at once".into(),
        ));
    }

    let count =
        watch_service::bulk_mark_alerts_read(&state.db, &unique_ids, workspace_id).await?;

    Ok(Json(BulkAlertResponse {
        updated_count: count,
    }))
}

// ---------------------------------------------------------------------------
// POST /alerts/bulk-unread — Bulk mark alerts as unread
// ---------------------------------------------------------------------------

async fn bulk_mark_alerts_unread(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<BulkAlertRequest>,
) -> Result<Json<BulkAlertResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    if request.execution_ids.is_empty() {
        return Err(kyomi_core::Error::BadRequest(
            "execution_ids must not be empty".into(),
        ));
    }

    let unique_ids: Vec<i32> = request
        .execution_ids
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    if unique_ids.len() > 100 {
        return Err(kyomi_core::Error::BadRequest(
            "Cannot process more than 100 alerts at once".into(),
        ));
    }

    let count =
        watch_service::bulk_mark_alerts_unread(&state.db, &unique_ids, workspace_id).await?;

    Ok(Json(BulkAlertResponse {
        updated_count: count,
    }))
}

// ---------------------------------------------------------------------------
// POST /alerts/{execution_id}/continue-chat — Create chat session from alert
// ---------------------------------------------------------------------------

async fn continue_alert_in_chat(
    State(state): State<AppState>,
    user: AuthUser,
    Path(execution_id): Path<i32>,
) -> Result<Json<ContinueChatResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    // Get the execution with full trace (works even if watch is deleted)
    let execution =
        watch_service::get_execution_by_id_only(&state.db, execution_id, workspace_id).await?;
    let execution = execution.ok_or_else(|| {
        kyomi_core::Error::NotFound("Alert not found".into())
    })?;

    if !execution.alert_triggered {
        return Err(kyomi_core::Error::BadRequest(
            "This execution did not trigger an alert".into(),
        ));
    }

    // Extract watch context from execution trace (snapshot from when alert was created)
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
        let watch = watch_service::get_watch(&state.db, wid, workspace_id).await?;
        watch_prompt = watch.map(|w| w.prompt);
    }

    let watch_prompt = watch_prompt.unwrap_or_else(|| "(Watch has been deleted)".to_string());

    // Load thinking events from execution session messages
    let mut thinking_events: Option<Value> = None;

    if let Some(ref session_id) = execution.session_id {
        match chat_service::get_session_messages(
            &state.db,
            &state.encryption_key,
            session_id,
            1000,
        )
        .await
        {
            Ok(messages) => {
                // Look for assistant message with thinking_events
                for msg in &messages {
                    if msg.message_type == "assistant" && !msg.thinking_events.is_empty() {
                        thinking_events =
                            Some(Value::Array(msg.thinking_events.clone()));
                        break;
                    }
                }
                tracing::info!(
                    "Loaded {} thinking events from session {}",
                    thinking_events
                        .as_ref()
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0),
                    session_id
                );
            }
            Err(e) => {
                tracing::warn!("Failed to load thinking events from session: {e}");
            }
        }
    }

    // Load agent_state from execution trace
    let mut agent_state: Option<Value> = None;
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

    chat_service::create_session_with_id(
        &state.db,
        &user.user_id,
        workspace_id,
        &session_id,
        Some(&title),
        "chat",
    )
    .await?;

    // Add the "user" message representing what was monitored
    let user_message = format!("Monitor: {watch_name}\n\n{watch_prompt}");
    chat_service::add_message(
        &state.db,
        &state.encryption_key,
        &session_id,
        "user",
        &user_message,
        None,  // metadata
        None,  // message_id
        None,  // current_time_user_tz
        Some(&user.user_id),
        None,  // tool_call_id
        None,  // tool_name
        None,  // tool_calls
    )
    .await?;

    // Add the "assistant" message with the alert response and thinking events
    let metadata = thinking_events
        .as_ref()
        .map(|events| json!({ "thinking_events": events }));

    chat_service::add_message(
        &state.db,
        &state.encryption_key,
        &session_id,
        "assistant",
        execution.agent_response.as_deref().unwrap_or(""),
        metadata.as_ref(),
        None,  // message_id
        None,  // current_time_user_tz
        None,  // sent_by_user_id
        None,  // tool_call_id
        None,  // tool_name
        None,  // tool_calls
    )
    .await?;

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
                        json!(std::cmp::max(0, idx_val - 1));
                }

                tracing::info!(
                    "Removed watch system prompt from agent_state for chat continuation"
                );
            }
        }

        // Update timestamp to current time
        state_val["timestamp"] = json!(Utc::now().to_rfc3339());

        let config = json!({ "agent_state": state_val });
        if let Err(e) =
            chat_service::update_session(&state.db, &session_id, None, None, Some(&config)).await
        {
            tracing::error!(
                "Failed to save agent_state for session {session_id}: {e}"
            );
            // Continue — session is still usable, just without preserved agent state
        }
    } else {
        // Fallback for old executions without agent_state
        let fallback_state = json!({
            "version": "2.0",
            "timestamp": Utc::now().to_rfc3339(),
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

        let config = json!({ "agent_state": fallback_state });
        if let Err(e) =
            chat_service::update_session(&state.db, &session_id, None, None, Some(&config)).await
        {
            tracing::error!(
                "Failed to save fallback agent_state for session {session_id}: {e}"
            );
        }

        tracing::warn!(
            "Alert {execution_id} missing agent_state in execution_trace, using fallback"
        );
    }

    tracing::info!(
        "Created chat session {session_id} from alert {execution_id} for watch {watch_name}"
    );

    Ok(Json(ContinueChatResponse { session_id }))
}

// ---------------------------------------------------------------------------
// GET /alerts/count — Get unread alerts count
// ---------------------------------------------------------------------------

async fn get_unread_alerts_count(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    let count = watch_service::get_unread_alerts_count(&state.db, workspace_id).await?;

    Ok(Json(json!({ "count": count })))
}

// ---------------------------------------------------------------------------
// GET /{watch_id} — Get a specific watch
// ---------------------------------------------------------------------------

async fn get_watch(
    State(state): State<AppState>,
    user: AuthUser,
    Path(watch_id): Path<String>,
) -> Result<Json<WatchResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    let watch = watch_service::get_watch(&state.db, &watch_id, workspace_id).await?;

    match watch {
        Some(w) => Ok(Json(watch_to_response(&state.db, &w).await)),
        None => Err(kyomi_core::Error::NotFound("Watch not found".into())),
    }
}

// ---------------------------------------------------------------------------
// PATCH /{watch_id} — Update a watch
// ---------------------------------------------------------------------------

async fn update_watch(
    State(state): State<AppState>,
    user: AuthUser,
    Path(watch_id): Path<String>,
    Json(request): Json<UpdateWatchRequest>,
) -> Result<Json<WatchResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    // Validate provided fields
    if let Some(ref name) = request.name
        && name.trim().len() < 3
    {
        return Err(kyomi_core::Error::BadRequest(
            "Watch name must be at least 3 characters".into(),
        ));
    }

    if let Some(ref prompt) = request.prompt
        && prompt.trim().len() < 10
    {
        return Err(kyomi_core::Error::BadRequest(
            "Watch prompt must be at least 10 characters".into(),
        ));
    }

    if let Some(ref mode) = request.mode
        && mode != "alert"
        && mode != "report"
    {
        return Err(kyomi_core::Error::BadRequest(
            "Mode must be 'alert' or 'report'".into(),
        ));
    }

    // Process alert_emails: empty/whitespace means clear
    let alert_emails = request.alert_emails.map(|s| {
        let trimmed = s.trim().to_string();
        if trimmed.is_empty() { String::new() } else { trimmed }
    });

    // Convert queries Vec<Value> to single Value
    let queries_value = request.queries.map(serde_json::Value::Array);

    // Build the update struct
    let updates = watch_service::WatchUpdate {
        name: request.name.map(|s| s.trim().to_string()),
        prompt: request.prompt.map(|s| s.trim().to_string()),
        schedule: request.schedule,
        mode: request.mode,
        enabled: request.enabled,
        alert_emails,
        alert_emails_enabled: request.alert_emails_enabled,
        queries: queries_value,
        datasource_hints: request.datasource_hints,
    };

    // Check if there are actual updates
    let has_updates = updates.name.is_some()
        || updates.prompt.is_some()
        || updates.schedule.is_some()
        || updates.mode.is_some()
        || updates.enabled.is_some()
        || updates.alert_emails.is_some()
        || updates.alert_emails_enabled.is_some()
        || updates.queries.is_some()
        || updates.datasource_hints.is_some();

    // Also check if slack channel fields were sent (not part of WatchUpdate)
    let has_slack_update = request.slack_channel_id.is_some();

    if !has_updates && !has_slack_update {
        return Err(kyomi_core::Error::BadRequest(
            "No updates provided".into(),
        ));
    }

    let watch = if has_updates {
        watch_service::update_watch(&state.db, &watch_id, workspace_id, &updates)
            .await
            .map_err(|e| match &e {
                kyomi_core::Error::NotFound(_) | kyomi_core::Error::BadRequest(_) => e,
                _ => {
                    tracing::error!("Failed to update watch: {e}");
                    kyomi_core::Error::Internal("Failed to update watch".into())
                }
            })?
    } else {
        // Only slack channel update — still need the watch for the response
        watch_service::get_watch(&state.db, &watch_id, workspace_id)
            .await?
            .ok_or_else(|| kyomi_core::Error::NotFound("Watch not found".into()))?
    };

    // Update the Slack alert channel if provided.
    if let Some(ref channel_id) = request.slack_channel_id {
        if channel_id.is_empty() {
            // Empty string means remove the channel
            kyomi_core::platform::remove_watch_alert_channel(&state.db, &watch_id, "slack")
                .await
                .map_err(|e| {
                    tracing::error!(watch_id = %watch_id, "Failed to remove alert channel: {e}");
                    kyomi_core::Error::Internal("Failed to remove alert channel".into())
                })?;
        } else {
            kyomi_core::platform::set_watch_alert_channel(
                &state.db,
                &watch_id,
                "slack",
                channel_id,
                request.slack_channel_name.as_deref(),
            )
            .await
            .map_err(|e| {
                tracing::error!(watch_id = %watch_id, "Failed to set alert channel: {e}");
                kyomi_core::Error::Internal("Failed to set alert channel".into())
            })?;
        }
    }

    tracing::info!(watch_id = %watch_id, "Updated watch");

    Ok(Json(watch_to_response(&state.db, &watch).await))
}

// ---------------------------------------------------------------------------
// DELETE /{watch_id} — Delete a watch
// ---------------------------------------------------------------------------

async fn delete_watch(
    State(state): State<AppState>,
    user: AuthUser,
    Path(watch_id): Path<String>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    watch_service::delete_watch(&state.db, &watch_id, workspace_id)
        .await
        .map_err(|e| match &e {
            kyomi_core::Error::NotFound(_) => e,
            _ => {
                tracing::error!("Failed to delete watch: {e}");
                kyomi_core::Error::Internal("Failed to delete watch".into())
            }
        })?;

    tracing::info!(watch_id = %watch_id, "Deleted watch");

    Ok(Json(json!({
        "message": "Watch deleted",
        "watch_id": watch_id,
    })))
}

// ---------------------------------------------------------------------------
// POST /{watch_id}/toggle — Toggle watch enabled/disabled
// ---------------------------------------------------------------------------

async fn toggle_watch(
    State(state): State<AppState>,
    user: AuthUser,
    Path(watch_id): Path<String>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    let watch = watch_service::get_watch(&state.db, &watch_id, workspace_id).await?;
    let watch = watch.ok_or_else(|| {
        kyomi_core::Error::NotFound("Watch not found".into())
    })?;

    let updated = watch_service::toggle_watch(
        &state.db,
        &watch_id,
        workspace_id,
        !watch.enabled,
    )
    .await
    .map_err(|e| {
        tracing::error!("Failed to toggle watch: {e}");
        kyomi_core::Error::Internal("Failed to toggle watch".into())
    })?;

    tracing::info!(
        watch_id = %watch_id,
        enabled = updated.enabled,
        "Toggled watch"
    );

    Ok(Json(json!({
        "watch_id": watch_id,
        "enabled": updated.enabled,
    })))
}

// ---------------------------------------------------------------------------
// POST /{watch_id}/run — Manually trigger a watch run
// ---------------------------------------------------------------------------

async fn run_watch_now(
    State(state): State<AppState>,
    user: AuthUser,
    Path(watch_id): Path<String>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    // Rate limit and concurrency check
    let (can_run, reason) =
        watch_service::can_run_watch_now(&state.db, &watch_id, workspace_id).await?;

    if !can_run {
        if reason == "Watch not found" {
            return Err(kyomi_core::Error::NotFound(reason));
        }
        return Err(kyomi_core::Error::TooManyRequests(reason, 60));
    }

    // Spawn background execution
    let bg_watch_id = watch_id.clone();
    let bg_db = state.db.clone();
    let bg_kv = state.kv.clone();
    let bg_encryption_key = state.encryption_key.clone();
    let bg_embedding = state.embedding.get().map_err(|e| {
        kyomi_core::Error::ServiceUnavailable(format!("Embedding model not ready: {e}"))
    })?.clone();
    let bg_ws_manager = state.ws_manager.clone();
    let bg_config = state.config.clone();
    let bg_connect = state.connect_registry.clone();
    let bg_platforms = state.platforms.clone();
    tokio::spawn(async move {
        if let Err(e) = kyomi_agent::watch_execution::execute_watch(
            &bg_db,
            &bg_kv,
            &bg_encryption_key,
            &bg_embedding,
            &bg_ws_manager,
            &bg_config,
            Some(bg_connect),
            &bg_platforms,
            &bg_watch_id,
        )
        .await
        {
            tracing::error!(watch_id = %bg_watch_id, error = %e, "Watch execution failed");
        }
    });

    tracing::info!(watch_id = %watch_id, "Triggered manual run for watch");

    Ok(Json(json!({
        "message": "Watch execution started",
        "watch_id": watch_id,
    })))
}

// ---------------------------------------------------------------------------
// GET /{watch_id}/executions — Get execution history
// ---------------------------------------------------------------------------

async fn get_executions(
    State(state): State<AppState>,
    user: AuthUser,
    Path(watch_id): Path<String>,
    Query(params): Query<ExecutionsParams>,
) -> Result<Json<Vec<ExecutionResponse>>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    // Verify watch exists and belongs to workspace
    let watch = watch_service::get_watch(&state.db, &watch_id, workspace_id).await?;
    if watch.is_none() {
        return Err(kyomi_core::Error::NotFound("Watch not found".into()));
    }

    let limit = params.limit.min(100);
    let executions =
        watch_service::get_executions(&state.db, &watch_id, workspace_id, limit).await?;

    let response: Vec<ExecutionResponse> = executions
        .iter()
        .map(|e| execution_to_response(e, false))
        .collect();

    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// GET /{watch_id}/executions/{execution_id} — Get a specific execution
// ---------------------------------------------------------------------------

async fn get_execution(
    State(state): State<AppState>,
    user: AuthUser,
    Path((watch_id, execution_id)): Path<(String, i32)>,
) -> Result<Json<ExecutionResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    let execution =
        watch_service::get_execution_by_id(&state.db, &watch_id, execution_id, workspace_id)
            .await?;

    match execution {
        Some(exec) => Ok(Json(execution_to_response(&exec, true))),
        None => Err(kyomi_core::Error::NotFound(
            "Execution not found".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// GET /{watch_id}/last-execution — Get the last execution for a watch
// ---------------------------------------------------------------------------

async fn get_last_execution(
    State(state): State<AppState>,
    user: AuthUser,
    Path(watch_id): Path<String>,
) -> Result<Json<Option<ExecutionResponse>>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    // Verify watch exists and belongs to workspace
    let watch = watch_service::get_watch(&state.db, &watch_id, workspace_id).await?;
    if watch.is_none() {
        return Err(kyomi_core::Error::NotFound("Watch not found".into()));
    }

    let executions =
        watch_service::get_executions(&state.db, &watch_id, workspace_id, 1).await?;

    let response = executions.first().map(|e| execution_to_response(e, true));

    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// GET /{watch_id}/executions/{execution_id}/thinking-events
// ---------------------------------------------------------------------------

async fn get_thinking_events(
    State(state): State<AppState>,
    user: AuthUser,
    Path((watch_id, execution_id)): Path<(String, i32)>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    check_watch_capability(&state, &user)?;

    let execution =
        watch_service::get_execution_by_id(&state.db, &watch_id, execution_id, workspace_id)
            .await?;
    let execution = execution.ok_or_else(|| {
        kyomi_core::Error::NotFound("Execution not found".into())
    })?;

    let mut thinking_events = Value::Array(Vec::new());

    // If execution has a session_id, load thinking events from session messages
    if let Some(ref session_id) = execution.session_id {
        match chat_service::get_session_messages(
            &state.db,
            &state.encryption_key,
            session_id,
            1000,
        )
        .await
        {
            Ok(messages) => {
                for msg in &messages {
                    if msg.message_type == "assistant" && !msg.thinking_events.is_empty() {
                        thinking_events = Value::Array(msg.thinking_events.clone());
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
    if thinking_events.as_array().map(|a| a.is_empty()).unwrap_or(true)
        && let Some(events) = execution
            .execution_trace
            .as_ref()
            .and_then(|t| t.get("events"))
            .filter(|e| e.is_array())
    {
        thinking_events = events.clone();
    }

    Ok(Json(json!({
        "execution_id": execution_id,
        "session_id": execution.session_id,
        "events": thinking_events,
    })))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // CreateWatchRequest contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn create_watch_request_deserializes_all_fields() {
        let json = json!({
            "name": "Revenue Monitor",
            "prompt": "Check if revenue dropped below threshold",
            "schedule": "0 9 * * *",
            "mode": "alert",
            "datasource_hints": {"datasources": ["prod-pg"]},
            "queries": [{"sql": "SELECT 1", "comment": "test"}]
        });

        let req: CreateWatchRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "Revenue Monitor");
        assert_eq!(req.prompt, "Check if revenue dropped below threshold");
        assert_eq!(req.schedule, "0 9 * * *");
        assert_eq!(req.mode, "alert");
        assert!(req.datasource_hints.is_some());
        assert!(req.queries.is_some());
        assert_eq!(req.queries.unwrap().len(), 1);
    }

    #[test]
    fn create_watch_request_defaults_mode_to_alert() {
        let json = json!({
            "name": "Test Watch",
            "prompt": "Check something important",
            "schedule": "0 9 * * *"
        });

        let req: CreateWatchRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.mode, "alert");
    }

    #[test]
    fn create_watch_request_defaults_optional_fields_to_none() {
        let json = json!({
            "name": "Test Watch",
            "prompt": "Check something important",
            "schedule": "0 9 * * *"
        });

        let req: CreateWatchRequest = serde_json::from_value(json).unwrap();
        assert!(req.datasource_hints.is_none());
        assert!(req.queries.is_none());
    }

    #[test]
    fn create_watch_request_fails_without_required_fields() {
        // Missing name
        let json = json!({"prompt": "test", "schedule": "0 9 * * *"});
        assert!(serde_json::from_value::<CreateWatchRequest>(json).is_err());

        // Missing prompt
        let json = json!({"name": "test", "schedule": "0 9 * * *"});
        assert!(serde_json::from_value::<CreateWatchRequest>(json).is_err());

        // Missing schedule
        let json = json!({"name": "test", "prompt": "test"});
        assert!(serde_json::from_value::<CreateWatchRequest>(json).is_err());
    }

    #[test]
    fn create_watch_request_with_report_mode() {
        let json = json!({
            "name": "Daily Report",
            "prompt": "Summarize daily metrics",
            "schedule": "0 18 * * *",
            "mode": "report"
        });

        let req: CreateWatchRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.mode, "report");
    }

    #[test]
    fn create_watch_request_serialization_round_trip() {
        let json = json!({
            "name": "Revenue Monitor",
            "prompt": "Check if revenue dropped below threshold",
            "schedule": "0 9 * * *",
            "mode": "alert",
            "datasource_hints": {"datasources": ["prod-pg"]},
            "queries": [{"sql": "SELECT 1", "comment": "test"}]
        });

        let req: CreateWatchRequest = serde_json::from_value(json).unwrap();
        let serialized = serde_json::to_value(&req).unwrap();
        let deserialized: CreateWatchRequest = serde_json::from_value(serialized).unwrap();
        assert_eq!(deserialized.name, req.name);
        assert_eq!(deserialized.prompt, req.prompt);
        assert_eq!(deserialized.schedule, req.schedule);
        assert_eq!(deserialized.mode, req.mode);
    }

    // -----------------------------------------------------------------------
    // UpdateWatchRequest contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn update_watch_request_all_fields_present() {
        let json = json!({
            "name": "Updated Name",
            "prompt": "Updated prompt text",
            "schedule": "0 18 * * *",
            "mode": "report",
            "enabled": false,

            "alert_emails": "user@example.com",
            "alert_emails_enabled": true,
            "datasource_hints": {"datasources": ["new-ds"]},
            "queries": [{"sql": "SELECT 2"}]
        });

        let req: UpdateWatchRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name.as_deref(), Some("Updated Name"));
        assert_eq!(req.prompt.as_deref(), Some("Updated prompt text"));
        assert_eq!(req.schedule.as_deref(), Some("0 18 * * *"));
        assert_eq!(req.mode.as_deref(), Some("report"));
        assert_eq!(req.enabled, Some(false));
        assert_eq!(req.alert_emails.as_deref(), Some("user@example.com"));
        assert_eq!(req.alert_emails_enabled, Some(true));
        assert!(req.datasource_hints.is_some());
        assert!(req.queries.is_some());
    }

    #[test]
    fn update_watch_request_partial_update_name_only() {
        let json = json!({"name": "New Name"});

        let req: UpdateWatchRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name.as_deref(), Some("New Name"));
        assert!(req.prompt.is_none());
        assert!(req.schedule.is_none());
        assert!(req.mode.is_none());
        assert!(req.enabled.is_none());
        assert!(req.alert_emails.is_none());
        assert!(req.alert_emails_enabled.is_none());
        assert!(req.datasource_hints.is_none());
        assert!(req.queries.is_none());
    }

    #[test]
    fn update_watch_request_empty_object_all_none() {
        let json = json!({});

        let req: UpdateWatchRequest = serde_json::from_value(json).unwrap();
        assert!(req.name.is_none());
        assert!(req.prompt.is_none());
        assert!(req.schedule.is_none());
        assert!(req.mode.is_none());
        assert!(req.enabled.is_none());
        assert!(req.alert_emails.is_none());
        assert!(req.alert_emails_enabled.is_none());
        assert!(req.datasource_hints.is_none());
        assert!(req.queries.is_none());
    }

    #[test]
    fn update_watch_request_enable_only() {
        let json = json!({"enabled": true});

        let req: UpdateWatchRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.enabled, Some(true));
        assert!(req.name.is_none());
    }

    #[test]
    fn update_watch_request_serialization_round_trip() {
        let json = json!({
            "name": "Updated",
            "schedule": "0 12 * * 1-5",
            "enabled": true
        });

        let req: UpdateWatchRequest = serde_json::from_value(json).unwrap();
        let serialized = serde_json::to_value(&req).unwrap();
        let deserialized: UpdateWatchRequest = serde_json::from_value(serialized).unwrap();
        assert_eq!(deserialized.name, req.name);
        assert_eq!(deserialized.schedule, req.schedule);
        assert_eq!(deserialized.enabled, req.enabled);
    }

    // -----------------------------------------------------------------------
    // WatchResponse contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn watch_response_serializes_all_fields() {
        let response = WatchResponse {
            watch_id: "watch-abc123".into(),
            name: "Revenue Monitor".into(),
            prompt: "Check revenue".into(),
            schedule: "0 9 * * *".into(),
            mode: "alert".into(),
            enabled: true,
            last_run_at: Some("2025-01-15T09:00:00+00:00".into()),
            last_run_status: Some("success".into()),
            next_run_at: Some("2025-01-16T09:00:00+00:00".into()),
            created_at: "2025-01-01T00:00:00+00:00".into(),
            created_by: "user-123".into(),
            alert_emails: Some("user@example.com".into()),
            alert_emails_enabled: true,
            queries: Some(json!([{"sql": "SELECT 1"}])),
            slack_channel_id: Some("C12345".into()),
            slack_channel_name: Some("general".into()),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["watch_id"], "watch-abc123");
        assert_eq!(json["name"], "Revenue Monitor");
        assert_eq!(json["mode"], "alert");
        assert_eq!(json["enabled"], true);
        assert_eq!(json["last_run_at"], "2025-01-15T09:00:00+00:00");
        assert_eq!(json["last_run_status"], "success");
        assert_eq!(json["next_run_at"], "2025-01-16T09:00:00+00:00");
        assert_eq!(json["alert_emails"], "user@example.com");
        assert_eq!(json["alert_emails_enabled"], true);
        assert!(json["queries"].is_array());
        assert_eq!(json["slack_channel_id"], "C12345");
        assert_eq!(json["slack_channel_name"], "general");
    }

    #[test]
    fn watch_response_serializes_null_optional_fields() {
        let response = WatchResponse {
            watch_id: "watch-abc123".into(),
            name: "Test".into(),
            prompt: "Test prompt".into(),
            schedule: "0 9 * * *".into(),
            mode: "alert".into(),
            enabled: false,
            last_run_at: None,
            last_run_status: None,
            next_run_at: None,
            created_at: "2025-01-01T00:00:00+00:00".into(),
            created_by: "user-123".into(),
            alert_emails: None,
            alert_emails_enabled: false,
            queries: None,
            slack_channel_id: None,
            slack_channel_name: None,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(json["last_run_at"].is_null());
        assert!(json["last_run_status"].is_null());
        assert!(json["next_run_at"].is_null());
        assert!(json["alert_emails"].is_null());
        assert!(json["queries"].is_null());
        assert_eq!(json["alert_emails_enabled"], false);
        assert!(json["slack_channel_id"].is_null());
        assert!(json["slack_channel_name"].is_null());
    }

    #[test]
    fn watch_response_round_trip() {
        let response = WatchResponse {
            watch_id: "watch-xyz".into(),
            name: "Test Watch".into(),
            prompt: "Check metrics".into(),
            schedule: "0 */6 * * *".into(),
            mode: "report".into(),
            enabled: true,
            last_run_at: None,
            last_run_status: None,
            next_run_at: Some("2025-01-15T12:00:00+00:00".into()),
            created_at: "2025-01-01T00:00:00+00:00".into(),
            created_by: "user-456".into(),
            alert_emails: None,
            alert_emails_enabled: false,
            queries: None,
            slack_channel_id: None,
            slack_channel_name: None,
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: WatchResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.watch_id, "watch-xyz");
        assert_eq!(deserialized.mode, "report");
        assert_eq!(deserialized.enabled, true);
    }

    // -----------------------------------------------------------------------
    // ExecutionResponse contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn execution_response_serializes_all_fields() {
        let response = ExecutionResponse {
            id: 42,
            watch_id: Some("watch-abc".into()),
            watch_name: Some("Revenue Monitor".into()),
            mode: Some("alert".into()),
            started_at: "2025-01-15T09:00:00+00:00".into(),
            completed_at: Some("2025-01-15T09:01:30+00:00".into()),
            status: "success".into(),
            agent_response: Some("Revenue is stable at $48K.".into()),
            error_message: None,
            input_tokens: 1500,
            output_tokens: 200,
            alert_triggered: true,
            notification_id: Some("notif-123".into()),
            execution_trace: Some(json!({"alert_title": "Revenue Alert"})),
            read_at: Some("2025-01-15T10:00:00+00:00".into()),
            deleted_at: None,
            deleted_by: None,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["id"], 42);
        assert_eq!(json["watch_id"], "watch-abc");
        assert_eq!(json["watch_name"], "Revenue Monitor");
        assert_eq!(json["mode"], "alert");
        assert_eq!(json["status"], "success");
        assert_eq!(json["input_tokens"], 1500);
        assert_eq!(json["output_tokens"], 200);
        assert_eq!(json["alert_triggered"], true);
        assert_eq!(json["notification_id"], "notif-123");
        assert!(json["execution_trace"].is_object());
        assert_eq!(json["read_at"], "2025-01-15T10:00:00+00:00");
        assert!(json["deleted_at"].is_null());
        assert!(json["deleted_by"].is_null());
    }

    #[test]
    fn execution_response_without_trace() {
        let response = ExecutionResponse {
            id: 1,
            watch_id: Some("watch-abc".into()),
            watch_name: Some("Test".into()),
            mode: Some("alert".into()),
            started_at: "2025-01-15T09:00:00+00:00".into(),
            completed_at: None,
            status: "running".into(),
            agent_response: None,
            error_message: None,
            input_tokens: 0,
            output_tokens: 0,
            alert_triggered: false,
            notification_id: None,
            execution_trace: None,
            read_at: None,
            deleted_at: None,
            deleted_by: None,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(json["execution_trace"].is_null());
        assert!(json["agent_response"].is_null());
        assert!(json["completed_at"].is_null());
        assert_eq!(json["alert_triggered"], false);
    }

    #[test]
    fn execution_response_with_error() {
        let response = ExecutionResponse {
            id: 99,
            watch_id: Some("watch-err".into()),
            watch_name: Some("Failed Watch".into()),
            mode: Some("alert".into()),
            started_at: "2025-01-15T09:00:00+00:00".into(),
            completed_at: Some("2025-01-15T09:00:05+00:00".into()),
            status: "error".into(),
            agent_response: None,
            error_message: Some("AI budget exhausted".into()),
            input_tokens: 0,
            output_tokens: 0,
            alert_triggered: false,
            notification_id: None,
            execution_trace: None,
            read_at: None,
            deleted_at: None,
            deleted_by: None,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["status"], "error");
        assert_eq!(json["error_message"], "AI budget exhausted");
        assert!(json["agent_response"].is_null());
    }

    #[test]
    fn execution_response_round_trip() {
        let response = ExecutionResponse {
            id: 10,
            watch_id: Some("w1".into()),
            watch_name: Some("Test".into()),
            mode: Some("report".into()),
            started_at: "2025-01-15T09:00:00+00:00".into(),
            completed_at: Some("2025-01-15T09:02:00+00:00".into()),
            status: "success".into(),
            agent_response: Some("Report content here".into()),
            error_message: None,
            input_tokens: 500,
            output_tokens: 100,
            alert_triggered: true,
            notification_id: None,
            execution_trace: Some(json!({"events": []})),
            read_at: None,
            deleted_at: None,
            deleted_by: None,
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: ExecutionResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.id, 10);
        assert_eq!(deserialized.mode.as_deref(), Some("report"));
        assert_eq!(deserialized.input_tokens, 500);
    }

    // -----------------------------------------------------------------------
    // AlertHistoryResponse contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn alert_history_response_with_items() {
        let response = AlertHistoryResponse {
            alerts: vec![
                ExecutionResponse {
                    id: 1,
                    watch_id: Some("w1".into()),
                    watch_name: Some("Watch 1".into()),
                    mode: Some("alert".into()),
                    started_at: "2025-01-15T09:00:00+00:00".into(),
                    completed_at: Some("2025-01-15T09:01:00+00:00".into()),
                    status: "success".into(),
                    agent_response: Some("Alert message".into()),
                    error_message: None,
                    input_tokens: 100,
                    output_tokens: 50,
                    alert_triggered: true,
                    notification_id: None,
                    execution_trace: None,
                    read_at: None,
                    deleted_at: None,
                    deleted_by: None,
                },
            ],
            total: 42,
            limit: 50,
            offset: 0,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["total"], 42);
        assert_eq!(json["limit"], 50);
        assert_eq!(json["offset"], 0);
        assert!(json["alerts"].is_array());
        assert_eq!(json["alerts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn alert_history_response_empty() {
        let response = AlertHistoryResponse {
            alerts: vec![],
            total: 0,
            limit: 50,
            offset: 0,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["total"], 0);
        assert_eq!(json["alerts"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn alert_history_response_round_trip() {
        let response = AlertHistoryResponse {
            alerts: vec![],
            total: 100,
            limit: 20,
            offset: 40,
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: AlertHistoryResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.total, 100);
        assert_eq!(deserialized.limit, 20);
        assert_eq!(deserialized.offset, 40);
    }

    // -----------------------------------------------------------------------
    // BulkAlertRequest contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn bulk_alert_request_with_ids() {
        let json = json!({"execution_ids": [1, 2, 3, 4, 5]});

        let req: BulkAlertRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.execution_ids, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn bulk_alert_request_empty_ids() {
        let json = json!({"execution_ids": []});

        let req: BulkAlertRequest = serde_json::from_value(json).unwrap();
        assert!(req.execution_ids.is_empty());
    }

    #[test]
    fn bulk_alert_request_fails_without_execution_ids() {
        let json = json!({});
        assert!(serde_json::from_value::<BulkAlertRequest>(json).is_err());
    }

    #[test]
    fn bulk_alert_request_round_trip() {
        let json = json!({"execution_ids": [10, 20, 30]});
        let req: BulkAlertRequest = serde_json::from_value(json).unwrap();
        let serialized = serde_json::to_value(&req).unwrap();
        let deserialized: BulkAlertRequest = serde_json::from_value(serialized).unwrap();
        assert_eq!(deserialized.execution_ids, vec![10, 20, 30]);
    }

    // -----------------------------------------------------------------------
    // BulkAlertResponse contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn bulk_alert_response_serializes() {
        let response = BulkAlertResponse { updated_count: 5 };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["updated_count"], 5);
    }

    #[test]
    fn bulk_alert_response_zero_count() {
        let response = BulkAlertResponse { updated_count: 0 };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["updated_count"], 0);
    }

    #[test]
    fn bulk_alert_response_round_trip() {
        let response = BulkAlertResponse { updated_count: 42 };
        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: BulkAlertResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.updated_count, 42);
    }

    // -----------------------------------------------------------------------
    // ContinueChatResponse contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn continue_chat_response_serializes() {
        let response = ContinueChatResponse {
            session_id: "sess-abc-123".into(),
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["session_id"], "sess-abc-123");
    }

    #[test]
    fn continue_chat_response_round_trip() {
        let response = ContinueChatResponse {
            session_id: "sess-xyz".into(),
        };
        let json_str = serde_json::to_string(&response).unwrap();
        let deserialized: ContinueChatResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.session_id, "sess-xyz");
    }

    // -----------------------------------------------------------------------
    // AlertsHistoryParams contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn alerts_history_params_defaults() {
        let json = json!({});
        let params: AlertsHistoryParams = serde_json::from_value(json).unwrap();
        assert!(params.watch_id.is_none());
        assert_eq!(params.limit, 50);
        assert_eq!(params.offset, 0);
        assert!(!params.include_deleted);
    }

    #[test]
    fn alerts_history_params_custom_values() {
        let json = json!({
            "watch_id": "watch-abc",
            "limit": 25,
            "offset": 10,
            "include_deleted": true
        });

        let params: AlertsHistoryParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.watch_id.as_deref(), Some("watch-abc"));
        assert_eq!(params.limit, 25);
        assert_eq!(params.offset, 10);
        assert!(params.include_deleted);
    }

    #[test]
    fn alerts_history_params_round_trip() {
        let json = json!({"limit": 10, "offset": 5});
        let params: AlertsHistoryParams = serde_json::from_value(json).unwrap();
        let serialized = serde_json::to_value(&params).unwrap();
        let deserialized: AlertsHistoryParams = serde_json::from_value(serialized).unwrap();
        assert_eq!(deserialized.limit, 10);
        assert_eq!(deserialized.offset, 5);
    }

    // -----------------------------------------------------------------------
    // ExecutionsParams contract tests
    // -----------------------------------------------------------------------

    #[test]
    fn executions_params_default_limit() {
        let json = json!({});
        let params: ExecutionsParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.limit, 20);
    }

    #[test]
    fn executions_params_custom_limit() {
        let json = json!({"limit": 50});
        let params: ExecutionsParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.limit, 50);
    }

    // -----------------------------------------------------------------------
    // Default function tests
    // -----------------------------------------------------------------------

    #[test]
    fn default_mode_is_alert() {
        assert_eq!(default_mode(), "alert");
    }

    #[test]
    fn default_alerts_limit_is_50() {
        assert_eq!(default_alerts_limit(), 50);
    }

    #[test]
    fn default_executions_limit_is_20() {
        assert_eq!(default_executions_limit(), 20);
    }
}
