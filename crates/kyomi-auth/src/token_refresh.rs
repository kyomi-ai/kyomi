// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared refresh-token flow used by both the explicit `POST /auth/refresh`
//! REST endpoint and the transparent auto-refresh middleware.
//!
//! This module encapsulates the pure token-minting + rotation logic so that
//! callers can wrap it with whatever HTTP concerns they need (rate limiting,
//! cookie setting, JSON body assembly) without duplicating the core flow.

use std::collections::HashMap;

use crate::{
    jwt, token_service,
    token_service::{DeviceInfo, RefreshTokenVerifyResult},
    user_service,
};

/// Result of a successful refresh: new tokens + user context for downstream
/// response assembly (cookies, JSON body, etc.).
pub struct RefreshedTokens {
    pub access_token: String,
    pub raw_refresh_token: String,
    pub access_expires_in_secs: i64,
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub roles: Vec<String>,
}

/// Attempt to mint a new access token and rotate the refresh token.
///
/// Handles grace-period and theft detection via
/// [`token_service::verify_refresh_token`]. Returns `Err` on verification
/// failure, theft detection, or database errors.
///
/// Does NOT perform rate limiting — callers layer that on top if they want it.
/// Does NOT set cookies — callers assemble the HTTP response themselves.
pub async fn refresh_tokens(
    db: &kyomi_core::DbPool,
    jwt_secret: &str,
    refresh_token_value: &str,
    device: &DeviceInfo,
) -> kyomi_core::Result<RefreshedTokens> {
    // Verify refresh token (handles grace period + theft detection)
    let verify_result = token_service::verify_refresh_token(db, refresh_token_value).await?;

    let user_data = match verify_result {
        RefreshTokenVerifyResult::Valid(data) | RefreshTokenVerifyResult::GracePeriod(data) => data,
        RefreshTokenVerifyResult::TheftDetected { .. } => {
            return Err(kyomi_core::Error::Unauthorized(
                "Refresh token has been revoked (possible token theft detected)".into(),
            ));
        }
        RefreshTokenVerifyResult::Invalid => {
            return Err(kyomi_core::Error::Unauthorized(
                "Invalid or expired refresh token".into(),
            ));
        }
    };

    // Build JWT claims — mirrors POST /auth/refresh
    let mut extra = HashMap::new();
    extra.insert("user_id".into(), serde_json::json!(user_data.user_id));
    extra.insert("email".into(), serde_json::json!(user_data.email));
    extra.insert("name".into(), serde_json::json!(user_data.name));
    extra.insert("roles".into(), serde_json::json!(user_data.roles));

    // Load workspace context (same behaviour as the REST handler: best-effort)
    if let Ok(Some((ws, wu))) =
        user_service::get_user_workspace_context(db, &user_data.user_id).await
    {
        extra.insert("workspace_id".into(), serde_json::json!(ws.workspace_id));
        extra.insert("workspace_roles".into(), serde_json::json!(vec![wu.role]));
    }

    let jwt_config = &kyomi_core::constants::get().jwt;
    let new_access_token = jwt::create_access_token_str(
        &user_data.user_id,
        jwt_secret,
        jwt_config.access_token_expire_minutes,
        extra,
    )?;

    // Always rotate: every tab gets a fresh token (prevents multi-tab sign-out bug)
    let new_raw_refresh = jwt::create_refresh_token();
    let new_token_hash = token_service::hash_refresh_token(&new_raw_refresh);
    let expires_at =
        chrono::Utc::now() + chrono::Duration::days(jwt_config.refresh_token_expire_days);

    token_service::rotate_refresh_token(
        db,
        &user_data.token_id,
        &user_data.user_id,
        &user_data.family_id,
        &new_token_hash,
        expires_at,
        device,
    )
    .await?;

    Ok(RefreshedTokens {
        access_token: new_access_token,
        raw_refresh_token: new_raw_refresh,
        access_expires_in_secs: jwt_config.access_token_expire_minutes * 60,
        user_id: user_data.user_id,
        email: user_data.email,
        name: user_data.name,
        roles: user_data.roles,
    })
}
