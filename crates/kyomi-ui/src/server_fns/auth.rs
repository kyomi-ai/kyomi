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
use super::{extract_context, IntoServerFnError};

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

/// Result of a Google account link callback.
///
/// Used by the `/auth/google/link-callback` route. The user is already
/// authenticated; this flow links their Google account for BigQuery access.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum GoogleLinkCallbackResult {
    /// Google account successfully linked.
    Success {
        google_email: String,
        bigquery_access: String,
    },
    /// Error during account linking.
    Error { message: String },
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
/// Delegates all orchestration to `kyomi_auth::auth_service::login_with_password_service`.
#[server(prefix = "/leptos-api")]
pub async fn login_with_password(
    email: String,
    password: String,
    totp_code: Option<String>,
) -> Result<LoginResult, ServerFnError> {
    use kyomi_auth::auth_service::{login_with_password_service, LoginServiceResult};

    let ctx = extract_context()?;
    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;
    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    let email = email.to_lowercase();
    let email = email.trim();
    let ip = extract_client_ip(&headers);
    let device = extract_device_info(&headers);

    let result = login_with_password_service(kyomi_auth::auth_service::LoginWithPasswordParams {
        db: &ctx.db,
        kv: &kv,
        jwt_secret: &ctx.config.jwt_secret,
        email,
        password: &password,
        totp_code: totp_code.as_deref(),
        ip: &ip,
        device: &device,
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "login_with_password_service error");
        ServerFnError::new("Internal server error")
    })?;

    match result {
        LoginServiceResult::Success(sess) => {
            set_session_cookies(&sess);
            Ok(LoginResult::Success {
                user_id: sess.user.user_id,
                email: sess.user.email,
                name: sess.user.name.unwrap_or_default(),
            })
        }
        LoginServiceResult::TwoFactorRequired { email } => {
            Ok(LoginResult::TwoFactorRequired { email })
        }
        LoginServiceResult::VerificationRequired { email } => {
            Ok(LoginResult::VerificationRequired { email })
        }
        LoginServiceResult::RateLimited { retry_after_secs } => {
            Ok(LoginResult::RateLimited { retry_after_secs })
        }
        LoginServiceResult::InvalidCredentials => Ok(LoginResult::Error {
            message: "Invalid credentials".to_string(),
        }),
    }
}

/// Start the signup flow.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/signup/start` in `apps/server/src/routes/auth_password.rs`.
///
/// Delegates all orchestration to `kyomi_auth::auth_service::signup_start_service`.
#[server(prefix = "/leptos-api")]
pub async fn signup_start(
    email: String,
    name: Option<String>,
    password: Option<String>,
) -> Result<SignupResult, ServerFnError> {
    use kyomi_auth::auth_service::{signup_start_service, SignupStartServiceResult};

    let ctx = extract_context()?;
    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;
    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    let email = email.to_lowercase();
    let email = email.trim();
    let ip = extract_client_ip(&headers);
    let device = extract_device_info(&headers);
    let success_message = "If this email is not already registered, a verification link has been sent. Please check your inbox.";

    let result = signup_start_service(kyomi_auth::auth_service::SignupStartParams {
        db: &ctx.db,
        kv: &kv,
        jwt_secret: &ctx.config.jwt_secret,
        email,
        name: name.as_deref(),
        password: password.as_deref(),
        ip: &ip,
        device: &device,
        self_hosted: ctx.config.self_hosted,
        smtp_configured: ctx.config.smtp_configured(),
        frontend_url: &ctx.config.frontend_url,
        slack_feedback_webhook_url: ctx.config.slack_feedback_webhook_url.as_deref(),
        support_email: &ctx.config.support_email,
        config: Some(&ctx.config),
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "signup_start_service error");
        ServerFnError::new("Internal server error")
    })?;

    match result {
        SignupStartServiceResult::AccountCreated(sess) => {
            set_session_cookies(&sess);
            Ok(SignupResult::AccountCreated {
                redirect: "/".to_string(),
            })
        }
        SignupStartServiceResult::VerificationRequired => Ok(SignupResult::VerificationRequired {
            message: success_message.to_string(),
        }),
        SignupStartServiceResult::RateLimited { retry_after_secs } => {
            Ok(SignupResult::RateLimited { retry_after_secs })
        }
        SignupStartServiceResult::Error { message } => Ok(SignupResult::Error { message }),
    }
}

/// Complete the signup flow after email verification.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/signup/complete` in `apps/server/src/routes/auth_password.rs`.
///
/// Delegates all orchestration to `kyomi_auth::auth_service::signup_complete_service`.
#[server(prefix = "/leptos-api")]
pub async fn signup_complete(
    token: String,
    name: String,
    password: String,
    terms_accepted: bool,
    marketing_consent: bool,
) -> Result<SignupCompleteResult, ServerFnError> {
    use kyomi_auth::auth_service::{signup_complete_service, SignupCompleteServiceResult};

    let ctx = extract_context()?;
    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;
    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;
    let device = extract_device_info(&headers);

    let result = signup_complete_service(kyomi_auth::auth_service::SignupCompleteParams {
        db: &ctx.db,
        kv: &kv,
        jwt_secret: &ctx.config.jwt_secret,
        token: &token,
        name: &name,
        password: &password,
        terms_accepted,
        marketing_consent,
        device: &device,
        config: Some(&ctx.config),
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "signup_complete_service error");
        ServerFnError::new("Internal server error")
    })?;

    match result {
        SignupCompleteServiceResult::Success(sess) => {
            set_session_cookies(&sess);
            Ok(SignupCompleteResult::Success {
                user_id: sess.user.user_id,
            })
        }
        SignupCompleteServiceResult::InvalidToken => Ok(SignupCompleteResult::Error {
            message: "Invalid or expired signup link. Please request a new one.".to_string(),
        }),
        SignupCompleteServiceResult::Error { message } => {
            Ok(SignupCompleteResult::Error { message })
        }
    }
}

/// Handle Google OAuth callback — exchange code for tokens and log in or start signup.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/google/callback` in `apps/server/src/routes/auth_google_oauth.rs`.
///
/// Delegates all orchestration to `kyomi_auth::auth_service::google_oauth_callback_service`.
#[server(prefix = "/leptos-api")]
pub async fn google_oauth_callback(
    code: String,
    state: Option<String>,
) -> Result<GoogleCallbackResult, ServerFnError> {
    use kyomi_auth::auth_service::{google_oauth_callback_service, GoogleOAuthServiceResult};

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

    let ip = extract_client_ip(&headers);
    let device = extract_device_info(&headers);

    let result = google_oauth_callback_service(kyomi_auth::auth_service::GoogleOAuthCallbackParams {
        db: &ctx.db,
        kv: &kv,
        jwt_secret: &ctx.config.jwt_secret,
        code: &code,
        state: state.as_deref(),
        ip: &ip,
        device: &device,
        client_id: &client_id,
        client_secret: &client_secret,
        frontend_url: &ctx.config.frontend_url,
        encryption_key: &encryption_key,
        config: Some(&ctx.config),
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "google_oauth_callback_service error");
        ServerFnError::new("Internal server error")
    })?;

    match result {
        GoogleOAuthServiceResult::PendingTerms { redirect_url } => {
            Ok(GoogleCallbackResult::PendingTerms { redirect_url })
        }
        GoogleOAuthServiceResult::Success {
            session,
            oauth_continue,
        } => {
            set_session_cookies(&session);
            Ok(GoogleCallbackResult::Success { oauth_continue })
        }
        GoogleOAuthServiceResult::RateLimited { retry_after_secs } => {
            Ok(GoogleCallbackResult::RateLimited { retry_after_secs })
        }
    }
}

/// Handle Google account link callback — exchange code for tokens and store OAuth data.
///
/// Authenticated endpoint — the user must be signed in.
/// Mirrors `POST /auth/google/link-callback` in
/// `apps/server/src/routes/auth_google_oauth.rs`.
///
/// Delegates all orchestration to `kyomi_auth::auth_service::google_link_callback_service`.
/// The WebSocket notification is not fired here — the client that initiated the
/// link will redirect to settings and reload, so the WS notification is
/// superfluous on this code path.
#[server(prefix = "/leptos-api")]
pub async fn google_link_callback(
    code: String,
    state: String,
) -> Result<GoogleLinkCallbackResult, ServerFnError> {
    use kyomi_auth::auth_service::{google_link_callback_service, GoogleLinkCallbackParams};

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

    let ip = extract_client_ip(&headers);

    let result = google_link_callback_service(GoogleLinkCallbackParams {
        db: &ctx.db,
        kv: &kv,
        encryption_key: &encryption_key,
        code: &code,
        state: &state,
        client_id: &client_id,
        client_secret: &client_secret,
        frontend_url: &ctx.config.frontend_url,
        ip: &ip,
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "google_link_callback_service error");
        ServerFnError::new(format!("{e}"))
    })?;

    Ok(GoogleLinkCallbackResult::Success {
        google_email: result.google_email,
        bigquery_access: result.bigquery_access,
    })
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

    if ctx.config.self_hosted && !ctx.config.smtp_configured() {
        return Ok(());
    }

    let email = email.to_lowercase().trim().to_string();
    let ip = extract_client_ip(&headers);

    let result = kyomi_auth::auth_service::resend_verification_service(
        &ctx.db, &kv, &ip, &email,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "resend_verification_service error");
        ServerFnError::new("Internal server error")
    })?;

    if let Some(r) = result {
        let verification_url = format!(
            "{}/verify-email?token={}",
            ctx.config.frontend_url.trim_end_matches('/'),
            r.raw_token
        );
        tokio::spawn(async move {
            let email_svc = kyomi_auth::email_service::EmailService::from_env();
            let sent = email_svc
                .send_verification_email(&email, &r.user_name, &verification_url)
                .await;
            if sent {
                tracing::info!("Verification email sent to {email}");
            } else {
                tracing::warn!("Failed to send verification email to {email}");
            }
        });
    }

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
/// Always returns `Ok(())` to prevent email enumeration. Rate-limits and
/// delegates email dispatch to a background task inline.
#[server(prefix = "/leptos-api")]
pub async fn recovery_start(email: String) -> Result<(), ServerFnError> {
    let ctx = extract_context()?;

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

    let ip = extract_client_ip(&headers);
    let email = email.to_lowercase();
    let email = email.trim();

    let result = kyomi_auth::auth_service::recovery_start_service(
        &ctx.db, &kv, &ip, email,
    )
    .await
    .into_sfn()?;

    if let Some(r) = result {
        let recovery_url = format!(
            "{}/account/recover/complete?token={}",
            ctx.config.frontend_url.trim_end_matches('/'),
            r.raw_token
        );
        let email_clone = email.to_string();
        tokio::spawn(async move {
            let email_svc = kyomi_auth::email_service::EmailService::from_env();
            let sent = email_svc
                .send_account_recovery(&email_clone, &r.user_name, &recovery_url)
                .await;
            if sent {
                tracing::info!("Account recovery email sent to {email_clone}");
            } else {
                tracing::warn!("Failed to send account recovery email to {email_clone}");
                tracing::info!("ACCOUNT RECOVERY LINK for {email_clone}: {recovery_url}");
            }
        });
    }

    Ok(())
}

/// Verify a recovery token and create a short-lived recovery session.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/recovery/verify` in `apps/server/src/routes/auth_recovery.rs`.
///
/// Delegates all orchestration to `kyomi_auth::auth_service::recovery_verify_service`.
#[server(prefix = "/leptos-api")]
pub async fn recovery_verify(
    token: String,
) -> Result<RecoveryVerifyResult, ServerFnError> {
    use kyomi_auth::auth_service::{recovery_verify_service, RecoveryVerifyServiceResult};

    let ctx = extract_context()?;
    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    let result = recovery_verify_service(&ctx.db, &kv, &token)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "recovery_verify_service error");
            ServerFnError::new("Internal server error")
        })?;

    match result {
        RecoveryVerifyServiceResult::Success {
            recovery_session_id,
            has_passkeys,
        } => Ok(RecoveryVerifyResult::Success {
            recovery_session_id,
            has_passkeys,
        }),
        RecoveryVerifyServiceResult::InvalidToken => {
            tracing::warn!("Account recovery/verify: invalid or expired token");
            Ok(RecoveryVerifyResult::Error {
                message: "Invalid or expired recovery link. Please request a new one.".into(),
            })
        }
        RecoveryVerifyServiceResult::AccountNotVerified => Ok(RecoveryVerifyResult::Error {
            message: "Account is not verified. Please complete signup first.".into(),
        }),
    }
}

/// Set a new password using a recovery session, completing the recovery flow.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/recovery/set-password` in `apps/server/src/routes/auth_recovery.rs`.
///
/// Delegates all orchestration to `kyomi_auth::auth_service::recovery_set_password_service`.
#[server(prefix = "/leptos-api")]
pub async fn recovery_set_password(
    recovery_session_id: String,
    new_password: String,
) -> Result<RecoverySetPasswordResult, ServerFnError> {
    use kyomi_auth::auth_service::{
        recovery_set_password_service, RecoverySetPasswordServiceResult,
    };

    let ctx = extract_context()?;
    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;
    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;
    let device = extract_device_info(&headers);

    let result = recovery_set_password_service(
        &ctx.db,
        &kv,
        &ctx.config.jwt_secret,
        &recovery_session_id,
        &new_password,
        &device,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "recovery_set_password_service error");
        ServerFnError::new("Internal server error")
    })?;

    match result {
        RecoverySetPasswordServiceResult::Success(sess) => {
            set_session_cookies(&sess);
            Ok(RecoverySetPasswordResult::Success)
        }
        RecoverySetPasswordServiceResult::Error { message } => {
            Ok(RecoverySetPasswordResult::Error { message })
        }
        RecoverySetPasswordServiceResult::InvalidSession => Ok(RecoverySetPasswordResult::Error {
            message: "Invalid or expired recovery session. Please start the recovery process again.".into(),
        }),
    }
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
            .into_sfn()?;

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
        .into_sfn()?;

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
/// Delegates all orchestration to `kyomi_auth::auth_service::passkey_login_complete_service`.
#[server(prefix = "/leptos-api")]
pub async fn passkey_login_complete(
    challenge_id: String,
    assertion_json: String,
) -> Result<LoginResult, ServerFnError> {
    use kyomi_auth::auth_service::{passkey_login_complete_service, PasskeyLoginServiceResult};

    let ctx = extract_context()?;
    let webauthn = ctx
        .webauthn
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebAuthn not configured"))?;
    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;
    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;
    let ip = extract_client_ip(&headers);
    let device = extract_device_info(&headers);

    let result = passkey_login_complete_service(kyomi_auth::auth_service::PasskeyLoginCompleteParams {
        db: &ctx.db,
        kv: &kv,
        jwt_secret: &ctx.config.jwt_secret,
        webauthn,
        challenge_id: &challenge_id,
        assertion_json: &assertion_json,
        ip: &ip,
        device: &device,
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "passkey_login_complete_service error");
        ServerFnError::new("Internal server error")
    })?;

    match result {
        PasskeyLoginServiceResult::Success(sess) => {
            set_session_cookies(&sess);
            Ok(LoginResult::Success {
                user_id: sess.user.user_id,
                email: sess.user.email,
                name: sess.user.name.unwrap_or_default(),
            })
        }
        PasskeyLoginServiceResult::RateLimited { retry_after_secs } => {
            Ok(LoginResult::RateLimited { retry_after_secs })
        }
        PasskeyLoginServiceResult::VerificationRequired { email } => {
            Ok(LoginResult::VerificationRequired { email })
        }
        PasskeyLoginServiceResult::InvalidCredentials => Ok(LoginResult::Error {
            message: "Invalid credentials".to_string(),
        }),
        PasskeyLoginServiceResult::InvalidChallenge => {
            Err(ServerFnError::new("Invalid or expired challenge"))
        }
        PasskeyLoginServiceResult::AuthFailed => {
            Err(ServerFnError::new("Authentication failed"))
        }
    }
}

/// Start passkey registration — create or find user and generate a WebAuthn challenge.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/passkeys/register/start` in `apps/server/src/routes/auth_passkeys.rs`.
///
/// Delegates all orchestration to `kyomi_auth::auth_service::passkey_register_start_service`.
/// For existing unverified users: resends verification email.
#[server(prefix = "/leptos-api")]
pub async fn passkey_register_start(
    email: String,
    name: Option<String>,
    device_name: String,
) -> Result<PasskeyRegisterStartResult, ServerFnError> {
    use kyomi_auth::auth_service::{
        passkey_register_start_service, PasskeyRegisterStartServiceResult,
    };

    let ctx = extract_context()?;
    let webauthn = ctx
        .webauthn
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebAuthn not configured"))?;
    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;
    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;
    let ip = extract_client_ip(&headers);

    let email_lower = email.to_lowercase();
    let email_trimmed = email_lower.trim();
    let name_str = name.unwrap_or_default();
    let device_name_str = if device_name.trim().is_empty() {
        "Unknown Device".to_string()
    } else {
        device_name.trim().to_string()
    };

    let result = passkey_register_start_service(kyomi_auth::auth_service::PasskeyRegisterStartParams {
        db: &ctx.db,
        kv: &kv,
        webauthn,
        email: email_trimmed,
        name: &name_str,
        device_name: &device_name_str,
        ip: &ip,
        self_hosted: ctx.config.self_hosted,
        smtp_configured: ctx.config.smtp_configured(),
        frontend_url: &ctx.config.frontend_url,
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "passkey_register_start_service error");
        ServerFnError::new("Internal server error")
    })?;

    match result {
        PasskeyRegisterStartServiceResult::Success {
            challenge_id,
            creation_challenge,
        } => Ok(PasskeyRegisterStartResult {
            challenge_id,
            creation_challenge,
        }),
        PasskeyRegisterStartServiceResult::RateLimited { retry_after_secs } => {
            Err(ServerFnError::new(format!(
                "Rate limited. Try again in {retry_after_secs} seconds"
            )))
        }
        PasskeyRegisterStartServiceResult::UnverifiedEmail => Err(ServerFnError::new(
            "Please verify your email before registering a passkey.",
        )),
        PasskeyRegisterStartServiceResult::VerificationEmailSent => Err(ServerFnError::new(
            "Please check your email to verify your account before registering a passkey.",
        )),
    }
}

/// Complete passkey registration — verify the browser credential and store it.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/passkeys/register/complete` in `apps/server/src/routes/auth_passkeys.rs`.
///
/// Delegates all orchestration to `kyomi_auth::auth_service::passkey_register_complete_service`.
#[server(prefix = "/leptos-api")]
pub async fn passkey_register_complete(
    challenge_id: String,
    credential_json: String,
) -> Result<LoginResult, ServerFnError> {
    let ctx = extract_context()?;
    let webauthn = ctx
        .webauthn
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebAuthn not configured"))?;
    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;
    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;
    let device = extract_device_info(&headers);

    let sess = kyomi_auth::auth_service::passkey_register_complete_service(
        &ctx.db,
        &kv,
        &ctx.config.jwt_secret,
        webauthn,
        &challenge_id,
        &credential_json,
        &device,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "passkey_register_complete_service error");
        ServerFnError::new("Internal server error")
    })?;

    set_session_cookies(&sess);
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
/// Delegates all orchestration to `kyomi_auth::auth_service::passkey_signup_complete_service`.
#[server(prefix = "/leptos-api")]
pub async fn passkey_signup_complete(
    token: String,
    name: String,
    terms_accepted: bool,
    marketing_consent: bool,
) -> Result<PasskeyRegisterStartResult, ServerFnError> {
    let ctx = extract_context()?;
    let webauthn = ctx
        .webauthn
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebAuthn not configured"))?;
    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    let result = kyomi_auth::auth_service::passkey_signup_complete_service(
        kyomi_auth::auth_service::PasskeySignupCompleteParams {
            db: &ctx.db,
            kv: &kv,
            webauthn,
            token: &token,
            name: &name,
            terms_accepted,
            marketing_consent,
            config: Some(&ctx.config),
        },
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "passkey_signup_complete_service error");
        ServerFnError::new("Internal server error")
    })?;

    match result {
        Ok((challenge_id, creation_challenge)) => Ok(PasskeyRegisterStartResult {
            challenge_id,
            creation_challenge,
        }),
        Err(message) => Err(ServerFnError::new(message)),
    }
}

/// Verify a passkey recovery token and generate a WebAuthn registration challenge.
///
/// Public endpoint — no authentication required.
/// Mirrors `POST /auth/passkeys/recovery/verify` from the React backend.
///
/// Delegates all orchestration to `kyomi_auth::auth_service::passkey_recovery_verify_service`.
#[server(prefix = "/leptos-api")]
pub async fn passkey_recovery_verify(
    token: String,
) -> Result<PasskeyRecoveryVerifyResult, ServerFnError> {
    let ctx = extract_context()?;
    let webauthn = ctx
        .webauthn
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebAuthn not configured"))?;
    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    let result = kyomi_auth::auth_service::passkey_recovery_verify_service(
        &ctx.db,
        &kv,
        webauthn,
        &token,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "passkey_recovery_verify_service error");
        ServerFnError::new("Internal server error")
    })?;

    match result {
        Ok((challenge_id, creation_challenge, email)) => {
            Ok(PasskeyRecoveryVerifyResult::Success {
                challenge_id,
                creation_challenge,
                email,
            })
        }
        Err(message) => Ok(PasskeyRecoveryVerifyResult::Error { message }),
    }
}

// ---------------------------------------------------------------------------
// Private helpers (server-only)
// ---------------------------------------------------------------------------

/// Apply session cookies to the current HTTP response.
///
/// Reads `HeaderMap` from the `AuthenticatedSession` and appends each
/// `Set-Cookie` header via `ResponseOptions`. Must be called from the server_fn
/// layer — `ResponseOptions` is a Leptos/Axum concern and cannot live in the
/// service layer.
#[cfg(feature = "ssr")]
fn set_session_cookies(sess: &kyomi_auth::session::AuthenticatedSession) {
    use leptos::prelude::expect_context;
    use leptos_axum::ResponseOptions;

    let opts = expect_context::<ResponseOptions>();
    for value in sess.cookie_headers.get_all(axum::http::header::SET_COOKIE) {
        opts.append_header(axum::http::header::SET_COOKIE, value.clone());
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
pub(crate) fn extract_client_ip(headers: &axum::http::HeaderMap) -> String {
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
        && let Some(first_ip) = xff.split(',').next()
    {
        let ip = first_ip.trim();
        if !ip.is_empty() && ip.parse::<IpAddr>().is_ok() {
            return ip.to_string();
        }
    }

    "unknown".to_string()
}

/// Extract device info from request headers.
///
/// Mirrors `apps/server/src/helpers.rs::extract_device_info`.
#[cfg(feature = "ssr")]
pub(crate) fn extract_device_info(headers: &axum::http::HeaderMap) -> kyomi_auth::token_service::DeviceInfo {
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
