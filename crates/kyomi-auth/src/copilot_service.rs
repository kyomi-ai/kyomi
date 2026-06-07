// SPDX-License-Identifier: AGPL-3.0-or-later

//! Copilot service — pre-spawn orchestration for copilot message handling.
//!
//! Extracts the shared orchestration that `send_copilot_message` (Leptos
//! server_fn) performs before handing off to the async agent executor.
//! Keeping this in `kyomi-auth` lets both the server_fn path and any future
//! REST path share the same logic without duplicating it.
//!
//! The agent execution itself (`kyomi_agent::execute_agent_chat`) and the
//! success delivery path cannot live here because `kyomi-agent` depends on
//! `kyomi-auth`, not the other way around — adding `kyomi-agent` as a
//! dependency of `kyomi-auth` would create a circular dependency.

use kyomi_core::{Config, DbPool};

use crate::websocket::WebSocketManager;

// ---------------------------------------------------------------------------
// Result type for prepare_copilot_message
// ---------------------------------------------------------------------------

/// Data produced by [`prepare_copilot_message`] and consumed by the caller
/// to configure and spawn agent execution.
pub struct CopilotMessagePrep {
    /// The full user message with injected context content.
    pub user_message: String,
    /// The message_id of the assistant placeholder row, which the agent will
    /// update when it finishes.
    pub assistant_message_id: String,
}

// ---------------------------------------------------------------------------
// Pre-spawn orchestration
// ---------------------------------------------------------------------------

/// Validate, check capabilities, verify session access, and store both the
/// user message and the assistant placeholder in one atomic service call.
///
/// Inputs for [`prepare_copilot_message`].
pub struct CopilotMessageInputs<'a> {
    pub db: &'a DbPool,
    pub encryption_key: &'a [u8; 32],
    pub config: &'a Config,
    pub workspace_id: &'a str,
    pub user_id: &'a str,
    pub session_id: &'a str,
    pub message: &'a str,
    pub content: Option<&'a str>,
    pub current_time_user_tz: Option<&'a str>,
}

/// Validate, check capabilities, verify session access, and store both the
/// user message and the assistant placeholder in one atomic service call.
pub async fn prepare_copilot_message(
    inputs: CopilotMessageInputs<'_>,
) -> kyomi_core::Result<CopilotMessagePrep> {
    let CopilotMessageInputs {
        db,
        encryption_key,
        config,
        workspace_id,
        user_id,
        session_id,
        message,
        content,
        current_time_user_tz,
    } = inputs;
    // Check AI capability.
    if !config.llm_configured() {
        return Err(kyomi_core::Error::Internal(
            "No LLM provider configured. Add ANTHROPIC_API_KEY or LLM_API_KEY to your environment."
                .to_string(),
        ));
    }

    let workspace = crate::workspace_service::get_workspace_full(db, workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("Workspace not found".to_string()))?;

    let capabilities = if config.self_hosted {
        kyomi_core::capability::compute_capabilities_self_hosted()
    } else {
        kyomi_core::capability::compute_capabilities(&workspace)
    };

    if !capabilities.ai_chat_enabled {
        return Err(kyomi_core::Error::Internal(
            "You have exceeded AI usage limits".to_string(),
        ));
    }

    // Verify session access.
    let session = crate::chat_service::get_session_info(db, user_id, session_id, Some(workspace_id))
        .await?;

    if session.is_none() {
        return Err(kyomi_core::Error::Internal(
            "Session not found or access denied".to_string(),
        ));
    }

    // Build user message with content context injection.
    let user_message = if let Some(ctx_content) = content {
        format!("{ctx_content}\n\n{message}")
    } else {
        message.to_string()
    };

    // Store user message.
    let _user_message_id = crate::chat_service::add_message(
        db,
        encryption_key,
        session_id,
        "user",
        &user_message,
        None,
        None,
        current_time_user_tz,
        Some(user_id),
        None,
        None,
        None,
    )
    .await?;

    // Create assistant placeholder.
    let assistant_message_id = crate::chat_service::add_message(
        db,
        encryption_key,
        session_id,
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

    Ok(CopilotMessagePrep {
        user_message,
        assistant_message_id,
    })
}

// ---------------------------------------------------------------------------
// Spawn error handler
// ---------------------------------------------------------------------------

/// Parameters for `handle_copilot_agent_error`.
pub struct CopilotAgentErrorParams<'a> {
    pub db: &'a DbPool,
    pub encryption_key: &'a [u8; 32],
    pub ws_manager: &'a WebSocketManager,
    pub user_id: &'a str,
    pub session_id: &'a str,
    pub assistant_message_id: &'a str,
    pub context_type: &'a str,
    pub error: &'a str,
}

/// Update the assistant placeholder with an error message and send a
/// WebSocket error event to the user.
///
/// Called inside the `tokio::spawn` block when `execute_agent_chat` returns
/// an error, so the user sees a meaningful failure message rather than a
/// permanently empty assistant bubble.
pub async fn handle_copilot_agent_error(params: CopilotAgentErrorParams<'_>) {
    let CopilotAgentErrorParams {
        db, encryption_key, ws_manager, user_id, session_id,
        assistant_message_id, context_type, error,
    } = params;
    let error_text = format!("I encountered an error while processing your request: {error}");
    let error_metadata = serde_json::json!({
        "status": "error",
        "error": error,
    });

    let _ = crate::chat_service::update_message(
        db,
        encryption_key,
        assistant_message_id,
        Some(&error_text),
        Some(&error_metadata),
    )
    .await;

    crate::websocket::helpers::send_error(
        ws_manager,
        user_id,
        Some(session_id),
        &format!("AI processing failed: {error}"),
        Some("agent_error"),
        Some(context_type),
    )
    .await;
}
