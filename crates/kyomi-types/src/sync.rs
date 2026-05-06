// SPDX-License-Identifier: AGPL-3.0-or-later

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncAction {
    pub sync_id: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub workspace_id: String,
    pub action: SyncActionType,
    pub data: Option<serde_json::Value>,
    pub timestamp: String, // RFC 3339
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncActionType {
    Insert,
    Update,
    Delete,
}

/// Client→server sync request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncRequest {
    SyncBootstrap,
    SyncDelta { last_sync_id: i64 },
}

/// Server→client sync response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncResponse {
    SyncAction(SyncAction),
    SyncComplete { last_sync_id: i64 },
    SyncReset,
}

/// Entity type constants
pub mod entity_types {
    pub const DASHBOARD: &str = "dashboard";
    pub const KNOWLEDGE: &str = "knowledge";
    pub const CHAT_SESSION: &str = "chat_session";
    pub const WATCH: &str = "watch";
    pub const WORKSPACE_SETTINGS: &str = "workspace_settings";

    // Tier 2 — on-demand detail caches (KYO-215).
    // These are written when a detail page resolves a server response and
    // read back on the next visit to skip the loading skeleton.
    // They are invalidated by the sync engine whenever the corresponding
    // Tier 1 list entry is touched (insert/update/delete).

    /// Full dashboard/knowledge-document content payload.
    pub const DASHBOARD_DETAIL: &str = "dashboard_detail";
    /// Ordered messages for a single chat session.
    pub const CHAT_MESSAGES: &str = "chat_messages";
}
