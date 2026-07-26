// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the chat system.
//!
//! Provides session CRUD, message retrieval, AI message sending, and
//! collaboration features (sharing, pinning, read status). Each function
//! calls the same service-layer code as the REST handlers in
//! `apps/server/src/routes/chat.rs`.
//!
//! ## Endpoints
//!
//! ### Session CRUD (Task 3.1)
//! - `get_session_messages`  — get messages for a session
//! - `update_session_title`  — rename a session
//! - `delete_chat_session`   — delete a session
//! - `bulk_delete_sessions`  — bulk delete sessions
//! - `search_chat_messages`  — search sessions by title
//!
//! ### Message Sending (Task 3.2)
//! - `send_chat_message`     — send user message + spawn AI agent
//!
//! ### Collaboration (Task 3.3)
//! - `share_session`         — share session with workspace
//! - `unshare_session`       — make session private
//! - `mark_session_read`     — mark session as read
//! - `toggle_message_pin`    — toggle message pin status
//! - `update_message_content` — edit message content
//!
//! ### WebSocket Config (existing)
//! - `get_websocket_config`  — obtain WebSocket auth token

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// WebSocket configuration returned to the client for establishing a connection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebSocketConfig {
    pub token: String,
    pub user_id: String,
    pub workspace_id: String,
}

/// A user who created/sent something (session owner or message sender).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionUser {
    pub user_id: String,
    pub display_name: String,
}

/// A chat session in a list/search result.
///
/// Maps from `chat_service::SessionListItem`, with timestamps as RFC 3339 strings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatSessionItem {
    pub session_id: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub session_type: Option<String>,
    #[serde(default)]
    pub shared: bool,
    pub shared_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub message_count: i64,
    #[serde(default)]
    pub pinned_count: i64,
    #[serde(default)]
    pub unread_count: i64,
    pub created_by: Option<SessionUser>,
    pub slack_channel_id: Option<String>,
}

/// A single chat message for display in the UI.
///
/// Maps from `chat_service::MessageItem`. Encrypted content is decrypted by
/// the service layer before reaching this struct.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMessageItem {
    pub message_id: String,
    pub message_type: String,
    pub content: String,
    pub timestamp: String,
    pub pinned: bool,
    pub sent_by: Option<SessionUser>,
    pub thinking_events: Vec<serde_json::Value>,
    pub token_usage: Option<serde_json::Value>,
}

/// Session metadata returned alongside messages.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionDetail {
    pub title: Option<String>,
    pub shared: bool,
    pub created_by: Option<SessionUser>,
    pub slack_channel_id: Option<String>,
}

/// Combined response for get_session_messages: messages + session metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMessagesResponse {
    pub messages: Vec<ChatMessageItem>,
    pub session: SessionDetail,
}

/// Response from sending a chat message.
///
/// The AI response is delivered asynchronously via WebSocket streaming events
/// (`chat_stream`, `chat_complete`). This response contains the message IDs
/// for tracking.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendMessageResponse {
    pub session_id: String,
    pub user_message_id: String,
    pub assistant_message_id: String,
    pub status: String,
    pub thinking_events: Vec<serde_json::Value>,
    pub token_usage: Option<serde_json::Value>,
    pub skip_ai: bool,
}

/// Chart context stored by the MCP chart tool for "Continue in Kyomi" deep-links.
///
/// The MCP chart app stores `{ spec, title, chartMarkdown }` in KV with key
/// `chart:context:<id>`. This struct maps the camelCase JSON fields to Rust.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChartContext {
    pub title: String,
    #[serde(rename = "chartMarkdown")]
    pub chart_markdown: String,
    pub spec: serde_json::Value,
}

// ─────────────────────────────────────────────────────────────────────────────
// MCP Deep-Link: Chart Context
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch a stored chart context for "Continue in Kyomi" deep-links.
///
/// When a user clicks "Continue in Kyomi" in the MCP chart app (Claude.ai),
/// they are directed to `/chat?chart=<id>`. This function retrieves the
/// chart context stored by `kyomi-agent/src/tools/chart.rs::store_chart_context`.
///
/// Returns `None` if the context has expired (TTL) or the ID is invalid.
///
/// Mirrors `GET /api/v1/chart-context/:id` in the REST API.
#[server(prefix = "/leptos-api")]
pub async fn get_chart_context(chart_id: String) -> Result<Option<ChartContext>, ServerFnError> {
    let _auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    let kv_key = format!("chart:context:{chart_id}");

    match kv.get(&kv_key).await {
        Ok(Some(json_str)) => {
            let chart_ctx: ChartContext = serde_json::from_str(&json_str)
                .map_err(|e| ServerFnError::new(format!("Invalid chart context data: {e}")))?;
            Ok(Some(chart_ctx))
        }
        Ok(None) => Ok(None),
        Err(e) => {
            tracing::warn!(error = %e, key = %kv_key, "Failed to fetch chart context from KV");
            Ok(None)
        }
    }
}

/// Store chart YAML from a dashboard widget so the user can ask about it in chat.
///
/// The "Ask about this chart" button in the dashboard viewer calls this to write
/// the raw ChartML markdown into KV with a fresh UUID key, then navigates to
/// `/chat?chart=<uuid>`. The chat page's existing `get_chart_context` call then
/// finds the entry by UUID and injects it as context — exactly the same path
/// used by MCP "Continue in Kyomi" deep-links.
///
/// Uses a 30-day TTL matching `CHART_CONTEXT_TTL_SECS` in `kyomi-agent`.
#[server(prefix = "/leptos-api")]
pub async fn store_chart_context_for_ask(
    chart_markdown: String,
    title: String,
) -> Result<String, ServerFnError> {
    let _auth = super::extract_auth().await?;
    let ctx = super::extract_context()?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    let chart_context_id = uuid::Uuid::new_v4().to_string();

    let context = ChartContext {
        title,
        chart_markdown,
        spec: serde_json::Value::Null,
    };

    let context_json = serde_json::to_string(&context)
        .map_err(|e| ServerFnError::new(format!("Failed to serialize chart context: {e}")))?;

    let kv_key = format!("chart:context:{chart_context_id}");
    // 30-day TTL — same as CHART_CONTEXT_TTL_SECS in kyomi-agent
    kv.set(&kv_key, &context_json, Some(30 * 24 * 60 * 60))
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to store chart context: {e}")))?;

    Ok(chart_context_id)
}

// ─────────────────────────────────────────────────────────────────────────────
// WebSocket Config (existing)
// ─────────────────────────────────────────────────────────────────────────────

/// Obtain a WebSocket authentication token for the current user.
///
/// Generates a short-lived JWT (15 minutes) signed with the app's JWT secret,
/// matching the token format produced by `GET /api/v1/auth/websocket-token`.
/// The client uses this token to authenticate the WebSocket upgrade request.
#[server(prefix = "/leptos-api")]
pub async fn get_websocket_config() -> Result<WebSocketConfig, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let mut extra = std::collections::HashMap::new();
    extra.insert("user_id".into(), serde_json::json!(ac.auth.user_id));
    extra.insert("email".into(), serde_json::json!(ac.auth.email));
    extra.insert("name".into(), serde_json::json!(ac.auth.name));
    extra.insert("roles".into(), serde_json::json!(ac.auth.roles));

    // Short-lived token (15 minutes) — matches apps/server/src/routes/auth.rs
    let token = kyomi_auth::jwt::create_access_token_str(
        &ac.auth.user_id,
        &ac.ctx.config.jwt_secret,
        15,
        extra,
    )
    .map_err(|e| ServerFnError::new(format!("Failed to create WebSocket token: {e}")))?;

    Ok(WebSocketConfig {
        token,
        user_id: ac.auth.user_id.clone(),
        workspace_id: ac.ws_id.clone(),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Task 3.1: Session CRUD Server Functions
// ─────────────────────────────────────────────────────────────────────────────

/// Get messages for a chat session, including session metadata.
///
/// Verifies the user has access (owner or shared in workspace) before
/// returning messages. Returns up to 200 messages, oldest first.
///
/// Mirrors `GET /chat/sessions/{session_id}/messages` in
/// `apps/server/src/routes/chat.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_session_messages(
    session_id: String,
) -> Result<SessionMessagesResponse, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Permission check: user must own or have shared access.
    let session = kyomi_auth::chat_service::get_session_info(
        ac.db(),
        &ac.auth.user_id,
        &session_id,
        Some(&ac.ws_id),
    )
    .await
    .into_sfn()?
    .ok_or_else(|| ServerFnError::new("Session not found or access denied"))?;

    let encryption_key = ac.encryption_key()?;

    let messages = kyomi_auth::chat_service::get_session_messages(
        ac.db(),
        &encryption_key,
        &session_id,
        200, // limit
    )
    .await
    .into_sfn()?;

    Ok(SessionMessagesResponse {
        messages: messages
            .into_iter()
            .map(message_item_to_chat_message_item)
            .collect(),
        session: session_detail_to_session_detail(&session),
    })
}

/// Update a session's title.
///
/// Only the session owner can update the title. Returns an error if the
/// session is not found, access is denied, or the user is not the owner.
///
/// Mirrors `PUT /chat/sessions/{session_id}` in
/// `apps/server/src/routes/chat.rs`.
#[server(prefix = "/leptos-api")]
pub async fn update_session_title(
    session_id: String,
    title: String,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Verify ownership (only owner can update title).
    let session = kyomi_auth::chat_service::get_session_info(
        ac.db(),
        &ac.auth.user_id,
        &session_id,
        Some(&ac.ws_id),
    )
    .await
    .into_sfn()?;

    match session {
        Some(s) if s.user_id == ac.auth.user_id => {}
        Some(_) => {
            return Err(ServerFnError::new(
                "Only the session owner can update the title",
            ));
        }
        None => {
            return Err(ServerFnError::new("Session not found or access denied"));
        }
    }

    let updated =
        kyomi_auth::chat_service::update_session_title(ac.db(), &session_id, &title)
            .await
            .into_sfn()?;

    if !updated {
        return Err(ServerFnError::new("Session not found"));
    }

    tracing::info!(
        session_id = %session_id,
        user_id = %ac.auth.user_id,
        "Updated session title"
    );

    Ok(())
}

/// Delete a chat session and all its messages.
///
/// Only the session owner can delete. Returns an error if the session is
/// not found or the user is not the owner.
///
/// Mirrors `DELETE /chat/sessions/{session_id}` in
/// `apps/server/src/routes/chat.rs`.
#[server(prefix = "/leptos-api")]
pub async fn delete_chat_session(session_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let deleted = kyomi_auth::chat_service::delete_session(
        ac.db(),
        &ac.auth.user_id,
        &session_id,
        Some(&ac.ws_id),
    )
    .await
    .into_sfn()?;

    if !deleted {
        return Err(ServerFnError::new(
            "Session not found or access denied",
        ));
    }

    tracing::info!(
        session_id = %session_id,
        user_id = %ac.auth.user_id,
        "Deleted chat session"
    );

    if let Some(ws_manager) = &ac.ctx.ws_manager {
        kyomi_auth::websocket::helpers::broadcast_entity_delete(
            ws_manager, kyomi_types::sync::entity_types::CHAT_SESSION,
            &session_id, &ac.ws_id,
        ).await;
    }

    Ok(())
}

/// Bulk delete multiple sessions at once.
///
/// Validates that the list is non-empty and capped at 100. Only deletes
/// sessions owned by the current user in the current workspace.
///
/// Mirrors `POST /chat/sessions/bulk-delete` in
/// `apps/server/src/routes/chat.rs`.
#[server(prefix = "/leptos-api")]
pub async fn bulk_delete_sessions(session_ids: Vec<String>) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    if session_ids.is_empty() {
        return Err(ServerFnError::new("session_ids cannot be empty"));
    }

    if session_ids.len() > 100 {
        return Err(ServerFnError::new(
            "Cannot delete more than 100 sessions at once",
        ));
    }

    let deleted_count = kyomi_auth::chat_service::bulk_delete_sessions(
        ac.db(),
        &ac.auth.user_id,
        &session_ids,
        &ac.ws_id,
    )
    .await
    .into_sfn()?;

    tracing::info!(
        deleted_count = deleted_count,
        user_id = %ac.auth.user_id,
        "Bulk deleted chat sessions"
    );

    if let Some(ws_manager) = &ac.ctx.ws_manager {
        for id in &session_ids {
            kyomi_auth::websocket::helpers::broadcast_entity_delete(
                ws_manager, kyomi_types::sync::entity_types::CHAT_SESSION,
                id, &ac.ws_id,
            ).await;
        }
    }

    Ok(())
}

/// Search chat sessions by title.
///
/// Returns sessions (owned + shared) whose title matches the query (ILIKE).
/// Returns an empty list when the query is empty.
///
/// Mirrors `GET /chat/search` in `apps/server/src/routes/chat.rs`.
#[server(prefix = "/leptos-api")]
pub async fn search_chat_messages(query: String) -> Result<Vec<ChatSessionItem>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    if query.is_empty() {
        return Ok(Vec::new());
    }

    let sessions = kyomi_auth::chat_service::search_sessions(
        ac.db(),
        &ac.auth.user_id,
        &ac.ws_id,
        &query,
        50, // limit (matches default_search_limit)
    )
    .await
    .into_sfn()?;

    Ok(sessions
        .into_iter()
        .map(session_list_item_to_chat_session_item)
        .collect())
}

// ─────────────────────────────────────────────────────────────────────────────
// Task 3.2: Message Send Server Function
// ─────────────────────────────────────────────────────────────────────────────

/// Send a user message and trigger AI agent execution.
///
/// Thin wrapper around `chat_service::prepare_chat_dispatch` + agent spawn.
/// Pre-spawn orchestration (find/create session, skip_ai store, shared
/// broadcast) lives in the service layer. This function handles only the
/// Leptos-specific context extraction, agent config construction, and spawn.
///
/// The AI response is delivered asynchronously via WebSocket streaming events.
#[server(prefix = "/leptos-api")]
pub async fn send_chat_message(
    message: String,
    session_id: Option<String>,
    current_time_user_tz: Option<String>,
    skip_ai: bool,
    model: Option<String>,
    // client_msg_id is the optimistic message ID from the client, used for
    // deduplication when shared_chat_message WebSocket broadcast arrives.
    client_msg_id: Option<String>,
) -> Result<SendMessageResponse, ServerFnError> {
    // lint-allow: server-fn-callouts=cancelled/success/error match arms are mutually exclusive — count inflated by branching, not complexity
    let ac = AuthenticatedContext::extract().await?;

    // 1. Validate message.
    if message.trim().is_empty() {
        return Err(ServerFnError::new("Message content cannot be empty"));
    }
    if message.len() > 100_000 {
        return Err(ServerFnError::new(
            "Message content exceeds maximum length",
        ));
    }

    // 2. Gate: LLM must be configured for AI features.
    if !ac.ctx.config.llm_configured() {
        return Err(ServerFnError::new(
            "No LLM provider configured. Add ANTHROPIC_API_KEY or LLM_API_KEY to your environment.",
        ));
    }

    let encryption_key = ac.encryption_key()?;

    let user_display_name = ac.auth
        .name
        .as_deref()
        .unwrap_or(&ac.auth.email)
        .to_string();

    // 3–5. Find/create session, handle skip_ai, broadcast user message.
    // Callout 1 of 3.
    let outcome = kyomi_auth::chat_service::prepare_chat_dispatch(
        kyomi_auth::chat_service::ChatDispatchParams {
            db: ac.db(),
            encryption_key: &encryption_key,
            ws_manager: ac.ctx.ws_manager.as_ref(),
            user_id: &ac.auth.user_id,
            workspace_id: &ac.ws_id,
            user_display_name: &user_display_name,
            session_id: session_id.as_deref(),
            message: &message,
            current_time_user_tz: current_time_user_tz.as_deref(),
            skip_ai,
            client_msg_id: client_msg_id.as_deref(),
        },
    )
    .await
    .into_sfn()?;

    // Early return for skip_ai path (service handled storage).
    let (session_id, is_new_session, user_message_id, assistant_message_id, is_shared) =
        match outcome {
            kyomi_auth::chat_service::ChatDispatchOutcome::SkippedAi {
                session_id,
                user_message_id,
            } => {
                return Ok(SendMessageResponse {
                    session_id,
                    user_message_id,
                    assistant_message_id: String::new(),
                    status: "skipped".to_string(),
                    thinking_events: Vec::new(),
                    token_usage: None,
                    skip_ai: true,
                });
            }
            kyomi_auth::chat_service::ChatDispatchOutcome::Ready {
                session_id,
                is_new_session,
                user_message_id,
                assistant_message_id,
                is_shared,
            } => (session_id, is_new_session, user_message_id, assistant_message_id, is_shared),
        };

    // 6. Build execution config and spawn agent task.
    // Requires ws_manager and cancel_registry to be provided in ServerContext.
    let ws_manager = ac.ctx
        .ws_manager
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebSocket manager not configured"))?
        .clone();

    let cancel_registry = ac.ctx
        .cancel_registry
        .as_ref()
        .ok_or_else(|| ServerFnError::new("Cancel registry not configured"))?
        .clone();

    let platforms = ac.ctx
        .platforms
        .as_ref()
        .ok_or_else(|| ServerFnError::new("Platform registry not configured"))?
        .clone();

    let cancel_token = tokio_util::sync::CancellationToken::new();

    let exec_config = kyomi_agent::AgentExecutionConfig {
        session_id: session_id.clone(),
        user_id: ac.auth.user_id.clone(),
        workspace_id: ac.ws_id.clone(),
        message: message.clone(),
        model_name: model,
        temperature: 0.7,
        is_shared_conversation: is_shared,
        context_type: "chat".to_string(),
        workspace_user_ids: None,
        cancel_token: cancel_token.clone(),
        current_time_user_tz: current_time_user_tz.clone(),
        message_source: Some("web".to_string()),
        system_prompt: None,
        tools_subset: None,
        max_iterations: 25,
        component: "custom_agent".to_string(),
        user_message_id: Some(user_message_id.clone()),
        assistant_message_id: Some(assistant_message_id.clone()),
        conversation_history: None,
        user_display_name: ac.auth.name.clone().unwrap_or_else(|| ac.auth.email.clone()),
        context_window: 0,
        workspace_roles: ac.auth.workspace.workspace_roles.clone(),
    };

    // Register cancel token so WebSocket cancel_request can stop this task.
    cancel_registry.register(&ac.auth.user_id, &session_id, cancel_token.clone());

    // Spawn async task for AI execution + response delivery.
    let db = ac.ctx.db.clone();
    let kv = ac.kv()?;

    let embedding = ac.ctx.embedding.clone();
    let app_config = ac.ctx.config.clone();
    let connect_registry = ac.ctx.connect_registry.clone();
    let spawn_user_id = ac.auth.user_id.clone();
    let spawn_session_id = session_id.clone();
    let spawn_assistant_message_id = assistant_message_id.clone();
    let spawn_workspace_id = ac.ws_id.clone();
    let spawn_is_shared = is_shared;
    let context_type = "chat".to_string();

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
                // transition out of Cancelling state. Do NOT call deliver_response
                // or broadcast — the partial response is discarded.
                kyomi_auth::websocket::helpers::send_request_cancelled(
                    &ws_manager,
                    &spawn_user_id,
                    &spawn_session_id,
                    &exec_result.assistant_message_id,
                    Some(&context_type),
                )
                .await;
            }
            Ok(exec_result) => {
                // Deliver response via WebSocket.
                kyomi_agent::deliver_response(
                    &ws_manager,
                    &spawn_user_id,
                    &spawn_session_id,
                    &exec_result.assistant_message_id,
                    &exec_result.response_text,
                    exec_result.model.as_deref().unwrap_or("unknown"),
                    exec_result.token_usage,
                    &context_type,
                    None,
                    None,
                )
                .await;

                // Broadcast assistant message to shared conversation members.
                // Callout 2 of 3.
                if spawn_is_shared {
                    kyomi_auth::websocket::helpers::send_shared_chat_message(
                        &ws_manager,
                        &spawn_workspace_id,
                        &spawn_session_id,
                        &exec_result.assistant_message_id,
                        "assistant",
                        &exec_result.response_text,
                        &chrono::Utc::now().to_rfc3339(),
                        None,
                        None,
                        None,
                    )
                    .await;
                }
            }
            Err(e) => {
                tracing::error!(
                    session_id = %spawn_session_id,
                    error = %e,
                    "Agent execution failed"
                );

                // Persist error message and notify the user. Callout 3 of 3.
                kyomi_auth::chat_service::save_agent_error(
                    kyomi_auth::chat_service::SaveAgentErrorParams {
                        db: &db,
                        encryption_key: &encryption_key,
                        ws_manager: &ws_manager,
                        session_id: &spawn_session_id,
                        user_id: &spawn_user_id,
                        assistant_message_id: &spawn_assistant_message_id,
                        context_type: &context_type,
                        error: &e.to_string(),
                    },
                )
                .await;
            }
        }

        // Clean up cancel token so it doesn't leak.
        cancel_registry.remove(&spawn_user_id, &spawn_session_id);
    });

    // 8. Fire-and-forget title generation for new sessions. The spawned task
    // loads WorkspaceAiConfig (Kyomi or BYOK) and logs a warning on failure;
    // no server-side config guard is needed — gating on
    // `resolve_provider_config` would silently skip titles for BYOK-only
    // deployments that never set server Kyomi keys.
    if is_new_session
        && let Some(ref ws_mgr) = ac.ctx.ws_manager
    {
        kyomi_agent::generate_session_title(
            ac.ctx.db.clone(),
            ws_mgr.clone(),
            session_id.clone(),
            ac.auth.user_id.clone(),
            ac.ws_id.clone(),
            message.clone(),
            ac.ctx.config.clone(),
        );
    }

    tracing::info!(
        session_id = %session_id,
        user_message_id = %user_message_id,
        "Dispatched AI processing for chat message"
    );

    // 9. Return immediately with IDs and status.
    Ok(SendMessageResponse {
        session_id,
        user_message_id,
        assistant_message_id,
        status: "processing".to_string(),
        thinking_events: Vec::new(),
        token_usage: None,
        skip_ai: false,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Task 3.3: Collaboration Server Functions
// ─────────────────────────────────────────────────────────────────────────────

/// Share a session with the workspace (makes it visible to all members).
///
/// Only the session owner can share. Sets `shared = true` and records
/// `shared_at` timestamp via `chat_service::set_session_shared`, which also
/// persists the visibility transition to `sync_log` so an offline
/// workspace member converges on their next delta sync.
#[server(prefix = "/leptos-api")]
pub async fn share_session(session_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    // Load session and verify ownership.
    #[derive(sqlx::FromRow)]
    struct ShareRow {
        user_id: String,
    }
    let row = kyomi_core::db_fetch_optional!(
        &ctx.db,
        ShareRow,
        "SELECT user_id FROM chat_sessions WHERE session_id = $1",
        &session_id
    )
    .into_sfn()?
    .ok_or_else(|| ServerFnError::new("Session not found"))?;

    if row.user_id != auth.user_id {
        return Err(ServerFnError::new(
            "Only the owner can share this conversation",
        ));
    }

    kyomi_auth::chat_service::set_session_shared(&ctx.db, &session_id, true)
        .await
        .into_sfn()?;

    tracing::info!(
        session_id = %session_id,
        user_id = %auth.user_id,
        "Session shared"
    );

    if let Some(ws_manager) = &ctx.ws_manager {
        let ws_id = super::workspace_id(&auth)?;
        kyomi_auth::websocket::helpers::broadcast_chat_session_sync(
            &ctx.db, ws_manager, &session_id, ws_id,
            kyomi_types::sync::SyncActionType::Update,
            &auth.user_id,
        ).await;
    }

    Ok(())
}

/// Make a session private (removes workspace-wide visibility).
///
/// Only the session owner can unshare. Blocks unsharing Slack channel
/// conversations (they're visible on the platform). Delegates the
/// `shared` flip and matching `sync_log` write to
/// `chat_service::set_session_shared`.
#[server(prefix = "/leptos-api")]
pub async fn unshare_session(session_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    #[derive(sqlx::FromRow)]
    struct UnshareRow {
        user_id: String,
        platform_type: Option<String>,
        platform_thread_key: Option<String>,
    }
    let row = kyomi_core::db_fetch_optional!(
        &ctx.db,
        UnshareRow,
        "SELECT user_id, platform_type, platform_thread_key FROM chat_sessions WHERE session_id = $1",
        &session_id
    )
    .into_sfn()?
    .ok_or_else(|| ServerFnError::new("Session not found"))?;

    if row.user_id != auth.user_id {
        return Err(ServerFnError::new(
            "Only the owner can unshare this conversation",
        ));
    }

    // Block unsharing platform channel/group conversations.
    if let Some(ref platform) = row.platform_type
        && platform == "slack"
    {
        let channel_id = row
            .platform_thread_key
            .as_deref()
            .and_then(|k| k.split(':').next())
            .unwrap_or("");
        if !channel_id.starts_with('D') {
            return Err(ServerFnError::new(
                "Slack channel conversations cannot be unshared because they're \
                 visible to your team in Slack. To have a private conversation, \
                 start a new chat in Kyomi.",
            ));
        }
    }

    kyomi_auth::chat_service::set_session_shared(&ctx.db, &session_id, false)
        .await
        .into_sfn()?;

    tracing::info!(
        session_id = %session_id,
        user_id = %auth.user_id,
        "Session unshared"
    );

    if let Some(ws_manager) = &ctx.ws_manager {
        let ws_id = super::workspace_id(&auth)?;
        kyomi_auth::websocket::helpers::broadcast_chat_session_unshare(
            &ctx.db, ws_manager, &session_id, ws_id, &auth.user_id,
        ).await;
    }

    Ok(())
}

/// Mark a session as read up to the specified message (or the latest message).
///
/// Upserts into `conversation_read_status` to track the user's read position.
/// Used for computing `unread_count` in session listings.
///
/// Mirrors `POST /chat/sessions/{session_id}/read` in
/// `apps/server/src/routes/chat.rs`.
#[server(prefix = "/leptos-api")]
pub async fn mark_session_read(
    session_id: String,
    last_message_id: Option<String>,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Permission check: user must own or have shared access.
    let session = kyomi_auth::chat_service::get_session_info(
        ac.db(),
        &ac.auth.user_id,
        &session_id,
        Some(&ac.ws_id),
    )
    .await
    .into_sfn()?;

    if session.is_none() {
        return Err(ServerFnError::new(
            "Access denied: you do not have permission to mark this conversation as read",
        ));
    }

    // Resolve message_id: use provided ID or fall back to latest message.
    let message_id = if let Some(mid) = last_message_id {
        Some(mid)
    } else {
        #[derive(sqlx::FromRow)]
        struct MsgIdRow {
            message_id: String,
        }
        let latest = kyomi_core::db_fetch_optional!(
            ac.db(),
            MsgIdRow,
            "SELECT message_id FROM chat_messages \
             WHERE session_id = $1 \
             ORDER BY created_at DESC LIMIT 1",
            &session_id
        )
        .into_sfn()?;

        latest.map(|r| r.message_id)
    };

    let now = chrono::Utc::now();
    let now_str = now.to_rfc3339();

    kyomi_core::db_execute!(
        ac.db(),
        "INSERT INTO conversation_read_status \
         (session_id, user_id, last_read_at, last_read_message_id) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (session_id, user_id) \
         DO UPDATE SET last_read_at = $3, last_read_message_id = $4",
        &session_id,
        &ac.auth.user_id,
        &now_str,
        message_id.as_deref()
    )
    .into_sfn()?;

    tracing::info!(
        session_id = %session_id,
        user_id = %ac.auth.user_id,
        message_id = ?message_id,
        "Marked session as read"
    );

    Ok(())
}

/// Toggle the pinned status of a message.
///
/// Requires access to the session (owner or shared in workspace).
/// Returns `true` if the toggle was successful (message found), `false`
/// otherwise.
///
/// Mirrors `POST /chat/sessions/{session_id}/messages/{message_id}/toggle-pin`
/// in `apps/server/src/routes/chat.rs`.
#[server(prefix = "/leptos-api")]
pub async fn toggle_message_pin(
    session_id: String,
    message_id: String,
) -> Result<bool, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let success = kyomi_auth::chat_service::toggle_message_pin(
        ac.db(),
        &session_id,
        &message_id,
        &ac.auth.user_id,
        Some(&ac.ws_id),
    )
    .await
    .into_sfn()?;

    if success {
        tracing::info!(
            session_id = %session_id,
            message_id = %message_id,
            "Toggled pin status"
        );
    }

    Ok(success)
}

/// Update the content of a message.
///
/// Only the session owner can edit messages. Re-encrypts the content before
/// storing. Thin wrapper around `chat_service::update_message_content_owned`.
///
/// Mirrors `PATCH /chat/sessions/{session_id}/messages/{message_id}` in
/// `apps/server/src/routes/chat.rs`.
#[server(prefix = "/leptos-api")]
pub async fn update_message_content(
    session_id: String,
    message_id: String,
    content: String,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    if content.len() > 100_000 {
        return Err(ServerFnError::new(
            "Message content exceeds maximum length",
        ));
    }

    let encryption_key = ac.encryption_key()?;

    // Verify ownership, verify message membership, re-encrypt, and persist.
    kyomi_auth::chat_service::update_message_content_owned(
        ac.db(),
        &encryption_key,
        &ac.auth.user_id,
        &ac.ws_id,
        &session_id,
        &message_id,
        &content,
    )
    .await
    .into_sfn()?;

    tracing::info!(
        session_id = %session_id,
        message_id = %message_id,
        user_id = %ac.auth.user_id,
        "Updated message content"
    );

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Thinking event details (full reasoning text on demand)
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch the full untruncated reasoning text for a thinking event.
///
/// Returns `None` if no full text was stored (the event was short enough to
/// fit within the 200-char display limit).
#[server(prefix = "/leptos-api")]
pub async fn get_thinking_event_detail(
    message_id: String,
    event_id: String,
) -> Result<Option<String>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    let encryption_key = ac.encryption_key()?;

    let text = kyomi_auth::chat_service::get_thinking_event_detail(
        ac.db(),
        &encryption_key,
        &message_id,
        &event_id,
        &ac.auth.user_id,
        &ac.ws_id,
    )
    .await
    .into_sfn()?;

    Ok(text)
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers — delegate to shared extractors in parent module
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, AuthenticatedContext, IntoServerFnError};

/// Convert a `chat_service::SessionListItem` to our `ChatSessionItem`.
#[cfg(feature = "ssr")]
fn session_list_item_to_chat_session_item(
    s: kyomi_auth::chat_service::SessionListItem,
) -> ChatSessionItem {
    // Determine Slack channel ID. The service layer stores `platform_type = "slack"`
    // and `platform_thread_key = "{channel_id}:{thread_ts}"`. The React frontend
    // uses `slack_channel_id` to show the Slack icon — for list items we use a
    // sentinel value since `SessionListItem` doesn't carry `platform_thread_key`.
    // The actual channel ID is only needed for deep-linking, which uses SessionDetail.
    let slack_channel_id = if s.platform_type.as_deref() == Some("slack") {
        // Use platform_type as a truthy marker — the UI only checks Some vs None
        // for showing the Slack badge on list items.
        Some("slack".to_string())
    } else {
        None
    };

    ChatSessionItem {
        session_id: s.session_id,
        title: s.title,
        model: s.model,
        session_type: Some(s.session_type),
        shared: s.shared,
        shared_at: s.shared_at,
        created_at: s.created_at.unwrap_or_default(),
        updated_at: s.updated_at.unwrap_or_default(),
        message_count: s.message_count,
        pinned_count: s.pinned_count,
        unread_count: s.unread_count,
        created_by: s.created_by.map(|cb| SessionUser {
            user_id: cb.user_id,
            display_name: cb.display_name.unwrap_or_default(),
        }),
        slack_channel_id,
    }
}

/// Convert a `chat_service::MessageItem` to our `ChatMessageItem`.
#[cfg(feature = "ssr")]
fn message_item_to_chat_message_item(
    m: kyomi_auth::chat_service::MessageItem,
) -> ChatMessageItem {
    ChatMessageItem {
        message_id: m.message_id,
        message_type: m.message_type,
        content: m.content,
        timestamp: m.timestamp.unwrap_or_default(),
        pinned: m.pinned,
        sent_by: m.sent_by.map(|cb| SessionUser {
            user_id: cb.user_id,
            display_name: cb.display_name.unwrap_or_default(),
        }),
        thinking_events: m.thinking_events,
        token_usage: m.token_usage,
    }
}

/// Convert a `chat_service::SessionMetadata` to our `SessionDetail`.
#[cfg(feature = "ssr")]
fn session_detail_to_session_detail(
    s: &kyomi_auth::chat_service::SessionMetadata,
) -> SessionDetail {
    SessionDetail {
        title: s.title.clone(),
        shared: s.shared,
        created_by: s.created_by.as_ref().map(|cb| SessionUser {
            user_id: cb.user_id.clone(),
            display_name: cb.display_name.clone().unwrap_or_default(),
        }),
        // Use platform_type as a truthy marker for Slack sessions.
        // SessionDetail doesn't carry platform_thread_key either.
        slack_channel_id: if s.platform_type.as_deref() == Some("slack") {
            Some("slack".to_string())
        } else {
            None
        },
    }
}
