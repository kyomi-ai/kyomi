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

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

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
    let ac = AuthenticatedContext::extract().await?;

    let context_type = kyomi_agent::copilot::normalize_context_type(&context_type);
    let title = kyomi_agent::copilot::session_title_for_context(context_type);

    let session_id = sqlx::types::Uuid::new_v4().to_string();

    kyomi_auth::chat_service::create_session_with_id(
        ac.db(),
        &ac.auth.user_id,
        &ac.ws_id,
        &session_id,
        Some(title),
        context_type,
        None,
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
    let ac = AuthenticatedContext::extract().await?;

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

    let encryption_key = ac.encryption_key()?;

    // Validate capabilities, verify session access, store user message and
    // assistant placeholder — all via the shared service layer.
    let prep = kyomi_auth::copilot_service::prepare_copilot_message(
        kyomi_auth::copilot_service::CopilotMessageInputs {
            db: ac.db(),
            encryption_key: &encryption_key,
            config: &ac.ctx.config,
            workspace_id: &ac.ws_id,
            user_id: &ac.auth.user_id,
            session_id: &session_id,
            message: &message,
            content: content.as_deref(),
            current_time_user_tz: current_time_user_tz.as_deref(),
        },
    )
    .await
    .into_sfn()?;

    // ── Spawn AI agent execution ────────────────────────────────────────
    // Follows the same pattern as send_chat_message in chat.rs.

    let ws_manager = ac
        .ctx
        .ws_manager
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebSocket manager not configured"))?
        .clone();

    let cancel_registry = ac
        .ctx
        .cancel_registry
        .as_ref()
        .ok_or_else(|| ServerFnError::new("Cancel registry not configured"))?
        .clone();

    let platforms = ac
        .ctx
        .platforms
        .as_ref()
        .ok_or_else(|| ServerFnError::new("Platform registry not configured"))?
        .clone();

    let user_timezone = timezone.as_deref().unwrap_or("UTC");
    let system_prompt = kyomi_agent::copilot::build_copilot_system_prompt(
        context_type,
        user_timezone,
        ac.auth.name.as_deref(),
    );

    let cancel_token = tokio_util::sync::CancellationToken::new();

    let exec_config = kyomi_agent::AgentExecutionConfig {
        session_id: session_id.clone(),
        user_id: ac.auth.user_id.clone(),
        workspace_id: ac.ws_id.clone(),
        message: prep.user_message,
        model_name: None,
        temperature: 0.1,
        is_shared_conversation: false,
        context_type: context_type.to_string(),
        workspace_user_ids: None,
        cancel_token: cancel_token.clone(),
        current_time_user_tz: current_time_user_tz.clone(),
        message_source: Some("web".to_string()),
        system_prompt: Some(system_prompt),
        tools_subset: Some(kyomi_agent::copilot::tools_for_context(context_type)),
        // Copilot: inline assist — a long loop is a UX failure regardless of
        // cost, so it keeps the tightest guards of any surface (KYO-345).
        max_iterations: 20,
        max_duration: Some(std::time::Duration::from_secs(3 * 60)),
        max_total_tokens: Some(400_000),
        component: context_type.to_string(),
        user_message_persistence: kyomi_agent::UserMessagePersistence::AdapterPersists(None),
        assistant_message_id: Some(prep.assistant_message_id.clone()),
        conversation_history: None,
        user_display_name: ac.auth.name.clone().unwrap_or_else(|| ac.auth.email.clone()),
        context_window: 0,
        workspace_roles: ac.auth.workspace.workspace_roles.clone(),
    };

    cancel_registry.register(&ac.auth.user_id, &session_id, cancel_token.clone());

    let db = ac.db().clone();
    let kv = ac.kv()?;
    let embedding = ac.ctx.embedding.clone();
    let app_config = ac.ctx.config.clone();
    let connect_registry = ac.ctx.connect_registry.clone();
    let spawn_user_id = ac.auth.user_id.clone();
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
            Ok(exec_result) if exec_result.status == "cancelled" => {
                // Notify the frontend that the request was cancelled so it can
                // transition out of Cancelling state. Do NOT call deliver_response.
                kyomi_auth::websocket::helpers::send_request_cancelled(
                    &ws_manager,
                    &spawn_user_id,
                    &spawn_session_id,
                    &exec_result.assistant_message_id,
                    Some(&spawn_context_type),
                )
                .await;
            }
            Ok(exec_result) => {
                kyomi_agent::deliver_response(
                    &ws_manager,
                    &spawn_user_id,
                    &spawn_session_id,
                    &exec_result.assistant_message_id,
                    &exec_result.response_text,
                    exec_result.model.as_deref().unwrap_or("unknown"),
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
                        error: e.user_message(),
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
    let ac = AuthenticatedContext::extract().await?;

    let deleted = kyomi_auth::chat_service::delete_session(
        ac.db(),
        &ac.auth.user_id,
        &session_id,
        Some(&ac.ws_id),
    )
    .await
    // user_message() (KYO-448) — Display would leak the variant tag.
    .map_err(|e| ServerFnError::new(format!("Failed to delete copilot session: {}", e.user_message())))?;

    if deleted {
        tracing::info!(session_id = %session_id, "Deleted copilot session");
    }

    Ok(())
}
