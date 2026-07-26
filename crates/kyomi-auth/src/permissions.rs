// SPDX-License-Identifier: AGPL-3.0-or-later

//! Single role → capability mapping for workspace authorization (KYO-189 P1).
//!
//! [`permissions_for`] is the ONLY place in the codebase that translates a
//! user's workspace role (`workspace_admin` / `workspace_user`) and owner
//! status into the set of [`Permission`]s they hold. Every server-side
//! authorization gate should call this — directly, or through a thin
//! per-crate wrapper that adapts the error type (see
//! `kyomi_ui::server_fns::require_permission` and
//! `kyomi_slack::routes::require_workspace_admin`) — rather than
//! re-checking `workspace_roles` / `is_owner` inline.
//!
//! ## Owner ⊇ Admin ⊇ Member
//!
//! This mapping treats owner as a strict superset of admin. That is only
//! sound because an owner structurally always carries the `workspace_admin`
//! role, enforced at three points:
//!
//! 1. **Workspace creation** — [`crate::user_service::create_workspace_for_user`]
//!    inserts the creator's `workspace_users` row with `role = 'workspace_admin'`
//!    in the same function that sets them as `owner_user_id`.
//! 2. **Role changes** — the `update_member_role` server function
//!    (`kyomi-ui/src/server_fns/team.rs`) rejects any attempt to change the
//!    current owner's role before calling
//!    [`crate::workspace_service::update_member_role`].
//! 3. **Ownership transfer** — [`crate::workspace_service::complete_ownership_transfer`]
//!    sets `owner_user_id` to the new owner AND sets their `workspace_users`
//!    role to `'workspace_admin'` in the same DB transaction.
//!
//! If any of these three enforcement points is ever removed, this mapping
//! must be revisited — an owner without the admin role would then get
//! implicitly granted every admin permission via the `is_owner` OR below,
//! which would be a real authorization change, not a refactor.

use std::collections::BTreeSet;

use kyomi_core::enums::WorkspaceRole;
use kyomi_types::Permission;

use crate::middleware::AuthUser;

/// Compute the set of [`Permission`]s held by `auth` in their active workspace.
///
/// Returns an empty set for users with no workspace role beyond
/// `workspace_user` — there are currently no member-level permissions;
/// every enforcement point in the codebase requires admin or owner.
pub fn permissions_for(auth: &AuthUser) -> BTreeSet<Permission> {
    let mut permissions = BTreeSet::new();

    let is_admin_or_owner = auth
        .workspace
        .workspace_roles
        .contains(&WorkspaceRole::WorkspaceAdmin)
        || auth.workspace.is_owner;

    if is_admin_or_owner {
        permissions.extend([
            Permission::ManageDatasources,
            Permission::RefreshCatalog,
            Permission::ManageWorkspaceSettings,
            Permission::ManageTeam,
            Permission::ManageAiConfig,
            Permission::ManageAnalytics,
            Permission::ManageConnect,
            Permission::SetWorkspaceDefaults,
            Permission::ManageIntegrations,
        ]);
    }

    // Billing and ownership transfer are owner-only. The owner is the
    // workspace's single spending authority (admins can invite users,
    // consuming seats, but cannot change the plan or make purchases), and
    // only the current owner can give ownership away.
    if auth.workspace.is_owner {
        permissions.insert(Permission::ManageBilling);
        permissions.insert(Permission::TransferOwnership);
    }

    permissions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::WorkspaceContext;
    use kyomi_core::enums::{SubscriptionStatus, SubscriptionTier};

    /// Minimal `AuthUser` for permission tests — only `workspace_roles` and
    /// `is_owner` vary between cases.
    fn auth_user(roles: &[WorkspaceRole], is_owner: bool) -> AuthUser {
        AuthUser {
            user_id: "user-1".to_string(),
            email: "user@example.com".to_string(),
            name: None,
            roles: Vec::new(),
            active: true,
            verified: true,
            workspace: WorkspaceContext {
                workspace_id: Some("ws-1".to_string()),
                workspace_name: Some("Test Workspace".to_string()),
                workspace_roles: roles.to_vec(),
                workspace_status: None,
                subscription_tier: SubscriptionTier::Free,
                subscription_status: SubscriptionStatus::Active,
                trial_ends_at: None,
                is_owner,
            },
            token_exp: None,
            token_jti: None,
        }
    }

    /// The full set of permissions granted by the `workspace_admin` role
    /// alone (i.e. everything except the owner-only `ManageBilling` and
    /// `TransferOwnership`).
    fn all_admin_permissions() -> BTreeSet<Permission> {
        BTreeSet::from([
            Permission::ManageDatasources,
            Permission::RefreshCatalog,
            Permission::ManageWorkspaceSettings,
            Permission::ManageTeam,
            Permission::ManageAiConfig,
            Permission::ManageAnalytics,
            Permission::ManageConnect,
            Permission::SetWorkspaceDefaults,
            Permission::ManageIntegrations,
        ])
    }

    /// The two permissions granted only to the workspace owner, never to a
    /// plain admin.
    fn owner_only_permissions() -> BTreeSet<Permission> {
        BTreeSet::from([Permission::ManageBilling, Permission::TransferOwnership])
    }

    #[test]
    fn owner_holds_every_admin_permission_plus_owner_only_ones() {
        let auth = auth_user(&[WorkspaceRole::WorkspaceAdmin], true);
        let expected: BTreeSet<Permission> = all_admin_permissions()
            .union(&owner_only_permissions())
            .copied()
            .collect();
        assert_eq!(permissions_for(&auth), expected);
    }

    #[test]
    fn admin_holds_every_admin_permission_but_no_owner_only_ones() {
        let auth = auth_user(&[WorkspaceRole::WorkspaceAdmin], false);
        assert_eq!(permissions_for(&auth), all_admin_permissions());
    }

    #[test]
    fn member_holds_no_permissions() {
        let auth = auth_user(&[WorkspaceRole::WorkspaceUser], false);
        assert_eq!(permissions_for(&auth), BTreeSet::new());
    }

    // KYO-183 removed `WorkspaceRole::WorkspaceViewer`. This test used to
    // have a `viewer_holds_no_permissions` sibling asserting the same
    // `BTreeSet::new()` outcome for the (now-gone) viewer role; with only
    // `WorkspaceAdmin`/`WorkspaceUser` left, that sibling would be a
    // byte-for-byte duplicate of this test, so it was deleted rather than
    // rewritten to assert the same thing twice under different names.

    /// Even if the structural invariant documented on `permissions_for`
    /// were ever violated (owner without the `workspace_admin` role), the
    /// `is_owner` OR still grants the full admin set plus the owner-only
    /// permissions — this locks in the *current* mapping behavior, which the
    /// module doc explicitly flags as depending on that invariant holding.
    #[test]
    fn owner_without_admin_role_still_holds_full_set() {
        let auth = auth_user(&[WorkspaceRole::WorkspaceUser], true);
        let expected: BTreeSet<Permission> = all_admin_permissions()
            .union(&owner_only_permissions())
            .copied()
            .collect();
        assert_eq!(permissions_for(&auth), expected);
    }

    // ─────────────────────────────────────────────────────────────────────
    // End-to-end: real `AuthUser` extracted from a JWT + DB round trip,
    // exercising the exact code path a gated server fn runs in production
    // (`AuthenticatedContext::extract()` → `leptos_axum::extract_with_state`
    // → this same `FromRequestParts` impl). `team::update_member_role` is
    // the representative gated server fn: it calls
    // `ac.require(Permission::ManageTeam, ...)`, which is
    // `permissions_for(&ac.auth).contains(&Permission::ManageTeam)`.
    //
    // Mirrors the `test_pool()` / `seed_membership()` pattern established in
    // `middleware.rs`'s characterization tests — no new test infrastructure.
    // ─────────────────────────────────────────────────────────────────────

    mod gated_server_fn {
        use super::*;
        use axum::extract::FromRequestParts;
        use axum::http::request::Parts;
        use sqlx::sqlite::SqlitePoolOptions;

        use crate::middleware::AuthState;

        const SECRET: &str = "test-secret-key";

        async fn test_pool() -> kyomi_core::DbPool {
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

            kyomi_core::DbPool::Sqlite(pool)
        }

        fn sqlite_of(pool: &kyomi_core::DbPool) -> &sqlx::SqlitePool {
            match pool {
                kyomi_core::DbPool::Sqlite(sq) => sq,
                _ => panic!("test requires sqlite pool"),
            }
        }

        async fn seed_user(pool: &kyomi_core::DbPool, user_id: &str) {
            sqlx::query("INSERT INTO users (user_id, email, active) VALUES ($1, $2, $3)")
                .bind(user_id)
                .bind(format!("{user_id}@test.local"))
                .bind(true)
                .execute(sqlite_of(pool))
                .await
                .expect("insert user");
        }

        async fn seed_workspace(pool: &kyomi_core::DbPool, workspace_id: &str, owner_user_id: &str) {
            sqlx::query(
                "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $2, $3)",
            )
            .bind(workspace_id)
            .bind(format!("Workspace {workspace_id}"))
            .bind(owner_user_id)
            .execute(sqlite_of(pool))
            .await
            .expect("insert workspace");
        }

        async fn seed_membership(
            pool: &kyomi_core::DbPool,
            workspace_id: &str,
            user_id: &str,
            role: &str,
        ) {
            sqlx::query(
                "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(workspace_id)
            .bind(user_id)
            .bind(role)
            .bind(true)
            .execute(sqlite_of(pool))
            .await
            .expect("insert membership");
        }

        fn mint_token(user_id: &str, workspace_id: &str) -> String {
            let mut extra: std::collections::HashMap<String, serde_json::Value> =
                std::collections::HashMap::new();
            extra.insert("user_id".into(), serde_json::json!(user_id));
            extra.insert("workspace_id".into(), serde_json::json!(workspace_id));
            crate::jwt::create_access_token_str(user_id, SECRET, 15, extra)
                .expect("mint test token")
        }

        fn parts_with_bearer(token: &str) -> Parts {
            axum::http::Request::builder()
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                .body(())
                .expect("build request")
                .into_parts()
                .0
        }

        #[tokio::test]
        async fn admin_accepted_member_rejected_by_manage_team_gate() {
            let pool = test_pool().await;
            seed_user(&pool, "owner-1").await;
            seed_user(&pool, "admin-1").await;
            seed_user(&pool, "member-1").await;
            seed_workspace(&pool, "ws-1", "owner-1").await;
            seed_membership(&pool, "ws-1", "owner-1", "workspace_admin").await;
            seed_membership(&pool, "ws-1", "admin-1", "workspace_admin").await;
            seed_membership(&pool, "ws-1", "member-1", "workspace_user").await;

            let state = AuthState {
                jwt_secret: SECRET.to_string(),
                db: pool.clone(),
                is_personal: false,
            };

            let admin_token = mint_token("admin-1", "ws-1");
            let mut admin_parts = parts_with_bearer(&admin_token);
            let admin_auth = AuthUser::from_request_parts(&mut admin_parts, &state)
                .await
                .expect("admin auth extraction");

            let member_token = mint_token("member-1", "ws-1");
            let mut member_parts = parts_with_bearer(&member_token);
            let member_auth = AuthUser::from_request_parts(&mut member_parts, &state)
                .await
                .expect("member auth extraction");

            // What `team::update_member_role`'s `ac.require(Permission::ManageTeam, ...)`
            // actually evaluates:
            assert!(
                permissions_for(&admin_auth).contains(&Permission::ManageTeam),
                "admin must be accepted by the ManageTeam gate"
            );
            assert!(
                !permissions_for(&member_auth).contains(&Permission::ManageTeam),
                "member must be rejected by the ManageTeam gate"
            );
        }
    }
}
