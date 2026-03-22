// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared types that cross the server/client boundary.
//!
//! All types here must be `Serialize + Deserialize + Clone` since they are
//! sent over the wire between server functions and WASM client code.

use serde::{Deserialize, Serialize};

/// User profile data returned by the get_profile server function.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileData {
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub theme: String,
    pub landing_page: String,
    pub default_dashboard_id: Option<String>,
    pub query_history_retention_days: i32,
    pub chart_palette: String,
    pub is_personal_mode: bool,
    pub is_self_hosted: bool,
}

/// Minimal dashboard info for the default dashboard selector.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DashboardSummary {
    pub dashboard_id: String,
    pub title: String,
}

/// Pending workspace invitation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvitationData {
    pub invitation_id: String,
    pub workspace_id: String,
    pub email: String,
    pub role: String,
    pub created_at: String,
    pub expires_at: String,
}

/// Slack connection status for the current user.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlackStatus {
    pub workspace_connected: bool,
    pub user_connected: bool,
    pub slack_username: Option<String>,
    pub slack_team_name: Option<String>,
}

/// A Slack channel available for watch notifications.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlackChannel {
    pub channel_id: String,
    pub channel_name: String,
}

/// Default watch channel setting.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WatchChannel {
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
}

/// Workspace settings data returned by the get_workspace_settings server function.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceSettingsData {
    pub workspace_name: String,
    pub default_model: String,
    pub chart_palette: String,
}

/// Result of a knowledge graph rebuild.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphRebuildResult {
    pub status: String,
    pub learnings_with_references: i64,
}

/// Workspace-level Slack integration status.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceSlackStatus {
    pub installed: bool,
    pub team_id: Option<String>,
    pub team_name: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Team management types
// ─────────────────────────────────────────────────────────────────────────────

/// A workspace member with user details.
///
/// Mirrors the JSON shape returned by `GET /api/v1/workspaces/members`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TeamMember {
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub role: String,
    pub is_owner: bool,
    pub joined_at: String,
}

/// A pending workspace invitation (admin view).
///
/// Mirrors the JSON shape returned by `GET /api/v1/workspaces/invitations`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeamInvitation {
    pub invitation_id: String,
    pub email: String,
    pub role: String,
    pub status: String,
    pub created_at: String,
    pub expires_at: String,
}

/// A pending ownership transfer.
///
/// Mirrors the JSON shape returned by `GET /api/v1/workspaces/ownership/transfers`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OwnershipTransferData {
    pub transfer_id: String,
    pub from_user_id: String,
    pub from_user_email: String,
    pub to_user_id: String,
    pub to_user_email: String,
    pub status: String,
    pub created_at: String,
    pub expires_at: String,
    pub is_initiator: bool,
    pub is_recipient: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Watch types
// ─────────────────────────────────────────────────────────────────────────────

/// A watch in a list result.
///
/// Maps from `kyomi_core::models::Watch` with timestamps converted to RFC 3339
/// strings and alert channel info resolved from the platform layer.
///
/// Mirrors `WatchResponse` in `apps/server/src/routes/watches.rs`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WatchListItem {
    pub watch_id: String,
    pub name: String,
    pub prompt: String,
    pub schedule: String,
    pub mode: String,
    pub enabled: bool,
    pub last_run_at: Option<String>,
    pub last_run_status: Option<String>,
    pub next_run_at: Option<String>,
    pub created_at: String,
    pub created_by: String,
    pub alert_emails: Option<String>,
    pub alert_emails_enabled: bool,
    pub queries: Option<serde_json::Value>,
    pub slack_channel_id: Option<String>,
    pub slack_channel_name: Option<String>,
}

/// A watch execution record.
///
/// Maps from `kyomi_core::models::WatchExecution` with timestamps converted to
/// RFC 3339 strings and enum fields converted to strings.
///
/// Mirrors `ExecutionResponse` in `apps/server/src/routes/watches.rs`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WatchExecutionItem {
    pub id: i32,
    pub watch_id: Option<String>,
    pub watch_name: Option<String>,
    pub mode: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub status: String,
    pub agent_response: Option<String>,
    pub error_message: Option<String>,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub alert_triggered: bool,
    pub notification_id: Option<String>,
    pub execution_trace: Option<serde_json::Value>,
    pub read_at: Option<String>,
    pub deleted_at: Option<String>,
    pub deleted_by: Option<String>,
}

/// An alert item (a watch execution that triggered an alert).
///
/// Mirrors the alert entries in `AlertHistoryResponse` in
/// `apps/server/src/routes/watches.rs`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlertItem {
    pub id: i32,
    pub watch_id: Option<String>,
    pub watch_name: Option<String>,
    pub mode: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub status: String,
    pub agent_response: Option<String>,
    pub error_message: Option<String>,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub alert_triggered: bool,
    pub notification_id: Option<String>,
    pub execution_trace: Option<serde_json::Value>,
    pub read_at: Option<String>,
    pub deleted_at: Option<String>,
    pub deleted_by: Option<String>,
}

/// Paginated alerts response with total count.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlertsPage {
    pub alerts: Vec<AlertItem>,
    pub total: i64,
}

/// Result of validating and describing a cron expression.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CronDescription {
    pub valid: bool,
    pub description: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Knowledge types
// ─────────────────────────────────────────────────────────────────────────────

/// A knowledge file tree entry (no content — used for tree rendering).
///
/// Mirrors `KnowledgeFileTreeEntry` in `kyomi-knowledge/src/knowledge_files.rs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeTreeEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub is_folder: bool,
    pub sort_order: i32,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

/// Full knowledge file detail with content (used by editor).
///
/// Mirrors `FileResponse` in `apps/server/src/routes/knowledge_files.rs`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeFileDetail {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub is_folder: bool,
    pub content: Option<String>,
    pub content_hash: Option<String>,
    pub sort_order: i32,
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A knowledge file search result.
///
/// Mirrors `KnowledgeFileSearchResult` in `kyomi-knowledge/src/knowledge_files.rs`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeSearchResult {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub is_folder: bool,
    pub content_preview: Option<String>,
}

