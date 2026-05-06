// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace invitation model — maps to `workspace_invitations` table.
//!
//! Used for inviting users to join a workspace (team/enterprise tiers).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::enums::{InvitationStatus, WorkspaceRole};

/// A pending or completed workspace invitation.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct WorkspaceInvitation {
    /// Primary key: "inv-{uuid_hex[0..24]}".
    pub invitation_id: String,

    /// Workspace this invitation belongs to.
    pub workspace_id: String,

    /// Email address of the invitee.
    pub email: String,

    /// Role to assign: workspace_admin, workspace_user, workspace_viewer.
    pub role: WorkspaceRole,

    /// User ID of the person who sent the invitation.
    pub invited_by_user_id: String,

    /// Invitation status: pending, accepted, expired, cancelled.
    pub status: InvitationStatus,

    /// When the invitation was created.
    pub created_at: DateTime<Utc>,

    /// When the invitation expires.
    pub expires_at: DateTime<Utc>,

    /// When the invitation was accepted (NULL if not yet accepted).
    pub accepted_at: Option<DateTime<Utc>>,

    /// User ID of the person who accepted (NULL if not yet accepted).
    pub accepted_by_user_id: Option<String>,
}
