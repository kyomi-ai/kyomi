// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for Security settings.
//!
//! These replace the REST API calls for password and TOTP management:
//! - `POST /auth/set-password` -> `set_password()`
//! - `POST /auth/change-password` -> `change_password()`
//! - `GET  /auth/2fa/status` -> `get_totp_status()`
//! - `POST /auth/2fa/setup` -> `setup_totp()`
//! - `POST /auth/2fa/enable` -> `enable_totp()`
//! - `POST /auth/2fa/disable` -> `disable_totp()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/auth_password.rs`
//! and `apps/server/src/routes/auth_totp.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context};

/// TOTP status returned by `get_totp_status()`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TotpStatus {
    pub enabled: bool,
}

/// TOTP setup data returned by `setup_totp()`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TotpSetup {
    pub secret: String,
    pub qr_uri: String,
}

/// Check whether the current user has a password set.
#[server(prefix = "/leptos-api")]
pub async fn has_password() -> Result<bool, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    kyomi_auth::user_service::has_password(&ctx.db, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Set a password for a user who does not yet have one (e.g. OAuth-only users).
///
/// Mirrors the validation in `apps/server/src/routes/auth_password.rs::set_password`:
/// - Password must be at least 8 characters.
/// - User must NOT already have a password.
#[server(prefix = "/leptos-api")]
pub async fn set_password(new_password: String) -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    // Validate password length
    if new_password.len() < 8 {
        return Err(ServerFnError::new(
            "Password must be at least 8 characters",
        ));
    }

    // Check user does NOT already have a password
    let has_pw = kyomi_auth::user_service::has_password(&ctx.db, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if has_pw {
        return Err(ServerFnError::new(
            "Password already set. Use change-password to update it.",
        ));
    }

    // Hash and store
    let hash = kyomi_auth::password::hash_password(&new_password)
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let auth_data = serde_json::json!({"hash": hash});
    kyomi_auth::user_service::upsert_auth_method(&ctx.db, &auth.user_id, "password", &auth_data)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok("Password set successfully".to_string())
}

/// Change password for a user who already has one.
///
/// Mirrors the validation in `apps/server/src/routes/auth_password.rs::change_password`:
/// - New password must be at least 8 characters.
/// - Current password must be verified first.
#[server(prefix = "/leptos-api")]
pub async fn change_password(
    current_password: String,
    new_password: String,
) -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    // Validate new password length
    if new_password.len() < 8 {
        return Err(ServerFnError::new(
            "New password must be at least 8 characters",
        ));
    }

    // Get existing password auth method
    let password_method =
        kyomi_auth::user_service::get_auth_method(&ctx.db, &auth.user_id, "password")
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    let Some(password_method) = password_method else {
        return Err(ServerFnError::new(
            "No password set. Use set-password to create one.",
        ));
    };

    // Extract and verify current password
    let Some(hash) = password_method
        .auth_data
        .get("hash")
        .and_then(|v| v.as_str())
    else {
        return Err(ServerFnError::new("Password auth method corrupted"));
    };

    let valid = kyomi_auth::password::verify_password(&current_password, hash)
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if !valid {
        return Err(ServerFnError::new("Current password is incorrect"));
    }

    // Hash new password and upsert
    let new_hash = kyomi_auth::password::hash_password(&new_password)
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let auth_data = serde_json::json!({"hash": new_hash});
    kyomi_auth::user_service::upsert_auth_method(&ctx.db, &auth.user_id, "password", &auth_data)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok("Password changed successfully".to_string())
}

// ---------------------------------------------------------------------------
// TOTP 2FA server functions
// ---------------------------------------------------------------------------

/// Check whether the current user has TOTP 2FA enabled.
///
/// Mirrors `GET /auth/2fa/status` in `apps/server/src/routes/auth_totp.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_totp_status() -> Result<TotpStatus, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let enabled = kyomi_auth::user_service::has_totp_enabled(&ctx.db, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(TotpStatus { enabled })
}

/// Begin 2FA setup: generate a secret and QR code data URI.
///
/// The secret is stored in Redis (10 min TTL) until the user confirms with
/// a verification code via `enable_totp()`.
///
/// Mirrors `POST /auth/2fa/setup` in `apps/server/src/routes/auth_totp.rs`.
#[server(prefix = "/leptos-api")]
pub async fn setup_totp() -> Result<TotpSetup, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    // Check not already enabled
    let enabled = kyomi_auth::user_service::has_totp_enabled(&ctx.db, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if enabled {
        return Err(ServerFnError::new("2FA is already enabled"));
    }

    let secret = kyomi_auth::totp::generate_secret();
    let qr_uri = kyomi_auth::totp::generate_qr_code(&secret, &auth.email)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Store pending secret in Redis (10 min TTL)
    let kv = ctx
        .kv
        .as_ref()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;
    kyomi_auth::redis_ops::store_pending_totp(kv, &auth.user_id, &secret)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(TotpSetup { secret, qr_uri })
}

/// Confirm 2FA setup by verifying a TOTP code against the pending secret.
///
/// On success the secret is persisted in `user_auth_methods`. On failure the
/// pending secret is re-stored in Redis so the user can retry.
///
/// Mirrors `POST /auth/2fa/enable` in `apps/server/src/routes/auth_totp.rs`.
#[server(prefix = "/leptos-api")]
pub async fn enable_totp(code: String) -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let kv = ctx
        .kv
        .as_ref()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Get pending secret from Redis (atomic get+delete)
    let secret = kyomi_auth::redis_ops::get_pending_totp(kv, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let Some(secret) = secret else {
        return Err(ServerFnError::new(
            "No pending 2FA setup found. Please start the setup process again.",
        ));
    };

    // Verify the code
    if !kyomi_auth::totp::verify_code(&secret, &code) {
        // Re-store secret so user can retry
        kyomi_auth::redis_ops::store_pending_totp(kv, &auth.user_id, &secret)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        return Err(ServerFnError::new(
            "Invalid verification code. Please try again.",
        ));
    }

    // Persist TOTP auth method
    let now = chrono::Utc::now().to_rfc3339();
    let auth_data = serde_json::json!({
        "secret": secret,
        "enabled_at": now,
    });
    kyomi_auth::user_service::upsert_auth_method(&ctx.db, &auth.user_id, "totp", &auth_data)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok("2FA has been successfully enabled".to_string())
}

/// Disable 2FA for the authenticated user.
///
/// Mirrors `POST /auth/2fa/disable` in `apps/server/src/routes/auth_totp.rs`.
/// The REST handler does not require a TOTP code — it simply removes the auth
/// method for the already-authenticated user.
#[server(prefix = "/leptos-api")]
pub async fn disable_totp() -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let enabled = kyomi_auth::user_service::has_totp_enabled(&ctx.db, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if !enabled {
        return Err(ServerFnError::new("2FA is not currently enabled"));
    }

    kyomi_auth::user_service::remove_auth_method(&ctx.db, &auth.user_id, "totp")
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok("2FA has been successfully disabled".to_string())
}

