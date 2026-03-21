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

                // TODO: Admin notification (Slack + email) for new SaaS signups.
                // The REST handler calls `admin_notify::notify_signup` here, but that
                // module lives in the server crate which kyomi-ui cannot depend on
                // (circular dependency). Move notification to kyomi-auth or a shared crate.
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
