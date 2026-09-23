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
    /// Mirrors `kyomi_core::Config::self_hosted` — `true` for **both**
    /// `KyomiMode::SelfHosted` and `KyomiMode::Personal` (see that field's
    /// doc comment). Threaded explicitly into `AuthState` rather than read
    /// from an env var here, so the billing gate below (and anything else
    /// that needs to know "does this deployment enforce billing") has one
    /// source of truth (KYO-805) — see
    /// `kyomi_core::capability::billing_gate_blocks`.
    pub self_hosted: bool,
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
    /// Whether this workspace must pay before continuing to use the app,
    /// per `kyomi_core::capability::billing_gate_blocks` — computed exactly
    /// once, here, from the freshly-loaded `Workspace` row and the
    /// deployment's `self_hosted` flag (KYO-805). Every other consumer
    /// (`get_sidebar_user`, the WS sync handlers) reads this field rather
    /// than re-deriving it, so there is one computation site. Always
    /// `false` when there is no resolved workspace (`workspace_id` is
    /// `None`) — there is nothing to be lapsed on.
    pub billing_lapsed: bool,
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
            billing_lapsed: false,
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

/// Load and fully populate an [`AuthUser`] — JWT validation, user lookup,
/// membership lookup, and the billing-gate verdict on
/// [`WorkspaceContext::billing_lapsed`] — **without** enforcing that gate.
///
/// This is the one code path both [`AuthUser`]'s and [`AuthUserAllowLapsed`]'s
/// `FromRequestParts` impls call (KYO-805): everything through "is this a
/// valid, active, still-a-member request" is identical for both, so there is
/// exactly one place that can load a user incorrectly. The two extractors
/// differ only in what they do with `billing_lapsed` once it's computed —
/// see [`AuthUser`]'s impl below.
async fn load_auth_user<S>(parts: &mut Parts, state: &S) -> kyomi_core::Result<AuthUser>
where
    S: Send + Sync,
    AuthState: FromRef<S>,
{
    let auth_state = AuthState::from_ref(state);

    // ── Personal mode: skip JWT, inject local user ──────────────
    if auth_state.is_personal {
        return load_personal_user(&auth_state.db, auth_state.self_hosted).await;
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
                        workspace_ctx.billing_lapsed = kyomi_core::capability::billing_gate_blocks(
                            &ws,
                            auth_state.self_hosted,
                            Utc::now(),
                        );
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
                        // Fail closed (KYO-805): a DB error here previously
                        // fell through to the default (Active, not lapsed)
                        // WorkspaceContext, which — once a billing gate reads
                        // that context — would silently let a request
                        // through a database outage should have blocked.
                        tracing::error!("database error loading workspace membership: {e}");
                        return Err(kyomi_core::Error::Internal("database error".into()));
                    }
                }
            }
            Ok(None) => {
                // Workspace genuinely doesn't exist — not a DB failure, and
                // not the fail-closed case above. Left exactly as before:
                // the caller proceeds with the default WorkspaceContext.
                tracing::warn!("workspace {ws_id} not found");
            }
            Err(e) => {
                // Same fail-closed reasoning as the membership lookup above.
                tracing::error!("database error loading workspace: {e}");
                return Err(kyomi_core::Error::Internal("database error".into()));
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

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    AuthState: FromRef<S>,
{
    type Rejection = kyomi_core::Error;

    /// Fails closed on a lapsed SaaS workspace (KYO-805): every handler that
    /// takes a bare `AuthUser` — including a brand-new one nobody has
    /// thought about billing for yet — is gated by default. An endpoint that
    /// must keep working while billing is lapsed (login/logout, the billing
    /// settings themselves, ...) opts out explicitly by taking
    /// [`AuthUserAllowLapsed`] instead; there is no opt-*in* list of gated
    /// routes to forget an entry in.
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = load_auth_user(parts, state).await?;
        if user.workspace.billing_lapsed {
            return Err(kyomi_core::Error::PaymentRequired(
                "This workspace's billing is past due.".into(),
            ));
        }
        Ok(user)
    }
}

/// The explicit opt-out from [`AuthUser`]'s billing gate.
///
/// Wraps the same fully-loaded [`AuthUser`] — via [`load_auth_user`], the one
/// shared loading path — but never rejects on `billing_lapsed`. Use this only
/// for the small, named set of endpoints the KYO-805 ticket allowlists (login/
/// logout/session refresh, the billing settings surfaces themselves,
/// workspace-switching, and user/sidebar context); everything else should
/// take a bare [`AuthUser`] and get the gate for free.
#[derive(Debug, Clone)]
pub struct AuthUserAllowLapsed(pub AuthUser);

impl std::ops::Deref for AuthUserAllowLapsed {
    type Target = AuthUser;
    fn deref(&self) -> &AuthUser {
        &self.0
    }
}

impl AuthUserAllowLapsed {
    /// Unwrap into the inner [`AuthUser`].
    pub fn into_inner(self) -> AuthUser {
        self.0
    }
}

impl<S> FromRequestParts<S> for AuthUserAllowLapsed
where
    S: Send + Sync,
    AuthState: FromRef<S>,
{
    type Rejection = kyomi_core::Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Ok(AuthUserAllowLapsed(load_auth_user(parts, state).await?))
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
async fn load_personal_user(db: &kyomi_core::DbPool, self_hosted: bool) -> kyomi_core::Result<AuthUser> {
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

    // Personal mode is always self_hosted (see AuthState::self_hosted's doc
    // comment), so this is always `false` in practice — routed through the
    // same shared predicate as every other caller rather than hardcoded,
    // so there's one place that could ever disagree (KYO-805).
    let billing_lapsed =
        kyomi_core::capability::billing_gate_blocks(&workspace, self_hosted, Utc::now());

    let workspace_ctx = WorkspaceContext {
        workspace_id: Some(workspace.workspace_id),
        workspace_name: workspace.name,
        workspace_roles: vec![WorkspaceRole::WorkspaceAdmin],
        workspace_status: Some(workspace.status),
        subscription_tier: workspace.subscription_tier,
        subscription_status: workspace.subscription_status,
        trial_ends_at: workspace.trial_ends_at,
        is_owner: true,
        billing_lapsed,
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
    use std::collections::HashMap;

    use crate::test_support::{seed_membership, seed_user_with_active, seed_workspace, sqlite_pool, test_pool};

    const SECRET: &str = "test-secret-key";

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
            self_hosted: false,
        }
    }

    // ── Case 1: valid token + active user + active membership ──────────────

    #[tokio::test]
    async fn valid_token_active_user_active_membership_yields_authuser() {
        let pool = test_pool().await;
        seed_user_with_active(sqlite_pool(&pool), "user-1", "user-1@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "ws-1", "user-1").await;
        seed_membership(sqlite_pool(&pool), "ws-1", "user-1", "workspace_admin", true).await;

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
        seed_user_with_active(sqlite_pool(&pool), "user-1", "user-1@test.local", true).await;

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
        seed_user_with_active(sqlite_pool(&pool), "user-1", "user-1@test.local", false).await;

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
        seed_user_with_active(sqlite_pool(&pool), "user-1", "user-1@test.local", true).await;
        seed_user_with_active(sqlite_pool(&pool), "owner-2", "owner-2@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "ws-1", "owner-2").await;
        // user-1 was a member once, but the membership row is now inactive —
        // simulating removal from the workspace after the JWT was issued.
        seed_membership(sqlite_pool(&pool), "ws-1", "user-1", "workspace_user", false).await;

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
        seed_user_with_active(sqlite_pool(&pool), "user-1", "user-1@test.local", true).await;
        seed_user_with_active(sqlite_pool(&pool), "owner-2", "owner-2@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "ws-1", "owner-2").await;
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
        seed_user_with_active(sqlite_pool(&pool), "owner-1", "owner-1@test.local", true).await;
        seed_user_with_active(sqlite_pool(&pool), "admin-2", "admin-2@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "ws-1", "owner-1").await;
        // admin-2 has the admin role but did not create/own the workspace.
        seed_membership(sqlite_pool(&pool), "ws-1", "admin-2", "workspace_admin", true).await;

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
        seed_user_with_active(sqlite_pool(&pool), "owner-1", "owner-1@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "ws-1", "owner-1").await;
        seed_membership(sqlite_pool(&pool), "ws-1", "owner-1", "workspace_admin", true).await;

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
        seed_user_with_active(sqlite_pool(&pool), "owner-1", "owner-1@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "ws-1", "owner-1").await;
        seed_membership(sqlite_pool(&pool), "ws-1", "owner-1", "workspace_admin", true).await;

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
        seed_user_with_active(sqlite_pool(&pool), "owner-1", "owner-1@test.local", true).await;
        seed_user_with_active(sqlite_pool(&pool), "user-2", "user-2@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "ws-1", "owner-1").await;
        seed_membership(sqlite_pool(&pool), "ws-1", "user-2", "workspace_user", true).await;

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
        seed_user_with_active(sqlite_pool(&pool), "user-local", "user-local@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "workspace-local", "user-local").await;
        seed_membership(
            sqlite_pool(&pool),
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
        seed_user_with_active(sqlite_pool(&pool), "user-local", "user-local@test.local", true).await;
        // No workspace-local row.

        let mut parts = parts_with_no_auth();
        let state = auth_state(&pool, true);

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect_err("missing local workspace must reject");
        assert!(matches!(err, kyomi_core::Error::ServiceUnavailable(_)));
    }

    // ── KYO-805: billing gate ────────────────────────────────────────────

    fn self_hosted_state(pool: &kyomi_core::DbPool) -> AuthState {
        AuthState {
            self_hosted: true,
            ..auth_state(pool, false)
        }
    }

    async fn set_subscription_status(sq: &sqlx::SqlitePool, workspace_id: &str, status: &str) {
        sqlx::query("UPDATE workspaces SET subscription_status = $1 WHERE workspace_id = $2")
            .bind(status)
            .bind(workspace_id)
            .execute(sq)
            .await
            .expect("update subscription_status");
    }

    #[tokio::test]
    async fn lapsed_saas_workspace_is_rejected_with_payment_required() {
        let pool = test_pool().await;
        seed_user_with_active(sqlite_pool(&pool), "user-1", "user-1@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "ws-1", "user-1").await;
        seed_membership(sqlite_pool(&pool), "ws-1", "user-1", "workspace_admin", true).await;
        set_subscription_status(sqlite_pool(&pool), "ws-1", "past_due").await;

        let token = mint_token("user-1", Some("ws-1"), 15);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect_err("a lapsed SaaS workspace must be rejected by the default AuthUser extractor");
        assert!(
            matches!(err, kyomi_core::Error::PaymentRequired(_)),
            "expected PaymentRequired, got {err:?}"
        );
    }

    #[tokio::test]
    async fn active_saas_workspace_is_not_gated() {
        let pool = test_pool().await;
        seed_user_with_active(sqlite_pool(&pool), "user-1", "user-1@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "ws-1", "user-1").await;
        seed_membership(sqlite_pool(&pool), "ws-1", "user-1", "workspace_admin", true).await;
        // seed_workspace's default subscription_status is 'active' (schema default) —
        // no explicit set_subscription_status call needed.

        let token = mint_token("user-1", Some("ws-1"), 15);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let auth_user = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect("an active SaaS workspace must not be gated");
        assert!(!auth_user.workspace.billing_lapsed);
    }

    #[tokio::test]
    async fn self_hosted_mode_is_never_gated_even_with_past_due_workspace() {
        let pool = test_pool().await;
        seed_user_with_active(sqlite_pool(&pool), "user-1", "user-1@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "ws-1", "user-1").await;
        seed_membership(sqlite_pool(&pool), "ws-1", "user-1", "workspace_admin", true).await;
        set_subscription_status(sqlite_pool(&pool), "ws-1", "past_due").await;

        let token = mint_token("user-1", Some("ws-1"), 15);
        let mut parts = parts_with_bearer(&token);
        let state = self_hosted_state(&pool);

        let auth_user = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect("self-hosted/personal mode must never be gated on billing, even with a past_due row");
        assert!(!auth_user.workspace.billing_lapsed);
    }

    #[tokio::test]
    async fn allow_lapsed_extractor_succeeds_on_a_lapsed_workspace_and_flags_it() {
        let pool = test_pool().await;
        seed_user_with_active(sqlite_pool(&pool), "user-1", "user-1@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "ws-1", "user-1").await;
        seed_membership(sqlite_pool(&pool), "ws-1", "user-1", "workspace_admin", true).await;
        set_subscription_status(sqlite_pool(&pool), "ws-1", "past_due").await;

        let token = mint_token("user-1", Some("ws-1"), 15);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let wrapped = AuthUserAllowLapsed::from_request_parts(&mut parts, &state)
            .await
            .expect("AuthUserAllowLapsed must succeed on a lapsed workspace — that's the opt-out's entire point");
        assert!(
            wrapped.workspace.billing_lapsed,
            "the allow-lapsed extractor must still report billing_lapsed=true so callers can branch on it"
        );
        assert_eq!(wrapped.user_id, "user-1");
    }

    #[tokio::test]
    async fn db_error_loading_workspace_fails_closed() {
        let pool = test_pool().await;
        seed_user_with_active(sqlite_pool(&pool), "user-1", "user-1@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "ws-1", "user-1").await;
        seed_membership(sqlite_pool(&pool), "ws-1", "user-1", "workspace_admin", true).await;
        // Corrupt subscription_status so `SELECT * FROM workspaces` fails to
        // decode — the cheapest way to force a genuine DB/decode error out
        // of `user_service::get_workspace` without dropping a table out
        // from under a live pool. KYO-805: this must now reject the
        // request (Internal), not silently continue with a default
        // (Active, not-lapsed) WorkspaceContext — that would be a
        // fail-open hole in the billing gate this same change adds.
        set_subscription_status(sqlite_pool(&pool), "ws-1", "not-a-real-status").await;

        let token = mint_token("user-1", Some("ws-1"), 15);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect_err("a DB/decode error loading the workspace must fail closed");
        assert!(matches!(err, kyomi_core::Error::Internal(_)), "expected Internal, got {err:?}");
    }

    #[tokio::test]
    async fn db_error_loading_workspace_membership_fails_closed() {
        let pool = test_pool().await;
        seed_user_with_active(sqlite_pool(&pool), "user-1", "user-1@test.local", true).await;
        seed_workspace(sqlite_pool(&pool), "ws-1", "user-1").await;
        seed_membership(sqlite_pool(&pool), "ws-1", "user-1", "workspace_admin", true).await;
        // Corrupt the membership row's role enum so decoding
        // `workspace_users` fails — same technique as the workspace-load
        // test above, applied to the second lookup in the same function.
        sqlx::query("UPDATE workspace_users SET role = 'not-a-real-role' WHERE workspace_id = 'ws-1' AND user_id = 'user-1'")
            .execute(sqlite_pool(&pool))
            .await
            .expect("corrupt role");

        let token = mint_token("user-1", Some("ws-1"), 15);
        let mut parts = parts_with_bearer(&token);
        let state = auth_state(&pool, false);

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect_err("a DB/decode error loading workspace membership must fail closed");
        assert!(matches!(err, kyomi_core::Error::Internal(_)), "expected Internal, got {err:?}");
    }
}
