// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the onboarding flow.
//!
//! These are public server functions (no `extract_auth()` call) used by the
//! welcome and onboarding pages:
//! - `accept_terms` — validates temp token, creates user/session, sets cookies
//!
//! Mirrors `POST /auth/accept-terms` in
//! `apps/server/src/routes/auth_google_oauth.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::extract_context;

/// Result of the terms acceptance flow.
///
/// Uses typed variants so the Leptos UI can pattern-match on outcomes.
/// Cookies are set via `ResponseOptions` for the `Success` variant.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AcceptTermsResult {
    /// Terms accepted, session created, cookies set.
    Success,
    /// Error — display message to the user and allow retry.
    Error { message: String },
}

/// Accept terms of service, completing the signup or re-acceptance flow.
///
/// Public endpoint — no authentication required. The user is identified
/// by a temporary token stored in Redis during the OAuth callback flow.
///
/// Flow:
/// 1. Try pending signup (new user via Google OAuth):
///    - Create user account (verified)
///    - Store OAuth data
///    - Update terms acceptance
///    - Register `google_oauth` auth method
///    - Create personal workspace
///    - Create authenticated session, set cookies
/// 2. Try pending terms (existing user needing re-acceptance):
///    - Update terms acceptance
///    - Create authenticated session, set cookies
/// 3. If neither found, return error (expired/invalid token)
///
/// Mirrors `POST /auth/accept-terms` in
/// `apps/server/src/routes/auth_google_oauth.rs`.
#[server(prefix = "/leptos-api")]
pub async fn accept_terms(
    temp_token: String,
    marketing_consent: bool,
) -> Result<AcceptTermsResult, ServerFnError> {
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

    // ── Try pending signup first (new user via Google OAuth) ─────────────
    if let Some(signup_data) =
        kyomi_auth::redis_ops::get_pending_signup(&kv, &temp_token)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get pending signup");
                ServerFnError::new("Internal server error")
            })?
    {
        let email = signup_data["email"]
            .as_str()
            .ok_or_else(|| ServerFnError::new("Missing email in signup data"))?;
        let name = signup_data["name"].as_str().unwrap_or("");

        // Create user (verified = true — OAuth means email is verified by Google)
        let user = kyomi_auth::user_service::create_user(&ctx.db, email, Some(name), true)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to create user");
                ServerFnError::new("Internal server error")
            })?;

        // TODO: Admin notification (Slack + email) for new signups.
        // The REST handler calls `admin_notify::notify_signup` here, but that
        // module lives in the server crate which kyomi-ui cannot depend on
        // (circular dependency). Move notification to kyomi-auth or a shared crate.

        // Store OAuth data
        if let Some(oauth_data_json) = signup_data.get("oauth_data") {
            let oauth = kyomi_auth::google_oauth::OAuthData {
                google_id: oauth_data_json
                    .get("google_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                oauth_provider: oauth_data_json
                    .get("oauth_provider")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                picture: oauth_data_json
                    .get("picture")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                last_oauth_login: Some(chrono::Utc::now().to_rfc3339()),
                ..Default::default()
            };
            let encrypted =
                kyomi_auth::google_oauth::build_oauth_data(&oauth, &encryption_key).map_err(
                    |e| {
                        tracing::error!(error = %e, "Failed to encrypt OAuth data");
                        ServerFnError::new("Internal server error")
                    },
                )?;
            kyomi_auth::user_service::update_user_oauth_data(
                &ctx.db,
                &user.user_id,
                Some(&encrypted),
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to store OAuth data");
                ServerFnError::new("Internal server error")
            })?;
        }

        // Update terms acceptance
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

        // Register google_oauth auth method
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
            tracing::error!(error = %e, "Failed to register auth method");
            ServerFnError::new("Internal server error")
        })?;

        // Create personal workspace
        kyomi_auth::user_service::create_workspace_for_user(
            &ctx.db,
            &user.user_id,
            Some(name),
            email,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create workspace");
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
        let response_options = expect_context::<leptos_axum::ResponseOptions>();
        for (name, value) in sess.cookie_headers.iter() {
            if name == axum::http::header::SET_COOKIE {
                response_options.append_header(name.clone(), value.clone());
            }
        }

        return Ok(AcceptTermsResult::Success);
    }

    // ── Try pending terms (existing user) ────────────────────────────────
    if let Some(terms_data) =
        kyomi_auth::redis_ops::get_pending_terms(&kv, &temp_token)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get pending terms");
                ServerFnError::new("Internal server error")
            })?
    {
        let user_id = terms_data["user_id"]
            .as_str()
            .ok_or_else(|| ServerFnError::new("Missing user_id in terms data"))?;

        // Update terms acceptance
        kyomi_auth::user_service::update_terms_acceptance(
            &ctx.db,
            user_id,
            kyomi_core::TERMS_VERSION,
            marketing_consent,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update terms acceptance");
            ServerFnError::new("Internal server error")
        })?;

        // Get fresh user for session creation
        let user = kyomi_auth::user_service::get_user_by_id(&ctx.db, user_id)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get user");
                ServerFnError::new("Internal server error")
            })?
            .ok_or_else(|| ServerFnError::new("User not found"))?;

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
        let response_options = expect_context::<leptos_axum::ResponseOptions>();
        for (name, value) in sess.cookie_headers.iter() {
            if name == axum::http::header::SET_COOKIE {
                response_options.append_header(name.clone(), value.clone());
            }
        }

        return Ok(AcceptTermsResult::Success);
    }

    // ── Neither found — token expired or invalid ─────────────────────────
    Ok(AcceptTermsResult::Error {
        message: "Invalid or expired token. Please try signing up again.".to_string(),
    })
}

// ---------------------------------------------------------------------------
// Helpers (duplicated from auth.rs — these should be extracted to a shared
// module; see auth.rs for the canonical versions)
// ---------------------------------------------------------------------------

/// Extract the client IP address from request headers.
///
/// Duplicated from `server_fns::auth` (private). When a shared helpers module
/// is created, both modules should use that instead.
#[cfg(feature = "ssr")]
fn extract_client_ip(headers: &axum::http::HeaderMap) -> String {
    use std::net::IpAddr;

    // 1. X-Real-IP — trustworthy: set by nginx from TCP peer ($remote_addr).
    if let Some(real_ip) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        let ip = real_ip.trim();
        if ip.parse::<IpAddr>().is_ok() {
            return ip.to_string();
        }
    }

    // 2. X-Forwarded-For — first entry
    if let Some(xff) = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(first) = xff.split(',').next() {
            let ip = first.trim();
            if ip.parse::<IpAddr>().is_ok() {
                return ip.to_string();
            }
        }
    }

    "unknown".to_string()
}

/// Extract device info from request headers.
///
/// Duplicated from `server_fns::auth` (private). When a shared helpers module
/// is created, both modules should use that instead.
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
