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
    /// The message_id of the user message row already written to the DB.
    /// The caller must pass this to
    /// `kyomi_agent::UserMessagePersistence::CallerPersisted` so the agent
    /// loop knows this row is already durable and must not be persisted a
    /// second time (KYO-554).
    pub user_message_id: String,
    /// The message_id minted for the assistant's reply. No row is written
    /// for it yet — `kyomi_agent::adapter::ChatAgentAdapter::persist_after_chat`
    /// inserts the real assistant message (with its real content) once the
    /// agent loop finishes. Mirrors `chat_service::prepare_chat_dispatch`'s
    /// `assistant_message_id`, which never pre-inserted a placeholder either;
    /// this used to (KYO-572), and the pre-insert caused every real
    /// assistant write to collide on the `chat_messages.message_id` primary
    /// key and get silently dropped.
    pub assistant_message_id: String,
}

// ---------------------------------------------------------------------------
// Pre-spawn orchestration
// ---------------------------------------------------------------------------

/// Validate, check capabilities, verify session access, and store the user
/// message; also mints (but does not persist) the assistant message id.
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
    /// Where this message originated ("web" for the one production caller,
    /// `kyomi_ui::server_fns::copilot::send_copilot_message`), stored
    /// alongside `current_time_user_tz` so a later turn's context load can
    /// reconstruct the same `[source: X, user_local_time: Y]` annotation
    /// `agent.chat()` builds for the live LLM call (KYO-506, KYO-554). See
    /// `chat_service::ChatDispatchParams::message_source` for the same
    /// contract on the other production write site.
    pub message_source: Option<&'a str>,
}

/// Validate, check capabilities, verify session access, and store the user
/// message; also mints (but does not persist) the assistant message id.
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
        message_source,
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
    //
    // The id is minted explicitly (rather than left to `add_message`'s
    // auto-generation) so it can be returned to the caller, which passes it
    // to `kyomi_agent::UserMessagePersistence::CallerPersisted` — this is
    // what tells the agent loop the row is already durable and must not be
    // persisted again (KYO-554). Mirrors
    // `chat_service::prepare_chat_dispatch`'s `user_message_id` exactly.
    let user_message_id = uuid::Uuid::new_v4().to_string();
    crate::chat_service::add_message(
        db,
        encryption_key,
        session_id,
        "user",
        &user_message,
        None,
        Some(&user_message_id),
        current_time_user_tz,
        message_source,
        Some(user_id),
        None,
        None,
        None,
    )
    .await?;

    // Mint the assistant message id without writing a row for it.
    //
    // This used to INSERT an empty placeholder row here, which the agent's
    // `persist_after_chat` would supposedly "update" once it had the real
    // reply. It never did — `persist_after_chat` INSERTs, it never UPDATEs,
    // so that real INSERT collided on `chat_messages.message_id`'s primary
    // key, the error aborted persistence mid-loop, and the row stayed a
    // permanent empty placeholder while the error itself was swallowed
    // (KYO-572). Mirrors `chat_service::prepare_chat_dispatch`, which mints
    // `assistant_message_id` the same way and never pre-inserted a row for
    // it.
    let assistant_message_id = uuid::Uuid::new_v4().to_string();

    Ok(CopilotMessagePrep {
        user_message,
        user_message_id,
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

/// Record an agent execution error as the assistant's reply and send a
/// WebSocket error event to the user.
///
/// Called inside the `tokio::spawn` block when `execute_agent_chat` returns
/// an error, so the user sees a meaningful failure message rather than a
/// permanently missing assistant reply.
///
/// Tries to update an existing row for `assistant_message_id` first, and
/// inserts a new one if nothing was affected. Since KYO-572,
/// `prepare_copilot_message` no longer pre-inserts a placeholder row, so an
/// agent error occurring before `ChatAgentAdapter::persist_after_chat` has
/// written anything (e.g. the very first LLM call fails) means no row
/// exists yet for this id — a bare `update_message` would silently affect
/// zero rows and the user's error would never be persisted. Mirrors
/// `chat_service::save_agent_error`'s update-then-insert shape exactly.
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

    // Try update first (persist_after_chat may have already saved a row).
    let updated = crate::chat_service::update_message(
        db,
        encryption_key,
        assistant_message_id,
        Some(&error_text),
        Some(&error_metadata),
    )
    .await
    .unwrap_or(false);

    // If no row existed, insert a new one so the user sees the error in the
    // conversation instead of losing it silently.
    if !updated {
        let _ = crate::chat_service::add_message(
            db,
            encryption_key,
            session_id,
            "assistant",
            &error_text,
            Some(&error_metadata),
            Some(assistant_message_id),
            None, // current_time_user_tz
            None, // message_source
            None,
            None,
            None,
            None,
        )
        .await;
    }

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
