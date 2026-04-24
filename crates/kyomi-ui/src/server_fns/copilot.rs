// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for copilot sessions (dashboard, chart builder, watch).
//!
//! All copilot types share the same infrastructure:
//! 1. `create_copilot_session` — create an ephemeral session for any context type
//! 2. `send_copilot_message` — send a user message + spawn AI agent execution
//! 3. `delete_copilot_session` — cleanup a session
//!
//! The AI agent runs asynchronously — responses stream back via WebSocket events
//! (`agent_thinking`, `chat_stream`, `chat_complete`, plus context-specific events
//! like `chart_update` or `dashboard_update`).
//!
//! Follows the same `AgentExecutionConfig` + `execute_agent_chat` pattern as
//! `send_chat_message` in `chat.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// Response from the copilot after submitting a user message.
///
/// The agent runs asynchronously — the actual AI content arrives via WebSocket
/// streaming events, not in this HTTP response.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CopilotResponse {
    pub status: String,
    pub message: String,
    pub suggested_content: Option<String>,
}

/// Create an ephemeral copilot session.
///
/// `context_type` must be one of: `"dashboard_copilot"`, `"chart_builder_copilot"`,
/// `"watch_copilot"`. Defaults to `"dashboard_copilot"` if unrecognized.
///
/// Returns the new session ID.
#[server(prefix = "/leptos-api")]
pub async fn create_copilot_session(
    context_type: String,
) -> Result<String, ServerFnError> {
    let auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;
    let workspace_id = super::workspace_id(&auth)?;

    let context_type = kyomi_agent::copilot::normalize_context_type(&context_type);
    let title = kyomi_agent::copilot::session_title_for_context(context_type);

    let session_id = sqlx::types::Uuid::new_v4().to_string();

    kyomi_auth::chat_service::create_session_with_id(
        &ctx.db,
        &auth.user_id,
        workspace_id,
        &session_id,
        Some(title),
        context_type,
    )
    .await
    .map_err(|e| ServerFnError::new(format!("Failed to create copilot session: {e}")))?;

    tracing::info!(
        session_id = %session_id,
        context_type = %context_type,
        "Created new copilot session"
    );

    Ok(session_id)
}

/// Send a message to the copilot and trigger AI agent execution.
///
/// Works for all copilot context types. The `context_type` determines which
/// system prompt and tool subset the agent uses.
///
/// The `content` parameter carries context-specific data:
/// - Dashboard copilot: the dashboard markdown (prefixed with `[Dashboard Content]`)
/// - Chart copilot: the chart YAML (prefixed with `[Chart Content]`)
/// - Watch copilot: the watch config JSON (prefixed with `[Watch Configuration]`)
///
/// The component is responsible for prefixing content appropriately.
#[server(prefix = "/leptos-api")]
pub async fn send_copilot_message(
    session_id: String,
    message: String,
    context_type: String,
    content: Option<String>,
    timezone: Option<String>,
    current_time_user_tz: Option<String>,
) -> Result<CopilotResponse, ServerFnError> {
    let auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;
    let workspace_id = super::workspace_id(&auth)?;

    // Validate message.
    if message.trim().is_empty() {
        return Err(ServerFnError::new("Message content cannot be empty"));
    }
    if message.len() > 100_000 {
        return Err(ServerFnError::new(
            "Message content exceeds maximum length",
        ));
    }

    // Normalize context type.
    let context_type = kyomi_agent::copilot::normalize_context_type(&context_type);

    let encryption_key = ctx
        .encryption_key
        .as_ref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    // Validate capabilities, verify session access, store user message and
    // assistant placeholder — all via the shared service layer.
    let prep = kyomi_auth::copilot_service::prepare_copilot_message(
        kyomi_auth::copilot_service::CopilotMessageInputs {
            db: &ctx.db,
            encryption_key,
            config: &ctx.config,
            workspace_id,
            user_id: &auth.user_id,
            session_id: &session_id,
            message: &message,
            content: content.as_deref(),
            current_time_user_tz: current_time_user_tz.as_deref(),
        },
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // ── Spawn AI agent execution ────────────────────────────────────────
    // Follows the same pattern as send_chat_message in chat.rs.

    let ws_manager = ctx
        .ws_manager
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebSocket manager not configured"))?
        .clone();

    let cancel_registry = ctx
        .cancel_registry
        .as_ref()
        .ok_or_else(|| ServerFnError::new("Cancel registry not configured"))?
        .clone();

    let platforms = ctx
        .platforms
        .as_ref()
        .ok_or_else(|| ServerFnError::new("Platform registry not configured"))?
        .clone();

    let user_timezone = timezone.as_deref().unwrap_or("UTC");
    let system_prompt = kyomi_agent::copilot::build_copilot_system_prompt(
        context_type,
        user_timezone,
        auth.name.as_deref(),
    );

    let cancel_token = tokio_util::sync::CancellationToken::new();

    let exec_config = kyomi_agent::AgentExecutionConfig {
        session_id: session_id.clone(),
        user_id: auth.user_id.clone(),
        workspace_id: workspace_id.to_string(),
        message: prep.user_message,
        model_name: Some("claude-haiku-4-5-20251001".to_string()),
        temperature: 0.1,
        is_shared_conversation: false,
        context_type: context_type.to_string(),
        workspace_user_ids: None,
        cancel_token: cancel_token.clone(),
        current_time_user_tz: current_time_user_tz.clone(),
        message_source: Some("web".to_string()),
        system_prompt: Some(system_prompt),
        tools_subset: Some(kyomi_agent::copilot::tools_for_context(context_type)),
        max_iterations: 20,
        component: context_type.to_string(),
        user_message_id: None,
        assistant_message_id: Some(prep.assistant_message_id.clone()),
        conversation_history: None,
        user_display_name: auth.name.clone().unwrap_or_else(|| auth.email.clone()),
    };

    cancel_registry.register(&auth.user_id, &session_id, cancel_token.clone());

    let db = ctx.db.clone();
    let kv = ctx
        .kv
        .as_ref()
        .ok_or_else(|| ServerFnError::new("KV store not configured"))?
        .clone();
    let encryption_key = encryption_key.clone();
    let embedding = ctx.embedding.clone();
    let app_config = ctx.config.clone();
    let connect_registry = ctx.connect_registry.clone();
    let spawn_user_id = auth.user_id.clone();
    let spawn_session_id = session_id.clone();
    let spawn_assistant_message_id = prep.assistant_message_id.clone();
    let spawn_context_type = context_type.to_string();
    let spawn_cancel_registry = cancel_registry;

    tokio::spawn(async move {
        let result = kyomi_agent::execute_agent_chat(
            exec_config,
            kyomi_agent::AgentExecutionEnv {
                db: &db,
                kv: &kv,
                encryption_key: &encryption_key,
                embedding: &embedding,
                ws_manager: &ws_manager,
                app_config: &app_config,
                connect_registry,
                platforms,
            },
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
                )
                .await;
            }
            Err(e) => {
                tracing::error!(
                    session_id = %spawn_session_id,
                    error = %e,
                    "Copilot agent execution failed"
                );

                kyomi_auth::copilot_service::handle_copilot_agent_error(
                    kyomi_auth::copilot_service::CopilotAgentErrorParams {
                        db: &db,
                        encryption_key: &encryption_key,
                        ws_manager: &ws_manager,
                        user_id: &spawn_user_id,
                        session_id: &spawn_session_id,
                        assistant_message_id: &spawn_assistant_message_id,
                        context_type: &spawn_context_type,
                        error: &e.to_string(),
                    },
                )
                .await;
            }
        }

        // Clean up cancel token so it doesn't leak.
        spawn_cancel_registry.remove(&spawn_user_id, &spawn_session_id);
    });

    tracing::info!(
        session_id = %session_id,
        context_type = %context_type,
        "Copilot message accepted — agent execution spawned"
    );

    Ok(CopilotResponse {
        status: "processing".to_string(),
        message: String::new(),
        suggested_content: None,
    })
}

/// Delete/cleanup a copilot session.
///
/// Called when the copilot sidebar/modal closes to clean up the ephemeral session.
#[server(prefix = "/leptos-api")]
pub async fn delete_copilot_session(session_id: String) -> Result<(), ServerFnError> {
    let auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;
    let workspace_id = super::workspace_id(&auth)?;

    let deleted = kyomi_auth::chat_service::delete_session(
        &ctx.db,
        &auth.user_id,
        &session_id,
        Some(workspace_id),
    )
    .await
    .map_err(|e| ServerFnError::new(format!("Failed to delete copilot session: {e}")))?;

    if deleted {
        tracing::info!(session_id = %session_id, "Deleted copilot session");
    }

    Ok(())
}
