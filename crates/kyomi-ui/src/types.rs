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
