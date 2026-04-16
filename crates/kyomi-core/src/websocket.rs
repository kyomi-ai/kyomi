// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket message types and shared types for the unified WS system.
//!
//! `MessageType` and `WebSocketMessage` live in `kyomi-core` so they can be
//! used by both `kyomi-auth` (the manager) and `kyomi-api` (the endpoints).

use chrono::Utc;
use serde::{Deserialize, Serialize};

/// WebSocket message types. 23 from `shared/constants.toml` plus workspace event types.
///
/// Serializes to/from snake_case strings (e.g., `ChatStream` ↔ `"chat_stream"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    ChatStream,
    ChatComplete,
    TitleUpdate,
    SessionCreated,
    AgentThinking,
    TokenUsageUpdate,
    OauthReconnectRequired,
    OauthCancel,
    OwnershipTransferOffered,
    WorkspaceInvitation,
    WorkspaceRemoved,
    DashboardUpdate,
    ChartUpdate,
    WatchAlert,
    WatchStateUpdate,
    WatchUpdate,
    CredentialStatusChanged,
    CatalogStatusUpdate,
    AiUsageUpdate,
    SharedConversationActivity,
    SharedChatMessage,
    RequestCancelled,
    Error,
    Heartbeat,
    // Workspace event types (not in constants.toml — Rust-only additions)
    MemberRoleChanged,
    MemberJoined,
    OwnershipTransferCompleted,
    OwnershipTransferDeclined,
    // Query streaming types (Rust-only — used for Connect streaming responses)
    QueryStreamHeader,
    QueryStreamChunk,
    QueryStreamComplete,
    QueryStreamError,
}

/// A WebSocket message sent to clients.
///
/// Serializes to JSON matching the Python backend's `WebSocketMessage` format exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSocketMessage {
    #[serde(rename = "type")]
    pub message_type: MessageType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl WebSocketMessage {
    /// Create a new message with auto-set UTC timestamp.
    pub fn new(message_type: MessageType) -> Self {
        Self {
            message_type,
            session_id: None,
            message_id: None,
            timestamp: Utc::now().to_rfc3339(),
            data: None,
        }
    }

    /// Builder: set session_id.
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Builder: set message_id.
    pub fn with_message_id(mut self, message_id: impl Into<String>) -> Self {
        self.message_id = Some(message_id.into());
        self
    }

    /// Builder: set data payload.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_type_serializes_to_snake_case() {
        let json = serde_json::to_string(&MessageType::ChatStream).unwrap();
        assert_eq!(json, "\"chat_stream\"");

        let json = serde_json::to_string(&MessageType::TokenUsageUpdate).unwrap();
        assert_eq!(json, "\"token_usage_update\"");

        let json = serde_json::to_string(&MessageType::OauthReconnectRequired).unwrap();
        assert_eq!(json, "\"oauth_reconnect_required\"");
    }

    #[test]
    fn message_type_deserializes_from_snake_case() {
        let mt: MessageType = serde_json::from_str("\"chat_stream\"").unwrap();
        assert_eq!(mt, MessageType::ChatStream);

        let mt: MessageType = serde_json::from_str("\"heartbeat\"").unwrap();
        assert_eq!(mt, MessageType::Heartbeat);
    }

    #[test]
    fn all_constants_toml_types_have_variants() {
        // Verify all 23 types from constants.toml can be deserialized.
        let constants_types = [
            "chat_stream", "chat_complete", "title_update", "session_created",
            "agent_thinking", "token_usage_update", "oauth_reconnect_required",
            "oauth_cancel", "ownership_transfer_offered", "workspace_invitation",
            "workspace_removed", "dashboard_update", "chart_update", "watch_alert",
            "watch_state_update", "credential_status_changed", "catalog_status_update",
            "ai_usage_update", "shared_conversation_activity", "shared_chat_message",
            "request_cancelled", "error", "heartbeat",
        ];
        for t in constants_types {
            let json = format!("\"{t}\"");
            let result: Result<MessageType, _> = serde_json::from_str(&json);
            assert!(result.is_ok(), "failed to deserialize message type: {t}");
        }
        assert_eq!(constants_types.len(), 23);

        // Verify Rust-only workspace event types
        let extra_types = [
            "member_role_changed", "member_joined",
            "ownership_transfer_completed", "ownership_transfer_declined",
        ];
        for t in extra_types {
            let json = format!("\"{t}\"");
            let result: Result<MessageType, _> = serde_json::from_str(&json);
            assert!(result.is_ok(), "failed to deserialize message type: {t}");
        }

        // Verify Rust-only query streaming types
        let stream_types = [
            "query_stream_header", "query_stream_chunk",
            "query_stream_complete", "query_stream_error",
        ];
        for t in stream_types {
            let json = format!("\"{t}\"");
            let result: Result<MessageType, _> = serde_json::from_str(&json);
            assert!(result.is_ok(), "failed to deserialize message type: {t}");
        }
    }

    #[test]
    fn websocket_message_builder() {
        let msg = WebSocketMessage::new(MessageType::ChatStream)
            .with_session("session-123")
            .with_message_id("msg-456")
            .with_data(serde_json::json!({"content": "hello"}));

        assert_eq!(msg.message_type, MessageType::ChatStream);
        assert_eq!(msg.session_id.as_deref(), Some("session-123"));
        assert_eq!(msg.message_id.as_deref(), Some("msg-456"));
        assert!(msg.data.is_some());
    }

    #[test]
    fn websocket_message_json_format() {
        let msg = WebSocketMessage::new(MessageType::Heartbeat);
        let json: serde_json::Value = serde_json::to_value(&msg).unwrap();

        assert_eq!(json["type"], "heartbeat");
        assert!(json["timestamp"].is_string());
        // session_id and message_id should be absent (skip_serializing_if = None)
        assert!(json.get("session_id").is_none());
        assert!(json.get("message_id").is_none());
    }

    #[test]
    fn websocket_message_with_data_serializes_correctly() {
        let msg = WebSocketMessage::new(MessageType::TitleUpdate)
            .with_session("s-1")
            .with_data(serde_json::json!({"title": "New Title"}));

        let json: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "title_update");
        assert_eq!(json["session_id"], "s-1");
        assert_eq!(json["data"]["title"], "New Title");
    }
}
