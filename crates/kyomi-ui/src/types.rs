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
    /// Raw DB role token (`workspace_admin` / `workspace_user` / `workspace_viewer`).
    pub role: String,
    /// Human-readable role label ("Admin" / "Viewer" / "Member"), humanized
    /// server-side via `humanize_workspace_role` — the client is `ssr`-gated
    /// out of `kyomi-core` so it cannot humanize the token itself.
    pub role_display: String,
    pub created_at: String,
    pub expires_at: String,
    pub workspace_name: Option<String>,
    pub inviter_name: Option<String>,
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
    #[serde(default)]
    pub default_model: Option<String>,
    pub chart_palette: String,
    /// Optional model used specifically for session title generation.
    ///
    /// When `None`, title generation falls back to the cheapest model for the
    /// configured provider. When `Some`, that model is used verbatim.
    #[serde(default)]
    pub title_model: Option<String>,
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
    /// Raw DB role token (`workspace_admin` / `workspace_user` / `workspace_viewer`).
    pub role: String,
    /// Human-readable role label ("Admin" / "Viewer" / "Member"), humanized
    /// server-side via `humanize_workspace_role` (mirrors
    /// `InvitationData::role_display`, KYO-169) — the client is `ssr`-gated
    /// out of `kyomi-core` so it cannot humanize the token itself. This is a
    /// **pure display string** — render it, never compare it. Display copy
    /// (wording, i18n) is not a stable classification signal; use
    /// `is_admin_role` for that.
    pub role_display: String,
    /// Whether `role` is the workspace-admin role. Computed server-side by
    /// comparing the raw DB token against the admin role constant (KYO-189
    /// P3) — the client never re-derives this from `role` or `role_display`,
    /// so there is no `"workspace_admin"` string literal, and no dependency
    /// on `role_display`'s wording, anywhere on the client.
    pub is_admin_role: bool,
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
    /// Raw DB role token (`workspace_admin` / `workspace_user` / `workspace_viewer`).
    pub role: String,
    /// Human-readable role label ("Admin" / "Viewer" / "Member"), humanized
    /// server-side via `humanize_workspace_role` — see `TeamMember::role_display`.
    /// Pure display string — render it, never compare it.
    pub role_display: String,
    /// Whether `role` is the workspace-admin role — see `TeamMember::is_admin_role`.
    pub is_admin_role: bool,
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


