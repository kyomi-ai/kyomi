// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for unauthenticated auth flows.
//!
//! These are public server functions (no `extract_auth()` call) used by the
//! login page and auth configuration:
//! - `GET  /api/v1/auth/config` -> `get_auth_config()`
//! - `POST /api/v1/auth/login`  -> `login_with_password()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/auth.rs`
//! and `apps/server/src/routes/auth_password.rs`.

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

// ---------------------------------------------------------------------------
// Private helpers (server-only)
// ---------------------------------------------------------------------------

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
