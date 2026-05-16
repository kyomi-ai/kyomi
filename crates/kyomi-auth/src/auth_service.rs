// SPDX-License-Identifier: AGPL-3.0-or-later

//! Auth orchestration service functions — extracted from Leptos server_fns.
//!
//! These functions contain the business logic that was previously inlined in
//! `crates/kyomi-ui/src/server_fns/auth.rs`. Server functions are now thin
//! wrappers that delegate to these service functions and apply HTTP concerns
//! (cookie setting via `ResponseOptions`) to the returned results.
//!
//! All functions take `&DbPool` as the first argument and return
//! `kyomi_core::Result<T>`. KV, config, and encryption key args follow.

use kyomi_core::{DbPool, KVPool};

use crate::rate_limiter::RateLimitResult;
use crate::session::{create_authenticated_session, AuthenticatedSession};
use crate::token_service::DeviceInfo;

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

/// Outcome of `login_with_password_service`.
pub enum LoginServiceResult {
    /// Authenticated successfully — server_fn should set cookies from `session`.
    Success(Box<AuthenticatedSession>),
    /// TOTP challenge needed.
    TwoFactorRequired { email: String },
    /// Email not yet verified.
    VerificationRequired { email: String },
    /// Rate limited.
    RateLimited { retry_after_secs: u64 },
    /// Invalid credentials or other non-fatal error.
    InvalidCredentials,
}

/// Parameters for `login_with_password_service`.
pub struct LoginWithPasswordParams<'a> {
    pub db: &'a DbPool,
    pub kv: &'a KVPool,
    pub jwt_secret: &'a str,
    pub email: &'a str,
    pub password: &'a str,
    pub totp_code: Option<&'a str>,
    pub ip: &'a str,
    pub device: &'a DeviceInfo,
}

/// Full login-with-password orchestration.
///
/// Rate-limits, looks up user, verifies password, checks TOTP, and creates
/// an authenticated session. The caller (server_fn) applies HTTP cookies from
/// the returned `AuthenticatedSession`.
pub async fn login_with_password_service(
    params: LoginWithPasswordParams<'_>,
) -> kyomi_core::Result<LoginServiceResult> {
    let LoginWithPasswordParams { db, kv, jwt_secret, email, password, totp_code, ip, device } = params;
    // Rate limit
    let rate = crate::rate_limiter::check_rate_limit(kv, ip, "login", None).await?;
    if !rate.allowed {
        return Ok(LoginServiceResult::RateLimited {
            retry_after_secs: rate.retry_after_secs,
        });
    }

    // Look up user by email
    let user = match crate::user_service::get_user_by_email(db, email).await? {
        Some(u) => u,
        None => return Ok(LoginServiceResult::InvalidCredentials),
    };

    // Get password auth method
    let password_method =
        match crate::user_service::get_auth_method(db, &user.user_id, "password").await? {
            Some(m) => m,
            None => return Ok(LoginServiceResult::InvalidCredentials),
        };

    // Extract hash
    let hash = password_method
        .auth_data
        .get("hash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| kyomi_core::Error::Internal("Password auth method missing hash".into()))?;

    // Verify password
    if !crate::password::verify_password(password, hash)
        .map_err(|e| kyomi_core::Error::Internal(format!("Password verification error: {e}")))?
    {
        return Ok(LoginServiceResult::InvalidCredentials);
    }

    // Check email verification before TOTP (don't leak TOTP status for unverified accounts)
    if !user.verified {
        return Ok(LoginServiceResult::VerificationRequired {
            email: user.email.clone(),
        });
    }

    // Check TOTP
    let totp_method =
        crate::user_service::get_auth_method(db, &user.user_id, "totp").await?;
    if let Some(totp_method) = totp_method
        && totp_method.active
    {
        match totp_code {
            None => {
                return Ok(LoginServiceResult::TwoFactorRequired {
                    email: user.email.clone(),
                });
            }
            Some(code) => {
                let secret = totp_method
                    .auth_data
                    .get("secret")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        kyomi_core::Error::Internal("TOTP auth method missing secret".into())
                    })?;
                if !crate::totp::verify_code(secret, code) {
                    return Ok(LoginServiceResult::InvalidCredentials);
                }
            }
        }
    }

    // Create authenticated session
    let sess = create_authenticated_session(db, kv, jwt_secret, &user, device).await?;

    // Touch last_used on password auth method (best-effort)
    let _ = crate::user_service::touch_auth_method(db, &user.user_id, "password").await;

    Ok(LoginServiceResult::Success(Box::new(sess)))
}

// ---------------------------------------------------------------------------
// Signup
// ---------------------------------------------------------------------------

/// Outcome of `signup_start_service`.
pub enum SignupStartServiceResult {
    /// Self-hosted SMTP-less: account created, cookies should be set from `session`.
    AccountCreated(Box<AuthenticatedSession>),
    /// SaaS flow: verification email sent.
    VerificationRequired,
    /// Rate limited.
    RateLimited { retry_after_secs: u64 },
    /// Non-fatal error (validation, registration closed, etc.).
    Error { message: String },
}

/// Parameters for `signup_start_service`.
pub struct SignupStartParams<'a> {
    pub db: &'a DbPool,
    pub kv: &'a KVPool,
    pub jwt_secret: &'a str,
    pub email: &'a str,
    pub name: Option<&'a str>,
    pub password: Option<&'a str>,
    pub ip: &'a str,
    pub device: &'a DeviceInfo,
    pub self_hosted: bool,
    pub smtp_configured: bool,
    pub frontend_url: &'a str,
    pub slack_feedback_webhook_url: Option<&'a str>,
    pub support_email: &'a str,
    pub config: Option<&'a kyomi_core::Config>,
}

/// Full signup-start orchestration.
///
/// Two modes:
/// - Self-hosted SMTP-less: creates user directly and returns a session.
/// - SaaS: creates unverified user, sends verification email.
pub async fn signup_start_service(
    params: SignupStartParams<'_>,
) -> kyomi_core::Result<SignupStartServiceResult> {
    let SignupStartParams {
        db, kv, jwt_secret, email, name, password, ip, device,
        self_hosted, smtp_configured, frontend_url, slack_feedback_webhook_url,
        support_email, config,
    } = params;
    // Rate limit
    let rate = crate::rate_limiter::check_rate_limit(kv, ip, "signup", None).await?;
    if !rate.allowed {
        return Ok(SignupStartServiceResult::RateLimited {
            retry_after_secs: rate.retry_after_secs,
        });
    }

    let smtp_less_self_hosted = self_hosted && !smtp_configured;

    // Look up existing user
    let existing_user = crate::user_service::get_user_by_email(db, email).await?;

    // Self-hosted without SMTP: only first user or invited users may register
    if smtp_less_self_hosted
        && existing_user.is_none()
        && crate::user_service::has_any_users(db).await?
    {
        let pending =
            crate::workspace_service::get_pending_invitations_for_email(db, email).await?;
        if pending.is_empty() {
            return Ok(SignupStartServiceResult::Error {
                message: "Registration is closed. Ask your administrator to invite you."
                    .to_string(),
            });
        }
    }

    match existing_user {
        None => {
            if smtp_less_self_hosted {
                let result = signup_smtp_less_new_user(SmtpLessNewUserParams {
                    db, kv, jwt_secret, email, name, password, device, config,
                })
                .await?;
                Ok(result)
            } else {
                signup_saas_new_user(
                    db,
                    email,
                    frontend_url,
                    slack_feedback_webhook_url,
                    support_email,
                )
                .await?;
                Ok(SignupStartServiceResult::VerificationRequired)
            }
        }
        Some(user) if !user.verified => {
            if smtp_less_self_hosted {
                let result = signup_smtp_less_existing_unverified(SmtpLessExistingUnverifiedParams {
                    db, kv, jwt_secret, email, user_id: &user.user_id, name, password, device, config,
                })
                .await?;
                Ok(result)
            } else {
                // Resend verification email
                let raw_token = crate::token_service::create_verification_token(
                    db,
                    email,
                    "email_verification",
                )
                .await?;
                let signup_url = format!(
                    "{}/signup/complete?token={raw_token}",
                    frontend_url.trim_end_matches('/')
                );
                tracing::info!(
                    "Password signup link (resend) for {email}: {signup_url} (user_id={})",
                    user.user_id
                );
                let user_name = user.name.clone().unwrap_or_default();
                spawn_verification_email(email.to_string(), user_name, signup_url);
                Ok(SignupStartServiceResult::VerificationRequired)
            }
        }
        Some(_) => {
            // Verified user — return VerificationRequired to prevent email enumeration
            Ok(SignupStartServiceResult::VerificationRequired)
        }
    }
}

struct SmtpLessNewUserParams<'a> {
    db: &'a DbPool,
    kv: &'a KVPool,
    jwt_secret: &'a str,
    email: &'a str,
    name: Option<&'a str>,
    password: Option<&'a str>,
    device: &'a DeviceInfo,
    config: Option<&'a kyomi_core::Config>,
}

/// Inner helper: self-hosted SMTP-less signup for a brand new user.
async fn signup_smtp_less_new_user(
    params: SmtpLessNewUserParams<'_>,
) -> kyomi_core::Result<SignupStartServiceResult> {
    let SmtpLessNewUserParams { db, kv, jwt_secret, email, name, password, device, config } = params;
    let name_str = name.unwrap_or("").trim();
    let password_str = password.unwrap_or("");
    if name_str.is_empty() || password_str.is_empty() {
        return Ok(SignupStartServiceResult::Error {
            message: "Name and password are required for self-hosted signup".to_string(),
        });
    }
    if password_str.len() < 8 {
        return Ok(SignupStartServiceResult::Error {
            message: "Password must be at least 8 characters".to_string(),
        });
    }

    let user = crate::user_service::create_user(db, email, Some(name_str), true).await?;

    let hash = crate::password::hash_password(password_str)
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to hash password: {e}")))?;
    crate::user_service::upsert_auth_method(
        db,
        &user.user_id,
        "password",
        &serde_json::json!({"hash": hash}),
    )
    .await?;

    // Check for pending invitations
    let pending =
        crate::workspace_service::get_pending_invitations_for_email(db, email).await?;
    if let Some(inv) = pending.first() {
        crate::workspace_service::accept_invitation_for_user(
            db,
            &inv.invitation_id,
            &user.user_id,
        )
        .await?;
        crate::user_service::update_last_workspace(db, &user.user_id, &inv.workspace_id).await?;
    } else {
        crate::user_service::create_workspace_for_user(
            db,
            &user.user_id,
            Some(name_str),
            email,
            config,
        )
        .await?;
    }

    // Re-fetch user after workspace setup
    let user = crate::user_service::get_user_by_email(db, email)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("User not found after creation".into()))?;

    let sess = create_authenticated_session(db, kv, jwt_secret, &user, device).await?;
    tracing::info!(
        email = %email,
        user_id = %user.user_id,
        "Self-hosted SMTP-less: one-step signup complete"
    );
    Ok(SignupStartServiceResult::AccountCreated(Box::new(sess)))
}

struct SmtpLessExistingUnverifiedParams<'a> {
    db: &'a DbPool,
    kv: &'a KVPool,
    jwt_secret: &'a str,
    email: &'a str,
    user_id: &'a str,
    name: Option<&'a str>,
    password: Option<&'a str>,
    device: &'a DeviceInfo,
    config: Option<&'a kyomi_core::Config>,
}

/// Inner helper: self-hosted SMTP-less signup for an existing unverified user.
async fn signup_smtp_less_existing_unverified(
    params: SmtpLessExistingUnverifiedParams<'_>,
) -> kyomi_core::Result<SignupStartServiceResult> {
    let SmtpLessExistingUnverifiedParams {
        db, kv, jwt_secret, email, user_id, name, password, device, config,
    } = params;
    let name_str = name.unwrap_or("").trim();
    let password_str = password.unwrap_or("");
    if name_str.is_empty() || password_str.is_empty() {
        return Ok(SignupStartServiceResult::Error {
            message: "Name and password are required for self-hosted signup".to_string(),
        });
    }
    if password_str.len() < 8 {
        return Ok(SignupStartServiceResult::Error {
            message: "Password must be at least 8 characters".to_string(),
        });
    }

    let hash = crate::password::hash_password(password_str)
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to hash password: {e}")))?;
    crate::user_service::upsert_auth_method(
        db,
        user_id,
        "password",
        &serde_json::json!({"hash": hash}),
    )
    .await?;
    crate::user_service::update_user_name(db, user_id, name_str).await?;
    crate::user_service::mark_user_verified(db, email).await?;
    crate::user_service::create_workspace_for_user(
        db, user_id, Some(name_str), email, config,
    )
    .await?;

    let user = crate::user_service::get_user_by_email(db, email)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("User not found after signup".into()))?;

    let sess = create_authenticated_session(db, kv, jwt_secret, &user, device).await?;
    tracing::info!(
        email = %email,
        user_id = %user_id,
        "Self-hosted SMTP-less: one-step signup complete for existing unverified user"
    );
    Ok(SignupStartServiceResult::AccountCreated(Box::new(sess)))
}

/// Inner helper: SaaS signup for a brand new user.
async fn signup_saas_new_user(
    db: &DbPool,
    email: &str,
    frontend_url: &str,
    slack_feedback_webhook_url: Option<&str>,
    support_email: &str,
) -> kyomi_core::Result<()> {
    let user =
        crate::user_service::create_user(db, email, None, false).await?;

    let raw_token =
        crate::token_service::create_verification_token(db, email, "email_verification").await?;
    let signup_url = format!(
        "{}/signup/complete?token={raw_token}",
        frontend_url.trim_end_matches('/')
    );
    tracing::info!(
        "Password signup link for {email}: {signup_url} (user_id={})",
        user.user_id
    );

    spawn_verification_email(email.to_string(), String::new(), signup_url);

    // Admin notification (Slack + email) — fire-and-forget
    let notify_webhook = slack_feedback_webhook_url.map(|s| s.to_string());
    let notify_support = support_email.to_string();
    let notify_email = email.to_string();
    let notify_user_id = user.user_id.clone();
    tokio::spawn(async move {
        crate::notifications::notify_signup(
            notify_webhook.as_deref(),
            &notify_support,
            &notify_email,
            "",
            &notify_user_id,
        )
        .await;
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Signup complete (email verification token flow)
// ---------------------------------------------------------------------------

/// Outcome of `signup_complete_service`.
pub enum SignupCompleteServiceResult {
    /// Validation error (terms not accepted, bad password, etc.).
    Error { message: String },
    /// Invalid or expired signup token.
    InvalidToken,
    /// Account created and authenticated — server_fn should set cookies.
    Success(Box<AuthenticatedSession>),
}

/// Parameters for `signup_complete_service`.
pub struct SignupCompleteParams<'a> {
    pub db: &'a DbPool,
    pub kv: &'a KVPool,
    pub jwt_secret: &'a str,
    pub token: &'a str,
    pub name: &'a str,
    pub password: &'a str,
    pub terms_accepted: bool,
    pub marketing_consent: bool,
    pub device: &'a DeviceInfo,
    pub config: Option<&'a kyomi_core::Config>,
}

/// Full signup-complete orchestration (email verification token flow).
pub async fn signup_complete_service(
    params: SignupCompleteParams<'_>,
) -> kyomi_core::Result<SignupCompleteServiceResult> {
    let SignupCompleteParams {
        db, kv, jwt_secret, token, name, password, terms_accepted, marketing_consent, device, config,
    } = params;
    if !terms_accepted {
        return Ok(SignupCompleteServiceResult::Error {
            message: "You must accept the Terms of Service and Privacy Policy to create an account.".to_string(),
        });
    }
    if password.len() < 8 {
        return Ok(SignupCompleteServiceResult::Error {
            message: "Password must be at least 8 characters".to_string(),
        });
    }
    let name = name.trim().to_string();
    if name.is_empty() {
        return Ok(SignupCompleteServiceResult::Error {
            message: "Name is required".to_string(),
        });
    }

    // Verify email verification token
    let email =
        crate::token_service::verify_verification_token(db, token, "email_verification").await?;
    let Some(email) = email else {
        return Ok(SignupCompleteServiceResult::InvalidToken);
    };

    // Get user (must exist — created in signup/start)
    let user = crate::user_service::get_user_by_email(db, &email)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("User not found for verified token".into()))?;

    // Hash password first (fail early before DB writes)
    let hash = crate::password::hash_password(password)
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to hash password: {e}")))?;
    let auth_data = serde_json::json!({"hash": hash});

    crate::user_service::update_user_name(db, &user.user_id, &name).await?;
    crate::user_service::mark_user_verified(db, &email).await?;
    crate::user_service::update_terms_acceptance(
        db,
        &user.user_id,
        kyomi_core::TERMS_VERSION,
        marketing_consent,
    )
    .await?;

    if marketing_consent {
        crate::user_service::update_extra_metadata(
            db,
            &user.user_id,
            &serde_json::json!({"marketing_consent": true}),
        )
        .await?;
    }

    crate::user_service::upsert_auth_method(db, &user.user_id, "password", &auth_data).await?;
    crate::user_service::create_workspace_for_user(
        db,
        &user.user_id,
        Some(&name),
        &email,
        config,
    )
    .await?;

    // Re-fetch user after updates
    let user = crate::user_service::get_user_by_email(db, &email)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Internal("User not found after signup completion".into())
        })?;

    let sess = create_authenticated_session(db, kv, jwt_secret, &user, device).await?;
    Ok(SignupCompleteServiceResult::Success(Box::new(sess)))
}

// ---------------------------------------------------------------------------
// Google OAuth callback
// ---------------------------------------------------------------------------

/// Outcome of `google_oauth_callback_service`.
pub enum GoogleOAuthServiceResult {
    /// New user or user needing terms — redirect to welcome page.
    PendingTerms { redirect_url: String },
    /// Existing user logged in — server_fn should set cookies.
    Success {
        session: Box<AuthenticatedSession>,
        oauth_continue: Option<String>,
    },
    /// Rate limited.
    RateLimited { retry_after_secs: u64 },
}

/// Parameters for `google_oauth_callback_service`.
pub struct GoogleOAuthCallbackParams<'a> {
    pub db: &'a DbPool,
    pub kv: &'a KVPool,
    pub jwt_secret: &'a str,
    pub code: &'a str,
    pub state: Option<&'a str>,
    pub ip: &'a str,
    pub device: &'a DeviceInfo,
    pub client_id: &'a str,
    pub client_secret: &'a str,
    pub frontend_url: &'a str,
    pub encryption_key: &'a [u8; 32],
    pub config: Option<&'a kyomi_core::Config>,
}

/// Full Google OAuth callback orchestration.
pub async fn google_oauth_callback_service(
    params: GoogleOAuthCallbackParams<'_>,
) -> kyomi_core::Result<GoogleOAuthServiceResult> {
    let GoogleOAuthCallbackParams {
        db, kv, jwt_secret, code, state, ip, device,
        client_id, client_secret, frontend_url, encryption_key, config,
    } = params;
    // Rate limit
    let rate = crate::rate_limiter::check_rate_limit(kv, ip, "login", None).await?;
    if !rate.allowed {
        return Ok(GoogleOAuthServiceResult::RateLimited {
            retry_after_secs: rate.retry_after_secs,
        });
    }

    // Verify CSRF state (optional)
    let mut oauth_continue = None;
    if let Some(csrf_state) = state {
        let state_data =
            crate::redis_ops::verify_oauth_state(kv, "google", csrf_state).await?;
        if let Some(state_data) = state_data {
            oauth_continue = state_data
                .get("oauth_continue")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }

    // Exchange code for tokens
    let redirect_uri = format!("{}/auth/google/callback", frontend_url.trim_end_matches('/'));
    let token_data = crate::google_oauth::exchange_code_for_tokens(
        client_id,
        client_secret,
        code,
        &redirect_uri,
    )
    .await
    .map_err(|e| {
        kyomi_core::Error::Internal(format!(
            "Failed to exchange Google OAuth code for tokens: {e}"
        ))
    })?;

    // Get user info from Google
    let user_info = crate::google_oauth::get_user_info(&token_data.access_token)
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("Failed to get user info from Google: {e}"))
        })?;
    let email = user_info.email.to_lowercase();

    // Look up existing user
    let existing_user = crate::user_service::get_user_by_email(db, &email).await?;

    match existing_user {
        None => {
            // New user — store pending signup, return temp token
            let temp_token = crate::redis_ops::generate_token();
            let signup_data = serde_json::json!({
                "email": email,
                "name": user_info.name.unwrap_or_default(),
                "oauth_data": {
                    "google_id": user_info.id,
                    "oauth_provider": "google",
                    "picture": user_info.picture,
                }
            });
            crate::redis_ops::store_pending_signup(kv, &temp_token, &signup_data).await?;
            let redirect_url = format!(
                "{}/welcome?temp_token={temp_token}",
                frontend_url.trim_end_matches('/')
            );
            Ok(GoogleOAuthServiceResult::PendingTerms { redirect_url })
        }
        Some(user) if user.terms_accepted_at.is_none() => {
            // Existing user — needs terms acceptance
            let temp_token = crate::redis_ops::generate_token();
            let terms_data = serde_json::json!({
                "user_id": user.user_id,
                "email": email,
            });
            crate::redis_ops::store_pending_terms(kv, &temp_token, &terms_data).await?;
            let redirect_url = format!(
                "{}/welcome?temp_token={temp_token}&existing_user=true",
                frontend_url.trim_end_matches('/')
            );
            Ok(GoogleOAuthServiceResult::PendingTerms { redirect_url })
        }
        Some(user) => {
            // Existing user — terms accepted, normal login
            ensure_google_oauth_auth_method(db, &user.user_id).await?;
            ensure_user_has_workspace(db, &user.user_id, user.name.as_deref(), &email, config)
                .await?;
            update_google_oauth_data(db, &user, &user_info, encryption_key).await?;

            let sess = create_authenticated_session(db, kv, jwt_secret, &user, device).await?;
            Ok(GoogleOAuthServiceResult::Success {
                session: Box::new(sess),
                oauth_continue,
            })
        }
    }
}

/// Ensure `google_oauth` auth method exists for the user (idempotent upsert).
async fn ensure_google_oauth_auth_method(
    db: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<()> {
    let auth_method =
        crate::user_service::get_auth_method(db, user_id, "google_oauth").await?;
    if auth_method.is_none() {
        let auth_data = serde_json::json!({
            "linked_at": chrono::Utc::now().to_rfc3339(),
        });
        crate::user_service::upsert_auth_method(db, user_id, "google_oauth", &auth_data).await?;
    }
    Ok(())
}

/// Ensure the user has at least one workspace; create one if not.
async fn ensure_user_has_workspace(
    db: &DbPool,
    user_id: &str,
    user_name: Option<&str>,
    email: &str,
    config: Option<&kyomi_core::Config>,
) -> kyomi_core::Result<()> {
    let ws_ctx = crate::user_service::get_user_workspace_context(db, user_id).await?;
    if ws_ctx.is_none() {
        crate::user_service::create_workspace_for_user(db, user_id, user_name, email, config)
            .await?;
    }
    Ok(())
}

/// Update stored Google OAuth data (profile, last login; preserve BigQuery tokens).
async fn update_google_oauth_data(
    db: &DbPool,
    user: &kyomi_core::models::User,
    user_info: &crate::google_oauth::GoogleUserInfo,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<()> {
    let existing_oauth =
        crate::google_oauth::parse_oauth_data(user.oauth_data.as_deref(), encryption_key)
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("Failed to parse existing OAuth data: {e}"))
            })?;

    let updated_oauth = crate::google_oauth::OAuthData {
        google_id: Some(user_info.id.clone()),
        oauth_provider: Some("google".to_string()),
        picture: user_info.picture.clone(),
        last_oauth_login: Some(chrono::Utc::now().to_rfc3339()),
        google_oauth_tokens: existing_oauth.and_then(|o| o.google_oauth_tokens),
        ..Default::default()
    };

    let encrypted = crate::google_oauth::build_oauth_data(&updated_oauth, encryption_key)
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("Failed to build OAuth data: {e}"))
        })?;
    crate::user_service::update_user_oauth_data(db, &user.user_id, Some(&encrypted)).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Google account link callback
// ---------------------------------------------------------------------------

/// Outcome of `google_link_callback_service`.
pub struct GoogleLinkCallbackServiceResult {
    /// The Google account email that was linked.
    pub google_email: String,
    /// BigQuery access level ("read", "write", "none", etc.).
    pub bigquery_access: String,
    /// The workspace_id from the OAuth CSRF state, if present.
    /// The caller (route handler) uses this to send a WebSocket notification.
    pub workspace_id: Option<String>,
    /// The user_id extracted from the CSRF state.
    pub user_id: String,
}

/// Parameters for `google_link_callback_service`.
pub struct GoogleLinkCallbackParams<'a> {
    pub db: &'a DbPool,
    pub kv: &'a KVPool,
    pub encryption_key: &'a [u8; 32],
    pub code: &'a str,
    pub state: &'a str,
    pub client_id: &'a str,
    pub client_secret: &'a str,
    pub frontend_url: &'a str,
    pub ip: &'a str,
}

/// Full Google account link callback orchestration.
///
/// Verifies the CSRF state, exchanges the authorization code for tokens,
/// fetches user info from Google, and updates the stored OAuth data.
///
/// Does NOT send WebSocket notifications — that is the caller's responsibility
/// since `ws_manager` is server-crate-specific and not available in kyomi-auth.
/// The returned `workspace_id` and `user_id` provide the data the caller needs
/// to fire the notification.
pub async fn google_link_callback_service(
    params: GoogleLinkCallbackParams<'_>,
) -> kyomi_core::Result<GoogleLinkCallbackServiceResult> {
    use crate::google_oauth::{OAuthData, GoogleOAuthTokens};

    let GoogleLinkCallbackParams {
        db, kv, encryption_key, code, state, client_id, client_secret, frontend_url, ip,
    } = params;

    // Rate limit
    let rate = crate::rate_limiter::check_rate_limit(kv, ip, "login", None).await?;
    if !rate.allowed {
        return Err(kyomi_core::Error::TooManyRequests(
            format!("Rate limited. Try again in {} seconds", rate.retry_after_secs),
            rate.retry_after_secs,
        ));
    }

    // Verify CSRF state
    let state_data = crate::redis_ops::verify_oauth_state(kv, "google_link", state)
        .await?
        .ok_or_else(|| {
            tracing::warn!(ip = %ip, "Google link-callback: invalid or expired CSRF state");
            kyomi_core::Error::BadRequest("Invalid or expired state".into())
        })?;

    let action = state_data["action"].as_str().unwrap_or("");
    if action != "link_account" {
        return Err(kyomi_core::Error::BadRequest("Invalid state action".into()));
    }

    let link_user_id = state_data["user_id"]
        .as_str()
        .ok_or_else(|| kyomi_core::Error::BadRequest("Missing user_id in state".into()))?;

    // Exchange code for tokens
    let redirect_uri = format!(
        "{}/auth/google/link-callback",
        frontend_url.trim_end_matches('/')
    );
    let token_data =
        crate::google_oauth::exchange_code_for_tokens(client_id, client_secret, code, &redirect_uri)
            .await?;

    tracing::info!(
        scope = ?token_data.scope,
        has_refresh_token = token_data.refresh_token.is_some(),
        expires_in = ?token_data.expires_in,
        "Google link-callback: token exchange response — ACTUAL scopes Google granted"
    );

    // Get user info from Google
    let user_info = crate::google_oauth::get_user_info(&token_data.access_token).await?;

    // Find the user
    let user = crate::user_service::get_user_by_id(db, link_user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))?;

    // Parse existing OAuth data to preserve any existing refresh token
    let existing_oauth = crate::google_oauth::parse_oauth_data(
        user.oauth_data.as_deref(),
        encryption_key,
    )?;

    // Preserve existing refresh token if Google doesn't return a new one
    let existing_refresh = existing_oauth
        .as_ref()
        .and_then(|o| o.google_oauth_tokens.as_ref())
        .and_then(|t| t.refresh_token.clone());

    let new_refresh_token = token_data.refresh_token.or(existing_refresh);

    let expires_in = token_data.expires_in.unwrap_or(3600);
    let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(expires_in)).to_rfc3339();

    let google_email = user_info.email.clone();

    // Build updated oauth data WITH tokens (this is the BigQuery connect flow)
    let updated_oauth = OAuthData {
        google_id: Some(user_info.id),
        oauth_provider: Some("google".to_string()),
        picture: user_info.picture,
        last_oauth_login: Some(chrono::Utc::now().to_rfc3339()),
        google_oauth_tokens: Some(GoogleOAuthTokens {
            access_token: token_data.access_token,
            refresh_token: new_refresh_token,
            token_type: "Bearer".to_string(),
            scope: token_data.scope.unwrap_or_default(),
            expires_in: Some(expires_in),
            expires_at: Some(expires_at),
            email: Some(google_email.clone()),
            name: user_info.name,
        }),
        ..Default::default()
    };

    let encrypted = crate::google_oauth::build_oauth_data(&updated_oauth, encryption_key)?;
    crate::user_service::update_user_oauth_data(db, &user.user_id, Some(&encrypted)).await?;

    // Determine BigQuery access level
    let bigquery_access = updated_oauth
        .google_oauth_tokens
        .as_ref()
        .map(|t| crate::google_oauth::bigquery_access_level(&t.scope))
        .unwrap_or("none")
        .to_string();

    let workspace_id = state_data
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(GoogleLinkCallbackServiceResult {
        google_email,
        bigquery_access,
        workspace_id,
        user_id: link_user_id.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Account recovery
// ---------------------------------------------------------------------------

/// Outcome of `recovery_verify_service`.
pub enum RecoveryVerifyServiceResult {
    /// Token verified — recovery session created.
    Success {
        recovery_session_id: String,
        has_passkeys: bool,
    },
    /// Invalid or expired token.
    InvalidToken,
    /// Account is not verified.
    AccountNotVerified,
}

/// Verify a recovery token and create a short-lived recovery session.
pub async fn recovery_verify_service(
    db: &DbPool,
    kv: &KVPool,
    token: &str,
) -> kyomi_core::Result<RecoveryVerifyServiceResult> {
    let email =
        crate::token_service::verify_verification_token(db, token, "account_recovery").await?;
    let Some(email) = email else {
        return Ok(RecoveryVerifyServiceResult::InvalidToken);
    };

    let user = crate::user_service::get_user_by_email(db, &email)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("User not found for recovery token".into()))?;

    if !user.verified {
        return Ok(RecoveryVerifyServiceResult::AccountNotVerified);
    }

    let creds =
        crate::user_service::get_passkey_credentials(db, &user.user_id).await?;
    let has_passkeys = !creds.is_empty();

    let recovery_session_id = crate::redis_ops::generate_token();
    crate::redis_ops::store_recovery_session(kv, &recovery_session_id, &user.user_id).await?;

    Ok(RecoveryVerifyServiceResult::Success {
        recovery_session_id,
        has_passkeys,
    })
}

/// Outcome of `recovery_set_password_service`.
pub enum RecoverySetPasswordServiceResult {
    /// Password changed and user logged in — server_fn should set cookies.
    Success(Box<AuthenticatedSession>),
    /// Password validation failed.
    Error { message: String },
    /// Invalid or expired recovery session.
    InvalidSession,
}

/// Set a new password using a recovery session, completing the recovery flow.
pub async fn recovery_set_password_service(
    db: &DbPool,
    kv: &KVPool,
    jwt_secret: &str,
    recovery_session_id: &str,
    new_password: &str,
    device: &DeviceInfo,
) -> kyomi_core::Result<RecoverySetPasswordServiceResult> {
    if new_password.len() < 8 {
        return Ok(RecoverySetPasswordServiceResult::Error {
            message: "Password must be at least 8 characters".into(),
        });
    }

    // Peek recovery session (non-destructive — keeps session alive if validation fails)
    let user_id =
        crate::redis_ops::peek_recovery_session(kv, recovery_session_id).await?;
    let Some(user_id) = user_id else {
        return Ok(RecoverySetPasswordServiceResult::InvalidSession);
    };

    let user = crate::user_service::get_user_by_id(db, &user_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Internal("User not found for recovery session".into())
        })?;

    // Require new password to differ from existing (if any)
    if let Some(existing) =
        crate::user_service::get_auth_method(db, &user_id, "password").await?
        && let Some(existing_hash) = existing.auth_data.get("hash").and_then(|v| v.as_str())
    {
        let same = crate::password::verify_password(new_password, existing_hash)
            .map_err(|e| kyomi_core::Error::Internal(format!("Password verification error: {e}")))?;
        if same {
            return Ok(RecoverySetPasswordServiceResult::Error {
                message: "New password must be different from your current password.".into(),
            });
        }
    }

    // Hash and store new password
    let hash = crate::password::hash_password(new_password)
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to hash password: {e}")))?;
    crate::user_service::upsert_auth_method(
        db,
        &user_id,
        "password",
        &serde_json::json!({"hash": hash}),
    )
    .await?;

    // Consume recovery session
    crate::redis_ops::delete_recovery_session(kv, recovery_session_id).await?;

    // Disable TOTP — only after password successfully changed
    let totp_disabled =
        crate::user_service::remove_auth_method(db, &user_id, "totp").await?;
    if totp_disabled {
        tracing::info!(user_id = %user_id, "TOTP disabled during account recovery");
    }

    let sess = create_authenticated_session(db, kv, jwt_secret, &user, device).await?;
    Ok(RecoverySetPasswordServiceResult::Success(Box::new(sess)))
}

// ---------------------------------------------------------------------------
// Passkey login complete
// ---------------------------------------------------------------------------

/// Outcome of `passkey_login_complete_service`.
pub enum PasskeyLoginServiceResult {
    /// Authenticated successfully — server_fn should set cookies.
    Success(Box<AuthenticatedSession>),
    /// Challenge not found or expired.
    InvalidChallenge,
    /// User not found for credential.
    InvalidCredentials,
    /// Email not verified.
    VerificationRequired { email: String },
    /// Rate limited.
    RateLimited { retry_after_secs: u64 },
    /// WebAuthn assertion verification failed.
    AuthFailed,
}

/// Parameters for `passkey_login_complete_service`.
pub struct PasskeyLoginCompleteParams<'a> {
    pub db: &'a DbPool,
    pub kv: &'a KVPool,
    pub jwt_secret: &'a str,
    pub webauthn: &'a webauthn_rs::Webauthn,
    pub challenge_id: &'a str,
    pub assertion_json: &'a str,
    pub ip: &'a str,
    pub device: &'a DeviceInfo,
}

/// Full passkey-login-complete orchestration.
pub async fn passkey_login_complete_service(
    params: PasskeyLoginCompleteParams<'_>,
) -> kyomi_core::Result<PasskeyLoginServiceResult> {
    let PasskeyLoginCompleteParams { db, kv, jwt_secret, webauthn, challenge_id, assertion_json, ip, device } = params;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use webauthn_rs::prelude::*;

    // Rate limit
    let rate = crate::rate_limiter::check_rate_limit(kv, ip, "login", None).await?;
    if !rate.allowed {
        return Ok(PasskeyLoginServiceResult::RateLimited {
            retry_after_secs: rate.retry_after_secs,
        });
    }

    // Parse assertion
    let credential: PublicKeyCredential = serde_json::from_str(assertion_json)
        .map_err(|e| kyomi_core::Error::Internal(format!("Invalid assertion JSON: {e}")))?;

    // Get and delete challenge (prevent replay)
    let challenge_data = crate::redis_ops::get_webauthn_challenge(kv, challenge_id).await?;
    let Some(challenge_data) = challenge_data else {
        return Ok(PasskeyLoginServiceResult::InvalidChallenge);
    };
    crate::redis_ops::delete_webauthn_challenge(kv, challenge_id).await?;

    // Find user by credential ID
    let cred_id_bytes: &[u8] = credential.raw_id.as_ref();
    let credential_id_b64 = URL_SAFE_NO_PAD.encode(cred_id_bytes);
    let user =
        crate::user_service::find_user_by_credential_id(db, &credential_id_b64).await?;
    let Some(user) = user else {
        return Ok(PasskeyLoginServiceResult::InvalidCredentials);
    };

    if !user.verified {
        return Ok(PasskeyLoginServiceResult::VerificationRequired {
            email: user.email.clone(),
        });
    }

    // Verify assertion (discoverable or standard flow)
    let is_discoverable = challenge_data["discoverable"].as_bool().unwrap_or(false);
    let passkeys = get_passkeys_for_user(db, &user.user_id).await?;

    if is_discoverable {
        let disc_state: DiscoverableAuthentication =
            serde_json::from_value(challenge_data["discoverable_state"].clone())
                .map_err(|e| {
                    kyomi_core::Error::Internal(format!(
                        "Deserialize discoverable state: {e}"
                    ))
                })?;
        if passkeys.is_empty() {
            return Ok(PasskeyLoginServiceResult::InvalidCredentials);
        }
        let auth_result = crate::webauthn::finish_discoverable_authentication(
            webauthn,
            &credential,
            disc_state,
            &passkeys,
        )
        .map_err(|e| {
            tracing::warn!(error = %e, "Passkey discoverable auth failed");
            kyomi_core::Error::Internal("Authentication failed".into())
        });
        match auth_result {
            Ok(auth_result) => {
                update_passkey_after_auth_inner(
                    db,
                    &user.user_id,
                    &credential_id_b64,
                    cred_id_bytes,
                    &passkeys,
                    &auth_result,
                )
                .await;
            }
            Err(_) => return Ok(PasskeyLoginServiceResult::AuthFailed),
        }
    } else {
        let auth_state: PasskeyAuthentication =
            serde_json::from_value(challenge_data["authentication_state"].clone())
                .map_err(|e| {
                    kyomi_core::Error::Internal(format!("Deserialize auth state: {e}"))
                })?;
        let auth_result = crate::webauthn::finish_authentication(webauthn, &credential, &auth_state)
            .map_err(|e| {
                tracing::warn!(error = %e, "Passkey auth failed");
                kyomi_core::Error::Internal("Authentication failed".into())
            });
        match auth_result {
            Ok(auth_result) => {
                update_passkey_after_auth_inner(
                    db,
                    &user.user_id,
                    &credential_id_b64,
                    cred_id_bytes,
                    &passkeys,
                    &auth_result,
                )
                .await;
            }
            Err(_) => return Ok(PasskeyLoginServiceResult::AuthFailed),
        }
    }

    // Touch last_used on webauthn auth method (best-effort)
    let _ = crate::user_service::touch_auth_method(db, &user.user_id, "webauthn").await;

    let sess = create_authenticated_session(db, kv, jwt_secret, &user, device).await?;
    Ok(PasskeyLoginServiceResult::Success(Box::new(sess)))
}

// ---------------------------------------------------------------------------
// Passkey register start
// ---------------------------------------------------------------------------

/// Outcome of `passkey_register_start_service`.
pub enum PasskeyRegisterStartServiceResult {
    /// Challenge generated — server_fn returns to client.
    Success {
        challenge_id: String,
        creation_challenge: String,
    },
    /// Rate limited.
    RateLimited { retry_after_secs: u64 },
    /// Unverified email — must verify first.
    UnverifiedEmail,
    /// SaaS: verification email sent — user must verify before proceeding.
    VerificationEmailSent,
}

/// Parameters for `passkey_register_start_service`.
pub struct PasskeyRegisterStartParams<'a> {
    pub db: &'a DbPool,
    pub kv: &'a KVPool,
    pub webauthn: &'a webauthn_rs::Webauthn,
    pub email: &'a str,
    pub name: &'a str,
    pub device_name: &'a str,
    pub ip: &'a str,
    pub self_hosted: bool,
    pub smtp_configured: bool,
    pub frontend_url: &'a str,
}

/// Full passkey-register-start orchestration.
pub async fn passkey_register_start_service(
    params: PasskeyRegisterStartParams<'_>,
) -> kyomi_core::Result<PasskeyRegisterStartServiceResult> {
    let PasskeyRegisterStartParams {
        db, kv, webauthn, email, name, device_name, ip, self_hosted, smtp_configured, frontend_url,
    } = params;
    // Rate limit
    let rate = crate::rate_limiter::check_rate_limit(kv, ip, "signup", None).await?;
    if !rate.allowed {
        return Ok(PasskeyRegisterStartServiceResult::RateLimited {
            retry_after_secs: rate.retry_after_secs,
        });
    }

    // Get or create user
    let existing_user = crate::user_service::get_user_by_email(db, email).await?;
    let user = match existing_user {
        Some(u) if u.verified => u,
        Some(_) => return Ok(PasskeyRegisterStartServiceResult::UnverifiedEmail),
        None => {
            let smtp_less_self_hosted = self_hosted && !smtp_configured;
            if smtp_less_self_hosted {
                crate::user_service::create_user(db, email, Some(name), true).await?
            } else {
                // SaaS: create unverified user, send verification email
                let user =
                    crate::user_service::create_user(db, email, Some(name), false).await?;
                let raw_token =
                    crate::token_service::create_verification_token(db, email, "signup").await?;
                let signup_url = format!(
                    "{}/auth/passkey-signup?token={raw_token}",
                    frontend_url.trim_end_matches('/')
                );
                tracing::info!(
                    "Passkey signup link for {email}: {signup_url} (user_id={})",
                    user.user_id
                );
                spawn_verification_email(email.to_string(), name.to_string(), signup_url);
                return Ok(PasskeyRegisterStartServiceResult::VerificationEmailSent);
            }
        }
    };

    // Generate WebAuthn registration challenge
    let user_unique_id = webauthn_user_id_inner(email);
    let display_name = user.name.as_deref().unwrap_or(email);

    let creds = crate::user_service::get_passkey_credentials(db, &user.user_id).await?;
    let exclude_ids = build_exclude_ids(&creds);
    let exclude_opt = if exclude_ids.is_empty() {
        None
    } else {
        Some(exclude_ids)
    };

    let (ccr, reg_state) =
        crate::webauthn::start_registration(webauthn, user_unique_id, email, display_name, exclude_opt)
            .map_err(|e| kyomi_core::Error::Internal(e.to_string()))?;

    let challenge_id = crate::redis_ops::generate_token();
    let reg_state_json = serde_json::to_value(&reg_state)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize reg state: {e}")))?;
    let challenge_data = serde_json::json!({
        "registration_state": reg_state_json,
        "email": email,
        "user_name": display_name,
        "user_id": user.user_id,
        "device_name": device_name,
        "is_signup": true,
    });
    crate::redis_ops::store_webauthn_challenge(kv, &challenge_id, &challenge_data).await?;

    let creation_challenge = serde_json::to_string(&ccr)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize creation challenge: {e}")))?;

    Ok(PasskeyRegisterStartServiceResult::Success {
        challenge_id,
        creation_challenge,
    })
}

// ---------------------------------------------------------------------------
// Passkey register complete
// ---------------------------------------------------------------------------

/// Full passkey-register-complete orchestration.
///
/// Returns an `AuthenticatedSession` on success (auto-login after registration).
pub async fn passkey_register_complete_service(
    db: &DbPool,
    kv: &KVPool,
    jwt_secret: &str,
    webauthn: &webauthn_rs::Webauthn,
    challenge_id: &str,
    credential_json: &str,
    device: &DeviceInfo,
) -> kyomi_core::Result<AuthenticatedSession> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use webauthn_rs::prelude::*;

    let credential: RegisterPublicKeyCredential =
        serde_json::from_str(credential_json)
            .map_err(|e| kyomi_core::Error::Internal(format!("Invalid credential JSON: {e}")))?;

    // Get and delete challenge
    let challenge_data = crate::redis_ops::get_webauthn_challenge(kv, challenge_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Internal("Invalid or expired challenge".into())
        })?;
    crate::redis_ops::delete_webauthn_challenge(kv, challenge_id).await?;

    // Extract challenge state
    let reg_state: PasskeyRegistration =
        serde_json::from_value(challenge_data["registration_state"].clone())
            .map_err(|e| kyomi_core::Error::Internal(format!("Deserialize reg state: {e}")))?;
    let email = challenge_data["email"]
        .as_str()
        .ok_or_else(|| kyomi_core::Error::Internal("Missing email in challenge".into()))?;
    let user_id = challenge_data["user_id"]
        .as_str()
        .ok_or_else(|| kyomi_core::Error::Internal("Missing user_id in challenge".into()))?;
    let device_name = challenge_data["device_name"]
        .as_str()
        .unwrap_or("Unknown Device");

    // Verify credential
    let passkey = crate::webauthn::finish_registration(webauthn, &credential, &reg_state)
        .map_err(|e| kyomi_core::Error::Internal(e.to_string()))?;

    // Extract and encode credential ID
    let cred_id_bytes: &[u8] = passkey.cred_id().as_ref();
    let credential_id_b64 = URL_SAFE_NO_PAD.encode(cred_id_bytes);

    // Serialize passkey
    let passkey_json = serde_json::to_value(&passkey)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize passkey: {e}")))?;
    let public_key_b64 = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&passkey)
            .map_err(|e| kyomi_core::Error::Internal(format!("Serialize passkey bytes: {e}")))?,
    );
    let initial_counter = passkey_json
        .get("cred")
        .and_then(|c| c.get("counter"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;

    // Store credential
    crate::user_service::add_passkey_to_user(
        db,
        user_id,
        &credential_id_b64,
        &public_key_b64,
        initial_counter,
        device_name,
        &passkey_json,
    )
    .await?;

    // Get user and create session
    let user = crate::user_service::get_user_by_id(db, user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("User not found".into()))?;

    let sess = create_authenticated_session(db, kv, jwt_secret, &user, device).await?;
    tracing::info!(
        user_id = %user.user_id,
        email = %email,
        credential_id = %credential_id_b64,
        "Passkey registered and user auto-logged in"
    );
    Ok(sess)
}

// ---------------------------------------------------------------------------
// Passkey signup complete
// ---------------------------------------------------------------------------

/// Parameters for `passkey_signup_complete_service`.
pub struct PasskeySignupCompleteParams<'a> {
    pub db: &'a DbPool,
    pub kv: &'a KVPool,
    pub webauthn: &'a webauthn_rs::Webauthn,
    pub token: &'a str,
    pub name: &'a str,
    pub terms_accepted: bool,
    pub marketing_consent: bool,
    pub config: Option<&'a kyomi_core::Config>,
}

/// Full passkey-signup-complete orchestration.
///
/// Verifies the email verification token, sets up the user account, and
/// generates a WebAuthn registration challenge. Returns the challenge for the
/// client to complete via `passkey_register_complete`.
pub async fn passkey_signup_complete_service(
    params: PasskeySignupCompleteParams<'_>,
) -> kyomi_core::Result<Result<(String, String), String>> {
    let PasskeySignupCompleteParams {
        db, kv, webauthn, token, name, terms_accepted, marketing_consent, config,
    } = params;
    // Returns Ok((challenge_id, creation_challenge)) or Err(message)

    if !terms_accepted {
        return Ok(Err(
            "You must accept the Terms of Service and Privacy Policy to create an account."
                .to_string(),
        ));
    }
    let name = name.trim().to_string();
    if name.is_empty() {
        return Ok(Err("Name is required".to_string()));
    }

    // Verify token
    let email =
        crate::token_service::verify_verification_token(db, token, "email_verification").await?;
    let Some(email) = email else {
        return Ok(Err(
            "Invalid or expired signup link. Please request a new one.".to_string(),
        ));
    };

    // Get user
    let user = crate::user_service::get_user_by_email(db, &email)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("User not found for verified token".into()))?;

    // Update user account
    crate::user_service::update_user_name(db, &user.user_id, &name).await?;
    crate::user_service::mark_user_verified(db, &email).await?;
    crate::user_service::update_terms_acceptance(
        db,
        &user.user_id,
        kyomi_core::TERMS_VERSION,
        marketing_consent,
    )
    .await?;
    if marketing_consent {
        crate::user_service::update_extra_metadata(
            db,
            &user.user_id,
            &serde_json::json!({"marketing_consent": true}),
        )
        .await?;
    }
    crate::user_service::create_workspace_for_user(
        db,
        &user.user_id,
        Some(&name),
        &email,
        config,
    )
    .await?;

    // Generate WebAuthn registration challenge
    let user_unique_id = webauthn_user_id_inner(&email);
    let creds = crate::user_service::get_passkey_credentials(db, &user.user_id).await?;
    let exclude_ids = build_exclude_ids(&creds);
    let exclude_opt = if exclude_ids.is_empty() {
        None
    } else {
        Some(exclude_ids)
    };

    let (ccr, reg_state) =
        crate::webauthn::start_registration(webauthn, user_unique_id, &email, &name, exclude_opt)
            .map_err(|e| kyomi_core::Error::Internal(e.to_string()))?;

    let challenge_id = crate::redis_ops::generate_token();
    let reg_state_json = serde_json::to_value(&reg_state)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize reg state: {e}")))?;
    let challenge_data = serde_json::json!({
        "registration_state": reg_state_json,
        "email": email,
        "user_name": &name,
        "user_id": user.user_id,
        "device_name": "Unknown Device",
        "is_signup": true,
    });
    crate::redis_ops::store_webauthn_challenge(kv, &challenge_id, &challenge_data).await?;

    let creation_challenge = serde_json::to_string(&ccr)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize creation challenge: {e}")))?;

    tracing::info!(
        email = %email,
        user_id = %user.user_id,
        "Passkey signup token verified, WebAuthn challenge generated"
    );
    Ok(Ok((challenge_id, creation_challenge)))
}

// ---------------------------------------------------------------------------
// Passkey recovery verify
// ---------------------------------------------------------------------------

/// Full passkey-recovery-verify orchestration.
///
/// Verifies the recovery token and generates a WebAuthn registration challenge
/// for replacing the user's passkey. Returns `(challenge_id, creation_challenge,
/// email)` on success, or an error message on failure.
pub async fn passkey_recovery_verify_service(
    db: &DbPool,
    kv: &KVPool,
    webauthn: &webauthn_rs::Webauthn,
    token: &str,
) -> kyomi_core::Result<Result<(String, String, String), String>> {
    // Returns Ok((challenge_id, creation_challenge, email)) or Err(message)

    let email =
        crate::token_service::verify_verification_token(db, token, "recovery").await?;
    let Some(email) = email else {
        return Ok(Err(
            "Invalid or expired recovery link. Please request a new one.".to_string(),
        ));
    };

    let user = crate::user_service::get_user_by_email(db, &email)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("User not found for recovery token".into()))?;

    let user_unique_id = webauthn_user_id_inner(&email);
    let display_name = user.name.as_deref().unwrap_or(&email);

    let creds = crate::user_service::get_passkey_credentials(db, &user.user_id).await?;
    let exclude_ids = build_exclude_ids(&creds);
    let exclude_opt = if exclude_ids.is_empty() {
        None
    } else {
        Some(exclude_ids)
    };

    let (ccr, reg_state) = crate::webauthn::start_registration(
        webauthn,
        user_unique_id,
        &email,
        display_name,
        exclude_opt,
    )
    .map_err(|e| kyomi_core::Error::Internal(e.to_string()))?;

    let challenge_id = crate::redis_ops::generate_token();
    let reg_state_json = serde_json::to_value(&reg_state)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize reg state: {e}")))?;
    let challenge_data = serde_json::json!({
        "registration_state": reg_state_json,
        "email": email,
        "user_name": display_name,
        "user_id": user.user_id,
        "device_name": "Unknown Device",
        "is_signup": false,
    });
    crate::redis_ops::store_webauthn_challenge(kv, &challenge_id, &challenge_data).await?;

    let creation_challenge = serde_json::to_string(&ccr)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize creation challenge: {e}")))?;

    tracing::info!(
        email = %email,
        user_id = %user.user_id,
        "Passkey recovery token verified, WebAuthn challenge generated"
    );
    Ok(Ok((challenge_id, creation_challenge, email)))
}

// ---------------------------------------------------------------------------
// Rate limit helper (exposed for server_fn use in resend_verification /
// recovery_start which only need the rate-limit check and some trivial work)
// ---------------------------------------------------------------------------

/// Check rate limit for a given IP and bucket, returning the result.
pub async fn check_rate_limit(
    kv: &KVPool,
    ip: &str,
    bucket: &str,
) -> kyomi_core::Result<RateLimitResult> {
    crate::rate_limiter::check_rate_limit(kv, ip, bucket, None).await
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Generate a WebAuthn user handle from email (deterministic, matching the server route).
///
/// `sha256(email)[:16]` interpreted as a UUID.
fn webauthn_user_id_inner(email: &str) -> webauthn_rs::prelude::Uuid {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(email.as_bytes());
    let hash = hasher.finalize();
    let bytes: [u8; 16] = hash[..16].try_into().expect("16 bytes");
    webauthn_rs::prelude::Uuid::from_bytes(bytes)
}

/// Retrieve `Passkey` objects from stored credentials for a user.
pub async fn get_passkeys_for_user(
    db: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<Vec<webauthn_rs::prelude::Passkey>> {
    let creds = crate::user_service::get_passkey_credentials(db, user_id).await?;
    let mut passkeys = Vec::new();
    for (_cred_id, cred_data) in &creds {
        if let Some(passkey_json) = cred_data.get("passkey")
            && let Ok(passkey) =
                serde_json::from_value::<webauthn_rs::prelude::Passkey>(passkey_json.clone())
        {
            passkeys.push(passkey);
        }
    }
    Ok(passkeys)
}

/// Build a list of `CredentialID` values from stored credentials to exclude during registration.
fn build_exclude_ids(
    creds: &serde_json::Map<String, serde_json::Value>,
) -> Vec<webauthn_rs::prelude::CredentialID> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use webauthn_rs::prelude::CredentialID;

    let mut ids = Vec::new();
    for (cred_id_b64, _) in creds {
        if let Ok(bytes) = URL_SAFE_NO_PAD.decode(cred_id_b64) {
            ids.push(CredentialID::from(bytes));
        }
    }
    ids
}

/// Update passkey credential usage after successful authentication (best-effort, fire-and-forget).
pub async fn update_passkey_after_auth_inner(
    db: &DbPool,
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
        let updated_json = match serde_json::to_value(&updated_pk) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    user_id = %user_id,
                    credential_id = %credential_id_b64,
                    error = %e,
                    "Failed to serialize updated passkey — skipping counter update"
                );
                return;
            }
        };
        if let Err(e) = crate::user_service::update_credential_usage(
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

/// Result of attempting to resend a verification email.
pub struct ResendVerificationResult {
    pub should_send: bool,
    pub user_name: String,
    pub raw_token: String,
}

/// Check rate limit, look up the unverified user, and create a verification
/// token in one service call. Returns `None` if rate-limited, user not found,
/// or user is already verified.
pub async fn resend_verification_service(
    db: &kyomi_core::DbPool,
    kv: &KVPool,
    ip: &str,
    email: &str,
) -> kyomi_core::Result<Option<ResendVerificationResult>> {
    let rate = check_rate_limit(kv, ip, "register").await?;
    if !rate.allowed {
        return Ok(None);
    }

    let user = crate::user_service::get_user_by_email(db, email).await?;
    let Some(user) = user else { return Ok(None) };
    if user.verified {
        return Ok(None);
    }

    let raw_token =
        crate::token_service::create_verification_token(db, email, "email_verification").await?;

    Ok(Some(ResendVerificationResult {
        should_send: true,
        user_name: user.name.unwrap_or_default(),
        raw_token,
    }))
}

/// Result of attempting to start account recovery.
pub struct RecoveryStartResult {
    pub user_name: String,
    pub raw_token: String,
}

/// Check rate limit, look up the verified user, and create a recovery token
/// in one service call. Returns `None` if rate-limited or user not found/unverified.
pub async fn recovery_start_service(
    db: &kyomi_core::DbPool,
    kv: &KVPool,
    ip: &str,
    email: &str,
) -> kyomi_core::Result<Option<RecoveryStartResult>> {
    let rate = check_rate_limit(kv, ip, "register").await?;
    if !rate.allowed {
        return Err(kyomi_core::Error::BadRequest(format!(
            "Rate limited. Try again in {} seconds",
            rate.retry_after_secs
        )));
    }

    let user = crate::user_service::get_user_by_email(db, email).await?;
    let Some(user) = user else { return Ok(None) };
    if !user.verified {
        return Ok(None);
    }

    let raw_token = crate::token_service::create_verification_token_with_expiry(
        db,
        email,
        "account_recovery",
        Some(0.25),
    )
    .await?;

    Ok(Some(RecoveryStartResult {
        user_name: user.name.unwrap_or_default(),
        raw_token,
    }))
}

/// Send a verification email in a background task (fire-and-forget).
fn spawn_verification_email(email: String, name: String, url: String) {
    tokio::spawn(async move {
        let email_svc = crate::email_service::EmailService::from_env();
        let sent = email_svc.send_verification_email(&email, &name, &url).await;
        if sent {
            tracing::info!("Verification email sent to {email}");
        } else {
            tracing::warn!("Failed to send verification email to {email}");
        }
    });
}
