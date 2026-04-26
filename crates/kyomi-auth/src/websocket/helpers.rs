// SPDX-License-Identifier: AGPL-3.0-or-later

//! Convenience functions for sending typed WebSocket messages.
//!
//! One function per message type — constructs `WebSocketMessage` with the correct
//! `MessageType` and data payload, then calls `send_to_user` or `broadcast_to_workspace`.

use kyomi_core::{MessageType, WebSocketMessage};

use super::WebSocketManager;

// ---------------------------------------------------------------------------
// Chat streaming
// ---------------------------------------------------------------------------

/// Send a chat_stream chunk to a user.
pub async fn send_chat_stream(
    manager: &WebSocketManager,
    user_id: &str,
    session_id: &str,
    message_id: &str,
    content: &str,
    context_type: Option<&str>,
) {
    let mut data = serde_json::json!({
        "content": content,
    });
    if let Some(ct) = context_type {
        data["context_type"] = serde_json::Value::String(ct.to_string());
    }

    let msg = WebSocketMessage::new(MessageType::ChatStream)
        .with_session(session_id)
        .with_message_id(message_id)
        .with_data(data);

    manager.send_to_user(user_id, msg).await;
}

/// Parameters for [`send_chat_complete`].
pub struct ChatCompleteParams<'a> {
    pub manager: &'a WebSocketManager,
    pub user_id: &'a str,
    pub session_id: &'a str,
    pub message_id: &'a str,
    pub full_content: &'a str,
    pub model: &'a str,
    pub usage_stats: Option<serde_json::Value>,
    pub context_type: Option<&'a str>,
}

/// Send a chat_complete message when AI response is finished.
pub async fn send_chat_complete(params: ChatCompleteParams<'_>) {
    let ChatCompleteParams {
        manager,
        user_id,
        session_id,
        message_id,
        full_content,
        model,
        usage_stats,
        context_type,
    } = params;

    let mut data = serde_json::json!({
        "full_content": full_content,
        "model": model,
    });
    if let Some(stats) = usage_stats {
        data["usage_stats"] = stats;
    }
    if let Some(ct) = context_type {
        data["context_type"] = serde_json::Value::String(ct.to_string());
    }

    let msg = WebSocketMessage::new(MessageType::ChatComplete)
        .with_session(session_id)
        .with_message_id(message_id)
        .with_data(data);

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// Session events
// ---------------------------------------------------------------------------

/// Send a session_created notification.
pub async fn send_session_created(
    manager: &WebSocketManager,
    user_id: &str,
    session_id: &str,
    session_data: serde_json::Value,
) {
    let msg = WebSocketMessage::new(MessageType::SessionCreated)
        .with_session(session_id)
        .with_data(session_data);

    manager.send_to_user(user_id, msg).await;
}

/// Send a title_update notification.
pub async fn send_title_update(
    manager: &WebSocketManager,
    user_id: &str,
    session_id: &str,
    title: &str,
) {
    let msg = WebSocketMessage::new(MessageType::TitleUpdate)
        .with_session(session_id)
        .with_data(serde_json::json!({"title": title}));

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// Agent thinking / token usage
// ---------------------------------------------------------------------------

/// Send an agent_thinking event.
pub async fn send_agent_thinking(
    manager: &WebSocketManager,
    user_id: &str,
    session_id: &str,
    thinking_event: serde_json::Value,
    message_id: Option<&str>,
) {
    let mut msg = WebSocketMessage::new(MessageType::AgentThinking)
        .with_session(session_id)
        .with_data(thinking_event);
    if let Some(mid) = message_id {
        msg = msg.with_message_id(mid);
    }

    manager.send_to_user(user_id, msg).await;
}

/// Send a token_usage_update.
pub async fn send_token_usage_update(
    manager: &WebSocketManager,
    user_id: &str,
    session_id: &str,
    token_usage: serde_json::Value,
    message_id: Option<&str>,
) {
    let mut msg = WebSocketMessage::new(MessageType::TokenUsageUpdate)
        .with_session(session_id)
        .with_data(token_usage);
    if let Some(mid) = message_id {
        msg = msg.with_message_id(mid);
    }

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Send an error message to a user.
pub async fn send_error(
    manager: &WebSocketManager,
    user_id: &str,
    session_id: Option<&str>,
    error_message: &str,
    error_code: Option<&str>,
    context_type: Option<&str>,
) {
    let mut data = serde_json::json!({
        "error": error_message,
    });
    if let Some(code) = error_code {
        data["error_code"] = serde_json::Value::String(code.to_string());
    }
    if let Some(ct) = context_type {
        data["context_type"] = serde_json::Value::String(ct.to_string());
    }

    let mut msg = WebSocketMessage::new(MessageType::Error).with_data(data);
    if let Some(sid) = session_id {
        msg = msg.with_session(sid);
    }

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// OAuth
// ---------------------------------------------------------------------------

/// Send an oauth_reconnect_required notification.
pub async fn send_oauth_reconnect_required(
    manager: &WebSocketManager,
    user_id: &str,
    workspace_id: &str,
    session_id: &str,
    state_id: &str,
    service: &str,
    message: &str,
) {
    let msg = WebSocketMessage::new(MessageType::OauthReconnectRequired)
        .with_session(session_id)
        .with_data(serde_json::json!({
            "workspace_id": workspace_id,
            "state_id": state_id,
            "service": service,
            "message": message,
        }));

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// Workspace events
// ---------------------------------------------------------------------------

/// Parameters for [`send_workspace_invitation`].
pub struct WorkspaceInvitationParams<'a> {
    pub manager: &'a WebSocketManager,
    pub user_id: &'a str,
    pub invitation_id: &'a str,
    pub workspace_id: &'a str,
    pub workspace_name: &'a str,
    pub invited_by_name: &'a str,
    pub role: &'a str,
    pub message: &'a str,
}

/// Send a workspace_invitation notification to an invitee.
pub async fn send_workspace_invitation(params: WorkspaceInvitationParams<'_>) {
    let WorkspaceInvitationParams {
        manager,
        user_id,
        invitation_id,
        workspace_id,
        workspace_name,
        invited_by_name,
        role,
        message,
    } = params;

    let msg = WebSocketMessage::new(MessageType::WorkspaceInvitation)
        .with_data(serde_json::json!({
            "invitation_id": invitation_id,
            "workspace_id": workspace_id,
            "workspace_name": workspace_name,
            "invited_by_name": invited_by_name,
            "role": role,
            "message": message,
        }));

    manager.send_to_user(user_id, msg).await;
}

/// Send a workspace_removed notification to a removed user.
pub async fn send_workspace_removed(
    manager: &WebSocketManager,
    user_id: &str,
    workspace_id: &str,
    workspace_name: &str,
    message: &str,
) {
    let msg = WebSocketMessage::new(MessageType::WorkspaceRemoved)
        .with_data(serde_json::json!({
            "workspace_id": workspace_id,
            "workspace_name": workspace_name,
            "message": message,
        }));

    manager.send_to_user(user_id, msg).await;
}

/// Send an ownership_transfer_offered notification.
pub async fn send_ownership_transfer_offered(
    manager: &WebSocketManager,
    user_id: &str,
    transfer_id: &str,
    workspace_name: &str,
    from_user_email: &str,
) {
    let msg = WebSocketMessage::new(MessageType::OwnershipTransferOffered)
        .with_data(serde_json::json!({
            "transfer_id": transfer_id,
            "workspace_name": workspace_name,
            "from_user_email": from_user_email,
        }));

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// Watch alerts
// ---------------------------------------------------------------------------

/// Send a watch_alert notification.
pub async fn send_watch_alert(
    manager: &WebSocketManager,
    user_id: &str,
    watch_id: &str,
    watch_name: &str,
    execution_id: &str,
    message: &str,
    summary: &str,
) {
    let msg = WebSocketMessage::new(MessageType::WatchAlert)
        .with_data(serde_json::json!({
            "watch_id": watch_id,
            "watch_name": watch_name,
            "execution_id": execution_id,
            "message": message,
            "summary": summary,
        }));

    manager.send_to_user(user_id, msg).await;
}

/// Send a watch_state_update notification.
///
/// Sent at key points during watch execution to update the frontend's watch list
/// in real-time. Status values: `"running"`, `"success"`, `"no_alert"`, `"error"`.
pub async fn send_watch_state_update(
    manager: &WebSocketManager,
    user_id: &str,
    watch_id: &str,
    status: &str,
) {
    let msg = WebSocketMessage::new(MessageType::WatchStateUpdate)
        .with_data(serde_json::json!({
            "watch_id": watch_id,
            "status": status,
        }));

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// Credential + catalog status
// ---------------------------------------------------------------------------

/// Send a credential_status_changed notification.
pub async fn send_credential_status_changed(
    manager: &WebSocketManager,
    user_id: &str,
    workspace_id: &str,
    datasource_slug: &str,
    status: &str,
    datasource_type: &str,
) {
    let msg = WebSocketMessage::new(MessageType::CredentialStatusChanged)
        .with_data(serde_json::json!({
            "workspace_id": workspace_id,
            "datasource_slug": datasource_slug,
            "status": status,
            "datasource_type": datasource_type,
        }));

    manager.send_to_user(user_id, msg).await;
}

/// Broadcast a catalog_status_update to all workspace members.
pub async fn send_catalog_status_update(
    manager: &WebSocketManager,
    workspace_id: &str,
    status: &str,
    progress: Option<f64>,
    datasource_slug: &str,
    datasource_name: &str,
    datasource_type: &str,
) {
    let mut data = serde_json::json!({
        "status": status,
        "datasource_slug": datasource_slug,
        "datasource_name": datasource_name,
        "datasource_type": datasource_type,
    });
    if let Some(p) = progress {
        data["progress"] = serde_json::json!(p);
    }

    let msg = WebSocketMessage::new(MessageType::CatalogStatusUpdate)
        .with_data(data);

    manager.broadcast_to_workspace(workspace_id, msg, None).await;
}

// ---------------------------------------------------------------------------
// AI usage
// ---------------------------------------------------------------------------

/// Send an ai_usage_update notification.
pub async fn send_ai_usage_update(
    manager: &WebSocketManager,
    user_id: &str,
    workspace_id: &str,
) {
    let msg = WebSocketMessage::new(MessageType::AiUsageUpdate)
        .with_data(serde_json::json!({
            "workspace_id": workspace_id,
        }));

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// Shared conversations
// ---------------------------------------------------------------------------

/// Broadcast shared_conversation_activity to workspace members.
pub async fn send_shared_conversation_activity(
    manager: &WebSocketManager,
    workspace_id: &str,
    session_id: &str,
    message_preview: &str,
    sent_by_user: &str,
) {
    let msg = WebSocketMessage::new(MessageType::SharedConversationActivity)
        .with_session(session_id)
        .with_data(serde_json::json!({
            "message_preview": message_preview,
            "sent_by_user": sent_by_user,
        }));

    manager.broadcast_to_workspace(workspace_id, msg, None).await;
}

/// Broadcast a shared_chat_message to workspace members.
#[allow(clippy::too_many_arguments)]
pub async fn send_shared_chat_message(
    manager: &WebSocketManager,
    workspace_id: &str,
    session_id: &str,
    message_id: &str,
    message_type: &str,
    content: &str,
    timestamp: &str,
    sent_by_user: Option<&str>,
    exclude_user_id: Option<&str>,
    client_msg_id: Option<&str>,
) {
    let mut data = serde_json::json!({
        "type": message_type,
        "content": content,
        "timestamp": timestamp,
    });
    if let Some(user) = sent_by_user {
        data["sent_by"] = serde_json::Value::String(user.to_string());
    }
    if let Some(cid) = client_msg_id {
        data["client_msg_id"] = serde_json::Value::String(cid.to_string());
    }

    let msg = WebSocketMessage::new(MessageType::SharedChatMessage)
        .with_session(session_id)
        .with_message_id(message_id)
        .with_data(data);

    manager
        .broadcast_to_workspace(workspace_id, msg, exclude_user_id)
        .await;
}

/// Send a user's own message echo back to themselves.
pub async fn send_user_message_to_self(
    manager: &WebSocketManager,
    user_id: &str,
    session_id: &str,
    message_id: &str,
    content: &str,
    timestamp: &str,
    user_display_name: &str,
) {
    let msg = WebSocketMessage::new(MessageType::SharedChatMessage)
        .with_session(session_id)
        .with_message_id(message_id)
        .with_data(serde_json::json!({
            "type": "user",
            "content": content,
            "timestamp": timestamp,
            "sent_by": user_display_name,
        }));

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// Broadcast variants (for shared conversation viewers)
// ---------------------------------------------------------------------------

/// Broadcast agent_thinking to workspace members viewing a shared session.
pub async fn broadcast_agent_thinking(
    manager: &WebSocketManager,
    workspace_id: &str,
    session_id: &str,
    thinking_event: serde_json::Value,
    message_id: Option<&str>,
    exclude_user_id: Option<&str>,
) {
    let mut msg = WebSocketMessage::new(MessageType::AgentThinking)
        .with_session(session_id)
        .with_data(thinking_event);
    if let Some(mid) = message_id {
        msg = msg.with_message_id(mid);
    }

    manager
        .broadcast_to_workspace(workspace_id, msg, exclude_user_id)
        .await;
}

/// Parameters for [`broadcast_chat_complete`].
pub struct BroadcastChatCompleteParams<'a> {
    pub manager: &'a WebSocketManager,
    pub workspace_id: &'a str,
    pub session_id: &'a str,
    pub message_id: &'a str,
    pub full_content: &'a str,
    pub model: &'a str,
    pub usage_stats: Option<serde_json::Value>,
    pub exclude_user_id: Option<&'a str>,
}

/// Broadcast chat_complete to workspace members viewing a shared session.
pub async fn broadcast_chat_complete(params: BroadcastChatCompleteParams<'_>) {
    let BroadcastChatCompleteParams {
        manager,
        workspace_id,
        session_id,
        message_id,
        full_content,
        model,
        usage_stats,
        exclude_user_id,
    } = params;

    let mut data = serde_json::json!({
        "full_content": full_content,
        "model": model,
    });
    if let Some(stats) = usage_stats {
        data["usage_stats"] = stats;
    }

    let msg = WebSocketMessage::new(MessageType::ChatComplete)
        .with_session(session_id)
        .with_message_id(message_id)
        .with_data(data);

    manager
        .broadcast_to_workspace(workspace_id, msg, exclude_user_id)
        .await;
}

/// Broadcast token_usage_update to workspace members viewing a shared session.
pub async fn broadcast_token_usage_update(
    manager: &WebSocketManager,
    workspace_id: &str,
    session_id: &str,
    token_usage: serde_json::Value,
    message_id: Option<&str>,
    exclude_user_id: Option<&str>,
) {
    let mut msg = WebSocketMessage::new(MessageType::TokenUsageUpdate)
        .with_session(session_id)
        .with_data(token_usage);
    if let Some(mid) = message_id {
        msg = msg.with_message_id(mid);
    }

    manager
        .broadcast_to_workspace(workspace_id, msg, exclude_user_id)
        .await;
}

// ---------------------------------------------------------------------------
// Dashboard
// ---------------------------------------------------------------------------

/// Send a member_role_changed notification to the affected member.
pub async fn send_member_role_changed(
    manager: &WebSocketManager,
    user_id: &str,
    workspace_id: &str,
    workspace_name: &str,
    new_role: &str,
) {
    let msg = WebSocketMessage::new(MessageType::MemberRoleChanged)
        .with_data(serde_json::json!({
            "workspace_id": workspace_id,
            "workspace_name": workspace_name,
            "new_role": new_role,
            "message": format!("Your role has been changed to {new_role}"),
        }));

    manager.send_to_user(user_id, msg).await;
}

/// Broadcast a member_joined notification to all workspace members.
pub async fn send_member_joined(
    manager: &WebSocketManager,
    workspace_id: &str,
    user_name: &str,
    role: &str,
) {
    let msg = WebSocketMessage::new(MessageType::MemberJoined)
        .with_data(serde_json::json!({
            "workspace_id": workspace_id,
            "user_name": user_name,
            "role": role,
            "message": format!("{user_name} joined the workspace"),
        }));

    manager.broadcast_to_workspace(workspace_id, msg, None).await;
}

/// Send an ownership_transfer_completed notification to the previous owner.
pub async fn send_ownership_transfer_completed(
    manager: &WebSocketManager,
    user_id: &str,
    transfer_id: &str,
    workspace_id: &str,
    workspace_name: &str,
    new_owner_name: &str,
) {
    let msg = WebSocketMessage::new(MessageType::OwnershipTransferCompleted)
        .with_data(serde_json::json!({
            "transfer_id": transfer_id,
            "workspace_id": workspace_id,
            "workspace_name": workspace_name,
            "new_owner_name": new_owner_name,
            "message": format!("{new_owner_name} accepted ownership of {workspace_name}"),
        }));

    manager.send_to_user(user_id, msg).await;
}

/// Send an ownership_transfer_declined notification to the original owner.
pub async fn send_ownership_transfer_declined(
    manager: &WebSocketManager,
    user_id: &str,
    transfer_id: &str,
    workspace_id: &str,
    workspace_name: &str,
    declined_by_name: &str,
) {
    let msg = WebSocketMessage::new(MessageType::OwnershipTransferDeclined)
        .with_data(serde_json::json!({
            "transfer_id": transfer_id,
            "workspace_id": workspace_id,
            "workspace_name": workspace_name,
            "declined_by_name": declined_by_name,
            "message": format!("{declined_by_name} declined ownership of {workspace_name}"),
        }));

    manager.send_to_user(user_id, msg).await;
}

// ---------------------------------------------------------------------------
// Dashboard CRUD events
// ---------------------------------------------------------------------------

/// Broadcast a dashboard_update to all workspace members (except the author).
pub async fn send_dashboard_update(
    manager: &WebSocketManager,
    workspace_id: &str,
    dashboard_id: &str,
    action: &str,
    changed_by: &str,
    changed_by_name: &str,
    exclude_user_id: Option<&str>,
) {
    let msg = WebSocketMessage::new(MessageType::DashboardUpdate)
        .with_data(serde_json::json!({
            "action": action,
            "dashboard_id": dashboard_id,
            "changed_by": changed_by,
            "changed_by_name": changed_by_name,
        }));

    manager
        .broadcast_to_workspace(workspace_id, msg, exclude_user_id)
        .await;
}

// ---------------------------------------------------------------------------
// Datasource CRUD events
// ---------------------------------------------------------------------------

/// Broadcast a datasource_update to all workspace members (except the actor).
pub async fn send_datasource_update(
    manager: &WebSocketManager,
    workspace_id: &str,
    datasource_id: &str,
    action: &str,
    changed_by: &str,
    changed_by_name: &str,
    exclude_user_id: Option<&str>,
) {
    let msg = WebSocketMessage::new(MessageType::DatasourceUpdate)
        .with_data(serde_json::json!({
            "action": action,
            "datasource_id": datasource_id,
            "changed_by": changed_by,
            "changed_by_name": changed_by_name,
        }));

    manager
        .broadcast_to_workspace(workspace_id, msg, exclude_user_id)
        .await;
}

// ---------------------------------------------------------------------------
// Watch CRUD events
// ---------------------------------------------------------------------------

/// Broadcast a watch_update to all workspace members (except the actor).
pub async fn send_watch_update(
    manager: &WebSocketManager,
    workspace_id: &str,
    watch_id: &str,
    action: &str,
    changed_by: &str,
    changed_by_name: &str,
    exclude_user_id: Option<&str>,
) {
    let msg = WebSocketMessage::new(MessageType::WatchUpdate)
        .with_data(serde_json::json!({
            "action": action,
            "watch_id": watch_id,
            "changed_by": changed_by,
            "changed_by_name": changed_by_name,
        }));

    manager
        .broadcast_to_workspace(workspace_id, msg, exclude_user_id)
        .await;
}

// ---------------------------------------------------------------------------
// Live sync broadcasts
// ---------------------------------------------------------------------------

/// Broadcast a SyncAction to all connected workspace members.
/// Used for live sync — clients receive these to update their local cache.
pub async fn send_sync_action(
    manager: &WebSocketManager,
    workspace_id: &str,
    sync_action: &kyomi_types::sync::SyncAction,
    exclude_user_id: Option<&str>,
) {
    let msg = WebSocketMessage::new(MessageType::SyncAction)
        .with_data(serde_json::to_value(sync_action).unwrap_or_default());

    manager
        .broadcast_to_workspace(workspace_id, msg, exclude_user_id)
        .await;
}

/// Send a dashboard_summary_ready notification.
pub async fn send_dashboard_summary_ready(
    manager: &WebSocketManager,
    user_id: &str,
    dashboard_id: &str,
    summary: &str,
    content: &str,
) {
    let msg = WebSocketMessage::new(MessageType::DashboardUpdate)
        .with_data(serde_json::json!({
            "dashboard_id": dashboard_id,
            "summary": summary,
            "content": content,
            "context_type": "dashboard_summary_ready",
        }));

    manager.send_to_user(user_id, msg).await;
}
