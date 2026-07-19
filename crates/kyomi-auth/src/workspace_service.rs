// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace service — query functions for workspace management.
//!
//! Used by workspace endpoints (4C/4D) and user endpoints that need
//! workspace context. Complements `user_service.rs` which already has
//! `get_workspace` and `get_workspace_user` for single-record lookups.

use chrono::{DateTime, Utc};
use kyomi_core::enums::TransferStatus;
use kyomi_core::models::{
    OwnershipTransfer, Workspace, WorkspaceInvitation, WorkspaceUser,
};
use kyomi_core::sql_compat;
use kyomi_core::DbPool;
use serde::{Deserialize, Serialize};

use crate::sync_log_service;
use kyomi_types::sync::{SyncActionType, entity_types};

/// Get all active workspace user memberships for a workspace.
///
/// Returns membership records only (not joined user data).
/// The route handler can do a second query for user details if needed,
/// matching the Python pattern.
pub async fn get_workspace_users(
    pool: &DbPool,
    workspace_id: &str,
) -> kyomi_core::Result<Vec<WorkspaceUser>> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT * FROM workspace_users \
         WHERE workspace_id = $1 AND active = {bt}"
    );
    let users = kyomi_core::db_fetch_all!(pool, WorkspaceUser, &sql, workspace_id)?;
    Ok(users)
}

/// Count active members in a workspace.
pub async fn count_workspace_users(
    pool: &DbPool,
    workspace_id: &str,
) -> kyomi_core::Result<i64> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT COUNT(*) as count FROM workspace_users \
         WHERE workspace_id = $1 AND active = {bt}"
    );
    let count: i64 = kyomi_core::db_fetch_scalar!(pool, i64, &sql, workspace_id)?;
    Ok(count)
}

/// Get all workspaces a user belongs to (active memberships).
///
/// Returns pairs of (Workspace, WorkspaceUser) for each membership.
pub async fn get_user_workspaces(
    pool: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<Vec<(Workspace, WorkspaceUser)>> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);

    // Get all active workspace memberships
    let memberships_sql = format!(
        "SELECT * FROM workspace_users \
         WHERE user_id = $1 AND active = {bt} \
         ORDER BY created_at ASC"
    );
    let memberships = kyomi_core::db_fetch_all!(pool, WorkspaceUser, &memberships_sql, user_id)?;

    let mut results = Vec::with_capacity(memberships.len());
    for wu in memberships {
        let ws = kyomi_core::db_fetch_optional!(
            pool, Workspace,
            "SELECT * FROM workspaces WHERE workspace_id = $1",
            &wu.workspace_id
        )?;

        if let Some(ws) = ws {
            results.push((ws, wu));
        }
    }

    Ok(results)
}

#[derive(Debug, sqlx::FromRow)]
struct WorkspaceSnapshotRow {
    workspace_id: String,
    name: Option<String>,
    settings: Option<String>,
    business_knowledge: Option<String>,
    updated_at: String,
}

async fn fetch_workspace_settings_snapshot(
    pool: &DbPool,
    workspace_id: &str,
) -> Option<serde_json::Value> {
    get_workspace_settings_for_sync(pool, workspace_id).await
}

/// Return a workspace settings snapshot (name, settings, business_knowledge,
/// updated_at) as a JSON value for the sync bootstrap protocol.
///
/// Returns `None` if the workspace does not exist or the query fails.
pub async fn get_workspace_settings_for_sync(
    pool: &DbPool,
    workspace_id: &str,
) -> Option<serde_json::Value> {
    let row = kyomi_core::db_fetch_optional!(
        pool,
        WorkspaceSnapshotRow,
        r#"SELECT workspace_id,
                  name,
                  CAST(settings AS TEXT) AS settings,
                  business_knowledge,
                  CAST(updated_at AS TEXT) AS updated_at
           FROM workspaces WHERE workspace_id = $1"#,
        workspace_id
    )
    .ok()?;

    let row = row?;
    let settings_json: Option<serde_json::Value> = row
        .settings
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    Some(serde_json::json!({
        "workspace_id": row.workspace_id,
        "name": row.name,
        "settings": settings_json,
        "business_knowledge": row.business_knowledge,
        "updated_at": row.updated_at,
    }))
}

/// Update workspace display name.
pub async fn update_workspace_name(
    pool: &DbPool,
    workspace_id: &str,
    name: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now_expr = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE workspaces SET name = $1, updated_at = {now_expr} WHERE workspace_id = $2"
    );
    let result = kyomi_core::db_execute!(pool, &sql, name, workspace_id)?;

    // Sync log — best-effort: log a warning and continue on failure.
    if result.rows_affected() > 0 {
        let snapshot = fetch_workspace_settings_snapshot(pool, workspace_id).await;
        if let Err(e) = sync_log_service::write_sync_entry(
            pool,
            entity_types::WORKSPACE_SETTINGS,
            workspace_id,
            workspace_id,
            SyncActionType::Update,
            snapshot,
        )
        .await
        {
            tracing::warn!(error = %e, workspace_id = %workspace_id, "Failed to write sync log entry");
        }
    }

    Ok(result.rows_affected() > 0)
}

/// Update workspace settings JSON (full replace).
///
/// `workspaces.settings` is a Postgres `json` column. Binding `$1` as text
/// and letting Postgres coerce it does NOT work — Postgres refuses the
/// implicit text-to-json cast (`column "settings" is of type json but
/// expression is of type text`). We keep the bind as text (sqlx serializes
/// `String` to TEXT on both backends) and perform the cast in SQL on
/// Postgres. SQLite stores JSON in TEXT columns, so no cast is needed.
pub async fn update_workspace_settings(
    pool: &DbPool,
    workspace_id: &str,
    settings: &serde_json::Value,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let settings_str = serde_json::to_string(settings)
        .map_err(|e| kyomi_core::Error::Internal(format!("JSON serialization failed: {e}")))?;
    let sql = if is_pg {
        format!(
            "UPDATE workspaces SET settings = $1::json, updated_at = {now} WHERE workspace_id = $2"
        )
    } else {
        format!(
            "UPDATE workspaces SET settings = $1, updated_at = {now} WHERE workspace_id = $2"
        )
    };
    let result = kyomi_core::db_execute!(pool, &sql, &settings_str, workspace_id)?;

    // Sync log — best-effort: log a warning and continue on failure.
    if result.rows_affected() > 0 {
        let snapshot = fetch_workspace_settings_snapshot(pool, workspace_id).await;
        if let Err(e) = sync_log_service::write_sync_entry(
            pool,
            entity_types::WORKSPACE_SETTINGS,
            workspace_id,
            workspace_id,
            SyncActionType::Update,
            snapshot,
        )
        .await
        {
            tracing::warn!(error = %e, workspace_id = %workspace_id, "Failed to write sync log entry");
        }
    }

    Ok(result.rows_affected() > 0)
}

/// Get a workspace with all fields (SELECT *).
///
/// This is functionally identical to `user_service::get_workspace` but lives
/// in workspace_service for domain clarity. Both are thin wrappers over the
/// same query — no duplication of logic, just organizational convenience.
pub async fn get_workspace_full(
    pool: &DbPool,
    workspace_id: &str,
) -> kyomi_core::Result<Option<Workspace>> {
    let ws = kyomi_core::db_fetch_optional!(
        pool, Workspace,
        "SELECT * FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?;
    Ok(ws)
}

/// Update workspace catalog onboarding fields.
///
/// Sets `catalog_onboarding_completed` and `catalog_indexed_projects` on the
/// workspace. Called from the onboarding/catalog/complete endpoint.
///
/// `catalog_indexed_projects` is a Postgres `json` column, so we cast the
/// text bind with `$2::json` (see `update_workspace_settings` for details).
pub async fn update_catalog_onboarding(
    pool: &DbPool,
    workspace_id: &str,
    completed: bool,
    indexed_projects: &serde_json::Value,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let projects_str = serde_json::to_string(indexed_projects)
        .map_err(|e| kyomi_core::Error::Internal(format!("JSON serialization failed: {e}")))?;
    let sql = if is_pg {
        format!(
            "UPDATE workspaces SET \
             catalog_onboarding_completed = $1, \
             catalog_indexed_projects = $2::json, \
             updated_at = {now} \
             WHERE workspace_id = $3"
        )
    } else {
        format!(
            "UPDATE workspaces SET \
             catalog_onboarding_completed = $1, \
             catalog_indexed_projects = $2, \
             updated_at = {now} \
             WHERE workspace_id = $3"
        )
    };
    let result = kyomi_core::db_execute!(pool, &sql, completed, &projects_str, workspace_id)?;
    Ok(result.rows_affected() > 0)
}

/// Update workspace business knowledge and its timestamp.
pub async fn update_workspace_knowledge(
    pool: &DbPool,
    workspace_id: &str,
    knowledge: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now_expr = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE workspaces SET \
         business_knowledge = $1, \
         knowledge_updated_at = {now_expr}, \
         updated_at = {now_expr} \
         WHERE workspace_id = $2"
    );
    let result = kyomi_core::db_execute!(pool, &sql, knowledge, workspace_id)?;

    // Sync log — best-effort: log a warning and continue on failure.
    if result.rows_affected() > 0 {
        let snapshot = fetch_workspace_settings_snapshot(pool, workspace_id).await;
        if let Err(e) = sync_log_service::write_sync_entry(
            pool,
            entity_types::WORKSPACE_SETTINGS,
            workspace_id,
            workspace_id,
            SyncActionType::Update,
            snapshot,
        )
        .await
        {
            tracing::warn!(error = %e, workspace_id = %workspace_id, "Failed to write sync log entry");
        }
    }

    Ok(result.rows_affected() > 0)
}

// ===========================================================================
// Phase 4D — Member management
// ===========================================================================

/// A workspace member with joined user data.
///
/// Used by the list_members endpoint to return user details alongside
/// membership info in a single query.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MemberWithUser {
    // WorkspaceUser fields
    pub wu_id: i32,
    pub workspace_id: String,
    pub user_id: String,
    pub role: String,
    pub active: bool,
    pub wu_created_at: chrono::DateTime<chrono::Utc>,
    // User fields
    pub email: String,
    pub name: Option<String>,
}

/// Get all active workspace members with their user details.
///
/// Performs a single JOIN query rather than N+1 lookups.
pub async fn get_workspace_members_with_users(
    pool: &DbPool,
    workspace_id: &str,
) -> kyomi_core::Result<Vec<MemberWithUser>> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT wu.id AS wu_id, wu.workspace_id, wu.user_id, wu.role, wu.active, \
                wu.created_at AS wu_created_at, u.email, u.name \
         FROM workspace_users wu \
         JOIN users u ON u.user_id = wu.user_id \
         WHERE wu.workspace_id = $1 AND wu.active = {bt} \
         ORDER BY wu.created_at ASC"
    );
    let members = kyomi_core::db_fetch_all!(pool, MemberWithUser, &sql, workspace_id)?;
    Ok(members)
}

/// Get a single workspace membership record.
pub async fn get_workspace_user(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> kyomi_core::Result<Option<WorkspaceUser>> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT * FROM workspace_users \
         WHERE workspace_id = $1 AND user_id = $2 AND active = {bt}"
    );
    let wu = kyomi_core::db_fetch_optional!(pool, WorkspaceUser, &sql, workspace_id, user_id)?;
    Ok(wu)
}

/// Update a member's role in a workspace.
pub async fn update_member_role(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
    new_role: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "UPDATE workspace_users SET role = $1 \
         WHERE workspace_id = $2 AND user_id = $3 AND active = {bt}"
    );
    let result = kyomi_core::db_execute!(pool, &sql, new_role, workspace_id, user_id)?;
    Ok(result.rows_affected() > 0)
}

/// Remove a member from a workspace (hard delete).
pub async fn remove_member(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> kyomi_core::Result<bool> {
    let result = kyomi_core::db_execute!(
        pool,
        "DELETE FROM workspace_users \
         WHERE workspace_id = $1 AND user_id = $2",
        workspace_id, user_id
    )?;
    Ok(result.rows_affected() > 0)
}

/// Count admins in a workspace.
pub async fn count_admins(
    pool: &DbPool,
    workspace_id: &str,
) -> kyomi_core::Result<i64> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT COUNT(*) as count FROM workspace_users \
         WHERE workspace_id = $1 AND role = 'workspace_admin' AND active = {bt}"
    );
    let count: i64 = kyomi_core::db_fetch_scalar!(pool, i64, &sql, workspace_id)?;
    Ok(count)
}

/// Create a new workspace membership.
pub async fn create_workspace_user(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
    role: &str,
) -> kyomi_core::Result<()> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
         VALUES ($1, $2, $3, {bt})"
    );
    kyomi_core::db_execute!(pool, &sql, workspace_id, user_id, role)?;
    Ok(())
}

// ===========================================================================
// Phase 4D — Invitation management
// ===========================================================================

/// Create a workspace invitation and return the inserted record.
pub async fn create_invitation(
    pool: &DbPool,
    invitation_id: &str,
    workspace_id: &str,
    email: &str,
    role: &str,
    invited_by: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> kyomi_core::Result<WorkspaceInvitation> {
    kyomi_core::db_execute!(
        pool,
        "INSERT INTO workspace_invitations \
         (invitation_id, workspace_id, email, role, invited_by_user_id, status, expires_at) \
         VALUES ($1, $2, $3, $4, $5, 'pending', $6)",
        invitation_id, workspace_id, email, role, invited_by, &expires_at
    )?;

    get_invitation(pool, invitation_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("Invitation created but not found".into()))
}

/// Get an invitation by ID.
pub async fn get_invitation(
    pool: &DbPool,
    invitation_id: &str,
) -> kyomi_core::Result<Option<WorkspaceInvitation>> {
    let inv = kyomi_core::db_fetch_optional!(
        pool, WorkspaceInvitation,
        "SELECT * FROM workspace_invitations WHERE invitation_id = $1",
        invitation_id
    )?;
    Ok(inv)
}

/// Get an invitation by ID scoped to a workspace.
pub async fn get_invitation_in_workspace(
    pool: &DbPool,
    invitation_id: &str,
    workspace_id: &str,
) -> kyomi_core::Result<Option<WorkspaceInvitation>> {
    let inv = kyomi_core::db_fetch_optional!(
        pool, WorkspaceInvitation,
        "SELECT * FROM workspace_invitations \
         WHERE invitation_id = $1 AND workspace_id = $2",
        invitation_id, workspace_id
    )?;
    Ok(inv)
}

/// Get all pending invitations for a workspace.
pub async fn get_pending_invitations_for_workspace(
    pool: &DbPool,
    workspace_id: &str,
) -> kyomi_core::Result<Vec<WorkspaceInvitation>> {
    let invitations = kyomi_core::db_fetch_all!(
        pool, WorkspaceInvitation,
        "SELECT * FROM workspace_invitations \
         WHERE workspace_id = $1 AND status = 'pending' \
         ORDER BY created_at DESC",
        workspace_id
    )?;
    Ok(invitations)
}

/// Get all pending invitations addressed to a specific email.
pub async fn get_pending_invitations_for_email(
    pool: &DbPool,
    email: &str,
) -> kyomi_core::Result<Vec<WorkspaceInvitation>> {
    let invitations = kyomi_core::db_fetch_all!(
        pool, WorkspaceInvitation,
        "SELECT * FROM workspace_invitations \
         WHERE LOWER(email) = LOWER($1) AND status = 'pending' \
         ORDER BY created_at DESC",
        email
    )?;
    Ok(invitations)
}

/// Workspace invitation enriched with workspace and inviter display names.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EnrichedInvitation {
    pub invitation_id: String,
    pub workspace_id: String,
    pub email: String,
    pub role: String,
    pub invited_by_user_id: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub accepted_by_user_id: Option<String>,
    pub workspace_name: Option<String>,
    pub inviter_name: Option<String>,
}

/// Get all pending invitations for a specific email, enriched with workspace
/// name and inviter name via LEFT JOINs.
pub async fn get_pending_invitations_enriched_for_email(
    pool: &DbPool,
    email: &str,
) -> kyomi_core::Result<Vec<EnrichedInvitation>> {
    let sql = "SELECT wi.invitation_id, wi.workspace_id, wi.email, wi.role, \
                      wi.invited_by_user_id, wi.status, wi.created_at, \
                      wi.expires_at, wi.accepted_at, wi.accepted_by_user_id, \
                      w.name AS workspace_name, \
                      u.name AS inviter_name \
               FROM workspace_invitations wi \
               LEFT JOIN workspaces w ON w.workspace_id = wi.workspace_id \
               LEFT JOIN users u ON u.user_id = wi.invited_by_user_id \
               WHERE LOWER(wi.email) = LOWER($1) AND wi.status = 'pending' \
               ORDER BY wi.created_at DESC";
    let invitations = kyomi_core::db_fetch_all!(pool, EnrichedInvitation, sql, email)?;
    Ok(invitations)
}

/// Count pending invitations for a workspace.
pub async fn count_pending_invitations(
    pool: &DbPool,
    workspace_id: &str,
) -> kyomi_core::Result<i64> {
    let count: i64 = kyomi_core::db_fetch_scalar!(
        pool, i64,
        "SELECT COUNT(*) as count FROM workspace_invitations \
         WHERE workspace_id = $1 AND status = 'pending'",
        workspace_id
    )?;
    Ok(count)
}

/// Check whether a user with the given email is already a member of the workspace.
pub async fn check_existing_member_by_email(
    pool: &DbPool,
    workspace_id: &str,
    email: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT COUNT(*) as count FROM workspace_users wu \
         JOIN users u ON u.user_id = wu.user_id \
         WHERE wu.workspace_id = $1 AND LOWER(u.email) = LOWER($2) AND wu.active = {bt}"
    );
    let count: i64 = kyomi_core::db_fetch_scalar!(pool, i64, &sql, workspace_id, email)?;
    Ok(count > 0)
}

/// Check whether a pending invitation already exists for the given email in the workspace.
pub async fn check_pending_invitation(
    pool: &DbPool,
    workspace_id: &str,
    email: &str,
) -> kyomi_core::Result<bool> {
    let count: i64 = kyomi_core::db_fetch_scalar!(
        pool, i64,
        "SELECT COUNT(*) as count FROM workspace_invitations \
         WHERE workspace_id = $1 AND LOWER(email) = LOWER($2) AND status = 'pending'",
        workspace_id, email
    )?;
    Ok(count > 0)
}

/// Update an invitation's status.
pub async fn update_invitation_status(
    pool: &DbPool,
    invitation_id: &str,
    status: &str,
) -> kyomi_core::Result<bool> {
    let result = kyomi_core::db_execute!(
        pool,
        "UPDATE workspace_invitations SET status = $1 WHERE invitation_id = $2",
        status, invitation_id
    )?;
    Ok(result.rows_affected() > 0)
}

/// Accept an invitation: set status to 'accepted', record who accepted and when.
pub async fn accept_invitation(
    pool: &DbPool,
    invitation_id: &str,
    user_id: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE workspace_invitations SET \
         status = 'accepted', accepted_at = {now}, accepted_by_user_id = $1 \
         WHERE invitation_id = $2 AND status = 'pending'"
    );
    let result = kyomi_core::db_execute!(pool, &sql, user_id, invitation_id)?;
    Ok(result.rows_affected() > 0)
}

/// Accept an invitation for a newly-created user (self-hosted SMTP-less flow).
///
/// Adds the user to the workspace and marks the invitation as accepted.
/// Used during one-step signup when the user has a pending invitation.
pub async fn accept_invitation_for_user(
    pool: &DbPool,
    invitation_id: &str,
    user_id: &str,
) -> kyomi_core::Result<()> {
    let invitation = get_invitation(pool, invitation_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Invitation not found".into()))?;

    // CAS: atomically transition status from 'pending' to 'accepted'.
    // If another concurrent accept already won the race, this returns false
    // and we skip the membership insert — no duplicate membership possible.
    let accepted = accept_invitation(pool, invitation_id, user_id).await?;
    if !accepted {
        return Ok(());
    }

    let db_role = invitation.role.as_ref();
    create_workspace_user(pool, &invitation.workspace_id, user_id, db_role).await?;
    Ok(())
}

// ===========================================================================
// Phase 4D — Ownership transfer
// ===========================================================================

/// Create an ownership transfer request and return the inserted record.
pub async fn create_ownership_transfer(
    pool: &DbPool,
    transfer_id: &str,
    workspace_id: &str,
    from_user_id: &str,
    to_user_id: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> kyomi_core::Result<OwnershipTransfer> {
    kyomi_core::db_execute!(
        pool,
        "INSERT INTO ownership_transfers \
         (transfer_id, workspace_id, from_user_id, to_user_id, status, expires_at) \
         VALUES ($1, $2, $3, $4, 'pending', $5)",
        transfer_id, workspace_id, from_user_id, to_user_id, &expires_at
    )?;

    get_ownership_transfer(pool, transfer_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("Transfer created but not found".into()))
}

/// Get an ownership transfer by ID.
pub async fn get_ownership_transfer(
    pool: &DbPool,
    transfer_id: &str,
) -> kyomi_core::Result<Option<OwnershipTransfer>> {
    let transfer = kyomi_core::db_fetch_optional!(
        pool, OwnershipTransfer,
        "SELECT * FROM ownership_transfers WHERE transfer_id = $1",
        transfer_id
    )?;
    Ok(transfer)
}

/// Get a pending transfer for a specific workspace.
pub async fn get_pending_transfer_for_workspace(
    pool: &DbPool,
    workspace_id: &str,
) -> kyomi_core::Result<Option<OwnershipTransfer>> {
    let transfer = kyomi_core::db_fetch_optional!(
        pool, OwnershipTransfer,
        "SELECT * FROM ownership_transfers \
         WHERE workspace_id = $1 AND status = 'pending' \
         ORDER BY created_at DESC LIMIT 1",
        workspace_id
    )?;
    Ok(transfer)
}

/// Get all pending transfers where the given user is the recipient.
pub async fn get_pending_transfers_for_user(
    pool: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<Vec<OwnershipTransfer>> {
    let transfers = kyomi_core::db_fetch_all!(
        pool, OwnershipTransfer,
        "SELECT * FROM ownership_transfers \
         WHERE to_user_id = $1 AND status = 'pending' \
         ORDER BY created_at DESC",
        user_id
    )?;
    Ok(transfers)
}

/// Update a transfer's status and set completed_at = NOW().
pub async fn update_transfer_status(
    pool: &DbPool,
    transfer_id: &str,
    status: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE ownership_transfers SET status = $1, completed_at = {now} \
         WHERE transfer_id = $2"
    );
    let result = kyomi_core::db_execute!(pool, &sql, status, transfer_id)?;
    Ok(result.rows_affected() > 0)
}

/// Complete an ownership transfer in a transaction:
/// 1. Update workspace owner_user_id
/// 2. Ensure new owner has workspace_admin role
/// 3. Mark transfer as accepted with completed_at
pub async fn complete_ownership_transfer(
    pool: &DbPool,
    transfer_id: &str,
    workspace_id: &str,
    new_owner_id: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let bt = sql_compat::bool_true(is_pg);

    let update_owner_sql = format!(
        "UPDATE workspaces SET owner_user_id = $1, updated_at = {now} \
         WHERE workspace_id = $2"
    );
    let update_role_sql = format!(
        "UPDATE workspace_users SET role = 'workspace_admin' \
         WHERE workspace_id = $1 AND user_id = $2 AND active = {bt}"
    );
    let update_transfer_sql = format!(
        "UPDATE ownership_transfers SET status = 'accepted', completed_at = {now} \
         WHERE transfer_id = $1"
    );

    match pool {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let mut tx = pg.begin().await?;
            sqlx::query(&update_owner_sql)
                .bind(new_owner_id).bind(workspace_id)
                .execute(&mut *tx).await?;
            sqlx::query(&update_role_sql)
                .bind(workspace_id).bind(new_owner_id)
                .execute(&mut *tx).await?;
            sqlx::query(&update_transfer_sql)
                .bind(transfer_id)
                .execute(&mut *tx).await?;
            tx.commit().await?;
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let mut tx = sq.begin().await?;
            sqlx::query(&update_owner_sql)
                .bind(new_owner_id).bind(workspace_id)
                .execute(&mut *tx).await?;
            sqlx::query(&update_role_sql)
                .bind(workspace_id).bind(new_owner_id)
                .execute(&mut *tx).await?;
            sqlx::query(&update_transfer_sql)
                .bind(transfer_id)
                .execute(&mut *tx).await?;
            tx.commit().await?;
        }
    }

    Ok(true)
}

/// Update workspace owner_user_id directly (used by ownership transfer).
pub async fn update_workspace_owner(
    pool: &DbPool,
    workspace_id: &str,
    new_owner_id: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE workspaces SET owner_user_id = $1, updated_at = {now} \
         WHERE workspace_id = $2"
    );
    let result = kyomi_core::db_execute!(pool, &sql, new_owner_id, workspace_id)?;
    Ok(result.rows_affected() > 0)
}

// ─── Orchestration ─────────────────────────────────────────────────────────

pub async fn initiate_transfer(
    pool: &DbPool,
    workspace_id: &str,
    from_user_id: &str,
    to_user_id: &str,
    from_email: &str,
    workspace_name: &str,
    from_name: &str,
) -> kyomi_core::Result<()> {
    let existing = get_pending_transfer_for_workspace(pool, workspace_id).await?;
    if existing.is_some() {
        return Err(kyomi_core::Error::Conflict(
            "There is already a pending ownership transfer for this workspace".into(),
        ));
    }

    let transfer_id = format!(
        "xfer-{}",
        &sqlx::types::Uuid::new_v4().simple().to_string()[..24]
    );
    let expires_at = chrono::Utc::now() + chrono::Duration::days(7);

    create_ownership_transfer(pool, &transfer_id, workspace_id, from_user_id, to_user_id, expires_at).await?;

    let to_user = crate::user_service::get_user_by_id(pool, to_user_id).await?;
    if let Some(to_user) = to_user {
        let to_name = to_user.name.clone().unwrap_or_else(|| to_user.email.clone());
        let to_email = to_user.email.clone();
        let ws_name = workspace_name.to_string();
        let f_name = from_name.to_string();
        let f_email = from_email.to_string();

        tokio::spawn(async move {
            let svc = crate::email_service::EmailService::from_env();
            svc.send_ownership_transfer(&to_email, &ws_name, &f_name, &to_name, "initiated").await;
            svc.send_ownership_transfer(&f_email, &ws_name, &f_name, &to_name, "confirmation").await;
        });
    }

    Ok(())
}

/// Enriched ownership transfer for display on the accept-ownership page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnershipTransferDetail {
    pub transfer_id: String,
    pub workspace_name: String,
    pub from_user_email: String,
    pub expires_at: DateTime<Utc>,
    pub status: TransferStatus,
}

/// Fetch an ownership transfer for a specific recipient, auto-expiring if past
/// its deadline.
///
/// Returns `None` if the transfer doesn't exist, isn't pending, is expired, or
/// `recipient_id` doesn't match `to_user_id`.
pub async fn get_transfer_for_recipient(
    pool: &DbPool,
    transfer_id: &str,
    recipient_id: &str,
) -> kyomi_core::Result<Option<OwnershipTransferDetail>> {
    let Some(transfer) = get_ownership_transfer(pool, transfer_id).await? else {
        return Ok(None);
    };

    if transfer.to_user_id != recipient_id {
        return Ok(None);
    }

    if transfer.status != TransferStatus::Pending {
        return Ok(None);
    }

    if transfer.expires_at < Utc::now() {
        let _ = update_transfer_status(pool, transfer_id, "expired").await;
        return Ok(None);
    }

    let workspace = get_workspace_full(pool, &transfer.workspace_id).await?;
    let workspace_name = workspace
        .and_then(|w| w.name)
        .unwrap_or_else(|| "Unnamed Workspace".to_string());

    let from_user =
        crate::user_service::get_user_by_id(pool, &transfer.from_user_id).await?;
    let from_user_email = from_user.map(|u| u.email).unwrap_or_default();

    Ok(Some(OwnershipTransferDetail {
        transfer_id: transfer.transfer_id,
        workspace_name,
        from_user_email,
        expires_at: transfer.expires_at,
        status: transfer.status,
    }))
}

/// Remove a member from a workspace, enforcing all business rules.
///
/// Returns an `Err` with a user-facing message on any rule violation.
pub async fn remove_workspace_member(
    pool: &DbPool,
    workspace_id: &str,
    owner_user_id: &str,
    requesting_user_id: &str,
    target_user_id: &str,
) -> kyomi_core::Result<()> {
    if target_user_id == owner_user_id {
        return Err(kyomi_core::Error::BadRequest(
            "Cannot remove workspace owner. Transfer ownership first.".into(),
        ));
    }

    if target_user_id == requesting_user_id {
        let admin_count = count_admins(pool, workspace_id).await?;
        if admin_count < 2 {
            return Err(kyomi_core::Error::BadRequest(
                "Cannot remove yourself: you are the only admin".into(),
            ));
        }
    }

    let target = get_workspace_user(pool, workspace_id, target_user_id).await?;
    if target.is_none() {
        return Err(kyomi_core::Error::NotFound(
            "Member not found in workspace".into(),
        ));
    }

    kyomi_core::db_execute!(
        pool,
        "UPDATE chat_sessions SET user_id = $1 \
         WHERE user_id = $2 AND workspace_id = $3 AND shared = true",
        owner_user_id,
        target_user_id,
        workspace_id
    )?;

    remove_member(pool, workspace_id, target_user_id).await?;
    Ok(())
}

/// Parameters for `invite_workspace_member`.
pub struct InviteWorkspaceMemberParams<'a> {
    pub pool: &'a DbPool,
    pub workspace_id: &'a str,
    pub email: &'a str,
    pub db_role: &'a str,
    pub invited_by: &'a str,
    pub invitation_id: &'a str,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub user_limit: Option<i64>,
}

/// Create a workspace invitation, enforcing duplicate and user-limit checks.
///
/// `email` is expected to be already trimmed and lowercased by the caller.
pub async fn invite_workspace_member(
    params: InviteWorkspaceMemberParams<'_>,
) -> kyomi_core::Result<()> {
    let InviteWorkspaceMemberParams {
        pool, workspace_id, email, db_role, invited_by, invitation_id, expires_at, user_limit,
    } = params;
    let is_member = check_existing_member_by_email(pool, workspace_id, email).await?;
    if is_member {
        return Err(kyomi_core::Error::BadRequest(
            "User is already a member of this workspace".into(),
        ));
    }

    let has_pending = check_pending_invitation(pool, workspace_id, email).await?;
    if has_pending {
        return Err(kyomi_core::Error::BadRequest(
            "Invitation already pending for this email".into(),
        ));
    }

    if let Some(limit) = user_limit {
        let current_users = count_workspace_users(pool, workspace_id).await?;
        let pending = count_pending_invitations(pool, workspace_id).await?;
        if current_users + pending >= limit {
            return Err(kyomi_core::Error::BadRequest(
                "Workspace user limit reached. Upgrade your plan to add more users."
                    .into(),
            ));
        }
    }

    create_invitation(
        pool,
        invitation_id,
        workspace_id,
        email,
        db_role,
        invited_by,
        expires_at,
    )
    .await?;
    Ok(())
}

/// A resolved ownership transfer with sender and recipient emails.
#[derive(Debug, Clone)]
pub struct ResolvedTransfer {
    pub transfer_id: String,
    pub from_user_id: String,
    pub from_user_email: String,
    pub to_user_id: String,
    pub to_user_email: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub is_initiator: bool,
    pub is_recipient: bool,
}

/// List all pending ownership transfers relevant to a user.
///
/// Combines transfers where the user is the recipient with any transfer they
/// initiated as workspace owner, deduplicates, and resolves email addresses.
pub async fn list_ownership_transfers_for_user(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> kyomi_core::Result<Vec<ResolvedTransfer>> {
    let mut transfers = get_pending_transfers_for_user(pool, user_id).await?;

    if let Some(initiated) =
        get_pending_transfer_for_workspace(pool, workspace_id).await?
        && !transfers
            .iter()
            .any(|t| t.transfer_id == initiated.transfer_id)
        {
            transfers.push(initiated);
        }

    let mut result = Vec::with_capacity(transfers.len());
    for transfer in &transfers {
        let from_email =
            crate::user_service::get_user_by_id(pool, &transfer.from_user_id)
                .await?
                .map(|u| u.email)
                .unwrap_or_default();

        let to_email =
            crate::user_service::get_user_by_id(pool, &transfer.to_user_id)
                .await?
                .map(|u| u.email)
                .unwrap_or_default();

        result.push(ResolvedTransfer {
            transfer_id: transfer.transfer_id.clone(),
            from_user_id: transfer.from_user_id.clone(),
            from_user_email: from_email,
            to_user_id: transfer.to_user_id.clone(),
            to_user_email: to_email,
            status: transfer.status.to_string(),
            created_at: transfer.created_at,
            expires_at: transfer.expires_at,
            is_initiator: transfer.from_user_id == user_id,
            is_recipient: transfer.to_user_id == user_id,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `accept_invitation` returns false when called on an
    /// already-accepted invitation (simulating the CAS loser in a double-accept
    /// race). Requires a running Postgres instance via DATABASE_URL.
    #[tokio::test]
    async fn accept_invitation_cas_rejects_double_accept() {
        let database_url = match std::env::var("DATABASE_URL") {
            Ok(url) if url.starts_with("postgresql://") || url.starts_with("postgres://") => url,
            _ => {
                eprintln!("skipping: DATABASE_URL not set or not Postgres");
                return;
            }
        };

        let pool = DbPool::connect(&database_url)
            .await
            .expect("failed to connect to database");

        // Use unique IDs to avoid collisions with other tests.
        let invitation_id = format!("test-cas-{}", uuid::Uuid::new_v4());
        let workspace_id = format!("test-ws-{}", uuid::Uuid::new_v4());
        let user_id = format!("test-user-{}", uuid::Uuid::new_v4());
        let email = format!("cas-test-{}@example.com", uuid::Uuid::new_v4());
        let expires_at = chrono::Utc::now() + chrono::Duration::days(7);

        // Create a workspace (needed for FK constraints).
        kyomi_core::db_execute!(
            &pool,
            "INSERT INTO workspaces (workspace_id, name, slug) \
             VALUES ($1, 'CAS Test', $2)",
            workspace_id,
            workspace_id
        )
        .expect("failed to create workspace");

        // Create a pending invitation.
        create_invitation(
            &pool,
            &invitation_id,
            &workspace_id,
            &email,
            "viewer",
            &user_id,
            expires_at,
        )
        .await
        .expect("failed to create invitation");

        // First accept should succeed (CAS wins, 1 row updated).
        let first = accept_invitation(&pool, &invitation_id, &user_id)
            .await
            .expect("first accept failed");
        assert!(first, "first accept should return true (CAS won)");

        // Second accept should be a no-op (CAS loses, 0 rows updated).
        let second = accept_invitation(&pool, &invitation_id, &user_id)
            .await
            .expect("second accept errored");
        assert!(!second, "second accept should return false (already accepted)");

        // Cleanup: delete the test rows.
        kyomi_core::db_execute!(
            &pool,
            "DELETE FROM workspace_invitations WHERE invitation_id = $1",
            invitation_id
        )
        .expect("cleanup failed");
        kyomi_core::db_execute!(
            &pool,
            "DELETE FROM workspaces WHERE workspace_id = $1",
            workspace_id
        )
        .expect("cleanup failed");
    }
}
