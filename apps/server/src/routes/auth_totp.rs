// SPDX-License-Identifier: AGPL-3.0-or-later

//! 2FA (TOTP) management endpoints.
//!
//! Provides status check, setup flow (generate secret + QR code), enable
//! (verify code and persist), and disable for authenticated users.

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use kyomi_auth::{middleware::AuthUser, redis_ops, totp, user_service};

use crate::state::AppState;

/// Build the 2FA router (mounted at `/auth`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/2fa/status", get(status))
        .route("/2fa/setup", post(setup))
        .route("/2fa/enable", post(enable))
        .route("/2fa/disable", post(disable))
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct EnableRequest {
    verification_code: String,
}

// ---------------------------------------------------------------------------
// GET /auth/2fa/status
// ---------------------------------------------------------------------------

/// Check whether the authenticated user has TOTP 2FA enabled.
async fn status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let enabled = user_service::has_totp_enabled(&state.db, &user.user_id).await?;

    Ok(Json(serde_json::json!({ "enabled": enabled })))
}

// ---------------------------------------------------------------------------
// POST /auth/2fa/setup
// ---------------------------------------------------------------------------

/// Begin 2FA setup: generate a secret, QR code, and provisioning URI.
///
/// The secret is stored in Redis (10 min TTL) until the user confirms with
/// a verification code via `POST /auth/2fa/enable`.
async fn setup(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Check not already enabled
    let enabled = user_service::has_totp_enabled(&state.db, &user.user_id).await?;
    if enabled {
        return Err(kyomi_core::Error::Conflict(
            "2FA is already enabled".into(),
        ));
    }

    let secret = totp::generate_secret();
    let qr_code = totp::generate_qr_code(&secret, &user.email)?;
    let provisioning_uri = totp::provisioning_uri(&secret, &user.email)?;

    // Store pending secret in Redis (10 min TTL)
    redis_ops::store_pending_totp(&state.kv, &user.user_id, &secret).await?;

    Ok(Json(serde_json::json!({
        "secret": secret,
        "qr_code": qr_code,
        "provisioning_uri": provisioning_uri,
    })))
}

// ---------------------------------------------------------------------------
// POST /auth/2fa/enable
// ---------------------------------------------------------------------------

/// Confirm 2FA setup by verifying a TOTP code against the pending secret.
///
/// On success the secret is persisted in `user_auth_methods`. On failure the
/// pending secret is re-stored in Redis so the user can retry.
async fn enable(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<EnableRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Get pending secret from Redis (atomic get+delete)
    let secret = redis_ops::get_pending_totp(&state.kv, &user.user_id).await?;

    let Some(secret) = secret else {
        return Err(kyomi_core::Error::BadRequest(
            "No pending 2FA setup found. Please start the setup process again.".into(),
        ));
    };

    // Verify the code
    if !totp::verify_code(&secret, &data.verification_code) {
        // Re-store secret so user can retry
        redis_ops::store_pending_totp(&state.kv, &user.user_id, &secret).await?;
        return Err(kyomi_core::Error::BadRequest(
            "Invalid verification code. Please try again.".into(),
        ));
    }

    // Persist TOTP auth method
    let now = chrono::Utc::now().to_rfc3339();
    let auth_data = serde_json::json!({
        "secret": secret,
        "enabled_at": now,
    });
    user_service::upsert_auth_method(&state.db, &user.user_id, "totp", &auth_data).await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "2FA has been successfully enabled",
    })))
}

// ---------------------------------------------------------------------------
// POST /auth/2fa/disable
// ---------------------------------------------------------------------------

/// Disable 2FA for the authenticated user.
async fn disable(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let enabled = user_service::has_totp_enabled(&state.db, &user.user_id).await?;
    if !enabled {
        return Err(kyomi_core::Error::BadRequest(
            "2FA is not currently enabled".into(),
        ));
    }

    user_service::remove_auth_method(&state.db, &user.user_id, "totp").await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "2FA has been successfully disabled",
    })))
}
