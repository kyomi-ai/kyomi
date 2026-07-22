// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared session creation helper.
//!
//! `create_authenticated_session` is used by:
//! - Google OAuth callback (existing user login)
//! - Accept-terms (new user signup)
//! - Passkey login complete
//! - Passkey register complete (signup flow)
//!
//! Extracts the duplicated session logic from `auth.rs` (refresh_token, switch_workspace).

use axum::http::HeaderMap;
use chrono::{Duration, Utc};
use kyomi_core::enums::WorkspaceRole;
use kyomi_core::models::User;
use kyomi_core::{DbPool, KVPool};

use crate::cookies;
use crate::jwt;
use crate::token_service::{self, DeviceInfo};
use crate::user_service;

/// Result of creating an authenticated session.
pub struct AuthenticatedSession {
    /// JWT access token
    pub access_token: String,
    /// Opaque refresh token
    pub refresh_token: String,
    /// Set-Cookie headers to include in the response
    pub cookie_headers: HeaderMap,
    /// User data for the response body
    pub user: User,
    /// Workspace context
    pub workspace_id: Option<String>,
    pub workspace_name: Option<String>,
    pub workspace_roles: Vec<WorkspaceRole>,
}

/// Create a full authenticated session for a user.
///
/// 1. Loads workspace context
/// 2. Creates JWT access token with user + workspace claims
/// 3. Creates opaque refresh token and stores in DB
/// 4. Sets HTTPOnly cookies
/// 5. Updates last_login timestamp
pub async fn create_authenticated_session(
    db: &DbPool,
    kv: &KVPool,
    jwt_secret: &str,
    user: &User,
    device_info: &DeviceInfo,
) -> kyomi_core::Result<AuthenticatedSession> {
    let _ = kv; // Reserved for future use (e.g., session tracking)

    let jwt_config = &kyomi_core::constants::get().jwt;

    // Load workspace context
    let workspace_ctx = user_service::get_user_workspace_context(db, &user.user_id).await?;

    // Build JWT claims
    let mut extra = std::collections::HashMap::new();
    extra.insert("user_id".into(), serde_json::json!(&user.user_id));
    extra.insert("email".into(), serde_json::json!(&user.email));
    extra.insert("name".into(), serde_json::json!(&user.name));
    extra.insert("roles".into(), serde_json::json!(user.roles()));

    let mut workspace_id = None;
    let mut workspace_name = None;
    let mut workspace_roles = vec![];

    if let Some((ws, wu)) = &workspace_ctx {
        extra.insert("workspace_id".into(), serde_json::json!(&ws.workspace_id));
        extra.insert("workspace_name".into(), serde_json::json!(&ws.name));
        extra.insert("workspace_status".into(), serde_json::json!(&ws.status));
        extra.insert("workspace_roles".into(), serde_json::json!(vec![&wu.role]));
        extra.insert("subscription_tier".into(), serde_json::json!(&ws.subscription_tier));

        workspace_id = Some(ws.workspace_id.clone());
        workspace_name = ws.name.clone();
        workspace_roles = vec![wu.role];
    }

    // Create access token
    let access_token = jwt::create_access_token_str(
        &user.user_id,
        jwt_secret,
        jwt_config.access_token_expire_minutes,
        extra,
    )?;

    // Create refresh token with a new family
    let raw_refresh = jwt::create_refresh_token();
    let token_hash = token_service::hash_refresh_token(&raw_refresh);
    let expires_at = Utc::now() + Duration::days(jwt_config.refresh_token_expire_days);
    let family_id = token_service::generate_family_id();
    token_service::store_refresh_token(db, &user.user_id, &token_hash, expires_at, device_info, &family_id)
        .await?;

    // Set cookies
    let mut cookie_headers = HeaderMap::new();
    cookies::set_token_cookies(&mut cookie_headers, Some(&access_token), Some(&raw_refresh));

    // Update last_login
    let _ = user_service::update_last_login(db, &user.user_id).await;

    // Fetch fresh user data after update
    let fresh_user = user_service::get_user_by_id(db, &user.user_id)
        .await?
        .unwrap_or_else(|| user.clone());

    Ok(AuthenticatedSession {
        access_token,
        refresh_token: raw_refresh,
        cookie_headers,
        user: fresh_user,
        workspace_id,
        workspace_name,
        workspace_roles,
    })
}

/// Switch the user's active workspace and mint a fresh session for it.
///
/// 1. Validates the caller has an ACTIVE membership in the target workspace
///    (rejects with `Forbidden` otherwise).
/// 2. Persists it as `users.last_workspace_id`.
/// 3. Re-mints the session JWT + refresh token + cookies via
///    `create_authenticated_session`, which now resolves the target workspace
///    as the active context.
pub async fn switch_active_workspace(
    db: &DbPool,
    kv: &KVPool,
    jwt_secret: &str,
    user: &User,
    target_workspace_id: &str,
    device_info: &DeviceInfo,
) -> kyomi_core::Result<AuthenticatedSession> {
    // Validate active membership. `get_workspace_user` filters `active = true`,
    // matching what `get_user_workspace_context` treats as a valid last workspace.
    user_service::get_workspace_user(db, target_workspace_id, &user.user_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Forbidden("You do not have access to this workspace".into())
        })?;

    user_service::update_last_workspace(db, &user.user_id, target_workspace_id).await?;

    create_authenticated_session(db, kv, jwt_secret, user, device_info).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    /// Build an in-memory SQLite pool with migrations applied.
    ///
    /// Mirrors the local `test_pool()` helper in `workspace_service.rs` and
    /// `workspace_ai_config.rs` — the established in-memory-sqlite pattern
    /// used across `kyomi-auth` unit tests.
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

    #[tokio::test]
    async fn switch_active_workspace_rejects_workspace_with_no_active_membership() {
        let pool = test_pool().await;

        // A user who belongs to ws-member but NOT ws-other.
        sqlx::query("INSERT INTO users (user_id, email) VALUES ('user-1', 'user@test.local')")
            .execute(match &pool {
                DbPool::Sqlite(sq) => sq,
                _ => unreachable!(),
            })
            .await
            .expect("insert user");

        sqlx::query("INSERT INTO users (user_id, email) VALUES ('owner-2', 'owner2@test.local')")
            .execute(match &pool {
                DbPool::Sqlite(sq) => sq,
                _ => unreachable!(),
            })
            .await
            .expect("insert other owner");

        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) \
             VALUES ('ws-member', 'Member Workspace', 'user-1')",
        )
        .execute(match &pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        })
        .await
        .expect("insert ws-member");

        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) \
             VALUES ('ws-other', 'Other Workspace', 'owner-2')",
        )
        .execute(match &pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        })
        .await
        .expect("insert ws-other");

        sqlx::query(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
             VALUES ('ws-member', 'user-1', 'workspace_admin', 1)",
        )
        .execute(match &pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        })
        .await
        .expect("insert membership");

        let user = user_service::get_user_by_id(&pool, "user-1")
            .await
            .expect("query user")
            .expect("user exists");

        let kv = kyomi_core::create_kv_store(None)
            .await
            .expect("in-memory kv store");

        let device = DeviceInfo {
            user_agent: None,
            ip_address: None,
            country_code: None,
            oauth_client_id: None,
        };

        let result =
            switch_active_workspace(&pool, &kv, "test-secret", &user, "ws-other", &device).await;

        match result {
            Err(kyomi_core::Error::Forbidden(msg)) => {
                assert!(msg.contains("do not have access"), "message: {msg}");
            }
            Err(other) => panic!("expected Forbidden error, got: {other:?}"),
            Ok(_) => panic!("expected Forbidden error, but switch succeeded"),
        }
    }
}
