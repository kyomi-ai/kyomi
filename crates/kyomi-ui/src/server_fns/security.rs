// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for Security settings.
//!
//! These replace the REST API calls for password management:
//! - `POST /auth/set-password` -> `set_password()`
//! - `POST /auth/change-password` -> `change_password()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/auth_password.rs`.

use leptos::prelude::*;

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context};

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

