// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat session CRUD endpoints.
//!
//! Wire-compatible with Python's `routers/chat.py`.
//! Provides session management, message retrieval, search, sharing,
//! collaboration, chart management, and a skeleton for AI message
//! processing (Phase 9).
//!
//! ## Endpoints (Phase 8B)
//!
//! - `GET    /sessions`                  — list_sessions
//! - `GET    /sessions/{session_id}/messages` — get_messages
//! - `PUT    /sessions/{session_id}`     — update_session
//! - `DELETE /sessions/{session_id}`     — delete_session
//! - `POST   /sessions/bulk-delete`      — bulk_delete_sessions
//! - `GET    /search`                    — search_sessions
//! - `GET    /models`                    — get_models
//! - `GET    /status`                    — get_status
//! - `POST   /message/websocket`         — send_message (Phase 9 skeleton)
//!
//! ## Endpoints (Phase 8C)
//!
//! - `POST   /sessions/{session_id}/share`     — share_session
//! - `POST   /sessions/{session_id}/unshare`   — unshare_session
//! - `POST   /sessions/{session_id}/read`      — mark_session_read
//! - `POST   /sessions/{session_id}/transfer`  — transfer_ownership
//! - `PATCH  /sessions/{session_id}/messages/{message_id}`         — update_message_content
//! - `POST   /sessions/{session_id}/messages/{message_id}/toggle-pin` — toggle_pin
//! - `PATCH  /charts/{chart_id}`               — update_chart_config
//! - `PUT    /charts/{chart_id}/sql`           — update_chart_sql

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch, post, put},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use kyomi_auth::{chat_service, encryption, middleware::AuthUser};

use crate::state::AppState;

/// Build the `/chat` router with all chat endpoints.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Static paths FIRST (before /{session_id} captures them)
        .route("/search", get(search_sessions))
        .route("/models", get(get_models))
        .route("/status", get(get_status))
        .route("/sessions/bulk-delete", post(bulk_delete_sessions))
        .route("/message/websocket", post(send_message))
        // Dynamic path handlers
        .route("/sessions", get(list_sessions))
        .route(
            "/sessions/{session_id}",
            get(get_session).put(update_session).delete(delete_session),
        )
        .route(
            "/sessions/{session_id}/messages",
            get(get_messages),
        )
        // Phase 8C: Sharing + Collaboration
        .route("/sessions/{session_id}/share", post(share_session))
        .route("/sessions/{session_id}/unshare", post(unshare_session))
        .route("/sessions/{session_id}/read", post(mark_session_read))
        .route("/sessions/{session_id}/transfer", post(transfer_ownership))
        // Phase 8C: Message + Chart
        .route(
            "/sessions/{session_id}/messages/{message_id}",
            patch(update_message_content),
        )
        .route(
            "/sessions/{session_id}/messages/{message_id}/toggle-pin",
            post(toggle_pin),
        )
        .route("/charts/{chart_id}", patch(update_chart_config))
        .route("/charts/{chart_id}/sql", put(update_chart_sql))
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
// Request / Response Types
// ===========================================================================

// -- List sessions query params --

#[derive(Deserialize)]
struct ListSessionsParams {
    #[serde(default = "default_session_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    pinned_only: bool,
    #[serde(default = "default_session_type")]
    session_type: String,
}

fn default_session_limit() -> i64 {
    50
}

fn default_session_type() -> String {
    "chat".to_string()
}

// -- Get messages query params --

#[derive(Deserialize)]
struct GetMessagesParams {
    #[serde(default = "default_message_limit")]
    limit: i64,
}

fn default_message_limit() -> i64 {
    200
}

// -- Search query params --

#[derive(Deserialize)]
struct SearchParams {
    #[serde(default)]
    query: String,
    #[serde(default = "default_search_limit")]
    limit: i64,
}

fn default_search_limit() -> i64 {
    50
}

// -- Update session request --

#[derive(Deserialize)]
struct UpdateSessionRequest {
    title: String,
}

// -- Bulk delete request --

#[derive(Deserialize)]
struct BulkDeleteRequest {
    session_ids: Vec<String>,
}

// -- Send message request --

#[derive(Deserialize)]
struct SendMessageRequest {
    #[serde(default)]
    session_id: Option<String>,
    message: String,
    #[serde(default)]
    skip_ai: Option<bool>,
    /// Model to use for AI response.
    #[serde(default)]
    model: Option<String>,
    /// User's local time in ISO format (e.g., "2025-01-15T10:30:00+11:00").
    #[serde(default)]
    current_time_user_tz: Option<String>,
    /// Context type (e.g., "chat", "trial_chat", "copilot").
    #[serde(default)]
    context_type: Option<String>,
}

// -- Mark read request (Phase 8C) --

#[derive(Deserialize, Default)]
struct MarkReadRequest {
    #[serde(default)]
    message_id: Option<String>,
}

// -- Transfer ownership request (Phase 8C) --

#[derive(Deserialize)]
struct TransferOwnershipRequest {
    new_owner_user_id: String,
}

// -- Update message content request (Phase 8C) --

#[derive(Deserialize)]
struct UpdateMessageContentRequest {
    content: String,
}

// -- Update chart config request (Phase 8C) --
// Accepts arbitrary JSON fields to merge into chart_data.

// -- Update chart SQL request (Phase 8C) --

#[derive(Deserialize)]
struct UpdateChartSqlRequest {
    sql: String,
}

// -- Model info --

#[derive(Serialize)]
struct ModelInfo {
    id: String,
    name: String,
    provider: String,
}

// ===========================================================================
// Endpoint Handlers
// ===========================================================================

// ---------------------------------------------------------------------------
// GET /sessions -- List chat sessions
// ---------------------------------------------------------------------------

async fn list_sessions(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ListSessionsParams>,
) -> Result<Json<Vec<chat_service::SessionListItem>>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let limit = params.limit.clamp(1, 200);
    let offset = params.offset.max(0);

    let sessions: Vec<chat_service::SessionListItem> = chat_service::get_user_sessions(
        &state.db,
        &user.user_id,
        workspace_id,
        limit,
        offset,
        params.pinned_only,
        &params.session_type,
    )
    .await?;

    tracing::info!(
        "Listed {} sessions for user {} in workspace {}",
        sessions.len(),
        user.user_id,
        workspace_id
    );

    Ok(Json(sessions))
}

// ---------------------------------------------------------------------------
// GET /sessions/{session_id} -- Get single session detail
// ---------------------------------------------------------------------------

async fn get_session(
    State(state): State<AppState>,
    user: AuthUser,
    Path(session_id): Path<String>,
) -> Result<Json<chat_service::SessionDetail>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let session = chat_service::get_session_info(
        &state.db,
        &user.user_id,
        &session_id,
        Some(workspace_id),
    )
    .await?;

    match session {
        Some(detail) => Ok(Json(detail)),
        None => Err(kyomi_core::Error::NotFound(
            "Session not found or access denied".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// GET /sessions/{session_id}/messages -- Get messages for a session
// ---------------------------------------------------------------------------

async fn get_messages(
    State(state): State<AppState>,
    user: AuthUser,
    Path(session_id): Path<String>,
    Query(params): Query<GetMessagesParams>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Permission check: user must own or have shared access to the session.
    let session = chat_service::get_session_info(
        &state.db,
        &user.user_id,
        &session_id,
        Some(workspace_id),
    )
    .await?;

    let session = match session {
        Some(s) => s,
        None => {
            return Err(kyomi_core::Error::NotFound(
                "Session not found or access denied".into(),
            ));
        }
    };

    let limit = params.limit.clamp(1, 1000);

    let messages = chat_service::get_session_messages(
        &state.db,
        &state.encryption_key,
        &session_id,
        limit,
    )
    .await?;

    Ok(Json(json!({
        "messages": messages,
        "session": session,
    })))
}

// ---------------------------------------------------------------------------
// PUT /sessions/{session_id} -- Update session title
// ---------------------------------------------------------------------------

async fn update_session(
    State(state): State<AppState>,
    user: AuthUser,
    Path(session_id): Path<String>,
    Json(request): Json<UpdateSessionRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Verify ownership (only owner can update title).
    let session = chat_service::get_session_info(
        &state.db,
        &user.user_id,
        &session_id,
        Some(workspace_id),
    )
    .await?;

    match session {
        Some(s) if s.user_id == user.user_id => {}
        Some(_) => {
            return Err(kyomi_core::Error::Forbidden(
                "Only the session owner can update the title".into(),
            ));
        }
        None => {
            return Err(kyomi_core::Error::NotFound(
                "Session not found or access denied".into(),
            ));
        }
    }

    let updated = chat_service::update_session_title(&state.db, &session_id, &request.title).await?;

    if updated {
        tracing::info!(
            "Updated session {} title for user {}",
            session_id,
            user.user_id
        );
        Ok(Json(json!({
            "success": true,
            "message": "Session updated",
        })))
    } else {
        Err(kyomi_core::Error::NotFound(
            "Session not found".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// DELETE /sessions/{session_id} -- Delete a session
// ---------------------------------------------------------------------------

async fn delete_session(
    State(state): State<AppState>,
    user: AuthUser,
    Path(session_id): Path<String>,
) -> Result<StatusCode, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let deleted = chat_service::delete_session(
        &state.db,
        &user.user_id,
        &session_id,
        Some(workspace_id),
    )
    .await?;

    if deleted {
        tracing::info!(
            "Deleted session {} for user {}",
            session_id,
            user.user_id
        );
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(kyomi_core::Error::NotFound(
            "Session not found or access denied".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// POST /sessions/bulk-delete -- Bulk delete sessions
// ---------------------------------------------------------------------------

async fn bulk_delete_sessions(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<BulkDeleteRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    if request.session_ids.is_empty() {
        return Err(kyomi_core::Error::BadRequest(
            "session_ids cannot be empty".into(),
        ));
    }

    if request.session_ids.len() > 100 {
        return Err(kyomi_core::Error::BadRequest(
            "Cannot delete more than 100 sessions at once".into(),
        ));
    }

    let deleted_count = chat_service::bulk_delete_sessions(
        &state.db,
        &user.user_id,
        &request.session_ids,
        workspace_id,
    )
    .await?;

    tracing::info!(
        "Bulk deleted {} sessions for user {} in workspace {}",
        deleted_count,
        user.user_id,
        workspace_id
    );

    Ok(Json(json!({ "deleted_count": deleted_count })))
}

// ---------------------------------------------------------------------------
// GET /search -- Search sessions by title
// ---------------------------------------------------------------------------

async fn search_sessions(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<SearchParams>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    if params.query.is_empty() {
        return Ok(Json(json!({ "sessions": [] })));
    }

    let limit = params.limit.clamp(1, 100);

    let sessions: Vec<chat_service::SessionListItem> = chat_service::search_sessions(
        &state.db,
        &user.user_id,
        workspace_id,
        &params.query,
        limit,
    )
    .await?;

    tracing::info!(
        "Search '{}' returned {} sessions for user {}",
        params.query,
        sessions.len(),
        user.user_id
    );

    Ok(Json(json!({ "sessions": sessions })))
}

// ---------------------------------------------------------------------------
// GET /models -- Return available AI models
// ---------------------------------------------------------------------------

async fn get_models() -> Json<serde_json::Value> {
    let models = vec![
        ModelInfo {
            id: "claude-haiku-4-5-20251001".into(),
            name: "Claude Haiku 4.5".into(),
            provider: "claude".into(),
        },
        ModelInfo {
            id: "claude-sonnet-4-5-20250929".into(),
            name: "Claude Sonnet 4.5".into(),
            provider: "claude".into(),
        },
    ];

    Json(json!({
        "models": {
            "claude": models.iter().map(|m| &m.id).collect::<Vec<_>>(),
        },
        "status": "success",
        "timestamp": Utc::now().to_rfc3339(),
    }))
}

// ---------------------------------------------------------------------------
// GET /status -- Chat service health status
// ---------------------------------------------------------------------------

async fn get_status() -> Json<serde_json::Value> {
    Json(json!({
        "status": "online",
        "timestamp": Utc::now().to_rfc3339(),
    }))
}

// ---------------------------------------------------------------------------
// POST /message/websocket -- Send user message + trigger AI response
// ---------------------------------------------------------------------------

async fn send_message(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<SendMessageRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

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

    // Gate: LLM must be configured for AI features.
    if !state.config.llm_configured() {
        return Err(kyomi_core::Error::ServiceUnavailable(
            "No LLM provider configured. Add ANTHROPIC_API_KEY or LLM_API_KEY to your environment.".into(),
        ));
    }

    // Find or create session.
    let is_new_session = request.session_id.is_none();
    let session_id = if let Some(ref sid) = request.session_id {
        // Verify the user has access to this session.
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
        // Create a new session.
        let new_sid =
            chat_service::create_session(&state.db, &user.user_id, workspace_id).await?;

        // Notify the frontend so the sidebar updates immediately.
        if let Ok(Some(session_info)) = chat_service::get_session_info(
            &state.db,
            &user.user_id,
            &new_sid,
            Some(workspace_id),
        )
        .await
        {
            if let Ok(data) = serde_json::to_value(&session_info) {
                kyomi_auth::websocket::helpers::send_session_created(
                    &state.ws_manager,
                    &user.user_id,
                    &new_sid,
                    data,
                )
                .await;
            }
        }

        new_sid
    };

    // Generate user message ID without saving to DB.
    // The adapter's persist_after_chat() will save the decorated user message
    // (with metadata prefix) so that reloading from DB produces a byte-identical
    // prefix for LLM prompt cache hits.
    let user_message_id = uuid::Uuid::new_v4().to_string();

    let skip_ai = request.skip_ai.unwrap_or(false);

    if skip_ai {
        // skip_ai bypasses the agent loop, so we must save the user message
        // directly (the adapter's persist_after_chat won't run).
        let saved_id = chat_service::add_message(
            &state.db,
            &state.encryption_key,
            &session_id,
            "user",
            &request.message,
            None,
            Some(&user_message_id),
            request.current_time_user_tz.as_deref(),
            Some(&user.user_id),
            None,
            None,
            None,
        )
        .await?;

        tracing::info!(
            "Stored user message {} in session {} (skip_ai=true)",
            saved_id,
            session_id
        );
        return Ok(Json(json!({
            "session_id": session_id,
            "user_message_id": saved_id,
            "status": "skipped",
        })));
    }

    // Generate assistant message ID without saving a placeholder to DB.
    // The adapter's persist_after_chat() will save the actual assistant response
    // with the correct content. This avoids empty placeholder rows and ensures
    // all messages are saved in a single pass with correct ordering.
    let assistant_message_id = uuid::Uuid::new_v4().to_string();

    // Check if this is a shared conversation.
    let session_detail = chat_service::get_session(&state.db, &session_id).await?;
    let is_shared = session_detail.as_ref().map(|s| s.shared).unwrap_or(false);

    let context_type = request
        .context_type
        .clone()
        .unwrap_or_else(|| "chat".to_string());

    // Build the execution config.
    let cancel_token = tokio_util::sync::CancellationToken::new();

    let exec_config = kyomi_agent::AgentExecutionConfig {
        session_id: session_id.clone(),
        user_id: user.user_id.clone(),
        workspace_id: workspace_id.to_string(),
        message: request.message.clone(),
        model_name: request.model.clone(),
        temperature: 0.7,
        is_shared_conversation: is_shared,
        context_type: context_type.clone(),
        workspace_user_ids: None,
        cancel_token: cancel_token.clone(),
        current_time_user_tz: request.current_time_user_tz.clone(),
        message_source: Some("web".to_string()),
        system_prompt: None,
        tools_subset: None,
        max_iterations: 25,
        component: "custom_agent".to_string(),
        user_message_id: Some(user_message_id.clone()),
        assistant_message_id: Some(assistant_message_id.clone()),
        conversation_history: None,
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
    let first_message = request.message.clone();

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
                // Deliver response via WebSocket.
                kyomi_agent::deliver_response(
                    &ws_manager,
                    &spawn_user_id,
                    &spawn_session_id,
                    &exec_result.assistant_message_id,
                    &exec_result.response_text,
                    exec_result.model.as_deref().unwrap_or(kyomi_agent::DEFAULT_MODEL),
                    exec_result.token_usage,
                    &context_type,
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
                    "Agent execution failed"
                );

                // Save error as an assistant message so the user sees it
                // in the chat history. persist_after_chat() may or may not
                // have saved the assistant message depending on where the
                // error occurred, so we use add_message with the known ID.
                // If persist already saved it, update_message sets the error
                // metadata; if not, add_message creates it.
                let error_text = format!(
                    "I encountered an error while processing your request: {e}"
                );
                let error_metadata = serde_json::json!({
                    "status": "error",
                    "error": e.to_string(),
                });
                // Try update first (persist may have saved the message).
                let updated = chat_service::update_message(
                    &db,
                    &encryption_key,
                    &spawn_assistant_message_id,
                    Some(&error_text),
                    Some(&error_metadata),
                )
                .await
                .unwrap_or(false);

                // If no message existed to update, create one.
                if !updated {
                    let _ = chat_service::add_message(
                        &db,
                        &encryption_key,
                        &spawn_session_id,
                        "assistant",
                        &error_text,
                        Some(&error_metadata),
                        Some(&spawn_assistant_message_id),
                        None,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await;
                }

                kyomi_auth::websocket::helpers::send_error(
                    &ws_manager,
                    &spawn_user_id,
                    Some(&spawn_session_id),
                    &format!("AI processing failed: {e}"),
                    Some("agent_error"),
                    Some(&context_type),
                )
                .await;
            }
        }

        // Clean up cancel token so it doesn't leak.
        cancel_registry.remove(&spawn_user_id, &spawn_session_id);
    });

    // Fire-and-forget title generation for new sessions.
    if is_new_session {
        // Only attempt title generation if an LLM provider is configured.
        if kyomi_agent::resolve_provider_config(&state.config).is_ok() {
            kyomi_agent::generate_session_title(
                state.db.clone(),
                state.ws_manager.clone(),
                session_id.clone(),
                user.user_id.clone(),
                first_message,
                state.config.clone(),
            );
        }
    }

    tracing::info!(
        "Dispatched AI processing for session {} (message_id={})",
        session_id,
        user_message_id,
    );

    Ok(Json(json!({
        "session_id": session_id,
        "user_message_id": user_message_id,
        "assistant_message_id": assistant_message_id,
        "status": "processing",
    })))
}

// ===========================================================================
// Phase 8C: Sharing + Collaboration
// ===========================================================================

// ---------------------------------------------------------------------------
// POST /sessions/{session_id}/share -- Share session with workspace
// ---------------------------------------------------------------------------

async fn share_session(
    State(state): State<AppState>,
    user: AuthUser,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Load session and verify ownership.
    #[derive(sqlx::FromRow)]
    struct ShareRow { user_id: String }
    let row = kyomi_core::db_fetch_optional!(
        &state.db, ShareRow,
        "SELECT user_id FROM chat_sessions WHERE session_id = $1",
        &session_id
    )?
    .ok_or_else(|| kyomi_core::Error::NotFound("Session not found".into()))?;
    let owner_id = row.user_id;

    if owner_id != user.user_id {
        return Err(kyomi_core::Error::Forbidden(
            "Only the owner can share this conversation".into(),
        ));
    }

    let now = Utc::now();
    let now_str = now.to_rfc3339();

    kyomi_core::db_execute!(
        &state.db,
        "UPDATE chat_sessions SET shared = true, shared_at = $1 WHERE session_id = $2",
        &now_str,
        &session_id
    )?;

    tracing::info!("Session {} shared by user {}", session_id, user.user_id);

    Ok(Json(json!({
        "shared": true,
        "shared_at": now.to_rfc3339(),
    })))
}

// ---------------------------------------------------------------------------
// POST /sessions/{session_id}/unshare -- Make session private
// ---------------------------------------------------------------------------

async fn unshare_session(
    State(state): State<AppState>,
    user: AuthUser,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    #[derive(sqlx::FromRow)]
    struct UnshareRow { user_id: String, platform_type: Option<String>, platform_thread_key: Option<String> }
    let row = kyomi_core::db_fetch_optional!(
        &state.db, UnshareRow,
        "SELECT user_id, platform_type, platform_thread_key FROM chat_sessions WHERE session_id = $1",
        &session_id
    )?
    .ok_or_else(|| kyomi_core::Error::NotFound("Session not found".into()))?;
    let owner_id = row.user_id;

    if owner_id != user.user_id {
        return Err(kyomi_core::Error::Forbidden(
            "Only the owner can unshare this conversation".into(),
        ));
    }

    // Block unsharing platform channel/group conversations — they're visible
    // to other channel members on the platform so "private" doesn't make sense.
    // Platform DMs (Slack D prefix) can be unshared like regular sessions.
    if let Some(ref platform) = row.platform_type {
        if platform == "slack" {
            // Slack thread keys are "channel_id:thread_ts"; extract channel_id
            let channel_id = row.platform_thread_key.as_deref()
                .and_then(|k| k.split(':').next())
                .unwrap_or("");
            if !channel_id.starts_with('D') {
                return Err(kyomi_core::Error::BadRequest(
                    "Slack channel conversations cannot be unshared because they're \
                     visible to your team in Slack. To have a private conversation, \
                     start a new chat in Kyomi."
                        .into(),
                ));
            }
        }
    }

    kyomi_core::db_execute!(
        &state.db,
        "UPDATE chat_sessions SET shared = false, shared_at = NULL WHERE session_id = $1",
        &session_id
    )?;

    tracing::info!("Session {} unshared by user {}", session_id, user.user_id);

    Ok(Json(json!({ "shared": false })))
}

// ---------------------------------------------------------------------------
// POST /sessions/{session_id}/read -- Mark session as read
// ---------------------------------------------------------------------------

async fn mark_session_read(
    State(state): State<AppState>,
    user: AuthUser,
    Path(session_id): Path<String>,
    body: Option<Json<MarkReadRequest>>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Permission check: user must own or have shared access.
    let session = chat_service::get_session_info(
        &state.db,
        &user.user_id,
        &session_id,
        Some(workspace_id),
    )
    .await?;

    if session.is_none() {
        return Err(kyomi_core::Error::Forbidden(
            "Access denied: you do not have permission to mark this conversation as read".into(),
        ));
    }

    // Get message_id from body, or fall back to latest message.
    let requested_message_id = body.and_then(|b| b.0.message_id);

    let message_id = if let Some(mid) = requested_message_id {
        Some(mid)
    } else {
        // Get latest message in session.
        #[derive(sqlx::FromRow)]
        struct MsgIdRow { message_id: String }
        let latest = kyomi_core::db_fetch_optional!(
            &state.db, MsgIdRow,
            "SELECT message_id FROM chat_messages \
             WHERE session_id = $1 \
             ORDER BY created_at DESC LIMIT 1",
            &session_id
        )?;

        latest.map(|r| r.message_id)
    };

    let now = Utc::now();
    let now_str = now.to_rfc3339();

    // Upsert: INSERT ... ON CONFLICT DO UPDATE.
    kyomi_core::db_execute!(
        &state.db,
        "INSERT INTO conversation_read_status \
         (session_id, user_id, last_read_at, last_read_message_id) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (session_id, user_id) \
         DO UPDATE SET last_read_at = $3, last_read_message_id = $4",
        &session_id,
        &user.user_id,
        &now_str,
        message_id.as_deref()
    )?;

    tracing::info!(
        "User {} marked session {} as read (up to message {:?})",
        user.user_id,
        session_id,
        message_id
    );

    Ok(Json(json!({
        "last_read_at": now.to_rfc3339(),
        "last_read_message_id": message_id,
    })))
}

// ---------------------------------------------------------------------------
// POST /sessions/{session_id}/transfer -- Transfer session ownership
// ---------------------------------------------------------------------------

async fn transfer_ownership(
    State(state): State<AppState>,
    user: AuthUser,
    Path(session_id): Path<String>,
    Json(request): Json<TransferOwnershipRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let new_owner_id = &request.new_owner_user_id;

    if new_owner_id.is_empty() {
        return Err(kyomi_core::Error::BadRequest(
            "new_owner_user_id is required".into(),
        ));
    }

    // Load the session.
    #[derive(sqlx::FromRow)]
    struct SessionOwnerRow { user_id: String, workspace_id: String }
    let row = kyomi_core::db_fetch_optional!(
        &state.db, SessionOwnerRow,
        "SELECT user_id, workspace_id FROM chat_sessions WHERE session_id = $1",
        &session_id
    )?
    .ok_or_else(|| kyomi_core::Error::NotFound("Session not found".into()))?;
    let session_owner_id = row.user_id;
    let session_workspace_id = row.workspace_id;

    // Check if new owner is already the owner.
    if new_owner_id == &session_owner_id {
        return Err(kyomi_core::Error::BadRequest(
            "User is already the owner of this conversation".into(),
        ));
    }

    // Permission check: session owner or workspace owner.
    let is_session_owner = session_owner_id == user.user_id;

    #[derive(sqlx::FromRow)]
    struct WsOwnerRow { owner_user_id: String }
    let workspace_owner = kyomi_core::db_fetch_optional!(
        &state.db, WsOwnerRow,
        "SELECT owner_user_id FROM workspaces WHERE workspace_id = $1",
        &session_workspace_id
    )?;

    let is_workspace_owner = workspace_owner
        .as_ref()
        .map(|r| r.owner_user_id == user.user_id)
        .unwrap_or(false);

    if !is_session_owner && !is_workspace_owner {
        return Err(kyomi_core::Error::Forbidden(
            "Only the session owner or workspace owner can transfer ownership".into(),
        ));
    }

    // Verify new owner is a workspace member.
    #[derive(sqlx::FromRow)]
    struct ExistsRow { n: i32 }
    let membership = kyomi_core::db_fetch_optional!(
        &state.db, ExistsRow,
        "SELECT 1 as n FROM workspace_users \
         WHERE workspace_id = $1 AND user_id = $2",
        &session_workspace_id,
        new_owner_id
    )?;

    if membership.is_none() {
        return Err(kyomi_core::Error::BadRequest(
            "New owner must be a workspace member".into(),
        ));
    }

    // Get new owner's display name.
    #[derive(sqlx::FromRow)]
    struct UserNameRow { name: Option<String>, email: String }
    let owner_row = kyomi_core::db_fetch_optional!(
        &state.db, UserNameRow,
        "SELECT name, email FROM users WHERE user_id = $1",
        new_owner_id
    )?
    .ok_or_else(|| kyomi_core::Error::NotFound("New owner user not found".into()))?;

    let display_name = owner_row.name.unwrap_or(owner_row.email);

    // Transfer ownership.
    kyomi_core::db_execute!(
        &state.db,
        "UPDATE chat_sessions SET user_id = $1 WHERE session_id = $2",
        new_owner_id,
        &session_id
    )?;

    tracing::info!(
        "Session {} ownership transferred from {} to {}",
        session_id,
        user.user_id,
        new_owner_id
    );

    Ok(Json(json!({
        "created_by": {
            "user_id": new_owner_id,
            "display_name": display_name,
        }
    })))
}

// ===========================================================================
// Phase 8C: Message + Chart
// ===========================================================================

// ---------------------------------------------------------------------------
// PATCH /sessions/{session_id}/messages/{message_id} -- Update message content
// ---------------------------------------------------------------------------

async fn update_message_content(
    State(state): State<AppState>,
    user: AuthUser,
    Path((session_id, message_id)): Path<(String, String)>,
    Json(request): Json<UpdateMessageContentRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    if request.content.len() > 100_000 {
        return Err(kyomi_core::Error::BadRequest(
            "Message content exceeds maximum length".into(),
        ));
    }

    // Verify session ownership (only owner can edit messages).
    let session = chat_service::get_session_info(
        &state.db,
        &user.user_id,
        &session_id,
        Some(workspace_id),
    )
    .await?;

    match session {
        Some(s) if s.user_id == user.user_id => {}
        Some(_) => {
            return Err(kyomi_core::Error::Forbidden(
                "Only the session owner can edit messages".into(),
            ));
        }
        None => {
            return Err(kyomi_core::Error::NotFound(
                "Session not found or access denied".into(),
            ));
        }
    }

    // Verify the message belongs to this session.
    #[derive(sqlx::FromRow)]
    struct ExistsRow2 { n: i32 }
    let msg_exists = kyomi_core::db_fetch_optional!(
        &state.db, ExistsRow2,
        "SELECT 1 as n FROM chat_messages \
         WHERE message_id = $1 AND session_id = $2",
        &message_id,
        &session_id
    )?;

    if msg_exists.is_none() {
        return Err(kyomi_core::Error::NotFound("Message not found".into()));
    }

    // Re-encrypt and update.
    let encrypted_content = encryption::encrypt(&request.content, &state.encryption_key)?;

    let result = kyomi_core::db_execute!(
        &state.db,
        "UPDATE chat_messages SET content = $1 WHERE message_id = $2",
        &encrypted_content,
        &message_id
    )?;

    if result.rows_affected() > 0 {
        tracing::info!(
            "Message {} content updated for user {}",
            message_id,
            user.user_id
        );
        Ok(Json(json!({
            "message": "Message content updated successfully"
        })))
    } else {
        Err(kyomi_core::Error::NotFound("Message not found".into()))
    }
}

// ---------------------------------------------------------------------------
// POST /sessions/{session_id}/messages/{message_id}/toggle-pin
// ---------------------------------------------------------------------------

async fn toggle_pin(
    State(state): State<AppState>,
    user: AuthUser,
    Path((session_id, message_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let success = chat_service::toggle_message_pin(
        &state.db,
        &session_id,
        &message_id,
        &user.user_id,
        Some(workspace_id),
    )
    .await?;

    if success {
        tracing::info!(
            "Toggled pin status for message {} in session {}",
            message_id,
            session_id
        );
        Ok(Json(json!({
            "message": "Pin status toggled successfully"
        })))
    } else {
        Err(kyomi_core::Error::NotFound(
            "Message or session not found".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// PATCH /charts/{chart_id} -- Update chart config fields
// ---------------------------------------------------------------------------

async fn update_chart_config(
    State(state): State<AppState>,
    user: AuthUser,
    Path(chart_id): Path<String>,
    Json(updates): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    tracing::info!("PATCH /charts/{} - Updates: {:?}", chart_id, updates);

    // Get existing chart.
    let chart = chat_service::get_chart(&state.db, &chart_id).await?;

    let chart = match chart {
        Some(c) => c,
        None => {
            return Err(kyomi_core::Error::NotFound(
                format!("Chart {} not found", chart_id),
            ));
        }
    };

    // Ownership chain: chart → message → session → user.
    #[derive(sqlx::FromRow)]
    struct SessionIdRow { session_id: String }
    let message_session_id = kyomi_core::db_fetch_optional!(
        &state.db, SessionIdRow,
        "SELECT session_id FROM chat_messages WHERE message_id = $1",
        &chart.message_id
    )?
    .map(|r| r.session_id)
    .ok_or_else(|| kyomi_core::Error::NotFound("Chart message not found".into()))?;

    #[derive(sqlx::FromRow)]
    struct OwnerRow { user_id: String }
    let session_owner = kyomi_core::db_fetch_optional!(
        &state.db, OwnerRow,
        "SELECT user_id FROM chat_sessions WHERE session_id = $1",
        &message_session_id
    )?;

    match session_owner {
        Some(ref row) if row.user_id == user.user_id => {} // Authorized
        Some(ref row) => {
            tracing::warn!(
                "User {} attempted to edit chart {} owned by {}",
                user.user_id,
                chart_id,
                row.user_id
            );
            return Err(kyomi_core::Error::Forbidden(
                "Access denied: you do not own this chart".into(),
            ));
        }
        None => {
            return Err(kyomi_core::Error::NotFound(
                "Chart session not found".into(),
            ));
        }
    }

    // Merge updates into existing chart_data.
    let mut chart_data = chart.chart_data.clone();
    if let (Some(existing), Some(new_fields)) = (chart_data.as_object_mut(), updates.as_object()) {
        for (key, value) in new_fields {
            existing.insert(key.clone(), value.clone());
        }
    }

    let updated = chat_service::update_chart(&state.db, &chart_id, &chart_data).await?;

    if !updated {
        return Err(kyomi_core::Error::NotFound(
            format!("Chart {} not found", chart_id),
        ));
    }

    tracing::info!("Successfully updated chart {}", chart_id);

    Ok(Json(json!({
        "success": true,
        "chart_id": chart_id,
    })))
}

// ---------------------------------------------------------------------------
// PUT /charts/{chart_id}/sql -- Update chart SQL (create new query, update chart)
// ---------------------------------------------------------------------------

async fn update_chart_sql(
    State(state): State<AppState>,
    user: AuthUser,
    Path(chart_id): Path<String>,
    Json(request): Json<UpdateChartSqlRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    if request.sql.is_empty() {
        return Err(kyomi_core::Error::BadRequest("SQL is required".into()));
    }

    tracing::info!(
        "PUT /charts/{}/sql - User {} updating SQL",
        chart_id,
        user.user_id
    );

    // Step 1: Get chart from DB.
    let chart = chat_service::get_chart(&state.db, &chart_id).await?;

    let chart = match chart {
        Some(c) => c,
        None => {
            return Err(kyomi_core::Error::NotFound(
                format!("Chart {} not found", chart_id),
            ));
        }
    };

    // Step 2: Ownership chain verification: chart -> message -> session -> user.
    #[derive(sqlx::FromRow)]
    struct SessionIdRow2 { session_id: String }
    let message_session_id = kyomi_core::db_fetch_optional!(
        &state.db, SessionIdRow2,
        "SELECT session_id FROM chat_messages WHERE message_id = $1",
        &chart.message_id
    )?
    .map(|r| r.session_id)
    .ok_or_else(|| kyomi_core::Error::NotFound("Chart message not found".into()))?;

    #[derive(sqlx::FromRow)]
    struct OwnerRow2 { user_id: String }
    let session_owner = kyomi_core::db_fetch_optional!(
        &state.db, OwnerRow2,
        "SELECT user_id FROM chat_sessions WHERE session_id = $1",
        &message_session_id
    )?;

    match session_owner {
        Some(ref row) if row.user_id == user.user_id => {} // Authorized
        Some(ref row) => {
            tracing::warn!(
                "User {} attempted to edit chart {} owned by {}",
                user.user_id,
                chart_id,
                row.user_id
            );
            return Err(kyomi_core::Error::Forbidden(
                "Access denied: you do not own this chart".into(),
            ));
        }
        None => {
            return Err(kyomi_core::Error::NotFound(
                "Chart session not found".into(),
            ));
        }
    }

    // Step 3: Create new query record with a UUID as the query_id.
    let new_query_id = uuid::Uuid::new_v4().to_string();

    chat_service::store_query(&state.db, &new_query_id, &request.sql).await?;

    tracing::info!(
        "Created new query {} for chart {}",
        new_query_id,
        chart_id
    );

    // Step 4: Update chart_data with new query_id (preserve existing fields like columnMap).
    let mut chart_data = chart.chart_data.clone();
    if let Some(obj) = chart_data.as_object_mut() {
        obj.insert("query_id".to_string(), json!(new_query_id));
    }

    let updated = chat_service::update_chart(&state.db, &chart_id, &chart_data).await?;

    if !updated {
        return Err(kyomi_core::Error::Internal(
            "Failed to update chart".into(),
        ));
    }

    // Step 5: Get updated chart to return complete data.
    let updated_chart = chat_service::get_chart(&state.db, &chart_id).await?;

    let updated_chart_data = updated_chart
        .map(|c| c.chart_data)
        .unwrap_or_else(|| json!({}));

    tracing::info!(
        "Successfully updated chart {} with new query {}",
        chart_id,
        new_query_id
    );

    // Return chart_id + query_id + all chart_data fields (matching Python: {chart_id, query_id, **updated_chart}).
    let mut response = serde_json::Map::new();
    response.insert("chart_id".to_string(), json!(chart_id));
    response.insert("query_id".to_string(), json!(new_query_id));

    if let Some(obj) = updated_chart_data.as_object() {
        for (key, value) in obj {
            response.insert(key.clone(), value.clone());
        }
    }

    Ok(Json(serde_json::Value::Object(response)))
}
