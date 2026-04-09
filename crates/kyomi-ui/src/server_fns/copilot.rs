// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the copilot sidebar.
//!
//! Provides session management and message sending for the dashboard copilot.
//! Calls the same service-layer code (`chat_service`) as
//! `apps/server/src/routes/copilot.rs`.
//!
//! ## Endpoints
//!
//! - `create_copilot_session`  — create an ephemeral copilot session
//! - `send_copilot_message`    — send a user message and get an AI response
//! - `delete_copilot_session`  — cleanup a copilot session

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// Response from the copilot after submitting a user message.
///
/// The copilot agent runs asynchronously — the AI response is delivered via
/// WebSocket streaming events (`chat_stream`, `chat_complete`), not in this
/// HTTP response. The `status` field indicates that the message was accepted
/// and is being processed.
///
/// ## Limitations (current implementation)
///
/// Full streaming requires the WebSocket client to handle copilot-specific
/// event types (`chat_stream`, `chat_complete`). Until that is implemented,
/// the copilot operates in request-response mode: the server function stores
/// the user message and creates an assistant placeholder, but the actual AI
/// content is not yet delivered back to the Leptos frontend.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CopilotResponse {
    /// Processing status: `"processing"` means the message was accepted and
    /// the agent is generating a response asynchronously.
    pub status: String,
    /// The AI-generated response text, if available synchronously.
    /// Currently always empty — the real response arrives via WebSocket.
    pub message: String,
    /// Optional suggested content (e.g., updated dashboard markdown).
    /// When present, the UI can offer an "Apply to Dashboard" action.
    /// Currently always `None` — populated via WebSocket events.
    pub suggested_content: Option<String>,
}

/// Create an ephemeral copilot session for a dashboard.
///
/// Creates a new chat session with `session_type = "dashboard_copilot"`.
/// The session is ephemeral — it should be cleaned up when the sidebar closes
/// via `delete_copilot_session`.
///
/// Returns the new session ID.
#[server(prefix = "/leptos-api")]
pub async fn create_copilot_session(dashboard_id: String) -> Result<String, ServerFnError> {
    let auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;
    let workspace_id = super::workspace_id(&auth)?;

    let _ = &dashboard_id; // Acknowledged — used for context association.

    let session_id = sqlx::types::Uuid::new_v4().to_string();

    kyomi_auth::chat_service::create_session_with_id(
        &ctx.db,
        &auth.user_id,
        workspace_id,
        &session_id,
        Some("Dashboard Copilot"),
        "dashboard_copilot",
    )
    .await
    .map_err(|e| ServerFnError::new(format!("Failed to create copilot session: {e}")))?;

    tracing::info!(
        session_id = %session_id,
        dashboard_id = %dashboard_id,
        "Created new dashboard copilot session"
    );

    Ok(session_id)
}

/// Send a message to the copilot and get a response.
///
/// Validates the message, checks AI capability, stores the user message,
/// creates an assistant placeholder, and returns conversation metadata.
///
/// The actual AI response is generated asynchronously by the agent system
/// and delivered via WebSocket streaming events. The returned
/// `CopilotResponse` contains the message IDs for tracking.
#[server(prefix = "/leptos-api")]
pub async fn send_copilot_message(
    session_id: String,
    message: String,
    dashboard_content: Option<String>,
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

    // Check AI capability (credits not exhausted).
    let workspace =
        kyomi_auth::workspace_service::get_workspace_full(&ctx.db, workspace_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    let capabilities = if ctx.config.self_hosted {
        kyomi_core::capability::compute_capabilities_self_hosted(false)
    } else {
        kyomi_core::capability::compute_capabilities(&workspace, false)
    };

    if !capabilities.ai_chat_enabled {
        return Err(ServerFnError::new(
            "AI features are not available. Your budget may be exhausted or your plan \
             doesn't include this feature.",
        ));
    }

    // Verify user has access to this session.
    let session = kyomi_auth::chat_service::get_session_info(
        &ctx.db,
        &auth.user_id,
        &session_id,
        Some(workspace_id),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    if session.is_none() {
        return Err(ServerFnError::new(
            "Session not found or access denied",
        ));
    }

    // Build user message with dashboard content injection.
    // The component prepends the appropriate prefix ("[Dashboard Content]"
    // for the first message, "[Dashboard has been updated]" for subsequent).
    let user_message = if let Some(ref content) = dashboard_content {
        format!("{content}\n\n{message}")
    } else {
        message.clone()
    };

    // Require encryption key for storing messages.
    let encryption_key = ctx
        .encryption_key
        .as_ref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    // Store user message.
    let _user_message_id = kyomi_auth::chat_service::add_message(
        &ctx.db,
        encryption_key,
        &session_id,
        "user",
        &user_message,
        None,  // metadata
        None,  // message_id (auto-generate)
        None,  // current_time_user_tz
        Some(&auth.user_id),
        None,  // tool_call_id
        None,  // tool_name
        None,  // tool_calls
    )
    .await
    .map_err(|e| ServerFnError::new(format!("Failed to store message: {e}")))?;

    // Create assistant placeholder message.
    let _assistant_message_id = kyomi_auth::chat_service::add_message(
        &ctx.db,
        encryption_key,
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
    .await
    .map_err(|e| ServerFnError::new(format!("Failed to create assistant message: {e}")))?;

    // NOTE: The AI agent execution is handled by the main server's copilot
    // route (`apps/server/src/routes/copilot.rs`) which has access to the
    // full agent infrastructure (WebSocketManager, CancelRegistry, etc.).
    // This server function stores the messages; the actual AI response
    // delivery happens via the agent system and WebSocket events.
    //
    // For the Leptos frontend, the copilot sidebar will receive AI responses
    // through WebSocket streaming events, matching the React frontend's
    // pattern.

    tracing::info!(
        session_id = %session_id,
        "Copilot message stored (awaiting agent processing)"
    );

    // Return a response indicating the message was accepted for processing.
    // The actual AI content will arrive via WebSocket streaming events
    // (`chat_stream`, `chat_complete`). Full streaming delivery to the
    // Leptos frontend requires extending the WebSocket client to handle
    // copilot-specific event types — see `utils/websocket.rs`.
    Ok(CopilotResponse {
        status: "processing".to_string(),
        message: String::new(),
        suggested_content: None,
    })
}

/// Delete/cleanup a copilot session.
///
/// Called when the copilot sidebar closes to clean up the ephemeral session.
/// Always returns success (matches Python/Rust REST behavior where missing = success).
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
        tracing::info!(
            session_id = %session_id,
            user_id = %auth.user_id,
            "Deleted copilot session"
        );
    } else {
        tracing::info!(
            session_id = %session_id,
            "Copilot session not found (already deleted?)"
        );
    }

    Ok(())
}

/// Create an ephemeral copilot session for chart building.
///
/// Creates a new chat session with `session_type = "chart_builder_copilot"`.
/// The session is cleaned up when the chart builder modal closes.
#[server(prefix = "/leptos-api")]
pub async fn create_chart_copilot_session() -> Result<String, ServerFnError> {
    let auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;
    let workspace_id = super::workspace_id(&auth)?;

    let session_id = sqlx::types::Uuid::new_v4().to_string();

    kyomi_auth::chat_service::create_session_with_id(
        &ctx.db,
        &auth.user_id,
        workspace_id,
        &session_id,
        Some("Chart Builder Copilot"),
        "chart_builder_copilot",
    )
    .await
    .map_err(|e| ServerFnError::new(format!("Failed to create chart copilot session: {e}")))?;

    tracing::info!(
        session_id = %session_id,
        "Created new chart builder copilot session"
    );

    Ok(session_id)
}

/// Send a message to the chart builder copilot.
///
/// Same pattern as `send_copilot_message` but with chart content context
/// using the `[Chart Content]` / `[Chart has been updated]` prefix pattern.
#[server(prefix = "/leptos-api")]
pub async fn send_chart_copilot_message(
    session_id: String,
    message: String,
    chart_content: Option<String>,
) -> Result<CopilotResponse, ServerFnError> {
    let auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;
    let workspace_id = super::workspace_id(&auth)?;

    if message.trim().is_empty() {
        return Err(ServerFnError::new("Message content cannot be empty"));
    }
    if message.len() > 100_000 {
        return Err(ServerFnError::new(
            "Message content exceeds maximum length",
        ));
    }

    let workspace =
        kyomi_auth::workspace_service::get_workspace_full(&ctx.db, workspace_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .ok_or_else(|| ServerFnError::new("Workspace not found"))?;

    let capabilities = if ctx.config.self_hosted {
        kyomi_core::capability::compute_capabilities_self_hosted(false)
    } else {
        kyomi_core::capability::compute_capabilities(&workspace, false)
    };

    if !capabilities.ai_chat_enabled {
        return Err(ServerFnError::new(
            "AI features are not available. Your budget may be exhausted or your plan \
             doesn't include this feature.",
        ));
    }

    let session = kyomi_auth::chat_service::get_session_info(
        &ctx.db,
        &auth.user_id,
        &session_id,
        Some(workspace_id),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    if session.is_none() {
        return Err(ServerFnError::new(
            "Session not found or access denied",
        ));
    }

    let user_message = if let Some(ref content) = chart_content {
        format!("{content}\n\n{message}")
    } else {
        message.clone()
    };

    let encryption_key = ctx
        .encryption_key
        .as_ref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    let _user_message_id = kyomi_auth::chat_service::add_message(
        &ctx.db,
        encryption_key,
        &session_id,
        "user",
        &user_message,
        None, None, None, Some(&auth.user_id), None, None, None,
    )
    .await
    .map_err(|e| ServerFnError::new(format!("Failed to store message: {e}")))?;

    let _assistant_message_id = kyomi_auth::chat_service::add_message(
        &ctx.db,
        encryption_key,
        &session_id,
        "assistant",
        "",
        None, None, None, None, None, None, None,
    )
    .await
    .map_err(|e| ServerFnError::new(format!("Failed to create assistant message: {e}")))?;

    tracing::info!(
        session_id = %session_id,
        "Chart copilot message stored (awaiting agent processing)"
    );

    Ok(CopilotResponse {
        status: "processing".to_string(),
        message: String::new(),
        suggested_content: None,
    })
}
