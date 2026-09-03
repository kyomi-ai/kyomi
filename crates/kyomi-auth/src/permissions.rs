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

        use crate::middleware::AuthState;
        use crate::test_support::{seed_membership, seed_user, seed_workspace, sqlite_pool, test_pool};

        const SECRET: &str = "test-secret-key";

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
            seed_user(sqlite_pool(&pool), "owner-1", "owner-1@test.local").await;
            seed_user(sqlite_pool(&pool), "admin-1", "admin-1@test.local").await;
            seed_user(sqlite_pool(&pool), "member-1", "member-1@test.local").await;
            seed_workspace(sqlite_pool(&pool), "ws-1", "owner-1").await;
            seed_membership(sqlite_pool(&pool), "ws-1", "owner-1", "workspace_admin", true).await;
            seed_membership(sqlite_pool(&pool), "ws-1", "admin-1", "workspace_admin", true).await;
            seed_membership(sqlite_pool(&pool), "ws-1", "member-1", "workspace_user", true).await;

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

        /// Sibling of `admin_accepted_member_rejected_by_manage_team_gate`.
        ///
        /// **What this proves, precisely:** that `permissions_for` — the
        /// single source of truth every `ac.require(Permission::X, ...)`
        /// call is defined in terms of — correctly excludes a
        /// `workspace_user` from `Permission::ManageAnalytics` and includes
        /// a `workspace_admin`. That is a necessary precondition for
        /// `get_analytics_usage`'s gate (`kyomi-ui/src/server_fns/analytics.rs`)
        /// to work, but it is **not sufficient**: this test never calls
        /// `get_analytics_usage`, so it cannot detect whether that function
        /// actually calls `ac.require(Permission::ManageAnalytics, ...)` at
        /// all. Deleting that call site does not fail this test — confirmed
        /// by mutation-testing during KYO-278 review. The call-site coverage
        /// lives in `server_fns::analytics::tests::
        /// get_analytics_usage_requires_manage_analytics_before_the_self_hosted_branch`,
        /// a source-assertion test (no request-context harness exists to
        /// call the `#[server]` fn directly — see that test's doc comment).
        ///
        /// Added for KYO-278: `get_analytics_usage` shipped with no
        /// permission check at all — every sibling server fn in that file
        /// (`list_analytics_sites`, `create_analytics_site`,
        /// `update_analytics_site`, `delete_analytics_site`) called
        /// `ac.require(Permission::ManageAnalytics, ...)` immediately after
        /// `AuthenticatedContext::extract()`; this one didn't, so any
        /// authenticated workspace member — not just admins — could read
        /// the workspace's analytics event usage, quota, and
        /// billing-adjacent bundle balance. Reuses the exact
        /// `test_pool`/`seed_*`/`mint_token` helpers above rather than
        /// adding a second copy (CODING_STANDARDS "third copy" rule — this
        /// is the second use, still within budget).
        #[tokio::test]
        async fn admin_accepted_member_rejected_by_manage_analytics_gate() {
            let pool = test_pool().await;
            seed_user(sqlite_pool(&pool), "owner-2", "owner-2@test.local").await;
            seed_user(sqlite_pool(&pool), "admin-2", "admin-2@test.local").await;
            seed_user(sqlite_pool(&pool), "member-2", "member-2@test.local").await;
            seed_workspace(sqlite_pool(&pool), "ws-2", "owner-2").await;
            seed_membership(sqlite_pool(&pool), "ws-2", "owner-2", "workspace_admin", true).await;
            seed_membership(sqlite_pool(&pool), "ws-2", "admin-2", "workspace_admin", true).await;
            seed_membership(sqlite_pool(&pool), "ws-2", "member-2", "workspace_user", true).await;

            let state = AuthState {
                jwt_secret: SECRET.to_string(),
                db: pool.clone(),
                is_personal: false,
            };

            let admin_token = mint_token("admin-2", "ws-2");
            let mut admin_parts = parts_with_bearer(&admin_token);
            let admin_auth = AuthUser::from_request_parts(&mut admin_parts, &state)
                .await
                .expect("admin auth extraction");

            let member_token = mint_token("member-2", "ws-2");
            let mut member_parts = parts_with_bearer(&member_token);
            let member_auth = AuthUser::from_request_parts(&mut member_parts, &state)
                .await
                .expect("member auth extraction");

            // What `analytics::get_analytics_usage`'s
            // `ac.require(Permission::ManageAnalytics, ...)` actually evaluates:
            assert!(
                permissions_for(&admin_auth).contains(&Permission::ManageAnalytics),
                "admin must be accepted by the ManageAnalytics gate"
            );
            assert!(
                !permissions_for(&member_auth).contains(&Permission::ManageAnalytics),
                "member must be rejected by the ManageAnalytics gate"
            );
        }
    }
}
