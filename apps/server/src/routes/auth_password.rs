// SPDX-License-Identifier: AGPL-3.0-or-later

//! Password authentication endpoints.
//!
//! Provides email+password login, set-password (for users who signed up
//! via OAuth/passkey), change-password, and unified signup (start + complete).

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;

use kyomi_auth::{
    middleware::AuthUser,
    password,
    rate_limiter,
    session,
    token_service,
    user_service,
};

use crate::state::AppState;

/// Terms of service version — matches passkey signup.
use kyomi_core::TERMS_VERSION;

/// Build the password auth router (mounted at `/auth`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/set-password", post(set_password))
        .route("/change-password", post(change_password))
        .route("/signup/start", post(signup_start))
        .route("/signup/complete", post(signup_complete))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return a JSON error response with `{"detail": "..."}`.
fn error_response(status: StatusCode, detail: &str) -> Response {
    (status, Json(serde_json::json!({"detail": detail}))).into_response()
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
    totp_code: Option<String>,
}

#[derive(Deserialize)]
struct SetPasswordRequest {
    new_password: String,
}

#[derive(Deserialize)]
struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

#[derive(Deserialize)]
struct SignupStartRequest {
    email: String,
    /// Name — required for self-hosted SMTP-less signup (one-step flow).
    #[serde(default)]
    name: Option<String>,
    /// Password — required for self-hosted SMTP-less signup (one-step flow).
    #[serde(default)]
    password: Option<String>,
}

#[derive(Deserialize)]
struct SignupCompleteRequest {
    token: String,
    name: String,
    password: String,
    #[serde(default = "crate::helpers::default_true")]
    terms_accepted: bool,
    #[serde(default)]
    marketing_consent: bool,
}

// ---------------------------------------------------------------------------
// Endpoint: POST /auth/login
// ---------------------------------------------------------------------------

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<LoginRequest>,
) -> Response {
    // Rate limit by IP
    let ip = crate::helpers::extract_client_ip(&headers, None);
    let rate_result = match rate_limiter::check_rate_limit(&state.kv, &ip, "login", None).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "Rate limiter error during login");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error");
        }
    };
    if !rate_result.allowed {
        return error_response(
            StatusCode::TOO_MANY_REQUESTS,
            &format!("Rate limited. Try again in {} seconds", rate_result.retry_after_secs),
        );
    }

    let email = data.email.to_lowercase().trim().to_string();

    // Look up user — return generic error to prevent enumeration
    let user = match user_service::get_user_by_email(&state.db, &email).await {
        Ok(u) => u,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    };

    let Some(user) = user else {
        return error_response(StatusCode::UNAUTHORIZED, "Invalid credentials");
    };

    // Get password auth method
    let password_method = match user_service::get_auth_method(&state.db, &user.user_id, "password").await {
        Ok(m) => m,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    };

    let Some(password_method) = password_method else {
        // User exists but has no password — return generic error
        return error_response(StatusCode::UNAUTHORIZED, "Invalid credentials");
    };

    // Extract hash from auth_data
    let Some(hash) = password_method.auth_data.get("hash").and_then(|v| v.as_str()) else {
        tracing::error!(user_id = %user.user_id, "Password auth method missing hash");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error");
    };

    // Verify password
    let valid = match password::verify_password(&data.password, hash) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(user_id = %user.user_id, error = %e, "Password verification error");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error");
        }
    };

    if !valid {
        return error_response(StatusCode::UNAUTHORIZED, "Invalid credentials");
    }

    // Check email verification BEFORE TOTP (don't leak TOTP status for unverified accounts)
    if !user.verified {
        let mut resp = error_response(
            StatusCode::FORBIDDEN,
            "Please verify your email before signing in. Check your inbox for the verification link.",
        );
        resp.headers_mut().insert(
            "x-verification-required",
            "true".parse().expect("valid header value"),
        );
        return resp;
    }

    // Check if TOTP is enabled
    let totp_method = match user_service::get_auth_method(&state.db, &user.user_id, "totp").await {
        Ok(m) => m,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
    };

    if let Some(totp_method) = totp_method
        && totp_method.active {
            match &data.totp_code {
                None => {
                    return error_response(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "2FA verification code is required",
                    );
                }
                Some(code) => {
                    // Extract TOTP secret and verify the code
                    let Some(secret) = totp_method.auth_data.get("secret").and_then(|v| v.as_str()) else {
                        tracing::error!(user_id = %user.user_id, "TOTP auth method missing secret");
                        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error");
                    };
                    if !kyomi_auth::totp::verify_code(secret, code) {
                        return error_response(
                            StatusCode::UNAUTHORIZED,
                            "Invalid 2FA verification code",
                        );
                    }
                }
            }
        }

    // Create authenticated session
    let device = crate::helpers::extract_device_info(&headers);
    let sess = match session::create_authenticated_session(
        &state.db,
        &state.kv,
        &state.config.jwt_secret,
        &user,
        &device,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(user_id = %user.user_id, error = %e, "Failed to create session");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error");
        }
    };

    // Touch last_used on password auth method
    if let Err(e) = user_service::touch_auth_method(&state.db, &user.user_id, "password").await {
        tracing::warn!(user_id = %user.user_id, error = %e, "Failed to touch password auth method");
    }

    let body = serde_json::json!({
        "success": true,
        "message": "Login successful",
        "user": {
            "user_id": sess.user.user_id,
            "email": sess.user.email,
            "name": sess.user.name,
            "roles": sess.user.roles(),
        },
        "access_token": sess.access_token,
        "refresh_token": sess.refresh_token,
    });

    (sess.cookie_headers, Json(body)).into_response()
}

// ---------------------------------------------------------------------------
// Endpoint: POST /auth/set-password
// ---------------------------------------------------------------------------

async fn set_password(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<SetPasswordRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Validate password length
    if data.new_password.len() < 8 {
        return Err(kyomi_core::Error::BadRequest(
            "Password must be at least 8 characters".into(),
        ));
    }

    // Check user does NOT already have a password
    let has_pw = user_service::has_password(&state.db, &user.user_id).await?;
    if has_pw {
        return Err(kyomi_core::Error::Conflict(
            "Password already set. Use change-password to update it.".into(),
        ));
    }

    // Hash and store
    let hash = password::hash_password(&data.new_password)?;
    let auth_data = serde_json::json!({"hash": hash});
    user_service::upsert_auth_method(&state.db, &user.user_id, "password", &auth_data).await?;

    Ok(Json(serde_json::json!({"message": "Password set successfully"})))
}

// ---------------------------------------------------------------------------
// Endpoint: POST /auth/change-password
// ---------------------------------------------------------------------------

async fn change_password(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<ChangePasswordRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Validate new password length
    if data.new_password.len() < 8 {
        return Err(kyomi_core::Error::BadRequest(
            "New password must be at least 8 characters".into(),
        ));
    }

    // Get existing password auth method
    let password_method =
        user_service::get_auth_method(&state.db, &user.user_id, "password").await?;

    let Some(password_method) = password_method else {
        return Err(kyomi_core::Error::BadRequest(
            "No password set. Use set-password to create one.".into(),
        ));
    };

    // Extract and verify current password
    let Some(hash) = password_method.auth_data.get("hash").and_then(|v| v.as_str()) else {
        tracing::error!(user_id = %user.user_id, "Password auth method missing hash");
        return Err(kyomi_core::Error::Internal("Password auth method corrupted".into()));
    };

    let valid = password::verify_password(&data.current_password, hash)?;
    if !valid {
        return Err(kyomi_core::Error::Unauthorized(
            "Current password is incorrect".into(),
        ));
    }

    // Hash new password and upsert
    let new_hash = password::hash_password(&data.new_password)?;
    let auth_data = serde_json::json!({"hash": new_hash});
    user_service::upsert_auth_method(&state.db, &user.user_id, "password", &auth_data).await?;

    Ok(Json(serde_json::json!({"message": "Password changed successfully"})))
}

// ---------------------------------------------------------------------------
// Endpoint: POST /auth/signup/start
// ---------------------------------------------------------------------------

async fn signup_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<SignupStartRequest>,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    // Rate limit — this endpoint sends verification emails
    let ip = crate::helpers::extract_client_ip(&headers, None);
    let rate_result = rate_limiter::check_rate_limit(&state.kv, &ip, "signup", None).await?;
    if !rate_result.allowed {
        tracing::warn!(ip = %ip, "Password signup/start rate limited");
        return Err(kyomi_core::Error::TooManyRequests(
            format!(
                "Rate limited. Try again in {} seconds",
                rate_result.retry_after_secs
            ),
            rate_result.retry_after_secs,
        ));
    }

    let email = data.email.to_lowercase().trim().to_string();

    // Look up existing user
    let existing_user = user_service::get_user_by_email(&state.db, &email).await?;

    // Return the same response in all cases to prevent email enumeration.
    let success_response = serde_json::json!({
        "status": "verification_required",
        "message": "If this email is not already registered, a verification link has been sent. Please check your inbox.",
    });

    // Self-hosted without SMTP: skip email verification entirely.
    let smtp_less_self_hosted = state.config.self_hosted && !state.config.smtp_configured();

    // Self-hosted without SMTP: only the first user can self-register.
    // Subsequent users must have a pending invitation from the admin.
    if smtp_less_self_hosted && existing_user.is_none()
        && user_service::has_any_users(&state.db).await? {
            let pending = kyomi_auth::workspace_service::get_pending_invitations_for_email(
                &state.db, &email,
            ).await?;
            if pending.is_empty() {
                return Err(kyomi_core::Error::Forbidden(
                    "Registration is closed. Ask your administrator to invite you.".into(),
                ));
            }
        }

    match existing_user {
        None => {
            // NEW USER
            if smtp_less_self_hosted {
                // One-step signup: create verified user with password, return session tokens.
                let name = data.name.as_deref().unwrap_or("").trim();
                let password = data.password.as_deref().unwrap_or("");
                if name.is_empty() || password.is_empty() {
                    return Err(kyomi_core::Error::BadRequest(
                        "Name and password are required for self-hosted signup".into(),
                    ));
                }
                if password.len() < 8 {
                    return Err(kyomi_core::Error::BadRequest(
                        "Password must be at least 8 characters".into(),
                    ));
                }

                let user = user_service::create_user(&state.db, &email, Some(name), true).await?;
                let hash = password::hash_password(password)?;
                user_service::upsert_auth_method(
                    &state.db,
                    &user.user_id,
                    "password",
                    &serde_json::json!({"hash": hash}),
                )
                .await?;

                // Check for pending invitations — if invited, join existing workspace
                // instead of creating a new one.
                let pending = kyomi_auth::workspace_service::get_pending_invitations_for_email(
                    &state.db, &email,
                ).await?;
                if let Some(inv) = pending.first() {
                    // Accept the invitation: add user to workspace + mark invitation accepted
                    kyomi_auth::workspace_service::accept_invitation_for_user(
                        &state.db,
                        &inv.invitation_id,
                        &user.user_id,
                    ).await?;
                    user_service::update_last_workspace(
                        &state.db, &user.user_id, &inv.workspace_id,
                    ).await?;
                } else {
                    // First user — create their own workspace
                    user_service::create_workspace_for_user(
                        &state.db,
                        &user.user_id,
                        Some(name),
                        &email,
                    )
                    .await?;
                }

                // Re-fetch user after workspace setup
                let user = user_service::get_user_by_email(&state.db, &email)
                    .await?
                    .ok_or_else(|| {
                        kyomi_core::Error::Internal("User not found after creation".into())
                    })?;

                let device = crate::helpers::extract_device_info(&headers);
                let sess = session::create_authenticated_session(
                    &state.db,
                    &state.kv,
                    &state.config.jwt_secret,
                    &user,
                    &device,
                )
                .await?;

                tracing::info!(
                    email = %email,
                    user_id = %user.user_id,
                    "Self-hosted SMTP-less: one-step signup complete"
                );

                let body = serde_json::json!({
                    "status": "account_created",
                    "success": true,
                    "message": "Account created successfully",
                    "user": {
                        "user_id": sess.user.user_id,
                        "email": sess.user.email,
                        "name": sess.user.name,
                        "roles": sess.user.roles(),
                    },
                    "access_token": sess.access_token,
                    "refresh_token": sess.refresh_token,
                    "redirect": "/",
                });
                return Ok((sess.cookie_headers, Json(body)).into_response());
            } else {
                // Standard flow: create unverified user, send verification email
                let user = user_service::create_user(&state.db, &email, None, false).await?;

                // Notify admin (Slack + email) — fire-and-forget
                let notify_state = state.clone();
                let notify_email = email.clone();
                let notify_user_id = user.user_id.clone();
                tokio::spawn(async move {
                    crate::routes::admin_notify::notify_signup(
                        &notify_state,
                        &notify_email,
                        "",
                        &notify_user_id,
                    )
                    .await;
                });

                // Create email verification token
                let raw_token = token_service::create_verification_token(
                    &state.db,
                    &email,
                    "email_verification",
                )
                .await?;

                let signup_url = format!(
                    "{}/signup/complete?token={raw_token}",
                    state.config.frontend_url.trim_end_matches('/')
                );
                tracing::info!(
                    "Password signup link for {email}: {signup_url} (user_id={})",
                    user.user_id
                );

                crate::helpers::spawn_verification_email(email, String::new(), signup_url);
            }

            Ok(Json(success_response).into_response())
        }
        Some(user) if !user.verified => {
            if smtp_less_self_hosted {
                // Existing unverified user — complete signup with password in one step.
                let name = data.name.as_deref().unwrap_or("").trim();
                let password = data.password.as_deref().unwrap_or("");
                if name.is_empty() || password.is_empty() {
                    return Err(kyomi_core::Error::BadRequest(
                        "Name and password are required for self-hosted signup".into(),
                    ));
                }
                if password.len() < 8 {
                    return Err(kyomi_core::Error::BadRequest(
                        "Password must be at least 8 characters".into(),
                    ));
                }

                let hash = password::hash_password(password)?;
                user_service::upsert_auth_method(
                    &state.db,
                    &user.user_id,
                    "password",
                    &serde_json::json!({"hash": hash}),
                )
                .await?;
                user_service::update_user_name(&state.db, &user.user_id, name).await?;
                user_service::mark_user_verified(&state.db, &email).await?;

                // Create workspace if they don't have one yet
                user_service::create_workspace_for_user(
                    &state.db,
                    &user.user_id,
                    Some(name),
                    &email,
                )
                .await?;

                let user = user_service::get_user_by_email(&state.db, &email)
                    .await?
                    .ok_or_else(|| {
                        kyomi_core::Error::Internal("User not found after signup".into())
                    })?;

                let device = crate::helpers::extract_device_info(&headers);
                let sess = session::create_authenticated_session(
                    &state.db,
                    &state.kv,
                    &state.config.jwt_secret,
                    &user,
                    &device,
                )
                .await?;

                tracing::info!(
                    email = %email,
                    user_id = %user.user_id,
                    "Self-hosted SMTP-less: one-step signup complete for existing unverified user"
                );

                let body = serde_json::json!({
                    "status": "account_created",
                    "success": true,
                    "message": "Account created successfully",
                    "user": {
                        "user_id": sess.user.user_id,
                        "email": sess.user.email,
                        "name": sess.user.name,
                        "roles": sess.user.roles(),
                    },
                    "access_token": sess.access_token,
                    "refresh_token": sess.refresh_token,
                    "redirect": "/",
                });
                return Ok((sess.cookie_headers, Json(body)).into_response());
            } else {
                // EXISTING UNVERIFIED USER — resend verification email
                tracing::info!(email = %email, user_id = %user.user_id, "Resending verification email for pending user");

                let raw_token = token_service::create_verification_token(
                    &state.db,
                    &email,
                    "email_verification",
                )
                .await?;

                let signup_url = format!(
                    "{}/signup/complete?token={raw_token}",
                    state.config.frontend_url.trim_end_matches('/')
                );
                tracing::info!(
                    "Password signup link (resend) for {email}: {signup_url} (user_id={})",
                    user.user_id
                );

                let name = user.name.clone().unwrap_or_default();
                crate::helpers::spawn_verification_email(email, name, signup_url);
            }

            Ok(Json(success_response).into_response())
        }
        Some(_) => {
            // VERIFIED USER — already has an account.
            // Return the same response to prevent email enumeration.
            Ok(Json(success_response).into_response())
        }
    }
}

// ---------------------------------------------------------------------------
// Endpoint: POST /auth/signup/complete
// ---------------------------------------------------------------------------

async fn signup_complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<SignupCompleteRequest>,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    // Validate terms acceptance
    if !data.terms_accepted {
        return Err(kyomi_core::Error::BadRequest(
            "You must accept the Terms of Service and Privacy Policy to create an account.".into(),
        ));
    }

    // Validate password
    if data.password.len() < 8 {
        return Err(kyomi_core::Error::BadRequest(
            "Password must be at least 8 characters".into(),
        ));
    }

    // Validate name
    let name = data.name.trim().to_string();
    if name.is_empty() {
        return Err(kyomi_core::Error::BadRequest(
            "Name is required".into(),
        ));
    }

    // Verify the email verification token
    let email = token_service::verify_verification_token(
        &state.db,
        &data.token,
        "email_verification",
    )
    .await?
    .ok_or_else(|| {
        kyomi_core::Error::BadRequest(
            "Invalid or expired signup link. Please request a new one.".into(),
        )
    })?;

    // Get user (must exist — was created in signup/start)
    let user = user_service::get_user_by_email(&state.db, &email)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Internal("User not found for verified token".into())
        })?;

    // Hash password first (sync operation — fail early before any DB writes)
    let hash = password::hash_password(&data.password)?;
    let auth_data = serde_json::json!({"hash": hash});

    // Update user name
    user_service::update_user_name(&state.db, &user.user_id, &name).await?;

    // Mark user as verified
    user_service::mark_user_verified(&state.db, &email).await?;

    // Accept terms
    user_service::update_terms_acceptance(
        &state.db,
        &user.user_id,
        TERMS_VERSION,
        data.marketing_consent,
    )
    .await?;

    // Store marketing consent in extra_metadata
    if data.marketing_consent {
        user_service::update_extra_metadata(
            &state.db,
            &user.user_id,
            &serde_json::json!({"marketing_consent": true}),
        )
        .await?;
    }

    // Store password
    user_service::upsert_auth_method(&state.db, &user.user_id, "password", &auth_data).await?;

    // Create personal workspace
    user_service::create_workspace_for_user(
        &state.db,
        &user.user_id,
        Some(&name),
        &email,
    )
    .await?;

    // Re-fetch user after updates (verified=true, name updated)
    let user = user_service::get_user_by_email(&state.db, &email)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Internal("User not found after signup completion".into())
        })?;

    // Create authenticated session
    let device = crate::helpers::extract_device_info(&headers);
    let sess = session::create_authenticated_session(
        &state.db,
        &state.kv,
        &state.config.jwt_secret,
        &user,
        &device,
    )
    .await?;

    let body = serde_json::json!({
        "success": true,
        "message": "Account created successfully",
        "user": {
            "user_id": sess.user.user_id,
            "email": sess.user.email,
            "name": sess.user.name,
            "roles": sess.user.roles(),
        },
        "access_token": sess.access_token,
        "refresh_token": sess.refresh_token,
        "redirect": "/",
    });

    Ok((sess.cookie_headers, Json(body)))
}
