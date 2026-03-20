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

use kyomi_core::enums::{SubscriptionTier, WorkspaceRole, WorkspaceStatus};

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

    // Fallback: access_token cookie (name from shared/constants.toml)
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
