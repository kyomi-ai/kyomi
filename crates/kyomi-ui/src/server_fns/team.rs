// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for team management.
//!
//! These replace the REST API calls that TeamManagement.jsx makes
//! to `/api/v1/workspaces/members`, `/api/v1/workspaces/invitations`,
//! and `/api/v1/workspaces/ownership/transfers` endpoints.
//!
//! Each function calls the same service-layer code as the existing REST
//! route handlers in `apps/server/src/routes/workspaces.rs`.

use leptos::prelude::*;

use crate::types::{OwnershipTransferData, TeamInvitation, TeamMember};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers (server-only)
// ─────────────────────────────────────────────────────────────────────────────

/// Map frontend role name ("admin"|"user") to DB role string.
///
/// Mirrors `map_role_to_db()` in `apps/server/src/routes/workspaces.rs`.
#[cfg(feature = "ssr")]
fn map_role_to_db(role: &str) -> &'static str {
    let roles = &kyomi_core::constants::get().workspace.roles;
    match role {
        "admin" => &roles.admin,
        _ => &roles.user,
    }
}

/// Generate an invitation ID: `inv-{uuid_hex[0..24]}`.
///
/// Mirrors `generate_invitation_id()` in `apps/server/src/routes/workspaces.rs`.
#[cfg(feature = "ssr")]
fn generate_invitation_id() -> String {
    let hex = sqlx::types::Uuid::new_v4().simple().to_string();
    format!("inv-{}", &hex[..24])
}

/// Reject non-workspace-admin users.
#[cfg(feature = "ssr")]
fn require_workspace_admin(
    auth: &kyomi_auth::middleware::AuthUser,
) -> Result<(), ServerFnError> {
    if !auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
    {
        return Err(ServerFnError::new("Workspace admin access required"));
    }
    Ok(())
}

/// Load the workspace record for the authenticated user.
#[cfg(feature = "ssr")]
async fn get_current_workspace(
    db: &kyomi_core::DbPool,
    ws_id: &str,
) -> Result<kyomi_core::models::Workspace, ServerFnError> {
    kyomi_auth::workspace_service::get_workspace_full(db, ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Workspace not found"))
}

// ─────────────────────────────────────────────────────────────────────────────
// Members
// ─────────────────────────────────────────────────────────────────────────────

/// List all workspace members with user details.
///
/// Mirrors `GET /api/v1/workspaces/members` in workspaces.rs.
#[server(prefix = "/leptos-api")]
pub async fn list_workspace_members() -> Result<Vec<TeamMember>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let workspace = get_current_workspace(ac.db(), &ac.ws_id).await?;
    let members =
        kyomi_auth::workspace_service::get_workspace_members_with_users(ac.db(), &ac.ws_id)
            .await
            .into_sfn()?;

    let result = members
        .iter()
        .map(|m| TeamMember {
            user_id: m.user_id.clone(),
            email: m.email.clone(),
            name: m.name.clone(),
            role: m.role.clone(),
            is_owner: m.user_id == workspace.owner_user_id,
            joined_at: m.wu_created_at.to_rfc3339(),
        })
        .collect();

    Ok(result)
}

/// Update a member's role. Requires admin.
///
/// Mirrors `PATCH /api/v1/workspaces/members/{id}/role` in workspaces.rs.
#[server(prefix = "/leptos-api")]
pub async fn update_member_role(user_id: String, role: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let workspace = get_current_workspace(ac.db(), &ac.ws_id).await?;

    // Cannot change the owner's role
    if user_id == workspace.owner_user_id {
        return Err(ServerFnError::new("Cannot change workspace owner's role"));
    }

    let db_role = map_role_to_db(&role);

    // Self-demotion guard
    if user_id == ac.auth.user_id && db_role == "workspace_user" {
        let admin_count =
            kyomi_auth::workspace_service::count_admins(ac.db(), &ac.ws_id)
                .await
                .into_sfn()?;
        if admin_count < 2 {
            return Err(ServerFnError::new(
                "Cannot demote yourself: you are the only admin",
            ));
        }
    }

    // Verify member exists
    let target = kyomi_auth::workspace_service::get_workspace_user(ac.db(), &ac.ws_id, &user_id)
        .await
        .into_sfn()?;
    if target.is_none() {
        return Err(ServerFnError::new("Member not found in workspace"));
    }

    kyomi_auth::workspace_service::update_member_role(ac.db(), &ac.ws_id, &user_id, db_role)
        .await
        .into_sfn()?;

    Ok(())
}

/// Remove a member from the workspace. Requires admin.
///
/// Mirrors `DELETE /api/v1/workspaces/members/{id}` in workspaces.rs.
#[server(prefix = "/leptos-api")]
pub async fn remove_member(user_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let workspace = get_current_workspace(ac.db(), &ac.ws_id).await?;

    kyomi_auth::workspace_service::remove_workspace_member(
        ac.db(),
        &ac.ws_id,
        &workspace.owner_user_id,
        &ac.auth.user_id,
        &user_id,
    )
    .await
    .into_sfn()?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Invitations
// ─────────────────────────────────────────────────────────────────────────────

/// List pending invitations for the workspace. Requires admin.
///
/// Mirrors `GET /api/v1/workspaces/invitations` in workspaces.rs.
#[server(prefix = "/leptos-api")]
pub async fn list_workspace_invitations() -> Result<Vec<TeamInvitation>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let invitations =
        kyomi_auth::workspace_service::get_pending_invitations_for_workspace(ac.db(), &ac.ws_id)
            .await
            .into_sfn()?;

    let result = invitations
        .iter()
        .map(|inv| TeamInvitation {
            invitation_id: inv.invitation_id.clone(),
            email: inv.email.clone(),
            role: inv.role.to_string(),
            status: inv.status.to_string(),
            created_at: inv.created_at.to_rfc3339(),
            expires_at: inv.expires_at.to_rfc3339(),
        })
        .collect();

    Ok(result)
}

/// Create a new invitation. Requires admin.
///
/// Mirrors `POST /api/v1/workspaces/invitations` in workspaces.rs.
#[server(prefix = "/leptos-api")]
pub async fn invite_member(email: String, role: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let email = email.trim().to_lowercase();

    if email.is_empty() || !email.contains('@') {
        return Err(ServerFnError::new("Invalid email address"));
    }

    // Resolve the per-plan user limit (None = self-hosted, no limit enforced).
    let user_limit = if ac.ctx.config.self_hosted {
        None
    } else {
        let workspace = get_current_workspace(ac.db(), &ac.ws_id).await?;
        Some(workspace.user_limit.unwrap_or(1) as i64)
    };

    let invitation_id = generate_invitation_id();
    let expires_at = chrono::Utc::now() + chrono::Duration::days(7);
    let db_role = map_role_to_db(&role);

    kyomi_auth::workspace_service::invite_workspace_member(
        kyomi_auth::workspace_service::InviteWorkspaceMemberParams {
            pool: ac.db(),
            workspace_id: &ac.ws_id,
            email: &email,
            db_role,
            invited_by: &ac.auth.user_id,
            invitation_id: &invitation_id,
            expires_at,
            user_limit,
        },
    )
    .await
    .into_sfn()?;

    // Send invitation email
    let invite_email = email.clone();
    let workspace_name = ac.auth.workspace.workspace_name.clone().unwrap_or_default();
    let inviter_name = ac.auth.name.clone().unwrap_or_else(|| ac.auth.email.clone());
    let display_role = role.clone();
    tokio::spawn(async move {
        let email_svc = kyomi_auth::email_service::EmailService::from_env();
        let sent = email_svc
            .send_workspace_invitation(&invite_email, &workspace_name, &inviter_name, &display_role)
            .await;
        if sent {
            tracing::info!("Invitation email sent to {invite_email}");
        } else {
            tracing::warn!("Failed to send invitation email to {invite_email}");
        }
    });

    Ok(())
}

/// Cancel a pending invitation. Requires admin.
///
/// Mirrors `DELETE /api/v1/workspaces/invitations/{id}` in workspaces.rs.
#[server(prefix = "/leptos-api")]
pub async fn cancel_invitation(invitation_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;

    let invitation =
        kyomi_auth::workspace_service::get_invitation_in_workspace(ac.db(), &invitation_id, &ac.ws_id)
            .await
            .into_sfn()?
            .ok_or_else(|| ServerFnError::new("Invitation not found"))?;

    if invitation.status != kyomi_core::enums::InvitationStatus::Pending {
        return Err(ServerFnError::new("Can only cancel pending invitations"));
    }

    kyomi_auth::workspace_service::update_invitation_status(ac.db(), &invitation_id, "cancelled")
        .await
        .into_sfn()?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Ownership transfers
// ─────────────────────────────────────────────────────────────────────────────

/// List ownership transfers for the current user.
///
/// Mirrors `GET /api/v1/workspaces/ownership/transfers` in workspaces.rs.
#[server(prefix = "/leptos-api")]
pub async fn list_ownership_transfers() -> Result<Vec<OwnershipTransferData>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let transfers =
        kyomi_auth::workspace_service::list_ownership_transfers_for_user(ac.db(), &ac.ws_id, &ac.auth.user_id)
            .await
            .into_sfn()?;

    let result = transfers
        .into_iter()
        .map(|t| OwnershipTransferData {
            transfer_id: t.transfer_id,
            from_user_id: t.from_user_id,
            from_user_email: t.from_user_email,
            to_user_id: t.to_user_id,
            to_user_email: t.to_user_email,
            status: t.status,
            created_at: t.created_at.to_rfc3339(),
            expires_at: t.expires_at.to_rfc3339(),
            is_initiator: t.is_initiator,
            is_recipient: t.is_recipient,
        })
        .collect();

    Ok(result)
}

/// Cancel an ownership transfer. Only the initiator can cancel.
///
/// Mirrors `DELETE /api/v1/workspaces/ownership/transfer/{id}` in workspaces.rs.
#[server(prefix = "/leptos-api")]
pub async fn cancel_ownership_transfer(transfer_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let transfer =
        kyomi_auth::workspace_service::get_ownership_transfer(&ctx.db, &transfer_id)
            .await
            .into_sfn()?
            .ok_or_else(|| ServerFnError::new("Transfer not found"))?;

    if transfer.status != kyomi_core::enums::TransferStatus::Pending {
        return Err(ServerFnError::new("Transfer is no longer pending"));
    }

    if transfer.from_user_id != auth.user_id {
        return Err(ServerFnError::new(
            "Only the transfer initiator can cancel",
        ));
    }

    kyomi_auth::workspace_service::update_transfer_status(&ctx.db, &transfer_id, "cancelled")
        .await
        .into_sfn()?;

    Ok(())
}

/// Initiate an ownership transfer to another workspace member.
///
/// Mirrors `POST /api/v1/workspaces/ownership/transfer` in workspaces.rs.
/// Only the workspace owner can call this.
#[server(prefix = "/leptos-api")]
pub async fn initiate_ownership_transfer(to_user_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Must be owner
    if !ac.auth.workspace.is_owner {
        return Err(ServerFnError::new(
            "Only the workspace owner can transfer ownership",
        ));
    }

    if to_user_id == ac.auth.user_id {
        return Err(ServerFnError::new("You are already the owner"));
    }

    // Check no existing pending transfer
    let existing =
        kyomi_auth::workspace_service::get_pending_transfer_for_workspace(ac.db(), &ac.ws_id)
            .await
            .into_sfn()?;
    if existing.is_some() {
        return Err(ServerFnError::new(
            "There is already a pending ownership transfer for this workspace",
        ));
    }

    let transfer_id = format!(
        "xfer-{}",
        &sqlx::types::Uuid::new_v4().simple().to_string()[..24]
    );
    let expires_at = chrono::Utc::now() + chrono::Duration::days(7);

    kyomi_auth::workspace_service::create_ownership_transfer(
        ac.db(),
        &transfer_id,
        &ac.ws_id,
        &ac.auth.user_id,
        &to_user_id,
        expires_at,
    )
    .await
    .into_sfn()?;

    Ok(())
}

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, AuthenticatedContext, IntoServerFnError};
