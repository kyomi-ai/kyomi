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

/// Send a request_cancelled event to a user.
pub async fn send_request_cancelled(
    manager: &WebSocketManager,
    user_id: &str,
    session_id: &str,
    message_id: &str,
    context_type: Option<&str>,
) {
    let mut data = serde_json::json!({});
    if let Some(ct) = context_type {
        data["context_type"] = serde_json::Value::String(ct.to_string());
    }

    let msg = WebSocketMessage::new(MessageType::RequestCancelled)
        .with_session(session_id)
        .with_message_id(message_id)
        .with_data(data);

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

/// Broadcast a dashboard or knowledge doc mutation to all workspace members.
///
/// Fetches the full entity snapshot from the database and resolves the correct
/// `entity_type` (dashboard vs knowledge) from the stored `doc_type`. For
/// delete actions the snapshot is skipped (the entity is already gone).
pub async fn broadcast_dashboard_sync(
    db: &kyomi_core::DbPool,
    manager: &WebSocketManager,
    dashboard_id: &str,
    workspace_id: &str,
    action: kyomi_types::sync::SyncActionType,
    user_id: &str,
) {
    use kyomi_types::sync::{SyncAction, SyncActionType, entity_types};

    let (entity_type, data) = if matches!(action, SyncActionType::Delete) {
        (entity_types::DASHBOARD.to_string(), None)
    } else {
        match crate::dashboard_service::fetch_dashboard_snapshot(db, dashboard_id, user_id).await {
            Ok(Some(snapshot)) => {
                let et = snapshot
                    .get("doc_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or(entity_types::DASHBOARD)
                    .to_string();
                (et, Some(snapshot))
            }
            Ok(None) => {
                tracing::warn!(
                    dashboard_id,
                    "dashboard sync: snapshot unavailable; skipping broadcast"
                );
                return;
            }
            Err(e) => {
                tracing::error!(
                    dashboard_id,
                    error = %e,
                    "dashboard sync: fetch failed; skipping broadcast"
                );
                return;
            }
        }
    };

    let sync_action = SyncAction {
        sync_id: 0,
        entity_type,
        entity_id: dashboard_id.to_string(),
        workspace_id: workspace_id.to_string(),
        action,
        data,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    // Route based on visibility: public docs go to all workspace members;
    // private docs go only to the document owner.
    let is_public =
        crate::dashboard_service::is_doc_publicly_visible(db, dashboard_id).await;
    if is_public {
        send_sync_action(manager, workspace_id, &sync_action, None).await;
    } else {
        let msg = WebSocketMessage::new(MessageType::SyncAction)
            .with_data(serde_json::to_value(&sync_action).unwrap_or_default());
        manager.send_to_user(user_id, msg).await;
    }
}

/// Broadcast a dashboard/knowledge doc's visibility transition caused by a
/// collection-membership change or a collection's `is_public` flip — not a
/// content edit. `now_public` is the visibility *after* the transition.
///
/// - Going public (`now_public = true`): sends an `Update` with the fresh
///   snapshot to every workspace member **except** the owner, who already
///   has it.
/// - Going private (`now_public = false`): sends a `Delete` to every
///   workspace member **except** the owner (so non-owners evict their
///   cached copy) and a separate `Update` with the fresh snapshot **to**
///   the owner (so they keep it, now correctly scoped as private) —
///   mirrors [`broadcast_chat_session_unshare`].
///
/// Fetches the snapshot with `owner_user_id` as the requesting user, which
/// always satisfies `dashboard_service::visibility_predicate` regardless of
/// collection state (the owner clause is unconditional). If the row is
/// genuinely gone (deleted mid-transition) or the fetch itself fails, no
/// sync action is emitted at all — distinguished in logs via
/// [`dashboard_service::fetch_dashboard_snapshot`]'s `Result` (KYO-245).
/// That's recoverable either way: the caller's matching `sync_log` write
/// (see `collection_service::write_visibility_sync_log`) still lets an
/// offline member converge on their next delta sync. Emitting a
/// non-`Delete` action with `data: None` is not recoverable the same way
/// (KYO-218), so this never does that.
pub async fn broadcast_dashboard_visibility_change(
    db: &kyomi_core::DbPool,
    manager: &WebSocketManager,
    dashboard_id: &str,
    workspace_id: &str,
    owner_user_id: &str,
    now_public: bool,
) {
    use kyomi_types::sync::{SyncAction, SyncActionType, entity_types};

    let snapshot = match crate::dashboard_service::fetch_dashboard_snapshot(
        db,
        dashboard_id,
        owner_user_id,
    )
    .await
    {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            tracing::warn!(
                dashboard_id,
                now_public,
                "dashboard visibility broadcast: snapshot unavailable; skipping broadcast"
            );
            return;
        }
        Err(e) => {
            tracing::error!(
                dashboard_id,
                now_public,
                error = %e,
                "dashboard visibility broadcast: fetch failed; skipping broadcast"
            );
            return;
        }
    };

    let entity_type = snapshot
        .get("doc_type")
        .and_then(|v| v.as_str())
        .unwrap_or(entity_types::DASHBOARD)
        .to_string();

    if !now_public {
        // Evict from every non-owner's cache immediately.
        let delete_action = SyncAction {
            sync_id: 0,
            entity_type: entity_type.clone(),
            entity_id: dashboard_id.to_string(),
            workspace_id: workspace_id.to_string(),
            action: SyncActionType::Delete,
            data: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        send_sync_action(manager, workspace_id, &delete_action, Some(owner_user_id)).await;
    }

    let update_action = SyncAction {
        sync_id: 0,
        entity_type,
        entity_id: dashboard_id.to_string(),
        workspace_id: workspace_id.to_string(),
        action: SyncActionType::Update,
        data: Some(snapshot),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    if now_public {
        // Owner already has it — push the newly-visible snapshot to everyone else.
        send_sync_action(manager, workspace_id, &update_action, Some(owner_user_id)).await;
    } else {
        // Owner keeps it, now correctly scoped as private.
        let msg = WebSocketMessage::new(MessageType::SyncAction)
            .with_data(serde_json::to_value(&update_action).unwrap_or_default());
        manager.send_to_user(owner_user_id, msg).await;
    }
}

/// Broadcast a watch mutation to its owner only.
///
/// Fetches the watch from the database and serializes the full model so the
/// client sync engine can deserialize it as `WatchListItem`. For delete
/// actions the snapshot is skipped.
///
/// Watches and their alert history have no sharing model — they are strictly
/// private to their creator — so unlike [`broadcast_dashboard_sync`] there is
/// no public/private branch: this always routes to `owner_user_id` via
/// `send_to_user`, never a workspace-wide broadcast.
pub async fn broadcast_watch_sync(
    db: &kyomi_core::DbPool,
    manager: &WebSocketManager,
    watch_id: &str,
    workspace_id: &str,
    action: kyomi_types::sync::SyncActionType,
    owner_user_id: &str,
) {
    use kyomi_types::sync::{SyncAction, SyncActionType, entity_types};

    let data = if matches!(action, SyncActionType::Delete) {
        None
    } else {
        match crate::watch_service::get_watch(db, watch_id, workspace_id, owner_user_id).await {
            Ok(Some(watch)) => match serde_json::to_value(&watch) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::error!(
                        %watch_id,
                        error = %e,
                        "watch sync: failed to serialize watch; skipping broadcast"
                    );
                    return;
                }
            },
            Ok(None) => {
                tracing::warn!(%watch_id, "watch sync: watch not found; skipping broadcast");
                return;
            }
            Err(e) => {
                tracing::error!(%watch_id, error = %e, "watch sync: fetch failed; skipping broadcast");
                return;
            }
        }
    };

    let sync_action = SyncAction {
        sync_id: 0,
        entity_type: entity_types::WATCH.to_string(),
        entity_id: watch_id.to_string(),
        workspace_id: workspace_id.to_string(),
        action,
        data,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    let msg = WebSocketMessage::new(MessageType::SyncAction)
        .with_data(serde_json::to_value(&sync_action).unwrap_or_default());
    manager.send_to_user(owner_user_id, msg).await;
}

/// Broadcast a chat session mutation to workspace members.
///
/// For shared sessions the sync action is broadcast to all workspace members.
/// For private (unshared) sessions the action is sent only to the session
/// owner, matching the visibility routing used by [`broadcast_dashboard_sync`].
pub async fn broadcast_chat_session_sync(
    db: &kyomi_core::DbPool,
    manager: &WebSocketManager,
    session_id: &str,
    workspace_id: &str,
    action: kyomi_types::sync::SyncActionType,
    user_id: &str,
) {
    use kyomi_types::sync::{SyncAction, SyncActionType, entity_types};

    let (data, is_shared) = if matches!(action, SyncActionType::Delete) {
        (None, false)
    } else {
        match crate::chat_service::fetch_session_snapshot(db, session_id).await {
            Ok(Some((_ws_id, snapshot))) => {
                let shared = snapshot
                    .get("shared")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                (Some(snapshot), shared)
            }
            Ok(None) => {
                tracing::warn!(
                    session_id,
                    "chat session sync broadcast: snapshot unavailable; skipping broadcast"
                );
                return;
            }
            Err(e) => {
                tracing::error!(
                    session_id,
                    error = %e,
                    "chat session sync broadcast: snapshot fetch failed; skipping broadcast"
                );
                return;
            }
        }
    };

    let sync_action = SyncAction {
        sync_id: 0,
        entity_type: entity_types::CHAT_SESSION.to_string(),
        entity_id: session_id.to_string(),
        workspace_id: workspace_id.to_string(),
        action,
        data,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    if is_shared {
        send_sync_action(manager, workspace_id, &sync_action, None).await;
    } else {
        let msg = WebSocketMessage::new(MessageType::SyncAction)
            .with_data(serde_json::to_value(&sync_action).unwrap_or_default());
        manager.send_to_user(user_id, msg).await;
    }
}

/// Broadcast the unshare (privatization) of a chat session.
///
/// Sends a `Delete` sync action to all workspace members except the owner
/// (so non-owners remove it from their cache) and an `Update` with the
/// latest snapshot to the owner (so they keep it with updated visibility).
pub async fn broadcast_chat_session_unshare(
    db: &kyomi_core::DbPool,
    manager: &WebSocketManager,
    session_id: &str,
    workspace_id: &str,
    owner_user_id: &str,
) {
    use kyomi_types::sync::{SyncAction, SyncActionType, entity_types};

    let delete_action = SyncAction {
        sync_id: 0,
        entity_type: entity_types::CHAT_SESSION.to_string(),
        entity_id: session_id.to_string(),
        workspace_id: workspace_id.to_string(),
        action: SyncActionType::Delete,
        data: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    // Send delete to all except owner so non-owners remove the session.
    send_sync_action(manager, workspace_id, &delete_action, Some(owner_user_id)).await;

    // Send update to owner with latest snapshot so they keep it.
    match crate::chat_service::fetch_session_snapshot(db, session_id).await {
        Ok(Some((_ws, snapshot))) => {
            let update_action = SyncAction {
                sync_id: 0,
                entity_type: entity_types::CHAT_SESSION.to_string(),
                entity_id: session_id.to_string(),
                workspace_id: workspace_id.to_string(),
                action: SyncActionType::Update,
                data: Some(snapshot),
                timestamp: chrono::Utc::now().to_rfc3339(),
            };
            let msg = WebSocketMessage::new(MessageType::SyncAction)
                .with_data(serde_json::to_value(&update_action).unwrap_or_default());
            manager.send_to_user(owner_user_id, msg).await;
        }
        Ok(None) => {
            tracing::warn!(
                session_id,
                "chat session unshare broadcast: snapshot unavailable; skipping owner update"
            );
        }
        Err(e) => {
            tracing::error!(
                session_id,
                error = %e,
                "chat session unshare broadcast: snapshot fetch failed; skipping owner update"
            );
        }
    }
}

/// Broadcast an entity deletion to all workspace members.
///
/// Generic helper for any entity type — no snapshot needed since the entity
/// is already gone from the database.
pub async fn broadcast_entity_delete(
    manager: &WebSocketManager,
    entity_type: &str,
    entity_id: &str,
    workspace_id: &str,
) {
    let sync_action = kyomi_types::sync::SyncAction {
        sync_id: 0,
        entity_type: entity_type.to_string(),
        entity_id: entity_id.to_string(),
        workspace_id: workspace_id.to_string(),
        action: kyomi_types::sync::SyncActionType::Delete,
        data: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    send_sync_action(manager, workspace_id, &sync_action, None).await;
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

// ─── Tests ───────────────────────────────────────────────────────────────────
//
// KYO-329: this module had zero tests despite owning the routing logic that
// decides who is told about a state change and what payload they receive.
// The dangerous failure mode (KYO-218) is a non-`Delete` `SyncAction` with
// `data: None` — that's the wire protocol's Delete signal, so a mistake here
// tells every connected client to evict an entity that's still there.
//
// These are real integration tests against an in-memory SQLite pool (with
// migrations applied) and a real `WebSocketManager` in single-instance mode
// (no Redis) — messages are delivered synchronously into the connection's
// `mpsc::Receiver`, so no sleeps/polling are needed to observe them.
#[cfg(test)]
mod tests {
    use super::*;
    use kyomi_core::DbPool;
    use kyomi_types::sync::{SyncAction, SyncActionType};
    use sqlx::sqlite::SqlitePoolOptions;
    use tokio::sync::mpsc;

    // ── Fixture setup ────────────────────────────────────────────────────

    async fn test_pool() -> DbPool {
        let _ = kyomi_core::constants::load_with_fallback();

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .expect("enable foreign keys");

        sqlx::migrate!("../../apps/server/migrations-sqlite")
            .run(&pool)
            .await
            .expect("run sqlite migrations");

        DbPool::Sqlite(pool)
    }

    fn sqlite_pool(db: &DbPool) -> &sqlx::SqlitePool {
        match db {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        }
    }

    async fn seed_user(sq: &sqlx::SqlitePool, user_id: &str, email: &str) {
        sqlx::query("INSERT INTO users (user_id, email) VALUES ($1, $2)")
            .bind(user_id)
            .bind(email)
            .execute(sq)
            .await
            .expect("insert user");
    }

    async fn seed_workspace(sq: &sqlx::SqlitePool, workspace_id: &str, owner_user_id: &str) {
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $2, $3)",
        )
        .bind(workspace_id)
        .bind(format!("Workspace {workspace_id}"))
        .bind(owner_user_id)
        .execute(sq)
        .await
        .expect("insert workspace");
    }

    async fn seed_workspace_member(sq: &sqlx::SqlitePool, workspace_id: &str, user_id: &str) {
        sqlx::query(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
             VALUES ($1, $2, 'user', 1)",
        )
        .bind(workspace_id)
        .bind(user_id)
        .execute(sq)
        .await
        .expect("insert workspace_users");
    }

    async fn seed_dashboard(
        sq: &sqlx::SqlitePool,
        dashboard_id: &str,
        user_id: &str,
        workspace_id: &str,
        title: &str,
    ) {
        sqlx::query(
            "INSERT INTO dashboards \
             (dashboard_id, user_id, workspace_id, title, content, doc_type) \
             VALUES ($1, $2, $3, $4, '# content', 'dashboard')",
        )
        .bind(dashboard_id)
        .bind(user_id)
        .bind(workspace_id)
        .bind(title)
        .execute(sq)
        .await
        .expect("insert dashboard");
    }

    async fn seed_public_collection(
        sq: &sqlx::SqlitePool,
        collection_id: &str,
        workspace_id: &str,
        created_by: &str,
    ) {
        sqlx::query(
            "INSERT INTO collections (id, workspace_id, name, is_public, created_by) \
             VALUES ($1, $2, $3, 1, $4)",
        )
        .bind(collection_id)
        .bind(workspace_id)
        .bind(format!("Public collection {collection_id}"))
        .bind(created_by)
        .execute(sq)
        .await
        .expect("insert public collection");
    }

    async fn link_dashboard_to_collection(
        sq: &sqlx::SqlitePool,
        collection_id: &str,
        dashboard_id: &str,
    ) {
        sqlx::query(
            "INSERT INTO collection_dashboards (collection_id, dashboard_id) VALUES ($1, $2)",
        )
        .bind(collection_id)
        .bind(dashboard_id)
        .execute(sq)
        .await
        .expect("insert collection_dashboards");
    }

    async fn seed_watch(
        sq: &sqlx::SqlitePool,
        watch_id: &str,
        workspace_id: &str,
        created_by: &str,
        name: &str,
    ) {
        sqlx::query(
            "INSERT INTO watches (watch_id, workspace_id, created_by, name, prompt, schedule) \
             VALUES ($1, $2, $3, $4, 'Check something', '0 9 * * *')",
        )
        .bind(watch_id)
        .bind(workspace_id)
        .bind(created_by)
        .bind(name)
        .execute(sq)
        .await
        .expect("insert watch");
    }

    async fn seed_chat_session(
        sq: &sqlx::SqlitePool,
        session_id: &str,
        user_id: &str,
        workspace_id: &str,
        title: &str,
        shared: bool,
    ) {
        sqlx::query(
            "INSERT INTO chat_sessions \
             (session_id, user_id, workspace_id, title, model, session_type, shared) \
             VALUES ($1, $2, $3, $4, 'test-model', 'chat', $5)",
        )
        .bind(session_id)
        .bind(user_id)
        .bind(workspace_id)
        .bind(title)
        .bind(shared)
        .execute(sq)
        .await
        .expect("insert chat session");
    }

    /// Seeds `owner` + `other` as members of `ws-1`, then connects both to a
    /// fresh single-instance `WebSocketManager`, draining the initial
    /// heartbeat each `connect()` sends. Shared by every test below so the
    /// fixture setup isn't duplicated six times over.
    async fn setup_workspace_and_connections(
        db: &DbPool,
    ) -> (WebSocketManager, mpsc::Receiver<String>, mpsc::Receiver<String>) {
        let sq = sqlite_pool(db);
        seed_user(sq, "owner", "owner@test.local").await;
        seed_user(sq, "other", "other@test.local").await;
        seed_workspace(sq, "ws-1", "owner").await;
        seed_workspace_member(sq, "ws-1", "owner").await;
        seed_workspace_member(sq, "ws-1", "other").await;

        let manager = WebSocketManager::new(None, db.clone());
        let (_owner_conn, mut rx_owner) = manager.connect("owner").expect("connect owner");
        let (_other_conn, mut rx_other) = manager.connect("other").expect("connect other");
        assert!(
            rx_owner
                .try_recv()
                .expect("connect() must send an immediate heartbeat")
                .contains("heartbeat")
        );
        assert!(
            rx_other
                .try_recv()
                .expect("connect() must send an immediate heartbeat")
                .contains("heartbeat")
        );
        (manager, rx_owner, rx_other)
    }

    /// Drain every currently-buffered message on `rx`, parse each as a
    /// `WebSocketMessage`, and return only the `SyncAction` payloads.
    /// `connect()` sends an immediate Heartbeat and other message types can
    /// share a receiver, so callers must never assume the next message is a
    /// sync action.
    fn drain_sync_actions(rx: &mut mpsc::Receiver<String>) -> Vec<SyncAction> {
        let mut actions = Vec::new();
        while let Ok(raw) = rx.try_recv() {
            let envelope: WebSocketMessage = serde_json::from_str(&raw)
                .unwrap_or_else(|e| panic!("invalid WebSocketMessage JSON: {e}: {raw}"));
            if envelope.message_type != MessageType::SyncAction {
                continue;
            }
            let data = envelope
                .data
                .expect("a sync_action message must carry a `data` field");
            let action: SyncAction = serde_json::from_value(data)
                .expect("sync_action `data` must deserialize as a SyncAction");
            actions.push(action);
        }
        actions
    }

    fn assert_no_sync_action(rx: &mut mpsc::Receiver<String>, context: &str) {
        let actions = drain_sync_actions(rx);
        assert!(
            actions.is_empty(),
            "{context}: expected no sync action, got {actions:?}"
        );
    }

    fn expect_single_sync_action(rx: &mut mpsc::Receiver<String>, context: &str) -> SyncAction {
        let mut actions = drain_sync_actions(rx);
        assert_eq!(
            actions.len(),
            1,
            "{context}: expected exactly one sync action, got {actions:?}"
        );
        actions.remove(0)
    }

    // ── KYO-218 invariant: every helper in this module, exercised together ──

    /// The highest-value single test in this module: drives every
    /// `SyncAction`-emitting helper — across both `Delete` and non-`Delete`
    /// actions, and across every routing branch — and asserts the invariant
    /// that actually matters (KYO-218): a non-`Delete` `SyncAction` must
    /// never carry `data: None`, because that's indistinguishable on the
    /// wire from a Delete and tells the client to evict an entity that's
    /// still there.
    #[tokio::test]
    async fn no_broadcast_helper_ever_emits_a_non_delete_sync_action_with_data_none() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;
        let sq = sqlite_pool(&db);

        seed_dashboard(sq, "dash-1", "owner", "ws-1", "Dash One").await;
        seed_public_collection(sq, "col-public", "ws-1", "owner").await;
        link_dashboard_to_collection(sq, "col-public", "dash-1").await;

        seed_watch(sq, "watch-1", "ws-1", "owner", "Watch One").await;
        seed_chat_session(sq, "sess-shared", "owner", "ws-1", "Shared", true).await;
        seed_chat_session(sq, "sess-private", "owner", "ws-1", "Private", false).await;

        broadcast_dashboard_sync(&db, &manager, "dash-1", "ws-1", SyncActionType::Update, "owner")
            .await;
        broadcast_dashboard_sync(&db, &manager, "dash-1", "ws-1", SyncActionType::Delete, "owner")
            .await;
        broadcast_dashboard_visibility_change(&db, &manager, "dash-1", "ws-1", "owner", true)
            .await;
        broadcast_dashboard_visibility_change(&db, &manager, "dash-1", "ws-1", "owner", false)
            .await;
        broadcast_watch_sync(&db, &manager, "watch-1", "ws-1", SyncActionType::Update, "owner")
            .await;
        broadcast_watch_sync(&db, &manager, "watch-1", "ws-1", SyncActionType::Delete, "owner")
            .await;
        broadcast_chat_session_sync(
            &db,
            &manager,
            "sess-shared",
            "ws-1",
            SyncActionType::Update,
            "owner",
        )
        .await;
        broadcast_chat_session_sync(
            &db,
            &manager,
            "sess-private",
            "ws-1",
            SyncActionType::Update,
            "owner",
        )
        .await;
        broadcast_chat_session_sync(
            &db,
            &manager,
            "sess-private",
            "ws-1",
            SyncActionType::Delete,
            "owner",
        )
        .await;
        broadcast_chat_session_unshare(&db, &manager, "sess-shared", "ws-1", "owner").await;
        broadcast_entity_delete(&manager, "dashboard", "dash-1", "ws-1").await;

        let manual = SyncAction {
            sync_id: 0,
            entity_type: "dashboard".to_string(),
            entity_id: "dash-manual".to_string(),
            workspace_id: "ws-1".to_string(),
            action: SyncActionType::Update,
            data: Some(serde_json::json!({"ok": true})),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        send_sync_action(&manager, "ws-1", &manual, None).await;

        let mut all_actions = drain_sync_actions(&mut rx_owner);
        all_actions.extend(drain_sync_actions(&mut rx_other));

        assert!(
            !all_actions.is_empty(),
            "the fixture must actually produce sync actions for this test to mean anything"
        );

        for action in &all_actions {
            if !matches!(action.action, SyncActionType::Delete) {
                assert!(
                    action.data.is_some(),
                    "a non-Delete SyncAction must never carry data: None — that's the wire \
                     protocol's Delete signal (KYO-218) and tells every client to evict an \
                     entity that's still there: {action:?}"
                );
            }
        }
    }

    // ── Routing: who actually receives each action ───────────────────────

    #[tokio::test]
    async fn broadcast_dashboard_sync_routes_public_to_workspace_private_to_owner_only() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;
        let sq = sqlite_pool(&db);

        seed_dashboard(sq, "dash-public", "owner", "ws-1", "Public Dash").await;
        seed_public_collection(sq, "col-public", "ws-1", "owner").await;
        link_dashboard_to_collection(sq, "col-public", "dash-public").await;
        seed_dashboard(sq, "dash-private", "owner", "ws-1", "Private Dash").await;

        broadcast_dashboard_sync(
            &db,
            &manager,
            "dash-public",
            "ws-1",
            SyncActionType::Update,
            "owner",
        )
        .await;
        let owner_public = expect_single_sync_action(&mut rx_owner, "public dash, owner");
        let other_public = expect_single_sync_action(&mut rx_other, "public dash, non-owner");
        assert_eq!(owner_public.entity_id, "dash-public");
        assert_eq!(other_public.entity_id, "dash-public");

        broadcast_dashboard_sync(
            &db,
            &manager,
            "dash-private",
            "ws-1",
            SyncActionType::Update,
            "owner",
        )
        .await;
        let owner_private = expect_single_sync_action(&mut rx_owner, "private dash, owner");
        assert_eq!(owner_private.entity_id, "dash-private");
        assert_no_sync_action(&mut rx_other, "private dash must not reach a non-owner");
    }

    #[tokio::test]
    async fn broadcast_watch_sync_never_broadcasts_to_workspace_only_owner() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;
        let sq = sqlite_pool(&db);
        seed_watch(sq, "watch-1", "ws-1", "owner", "Owner's Watch").await;

        broadcast_watch_sync(&db, &manager, "watch-1", "ws-1", SyncActionType::Update, "owner")
            .await;

        let owner_action = expect_single_sync_action(&mut rx_owner, "watch, owner");
        assert_eq!(owner_action.entity_id, "watch-1");
        assert_no_sync_action(
            &mut rx_other,
            "watches have no sharing model — must never reach a non-owner",
        );
    }

    #[tokio::test]
    async fn broadcast_chat_session_sync_routes_shared_to_workspace_private_to_owner_only() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;
        let sq = sqlite_pool(&db);
        seed_chat_session(sq, "sess-shared", "owner", "ws-1", "Shared", true).await;
        seed_chat_session(sq, "sess-private", "owner", "ws-1", "Private", false).await;

        broadcast_chat_session_sync(
            &db,
            &manager,
            "sess-shared",
            "ws-1",
            SyncActionType::Update,
            "owner",
        )
        .await;
        expect_single_sync_action(&mut rx_owner, "shared session, owner");
        expect_single_sync_action(&mut rx_other, "shared session, non-owner");

        broadcast_chat_session_sync(
            &db,
            &manager,
            "sess-private",
            "ws-1",
            SyncActionType::Update,
            "owner",
        )
        .await;
        expect_single_sync_action(&mut rx_owner, "private session, owner");
        assert_no_sync_action(&mut rx_other, "private session must not reach a non-owner");
    }

    #[tokio::test]
    async fn broadcast_dashboard_visibility_change_going_private_deletes_non_owner_updates_owner()
    {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;
        let sq = sqlite_pool(&db);
        seed_dashboard(sq, "dash-1", "owner", "ws-1", "Dash One").await;

        broadcast_dashboard_visibility_change(&db, &manager, "dash-1", "ws-1", "owner", false)
            .await;

        let other_action = expect_single_sync_action(&mut rx_other, "going private, non-owner");
        assert!(matches!(other_action.action, SyncActionType::Delete));
        assert!(other_action.data.is_none());

        let owner_action = expect_single_sync_action(&mut rx_owner, "going private, owner");
        assert!(matches!(owner_action.action, SyncActionType::Update));
        assert!(
            owner_action.data.is_some(),
            "owner keeps the doc and must receive the refreshed snapshot: {owner_action:?}"
        );
    }

    #[tokio::test]
    async fn broadcast_dashboard_visibility_change_going_public_updates_non_owner_excludes_owner()
    {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;
        let sq = sqlite_pool(&db);
        seed_dashboard(sq, "dash-1", "owner", "ws-1", "Dash One").await;

        broadcast_dashboard_visibility_change(&db, &manager, "dash-1", "ws-1", "owner", true)
            .await;

        let other_action = expect_single_sync_action(&mut rx_other, "going public, non-owner");
        assert!(matches!(other_action.action, SyncActionType::Update));
        assert!(other_action.data.is_some());

        assert_no_sync_action(
            &mut rx_owner,
            "owner already has it — must not receive an extra broadcast",
        );
    }

    #[tokio::test]
    async fn broadcast_chat_session_unshare_deletes_non_owner_updates_owner_with_snapshot() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;
        let sq = sqlite_pool(&db);
        seed_chat_session(sq, "sess-1", "owner", "ws-1", "Shared", true).await;

        broadcast_chat_session_unshare(&db, &manager, "sess-1", "ws-1", "owner").await;

        let other_action = expect_single_sync_action(&mut rx_other, "unshare, non-owner");
        assert!(matches!(other_action.action, SyncActionType::Delete));
        assert!(other_action.data.is_none());

        let owner_action = expect_single_sync_action(&mut rx_owner, "unshare, owner");
        assert!(matches!(owner_action.action, SyncActionType::Update));
        assert!(
            owner_action.data.is_some(),
            "owner keeps the session and must receive the refreshed snapshot: {owner_action:?}"
        );
    }

    #[tokio::test]
    async fn broadcast_entity_delete_reaches_every_workspace_member() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;

        broadcast_entity_delete(&manager, "watch", "watch-1", "ws-1").await;

        for (rx, who) in [(&mut rx_owner, "owner"), (&mut rx_other, "other")] {
            let action = expect_single_sync_action(rx, who);
            assert!(matches!(action.action, SyncActionType::Delete));
            assert!(action.data.is_none());
            assert_eq!(action.entity_id, "watch-1");
        }
    }

    #[tokio::test]
    async fn send_sync_action_respects_exclude_user_id() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;

        let action = SyncAction {
            sync_id: 0,
            entity_type: "dashboard".to_string(),
            entity_id: "dash-1".to_string(),
            workspace_id: "ws-1".to_string(),
            action: SyncActionType::Update,
            data: Some(serde_json::json!({"ok": true})),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        send_sync_action(&manager, "ws-1", &action, Some("owner")).await;

        assert_no_sync_action(&mut rx_owner, "excluded user must not receive the broadcast");
        expect_single_sync_action(&mut rx_other, "non-excluded user must receive the broadcast");
    }

    // ── Skip paths: snapshot-absent and snapshot-failed ──────────────────

    #[tokio::test]
    async fn broadcast_dashboard_sync_snapshot_absent_skips_broadcast() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;

        broadcast_dashboard_sync(
            &db,
            &manager,
            "dash-does-not-exist",
            "ws-1",
            SyncActionType::Update,
            "owner",
        )
        .await;

        assert_no_sync_action(&mut rx_owner, "snapshot-absent must skip the broadcast (owner)");
        assert_no_sync_action(&mut rx_other, "snapshot-absent must skip the broadcast (other)");
    }

    #[tokio::test]
    async fn broadcast_dashboard_sync_fetch_failed_skips_broadcast() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;
        let sq = sqlite_pool(&db);
        seed_dashboard(sq, "dash-1", "owner", "ws-1", "Dash One").await;

        // Force a real sqlx query failure (not a mock): closing the pool
        // makes every subsequent query return `sqlx::Error::PoolClosed`,
        // exercising the actual failure path a saturated/dropped pool would
        // hit in production (same technique as KYO-269 in chat_service.rs).
        sq.close().await;

        broadcast_dashboard_sync(&db, &manager, "dash-1", "ws-1", SyncActionType::Update, "owner")
            .await;

        assert_no_sync_action(&mut rx_owner, "fetch-failed must skip the broadcast (owner)");
        assert_no_sync_action(&mut rx_other, "fetch-failed must skip the broadcast (other)");
    }

    #[tokio::test]
    async fn broadcast_dashboard_visibility_change_snapshot_absent_skips_broadcast() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;

        broadcast_dashboard_visibility_change(
            &db,
            &manager,
            "dash-does-not-exist",
            "ws-1",
            "owner",
            false,
        )
        .await;

        assert_no_sync_action(&mut rx_owner, "snapshot-absent must skip the broadcast (owner)");
        assert_no_sync_action(&mut rx_other, "snapshot-absent must skip the broadcast (other)");
    }

    #[tokio::test]
    async fn broadcast_dashboard_visibility_change_fetch_failed_skips_broadcast() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;
        let sq = sqlite_pool(&db);
        seed_dashboard(sq, "dash-1", "owner", "ws-1", "Dash One").await;

        sq.close().await;

        broadcast_dashboard_visibility_change(&db, &manager, "dash-1", "ws-1", "owner", false)
            .await;

        assert_no_sync_action(&mut rx_owner, "fetch-failed must skip the broadcast (owner)");
        assert_no_sync_action(&mut rx_other, "fetch-failed must skip the broadcast (other)");
    }

    #[tokio::test]
    async fn broadcast_watch_sync_not_found_skips_broadcast() {
        let db = test_pool().await;
        let (manager, mut rx_owner, _rx_other) = setup_workspace_and_connections(&db).await;

        broadcast_watch_sync(
            &db,
            &manager,
            "watch-does-not-exist",
            "ws-1",
            SyncActionType::Update,
            "owner",
        )
        .await;

        assert_no_sync_action(&mut rx_owner, "not-found must skip the broadcast");
    }

    #[tokio::test]
    async fn broadcast_watch_sync_fetch_failed_skips_broadcast() {
        let db = test_pool().await;
        let (manager, mut rx_owner, _rx_other) = setup_workspace_and_connections(&db).await;
        let sq = sqlite_pool(&db);
        seed_watch(sq, "watch-1", "ws-1", "owner", "Owner's Watch").await;

        sq.close().await;

        broadcast_watch_sync(&db, &manager, "watch-1", "ws-1", SyncActionType::Update, "owner")
            .await;

        assert_no_sync_action(&mut rx_owner, "fetch-failed must skip the broadcast");
    }

    #[tokio::test]
    async fn broadcast_chat_session_sync_snapshot_absent_skips_broadcast() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;

        broadcast_chat_session_sync(
            &db,
            &manager,
            "sess-does-not-exist",
            "ws-1",
            SyncActionType::Update,
            "owner",
        )
        .await;

        assert_no_sync_action(&mut rx_owner, "snapshot-absent must skip the broadcast (owner)");
        assert_no_sync_action(&mut rx_other, "snapshot-absent must skip the broadcast (other)");
    }

    #[tokio::test]
    async fn broadcast_chat_session_sync_fetch_failed_skips_broadcast() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;
        let sq = sqlite_pool(&db);
        seed_chat_session(sq, "sess-1", "owner", "ws-1", "Shared", true).await;

        sq.close().await;

        broadcast_chat_session_sync(
            &db,
            &manager,
            "sess-1",
            "ws-1",
            SyncActionType::Update,
            "owner",
        )
        .await;

        assert_no_sync_action(&mut rx_owner, "fetch-failed must skip the broadcast (owner)");
        assert_no_sync_action(&mut rx_other, "fetch-failed must skip the broadcast (other)");
    }

    /// KYO-329 criterion 3: for `broadcast_chat_session_unshare` specifically,
    /// a snapshot-absent owner-update fetch must NOT suppress the Delete to
    /// non-owners — only the owner's restoring Update is skipped. See the
    /// function's own doc comment for why: the Delete is unconditional.
    #[tokio::test]
    async fn broadcast_chat_session_unshare_snapshot_absent_skips_owner_update_sends_delete() {
        let db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) = setup_workspace_and_connections(&db).await;

        broadcast_chat_session_unshare(&db, &manager, "sess-does-not-exist", "ws-1", "owner")
            .await;

        let other_action = expect_single_sync_action(&mut rx_other, "unshare, non-owner");
        assert!(matches!(other_action.action, SyncActionType::Delete));
        assert!(other_action.data.is_none());

        assert_no_sync_action(
            &mut rx_owner,
            "owner update must be skipped when the snapshot is absent",
        );
    }

    /// Same as above but for a real sqlx query failure rather than a
    /// genuine miss. Uses two independent pools: `manager`'s own pool (which
    /// `broadcast_to_workspace` queries for `workspace_users`) stays open,
    /// while the separate `db` argument passed to the function under test is
    /// closed — isolating exactly the owner-update snapshot fetch as the
    /// failing query, the way one flaky connection would in production,
    /// without a mock and without also killing the Delete broadcast's own
    /// (unrelated) query.
    #[tokio::test]
    async fn broadcast_chat_session_unshare_fetch_failed_skips_owner_update_sends_delete() {
        let manager_db = test_pool().await;
        let (manager, mut rx_owner, mut rx_other) =
            setup_workspace_and_connections(&manager_db).await;

        let snapshot_db = test_pool().await;
        let sq_snapshot = sqlite_pool(&snapshot_db);
        seed_user(sq_snapshot, "owner", "owner@test.local").await;
        seed_workspace(sq_snapshot, "ws-1", "owner").await;
        seed_chat_session(sq_snapshot, "sess-1", "owner", "ws-1", "Shared", true).await;
        sq_snapshot.close().await;

        broadcast_chat_session_unshare(&snapshot_db, &manager, "sess-1", "ws-1", "owner").await;

        let other_action = expect_single_sync_action(&mut rx_other, "unshare, non-owner");
        assert!(matches!(other_action.action, SyncActionType::Delete));
        assert!(other_action.data.is_none());

        assert_no_sync_action(
            &mut rx_owner,
            "owner update must be skipped when the snapshot fetch fails",
        );
    }
}
