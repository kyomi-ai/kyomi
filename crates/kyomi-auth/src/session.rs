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
