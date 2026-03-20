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
