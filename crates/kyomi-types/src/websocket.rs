// SPDX-License-Identifier: AGPL-3.0-or-later

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
    DatasourceUpdate,
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
    // Live sync broadcast types (Rust-only — used for real-time cache invalidation)
    SyncAction,
    SyncComplete,
    SyncReset,
}

impl std::fmt::Display for MessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(s)) => f.write_str(&s),
            _ => f.write_str("unknown"),
        }
    }
}

/// A WebSocket message sent to clients.
///
/// Serializes to JSON matching the Python backend's `WebSocketMessage` format exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSocketMessage {
    #[serde(rename = "type")]
    pub message_type: MessageType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl WebSocketMessage {
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_message_id(mut self, message_id: impl Into<String>) -> Self {
        self.message_id = Some(message_id.into());
        self
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl WebSocketMessage {
    pub fn new(message_type: MessageType) -> Self {
        Self {
            message_type,
            session_id: None,
            message_id: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
            data: None,
        }
    }
}
