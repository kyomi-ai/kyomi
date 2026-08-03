// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace service — query functions for workspace management.
//!
//! Used by workspace endpoints (4C/4D) and user endpoints that need
//! workspace context. Complements `user_service.rs` which already has
//! `get_workspace` and `get_workspace_user` for single-record lookups.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use kyomi_core::db::in_clause_placeholders;
use kyomi_core::enums::{
    CatalogRefreshStatus, SubscriptionStatus, SubscriptionTier, TransferStatus, WorkspaceRole,
    WorkspaceStatus,
};

/// Build a `StripeService` from application config when Stripe is configured.
/// Returns `None` for self-hosted installs without Stripe keys.
fn stripe_from_config(
    config: &kyomi_core::Config,
) -> Option<crate::stripe_service::StripeService> {
    config.stripe_secret_key.as_ref().map(|sk| {
        crate::stripe_service::StripeService::new(
            sk,
            config.stripe_webhook_secret.as_deref().unwrap_or_default(),
        )
    })
}
use kyomi_core::models::{
    OwnershipTransfer, Workspace, WorkspaceInvitation, WorkspaceUser,
};
use kyomi_core::sql_compat;
use kyomi_core::{DbPool, KVPool};
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

/// Summary of a workspace a user belongs to, for the workspace switcher.
#[derive(Debug, Clone)]
pub struct UserWorkspaceSummary {
    pub workspace_id: String,
    pub name: Option<String>,
    pub member_count: i64,
    pub subscription_tier: kyomi_core::enums::SubscriptionTier,
    pub role: kyomi_core::enums::WorkspaceRole,
}

/// Get all workspaces a user belongs to, enriched with member counts.
///
/// 2 queries total: one JOIN for the memberships (via `get_user_workspaces`),
/// one grouped query for all member counts.
pub async fn get_user_workspaces_with_counts(
    pool: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<Vec<UserWorkspaceSummary>> {
    let pairs = get_user_workspaces(pool, user_id).await?;

    let workspace_ids: Vec<String> = pairs.iter().map(|(ws, _)| ws.workspace_id.clone()).collect();
    let counts = fetch_member_counts(pool, &workspace_ids).await?;

    let out = pairs
        .into_iter()
        .map(|(ws, wu)| {
            let member_count = counts.get(&ws.workspace_id).copied().unwrap_or(0);
            UserWorkspaceSummary {
                workspace_id: ws.workspace_id,
                name: ws.name,
                member_count,
                subscription_tier: ws.subscription_tier,
                role: wu.role,
            }
        })
        .collect();
    Ok(out)
}

#[derive(Debug, sqlx::FromRow)]
struct MemberCountRow {
    workspace_id: String,
    count: i64,
}

/// Count active members for a list of workspaces in a single grouped query.
///
/// Uses `= ANY($1)` on Postgres and individual placeholders on SQLite,
/// mirroring `chat_service::fetch_session_counts`. Workspaces with zero
/// active members are simply absent from the returned map — callers should
/// default missing entries to `0`.
async fn fetch_member_counts(
    pool: &DbPool,
    workspace_ids: &[String],
) -> kyomi_core::Result<HashMap<String, i64>> {
    if workspace_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);

    let rows: Vec<MemberCountRow> = match pool {
        DbPool::Postgres(pg) => {
            let sql = format!(
                "SELECT workspace_id, COUNT(*) as count FROM workspace_users \
                 WHERE workspace_id = ANY($1) AND active = {bt} \
                 GROUP BY workspace_id"
            );
            sqlx::query_as::<_, MemberCountRow>(&sql)
                .bind(workspace_ids)
                .fetch_all(pg)
                .await?
        }
        DbPool::Sqlite(sq) => {
            let (in_clause, _) = in_clause_placeholders(workspace_ids.len(), 1);
            let sql = format!(
                "SELECT workspace_id, COUNT(*) as count FROM workspace_users \
                 WHERE workspace_id IN {in_clause} AND active = {bt} \
                 GROUP BY workspace_id"
            );
            let mut query = sqlx::query_as::<_, MemberCountRow>(&sql);
            for id in workspace_ids {
                query = query.bind(id);
            }
            query.fetch_all(sq).await?
        }
    };

    Ok(rows.into_iter().map(|r| (r.workspace_id, r.count)).collect())
}

/// Combined row shape for the `workspace_users JOIN workspaces` query used by
/// `get_user_workspaces`.
///
/// `Workspace` and `WorkspaceUser` both have `workspace_id` and `created_at`
/// columns, so the `WorkspaceUser` side is selected with a `wu_` alias
/// prefix to avoid a same-named-column collision — relying on positional or
/// duplicate-name resolution instead differs between Postgres and SQLite.
#[derive(Debug, sqlx::FromRow)]
struct WorkspaceMembershipRow {
    // -- workspaces columns (unaliased; field names match column names) --
    workspace_id: String,
    name: Option<String>,
    domain: Option<String>,
    status: WorkspaceStatus,
    admin_email: Option<String>,
    owner_user_id: String,
    subscription_tier: SubscriptionTier,
    subscription_status: SubscriptionStatus,
    billing_cycle: Option<String>,
    subscription_period_start: Option<DateTime<Utc>>,
    subscription_period_end: Option<DateTime<Utc>>,
    trial_ends_at: Option<DateTime<Utc>>,
    #[sqlx(default)]
    ai_credits_used_usd: f64,
    #[sqlx(default)]
    ai_bundle_balance_usd: f64,
    #[sqlx(default)]
    analytics_bundle_events: i64,
    user_limit: Option<i32>,
    stripe_customer_id: Option<String>,
    stripe_subscription_id: Option<String>,
    settings: Option<serde_json::Value>,
    business_knowledge: Option<String>,
    knowledge_updated_at: Option<DateTime<Utc>>,
    last_catalog_refresh: Option<DateTime<Utc>>,
    catalog_refresh_status: Option<CatalogRefreshStatus>,
    catalog_refresh_progress: Option<serde_json::Value>,
    #[sqlx(default)]
    catalog_onboarding_completed: bool,
    catalog_indexed_projects: Option<serde_json::Value>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    // -- workspace_users columns (wu_-aliased to dodge the collision above) --
    wu_id: i32,
    wu_workspace_id: String,
    wu_user_id: String,
    wu_role: WorkspaceRole,
    wu_active: bool,
    wu_created_at: DateTime<Utc>,
    wu_last_active: Option<DateTime<Utc>>,
    wu_extra_metadata: Option<serde_json::Value>,
}

impl WorkspaceMembershipRow {
    fn into_pair(self) -> (Workspace, WorkspaceUser) {
        let workspace = Workspace {
            workspace_id: self.workspace_id,
            name: self.name,
            domain: self.domain,
            status: self.status,
            admin_email: self.admin_email,
            owner_user_id: self.owner_user_id,
            subscription_tier: self.subscription_tier,
            subscription_status: self.subscription_status,
            billing_cycle: self.billing_cycle,
            subscription_period_start: self.subscription_period_start,
            subscription_period_end: self.subscription_period_end,
            trial_ends_at: self.trial_ends_at,
            ai_credits_used_usd: self.ai_credits_used_usd,
            ai_bundle_balance_usd: self.ai_bundle_balance_usd,
            analytics_bundle_events: self.analytics_bundle_events,
            user_limit: self.user_limit,
            stripe_customer_id: self.stripe_customer_id,
            stripe_subscription_id: self.stripe_subscription_id,
            settings: self.settings,
            business_knowledge: self.business_knowledge,
            knowledge_updated_at: self.knowledge_updated_at,
            last_catalog_refresh: self.last_catalog_refresh,
            catalog_refresh_status: self.catalog_refresh_status,
            catalog_refresh_progress: self.catalog_refresh_progress,
            catalog_onboarding_completed: self.catalog_onboarding_completed,
            catalog_indexed_projects: self.catalog_indexed_projects,
            created_at: self.created_at,
            updated_at: self.updated_at,
        };
        let membership = WorkspaceUser {
            id: self.wu_id,
            workspace_id: self.wu_workspace_id,
            user_id: self.wu_user_id,
            role: self.wu_role,
            active: self.wu_active,
            created_at: self.wu_created_at,
            last_active: self.wu_last_active,
            extra_metadata: self.wu_extra_metadata,
        };
        (workspace, membership)
    }
}

/// Get all workspaces a user belongs to (active memberships).
///
/// Returns pairs of (Workspace, WorkspaceUser) for each membership, ordered
/// by `wu.created_at ASC`. That ordering is the function's contract and
/// callers may rely on it — the sidebar switcher renders in this order.
/// (KYO-201 claimed `get_user_workspace_context` depends on it; it does not
/// — that function issues its own `ORDER BY created_at ASC LIMIT 1` query
/// against `workspace_users` and never calls this one.)
///
/// A single JOIN query; a membership row whose workspace row is missing is
/// dropped by the INNER JOIN, matching the previous `if let Some(ws)`
/// behavior.
pub async fn get_user_workspaces(
    pool: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<Vec<(Workspace, WorkspaceUser)>> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);

    let sql = format!(
        "SELECT \
           w.workspace_id, w.name, w.domain, w.status, w.admin_email, w.owner_user_id, \
           w.subscription_tier, w.subscription_status, w.billing_cycle, \
           w.subscription_period_start, w.subscription_period_end, w.trial_ends_at, \
           w.ai_credits_used_usd, w.ai_bundle_balance_usd, w.analytics_bundle_events, \
           w.user_limit, w.stripe_customer_id, w.stripe_subscription_id, w.settings, \
           w.business_knowledge, w.knowledge_updated_at, w.last_catalog_refresh, \
           w.catalog_refresh_status, w.catalog_refresh_progress, w.catalog_onboarding_completed, \
           w.catalog_indexed_projects, w.created_at, w.updated_at, \
           wu.id AS wu_id, wu.workspace_id AS wu_workspace_id, wu.user_id AS wu_user_id, \
           wu.role AS wu_role, wu.active AS wu_active, wu.created_at AS wu_created_at, \
           wu.last_active AS wu_last_active, wu.extra_metadata AS wu_extra_metadata \
         FROM workspace_users wu \
         JOIN workspaces w ON w.workspace_id = wu.workspace_id \
         WHERE wu.user_id = $1 AND wu.active = {bt} \
         ORDER BY wu.created_at ASC"
    );
    let rows =
        kyomi_core::db_fetch_all!(pool, WorkspaceMembershipRow, &sql, user_id)?;

    Ok(rows.into_iter().map(WorkspaceMembershipRow::into_pair).collect())
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
            sync_log_service::SyncEntryParams {
                entity_type: entity_types::WORKSPACE_SETTINGS,
                entity_id: workspace_id,
                workspace_id,
                action: SyncActionType::Update,
                data: snapshot,
                owner_user_id: None,
                is_workspace_visible: true,
            },
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
            sync_log_service::SyncEntryParams {
                entity_type: entity_types::WORKSPACE_SETTINGS,
                entity_id: workspace_id,
                workspace_id,
                action: SyncActionType::Update,
                data: snapshot,
                owner_user_id: None,
                is_workspace_visible: true,
            },
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
            sync_log_service::SyncEntryParams {
                entity_type: entity_types::WORKSPACE_SETTINGS,
                entity_id: workspace_id,
                workspace_id,
                action: SyncActionType::Update,
                data: snapshot,
                owner_user_id: None,
                is_workspace_visible: true,
            },
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
         VALUES ($1, $2, $3, {bt}) \
         ON CONFLICT (workspace_id, user_id) DO NOTHING"
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

/// Get a single invitation by ID, enriched with workspace name and inviter
/// name via LEFT JOINs. No status filter — the caller is responsible for
/// checking recipient/status/expiry (see `check_invitation_acceptable`).
pub async fn get_invitation_enriched(
    pool: &DbPool,
    invitation_id: &str,
) -> kyomi_core::Result<Option<EnrichedInvitation>> {
    let sql = "SELECT wi.invitation_id, wi.workspace_id, wi.email, wi.role, \
                      wi.invited_by_user_id, wi.status, wi.created_at, \
                      wi.expires_at, wi.accepted_at, wi.accepted_by_user_id, \
                      w.name AS workspace_name, \
                      u.name AS inviter_name \
               FROM workspace_invitations wi \
               LEFT JOIN workspaces w ON w.workspace_id = wi.workspace_id \
               LEFT JOIN users u ON u.user_id = wi.invited_by_user_id \
               WHERE wi.invitation_id = $1";
    let invitation = kyomi_core::db_fetch_optional!(pool, EnrichedInvitation, sql, invitation_id)?;
    Ok(invitation)
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
    config: Option<&kyomi_core::Config>,
) -> kyomi_core::Result<()> {
    let invitation = get_invitation(pool, invitation_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Invitation not found".into()))?;

    // CAS: atomically transition status from 'pending' to 'accepted'.
    // If another concurrent accept already consumed this invitation,
    // rows_affected == 0 and we bail — no duplicate membership is created.
    let accepted = accept_invitation(pool, invitation_id, user_id).await?;
    if !accepted {
        return Err(kyomi_core::Error::Conflict(
            "Invitation already accepted".into(),
        ));
    }

    let db_role = invitation.role.as_ref();
    create_workspace_user(pool, &invitation.workspace_id, user_id, db_role).await?;

    if let Some(config) = config
        && let Some(stripe) = stripe_from_config(config)
    {
        let user_count = count_workspace_users(pool, &invitation.workspace_id).await?;
        crate::billing_service::update_billing_users(
            pool,
            &stripe,
            &invitation.workspace_id,
            user_count,
            1,
        )
        .await?;
    }

    Ok(())
}

/// Shared context for invitation acceptance operations.
pub struct AcceptInvitationCtx<'a> {
    pub db: &'a DbPool,
    pub kv: &'a KVPool,
    pub jwt_secret: &'a str,
    pub config: Option<&'a kyomi_core::Config>,
}

/// Accept an invitation, then switch the user into the workspace they just
/// joined, re-minting the session for it.
///
/// Acceptance (atomic CAS pending→accepted + membership creation via
/// `accept_invitation_for_user`) is committed FIRST and its error propagates —
/// a failed accept is a real failure. The subsequent switch is best-effort:
/// the membership is already durably created, so a switch failure (user row
/// missing, or `switch_active_workspace` erroring) is logged and returns
/// `Ok(None)` rather than undoing the join. Returns `Some(session)` with fresh
/// cookies to apply when the switch succeeded, `None` when it was skipped/failed.
///
/// Reuses `crate::session::switch_active_workspace` (KYO-170) — no duplicated
/// JWT/cookie logic.
pub async fn accept_invitation_and_switch(
    ctx: &AcceptInvitationCtx<'_>,
    invitation_id: &str,
    user_id: &str,
    joined_workspace_id: &str,
    device_info: &crate::token_service::DeviceInfo,
) -> kyomi_core::Result<Option<crate::session::AuthenticatedSession>> {
    let AcceptInvitationCtx { db, kv, jwt_secret, config } = ctx;
    // Acceptance first — its failure is a genuine error and must propagate.
    accept_invitation_for_user(db, invitation_id, user_id, *config).await?;

    // Best-effort switch: acceptance is already committed, so from here we never
    // turn a successful join into a failure.
    let Some(user) = crate::user_service::get_user_by_id(db, user_id).await? else {
        tracing::warn!(%user_id, "invite accepted but user row not found; skipping active-workspace switch");
        return Ok(None);
    };

    match crate::session::switch_active_workspace(db, kv, jwt_secret, &user, joined_workspace_id, device_info).await {
        Ok(sess) => Ok(Some(sess)),
        Err(e) => {
            tracing::warn!(%user_id, workspace_id = %joined_workspace_id, error = %e, "invite accepted but failed to switch active workspace; user can switch manually");
            Ok(None)
        }
    }
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
    config: Option<&kyomi_core::Config>,
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

    // Reassign the departing user's shared sessions, drop their membership,
    // and drop their per-user platform links/credentials atomically. The
    // link cleanup matters even though KYO-223 already gates the one known
    // exploitable read path (`resolve_active_workspace_roles` in
    // `enterprise/kyomi-slack/src/routes.rs`): deleting the row here means a
    // *future* integration entry point that forgets to check membership
    // fails closed (no link row to resolve) instead of failing open. This
    // is defence in depth, not a replacement for that gate.
    //
    // `workspace_integrations` (the workspace's platform installation) is
    // deliberately untouched — it is keyed `(workspace_id, platform_type)`,
    // not per-user, and removing one member must not tear down the
    // workspace's Slack/etc. installation.
    let update_chat_sessions_sql = "UPDATE chat_sessions SET user_id = $1 \
         WHERE user_id = $2 AND workspace_id = $3 AND shared = true";
    let delete_workspace_users_sql = "DELETE FROM workspace_users \
         WHERE workspace_id = $1 AND user_id = $2";
    let delete_platform_user_links_sql = "DELETE FROM platform_user_links \
         WHERE workspace_id = $1 AND user_id = $2";
    let delete_workspace_user_integrations_sql = "DELETE FROM workspace_user_integrations \
         WHERE workspace_id = $1 AND user_id = $2";

    match pool {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let mut tx = pg.begin().await?;
            sqlx::query(update_chat_sessions_sql)
                .bind(owner_user_id)
                .bind(target_user_id)
                .bind(workspace_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query(delete_workspace_users_sql)
                .bind(workspace_id)
                .bind(target_user_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query(delete_platform_user_links_sql)
                .bind(workspace_id)
                .bind(target_user_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query(delete_workspace_user_integrations_sql)
                .bind(workspace_id)
                .bind(target_user_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let mut tx = sq.begin().await?;
            sqlx::query(update_chat_sessions_sql)
                .bind(owner_user_id)
                .bind(target_user_id)
                .bind(workspace_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query(delete_workspace_users_sql)
                .bind(workspace_id)
                .bind(target_user_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query(delete_platform_user_links_sql)
                .bind(workspace_id)
                .bind(target_user_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query(delete_workspace_user_integrations_sql)
                .bind(workspace_id)
                .bind(target_user_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
    }

    if let Some(config) = config
        && let Some(stripe) = stripe_from_config(config)
    {
        let user_count = count_workspace_users(pool, workspace_id).await?;
        crate::billing_service::update_billing_users(
            pool,
            &stripe,
            workspace_id,
            user_count,
            -1,
        )
        .await?;
    }

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
    use sqlx::sqlite::SqlitePoolOptions;

    /// Build an in-memory SQLite pool with migrations applied.
    async fn test_pool() -> DbPool {
        let _ = kyomi_core::constants::load_with_fallback();

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .expect("enable foreign keys");

        sqlx::migrate!("../../apps/server/migrations-sqlite")
            .run(&pool)
            .await
            .expect("run sqlite migrations");

        DbPool::Sqlite(pool)
    }

    /// Build an in-memory SQLite pool with migrations applied and FK
    /// enforcement explicitly turned OFF (sqlx defaults it on). Used only
    /// by the orphan-membership test, which needs to insert a
    /// `workspace_users` row whose `workspace_id` doesn't exist in
    /// `workspaces` — impossible with FK enforcement on.
    async fn test_pool_no_fk() -> DbPool {
        let _ = kyomi_core::constants::load_with_fallback();

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        sqlx::migrate!("../../apps/server/migrations-sqlite")
            .run(&pool)
            .await
            .expect("run sqlite migrations");

        sqlx::query("PRAGMA foreign_keys=OFF")
            .execute(&pool)
            .await
            .expect("disable foreign keys");

        DbPool::Sqlite(pool)
    }

    async fn seed_user(pool: &DbPool, user_id: &str, email: &str) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        };
        sqlx::query("INSERT INTO users (user_id, email) VALUES (?1, ?2)")
            .bind(user_id)
            .bind(email)
            .execute(sq)
            .await
            .expect("insert user");
    }

    async fn seed_workspace(pool: &DbPool, workspace_id: &str, owner_user_id: &str) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        };
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES (?1, ?2, ?3)",
        )
        .bind(workspace_id)
        .bind(format!("Workspace {workspace_id}"))
        .bind(owner_user_id)
        .execute(sq)
        .await
        .expect("insert workspace");
    }

    /// Insert an active or inactive `workspace_users` membership at an
    /// explicit `created_at`, so ordering can be asserted deterministically.
    async fn seed_membership(
        pool: &DbPool,
        workspace_id: &str,
        user_id: &str,
        active: bool,
        created_at: &str,
    ) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        };
        sqlx::query(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active, created_at) \
             VALUES (?1, ?2, 'workspace_user', ?3, ?4)",
        )
        .bind(workspace_id)
        .bind(user_id)
        .bind(active)
        .bind(created_at)
        .execute(sq)
        .await
        .expect("insert membership");
    }

    /// Insert a user, workspace, and workspace_invitation for testing.
    async fn seed_invitation(pool: &DbPool, invitation_id: &str) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        };

        // Owner user
        sqlx::query("INSERT INTO users (user_id, email) VALUES ('owner-1', 'owner@test.local')")
            .execute(sq)
            .await
            .expect("insert owner");

        // Invitee user
        sqlx::query("INSERT INTO users (user_id, email) VALUES ('user-1', 'user@test.local')")
            .execute(sq)
            .await
            .expect("insert invitee");

        // Workspace
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) \
             VALUES ('ws-1', 'Test Workspace', 'owner-1')",
        )
        .execute(sq)
        .await
        .expect("insert workspace");

        // Pending invitation
        sqlx::query(
            "INSERT INTO workspace_invitations \
             (invitation_id, workspace_id, email, role, invited_by_user_id, status, expires_at) \
             VALUES (?1, 'ws-1', 'user@test.local', 'workspace_user', 'owner-1', 'pending', datetime('now', '+7 days'))",
        )
        .bind(invitation_id)
        .execute(sq)
        .await
        .expect("insert invitation");
    }

    /// Insert a `platform_user_links` row (e.g. a Slack identity link).
    async fn seed_platform_user_link(
        pool: &DbPool,
        workspace_id: &str,
        user_id: &str,
        platform_type: &str,
        platform_user_id: &str,
    ) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        };
        sqlx::query(
            "INSERT INTO platform_user_links \
             (id, workspace_id, user_id, platform_type, platform_user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(format!("link-{workspace_id}-{user_id}-{platform_type}"))
        .bind(workspace_id)
        .bind(user_id)
        .bind(platform_type)
        .bind(platform_user_id)
        .execute(sq)
        .await
        .expect("insert platform_user_link");
    }

    /// Insert a `workspace_user_integrations` row (per-user platform credentials).
    async fn seed_workspace_user_integration(
        pool: &DbPool,
        workspace_id: &str,
        user_id: &str,
        platform_type: &str,
    ) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        };
        sqlx::query(
            "INSERT INTO workspace_user_integrations \
             (id, workspace_id, user_id, platform_type, config) \
             VALUES (?1, ?2, ?3, ?4, '{}')",
        )
        .bind(format!("uint-{workspace_id}-{user_id}-{platform_type}"))
        .bind(workspace_id)
        .bind(user_id)
        .bind(platform_type)
        .execute(sq)
        .await
        .expect("insert workspace_user_integration");
    }

    /// Insert a `workspace_integrations` row (workspace-level platform install).
    async fn seed_workspace_integration(
        pool: &DbPool,
        workspace_id: &str,
        platform_type: &str,
        installed_by: &str,
    ) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        };
        sqlx::query(
            "INSERT INTO workspace_integrations \
             (id, workspace_id, platform_type, config, installed_by) \
             VALUES (?1, ?2, ?3, '{}', ?4)",
        )
        .bind(format!("wint-{workspace_id}-{platform_type}"))
        .bind(workspace_id)
        .bind(platform_type)
        .bind(installed_by)
        .execute(sq)
        .await
        .expect("insert workspace_integration");
    }

    #[tokio::test]
    async fn accept_invitation_cas_succeeds_first_time() {
        let pool = test_pool().await;
        seed_invitation(&pool, "inv-cas-1").await;

        let accepted = accept_invitation(&pool, "inv-cas-1", "user-1").await.unwrap();
        assert!(accepted, "first accept should succeed (CAS won)");

        // Verify status is now 'accepted'.
        let inv = get_invitation(&pool, "inv-cas-1").await.unwrap().unwrap();
        assert_eq!(inv.status.to_string(), "accepted");
        assert!(inv.accepted_at.is_some());
        assert_eq!(inv.accepted_by_user_id.as_deref(), Some("user-1"));
    }

    #[tokio::test]
    async fn accept_invitation_cas_fails_second_time() {
        let pool = test_pool().await;
        seed_invitation(&pool, "inv-cas-2").await;

        // First accept wins the CAS.
        let first = accept_invitation(&pool, "inv-cas-2", "user-1").await.unwrap();
        assert!(first);

        // Second accept loses — status is no longer 'pending'.
        let second = accept_invitation(&pool, "inv-cas-2", "user-1").await.unwrap();
        assert!(!second, "second accept should fail (CAS lost)");
    }

    #[tokio::test]
    async fn accept_invitation_for_user_returns_conflict_on_double_accept() {
        let pool = test_pool().await;
        seed_invitation(&pool, "inv-cas-3").await;

        // First accept succeeds.
        accept_invitation_for_user(&pool, "inv-cas-3", "user-1", None)
            .await
            .unwrap();

        // Second accept returns Conflict.
        let err = accept_invitation_for_user(&pool, "inv-cas-3", "user-1", None)
            .await
            .unwrap_err();

        match err {
            kyomi_core::Error::Conflict(msg) => {
                assert!(msg.contains("already accepted"), "message: {msg}");
            }
            other => panic!("expected Conflict error, got: {other:?}"),
        }
    }

    // -- get_user_workspaces (KYO-201: N+1 -> single JOIN) --

    #[tokio::test]
    async fn get_user_workspaces_orders_by_created_at_and_excludes_inactive() {
        let pool = test_pool().await;
        seed_user(&pool, "user-a", "a@test.local").await;
        seed_workspace(&pool, "ws-second", "user-a").await;
        seed_workspace(&pool, "ws-first", "user-a").await;
        seed_workspace(&pool, "ws-inactive", "user-a").await;

        // Inserted out of created_at order on purpose — the JOIN's
        // `ORDER BY wu.created_at ASC` must still sort them correctly.
        seed_membership(&pool, "ws-second", "user-a", true, "2026-01-01 00:00:00").await;
        seed_membership(&pool, "ws-first", "user-a", true, "2026-01-02 00:00:00").await;
        // Later created_at, but inactive — must be excluded entirely.
        seed_membership(&pool, "ws-inactive", "user-a", false, "2026-01-03 00:00:00").await;

        let pairs = get_user_workspaces(&pool, "user-a").await.unwrap();

        let ids: Vec<&str> = pairs.iter().map(|(ws, _)| ws.workspace_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["ws-second", "ws-first"],
            "expected created_at ASC order with the inactive membership excluded"
        );
        for (ws, wu) in &pairs {
            assert_eq!(ws.workspace_id, wu.workspace_id, "workspace/membership pair mismatch");
            assert!(wu.active, "only active memberships should be returned");
        }
    }

    #[tokio::test]
    async fn get_user_workspaces_skips_membership_with_missing_workspace() {
        // Uses the no-FK pool so an orphan workspace_users row (pointing at
        // a workspace that doesn't exist) can actually be inserted — this
        // asserts the INNER JOIN drops it, matching the pre-JOIN
        // `if let Some(ws) = ...` behavior, so a future LEFT JOIN regression
        // would be caught here.
        let pool = test_pool_no_fk().await;
        seed_user(&pool, "user-b", "b@test.local").await;
        seed_workspace(&pool, "ws-real", "user-b").await;

        seed_membership(&pool, "ws-real", "user-b", true, "2026-01-01 00:00:00").await;
        seed_membership(&pool, "ws-missing", "user-b", true, "2026-01-02 00:00:00").await;

        let pairs = get_user_workspaces(&pool, "user-b").await.unwrap();

        assert_eq!(pairs.len(), 1, "orphan membership must be dropped by the JOIN");
        assert_eq!(pairs[0].0.workspace_id, "ws-real");
    }

    #[tokio::test]
    async fn get_user_workspaces_returns_empty_for_user_with_no_memberships() {
        let pool = test_pool().await;
        seed_user(&pool, "user-lonely", "lonely@test.local").await;

        let pairs = get_user_workspaces(&pool, "user-lonely").await.unwrap();
        assert!(pairs.is_empty());
    }

    // -- get_user_workspaces_with_counts (KYO-201: 1+2N -> 2 queries) --

    #[tokio::test]
    async fn get_user_workspaces_with_counts_returns_correct_member_counts() {
        let pool = test_pool().await;
        seed_user(&pool, "user-c", "c@test.local").await;
        seed_user(&pool, "user-d", "d@test.local").await;
        seed_user(&pool, "user-e", "e@test.local").await;
        seed_workspace(&pool, "ws-multi", "user-c").await;
        seed_workspace(&pool, "ws-solo", "user-c").await;

        // ws-multi: 2 active members (user-c, user-d) + 1 inactive (user-e,
        // must not count).
        seed_membership(&pool, "ws-multi", "user-c", true, "2026-01-01 00:00:00").await;
        seed_membership(&pool, "ws-multi", "user-d", true, "2026-01-01 00:00:01").await;
        seed_membership(&pool, "ws-multi", "user-e", false, "2026-01-01 00:00:02").await;
        // ws-solo: 1 active member (user-c only).
        seed_membership(&pool, "ws-solo", "user-c", true, "2026-01-02 00:00:00").await;

        let summaries = get_user_workspaces_with_counts(&pool, "user-c").await.unwrap();
        assert_eq!(summaries.len(), 2);

        let multi = summaries
            .iter()
            .find(|s| s.workspace_id == "ws-multi")
            .expect("ws-multi present");
        assert_eq!(multi.member_count, 2);

        let solo = summaries
            .iter()
            .find(|s| s.workspace_id == "ws-solo")
            .expect("ws-solo present");
        assert_eq!(solo.member_count, 1);
    }

    #[tokio::test]
    async fn get_user_workspaces_with_counts_returns_empty_without_erroring() {
        let pool = test_pool().await;
        seed_user(&pool, "user-empty", "empty@test.local").await;

        let summaries = get_user_workspaces_with_counts(&pool, "user-empty").await.unwrap();
        assert!(summaries.is_empty());
    }

    // -- remove_workspace_member (KYO-247: offboard platform links) --
    //
    // NOTE: crates/kyomi-auth's test pools are all in-memory SQLite (KYO-292,
    // open) — every test below exercises only the `DbPool::Sqlite` arm of
    // `remove_workspace_member`'s transaction. The `DbPool::Postgres` arm is
    // structurally identical (same SQL strings, same statement order) but is
    // not covered by an automated test in this crate.

    /// Seed an owner + a member with a Slack link and workspace-level Slack
    /// install, ready for `remove_workspace_member` to act on.
    async fn seed_offboard_fixture(pool: &DbPool, workspace_id: &str) {
        seed_user(pool, "owner-off", "owner-off@test.local").await;
        seed_user(pool, "member-off", "member-off@test.local").await;
        seed_workspace(pool, workspace_id, "owner-off").await;
        seed_membership(pool, workspace_id, "owner-off", true, "2026-01-01T00:00:00Z").await;
        seed_membership(pool, workspace_id, "member-off", true, "2026-01-02T00:00:00Z").await;
    }

    #[tokio::test]
    async fn remove_workspace_member_deletes_platform_user_link() {
        let pool = test_pool().await;
        seed_offboard_fixture(&pool, "ws-off-1").await;
        seed_platform_user_link(&pool, "ws-off-1", "member-off", "slack", "U123").await;

        remove_workspace_member(&pool, "ws-off-1", "owner-off", "owner-off", "member-off", None)
            .await
            .expect("remove member");

        let link =
            kyomi_core::platform::get_platform_user_link(&pool, "ws-off-1", "member-off", "slack")
                .await
                .expect("query platform_user_links");
        assert!(
            link.is_none(),
            "platform_user_links row must not survive offboarding"
        );
    }

    #[tokio::test]
    async fn remove_workspace_member_deletes_workspace_user_integration() {
        let pool = test_pool().await;
        seed_offboard_fixture(&pool, "ws-off-2").await;
        seed_workspace_user_integration(&pool, "ws-off-2", "member-off", "slack").await;

        remove_workspace_member(&pool, "ws-off-2", "owner-off", "owner-off", "member-off", None)
            .await
            .expect("remove member");

        let integration =
            kyomi_core::platform::get_user_integration(&pool, "ws-off-2", "member-off", "slack")
                .await
                .expect("query workspace_user_integrations");
        assert!(
            integration.is_none(),
            "workspace_user_integrations row must not survive offboarding"
        );
    }

    #[tokio::test]
    async fn remove_workspace_member_then_readd_does_not_restore_platform_link() {
        let pool = test_pool().await;
        seed_offboard_fixture(&pool, "ws-off-3").await;
        seed_platform_user_link(&pool, "ws-off-3", "member-off", "slack", "U123").await;

        remove_workspace_member(&pool, "ws-off-3", "owner-off", "owner-off", "member-off", None)
            .await
            .expect("remove member");

        // Re-add the same user to the workspace (e.g. re-invited and accepted).
        seed_membership(&pool, "ws-off-3", "member-off", true, "2026-01-03T00:00:00Z").await;

        let link =
            kyomi_core::platform::get_platform_user_link(&pool, "ws-off-3", "member-off", "slack")
                .await
                .expect("query platform_user_links");
        assert!(
            link.is_none(),
            "re-adding a removed member must not silently reactivate their old Slack link"
        );
    }

    /// KYO-223 put its active-membership gate on the query call site
    /// (`resolve_active_workspace_roles`), not inside `resolve_platform_user`
    /// or the disconnect helpers, precisely so a user can still unlink their
    /// own platform identity after being removed from the workspace. This
    /// pins that the offboarding cleanup doesn't couple `delete_platform_user_link`
    /// to an active-membership check — it must remain a plain, idempotent
    /// delete regardless of membership state.
    #[tokio::test]
    async fn disconnect_path_still_works_after_member_already_removed() {
        let pool = test_pool().await;
        seed_offboard_fixture(&pool, "ws-off-4").await;
        seed_platform_user_link(&pool, "ws-off-4", "member-off", "slack", "U123").await;

        remove_workspace_member(&pool, "ws-off-4", "owner-off", "owner-off", "member-off", None)
            .await
            .expect("remove member");

        // The offboarding transaction already deleted the link. A stale
        // client (or a retry) hitting "disconnect Slack" afterward must not
        // error just because the user has no active membership.
        kyomi_core::platform::delete_platform_user_link(&pool, "ws-off-4", "member-off", "slack")
            .await
            .expect("disconnect path must not require active membership");
    }

    #[tokio::test]
    async fn remove_workspace_member_does_not_delete_workspace_integration() {
        let pool = test_pool().await;
        seed_offboard_fixture(&pool, "ws-off-5").await;
        seed_workspace_integration(&pool, "ws-off-5", "slack", "owner-off").await;

        remove_workspace_member(&pool, "ws-off-5", "owner-off", "owner-off", "member-off", None)
            .await
            .expect("remove member");

        let integration =
            kyomi_core::platform::get_workspace_integration(&pool, "ws-off-5", "slack")
                .await
                .expect("query workspace_integrations");
        assert!(
            integration.is_some(),
            "workspace_integrations is workspace-scoped, not per-user — \
             removing one member must not delete the workspace's platform install"
        );
    }

    // Atomicity (point 6 of the ticket): skipped. All four statements in the
    // transaction are unconditional DELETE/UPDATE by primary/foreign key with
    // no CHECK constraints that can fail under normal data, so forcing a
    // partial-failure without adding fault-injection scaffolding (e.g. a
    // test-only trigger that raises mid-transaction) isn't possible with the
    // existing test helpers. That scaffolding would be exactly the "fragile
    // harness" the ticket says to avoid, so this is intentionally not tested
    // here. The `match pool { ... tx.commit().await? ... }` shape (copied
    // from `accept_ownership_transfer`) is the same shape already relied on
    // elsewhere in this file for atomicity.
}
