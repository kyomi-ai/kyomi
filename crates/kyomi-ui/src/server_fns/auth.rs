// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for unauthenticated auth flows.
//!
//! These are public server functions (no `extract_auth()` call) used by the
//! login page, signup pages, and auth configuration:
//! - `GET  /api/v1/auth/config`        -> `get_auth_config()`
//! - `POST /api/v1/auth/login`         -> `login_with_password()`
//! - `POST /auth/signup/start`         -> `signup_start()`
//! - `POST /auth/signup/complete`      -> `signup_complete()`
//! - `POST /auth/google/callback`      -> `google_oauth_callback()`
//! - `POST /auth/signup/resend`        -> `resend_verification()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/auth.rs`,
//! `apps/server/src/routes/auth_password.rs`, and
//! `apps/server/src/routes/auth_google_oauth.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::extract_context;

/// Auth configuration — which authentication methods are available.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthConfig {
    pub google_oauth: bool,
    pub passkeys: bool,
    pub password: bool,
    pub self_hosted: bool,
    pub smtp_configured: bool,
}

/// Result of a login attempt.
///
/// Uses typed variants instead of HTTP status codes so the Leptos UI can
/// pattern-match on outcomes without string parsing.
///
/// Note: `Success` does not include `access_token` / `refresh_token` in the
/// response body. Tokens are set as HTTPOnly cookies by the server function
/// via `ResponseOptions`. This is a deliberate design choice — the Leptos
/// client relies exclusively on cookies for authentication, not body tokens.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LoginResult {
    Success {
        user_id: String,
        email: String,
        name: String,
    },
    TwoFactorRequired {
        email: String,
    },
    VerificationRequired {
        email: String,
    },
    RateLimited {
        retry_after_secs: u64,
    },
    Error {
        message: String,
    },
}

/// Result of a signup start attempt.
///
/// Uses typed variants so the Leptos UI can pattern-match on outcomes.
/// Tokens are set as HTTPOnly cookies by the server function for
/// `AccountCreated` (self-hosted SMTP-less one-step flow).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SignupResult {
    /// SaaS flow: verification email sent, user must click link.
    VerificationRequired { message: String },
    /// Self-hosted SMTP-less flow: account created directly, cookies set.
    AccountCreated { redirect: String },
    /// Error during signup.
    Error { message: String },
    /// Rate limited.
    RateLimited { retry_after_secs: u64 },
}

/// Result of completing signup (email verification token flow).
///
/// Cookies are set via `ResponseOptions` for the `Success` variant.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SignupCompleteResult {
    /// Account created and authenticated successfully.
    Success { user_id: String },
    /// Error during signup completion.
    Error { message: String },
}

/// Result of a Google OAuth callback.
///
/// Cookies are set via `ResponseOptions` for the `Success` variant.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum GoogleCallbackResult {
    /// Existing user logged in successfully.
    Success { oauth_continue: Option<String> },
    /// New user or user needing terms acceptance — redirect to welcome page.
    PendingTerms { redirect_url: String },
    /// Error during OAuth callback.
    Error { message: String },
    /// Rate limited.
    RateLimited { retry_after_secs: u64 },
}

/// Get the auth configuration (which methods are available).
///
/// Public endpoint — no authentication required.
/// Mirrors `GET /auth/config` in `apps/server/src/routes/auth.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_auth_config() -> Result<AuthConfig, ServerFnError> {
    let ctx = extract_context()?;

    Ok(AuthConfig {
        google_oauth: ctx.config.google_oauth_client_id.is_some()
            && ctx.config.google_oauth_client_secret.is_some(),
        passkeys: ctx.config.passkeys_enabled,
        password: ctx.config.password_auth_enabled,
        self_hosted: ctx.config.self_hosted,
        smtp_configured: ctx.config.smtp_configured(),
    })
}

/// Log in with email and password, optionally providing a TOTP code.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/login` in `apps/server/src/routes/auth_password.rs`.
///
/// Flow:
/// 1. Rate limit by IP
/// 2. Look up user by email (generic error if not found)
/// 3. Verify password
/// 4. Check email verification status
/// 5. Check TOTP — if enabled and no code, return `TwoFactorRequired`
/// 6. If TOTP code provided, verify it
/// 7. Create authenticated session
/// 8. Set HTTPOnly cookies via `ResponseOptions`
/// 9. Return `Success`
#[server(prefix = "/leptos-api")]
pub async fn login_with_password(
    email: String,
    password: String,
    totp_code: Option<String>,
) -> Result<LoginResult, ServerFnError> {
    let ctx = extract_context()?;

    // Extract headers for rate limiting and device info
    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;

    // Clone KVPool (cheap Arc clone) so ctx remains available for borrowing other fields
    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Rate limit by IP
    let ip = extract_client_ip(&headers);

    let rate_result = kyomi_auth::rate_limiter::check_rate_limit(&kv, &ip, "login", None)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Rate limiter error during login");
            ServerFnError::new("Internal server error")
        })?;
    if !rate_result.allowed {
        return Ok(LoginResult::RateLimited {
            retry_after_secs: rate_result.retry_after_secs,
        });
    }

    let email = email.to_lowercase().trim().to_string();

    // Look up user — return generic error to prevent enumeration
    let user = kyomi_auth::user_service::get_user_by_email(&ctx.db, &email)
        .await
        .map_err(|_| ServerFnError::new("Internal server error"))?;

    let Some(user) = user else {
        return Ok(LoginResult::Error {
            message: "Invalid credentials".to_string(),
        });
    };

    // Get password auth method
    let password_method =
        kyomi_auth::user_service::get_auth_method(&ctx.db, &user.user_id, "password")
            .await
            .map_err(|_| ServerFnError::new("Internal server error"))?;

    let Some(password_method) = password_method else {
        // User exists but has no password — return generic error
        return Ok(LoginResult::Error {
            message: "Invalid credentials".to_string(),
        });
    };

    // Extract hash from auth_data
    let Some(hash) = password_method
        .auth_data
        .get("hash")
        .and_then(|v| v.as_str())
    else {
        tracing::error!(user_id = %user.user_id, "Password auth method missing hash");
        return Err(ServerFnError::new("Internal server error"));
    };

    // Verify password
    let valid = kyomi_auth::password::verify_password(&password, hash).map_err(|e| {
        tracing::error!(user_id = %user.user_id, error = %e, "Password verification error");
        ServerFnError::new("Internal server error")
    })?;

    if !valid {
        return Ok(LoginResult::Error {
            message: "Invalid credentials".to_string(),
        });
    }

    // Check email verification BEFORE TOTP (don't leak TOTP status for unverified accounts)
    if !user.verified {
        return Ok(LoginResult::VerificationRequired {
            email: user.email.clone(),
        });
    }

    // Check if TOTP is enabled
    let totp_method =
        kyomi_auth::user_service::get_auth_method(&ctx.db, &user.user_id, "totp")
            .await
            .map_err(|_| ServerFnError::new("Internal server error"))?;

    if let Some(totp_method) = totp_method {
        if totp_method.active {
            match &totp_code {
                None => {
                    return Ok(LoginResult::TwoFactorRequired {
                        email: user.email.clone(),
                    });
                }
                Some(code) => {
                    // Extract TOTP secret and verify the code
                    let Some(secret) =
                        totp_method.auth_data.get("secret").and_then(|v| v.as_str())
                    else {
                        tracing::error!(user_id = %user.user_id, "TOTP auth method missing secret");
                        return Err(ServerFnError::new("Internal server error"));
                    };
                    if !kyomi_auth::totp::verify_code(secret, code) {
                        return Ok(LoginResult::Error {
                            message: "Invalid 2FA verification code".to_string(),
                        });
                    }
                }
            }
        }
    }

    // Build device info from headers
    let device = extract_device_info(&headers);

    // Create authenticated session
    let sess = kyomi_auth::session::create_authenticated_session(
        &ctx.db,
        &kv,
        &ctx.config.jwt_secret,
        &user,
        &device,
    )
    .await
    .map_err(|e| {
        tracing::error!(user_id = %user.user_id, error = %e, "Failed to create session");
        ServerFnError::new("Internal server error")
    })?;

    // Touch last_used on password auth method
    if let Err(e) =
        kyomi_auth::user_service::touch_auth_method(&ctx.db, &user.user_id, "password").await
    {
        tracing::warn!(user_id = %user.user_id, error = %e, "Failed to touch password auth method");
    }

    // Set HTTPOnly cookies via ResponseOptions
    let response_options = leptos::prelude::expect_context::<leptos_axum::ResponseOptions>();
    for (name, value) in sess.cookie_headers.iter() {
        if name == axum::http::header::SET_COOKIE {
            response_options.append_header(name.clone(), value.clone());
        }
    }

    Ok(LoginResult::Success {
        user_id: sess.user.user_id,
        email: sess.user.email,
        name: sess.user.name.unwrap_or_default(),
    })
}

/// Start the signup flow.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/signup/start` in `apps/server/src/routes/auth_password.rs`.
///
/// Two modes:
/// - **Self-hosted without SMTP** (name + password provided): Creates account
///   directly, sets cookies, returns `AccountCreated`.
/// - **SaaS with SMTP**: Creates unverified user, sends verification email,
///   returns `VerificationRequired`.
#[server(prefix = "/leptos-api")]
pub async fn signup_start(
    email: String,
    name: Option<String>,
    password: Option<String>,
) -> Result<SignupResult, ServerFnError> {
    let ctx = extract_context()?;

    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Rate limit by IP
    let ip = extract_client_ip(&headers);

    let rate_result = kyomi_auth::rate_limiter::check_rate_limit(&kv, &ip, "signup", None)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Rate limiter error during signup");
            ServerFnError::new("Internal server error")
        })?;
    if !rate_result.allowed {
        return Ok(SignupResult::RateLimited {
            retry_after_secs: rate_result.retry_after_secs,
        });
    }

    let email = email.to_lowercase().trim().to_string();

    // Look up existing user
    let existing_user = kyomi_auth::user_service::get_user_by_email(&ctx.db, &email)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to look up user by email");
            ServerFnError::new("Internal server error")
        })?;

    // Anti-enumeration: return the same message whether email exists or not
    let success_message =
        "If this email is not already registered, a verification link has been sent. Please check your inbox.";

    // Self-hosted without SMTP: skip email verification entirely.
    let smtp_less_self_hosted = ctx.config.self_hosted && !ctx.config.smtp_configured();

    // Self-hosted without SMTP: only the first user can self-register.
    // Subsequent users must have a pending invitation from the admin.
    if smtp_less_self_hosted && existing_user.is_none() {
        if kyomi_auth::user_service::has_any_users(&ctx.db).await.map_err(|e| {
            tracing::error!(error = %e, "Failed to check if any users exist");
            ServerFnError::new("Internal server error")
        })? {
            let pending =
                kyomi_auth::workspace_service::get_pending_invitations_for_email(&ctx.db, &email)
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "Failed to check pending invitations");
                        ServerFnError::new("Internal server error")
                    })?;
            if pending.is_empty() {
                return Ok(SignupResult::Error {
                    message: "Registration is closed. Ask your administrator to invite you."
                        .to_string(),
                });
            }
        }
    }

    match existing_user {
        None => {
            // NEW USER
            if smtp_less_self_hosted {
                // One-step signup: create verified user with password, return session tokens.
                let name_str = name.as_deref().unwrap_or("").trim();
                let password_str = password.as_deref().unwrap_or("");
                if name_str.is_empty() || password_str.is_empty() {
                    return Ok(SignupResult::Error {
                        message: "Name and password are required for self-hosted signup"
                            .to_string(),
                    });
                }
                if password_str.len() < 8 {
                    return Ok(SignupResult::Error {
                        message: "Password must be at least 8 characters".to_string(),
                    });
                }

                let user =
                    kyomi_auth::user_service::create_user(&ctx.db, &email, Some(name_str), true)
                        .await
                        .map_err(|e| {
                            tracing::error!(error = %e, "Failed to create user");
                            ServerFnError::new("Internal server error")
                        })?;

                let hash =
                    kyomi_auth::password::hash_password(password_str).map_err(|e| {
                        tracing::error!(error = %e, "Failed to hash password");
                        ServerFnError::new("Internal server error")
                    })?;
                kyomi_auth::user_service::upsert_auth_method(
                    &ctx.db,
                    &user.user_id,
                    "password",
                    &serde_json::json!({"hash": hash}),
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to store password auth method");
                    ServerFnError::new("Internal server error")
                })?;

                // Check for pending invitations — if invited, join existing workspace
                let pending =
                    kyomi_auth::workspace_service::get_pending_invitations_for_email(
                        &ctx.db, &email,
                    )
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "Failed to check pending invitations");
                        ServerFnError::new("Internal server error")
                    })?;
                if let Some(inv) = pending.first() {
                    kyomi_auth::workspace_service::accept_invitation_for_user(
                        &ctx.db,
                        &inv.invitation_id,
                        &user.user_id,
                    )
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "Failed to accept invitation");
                        ServerFnError::new("Internal server error")
                    })?;
                    kyomi_auth::user_service::update_last_workspace(
                        &ctx.db,
                        &user.user_id,
                        &inv.workspace_id,
                    )
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "Failed to update last workspace");
                        ServerFnError::new("Internal server error")
                    })?;
                } else {
                    // First user — create their own workspace
                    kyomi_auth::user_service::create_workspace_for_user(
                        &ctx.db,
                        &user.user_id,
                        Some(name_str),
                        &email,
                    )
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "Failed to create workspace");
                        ServerFnError::new("Internal server error")
                    })?;
                }

                // Re-fetch user after workspace setup
                let user = kyomi_auth::user_service::get_user_by_email(&ctx.db, &email)
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "Failed to re-fetch user");
                        ServerFnError::new("Internal server error")
                    })?
                    .ok_or_else(|| ServerFnError::new("User not found after creation"))?;

                let device = extract_device_info(&headers);
                let sess = kyomi_auth::session::create_authenticated_session(
                    &ctx.db,
                    &kv,
                    &ctx.config.jwt_secret,
                    &user,
                    &device,
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to create session");
                    ServerFnError::new("Internal server error")
                })?;

                tracing::info!(
                    email = %email,
                    user_id = %user.user_id,
                    "Self-hosted SMTP-less: one-step signup complete"
                );

                // Set HTTPOnly cookies via ResponseOptions
                let response_options =
                    leptos::prelude::expect_context::<leptos_axum::ResponseOptions>();
                for (name, value) in sess.cookie_headers.iter() {
                    if name == axum::http::header::SET_COOKIE {
                        response_options.append_header(name.clone(), value.clone());
                    }
                }

                return Ok(SignupResult::AccountCreated {
                    redirect: "/".to_string(),
                });
            } else {
                // Standard flow: create unverified user, send verification email
                let user =
                    kyomi_auth::user_service::create_user(&ctx.db, &email, None, false)
                        .await
                        .map_err(|e| {
                            tracing::error!(error = %e, "Failed to create user");
                            ServerFnError::new("Internal server error")
                        })?;

                // Create email verification token
                let raw_token = kyomi_auth::token_service::create_verification_token(
                    &ctx.db,
                    &email,
                    "email_verification",
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to create verification token");
                    ServerFnError::new("Internal server error")
                })?;

                let signup_url = format!(
                    "{}/signup/complete?token={raw_token}",
                    ctx.config.frontend_url.trim_end_matches('/')
                );
                tracing::info!(
                    "Password signup link for {email}: {signup_url} (user_id={})",
                    user.user_id
                );

                // Send verification email (fire-and-forget)
                spawn_verification_email(email.clone(), String::new(), signup_url);

                // Admin notification (Slack + email) — fire-and-forget
                let notify_webhook = ctx.config.slack_feedback_webhook_url.clone();
                let notify_support = ctx.config.support_email.clone();
                let notify_email = email.clone();
                let notify_user_id = user.user_id.clone();
                tokio::spawn(async move {
                    kyomi_auth::notifications::notify_signup(
                        notify_webhook.as_deref(),
                        &notify_support,
                        &notify_email,
                        "",
                        &notify_user_id,
                    )
                    .await;
                });
            }

            Ok(SignupResult::VerificationRequired {
                message: success_message.to_string(),
            })
        }
        Some(user) if !user.verified => {
            if smtp_less_self_hosted {
                // Existing unverified user — complete signup with password in one step.
                let name_str = name.as_deref().unwrap_or("").trim();
                let password_str = password.as_deref().unwrap_or("");
                if name_str.is_empty() || password_str.is_empty() {
                    return Ok(SignupResult::Error {
                        message: "Name and password are required for self-hosted signup"
                            .to_string(),
                    });
                }
                if password_str.len() < 8 {
                    return Ok(SignupResult::Error {
                        message: "Password must be at least 8 characters".to_string(),
                    });
                }

                let hash =
                    kyomi_auth::password::hash_password(password_str).map_err(|e| {
                        tracing::error!(error = %e, "Failed to hash password");
                        ServerFnError::new("Internal server error")
                    })?;
                kyomi_auth::user_service::upsert_auth_method(
                    &ctx.db,
                    &user.user_id,
                    "password",
                    &serde_json::json!({"hash": hash}),
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to store password auth method");
                    ServerFnError::new("Internal server error")
                })?;
                kyomi_auth::user_service::update_user_name(&ctx.db, &user.user_id, name_str)
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "Failed to update user name");
                        ServerFnError::new("Internal server error")
                    })?;
                kyomi_auth::user_service::mark_user_verified(&ctx.db, &email)
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "Failed to mark user verified");
                        ServerFnError::new("Internal server error")
                    })?;

                // Create workspace if they don't have one yet
                kyomi_auth::user_service::create_workspace_for_user(
                    &ctx.db,
                    &user.user_id,
                    Some(name_str),
                    &email,
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to create workspace");
                    ServerFnError::new("Internal server error")
                })?;

                let user = kyomi_auth::user_service::get_user_by_email(&ctx.db, &email)
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "Failed to re-fetch user");
                        ServerFnError::new("Internal server error")
                    })?
                    .ok_or_else(|| ServerFnError::new("User not found after signup"))?;

                let device = extract_device_info(&headers);
                let sess = kyomi_auth::session::create_authenticated_session(
                    &ctx.db,
                    &kv,
                    &ctx.config.jwt_secret,
                    &user,
                    &device,
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to create session");
                    ServerFnError::new("Internal server error")
                })?;

                tracing::info!(
                    email = %email,
                    user_id = %user.user_id,
                    "Self-hosted SMTP-less: one-step signup complete for existing unverified user"
                );

                // Set HTTPOnly cookies via ResponseOptions
                let response_options =
                    leptos::prelude::expect_context::<leptos_axum::ResponseOptions>();
                for (name, value) in sess.cookie_headers.iter() {
                    if name == axum::http::header::SET_COOKIE {
                        response_options.append_header(name.clone(), value.clone());
                    }
                }

                return Ok(SignupResult::AccountCreated {
                    redirect: "/".to_string(),
                });
            } else {
                // EXISTING UNVERIFIED USER — resend verification email
                tracing::info!(email = %email, user_id = %user.user_id, "Resending verification email for pending user");

                let raw_token = kyomi_auth::token_service::create_verification_token(
                    &ctx.db,
                    &email,
                    "email_verification",
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to create verification token");
                    ServerFnError::new("Internal server error")
                })?;

                let signup_url = format!(
                    "{}/signup/complete?token={raw_token}",
                    ctx.config.frontend_url.trim_end_matches('/')
                );
                tracing::info!(
                    "Password signup link (resend) for {email}: {signup_url} (user_id={})",
                    user.user_id
                );

                let user_name = user.name.clone().unwrap_or_default();
                spawn_verification_email(email.clone(), user_name, signup_url);
            }

            Ok(SignupResult::VerificationRequired {
                message: success_message.to_string(),
            })
        }
        Some(_) => {
            // VERIFIED USER — already has an account.
            // Return the same response to prevent email enumeration.
            Ok(SignupResult::VerificationRequired {
                message: success_message.to_string(),
            })
        }
    }
}

/// Complete the signup flow after email verification.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/signup/complete` in `apps/server/src/routes/auth_password.rs`.
///
/// Verifies the signup token, creates the user account with password,
/// creates a workspace, sets auth cookies, and returns `Success`.
#[server(prefix = "/leptos-api")]
pub async fn signup_complete(
    token: String,
    name: String,
    password: String,
    terms_accepted: bool,
    marketing_consent: bool,
) -> Result<SignupCompleteResult, ServerFnError> {
    let ctx = extract_context()?;

    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Validate terms acceptance
    if !terms_accepted {
        return Ok(SignupCompleteResult::Error {
            message:
                "You must accept the Terms of Service and Privacy Policy to create an account."
                    .to_string(),
        });
    }

    // Validate password
    if password.len() < 8 {
        return Ok(SignupCompleteResult::Error {
            message: "Password must be at least 8 characters".to_string(),
        });
    }

    // Validate name
    let name = name.trim().to_string();
    if name.is_empty() {
        return Ok(SignupCompleteResult::Error {
            message: "Name is required".to_string(),
        });
    }

    // Verify the email verification token
    let email = kyomi_auth::token_service::verify_verification_token(
        &ctx.db,
        &token,
        "email_verification",
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to verify verification token");
        ServerFnError::new("Internal server error")
    })?;

    let Some(email) = email else {
        return Ok(SignupCompleteResult::Error {
            message: "Invalid or expired signup link. Please request a new one.".to_string(),
        });
    };

    // Get user (must exist — was created in signup/start)
    let user = kyomi_auth::user_service::get_user_by_email(&ctx.db, &email)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to look up user");
            ServerFnError::new("Internal server error")
        })?
        .ok_or_else(|| ServerFnError::new("User not found for verified token"))?;

    // Hash password first (fail early before any DB writes)
    let hash = kyomi_auth::password::hash_password(&password).map_err(|e| {
        tracing::error!(error = %e, "Failed to hash password");
        ServerFnError::new("Internal server error")
    })?;
    let auth_data = serde_json::json!({"hash": hash});

    // Update user name
    kyomi_auth::user_service::update_user_name(&ctx.db, &user.user_id, &name)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update user name");
            ServerFnError::new("Internal server error")
        })?;

    // Mark user as verified
    kyomi_auth::user_service::mark_user_verified(&ctx.db, &email)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to mark user verified");
            ServerFnError::new("Internal server error")
        })?;

    // Accept terms
    kyomi_auth::user_service::update_terms_acceptance(
        &ctx.db,
        &user.user_id,
        kyomi_core::TERMS_VERSION,
        marketing_consent,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to update terms acceptance");
        ServerFnError::new("Internal server error")
    })?;

    // Store marketing consent in extra_metadata
    if marketing_consent {
        kyomi_auth::user_service::update_extra_metadata(
            &ctx.db,
            &user.user_id,
            &serde_json::json!({"marketing_consent": true}),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update extra metadata");
            ServerFnError::new("Internal server error")
        })?;
    }

    // Store password
    kyomi_auth::user_service::upsert_auth_method(&ctx.db, &user.user_id, "password", &auth_data)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to store password auth method");
            ServerFnError::new("Internal server error")
        })?;

    // Create personal workspace
    kyomi_auth::user_service::create_workspace_for_user(
        &ctx.db,
        &user.user_id,
        Some(&name),
        &email,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create workspace");
        ServerFnError::new("Internal server error")
    })?;

    // Re-fetch user after updates (verified=true, name updated)
    let user = kyomi_auth::user_service::get_user_by_email(&ctx.db, &email)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to re-fetch user");
            ServerFnError::new("Internal server error")
        })?
        .ok_or_else(|| ServerFnError::new("User not found after signup completion"))?;

    // Create authenticated session
    let device = extract_device_info(&headers);
    let sess = kyomi_auth::session::create_authenticated_session(
        &ctx.db,
        &kv,
        &ctx.config.jwt_secret,
        &user,
        &device,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create session");
        ServerFnError::new("Internal server error")
    })?;

    // Set HTTPOnly cookies via ResponseOptions
    let response_options = leptos::prelude::expect_context::<leptos_axum::ResponseOptions>();
    for (header_name, value) in sess.cookie_headers.iter() {
        if header_name == axum::http::header::SET_COOKIE {
            response_options.append_header(header_name.clone(), value.clone());
        }
    }

    Ok(SignupCompleteResult::Success {
        user_id: sess.user.user_id,
    })
}

/// Handle Google OAuth callback — exchange code for tokens and log in or start signup.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/google/callback` in `apps/server/src/routes/auth_google_oauth.rs`.
///
/// Flow:
/// 1. Rate limit by IP
/// 2. Verify CSRF state (if provided)
/// 3. Exchange authorization code for Google tokens
/// 4. Get user info from Google
/// 5. If new user or user needs terms: store pending signup in KV, return `PendingTerms`
/// 6. If existing user with terms accepted: create session, set cookies, return `Success`
#[server(prefix = "/leptos-api")]
pub async fn google_oauth_callback(
    code: String,
    state: Option<String>,
) -> Result<GoogleCallbackResult, ServerFnError> {
    let ctx = extract_context()?;

    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    let encryption_key = ctx
        .encryption_key
        .clone()
        .ok_or_else(|| ServerFnError::new("Encryption key not available"))?;

    // Get OAuth credentials from config
    let client_id = ctx
        .config
        .google_oauth_client_id
        .as_ref()
        .ok_or_else(|| ServerFnError::new("GOOGLE_OAUTH_CLIENT_ID not configured"))?
        .clone();
    let client_secret = ctx
        .config
        .google_oauth_client_secret
        .as_ref()
        .ok_or_else(|| ServerFnError::new("GOOGLE_OAUTH_CLIENT_SECRET not configured"))?
        .clone();

    // Rate limit
    let ip = extract_client_ip(&headers);
    let rate_result = kyomi_auth::rate_limiter::check_rate_limit(&kv, &ip, "login", None)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Rate limiter error during Google OAuth callback");
            ServerFnError::new("Internal server error")
        })?;
    if !rate_result.allowed {
        return Ok(GoogleCallbackResult::RateLimited {
            retry_after_secs: rate_result.retry_after_secs,
        });
    }

    // Verify CSRF state (optional — frontend may not send it)
    let mut oauth_continue = None;
    if let Some(ref csrf_state) = state {
        let state_data =
            kyomi_auth::redis_ops::verify_oauth_state(&kv, "google", csrf_state)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to verify OAuth state");
                    ServerFnError::new("Internal server error")
                })?;
        if let Some(state_data) = state_data {
            oauth_continue = state_data
                .get("oauth_continue")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }

    // Exchange code for tokens
    let redirect_uri = format!(
        "{}/auth/google/callback",
        ctx.config.frontend_url.trim_end_matches('/')
    );
    let token_data = kyomi_auth::google_oauth::exchange_code_for_tokens(
        &client_id,
        &client_secret,
        &code,
        &redirect_uri,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to exchange Google OAuth code for tokens");
        ServerFnError::new("Failed to exchange authorization code")
    })?;

    // Get user info from Google
    let user_info = kyomi_auth::google_oauth::get_user_info(&token_data.access_token)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get user info from Google");
            ServerFnError::new("Failed to get user info from Google")
        })?;
    let email = user_info.email.to_lowercase();

    // Look up existing user
    let existing_user = kyomi_auth::user_service::get_user_by_email(&ctx.db, &email)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to look up user by email");
            ServerFnError::new("Internal server error")
        })?;

    match existing_user {
        None => {
            // NEW USER — store pending signup, return temp token
            let temp_token = kyomi_auth::redis_ops::generate_token();
            let signup_data = serde_json::json!({
                "email": email,
                "name": user_info.name.unwrap_or_default(),
                "oauth_data": {
                    "google_id": user_info.id,
                    "oauth_provider": "google",
                    "picture": user_info.picture,
                }
            });
            kyomi_auth::redis_ops::store_pending_signup(&kv, &temp_token, &signup_data)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to store pending signup");
                    ServerFnError::new("Internal server error")
                })?;

            let redirect_url = format!(
                "{}/welcome?temp_token={temp_token}",
                ctx.config.frontend_url.trim_end_matches('/')
            );

            Ok(GoogleCallbackResult::PendingTerms { redirect_url })
        }
        Some(user) if user.terms_accepted_at.is_none() => {
            // EXISTING USER — needs terms acceptance
            let temp_token = kyomi_auth::redis_ops::generate_token();
            let terms_data = serde_json::json!({
                "user_id": user.user_id,
                "email": email,
            });
            kyomi_auth::redis_ops::store_pending_terms(&kv, &temp_token, &terms_data)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to store pending terms");
                    ServerFnError::new("Internal server error")
                })?;

            let redirect_url = format!(
                "{}/welcome?temp_token={temp_token}&existing_user=true",
                ctx.config.frontend_url.trim_end_matches('/')
            );

            Ok(GoogleCallbackResult::PendingTerms { redirect_url })
        }
        Some(user) => {
            // EXISTING USER — terms accepted, normal login

            // Ensure google_oauth auth method exists
            let auth_method =
                kyomi_auth::user_service::get_auth_method(&ctx.db, &user.user_id, "google_oauth")
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "Failed to get auth method");
                        ServerFnError::new("Internal server error")
                    })?;
            if auth_method.is_none() {
                let auth_data = serde_json::json!({
                    "linked_at": chrono::Utc::now().to_rfc3339(),
                });
                kyomi_auth::user_service::upsert_auth_method(
                    &ctx.db,
                    &user.user_id,
                    "google_oauth",
                    &auth_data,
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to upsert auth method");
                    ServerFnError::new("Internal server error")
                })?;
            }

            // Ensure user has a workspace
            let ws_ctx =
                kyomi_auth::user_service::get_user_workspace_context(&ctx.db, &user.user_id)
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "Failed to get workspace context");
                        ServerFnError::new("Internal server error")
                    })?;
            if ws_ctx.is_none() {
                kyomi_auth::user_service::create_workspace_for_user(
                    &ctx.db,
                    &user.user_id,
                    user.name.as_deref(),
                    &email,
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to create workspace");
                    ServerFnError::new("Internal server error")
                })?;
            }

            // Update profile in oauth_data (NOT tokens — login doesn't store tokens)
            let existing_oauth = kyomi_auth::google_oauth::parse_oauth_data(
                user.oauth_data.as_deref(),
                &encryption_key,
            )
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to parse existing OAuth data");
                ServerFnError::new("Internal server error")
            })?;

            let updated_oauth = kyomi_auth::google_oauth::OAuthData {
                google_id: Some(user_info.id),
                oauth_provider: Some("google".to_string()),
                picture: user_info.picture,
                last_oauth_login: Some(chrono::Utc::now().to_rfc3339()),
                // Preserve existing BigQuery tokens
                google_oauth_tokens: existing_oauth.and_then(|o| o.google_oauth_tokens),
                ..Default::default()
            };

            let encrypted =
                kyomi_auth::google_oauth::build_oauth_data(&updated_oauth, &encryption_key)
                    .map_err(|e| {
                        tracing::error!(error = %e, "Failed to build OAuth data");
                        ServerFnError::new("Internal server error")
                    })?;
            kyomi_auth::user_service::update_user_oauth_data(
                &ctx.db,
                &user.user_id,
                Some(&encrypted),
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to update user OAuth data");
                ServerFnError::new("Internal server error")
            })?;

            // Create authenticated session
            let device = extract_device_info(&headers);
            let sess = kyomi_auth::session::create_authenticated_session(
                &ctx.db,
                &kv,
                &ctx.config.jwt_secret,
                &user,
                &device,
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to create session");
                ServerFnError::new("Internal server error")
            })?;

            // Set HTTPOnly cookies via ResponseOptions
            let response_options =
                leptos::prelude::expect_context::<leptos_axum::ResponseOptions>();
            for (header_name, value) in sess.cookie_headers.iter() {
                if header_name == axum::http::header::SET_COOKIE {
                    response_options.append_header(header_name.clone(), value.clone());
                }
            }

            Ok(GoogleCallbackResult::Success { oauth_continue })
        }
    }
}

/// Resend the verification email for a pending signup.
///
/// Public endpoint — no authentication required.
/// Always returns `Ok(())` to prevent email enumeration.
///
/// Mirrors the resend logic in `signup_start` for existing unverified users
/// in `apps/server/src/routes/auth_password.rs`.
#[server(prefix = "/leptos-api")]
pub async fn resend_verification(email: String) -> Result<(), ServerFnError> {
    let ctx = extract_context()?;

    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Rate limit by IP
    let ip = extract_client_ip(&headers);

    let rate_result = kyomi_auth::rate_limiter::check_rate_limit(&kv, &ip, "register", None)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Rate limiter error during resend verification");
            ServerFnError::new("Internal server error")
        })?;
    if !rate_result.allowed {
        // Silently succeed to prevent enumeration
        return Ok(());
    }

    // Self-hosted without SMTP: all users are pre-verified, nothing to send
    if ctx.config.self_hosted && !ctx.config.smtp_configured() {
        return Ok(());
    }

    let email = email.to_lowercase().trim().to_string();

    // Look up user — only resend if unverified
    let user = kyomi_auth::user_service::get_user_by_email(&ctx.db, &email)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to look up user");
            ServerFnError::new("Internal server error")
        })?;

    if let Some(user) = user {
        if !user.verified {
            tracing::info!(email = %email, user_id = %user.user_id, "Resending verification email");

            let raw_token = kyomi_auth::token_service::create_verification_token(
                &ctx.db,
                &email,
                "email_verification",
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to create verification token");
                ServerFnError::new("Internal server error")
            })?;

            let verification_url = format!(
                "{}/verify-email?token={raw_token}",
                ctx.config.frontend_url.trim_end_matches('/')
            );
            tracing::info!(
                "Resend verification link for {email}: {verification_url} (user_id={})",
                user.user_id
            );

            let user_name = user.name.clone().unwrap_or_default();
            spawn_verification_email(email, user_name, verification_url);
        }
    }

    // Always return Ok to prevent email enumeration
    Ok(())
}

// ---------------------------------------------------------------------------
// Account Recovery
// ---------------------------------------------------------------------------

/// Result of verifying a recovery token.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RecoveryVerifyResult {
    Success {
        recovery_session_id: String,
        has_passkeys: bool,
    },
    Error {
        message: String,
    },
}

/// Result of setting a new password during recovery.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RecoverySetPasswordResult {
    Success,
    Error { message: String },
}

/// Start the account recovery flow by sending a recovery email.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/recovery/start` in `apps/server/src/routes/auth_recovery.rs`.
///
/// Always returns `Ok(())` to prevent email enumeration. If a verified account
/// exists with the given email, a recovery link is sent in the background.
#[server(prefix = "/leptos-api")]
pub async fn recovery_start(email: String) -> Result<(), ServerFnError> {
    let ctx = extract_context()?;

    // Self-hosted without SMTP: account recovery via email is impossible
    if ctx.config.self_hosted && !ctx.config.smtp_configured() {
        return Err(ServerFnError::new(
            "Password reset requires email. Ask your administrator to configure SMTP.",
        ));
    }

    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Rate limit by IP — reuse "register" bucket (conservative: email-sending endpoint)
    let ip = extract_client_ip(&headers);

    let rate_result =
        kyomi_auth::rate_limiter::check_rate_limit(&kv, &ip, "register", None)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Rate limiter error during recovery/start");
                ServerFnError::new("Internal server error")
            })?;
    if !rate_result.allowed {
        tracing::warn!(ip = %ip, "Account recovery/start rate limited");
        return Err(ServerFnError::new(format!(
            "Rate limited. Try again in {} seconds",
            rate_result.retry_after_secs
        )));
    }

    let email = email.to_lowercase().trim().to_string();

    // Always return success to prevent email enumeration — do work silently
    let user = kyomi_auth::user_service::get_user_by_email(&ctx.db, &email)
        .await
        .ok()
        .flatten();

    if let Some(user) = user {
        if user.verified {
            // Create recovery token (15 min = 0.25 hours)
            if let Ok(raw_token) =
                kyomi_auth::token_service::create_verification_token_with_expiry(
                    &ctx.db,
                    &email,
                    "account_recovery",
                    Some(0.25),
                )
                .await
            {
                let recovery_url = format!(
                    "{}/account/recover/complete?token={raw_token}",
                    ctx.config.frontend_url.trim_end_matches('/')
                );

                // Send recovery email (async, non-blocking)
                let user_name = user.name.clone().unwrap_or_default();
                let email_clone = email.clone();
                let url_clone = recovery_url.clone();
                tokio::spawn(async move {
                    let email_svc = kyomi_auth::email_service::EmailService::from_env();
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
    }

    Ok(())
}

/// Verify a recovery token and create a short-lived recovery session.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/recovery/verify` in `apps/server/src/routes/auth_recovery.rs`.
///
/// On success, returns a `recovery_session_id` (stored in KV with 15 min TTL)
/// and whether the user has passkeys registered.
#[server(prefix = "/leptos-api")]
pub async fn recovery_verify(
    token: String,
) -> Result<RecoveryVerifyResult, ServerFnError> {
    let ctx = extract_context()?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Verify recovery token (one-time use)
    let email = kyomi_auth::token_service::verify_verification_token(
        &ctx.db,
        &token,
        "account_recovery",
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Recovery token verification error");
        ServerFnError::new("Internal server error")
    })?;

    let Some(email) = email else {
        tracing::warn!("Account recovery/verify: invalid or expired token");
        return Ok(RecoveryVerifyResult::Error {
            message: "Invalid or expired recovery link. Please request a new one.".into(),
        });
    };

    // Get user and verify they're still in a valid state
    let user = kyomi_auth::user_service::get_user_by_email(&ctx.db, &email)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to look up user during recovery/verify");
            ServerFnError::new("Internal server error")
        })?
        .ok_or_else(|| ServerFnError::new("User not found for recovery token"))?;

    if !user.verified {
        return Ok(RecoveryVerifyResult::Error {
            message: "Account is not verified. Please complete signup first.".into(),
        });
    }

    // Check if user has passkeys
    let has_passkeys = {
        let creds = kyomi_auth::user_service::get_passkey_credentials(&ctx.db, &user.user_id)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to check passkeys during recovery/verify");
                ServerFnError::new("Internal server error")
            })?;
        !creds.is_empty()
    };

    // Create a short-lived recovery session in KV (15 min TTL)
    let recovery_session_id = kyomi_auth::redis_ops::generate_token();
    kyomi_auth::redis_ops::store_recovery_session(&kv, &recovery_session_id, &user.user_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to store recovery session");
            ServerFnError::new("Internal server error")
        })?;

    Ok(RecoveryVerifyResult::Success {
        recovery_session_id,
        has_passkeys,
    })
}

/// Set a new password using a recovery session, completing the recovery flow.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/recovery/set-password` in `apps/server/src/routes/auth_recovery.rs`.
///
/// Flow:
/// 1. Validate password (min 8 chars)
/// 2. Peek recovery session from KV (non-destructive read)
/// 3. Verify new password differs from existing (if any)
/// 4. Hash password with argon2id and upsert auth method
/// 5. Delete recovery session from KV
/// 6. Disable TOTP if enabled
/// 7. Create authenticated session and set cookies
#[server(prefix = "/leptos-api")]
pub async fn recovery_set_password(
    recovery_session_id: String,
    new_password: String,
) -> Result<RecoverySetPasswordResult, ServerFnError> {
    let ctx = extract_context()?;

    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Validate password
    if new_password.len() < 8 {
        return Ok(RecoverySetPasswordResult::Error {
            message: "Password must be at least 8 characters".into(),
        });
    }

    // Validate recovery session from KV (non-destructive read).
    // We peek first so the session survives validation errors (e.g., same password).
    // The session is only deleted after the password is successfully changed.
    let user_id = kyomi_auth::redis_ops::peek_recovery_session(&kv, &recovery_session_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to peek recovery session");
            ServerFnError::new("Internal server error")
        })?;

    let Some(user_id) = user_id else {
        return Ok(RecoverySetPasswordResult::Error {
            message: "Invalid or expired recovery session. Please start the recovery process again.".into(),
        });
    };

    // Get user
    let user = kyomi_auth::user_service::get_user_by_id(&ctx.db, &user_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to look up user during recovery/set-password");
            ServerFnError::new("Internal server error")
        })?
        .ok_or_else(|| ServerFnError::new("User not found for recovery session"))?;

    // If the user already has a password, verify the new one is different.
    // This is critical for security: recovery disables TOTP, so we must
    // invalidate any compromised password by requiring a different one.
    if let Some(existing) =
        kyomi_auth::user_service::get_auth_method(&ctx.db, &user_id, "password")
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get existing password auth method");
                ServerFnError::new("Internal server error")
            })?
    {
        if let Some(existing_hash) = existing.auth_data.get("hash").and_then(|v| v.as_str()) {
            let same = kyomi_auth::password::verify_password(&new_password, existing_hash)
                .map_err(|e| {
                    tracing::error!(error = %e, "Password verification error during recovery");
                    ServerFnError::new("Internal server error")
                })?;
            if same {
                return Ok(RecoverySetPasswordResult::Error {
                    message: "New password must be different from your current password.".into(),
                });
            }
        }
    }

    // Hash password and upsert auth method (create new or replace existing)
    let hash = kyomi_auth::password::hash_password(&new_password).map_err(|e| {
        tracing::error!(error = %e, "Failed to hash password during recovery");
        ServerFnError::new("Internal server error")
    })?;
    let auth_data = serde_json::json!({"hash": hash});
    kyomi_auth::user_service::upsert_auth_method(&ctx.db, &user_id, "password", &auth_data)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to upsert password auth method during recovery");
            ServerFnError::new("Internal server error")
        })?;

    // Consume the recovery session now that the password has been successfully changed.
    kyomi_auth::redis_ops::delete_recovery_session(&kv, &recovery_session_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to delete recovery session");
            ServerFnError::new("Internal server error")
        })?;

    // Disable TOTP if enabled — only AFTER password has been successfully changed.
    // Recovery proves email ownership (legitimate user). Requiring a different password
    // ensures an attacker's stolen password is invalidated before TOTP is removed.
    let totp_disabled =
        kyomi_auth::user_service::remove_auth_method(&ctx.db, &user_id, "totp")
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to remove TOTP during recovery");
                ServerFnError::new("Internal server error")
            })?;
    if totp_disabled {
        tracing::info!(user_id = %user_id, "TOTP disabled during account recovery");
    }

    // Create authenticated session (log user in)
    let device = extract_device_info(&headers);
    let sess = kyomi_auth::session::create_authenticated_session(
        &ctx.db,
        &kv,
        &ctx.config.jwt_secret,
        &user,
        &device,
    )
    .await
    .map_err(|e| {
        tracing::error!(user_id = %user_id, error = %e, "Failed to create session during recovery");
        ServerFnError::new("Internal server error")
    })?;

    // Set HTTPOnly cookies via ResponseOptions
    let response_options = leptos::prelude::expect_context::<leptos_axum::ResponseOptions>();
    for (name, value) in sess.cookie_headers.iter() {
        if name == axum::http::header::SET_COOKIE {
            response_options.append_header(name.clone(), value.clone());
        }
    }

    Ok(RecoverySetPasswordResult::Success)
}

// ---------------------------------------------------------------------------
// Passkey login/register (public, unauthenticated)
// ---------------------------------------------------------------------------

/// Result of starting a passkey login challenge.
///
/// Contains the challenge_id (to correlate start/complete) and the serialized
/// `PublicKeyCredentialRequestOptions` for the browser.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PasskeyLoginStartResult {
    pub challenge_id: String,
    /// JSON string of PublicKeyCredentialRequestOptions for `navigator.credentials.get()`.
    pub request_challenge: String,
}

/// Result of starting a passkey registration challenge.
///
/// Contains the challenge_id (to correlate start/complete) and the serialized
/// `PublicKeyCredentialCreationOptions` for the browser.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PasskeyRegisterStartResult {
    pub challenge_id: String,
    /// JSON string of PublicKeyCredentialCreationOptions for `navigator.credentials.create()`.
    pub creation_challenge: String,
}

/// Start passkey login — generate a WebAuthn assertion challenge.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/passkeys/login/start` in `apps/server/src/routes/auth_passkeys.rs`.
///
/// Uses discoverable credential flow (empty `allowCredentials`) so the browser
/// presents all available passkeys. If an email is provided and the user has
/// registered passkeys, uses the standard flow with `allowCredentials` populated.
#[server(prefix = "/leptos-api")]
pub async fn passkey_login_start() -> Result<PasskeyLoginStartResult, ServerFnError> {
    let ctx = extract_context()?;

    let webauthn = ctx
        .webauthn
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebAuthn not configured"))?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Use discoverable credential flow — no email required.
    // The browser will show all available passkeys for this relying party.
    let (mut rcr, disc_state) =
        kyomi_auth::webauthn::start_discoverable_authentication(webauthn)
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Remove mediation hint — we want a modal prompt, not conditional UI autofill
    rcr.mediation = None;

    let challenge_id = kyomi_auth::redis_ops::generate_token();
    let disc_state_json = serde_json::to_value(&disc_state)
        .map_err(|e| ServerFnError::new(format!("Serialize discoverable state: {e}")))?;

    let challenge_data = serde_json::json!({
        "discoverable_state": disc_state_json,
        "discoverable": true,
    });
    kyomi_auth::redis_ops::store_webauthn_challenge(&kv, &challenge_id, &challenge_data)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let request_challenge = serde_json::to_string(&rcr)
        .map_err(|e| ServerFnError::new(format!("Serialize request challenge: {e}")))?;

    Ok(PasskeyLoginStartResult {
        challenge_id,
        request_challenge,
    })
}

/// Complete passkey login — verify the WebAuthn assertion.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/passkeys/login/complete` in `apps/server/src/routes/auth_passkeys.rs`.
///
/// Flow:
/// 1. Rate limit by IP
/// 2. Retrieve challenge state from KV (and delete to prevent replay)
/// 3. Find user by credential ID from the assertion
/// 4. Verify user is verified and active
/// 5. Verify assertion with WebAuthn (discoverable or standard flow)
/// 6. Update credential usage (sign count)
/// 7. Create authenticated session and set cookies
/// 8. Return LoginResult::Success
#[server(prefix = "/leptos-api")]
pub async fn passkey_login_complete(
    challenge_id: String,
    assertion_json: String,
) -> Result<LoginResult, ServerFnError> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use webauthn_rs::prelude::*;

    let ctx = extract_context()?;

    let webauthn = ctx
        .webauthn
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebAuthn not configured"))?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Extract headers for rate limiting and device info
    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;

    // Rate limit by IP
    let ip = extract_client_ip(&headers);
    let rate_result = kyomi_auth::rate_limiter::check_rate_limit(&kv, &ip, "login", None)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Rate limiter error during passkey login");
            ServerFnError::new("Internal server error")
        })?;
    if !rate_result.allowed {
        return Ok(LoginResult::RateLimited {
            retry_after_secs: rate_result.retry_after_secs,
        });
    }

    // Parse the assertion from JSON
    let credential: PublicKeyCredential = serde_json::from_str(&assertion_json)
        .map_err(|e| ServerFnError::new(format!("Invalid assertion JSON: {e}")))?;

    // Get challenge from KV
    let challenge_data =
        kyomi_auth::redis_ops::get_webauthn_challenge(&kv, &challenge_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .ok_or_else(|| {
                tracing::warn!(ip = %ip, "Passkey login: invalid or expired challenge");
                ServerFnError::new("Invalid or expired challenge")
            })?;

    // Delete challenge (prevent replay)
    kyomi_auth::redis_ops::delete_webauthn_challenge(&kv, &challenge_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Find the user by credential ID
    let cred_id_bytes: &[u8] = credential.raw_id.as_ref();
    let credential_id_b64 = URL_SAFE_NO_PAD.encode(cred_id_bytes);

    let user = kyomi_auth::user_service::find_user_by_credential_id(&ctx.db, &credential_id_b64)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to find user by credential ID");
            ServerFnError::new("Internal server error")
        })?;

    let Some(user) = user else {
        return Ok(LoginResult::Error {
            message: "Invalid credentials".to_string(),
        });
    };

    // Check email verification
    if !user.verified {
        return Ok(LoginResult::VerificationRequired {
            email: user.email.clone(),
        });
    }

    // Verify the assertion based on challenge type (discoverable vs standard)
    let is_discoverable = challenge_data["discoverable"].as_bool().unwrap_or(false);

    if is_discoverable {
        let disc_state: DiscoverableAuthentication = serde_json::from_value(
            challenge_data["discoverable_state"].clone(),
        )
        .map_err(|e| ServerFnError::new(format!("Deserialize discoverable state: {e}")))?;

        let passkeys = get_passkeys_for_auth(&ctx.db, &user.user_id).await?;
        if passkeys.is_empty() {
            return Ok(LoginResult::Error {
                message: "No passkeys found for user".to_string(),
            });
        }

        let auth_result = kyomi_auth::webauthn::finish_discoverable_authentication(
            webauthn,
            &credential,
            disc_state,
            &passkeys,
        )
        .map_err(|e| {
            tracing::warn!(error = %e, "Passkey discoverable auth failed");
            ServerFnError::new("Authentication failed")
        })?;

        // Update credential usage
        update_passkey_after_auth(&ctx.db, &user.user_id, &credential_id_b64, cred_id_bytes, &passkeys, &auth_result).await;
    } else {
        let auth_state: PasskeyAuthentication = serde_json::from_value(
            challenge_data["authentication_state"].clone(),
        )
        .map_err(|e| ServerFnError::new(format!("Deserialize auth state: {e}")))?;

        let auth_result = kyomi_auth::webauthn::finish_authentication(
            webauthn,
            &credential,
            &auth_state,
        )
        .map_err(|e| {
            tracing::warn!(error = %e, "Passkey auth failed");
            ServerFnError::new("Authentication failed")
        })?;

        // Update credential usage
        let passkeys = get_passkeys_for_auth(&ctx.db, &user.user_id).await?;
        update_passkey_after_auth(&ctx.db, &user.user_id, &credential_id_b64, cred_id_bytes, &passkeys, &auth_result).await;
    }

    // Build device info from headers
    let device = extract_device_info(&headers);

    // Create authenticated session
    let sess = kyomi_auth::session::create_authenticated_session(
        &ctx.db,
        &kv,
        &ctx.config.jwt_secret,
        &user,
        &device,
    )
    .await
    .map_err(|e| {
        tracing::error!(user_id = %user.user_id, error = %e, "Failed to create session");
        ServerFnError::new("Internal server error")
    })?;

    // Touch last_used on webauthn auth method
    if let Err(e) =
        kyomi_auth::user_service::touch_auth_method(&ctx.db, &user.user_id, "webauthn").await
    {
        tracing::warn!(user_id = %user.user_id, error = %e, "Failed to touch webauthn auth method");
    }

    // Set HTTPOnly cookies via ResponseOptions
    let response_options = leptos::prelude::expect_context::<leptos_axum::ResponseOptions>();
    for (name, value) in sess.cookie_headers.iter() {
        if name == axum::http::header::SET_COOKIE {
            response_options.append_header(name.clone(), value.clone());
        }
    }

    Ok(LoginResult::Success {
        user_id: sess.user.user_id,
        email: sess.user.email,
        name: sess.user.name.unwrap_or_default(),
    })
}

/// Start passkey registration — create or find user and generate a WebAuthn challenge.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/passkeys/register/start` in `apps/server/src/routes/auth_passkeys.rs`.
///
/// For new users: creates an unverified user, sends a verification email (SaaS)
/// or issues a token directly (self-hosted SMTP-less).
/// For existing verified users: starts passkey registration directly.
/// For existing unverified users: resends verification email.
#[server(prefix = "/leptos-api")]
pub async fn passkey_register_start(
    email: String,
    name: Option<String>,
    device_name: String,
) -> Result<PasskeyRegisterStartResult, ServerFnError> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use webauthn_rs::prelude::*;

    let ctx = extract_context()?;

    let webauthn = ctx
        .webauthn
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebAuthn not configured"))?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Extract headers for rate limiting
    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;

    // Rate limit by IP
    let ip = extract_client_ip(&headers);
    let rate_result = kyomi_auth::rate_limiter::check_rate_limit(&kv, &ip, "signup", None)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Rate limiter error during passkey register");
            ServerFnError::new("Internal server error")
        })?;
    if !rate_result.allowed {
        return Err(ServerFnError::new(format!(
            "Rate limited. Try again in {} seconds",
            rate_result.retry_after_secs
        )));
    }

    let email = email.to_lowercase().trim().to_string();
    let name = name.unwrap_or_default();
    let device_name = if device_name.trim().is_empty() {
        "Unknown Device".to_string()
    } else {
        device_name.trim().to_string()
    };

    // Look up existing user
    let existing_user = kyomi_auth::user_service::get_user_by_email(&ctx.db, &email)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to look up user by email");
            ServerFnError::new("Internal server error")
        })?;

    // Get or create user — must be verified to proceed with passkey registration
    let user = match existing_user {
        Some(user) if user.verified => user,
        Some(_user) => {
            // Unverified user — cannot register passkey yet
            return Err(ServerFnError::new(
                "Please verify your email before registering a passkey.",
            ));
        }
        None => {
            // Create new user (unverified for SaaS, verified for SMTP-less self-hosted)
            let smtp_less_self_hosted =
                ctx.config.self_hosted && !ctx.config.smtp_configured();

            if smtp_less_self_hosted {
                kyomi_auth::user_service::create_user(&ctx.db, &email, Some(&name), true)
                    .await
                    .map_err(|e| {
                        tracing::error!(error = %e, "Failed to create user");
                        ServerFnError::new("Internal server error")
                    })?
            } else {
                // SaaS flow: create unverified user, send verification email
                let user = kyomi_auth::user_service::create_user(
                    &ctx.db, &email, Some(&name), false,
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to create user");
                    ServerFnError::new("Internal server error")
                })?;

                let raw_token = kyomi_auth::token_service::create_verification_token(
                    &ctx.db, &email, "signup",
                )
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "Failed to create verification token");
                    ServerFnError::new("Internal server error")
                })?;

                let signup_url = format!(
                    "{}/auth/passkey-signup?token={raw_token}",
                    ctx.config.frontend_url.trim_end_matches('/')
                );
                tracing::info!(
                    "Passkey signup link for {email}: {signup_url} (user_id={})",
                    user.user_id
                );

                spawn_verification_email(email.clone(), name.clone(), signup_url);

                return Err(ServerFnError::new(
                    "Please check your email to verify your account before registering a passkey.",
                ));
            }
        }
    };

    // Generate deterministic user handle from email (same as auth_passkeys.rs)
    let user_unique_id = webauthn_user_id(&email);
    let display_name = user.name.as_deref().unwrap_or(&email);

    // Get existing credential IDs to exclude (prevent re-registration)
    let creds = kyomi_auth::user_service::get_passkey_credentials(&ctx.db, &user.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let mut exclude_ids = Vec::new();
    for (cred_id_b64, _) in &creds {
        if let Ok(bytes) = URL_SAFE_NO_PAD.decode(cred_id_b64) {
            exclude_ids.push(CredentialID::from(bytes));
        }
    }
    let exclude_opt = if exclude_ids.is_empty() {
        None
    } else {
        Some(exclude_ids)
    };

    let (ccr, reg_state) = kyomi_auth::webauthn::start_registration(
        webauthn,
        user_unique_id,
        &email,
        display_name,
        exclude_opt,
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Store challenge in KV
    let challenge_id = kyomi_auth::redis_ops::generate_token();
    let reg_state_json = serde_json::to_value(&reg_state)
        .map_err(|e| ServerFnError::new(format!("Serialize reg state: {e}")))?;

    let challenge_data = serde_json::json!({
        "registration_state": reg_state_json,
        "email": email,
        "user_name": display_name,
        "user_id": user.user_id,
        "device_name": device_name,
        "is_signup": true,
    });
    kyomi_auth::redis_ops::store_webauthn_challenge(&kv, &challenge_id, &challenge_data)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let creation_challenge = serde_json::to_string(&ccr)
        .map_err(|e| ServerFnError::new(format!("Serialize creation challenge: {e}")))?;

    Ok(PasskeyRegisterStartResult {
        challenge_id,
        creation_challenge,
    })
}

/// Complete passkey registration — verify the browser credential and store it.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/passkeys/register/complete` in `apps/server/src/routes/auth_passkeys.rs`.
///
/// Flow:
/// 1. Retrieve registration state from KV (and delete to prevent replay)
/// 2. Parse credential from JSON
/// 3. Verify credential with WebAuthn
/// 4. Store passkey in DB
/// 5. Create authenticated session and set cookies (auto-login for signup)
/// 6. Return LoginResult::Success
#[server(prefix = "/leptos-api")]
pub async fn passkey_register_complete(
    challenge_id: String,
    credential_json: String,
) -> Result<LoginResult, ServerFnError> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use webauthn_rs::prelude::*;

    let ctx = extract_context()?;

    let webauthn = ctx
        .webauthn
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebAuthn not configured"))?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Extract headers for device info
    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;

    // Parse credential from JSON
    let credential: RegisterPublicKeyCredential = serde_json::from_str(&credential_json)
        .map_err(|e| ServerFnError::new(format!("Invalid credential JSON: {e}")))?;

    // Get challenge from KV
    let challenge_data =
        kyomi_auth::redis_ops::get_webauthn_challenge(&kv, &challenge_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .ok_or_else(|| ServerFnError::new("Invalid or expired challenge"))?;

    // Delete challenge (prevent replay)
    kyomi_auth::redis_ops::delete_webauthn_challenge(&kv, &challenge_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Extract challenge state
    let reg_state: PasskeyRegistration =
        serde_json::from_value(challenge_data["registration_state"].clone())
            .map_err(|e| ServerFnError::new(format!("Deserialize reg state: {e}")))?;

    let email = challenge_data["email"]
        .as_str()
        .ok_or_else(|| ServerFnError::new("Missing email in challenge"))?;
    let user_id = challenge_data["user_id"]
        .as_str()
        .ok_or_else(|| ServerFnError::new("Missing user_id in challenge"))?;
    let device_name = challenge_data["device_name"]
        .as_str()
        .unwrap_or("Unknown Device");

    // Verify the credential with webauthn-rs
    let passkey = kyomi_auth::webauthn::finish_registration(webauthn, &credential, &reg_state)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Extract credential ID as base64url (no padding) for storage
    let cred_id_bytes: &[u8] = passkey.cred_id().as_ref();
    let credential_id_b64 = URL_SAFE_NO_PAD.encode(cred_id_bytes);

    // Serialize passkey for storage
    let passkey_json = serde_json::to_value(&passkey)
        .map_err(|e| ServerFnError::new(format!("Serialize passkey: {e}")))?;

    let public_key_b64 = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&passkey)
            .map_err(|e| ServerFnError::new(format!("Serialize passkey bytes: {e}")))?,
    );

    let initial_counter = passkey_json
        .get("cred")
        .and_then(|c| c.get("counter"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;

    // Store credential in user's webauthn auth method
    kyomi_auth::user_service::add_passkey_to_user(
        &ctx.db,
        user_id,
        &credential_id_b64,
        &public_key_b64,
        initial_counter,
        device_name,
        &passkey_json,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to store passkey credential");
        ServerFnError::new("Internal server error")
    })?;

    // Get user for session creation
    let user = kyomi_auth::user_service::get_user_by_id(&ctx.db, user_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get user by ID");
            ServerFnError::new("Internal server error")
        })?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    // Build device info from headers
    let device = extract_device_info(&headers);

    // Create authenticated session (auto-login after registration)
    let sess = kyomi_auth::session::create_authenticated_session(
        &ctx.db,
        &kv,
        &ctx.config.jwt_secret,
        &user,
        &device,
    )
    .await
    .map_err(|e| {
        tracing::error!(user_id = %user.user_id, error = %e, "Failed to create session");
        ServerFnError::new("Internal server error")
    })?;

    // Set HTTPOnly cookies via ResponseOptions
    let response_options = leptos::prelude::expect_context::<leptos_axum::ResponseOptions>();
    for (name, value) in sess.cookie_headers.iter() {
        if name == axum::http::header::SET_COOKIE {
            response_options.append_header(name.clone(), value.clone());
        }
    }

    tracing::info!(
        user_id = %user.user_id,
        email = %email,
        credential_id = %credential_id_b64,
        "Passkey registered and user auto-logged in"
    );

    Ok(LoginResult::Success {
        user_id: sess.user.user_id,
        email: sess.user.email,
        name: sess.user.name.unwrap_or_default(),
    })
}

/// Result of verifying a passkey recovery token.
///
/// On success, returns the WebAuthn challenge for creating a new passkey,
/// plus the user's email for display purposes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PasskeyRecoveryVerifyResult {
    Success {
        challenge_id: String,
        /// JSON string of PublicKeyCredentialCreationOptions for `navigator.credentials.create()`.
        creation_challenge: String,
        email: String,
    },
    Error {
        message: String,
    },
}

/// Verify a passkey signup token and generate a WebAuthn registration challenge.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/passkeys/signup/complete` from the React backend.
///
/// Flow:
/// 1. Verify the email verification token
/// 2. Update user name, terms acceptance, marketing consent
/// 3. Mark user as verified
/// 4. Create personal workspace
/// 5. Generate WebAuthn registration challenge
/// 6. Return challenge for browser-side passkey creation
///
/// After this, the client calls `passkey_register_complete()` with the credential.
#[server(prefix = "/leptos-api")]
pub async fn passkey_signup_complete(
    token: String,
    name: String,
    terms_accepted: bool,
    marketing_consent: bool,
) -> Result<PasskeyRegisterStartResult, ServerFnError> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use webauthn_rs::prelude::*;

    let ctx = extract_context()?;

    let webauthn = ctx
        .webauthn
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebAuthn not configured"))?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Validate terms acceptance
    if !terms_accepted {
        return Err(ServerFnError::new(
            "You must accept the Terms of Service and Privacy Policy to create an account.",
        ));
    }

    // Validate name
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(ServerFnError::new("Name is required"));
    }

    // Verify the email verification token
    let email = kyomi_auth::token_service::verify_verification_token(
        &ctx.db,
        &token,
        "email_verification",
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to verify verification token");
        ServerFnError::new("Internal server error")
    })?;

    let Some(email) = email else {
        return Err(ServerFnError::new(
            "Invalid or expired signup link. Please request a new one.",
        ));
    };

    // Get user (must exist — was created in signup/start or passkey_register_start)
    let user = kyomi_auth::user_service::get_user_by_email(&ctx.db, &email)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to look up user");
            ServerFnError::new("Internal server error")
        })?
        .ok_or_else(|| ServerFnError::new("User not found for verified token"))?;

    // Update user name
    kyomi_auth::user_service::update_user_name(&ctx.db, &user.user_id, &name)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update user name");
            ServerFnError::new("Internal server error")
        })?;

    // Mark user as verified
    kyomi_auth::user_service::mark_user_verified(&ctx.db, &email)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to mark user verified");
            ServerFnError::new("Internal server error")
        })?;

    // Accept terms
    kyomi_auth::user_service::update_terms_acceptance(
        &ctx.db,
        &user.user_id,
        kyomi_core::TERMS_VERSION,
        marketing_consent,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to update terms acceptance");
        ServerFnError::new("Internal server error")
    })?;

    // Store marketing consent in extra_metadata
    if marketing_consent {
        kyomi_auth::user_service::update_extra_metadata(
            &ctx.db,
            &user.user_id,
            &serde_json::json!({"marketing_consent": true}),
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update extra metadata");
            ServerFnError::new("Internal server error")
        })?;
    }

    // Create personal workspace
    kyomi_auth::user_service::create_workspace_for_user(
        &ctx.db,
        &user.user_id,
        Some(&name),
        &email,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create workspace");
        ServerFnError::new("Internal server error")
    })?;

    // Generate WebAuthn registration challenge
    let user_unique_id = webauthn_user_id(&email);
    let display_name = &name;

    // Get existing credential IDs to exclude (prevent re-registration)
    let creds = kyomi_auth::user_service::get_passkey_credentials(&ctx.db, &user.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let mut exclude_ids = Vec::new();
    for (cred_id_b64, _) in &creds {
        if let Ok(bytes) = URL_SAFE_NO_PAD.decode(cred_id_b64) {
            exclude_ids.push(CredentialID::from(bytes));
        }
    }
    let exclude_opt = if exclude_ids.is_empty() {
        None
    } else {
        Some(exclude_ids)
    };

    let (ccr, reg_state) = kyomi_auth::webauthn::start_registration(
        webauthn,
        user_unique_id,
        &email,
        display_name,
        exclude_opt,
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Store challenge in KV
    let challenge_id = kyomi_auth::redis_ops::generate_token();
    let reg_state_json = serde_json::to_value(&reg_state)
        .map_err(|e| ServerFnError::new(format!("Serialize reg state: {e}")))?;

    let challenge_data = serde_json::json!({
        "registration_state": reg_state_json,
        "email": email,
        "user_name": display_name,
        "user_id": user.user_id,
        "device_name": "Unknown Device",
        "is_signup": true,
    });
    kyomi_auth::redis_ops::store_webauthn_challenge(&kv, &challenge_id, &challenge_data)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let creation_challenge = serde_json::to_string(&ccr)
        .map_err(|e| ServerFnError::new(format!("Serialize creation challenge: {e}")))?;

    tracing::info!(
        email = %email,
        user_id = %user.user_id,
        "Passkey signup token verified, WebAuthn challenge generated"
    );

    Ok(PasskeyRegisterStartResult {
        challenge_id,
        creation_challenge,
    })
}

/// Verify a passkey recovery token and generate a WebAuthn registration challenge.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/passkeys/recovery/verify` from the React backend.
///
/// Flow:
/// 1. Verify the recovery token (same as `recovery_verify`)
/// 2. Generate WebAuthn registration challenge for the user
/// 3. Return challenge + email for display
///
/// After this, the client calls `passkey_register_complete()` with the credential.
#[server(prefix = "/leptos-api")]
pub async fn passkey_recovery_verify(
    token: String,
) -> Result<PasskeyRecoveryVerifyResult, ServerFnError> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use webauthn_rs::prelude::*;

    let ctx = extract_context()?;

    let webauthn = ctx
        .webauthn
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebAuthn not configured"))?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Verify the recovery token
    let email = kyomi_auth::token_service::verify_verification_token(
        &ctx.db,
        &token,
        "recovery",
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to verify recovery token");
        ServerFnError::new("Internal server error")
    })?;

    let Some(email) = email else {
        return Ok(PasskeyRecoveryVerifyResult::Error {
            message: "Invalid or expired recovery link. Please request a new one.".to_string(),
        });
    };

    // Get user
    let user = kyomi_auth::user_service::get_user_by_email(&ctx.db, &email)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to look up user");
            ServerFnError::new("Internal server error")
        })?
        .ok_or_else(|| ServerFnError::new("User not found for recovery token"))?;

    // Generate WebAuthn registration challenge
    let user_unique_id = webauthn_user_id(&email);
    let display_name = user.name.as_deref().unwrap_or(&email);

    // Get existing credential IDs to exclude
    let creds = kyomi_auth::user_service::get_passkey_credentials(&ctx.db, &user.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let mut exclude_ids = Vec::new();
    for (cred_id_b64, _) in &creds {
        if let Ok(bytes) = URL_SAFE_NO_PAD.decode(cred_id_b64) {
            exclude_ids.push(CredentialID::from(bytes));
        }
    }
    let exclude_opt = if exclude_ids.is_empty() {
        None
    } else {
        Some(exclude_ids)
    };

    let (ccr, reg_state) = kyomi_auth::webauthn::start_registration(
        webauthn,
        user_unique_id,
        &email,
        display_name,
        exclude_opt,
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Store challenge in KV
    let challenge_id = kyomi_auth::redis_ops::generate_token();
    let reg_state_json = serde_json::to_value(&reg_state)
        .map_err(|e| ServerFnError::new(format!("Serialize reg state: {e}")))?;

    let challenge_data = serde_json::json!({
        "registration_state": reg_state_json,
        "email": email,
        "user_name": display_name,
        "user_id": user.user_id,
        "device_name": "Unknown Device",
        "is_signup": false,
    });
    kyomi_auth::redis_ops::store_webauthn_challenge(&kv, &challenge_id, &challenge_data)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let creation_challenge = serde_json::to_string(&ccr)
        .map_err(|e| ServerFnError::new(format!("Serialize creation challenge: {e}")))?;

    tracing::info!(
        email = %email,
        user_id = %user.user_id,
        "Passkey recovery token verified, WebAuthn challenge generated"
    );

    Ok(PasskeyRecoveryVerifyResult::Success {
        challenge_id,
        creation_challenge,
        email,
    })
}

// ---------------------------------------------------------------------------
// Private helpers (server-only)
// ---------------------------------------------------------------------------

/// Send a verification email in a background task (fire-and-forget).
///
/// Equivalent to `apps/server/src/helpers.rs::spawn_verification_email`.
/// Duplicated here because kyomi-ui cannot depend on the server crate
/// (that would create a circular dependency).
#[cfg(feature = "ssr")]
fn spawn_verification_email(email: String, name: String, url: String) {
    tokio::spawn(async move {
        let email_svc = kyomi_auth::email_service::EmailService::from_env();
        let sent = email_svc
            .send_verification_email(&email, &name, &url)
            .await;
        if sent {
            tracing::info!("Verification email sent to {email}");
        } else {
            tracing::warn!("Failed to send verification email to {email}");
        }
    });
}

/// Generate a WebAuthn user unique ID from email (matching Python / auth_passkeys.rs).
///
/// `sha256(email)[:16]` interpreted as a UUID — produces a stable, deterministic user handle.
#[cfg(feature = "ssr")]
fn webauthn_user_id(email: &str) -> webauthn_rs::prelude::Uuid {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(email.as_bytes());
    let hash = hasher.finalize();
    let bytes: [u8; 16] = hash[..16].try_into().expect("16 bytes");
    webauthn_rs::prelude::Uuid::from_bytes(bytes)
}

/// Get Passkey objects for authentication from user's stored credentials.
///
/// Mirrors `get_passkeys_for_auth` in `apps/server/src/routes/auth_passkeys.rs`.
#[cfg(feature = "ssr")]
async fn get_passkeys_for_auth(
    db: &kyomi_core::DbPool,
    user_id: &str,
) -> Result<Vec<webauthn_rs::prelude::Passkey>, leptos::prelude::ServerFnError> {
    let creds = kyomi_auth::user_service::get_passkey_credentials(db, user_id)
        .await
        .map_err(|e| leptos::prelude::ServerFnError::new(e.to_string()))?;

    let mut passkeys = Vec::new();
    for (_cred_id, cred_data) in &creds {
        if let Some(passkey_json) = cred_data.get("passkey") {
            if let Ok(passkey) =
                serde_json::from_value::<webauthn_rs::prelude::Passkey>(passkey_json.clone())
            {
                passkeys.push(passkey);
            }
        }
    }
    Ok(passkeys)
}

/// Update passkey credential usage after successful authentication.
///
/// Finds the matching passkey, updates its sign count and serialized data.
/// Fire-and-forget: logs a warning on failure rather than propagating errors.
#[cfg(feature = "ssr")]
async fn update_passkey_after_auth(
    db: &kyomi_core::DbPool,
    user_id: &str,
    credential_id_b64: &str,
    cred_id_bytes: &[u8],
    passkeys: &[webauthn_rs::prelude::Passkey],
    auth_result: &webauthn_rs::prelude::AuthenticationResult,
) {
    let updated_passkey = passkeys.iter().find(|pk| {
        let pk_cred_id: &[u8] = pk.cred_id().as_ref();
        pk_cred_id == cred_id_bytes
    });

    if let Some(pk) = updated_passkey {
        let mut updated_pk = pk.clone();
        updated_pk.update_credential(auth_result);
        let updated_json = serde_json::to_value(&updated_pk).unwrap_or_default();
        if let Err(e) = kyomi_auth::user_service::update_credential_usage(
            db,
            user_id,
            credential_id_b64,
            auth_result.counter(),
            &updated_json,
        )
        .await
        {
            tracing::warn!(
                user_id = %user_id,
                credential_id = %credential_id_b64,
                error = %e,
                "Failed to update credential usage after passkey auth"
            );
        }
    }
}

/// Extract the client IP from request headers.
///
/// Mirrors `apps/server/src/helpers.rs::extract_client_ip` — checks
/// `X-Real-IP`, then `X-Forwarded-For`, falling back to `"unknown"`.
///
/// Note: The canonical `extract_client_ip` in `helpers.rs` also accepts a
/// `peer_addr: Option<SocketAddr>` for TCP peer fallback. That parameter is
/// not available in Leptos server functions without additional extractor setup.
/// In production (behind nginx), `X-Real-IP` is always set, so this omission
/// is safe. In local dev without a reverse proxy, rate limiting will key all
/// requests to `"unknown"`.
#[cfg(feature = "ssr")]
fn extract_client_ip(headers: &axum::http::HeaderMap) -> String {
    use std::net::IpAddr;

    // 1. X-Real-IP — trustworthy: set by nginx from TCP peer ($remote_addr).
    if let Some(real_ip) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        let ip = real_ip.trim();
        if !ip.is_empty() && ip.parse::<IpAddr>().is_ok() {
            return ip.to_string();
        }
    }

    // 2. X-Forwarded-For — less reliable: nginx appends but doesn't replace,
    //    so clients can inject fake first entries. Use first entry as fallback.
    if let Some(xff) = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(first_ip) = xff.split(',').next() {
            let ip = first_ip.trim();
            if !ip.is_empty() && ip.parse::<IpAddr>().is_ok() {
                return ip.to_string();
            }
        }
    }

    "unknown".to_string()
}

/// Extract device info from request headers.
///
/// Mirrors `apps/server/src/helpers.rs::extract_device_info`.
#[cfg(feature = "ssr")]
fn extract_device_info(headers: &axum::http::HeaderMap) -> kyomi_auth::token_service::DeviceInfo {
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let ip_address = extract_client_ip(headers);

    let country_code = headers
        .get("cf-ipcountry")
        .and_then(|v| v.to_str().ok())
        .filter(|s| *s != "XX")
        .map(|s| s.to_uppercase());

    kyomi_auth::token_service::DeviceInfo {
        user_agent,
        ip_address: Some(ip_address),
        country_code,
        oauth_client_id: None,
    }
}
