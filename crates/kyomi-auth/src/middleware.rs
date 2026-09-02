// SPDX-License-Identifier: AGPL-3.0-or-later

//! Axum middleware for JWT-based authentication.
//!
//! Provides the `AuthUser` extractor that validates the JWT, loads the user
//! from the database, and enriches with workspace context.
//! Wire-compatible with Python's `get_current_user` dependency.

use axum::{
    extract::{FromRef, FromRequestParts},
    http::request::Parts,
};
use chrono::Utc;

use kyomi_core::enums::{SubscriptionStatus, SubscriptionTier, WorkspaceRole, WorkspaceStatus};

use crate::jwt;

/// Shared state needed by the auth extractor.
#[derive(Clone)]
pub struct AuthState {
    pub jwt_secret: String,
    pub db: kyomi_core::DbPool,
    /// When true, skip JWT validation and inject the local user context.
    pub is_personal: bool,
}

/// Workspace context enriched from the database.
#[derive(Debug, Clone)]
pub struct WorkspaceContext {
    pub workspace_id: Option<String>,
    pub workspace_name: Option<String>,
    pub workspace_roles: Vec<WorkspaceRole>,
    pub workspace_status: Option<WorkspaceStatus>,
    pub subscription_tier: SubscriptionTier,
    pub subscription_status: SubscriptionStatus,
    pub trial_ends_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_owner: bool,
}

impl Default for WorkspaceContext {
    fn default() -> Self {
        Self {
            workspace_id: None,
            workspace_name: None,
            workspace_roles: Vec::new(),
            workspace_status: None,
            subscription_tier: SubscriptionTier::Free,
            subscription_status: SubscriptionStatus::Active,
            trial_ends_at: None,
            is_owner: false,
        }
    }
}

/// Authenticated user extracted from the request.
///
/// Use as an axum extractor: `AuthUser` in handler params.
/// Rejects with 401 if the token is missing, expired, or invalid.
///
/// The `user_id` is a String (format: `"user-{token_urlsafe(16)}"`),
/// NOT a UUID — matching the Python database schema.
#[derive(Debug, Clone)]
pub struct AuthUser {
    /// User ID from the database (String, not UUID).
    pub user_id: String,
    /// User's email address.
    pub email: String,
    /// User's display name.
    pub name: Option<String>,
    /// User's roles (from extra_metadata).
    pub roles: Vec<String>,
    /// Whether the user account is active.
    pub active: bool,
    /// Whether the user's email is verified.
    pub verified: bool,
    /// Workspace context (enriched from DB).
    pub workspace: WorkspaceContext,
    /// JWT claims (for token_exp, jti access).
    pub token_exp: Option<i64>,
    pub token_jti: Option<String>,
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    AuthState: FromRef<S>,
{
    type Rejection = kyomi_core::Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth_state = AuthState::from_ref(state);

        // ── Personal mode: skip JWT, inject local user ──────────────
        if auth_state.is_personal {
            return load_personal_user(&auth_state.db).await;
        }

        // Try Authorization header first, then cookie
        let token = extract_token(parts)?;

        let token_data = jwt::validate_token(&token, &auth_state.jwt_secret)?;

        // Get user_id from claims — Python puts it in the `extra` map as "user_id"
        let user_id = token_data.claims.extra
            .get("user_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| token_data.claims.sub.clone());

        // Load user from database
        let user = crate::user_service::get_user_by_id(&auth_state.db, &user_id)
            .await
            .map_err(|e| {
                tracing::error!("database error loading user: {e}");
                kyomi_core::Error::Internal("database error".into())
            })?
            .ok_or_else(|| kyomi_core::Error::Unauthorized("User not found".into()))?;

        if !user.active {
            return Err(kyomi_core::Error::Unauthorized("User account is inactive".into()));
        }

        // Build workspace context from JWT's workspace_id claim
        let mut workspace_ctx = WorkspaceContext::default();

        let jwt_workspace_id = token_data.claims.extra
            .get("workspace_id")
            .and_then(|v| v.as_str());

        if let Some(ws_id) = jwt_workspace_id {
            // Fetch fresh workspace details from database
            match crate::user_service::get_workspace(&auth_state.db, ws_id).await {
                Ok(Some(ws)) => {
                    match crate::user_service::get_workspace_user(&auth_state.db, ws_id, &user_id).await {
                        Ok(Some(wu)) => {
                            workspace_ctx.workspace_id = Some(ws_id.to_string());
                            workspace_ctx.workspace_name = ws.name.clone();
                            workspace_ctx.workspace_roles = vec![wu.role];
                            workspace_ctx.workspace_status = Some(ws.status);
                            workspace_ctx.subscription_tier = ws.subscription_tier;
                            workspace_ctx.subscription_status = ws.subscription_status;
                            workspace_ctx.trial_ends_at = ws.trial_ends_at;
                            workspace_ctx.is_owner = ws.owner_user_id == user_id;
                        }
                        Ok(None) => {
                            // User was removed from this workspace
                            return Err(kyomi_core::Error::Unauthorized(
                                "Workspace membership revoked. Please log in again.".into()
                            ));
                        }
                        Err(e) => {
                            tracing::warn!("could not load workspace user: {e}");
                        }
                    }
                }
                Ok(None) => {
                    tracing::warn!("workspace {ws_id} not found");
                }
                Err(e) => {
                    tracing::warn!("could not load workspace: {e}");
                }
            }
        }

        // Check if token is near expiry (< 5 min) — set header via extensions
        // The actual header is set in the response layer, not here.
        // We store the expiry time for the handler to check.
        let token_exp = Some(token_data.claims.exp);
        let token_jti = token_data.claims.jti.clone();

        let roles = user.roles();
        Ok(AuthUser {
            user_id: user.user_id,
            email: user.email,
            name: user.name,
            roles,
            active: user.active,
            verified: user.verified,
            workspace: workspace_ctx,
            token_exp,
            token_jti,
        })
    }
}

impl AuthUser {
    /// Check if the access token is near expiry (< 5 minutes remaining).
    pub fn token_needs_refresh(&self) -> bool {
        if let Some(exp) = self.token_exp {
            let now = Utc::now().timestamp();
            let time_until_expiry = exp - now;
            time_until_expiry < 300 // 5 minutes in seconds
        } else {
            false
        }
    }
}

/// Extract a bearer token from the Authorization header or `access_token` cookie.
fn extract_token(parts: &Parts) -> kyomi_core::Result<String> {
    // Check Authorization: Bearer <token>
    if let Some(auth_header) = parts.headers.get("authorization") {
        let value = auth_header
            .to_str()
            .map_err(|_| kyomi_core::Error::Unauthorized("invalid auth header".into()))?;

        if let Some(token) = value.strip_prefix("Bearer ") {
            return Ok(token.to_string());
        }
    }

    // Fallback: access_token cookie (name from data/constants.toml)
    let cookie_name = &kyomi_core::constants::get().cookies.access_token_name;
    let cookie_prefix = format!("{cookie_name}=");
    if let Some(cookie_header) = parts.headers.get("cookie") {
        let cookies = cookie_header
            .to_str()
            .map_err(|_| kyomi_core::Error::Unauthorized("invalid cookie header".into()))?;

        for cookie in cookies.split(';') {
            let cookie = cookie.trim();
            if let Some(token) = cookie.strip_prefix(&cookie_prefix) {
                return Ok(token.to_string());
            }
        }
    }

    Err(kyomi_core::Error::Unauthorized(
        "Not authenticated".into(),
    ))
}

/// Load the personal-mode user and workspace context.
///
/// In personal mode there is no JWT — a single local user ("user-local") and
/// workspace ("workspace-local") are provisioned at first boot. This function
/// loads them from the database and returns a fully-populated `AuthUser`.
///
/// Returns 503 if the local user doesn't exist yet (first-boot race condition).
async fn load_personal_user(db: &kyomi_core::DbPool) -> kyomi_core::Result<AuthUser> {
    let user = crate::user_service::get_user_by_id(db, "user-local")
        .await
        .map_err(|e| {
            tracing::error!("personal mode: database error loading local user: {e}");
            kyomi_core::Error::Internal("database error".into())
        })?
        .ok_or_else(|| {
            tracing::warn!("personal mode: user-local not found — still initializing");
            kyomi_core::Error::ServiceUnavailable(
                "Personal mode initializing, please retry".into(),
            )
        })?;

    let workspace = crate::user_service::get_workspace(db, "workspace-local")
        .await
        .map_err(|e| {
            tracing::error!("personal mode: database error loading local workspace: {e}");
            kyomi_core::Error::Internal("database error".into())
        })?
        .ok_or_else(|| {
            tracing::warn!("personal mode: workspace-local not found — still initializing");
            kyomi_core::Error::ServiceUnavailable(
                "Personal mode initializing, please retry".into(),
            )
        })?;

    let workspace_ctx = WorkspaceContext {
        workspace_id: Some(workspace.workspace_id),
        workspace_name: workspace.name,
        workspace_roles: vec![WorkspaceRole::WorkspaceAdmin],
        workspace_status: Some(workspace.status),
        subscription_tier: workspace.subscription_tier,
        subscription_status: workspace.subscription_status,
        trial_ends_at: workspace.trial_ends_at,
        is_owner: true,
    };

    let roles = user.roles();
    Ok(AuthUser {
        user_id: user.user_id,
        email: user.email,
        name: user.name,
        roles,
        active: user.active,
        verified: user.verified,
        workspace: workspace_ctx,
        token_exp: None,
        token_jti: None,
    })
}

// Identity impl — when the state IS AuthState directly.
// (Axum's FromRef blanket impl handles this for types that impl Clone.)
// The AppState → AuthState impl is in kyomi-api/src/state.rs.

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use sqlx::sqlite::SqlitePoolOptions;
    use std::collections::HashMap;

    const SECRET: &str = "test-secret-key";

    /// Build an in-memory SQLite pool with migrations applied.
    ///
    /// Mirrors the established `test_pool()` helper in `session.rs`,
    /// `workspace_service.rs`, and `workspace_ai_config.rs`.
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

    async fn seed_user(pool: &kyomi_core::DbPool, user_id: &str, active: bool) {
        sqlx::query("INSERT INTO users (user_id, email, active) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind(format!("{user_id}@test.local"))
            .bind(active)
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
        active: bool,
    ) {
        sqlx::query(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(workspace_id)
        .bind(user_id)
        .bind(role)
        .bind(active)
        .execute(sqlite_of(pool))
        .await
        .expect("insert membership");
    }

    fn mint_token(user_id: &str, workspace_id: Option<&str>, expires_minutes: i64) -> String {
        let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
        extra.insert("user_id".into(), serde_json::json!(user_id));
        if let Some(ws_id) = workspace_id {
            extra.insert("workspace_id".into(), serde_json::json!(ws_id));
        }
        jwt::create_access_token_str(user_id, SECRET, expires_minutes, extra)
            .expect("mint test token")
    }

    fn parts_with_bearer(token: &str) -> Parts {
        let request = axum::http::Request::builder()
            .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
            .body(())
            .expect("build request");
        request.into_parts().0
    }

    fn parts_with_no_auth() -> Parts {
        axum::http::Request::builder()
            .body(())
            .expect("build request")
            .into_parts()
            .0
    }

    fn auth_state(pool: &kyomi_core::DbPool, is_personal: bool) -> AuthState {
        AuthState {
            jwt_secret: SECRET.to_string(),
            db: pool.clone(),
            is_personal,
        }
    }

    // ── Case 1: valid token + active user + active membership ──────────────

    #[tokio::test]
    async fn valid_token_active_user_active_membership_yields_authuser() {
        let pool = test_pool().await;
        seed_user(&pool, "user-1", true).await;
        seed_workspace(&pool, "ws-1", "user-1").await;
        seed_membership(&pool, "ws-1", "user-1", "workspace_admin", true).await;

        let token = mint_token("user-1", Some("ws-1"), 15);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let auth_user = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect("should authenticate");

        assert_eq!(auth_user.user_id, "user-1");
        assert_eq!(auth_user.workspace.workspace_id.as_deref(), Some("ws-1"));
        assert_eq!(
            auth_user.workspace.workspace_roles,
            vec![WorkspaceRole::WorkspaceAdmin]
        );
        assert!(auth_user.workspace.is_owner, "user-1 owns ws-1");
    }

    // ── Case 2: missing / malformed / expired token ─────────────────────────

    #[tokio::test]
    async fn missing_token_is_unauthorized() {
        let pool = test_pool().await;
        let mut parts = parts_with_no_auth();
        let state = auth_state(&pool, false);

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect_err("no token must reject");
        assert!(matches!(err, kyomi_core::Error::Unauthorized(_)));
    }

    #[tokio::test]
    async fn malformed_token_is_unauthorized() {
        let pool = test_pool().await;
        let mut parts = parts_with_bearer("not-a-real-jwt");
        let state = auth_state(&pool, false);

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect_err("malformed token must reject");
        assert!(matches!(err, kyomi_core::Error::Unauthorized(_)));
    }

    #[tokio::test]
    async fn expired_token_is_unauthorized() {
        let pool = test_pool().await;
        seed_user(&pool, "user-1", true).await;

        // Expired 5 minutes ago — well past jsonwebtoken's default leeway.
        let token = mint_token("user-1", None, -5);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect_err("expired token must reject");
        assert!(matches!(err, kyomi_core::Error::Unauthorized(_)));
        assert!(
            err.to_string().contains("token expired"),
            "expected token-expired message, got: {err}"
        );
    }

    #[test]
    fn extract_token_reads_bearer_authorization_header() {
        let parts = parts_with_bearer("abc.def.ghi");
        let token = extract_token(&parts).expect("bearer token present");
        assert_eq!(token, "abc.def.ghi");
    }

    #[test]
    fn extract_token_falls_back_to_access_token_cookie() {
        let _ = kyomi_core::constants::load_with_fallback();

        let request = axum::http::Request::builder()
            .header(
                axum::http::header::COOKIE,
                "other_cookie=xyz; access_token=cookie-token-value; another=1",
            )
            .body(())
            .expect("build request");
        let (parts, _) = request.into_parts();

        let token = extract_token(&parts).expect("cookie token present");
        assert_eq!(token, "cookie-token-value");
    }

    #[test]
    fn extract_token_rejects_when_neither_header_nor_cookie_present() {
        let _ = kyomi_core::constants::load_with_fallback();

        let parts = parts_with_no_auth();
        let err = extract_token(&parts).expect_err("neither present must reject");
        match err {
            kyomi_core::Error::Unauthorized(msg) => {
                assert!(msg.contains("Not authenticated"), "message: {msg}");
            }
            other => panic!("expected Unauthorized, got: {other:?}"),
        }
    }

    #[test]
    fn extract_token_rejects_malformed_authorization_value() {
        // A header value that is not valid UTF-8/visible-ASCII fails `to_str()`.
        let mut parts = parts_with_no_auth();
        let invalid = HeaderValue::from_bytes(&[0xC0, 0xC1, 0xFE, 0xFF]).expect("raw bytes header");
        parts
            .headers
            .insert(axum::http::header::AUTHORIZATION, invalid);

        let err = extract_token(&parts).expect_err("invalid header bytes must reject");
        match err {
            kyomi_core::Error::Unauthorized(msg) => {
                assert!(msg.contains("invalid auth header"), "message: {msg}");
            }
            other => panic!("expected Unauthorized, got: {other:?}"),
        }
    }

    #[test]
    fn extract_token_ignores_non_bearer_authorization_and_falls_through() {
        let _ = kyomi_core::constants::load_with_fallback();

        // A non-"Bearer " Authorization header (e.g. Basic auth) is not treated
        // as a bearer token; without a cookie fallback this must reject.
        let request = axum::http::Request::builder()
            .header(axum::http::header::AUTHORIZATION, "Basic dXNlcjpwYXNz")
            .body(())
            .expect("build request");
        let (parts, _) = request.into_parts();

        let err = extract_token(&parts).expect_err("non-bearer header must not match");
        assert!(matches!(err, kyomi_core::Error::Unauthorized(_)));
    }

    // ── Case 3: inactive user account ────────────────────────────────────

    #[tokio::test]
    async fn inactive_user_is_unauthorized() {
        let pool = test_pool().await;
        seed_user(&pool, "user-1", false).await;

        let token = mint_token("user-1", None, 15);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect_err("inactive user must reject");
        match err {
            kyomi_core::Error::Unauthorized(msg) => {
                assert!(msg.contains("inactive"), "message: {msg}");
            }
            other => panic!("expected Unauthorized, got: {other:?}"),
        }
    }

    // ── Case 4: workspace membership revoked (security-critical) ───────────

    #[tokio::test]
    async fn revoked_workspace_membership_is_unauthorized() {
        let pool = test_pool().await;
        seed_user(&pool, "user-1", true).await;
        seed_user(&pool, "owner-2", true).await;
        seed_workspace(&pool, "ws-1", "owner-2").await;
        // user-1 was a member once, but the membership row is now inactive —
        // simulating removal from the workspace after the JWT was issued.
        seed_membership(&pool, "ws-1", "user-1", "workspace_user", false).await;

        let token = mint_token("user-1", Some("ws-1"), 15);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect_err("revoked membership must reject");
        match err {
            kyomi_core::Error::Unauthorized(msg) => {
                assert!(
                    msg.contains("Workspace membership revoked"),
                    "message: {msg}"
                );
            }
            other => panic!("expected Unauthorized, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_workspace_membership_row_at_all_is_unauthorized() {
        let pool = test_pool().await;
        seed_user(&pool, "user-1", true).await;
        seed_user(&pool, "owner-2", true).await;
        seed_workspace(&pool, "ws-1", "owner-2").await;
        // No workspace_users row for user-1 in ws-1 whatsoever.

        let token = mint_token("user-1", Some("ws-1"), 15);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect_err("missing membership must reject");
        match err {
            kyomi_core::Error::Unauthorized(msg) => {
                assert!(
                    msg.contains("Workspace membership revoked"),
                    "message: {msg}"
                );
            }
            other => panic!("expected Unauthorized, got: {other:?}"),
        }
    }

    // ── Case 5: is_owner only true for the workspace's actual owner ────────

    #[tokio::test]
    async fn is_owner_false_for_admin_who_is_not_the_workspace_owner() {
        let pool = test_pool().await;
        seed_user(&pool, "owner-1", true).await;
        seed_user(&pool, "admin-2", true).await;
        seed_workspace(&pool, "ws-1", "owner-1").await;
        // admin-2 has the admin role but did not create/own the workspace.
        seed_membership(&pool, "ws-1", "admin-2", "workspace_admin", true).await;

        let token = mint_token("admin-2", Some("ws-1"), 15);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let auth_user = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect("admin-2 has an active membership");

        assert_eq!(
            auth_user.workspace.workspace_roles,
            vec![WorkspaceRole::WorkspaceAdmin],
            "admin-2 is still a workspace_admin"
        );
        assert!(
            !auth_user.workspace.is_owner,
            "is_owner must be false — admin-2 did not create ws-1, owner-1 did"
        );
    }

    #[tokio::test]
    async fn is_owner_true_for_the_actual_workspace_owner() {
        let pool = test_pool().await;
        seed_user(&pool, "owner-1", true).await;
        seed_workspace(&pool, "ws-1", "owner-1").await;
        seed_membership(&pool, "ws-1", "owner-1", "workspace_admin", true).await;

        let token = mint_token("owner-1", Some("ws-1"), 15);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let auth_user = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect("owner-1 has an active membership");

        assert!(auth_user.workspace.is_owner);
    }

    // ── Case 6: workspace_roles mirrors the membership row exactly ─────────

    #[tokio::test]
    async fn workspace_roles_reflects_workspace_admin_membership_row() {
        let pool = test_pool().await;
        seed_user(&pool, "owner-1", true).await;
        seed_workspace(&pool, "ws-1", "owner-1").await;
        seed_membership(&pool, "ws-1", "owner-1", "workspace_admin", true).await;

        let token = mint_token("owner-1", Some("ws-1"), 15);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let auth_user = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect("owner-1 has an active membership");
        assert_eq!(
            auth_user.workspace.workspace_roles,
            vec![WorkspaceRole::WorkspaceAdmin]
        );
    }

    #[tokio::test]
    async fn workspace_roles_reflects_workspace_user_membership_row() {
        let pool = test_pool().await;
        seed_user(&pool, "owner-1", true).await;
        seed_user(&pool, "user-2", true).await;
        seed_workspace(&pool, "ws-1", "owner-1").await;
        seed_membership(&pool, "ws-1", "user-2", "workspace_user", true).await;

        let token = mint_token("user-2", Some("ws-1"), 15);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let auth_user = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect("user-2 has an active membership");
        assert_eq!(
            auth_user.workspace.workspace_roles,
            vec![WorkspaceRole::WorkspaceUser]
        );
        assert!(!auth_user.workspace.is_owner);
    }

    // ── Bonus: personal mode (`load_personal_user`) ─────────────────────────

    #[tokio::test]
    async fn personal_mode_returns_owner_admin_for_local_user_and_workspace() {
        let pool = test_pool().await;
        seed_user(&pool, "user-local", true).await;
        seed_workspace(&pool, "workspace-local", "user-local").await;
        seed_membership(
            &pool,
            "workspace-local",
            "user-local",
            "workspace_admin",
            true,
        )
        .await;

        // Personal mode skips JWT entirely — no Authorization header needed.
        let mut parts = parts_with_no_auth();
        let state = auth_state(&pool, true);

        let auth_user = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect("personal mode should succeed once local user/workspace exist");

        assert_eq!(auth_user.user_id, "user-local");
        assert!(auth_user.workspace.is_owner);
        assert_eq!(
            auth_user.workspace.workspace_roles,
            vec![WorkspaceRole::WorkspaceAdmin]
        );
    }

    #[tokio::test]
    async fn personal_mode_without_local_user_is_service_unavailable() {
        let pool = test_pool().await;
        // Neither user-local nor workspace-local exist yet (first-boot race).

        let mut parts = parts_with_no_auth();
        let state = auth_state(&pool, true);

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect_err("missing local user must reject");
        assert!(matches!(err, kyomi_core::Error::ServiceUnavailable(_)));
    }

    #[tokio::test]
    async fn personal_mode_without_local_workspace_is_service_unavailable() {
        let pool = test_pool().await;
        seed_user(&pool, "user-local", true).await;
        // No workspace-local row.

        let mut parts = parts_with_no_auth();
        let state = auth_state(&pool, true);

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect_err("missing local workspace must reject");
        assert!(matches!(err, kyomi_core::Error::ServiceUnavailable(_)));
    }
}
