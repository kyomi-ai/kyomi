// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace service — query functions for workspace management.
//!
//! Used by workspace endpoints (4C/4D) and user endpoints that need
//! workspace context. Complements `user_service.rs` which already has
//! `get_workspace` and `get_workspace_user` for single-record lookups.

use kyomi_core::models::{
    OwnershipTransfer, Workspace, WorkspaceInvitation, WorkspaceUser,
};
use kyomi_core::sql_compat;
use kyomi_core::DbPool;

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

/// Update workspace display name.
pub async fn update_workspace_name(
    pool: &DbPool,
    workspace_id: &str,
    name: &str,
) -> kyomi_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE workspaces SET name = $1, updated_at = {now} WHERE workspace_id = $2"
    );
    let result = kyomi_core::db_execute!(pool, &sql, name, workspace_id)?;
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
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE workspaces SET \
         business_knowledge = $1, \
         knowledge_updated_at = {now}, \
         updated_at = {now} \
         WHERE workspace_id = $2"
    );
    let result = kyomi_core::db_execute!(pool, &sql, knowledge, workspace_id)?;
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
         WHERE invitation_id = $2"
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

    let db_role = invitation.role.as_ref();
    create_workspace_user(pool, &invitation.workspace_id, user_id, db_role).await?;
    accept_invitation(pool, invitation_id, user_id).await?;
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
