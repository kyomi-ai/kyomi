// SPDX-License-Identifier: AGPL-3.0-or-later

//! Unified account recovery endpoints.
//!
//! Provides a three-step recovery flow:
//! 1. `POST /auth/recovery/start` — send recovery email
//! 2. `POST /auth/recovery/verify` — verify token, create short-lived recovery session
//! 3. `POST /auth/recovery/set-password` — set new password using recovery session
//!
//! This replaces the passkey-specific recovery with a unified flow that supports
//! password recovery (and can be extended for passkey re-registration later).

use axum::{
    extract::State,
    http::HeaderMap,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::Deserialize;

use kyomi_auth::{
    email_service::EmailService,
    password,
    rate_limiter,
    redis_ops,
    session,
    token_service,
    user_service,
};

use crate::state::AppState;

/// Build the recovery router (mounted at `/auth`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/recovery/start", post(recovery_start))
        .route("/recovery/verify", post(recovery_verify))
        .route("/recovery/set-password", post(recovery_set_password))
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RecoveryStartRequest {
    email: String,
}

#[derive(Deserialize)]
struct RecoveryVerifyRequest {
    token: String,
}

#[derive(Deserialize)]
struct RecoverySetPasswordRequest {
    recovery_session_id: String,
    new_password: String,
}

// ---------------------------------------------------------------------------
// POST /auth/recovery/start
// ---------------------------------------------------------------------------

async fn recovery_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<RecoveryStartRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Self-hosted without SMTP: account recovery via email is impossible
    if state.config.self_hosted && !state.config.smtp_configured() {
        return Err(kyomi_core::Error::ServiceUnavailable(
            "Password reset requires email. Ask your administrator to configure SMTP.".into(),
        ));
    }

    let success_msg =
        "If a verified account exists with this email, a recovery link has been sent.";

    // Rate limit — reuse "register" bucket (conservative: email-sending endpoint)
    let ip = crate::helpers::extract_client_ip(&headers, None);
    let rate_result =
        rate_limiter::check_rate_limit(&state.kv, &ip, "register", None).await?;
    if !rate_result.allowed {
        tracing::warn!(ip = %ip, "Account recovery/start rate limited");
        return Err(kyomi_core::Error::TooManyRequests(
            format!(
                "Rate limited. Try again in {} seconds",
                rate_result.retry_after_secs
            ),
            rate_result.retry_after_secs,
        ));
    }

    let email = data.email.to_lowercase().trim().to_string();

    // Always return success to prevent email enumeration — do work silently
    let user = user_service::get_user_by_email(&state.db, &email)
        .await
        .ok()
        .flatten();

    if let Some(user) = user
        && user.verified {
            // Create recovery token (15 min = 0.25 hours)
            if let Ok(raw_token) = token_service::create_verification_token_with_expiry(
                &state.db,
                &email,
                "account_recovery",
                Some(0.25),
            )
            .await
            {
                let recovery_url = format!(
                    "{}/account/recover/complete?token={raw_token}",
                    state.config.frontend_url.trim_end_matches('/')
                );

                // Send recovery email (async, non-blocking)
                let user_name = user.name.clone().unwrap_or_default();
                let email_clone = email.clone();
                let url_clone = recovery_url.clone();
                tokio::spawn(async move {
                    let email_svc = EmailService::from_env();
                    let sent = email_svc
                        .send_account_recovery(&email_clone, &user_name, &url_clone)
                        .await;
                    if sent {
                        tracing::info!("Account recovery email sent to {email_clone}");
                    } else {
                        tracing::warn!(
                            "Failed to send account recovery email to {email_clone}"
                        );
                        tracing::info!(
                            "ACCOUNT RECOVERY LINK for {email_clone}: {url_clone}"
                        );
                    }
                });
            }
        }

    Ok(Json(serde_json::json!({
        "message": success_msg,
    })))
}

// ---------------------------------------------------------------------------
// POST /auth/recovery/verify
// ---------------------------------------------------------------------------

async fn recovery_verify(
    State(state): State<AppState>,
    Json(data): Json<RecoveryVerifyRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Verify recovery token (one-time use)
    let email = token_service::verify_verification_token(
        &state.db,
        &data.token,
        "account_recovery",
    )
    .await?
    .ok_or_else(|| {
        tracing::warn!("Account recovery/verify: invalid or expired token");
        kyomi_core::Error::BadRequest(
            "Invalid or expired recovery link. Please request a new one.".into(),
        )
    })?;

    // Get user and verify they're still in a valid state
    let user = user_service::get_user_by_email(&state.db, &email)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Internal("User not found for recovery token".into())
        })?;

    if !user.verified {
        return Err(kyomi_core::Error::BadRequest(
            "Account is not verified. Please complete signup first.".into(),
        ));
    }

    // Check if user has passkeys
    let has_passkeys = {
        let creds = user_service::get_passkey_credentials(&state.db, &user.user_id).await?;
        !creds.is_empty()
    };

    // Create a short-lived recovery session in Redis (15 min TTL)
    let recovery_session_id = redis_ops::generate_token();
    redis_ops::store_recovery_session(&state.kv, &recovery_session_id, &user.user_id)
        .await?;

    Ok(Json(serde_json::json!({
        "email": email,
        "has_passkeys": has_passkeys,
        "recovery_session_id": recovery_session_id,
    })))
}

// ---------------------------------------------------------------------------
// POST /auth/recovery/set-password
// ---------------------------------------------------------------------------

async fn recovery_set_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<RecoverySetPasswordRequest>,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    // Validate password
    if data.new_password.len() < 8 {
        return Err(kyomi_core::Error::BadRequest(
            "Password must be at least 8 characters".into(),
        ));
    }

    // Validate recovery session from Redis (non-destructive read).
    // We peek first so the session survives validation errors (e.g., same password).
    // The session is only deleted after the password is successfully changed.
    let user_id = redis_ops::peek_recovery_session(&state.kv, &data.recovery_session_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest(
                "Invalid or expired recovery session. Please start the recovery process again."
                    .into(),
            )
        })?;

    // Get user
    let user = user_service::get_user_by_id(&state.db, &user_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Internal("User not found for recovery session".into())
        })?;

    // If the user already has a password, verify the new one is different.
    // This is critical for security: recovery disables TOTP, so we must
    // invalidate any compromised password by requiring a different one.
    if let Some(existing) = user_service::get_auth_method(&state.db, &user_id, "password").await?
        && let Some(existing_hash) = existing.auth_data.get("hash").and_then(|v| v.as_str())
        && password::verify_password(&data.new_password, existing_hash)? {
            return Err(kyomi_core::Error::BadRequest(
                "New password must be different from your current password.".into(),
            ));
        }

    // Hash password and upsert auth method (create new or replace existing)
    let hash = password::hash_password(&data.new_password)?;
    let auth_data = serde_json::json!({"hash": hash});
    user_service::upsert_auth_method(&state.db, &user_id, "password", &auth_data).await?;

    // Consume the recovery session now that the password has been successfully changed.
    redis_ops::delete_recovery_session(&state.kv, &data.recovery_session_id).await?;

    // Disable TOTP if enabled — only AFTER password has been successfully changed.
    // Recovery proves email ownership (legitimate user). Requiring a different password
    // ensures an attacker's stolen password is invalidated before TOTP is removed.
    let totp_disabled = user_service::remove_auth_method(&state.db, &user_id, "totp").await?;
    if totp_disabled {
        tracing::info!(user_id = %user_id, "TOTP disabled during account recovery");
    }

    // Create authenticated session (log user in)
    let device = crate::helpers::extract_device_info(&headers);
    let sess = session::create_authenticated_session(
        &state.db,
        &state.kv,
        &state.config.jwt_secret,
        &user,
        &device,
    )
    .await?;

    let message = if totp_disabled {
        "Password set successfully. Two-factor authentication has been disabled. You are now logged in."
    } else {
        "Password set successfully. You are now logged in."
    };

    let body = serde_json::json!({
        "success": true,
        "message": message,
        "totp_disabled": totp_disabled,
        "user": {
            "user_id": sess.user.user_id,
            "email": sess.user.email,
            "name": sess.user.name,
            "roles": sess.user.roles(),
        },
        "access_token": sess.access_token,
        "refresh_token": sess.refresh_token,
    });

    Ok((sess.cookie_headers, Json(body)))
}
