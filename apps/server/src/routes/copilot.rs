// SPDX-License-Identifier: AGPL-3.0-or-later

//! Copilot API endpoints.
//!
//! Wire-compatible with Python's `routers/copilot.py`.
//! Provides conversational AI for editing dashboards, charts, and watches.
//! Reuses the core chat agent infrastructure with specialized system prompts
//! and context-specific tools.
//!
//! ## Endpoints
//!
//! - `POST   /message`               — send_copilot_message
//! - `DELETE /session/{session_id}`   — delete_copilot_session

use axum::{
    extract::{Path, State},
    routing::{delete, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use kyomi_auth::{chat_service, middleware::AuthUser, workspace_service};
use kyomi_core::capability;

use crate::state::AppState;

/// Build the `/chat/copilot` router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/message", post(send_copilot_message))
        .route("/session/{session_id}", delete(delete_copilot_session))
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Extract workspace_id from user, or return 400.
fn get_workspace_id(user: &AuthUser) -> Result<&str, kyomi_core::Error> {
    user.workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("User not associated with a workspace".into()))
}

// ===========================================================================
// Request Types
// ===========================================================================

#[derive(Deserialize)]
struct CopilotMessageRequest {
    message: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    context: Option<CopilotContext>,
    #[serde(default)]
    current_time_user_tz: Option<String>,
}

#[derive(Deserialize, Default)]
struct CopilotContext {
    /// Context type: dashboard_copilot | chart_builder_copilot | watch_copilot
    #[serde(default, rename = "type")]
    context_type: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    /// Full dashboard markdown content (camelCase from frontend).
    #[serde(default, rename = "dashboardContent")]
    dashboard_content: Option<String>,
    /// Single ChartML block content.
    #[serde(default, rename = "chartContent")]
    chart_content: Option<String>,
    /// Watch configuration JSON.
    #[serde(default, rename = "watchConfig")]
    watch_config: Option<serde_json::Value>,
}

// ===========================================================================
// Shared copilot configuration (prompts, tools) — re-exported from kyomi-agent
// ===========================================================================

use kyomi_agent::copilot::{build_copilot_system_prompt, tools_for_context};

// Prompt builders, tool subsets, and context helpers are in kyomi_agent::copilot module
// (shared between this REST route and Leptos server functions).

// ===========================================================================
// Endpoint Handlers
// ===========================================================================

// ---------------------------------------------------------------------------
// POST /message — Send a copilot message + trigger AI response
// ---------------------------------------------------------------------------

async fn send_copilot_message(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<CopilotMessageRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Validate message.
    if request.message.trim().is_empty() {
        return Err(kyomi_core::Error::BadRequest(
            "Message content cannot be empty".into(),
        ));
    }
    if request.message.len() > 100_000 {
        return Err(kyomi_core::Error::BadRequest(
            "Message content exceeds maximum length".into(),
        ));
    }

    // Check AI capability (credits not exhausted).
    let workspace = workspace_service::get_workspace_full(&state.db, workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Workspace not found".into()))?;
    let capabilities = if state.config.self_hosted {
        capability::compute_capabilities_self_hosted()
    } else {
        capability::compute_capabilities(&workspace)
    };
    if !capabilities.ai_chat_enabled {
        return Err(kyomi_core::Error::Forbidden(
            "AI features are not available. Your budget may be exhausted or your plan \
             doesn't include this feature."
                .into(),
        ));
    }

    // Parse context.
    let ctx = request.context.unwrap_or_default();
    let context_type = ctx
        .context_type
        .as_deref()
        .unwrap_or("dashboard_copilot");

    // Validate context type.
    let context_type = match context_type {
        "dashboard_copilot" | "chart_builder_copilot" | "watch_copilot" => context_type,
        _ => "dashboard_copilot",
    };

    let user_timezone = ctx.timezone.as_deref().unwrap_or("UTC");
    let user_name = user.name.as_deref();

    // Get or create session.
    let is_new_session = request.session_id.is_none();
    let session_id = if let Some(ref sid) = request.session_id {
        // Verify user has access to this session.
        let session = chat_service::get_session_info(
            &state.db,
            &user.user_id,
            sid,
            Some(workspace_id),
        )
        .await?;

        match session {
            Some(_) => sid.clone(),
            None => {
                return Err(kyomi_core::Error::NotFound(
                    "Session not found or access denied".into(),
                ));
            }
        }
    } else {
        // Create new copilot session.
        let new_sid = uuid::Uuid::new_v4().to_string();

        let session_title = match context_type {
            "chart_builder_copilot" => "Chart Builder Copilot",
            "watch_copilot" => "Watch Copilot",
            _ => "Dashboard Copilot",
        };

        chat_service::create_session_with_id(
            &state.db,
            &user.user_id,
            workspace_id,
            &new_sid,
            Some(session_title),
            context_type,
        )
        .await?;

        tracing::info!(
            "Created new {} session: {}",
            context_type,
            new_sid
        );

        new_sid
    };

    // Build user message with content injection.
    // For watch_copilot, serialize JSON config to a string first (needs to outlive the match).
    let watch_content_string = ctx
        .watch_config
        .as_ref()
        .map(|v| serde_json::to_string_pretty(v).unwrap_or_default());

    let (effective_content, content_label, content_update_label) = match context_type {
        "chart_builder_copilot" => (
            ctx.chart_content.as_deref(),
            "Chart Content",
            "Chart has been updated",
        ),
        "watch_copilot" => (
            watch_content_string.as_deref(),
            "Watch Configuration",
            "Watch has been updated",
        ),
        _ => (
            ctx.dashboard_content.as_deref(),
            "Dashboard Content",
            "Dashboard has been updated",
        ),
    };

    let user_message = if let Some(c) = effective_content {
        if is_new_session {
            format!("[{content_label}]\n{c}\n\n{}", request.message)
        } else {
            format!("[{content_update_label}]\n{c}\n\n{}", request.message)
        }
    } else {
        request.message.clone()
    };

    // Store user message.
    let user_message_id = chat_service::add_message(
        &state.db,
        &state.encryption_key,
        &session_id,
        "user",
        &user_message,
        None,  // metadata
        None,  // message_id (auto-generate)
        request.current_time_user_tz.as_deref(),
        Some(&user.user_id),
        None,  // tool_call_id
        None,  // tool_name
        None,  // tool_calls
    )
    .await?;

    // Create assistant placeholder.
    let assistant_message_id = chat_service::add_message(
        &state.db,
        &state.encryption_key,
        &session_id,
        "assistant",
        "",
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await?;

    // Build system prompt.
    let system_prompt = build_copilot_system_prompt(context_type, user_timezone, user_name);

    // Build execution config.
    let cancel_token = tokio_util::sync::CancellationToken::new();

    let exec_config = kyomi_agent::AgentExecutionConfig {
        session_id: session_id.clone(),
        user_id: user.user_id.clone(),
        workspace_id: workspace_id.to_string(),
        message: user_message.clone(),
        model_name: Some("claude-haiku-4-5-20251001".to_string()),
        temperature: 0.1,
        is_shared_conversation: false,
        context_type: context_type.to_string(),
        workspace_user_ids: None,
        cancel_token: cancel_token.clone(),
        current_time_user_tz: request.current_time_user_tz.clone(),
        message_source: Some("web".to_string()),
        system_prompt: Some(system_prompt),
        tools_subset: Some(tools_for_context(context_type)),
        max_iterations: 20,
        component: context_type.to_string(),
        user_message_id: None,
        assistant_message_id: Some(assistant_message_id.clone()),
        conversation_history: None,
        user_display_name: user.name.as_deref().unwrap_or(&user.email).to_string(),
    };

    // Register cancel token so WebSocket cancel_request can stop this task.
    state
        .cancel_registry
        .register(&user.user_id, &session_id, cancel_token.clone());

    // Spawn async task for AI execution + response delivery.
    let db = state.db.clone();
    let kv = state.kv.clone();

    let encryption_key = state.encryption_key.clone();
    let embedding = state.embedding.clone();
    let ws_manager = state.ws_manager.clone();
    let app_config = state.config.clone();
    let cancel_registry = state.cancel_registry.clone();
    let connect_registry = state.connect_registry.clone();
    let platforms = state.platforms.clone();
    let spawn_user_id = user.user_id.clone();
    let spawn_session_id = session_id.clone();
    let spawn_assistant_message_id = assistant_message_id.clone();
    let spawn_context_type = context_type.to_string();

    tokio::spawn(async move {
        let result = kyomi_agent::execute_agent_chat(
            exec_config,
            &db,
            &kv,
            &encryption_key,
            &embedding,
            &ws_manager,
            &app_config,
            Some(connect_registry),
            platforms,
        )
        .await;

        match result {
            Ok(exec_result) => {
                kyomi_agent::deliver_response(
                    &ws_manager,
                    &spawn_user_id,
                    &spawn_session_id,
                    &exec_result.assistant_message_id,
                    &exec_result.response_text,
                    exec_result
                        .model
                        .as_deref()
                        .unwrap_or(kyomi_agent::DEFAULT_MODEL),
                    exec_result.token_usage,
                    &spawn_context_type,
                    None,
                    None,
                    None, // trial_session_id: not a trial chat
                )
                .await;
            }
            Err(e) => {
                tracing::error!(
                    session_id = %spawn_session_id,
                    error = %e,
                    "Copilot agent execution failed"
                );

                // Update assistant placeholder with error text.
                let error_text = format!(
                    "I encountered an error while processing your request: {e}"
                );
                let error_metadata = json!({
                    "status": "error",
                    "error": e.to_string(),
                });
                let _ = chat_service::update_message(
                    &db,
                    &encryption_key,
                    &spawn_assistant_message_id,
                    Some(&error_text),
                    Some(&error_metadata),
                )
                .await;

                kyomi_auth::websocket::helpers::send_error(
                    &ws_manager,
                    &spawn_user_id,
                    Some(&spawn_session_id),
                    &format!("Copilot error: {e}"),
                    Some("copilot_error"),
                    Some(&spawn_context_type),
                )
                .await;
            }
        }

        // Clean up cancel token so it doesn't leak.
        cancel_registry.remove(&spawn_user_id, &spawn_session_id);
    });

    // No title generation for copilot sessions (they have fixed titles).

    tracing::info!(
        "Copilot message stored in session {} (AI processing spawned)",
        session_id
    );

    Ok(Json(json!({
        "session_id": session_id,
        "message_id": assistant_message_id,
        "user_message_id": user_message_id,
        "status": "processing",
    })))
}

// ---------------------------------------------------------------------------
// DELETE /session/{session_id} — Delete a copilot session
// ---------------------------------------------------------------------------

async fn delete_copilot_session(
    State(state): State<AppState>,
    user: AuthUser,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let deleted = chat_service::delete_session(
        &state.db,
        &user.user_id,
        &session_id,
        Some(workspace_id),
    )
    .await?;

    if deleted {
        tracing::info!("Deleted copilot session {} for user {}", session_id, user.user_id);
    } else {
        // Session already deleted or doesn't exist — that's fine for copilot cleanup.
        tracing::info!("Copilot session {} not found (already deleted?)", session_id);
    }

    // Always return success (matches Python behavior where missing = success).
    Ok(Json(json!({
        "success": true,
        "message": "Session deleted",
    })))
}
