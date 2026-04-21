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
        .map_err(|e| ServerFnError::new(e.to_string()))?
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
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let workspace = get_current_workspace(&ctx.db, ws_id).await?;
    let members =
        kyomi_auth::workspace_service::get_workspace_members_with_users(&ctx.db, ws_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

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
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_admin(&auth)?;

    let ws_id = workspace_id(&auth)?;
    let workspace = get_current_workspace(&ctx.db, ws_id).await?;

    // Cannot change the owner's role
    if user_id == workspace.owner_user_id {
        return Err(ServerFnError::new("Cannot change workspace owner's role"));
    }

    let db_role = map_role_to_db(&role);

    // Self-demotion guard
    if user_id == auth.user_id && db_role == "workspace_user" {
        let admin_count =
            kyomi_auth::workspace_service::count_admins(&ctx.db, ws_id)
                .await
                .map_err(|e| ServerFnError::new(e.to_string()))?;
        if admin_count < 2 {
            return Err(ServerFnError::new(
                "Cannot demote yourself: you are the only admin",
            ));
        }
    }

    // Verify member exists
    let target = kyomi_auth::workspace_service::get_workspace_user(&ctx.db, ws_id, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if target.is_none() {
        return Err(ServerFnError::new("Member not found in workspace"));
    }

    kyomi_auth::workspace_service::update_member_role(&ctx.db, ws_id, &user_id, db_role)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

/// Remove a member from the workspace. Requires admin.
///
/// Mirrors `DELETE /api/v1/workspaces/members/{id}` in workspaces.rs.
#[server(prefix = "/leptos-api")]
// lint-allow: server-fn-callouts=pre-existing orchestration drift tracked in KYO-124
pub async fn remove_member(user_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_admin(&auth)?;

    let ws_id = workspace_id(&auth)?;
    let workspace = get_current_workspace(&ctx.db, ws_id).await?;

    // Cannot remove workspace owner
    if user_id == workspace.owner_user_id {
        return Err(ServerFnError::new(
            "Cannot remove workspace owner. Transfer ownership first.",
        ));
    }

    // Self-removal guard
    if user_id == auth.user_id {
        let admin_count =
            kyomi_auth::workspace_service::count_admins(&ctx.db, ws_id)
                .await
                .map_err(|e| ServerFnError::new(e.to_string()))?;
        if admin_count < 2 {
            return Err(ServerFnError::new(
                "Cannot remove yourself: you are the only admin",
            ));
        }
    }

    // Verify member exists
    let target = kyomi_auth::workspace_service::get_workspace_user(&ctx.db, ws_id, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if target.is_none() {
        return Err(ServerFnError::new("Member not found in workspace"));
    }

    // Auto-transfer shared conversations to workspace owner
    let _ = kyomi_core::db_execute!(
        &ctx.db,
        "UPDATE chat_sessions SET user_id = $1 \
         WHERE user_id = $2 AND workspace_id = $3 AND shared = true",
        &workspace.owner_user_id,
        &user_id,
        ws_id
    );

    kyomi_auth::workspace_service::remove_member(&ctx.db, ws_id, &user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

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
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_admin(&auth)?;

    let ws_id = workspace_id(&auth)?;
    let invitations =
        kyomi_auth::workspace_service::get_pending_invitations_for_workspace(&ctx.db, ws_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

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
// lint-allow: server-fn-callouts=pre-existing orchestration drift tracked in KYO-124
pub async fn invite_member(email: String, role: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_admin(&auth)?;

    let ws_id = workspace_id(&auth)?;
    let email = email.trim().to_lowercase();

    if email.is_empty() || !email.contains('@') {
        return Err(ServerFnError::new("Invalid email address"));
    }

    // Check if already a member
    let is_member =
        kyomi_auth::workspace_service::check_existing_member_by_email(&ctx.db, ws_id, &email)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    if is_member {
        return Err(ServerFnError::new(
            "User is already a member of this workspace",
        ));
    }

    // Check for existing pending invitation
    let has_pending =
        kyomi_auth::workspace_service::check_pending_invitation(&ctx.db, ws_id, &email)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    if has_pending {
        return Err(ServerFnError::new(
            "Invitation already pending for this email",
        ));
    }

    // User limit check (skip for self-hosted)
    if !ctx.config.self_hosted {
        let workspace = get_current_workspace(&ctx.db, ws_id).await?;
        let current_users =
            kyomi_auth::workspace_service::count_workspace_users(&ctx.db, ws_id)
                .await
                .map_err(|e| ServerFnError::new(e.to_string()))?;
        let pending_invitations =
            kyomi_auth::workspace_service::count_pending_invitations(&ctx.db, ws_id)
                .await
                .map_err(|e| ServerFnError::new(e.to_string()))?;
        let user_limit = workspace.user_limit.unwrap_or(1) as i64;

        if current_users + pending_invitations >= user_limit {
            return Err(ServerFnError::new(
                "Workspace user limit reached. Upgrade your plan to add more users.",
            ));
        }
    }

    let invitation_id = generate_invitation_id();
    let expires_at = chrono::Utc::now() + chrono::Duration::days(7);
    let db_role = map_role_to_db(&role);

    kyomi_auth::workspace_service::create_invitation(
        &ctx.db,
        &invitation_id,
        ws_id,
        &email,
        db_role,
        &auth.user_id,
        expires_at,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

/// Cancel a pending invitation. Requires admin.
///
/// Mirrors `DELETE /api/v1/workspaces/invitations/{id}` in workspaces.rs.
#[server(prefix = "/leptos-api")]
pub async fn cancel_invitation(invitation_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    require_workspace_admin(&auth)?;

    let ws_id = workspace_id(&auth)?;
    let invitation =
        kyomi_auth::workspace_service::get_invitation_in_workspace(&ctx.db, &invitation_id, ws_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .ok_or_else(|| ServerFnError::new("Invitation not found"))?;

    if invitation.status != kyomi_core::enums::InvitationStatus::Pending {
        return Err(ServerFnError::new("Can only cancel pending invitations"));
    }

    kyomi_auth::workspace_service::update_invitation_status(&ctx.db, &invitation_id, "cancelled")
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Ownership transfers
// ─────────────────────────────────────────────────────────────────────────────

/// List ownership transfers for the current user.
///
/// Mirrors `GET /api/v1/workspaces/ownership/transfers` in workspaces.rs.
#[server(prefix = "/leptos-api")]
// lint-allow: server-fn-callouts=pre-existing orchestration drift tracked in KYO-124
pub async fn list_ownership_transfers() -> Result<Vec<OwnershipTransferData>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Get transfers where user is recipient
    let received =
        kyomi_auth::workspace_service::get_pending_transfers_for_user(&ctx.db, &auth.user_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Also get initiated transfers (as workspace owner)
    let initiated =
        kyomi_auth::workspace_service::get_pending_transfer_for_workspace(&ctx.db, ws_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Merge and deduplicate
    let mut all_transfers = received;
    if let Some(t) = initiated
        && !all_transfers.iter().any(|existing| existing.transfer_id == t.transfer_id)
    {
        all_transfers.push(t);
    }

    let mut result = Vec::new();
    for transfer in &all_transfers {
        let from_user = kyomi_auth::user_service::get_user_by_id(&ctx.db, &transfer.from_user_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        let from_email = from_user.map(|u| u.email).unwrap_or_default();

        let to_user = kyomi_auth::user_service::get_user_by_id(&ctx.db, &transfer.to_user_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        let to_email = to_user.map(|u| u.email).unwrap_or_default();

        result.push(OwnershipTransferData {
            transfer_id: transfer.transfer_id.clone(),
            from_user_id: transfer.from_user_id.clone(),
            from_user_email: from_email,
            to_user_id: transfer.to_user_id.clone(),
            to_user_email: to_email,
            status: transfer.status.to_string(),
            created_at: transfer.created_at.to_rfc3339(),
            expires_at: transfer.expires_at.to_rfc3339(),
            is_initiator: transfer.from_user_id == auth.user_id,
            is_recipient: transfer.to_user_id == auth.user_id,
        });
    }

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
            .map_err(|e| ServerFnError::new(e.to_string()))?
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
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

/// Initiate an ownership transfer to another workspace member.
///
/// Mirrors `POST /api/v1/workspaces/ownership/transfer` in workspaces.rs.
/// Only the workspace owner can call this.
#[server(prefix = "/leptos-api")]
pub async fn initiate_ownership_transfer(to_user_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Must be owner
    if !auth.workspace.is_owner {
        return Err(ServerFnError::new(
            "Only the workspace owner can transfer ownership",
        ));
    }

    if to_user_id == auth.user_id {
        return Err(ServerFnError::new("You are already the owner"));
    }

    // Check no existing pending transfer
    let existing =
        kyomi_auth::workspace_service::get_pending_transfer_for_workspace(&ctx.db, ws_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
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
        &ctx.db,
        &transfer_id,
        ws_id,
        &auth.user_id,
        &to_user_id,
        expires_at,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(())
}

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, workspace_id};
