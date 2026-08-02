// SPDX-License-Identifier: AGPL-3.0-or-later

//! Passkey (WebAuthn) endpoints.
//!
//! Wire-compatible with Python's `routers/auth_passkeys.py`.
//!
//! Phase 3D: Core endpoints (5) — signup, register, login
//! Phase 3E: Management + recovery (8) — list, add, delete, rename, recovery

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Json, Router,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use webauthn_rs::prelude::*;

use kyomi_auth::{
    cookies,
    jwt,
    middleware::AuthUser,
    rate_limiter,
    redis_ops,
    session,
    token_service::{self},
    user_service,
    webauthn as wa,
    webauthn_challenge_purpose::{
        PASSKEY_ADD_DEVICE, PASSKEY_LOGIN, PASSKEY_RECOVERY, PASSKEY_SIGNUP,
    },
};
use kyomi_core::models::User;

use crate::state::AppState;

/// Build the passkey router (mounted at `/auth`).
pub fn routes() -> Router<AppState> {
    Router::new()
        // Phase 3D: Core passkey endpoints
        .route("/passkeys/register/start", post(register_start))
        .route("/passkeys/signup/complete", post(signup_complete))
        .route("/passkeys/register/complete", post(register_complete))
        .route("/passkeys/login/start", post(login_start))
        .route("/passkeys/login/complete", post(login_complete))
        // Phase 3E: Management + recovery endpoints
        .route("/passkeys/list", get(passkeys_list))
        .route("/passkeys/add/start", post(add_start))
        .route("/passkeys/add/complete", post(add_complete))
        .route("/passkeys/{credential_id}", delete(passkey_delete))
        .route("/passkeys/{credential_id}", patch(passkey_rename))
        .route("/passkeys/recovery/request", post(recovery_request))
        .route("/passkeys/recovery/verify", post(recovery_verify))
        .route("/passkeys/recovery/register", post(recovery_register))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_client_ip(headers: &HeaderMap) -> String {
    kyomi_auth::request_meta::extract_client_ip(headers, None)
}

/// Reject a challenge minted by any other flow (KYO-279/KYO-280).
///
/// Maps a missing/unrecognised/wrong `purpose` to the same
/// `BadRequest("Invalid or expired challenge")` these handlers already return
/// for a challenge that isn't in the KV store, so a purpose mismatch is
/// indistinguishable from "not found" and cannot be used as an oracle.
fn require_purpose(
    challenge_data: &serde_json::Value,
    allowed: &[&str],
) -> Result<(), kyomi_core::Error> {
    if kyomi_auth::webauthn_challenge_purpose::has_purpose(challenge_data, allowed) {
        Ok(())
    } else {
        Err(kyomi_core::Error::BadRequest(
            "Invalid or expired challenge".into(),
        ))
    }
}

/// Terms of service version — matches Python's hardcoded version.
use kyomi_core::TERMS_VERSION;

/// Generate a WebAuthn user unique ID from email (matching Python).
///
/// `base64url(sha256(email)[:32])` — produces a stable, deterministic user handle.
fn webauthn_user_id(email: &str) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(email.as_bytes());
    let hash = hasher.finalize();
    // Use the first 16 bytes of SHA-256 as a UUID
    let bytes: [u8; 16] = hash[..16].try_into().expect("16 bytes");
    Uuid::from_bytes(bytes)
}

/// Extract existing credential IDs from a user's webauthn auth data.
async fn get_exclude_credential_ids(
    db: &kyomi_core::DbPool,
    user_id: &str,
) -> kyomi_core::Result<Vec<CredentialID>> {
    let creds = user_service::get_passkey_credentials(db, user_id).await?;
    let mut ids = Vec::new();
    for (cred_id_b64, _) in &creds {
        if let Ok(bytes) = URL_SAFE_NO_PAD.decode(cred_id_b64) {
            ids.push(CredentialID::from(bytes));
        } else {
            // Try with padding
            let padded = format!("{}{}", cred_id_b64, "==");
            if let Ok(bytes) = URL_SAFE_NO_PAD.decode(&padded) {
                ids.push(CredentialID::from(bytes));
            }
        }
    }
    Ok(ids)
}

/// Get Passkey objects for authentication from user's stored credentials.
async fn get_passkeys_for_auth(
    db: &kyomi_core::DbPool,
    user_id: &str,
) -> kyomi_core::Result<Vec<Passkey>> {
    let creds = user_service::get_passkey_credentials(db, user_id).await?;
    let mut passkeys = Vec::new();
    for (_cred_id, cred_data) in &creds {
        if let Some(passkey_json) = cred_data.get("passkey")
            && let Ok(passkey) = serde_json::from_value::<Passkey>(passkey_json.clone())
        {
            passkeys.push(passkey);
        }
    }
    Ok(passkeys)
}

// ---------------------------------------------------------------------------
// Request/Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RegisterStartRequest {
    email: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "default_device_name")]
    device_name: String,
}

fn default_device_name() -> String {
    "Unknown Device".to_string()
}

#[derive(Deserialize)]
struct SignupCompleteRequest {
    token: String,
    name: String,
    #[serde(default = "crate::helpers::default_true")]
    terms_accepted: bool,
    #[serde(default)]
    marketing_consent: bool,
    #[serde(default = "default_device_name")]
    device_name: String,
}

#[derive(Deserialize)]
struct RegisterCompleteRequest {
    challenge_id: String,
    credential: RegisterPublicKeyCredential,
}

#[derive(Deserialize)]
struct LoginStartRequest {
    #[serde(default)]
    email: Option<String>,
}

#[derive(Deserialize)]
struct LoginCompleteRequest {
    challenge_id: String,
    credential: PublicKeyCredential,
}

// ---------------------------------------------------------------------------
// POST /auth/passkeys/register/start
// ---------------------------------------------------------------------------

async fn register_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<RegisterStartRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Rate limit — this endpoint sends verification emails
    let ip = extract_client_ip(&headers);
    let rate_result = rate_limiter::check_rate_limit(&state.kv, &ip, "signup", None).await?;
    if !rate_result.allowed {
        tracing::warn!(ip = %ip, "Passkey register/start rate limited");
        return Err(kyomi_core::Error::TooManyRequests(
            format!(
                "Rate limited. Try again in {} seconds",
                rate_result.retry_after_secs
            ),
            rate_result.retry_after_secs,
        ));
    }

    let email = data.email.to_lowercase().trim().to_string();
    let name = data.name.unwrap_or_default();
    let _device_name = data.device_name;

    // Look up existing user
    let existing_user = user_service::get_user_by_email(&state.db, &email).await?;

    // Self-hosted without SMTP: skip email verification entirely.
    let smtp_less_self_hosted = state.config.self_hosted && !state.config.smtp_configured();

    match existing_user {
        None => {
            if smtp_less_self_hosted {
                // Create user pre-verified — no email needed.
                // Still issue a signup token so the frontend can proceed to passkey registration.
                let user =
                    user_service::create_user(&state.db, &email, Some(&name), true).await?;

                let raw_token =
                    token_service::create_verification_token(&state.db, &email, "signup").await?;

                tracing::info!(
                    email = %email,
                    user_id = %user.user_id,
                    "Self-hosted SMTP-less: created passkey user as pre-verified, token issued directly"
                );

                Ok(Json(serde_json::json!({
                    "status": "token_issued",
                    "token": raw_token,
                    "message": "Email verification skipped (SMTP not configured). Proceed with passkey registration.",
                })))
            } else {
                // Standard flow: create unverified user, send verification email
                let user =
                    user_service::create_user(&state.db, &email, Some(&name), false).await?;

                // Notify admin (Slack + email) — fire-and-forget
                let notify_state = state.clone();
                let notify_email = email.clone();
                let notify_name = name.clone();
                let notify_user_id = user.user_id.clone();
                tokio::spawn(async move {
                    crate::routes::admin_notify::notify_signup(
                        &notify_state,
                        &notify_email,
                        &notify_name,
                        &notify_user_id,
                    )
                    .await;
                });

                // Create signup verification token (stored in DB with bcrypt)
                let raw_token =
                    token_service::create_verification_token(&state.db, &email, "signup").await?;

                let signup_url = format!(
                    "{}/auth/passkey-signup?token={raw_token}",
                    state.config.frontend_url.trim_end_matches('/')
                );
                tracing::info!(
                    "Passkey signup link for {email}: {signup_url} (user_id={})",
                    user.user_id
                );

                crate::helpers::spawn_verification_email(email, name, signup_url);

                Ok(Json(serde_json::json!({
                    "status": "verification_required",
                    "message": "Please check your email to complete signup and create your passkey.",
                })))
            }
        }
        Some(user) if !user.verified => {
            if smtp_less_self_hosted {
                // Mark existing unverified user as verified and issue token directly
                tracing::info!(
                    email = %email,
                    user_id = %user.user_id,
                    "Self-hosted SMTP-less: marking pending passkey user as verified, token issued directly"
                );
                user_service::mark_user_verified(&state.db, &email).await?;

                let raw_token =
                    token_service::create_verification_token(&state.db, &email, "signup").await?;

                Ok(Json(serde_json::json!({
                    "status": "token_issued",
                    "token": raw_token,
                    "message": "Email verification skipped (SMTP not configured). Proceed with passkey registration.",
                })))
            } else {
                // EXISTING UNVERIFIED USER — resend verification
                tracing::info!(email = %email, user_id = %user.user_id, "Resending passkey verification email for pending user");

                let raw_token =
                    token_service::create_verification_token(&state.db, &email, "signup").await?;

                let signup_url = format!(
                    "{}/auth/passkey-signup?token={raw_token}",
                    state.config.frontend_url.trim_end_matches('/')
                );
                tracing::info!(
                    "Passkey signup link (resend) for {email}: {signup_url} (user_id={})",
                    user.user_id
                );

                let name = user.name.clone().unwrap_or_default();
                crate::helpers::spawn_verification_email(email, name, signup_url);

                Ok(Json(serde_json::json!({
                    "status": "verification_required",
                    "message": "Please check your email to complete signup and create your passkey.",
                })))
            }
        }
        Some(_user) => {
            // VERIFIED USER — already has an account, tell them to sign in
            Err(kyomi_core::Error::BadRequest(
                "Email address is already registered. Please sign in instead.".into(),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// POST /auth/passkeys/signup/complete
// ---------------------------------------------------------------------------

async fn signup_complete(
    State(state): State<AppState>,
    Json(data): Json<SignupCompleteRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Validate terms acceptance
    if !data.terms_accepted {
        return Err(kyomi_core::Error::BadRequest(
            "You must accept the Terms of Service and Privacy Policy to create an account.".into(),
        ));
    }

    // Verify signup token
    let email = token_service::verify_verification_token(&state.db, &data.token, "signup")
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest(
                "Invalid or expired signup link. Please request a new one.".into(),
            )
        })?;

    // Get user (must exist — was created in register/start)
    let user = user_service::get_user_by_email(&state.db, &email)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Internal("User not found for verified token".into())
        })?;

    // Update user name
    user_service::update_user_name(&state.db, &user.user_id, &data.name).await?;

    // Mark verified
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

    // Create personal workspace
    user_service::create_workspace_for_user(
        &state.db,
        &user.user_id,
        Some(&data.name),
        &email,
        Some(&state.config),
    )
    .await?;

    // Generate WebAuthn registration challenge
    let user_unique_id = webauthn_user_id(&email);
    let display_name = &data.name;

    let (ccr, reg_state) = wa::start_registration(
        &state.webauthn,
        user_unique_id,
        &email,
        display_name,
        None, // No existing credentials for new signup
    )?;

    // Store challenge in Redis, purpose-bound to signup (KYO-279/KYO-280).
    let challenge_id = redis_ops::generate_token();
    let reg_state_json = serde_json::to_value(&reg_state)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize reg state: {e}")))?;

    let challenge_data = serde_json::json!({
        "registration_state": reg_state_json,
        "email": email,
        "user_name": display_name,
        "user_id": user.user_id,
        "device_name": data.device_name,
        "purpose": PASSKEY_SIGNUP,
    });
    redis_ops::store_webauthn_challenge(&state.kv, &challenge_id, &challenge_data).await?;

    Ok(Json(serde_json::json!({
        "status": "ready_for_passkey",
        "challenge_id": challenge_id,
        "options": ccr,
        "user": {
            "email": email,
            "name": display_name,
        },
    })))
}

// ---------------------------------------------------------------------------
// POST /auth/passkeys/register/complete
// ---------------------------------------------------------------------------

async fn register_complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<RegisterCompleteRequest>,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    // Get challenge from Redis
    let challenge_data =
        redis_ops::get_webauthn_challenge(&state.kv, &data.challenge_id)
            .await?
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Invalid or expired challenge".into())
            })?;

    // Delete challenge (prevent replay)
    redis_ops::delete_webauthn_challenge(&state.kv, &data.challenge_id).await?;

    // Reject a challenge minted by any other flow (KYO-279/KYO-280) — deliberately
    // NOT PASSKEY_RECOVERY: recovery registration has its own handler
    // (`recovery_register`) which additionally requires an HttpOnly
    // `recovery_session` cookie. Accepting a recovery-purpose challenge here
    // would let a caller holding only a `challenge_id` bypass that cookie
    // gate. Same rejection as "not found" so this can't be used to probe
    // purpose.
    require_purpose(&challenge_data, &[PASSKEY_SIGNUP])?;

    // Extract challenge state
    let reg_state: PasskeyRegistration = serde_json::from_value(
        challenge_data["registration_state"].clone(),
    )
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

    // Verify the credential with webauthn-rs
    let passkey = wa::finish_registration(
        &state.webauthn,
        &data.credential,
        &reg_state,
    )?;

    // Extract credential ID as base64url (no padding) for storage
    let cred_id_bytes: &[u8] = passkey.cred_id().as_ref();
    let credential_id_b64 = URL_SAFE_NO_PAD.encode(cred_id_bytes);

    // Serialize passkey for Rust-native storage
    let passkey_json = serde_json::to_value(&passkey)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize passkey: {e}")))?;

    // Python compatibility: the `public_key` field stores the full serialized
    // webauthn-rs Passkey object (base64url-encoded JSON), NOT a raw public key.
    // This matches the Python schema where `public_key` is opaque bytes used for auth.
    // Rust uses the `passkey` JSON field directly; `public_key` exists for Python parity.
    let public_key_b64 = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&passkey)
            .map_err(|e| kyomi_core::Error::Internal(format!("Serialize passkey bytes: {e}")))?
    );

    // Initial sign count from the serialized passkey JSON
    let initial_counter = passkey_json
        .get("cred")
        .and_then(|c| c.get("counter"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;

    // Store credential in user's webauthn auth method
    user_service::add_passkey_to_user(
        &state.db,
        user_id,
        &credential_id_b64,
        &public_key_b64,
        initial_counter,
        device_name,
        &passkey_json,
    )
    .await?;

    // Get user for response
    let user = user_service::get_user_by_id(&state.db, user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))?;

    // The purpose gate above accepts only PASSKEY_SIGNUP, so every challenge
    // reaching this point is a signup challenge — auto-login unconditionally.
    let device = kyomi_auth::request_meta::extract_device_info(&headers);
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
        "credential_id": credential_id_b64,
        "user_email": email,
        "verified": user.verified,
        "user": {
            "email": sess.user.email,
            "name": sess.user.name,
            "billing_project": sess.user.billing_project,
        },
        "access_token": sess.access_token,
        "refresh_token": sess.refresh_token,
        "message": "Account created successfully!",
    });

    Ok((sess.cookie_headers, Json(body)))
}

// ---------------------------------------------------------------------------
// POST /auth/passkeys/login/start
// ---------------------------------------------------------------------------

async fn login_start(
    State(state): State<AppState>,
    Json(data): Json<LoginStartRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let mut passkeys_for_auth: Vec<Passkey> = Vec::new();
    let mut email_for_challenge: Option<String> = None;

    if let Some(ref email) = data.email {
        let email = email.to_lowercase().trim().to_string();
        email_for_challenge = Some(email.clone());

        if let Some(user) = user_service::get_user_by_email(&state.db, &email).await? {
            passkeys_for_auth = get_passkeys_for_auth(&state.db, &user.user_id).await?;
        }
    }

    if passkeys_for_auth.is_empty() {
        // No credentials found — use discoverable credential flow.
        // webauthn-rs `conditional-ui` feature provides start_discoverable_authentication()
        // which generates a proper challenge with empty allowCredentials.
        let (mut rcr, disc_state) = wa::start_discoverable_authentication(&state.webauthn)?;

        // Remove mediation hint — we want a modal prompt, not conditional UI autofill
        rcr.mediation = None;

        let challenge_id = redis_ops::generate_token();
        let disc_state_json = serde_json::to_value(&disc_state)
            .map_err(|e| kyomi_core::Error::Internal(format!("Serialize discoverable state: {e}")))?;

        let challenge_data = serde_json::json!({
            "discoverable_state": disc_state_json,
            "email": email_for_challenge,
            "discoverable": true,
            "purpose": PASSKEY_LOGIN,
        });
        redis_ops::store_webauthn_challenge(&state.kv, &challenge_id, &challenge_data).await?;

        return Ok(Json(serde_json::json!({
            "challenge_id": challenge_id,
            "options": rcr,
        })));
    }

    // Start authentication with webauthn-rs
    let (rcr, auth_state) = wa::start_authentication(&state.webauthn, &passkeys_for_auth)?;

    let challenge_id = redis_ops::generate_token();
    let auth_state_json = serde_json::to_value(&auth_state)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize auth state: {e}")))?;

    let challenge_data = serde_json::json!({
        "authentication_state": auth_state_json,
        "email": email_for_challenge,
        "discoverable": false,
        "purpose": PASSKEY_LOGIN,
    });
    redis_ops::store_webauthn_challenge(&state.kv, &challenge_id, &challenge_data).await?;

    Ok(Json(serde_json::json!({
        "challenge_id": challenge_id,
        "options": rcr,
    })))
}

// ---------------------------------------------------------------------------
// POST /auth/passkeys/login/complete
// ---------------------------------------------------------------------------

async fn login_complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<LoginCompleteRequest>,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    // Rate limit
    let ip = extract_client_ip(&headers);
    let rate_result = rate_limiter::check_rate_limit(&state.kv, &ip, "login", None).await?;
    if !rate_result.allowed {
        tracing::warn!(ip = %ip, "Passkey login/complete rate limited");
        return Err(kyomi_core::Error::TooManyRequests(
            format!("Rate limited. Try again in {} seconds", rate_result.retry_after_secs),
            rate_result.retry_after_secs,
        ));
    }

    // Get challenge from Redis
    let challenge_data =
        redis_ops::get_webauthn_challenge(&state.kv, &data.challenge_id)
            .await?
            .ok_or_else(|| {
                tracing::warn!(ip = %ip, "Passkey login: invalid or expired challenge");
                kyomi_core::Error::BadRequest("Invalid or expired challenge".into())
            })?;

    // Delete challenge (prevent replay)
    redis_ops::delete_webauthn_challenge(&state.kv, &data.challenge_id).await?;

    // Reject a challenge minted by any other flow (KYO-279/KYO-280) — e.g. a
    // signup, add-device, or recovery registration challenge replayed here to
    // authenticate as the credential's owner. Same rejection as "not found"
    // so this can't be used to probe purpose.
    require_purpose(&challenge_data, &[PASSKEY_LOGIN]).inspect_err(|_| {
        // Logged, not surfaced — the response stays identical to "not found"
        // above, which also warns. Symmetric logging keeps a cross-flow replay
        // attempt as visible as an expired challenge.
        tracing::warn!(ip = %ip, "Passkey login: challenge minted by another flow");
    })?;

    // Find the user by credential ID
    let cred_id_bytes: &[u8] = data.credential.raw_id.as_ref();
    let credential_id_b64 = URL_SAFE_NO_PAD.encode(cred_id_bytes);

    // Look up user by credential ID
    let user = user_service::find_user_by_credential_id(&state.db, &credential_id_b64)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Unauthorized("User not found for credential".into())
        })?;

    // Check email verification BEFORE expensive WebAuthn verification
    if !user.verified {
        return Err(kyomi_core::Error::Forbidden(
            "Please verify your email before signing in. Check your inbox for the verification link.".into(),
        ));
    }

    // Get the authentication state
    let is_discoverable = challenge_data["discoverable"].as_bool().unwrap_or(false);

    if is_discoverable {
        // Discoverable credential flow — deserialize the DiscoverableAuthentication state
        // that was created at login_start time, inject the user's real credentials, and verify.
        let disc_state: DiscoverableAuthentication = serde_json::from_value(
            challenge_data["discoverable_state"].clone(),
        )
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("Deserialize discoverable state: {e}"))
        })?;

        let passkeys = get_passkeys_for_auth(&state.db, &user.user_id).await?;
        if passkeys.is_empty() {
            return Err(kyomi_core::Error::Unauthorized("No passkeys found for user".into()));
        }

        // finish_discoverable_authentication injects the real credentials into the state
        // and verifies the signature against the original challenge.
        let auth_result = wa::finish_discoverable_authentication(
            &state.webauthn,
            &data.credential,
            disc_state,
            &passkeys,
        )?;

        // Update credential usage
        let updated_passkey = passkeys.iter().find(|pk| {
            let pk_cred_id: &[u8] = pk.cred_id().as_ref();
            pk_cred_id == cred_id_bytes
        });

        if let Some(pk) = updated_passkey {
            let mut updated_pk = pk.clone();
            updated_pk.update_credential(&auth_result);
            let updated_json = serde_json::to_value(&updated_pk)
                .unwrap_or_default();
            user_service::update_credential_usage(
                &state.db,
                &user.user_id,
                &credential_id_b64,
                auth_result.counter(),
                &updated_json,
            )
            .await?;
        }
    } else {
        // Standard flow — use the stored authentication state
        let auth_state: PasskeyAuthentication = serde_json::from_value(
            challenge_data["authentication_state"].clone(),
        )
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("Deserialize auth state: {e}"))
        })?;

        let auth_result = wa::finish_authentication(
            &state.webauthn,
            &data.credential,
            &auth_state,
        )?;

        // Update credential usage
        let passkeys = get_passkeys_for_auth(&state.db, &user.user_id).await?;
        let updated_passkey = passkeys.iter().find(|pk| {
            let pk_cred_id: &[u8] = pk.cred_id().as_ref();
            pk_cred_id == cred_id_bytes
        });

        if let Some(pk) = updated_passkey {
            let mut updated_pk = pk.clone();
            updated_pk.update_credential(&auth_result);
            let updated_json = serde_json::to_value(&updated_pk)
                .unwrap_or_default();
            user_service::update_credential_usage(
                &state.db,
                &user.user_id,
                &credential_id_b64,
                auth_result.counter(),
                &updated_json,
            )
            .await?;
        }
    }

    // Create authenticated session
    let device = kyomi_auth::request_meta::extract_device_info(&headers);
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
        "message": "Passkey authentication successful",
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

// ===========================================================================
// Phase 3E: Management + Recovery (8 endpoints)
// ===========================================================================

// ---------------------------------------------------------------------------
// Request types (Phase 3E)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AddStartRequest {
    #[serde(default = "default_device_name")]
    device_name: String,
}

#[derive(Deserialize)]
struct AddCompleteRequest {
    challenge_id: String,
    credential: RegisterPublicKeyCredential,
}

#[derive(Deserialize)]
struct RenameRequest {
    device_name: String,
}

#[derive(Deserialize)]
struct RecoveryRequestBody {
    email: String,
}

#[derive(Deserialize)]
struct RecoveryVerifyRequest {
    token: String,
    #[serde(default = "default_recovery_device")]
    device_name: String,
}

fn default_recovery_device() -> String {
    "Recovery Device".to_string()
}

#[derive(Deserialize)]
struct RecoveryRegisterRequest {
    challenge_id: String,
    credential: RegisterPublicKeyCredential,
    #[serde(default = "default_recovery_device")]
    device_name: String,
}

// ---------------------------------------------------------------------------
// GET /auth/passkeys/list
// ---------------------------------------------------------------------------

async fn passkeys_list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let creds = user_service::get_passkey_credentials(&state.db, &user.user_id).await?;

    let credentials: Vec<serde_json::Value> = creds
        .iter()
        .map(|(cred_id, data)| {
            serde_json::json!({
                "credential_id": cred_id,
                "device_name": data.get("device_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unnamed Device"),
                "created_at": data.get("created_at")
                    .and_then(|v| v.as_str()),
                "last_used": data.get("last_used")
                    .and_then(|v| v.as_str()),
            })
        })
        .collect();

    let count = credentials.len();

    Ok(Json(serde_json::json!({
        "credentials": credentials,
        "count": count,
    })))
}

// ---------------------------------------------------------------------------
// POST /auth/passkeys/add/start
// ---------------------------------------------------------------------------

async fn add_start(
    State(state): State<AppState>,
    user: AuthUser,
    body: Option<Json<AddStartRequest>>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let device_name = body
        .map(|b| b.0.device_name)
        .unwrap_or_else(|| "Unknown Device".to_string());

    let db_user = user_service::get_user_by_id(&state.db, &user.user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))?;

    let email = &db_user.email;
    let display_name = db_user.name.as_deref().unwrap_or(email);
    let user_unique_id = webauthn_user_id(email);

    let exclude = get_exclude_credential_ids(&state.db, &user.user_id).await?;
    let exclude_opt = if exclude.is_empty() {
        None
    } else {
        Some(exclude)
    };

    let (ccr, reg_state) = wa::start_registration(
        &state.webauthn,
        user_unique_id,
        email,
        display_name,
        exclude_opt,
    )?;

    let challenge_id = redis_ops::generate_token();
    let reg_state_json = serde_json::to_value(&reg_state)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize reg state: {e}")))?;

    let challenge_data = serde_json::json!({
        "registration_state": reg_state_json,
        "email": email,
        "user_name": display_name,
        "user_id": user.user_id,
        "device_name": device_name,
        "purpose": PASSKEY_ADD_DEVICE,
    });
    redis_ops::store_webauthn_challenge(&state.kv, &challenge_id, &challenge_data).await?;

    Ok(Json(serde_json::json!({
        "challenge_id": challenge_id,
        "options": ccr,
    })))
}

// ---------------------------------------------------------------------------
// POST /auth/passkeys/add/complete
// ---------------------------------------------------------------------------

async fn add_complete(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<AddCompleteRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Get challenge from Redis
    let challenge_data =
        redis_ops::get_webauthn_challenge(&state.kv, &data.challenge_id)
            .await?
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Invalid or expired challenge".into())
            })?;

    // Delete challenge
    redis_ops::delete_webauthn_challenge(&state.kv, &data.challenge_id).await?;

    // Reject a challenge minted by any other flow (KYO-279/KYO-280) — e.g. an
    // unauthenticated signup or recovery challenge replayed here to attach an
    // attacker-controlled passkey to an authenticated session's account.
    // Same rejection as "not found" so this can't be used to probe purpose.
    require_purpose(&challenge_data, &[PASSKEY_ADD_DEVICE])?;

    // Verify the registration state matches this user
    let challenge_user_id = challenge_data["user_id"].as_str().unwrap_or("");
    if challenge_user_id != user.user_id {
        return Err(kyomi_core::Error::BadRequest(
            "Challenge does not match authenticated user".into(),
        ));
    }

    let reg_state: PasskeyRegistration = serde_json::from_value(
        challenge_data["registration_state"].clone(),
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("Deserialize reg state: {e}")))?;

    let device_name = challenge_data["device_name"]
        .as_str()
        .unwrap_or("Unknown Device");

    // Verify credential
    let passkey = wa::finish_registration(
        &state.webauthn,
        &data.credential,
        &reg_state,
    )?;

    let cred_id_bytes: &[u8] = passkey.cred_id().as_ref();
    let credential_id_b64 = URL_SAFE_NO_PAD.encode(cred_id_bytes);

    let passkey_json = serde_json::to_value(&passkey)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize passkey: {e}")))?;

    let public_key_b64 = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&passkey)
            .map_err(|e| kyomi_core::Error::Internal(format!("Serialize passkey bytes: {e}")))?
    );

    let initial_counter = passkey_json
        .get("cred")
        .and_then(|c| c.get("counter"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;

    // Store credential — no auto-login
    user_service::add_passkey_to_user(
        &state.db,
        &user.user_id,
        &credential_id_b64,
        &public_key_b64,
        initial_counter,
        device_name,
        &passkey_json,
    )
    .await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "credential_id": credential_id_b64,
        "device_name": device_name,
        "message": "Passkey added successfully",
    })))
}

// ---------------------------------------------------------------------------
// DELETE /auth/passkeys/{credential_id}
// ---------------------------------------------------------------------------

async fn passkey_delete(
    State(state): State<AppState>,
    user: AuthUser,
    Path(credential_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    match user_service::delete_passkey_from_user(&state.db, &user.user_id, &credential_id).await? {
        None => Ok(Json(serde_json::json!({
            "success": true,
            "message": "Passkey deleted successfully",
        }))),
        Some(error_msg) => {
            if error_msg.contains("not found") {
                Err(kyomi_core::Error::NotFound(error_msg.into()))
            } else {
                Err(kyomi_core::Error::BadRequest(error_msg.into()))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PATCH /auth/passkeys/{credential_id}
// ---------------------------------------------------------------------------

async fn passkey_rename(
    State(state): State<AppState>,
    user: AuthUser,
    Path(credential_id): Path<String>,
    Json(data): Json<RenameRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let trimmed = data.device_name.trim().to_string();

    if trimmed.is_empty() {
        return Err(kyomi_core::Error::BadRequest(
            "Device name cannot be empty".into(),
        ));
    }

    if trimmed.len() > 100 {
        return Err(kyomi_core::Error::BadRequest(
            "Device name cannot exceed 100 characters".into(),
        ));
    }

    let updated = user_service::update_passkey_device_name(
        &state.db,
        &user.user_id,
        &credential_id,
        &trimmed,
    )
    .await?;

    if !updated {
        return Err(kyomi_core::Error::NotFound("Passkey not found".into()));
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "device_name": trimmed,
        "message": "Passkey updated successfully",
    })))
}

// ---------------------------------------------------------------------------
// POST /auth/passkeys/recovery/request
// ---------------------------------------------------------------------------

/// Look up the user for the enumeration-resistant passkey-recovery path.
///
/// Returns `None` both when no such user exists and when the lookup fails —
/// callers must not be able to distinguish those, or the endpoint leaks
/// account existence. The difference is recorded in the logs instead, so a
/// database outage on the recovery path is visible to operators rather than
/// silently returning success to every caller.
async fn lookup_recovery_user(db: &kyomi_core::DbPool, email: &str) -> Option<User> {
    match user_service::get_user_by_email(db, email).await {
        Ok(user) => user,
        Err(e) => {
            tracing::error!(error = %e, "passkey recovery: user lookup failed");
            None
        }
    }
}

async fn recovery_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<RecoveryRequestBody>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let success_msg = "If a verified account exists with this email, a recovery link has been sent.";

    // Rate limit
    let ip = extract_client_ip(&headers);
    let rate_result =
        rate_limiter::check_rate_limit(&state.kv, &ip, "passkey_recovery", None).await?;
    if !rate_result.allowed {
        tracing::warn!(ip = %ip, "Passkey recovery/request rate limited");
        return Err(kyomi_core::Error::TooManyRequests(
            format!("Rate limited. Try again in {} seconds", rate_result.retry_after_secs),
            rate_result.retry_after_secs,
        ));
    }

    let email = data.email.to_lowercase().trim().to_string();

    // Always return success to prevent enumeration — do work silently
    let user = lookup_recovery_user(&state.db, &email).await;

    if let Some(user) = user
        && user.verified
    {
        // Create recovery token (15 min = 0.25 hours)
        match token_service::create_verification_token_with_expiry(
            &state.db,
            &email,
            "passkey_recovery",
            Some(0.25),
        )
        .await
        {
            Ok(raw_token) => {
                let recovery_url = format!(
                    "{}/auth/recover-passkey/complete?token={raw_token}",
                    state.config.frontend_url.trim_end_matches('/')
                );

                // Send recovery email
                let user_name = user.name.clone().unwrap_or_default();
                let email_clone = email.clone();
                let url_clone = recovery_url.clone();
                tokio::spawn(async move {
                    let email_svc = kyomi_auth::email_service::EmailService::from_env();
                    let sent = email_svc
                        .send_passkey_recovery(&email_clone, &user_name, &url_clone)
                        .await;
                    if sent {
                        tracing::info!("📧 Passkey recovery email sent to {email_clone}");
                    } else {
                        tracing::warn!(
                            "⚠️ Failed to send passkey recovery email to {email_clone}"
                        );
                        tracing::info!(
                            "📧 PASSKEY RECOVERY LINK for {email_clone}: {url_clone}"
                        );
                    }
                });
            }
            Err(e) => {
                tracing::error!(error = %e, "passkey recovery: token creation failed");
            }
        }
    }

    Ok(Json(serde_json::json!({
        "message": success_msg,
    })))
}

// ---------------------------------------------------------------------------
// POST /auth/passkeys/recovery/verify
// ---------------------------------------------------------------------------

async fn recovery_verify(
    State(state): State<AppState>,
    Json(data): Json<RecoveryVerifyRequest>,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    // Verify recovery token (one-time use)
    let email = token_service::verify_verification_token(
        &state.db,
        &data.token,
        "passkey_recovery",
    )
    .await?
    .ok_or_else(|| {
        tracing::warn!("Passkey recovery/verify: invalid or expired token");
        kyomi_core::Error::BadRequest(
            "Invalid or expired recovery link. Please request a new one.".into(),
        )
    })?;

    // Get user
    let user = user_service::get_user_by_email(&state.db, &email)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Internal("User not found for recovery token".into())
        })?;

    // Create limited-scope recovery JWT (15 min)
    let mut extra = std::collections::HashMap::new();
    extra.insert("user_id".into(), serde_json::json!(&user.user_id));
    extra.insert("email".into(), serde_json::json!(&email));
    extra.insert("scope".into(), serde_json::json!("passkey_recovery"));

    let recovery_jwt = jwt::create_access_token_str(
        &user.user_id,
        &state.config.jwt_secret,
        15, // 15 minutes
        extra,
    )?;

    // Generate WebAuthn registration challenge
    let user_unique_id = webauthn_user_id(&email);
    let display_name = user.name.as_deref().unwrap_or(&email);

    let exclude = get_exclude_credential_ids(&state.db, &user.user_id).await?;
    let exclude_opt = if exclude.is_empty() {
        None
    } else {
        Some(exclude)
    };

    let (ccr, reg_state) = wa::start_registration(
        &state.webauthn,
        user_unique_id,
        &email,
        display_name,
        exclude_opt,
    )?;

    let challenge_id = redis_ops::generate_token();
    let reg_state_json = serde_json::to_value(&reg_state)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize reg state: {e}")))?;

    let challenge_data = serde_json::json!({
        "registration_state": reg_state_json,
        "email": email,
        "user_name": display_name,
        "user_id": user.user_id,
        "device_name": data.device_name,
        "purpose": PASSKEY_RECOVERY,
    });
    redis_ops::store_webauthn_challenge(&state.kv, &challenge_id, &challenge_data).await?;

    // Set recovery session cookie
    let mut cookie_headers = HeaderMap::new();
    cookies::set_recovery_session_cookie(&mut cookie_headers, &recovery_jwt);

    let body = serde_json::json!({
        "status": "ready_for_passkey",
        "challenge_id": challenge_id,
        "options": ccr,
        "user": {
            "email": email,
            "name": display_name,
        },
    });

    Ok((cookie_headers, Json(body)))
}

// ---------------------------------------------------------------------------
// POST /auth/passkeys/recovery/register
// ---------------------------------------------------------------------------

async fn recovery_register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<RecoveryRegisterRequest>,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    // Validate recovery session cookie
    let recovery_token = cookies::get_cookie_value(&headers, "recovery_session")
        .ok_or_else(|| {
            tracing::warn!("Passkey recovery/register: missing recovery_session cookie");
            kyomi_core::Error::Unauthorized(
                "Recovery session expired or not found. Please start the recovery process again."
                    .into(),
            )
        })?;

    // Validate JWT
    let token_data = jwt::validate_token(recovery_token, &state.config.jwt_secret)
        .map_err(|_| {
            tracing::warn!("Passkey recovery/register: invalid or expired recovery JWT");
            kyomi_core::Error::Unauthorized(
                "Invalid or expired recovery session. Please start the recovery process again."
                    .into(),
            )
        })?;

    // Verify scope
    let scope = token_data
        .claims
        .extra
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if scope != "passkey_recovery" {
        tracing::warn!("Passkey recovery/register: JWT has wrong scope: {scope}");
        return Err(kyomi_core::Error::Unauthorized(
            "Invalid session type. This session cannot be used for passkey recovery.".into(),
        ));
    }

    let recovery_user_id = token_data
        .claims
        .extra
        .get("user_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            kyomi_core::Error::Internal("Missing user_id in recovery session".into())
        })?;
    let recovery_email = token_data
        .claims
        .extra
        .get("email")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            kyomi_core::Error::Internal("Missing email in recovery session".into())
        })?;

    // Get challenge
    let challenge_data =
        redis_ops::get_webauthn_challenge(&state.kv, &data.challenge_id)
            .await?
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Invalid or expired challenge".into())
            })?;

    // Always clean up the challenge
    redis_ops::delete_webauthn_challenge(&state.kv, &data.challenge_id).await?;

    // Reject a challenge minted by any other flow (KYO-279/KYO-280) — e.g. a
    // signup or add-device challenge replayed here to register a passkey
    // under the recovery session's user without the account's original
    // consent. Same rejection as "not found" so this can't be used to probe
    // purpose.
    require_purpose(&challenge_data, &[PASSKEY_RECOVERY])?;

    // Verify challenge email matches recovery session
    let challenge_email = challenge_data["email"].as_str().unwrap_or("");
    if challenge_email != recovery_email {
        return Err(kyomi_core::Error::Unauthorized(
            "Challenge does not match recovery session".into(),
        ));
    }

    // Verify user still exists
    let user = user_service::get_user_by_id(&state.db, recovery_user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Unauthorized("User not found".into()))?;

    // Verify registration
    let reg_state: PasskeyRegistration = serde_json::from_value(
        challenge_data["registration_state"].clone(),
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("Deserialize reg state: {e}")))?;

    let passkey = wa::finish_registration(
        &state.webauthn,
        &data.credential,
        &reg_state,
    )?;

    let cred_id_bytes: &[u8] = passkey.cred_id().as_ref();
    let credential_id_b64 = URL_SAFE_NO_PAD.encode(cred_id_bytes);

    let passkey_json = serde_json::to_value(&passkey)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize passkey: {e}")))?;

    let public_key_b64 = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&passkey)
            .map_err(|e| kyomi_core::Error::Internal(format!("Serialize passkey bytes: {e}")))?
    );

    let initial_counter = passkey_json
        .get("cred")
        .and_then(|c| c.get("counter"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;

    // Store credential — no auto-login for recovery
    user_service::add_passkey_to_user(
        &state.db,
        &user.user_id,
        &credential_id_b64,
        &public_key_b64,
        initial_counter,
        &data.device_name,
        &passkey_json,
    )
    .await?;

    // Clear recovery session cookie
    let mut cookie_headers = HeaderMap::new();
    cookies::clear_recovery_session_cookie(&mut cookie_headers);

    let body = serde_json::json!({
        "status": "success",
        "message": "New passkey registered successfully. Please sign in with your new passkey.",
    });

    Ok((cookie_headers, Json(body)))
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::sync::{Arc, Mutex};

    use tracing::Level;
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::{Layer, Registry};
    use webauthn_rs::prelude::Passkey;

    use super::{
        lookup_recovery_user, require_purpose, PASSKEY_ADD_DEVICE, PASSKEY_LOGIN,
        PASSKEY_RECOVERY, PASSKEY_SIGNUP,
    };

    /// Shared sink that a `CaptureLayer` writes tracing events into.
    type EventLog = Arc<Mutex<Vec<(Level, String)>>>;

    /// Minimal `tracing_subscriber::Layer` that records every event's level
    /// and rendered fields, so tests can assert on log output without a real
    /// subscriber (fmt/json) formatting to stdout.
    struct CaptureLayer(EventLog);

    impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            struct FieldVisitor(String);
            impl tracing::field::Visit for FieldVisitor {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "message" {
                        let _ = write!(self.0, "{value:?}");
                    } else {
                        let _ = write!(self.0, " {}={value:?}", field.name());
                    }
                }
            }

            let mut visitor = FieldVisitor(String::new());
            event.record(&mut visitor);
            self.0
                .lock()
                .expect("capture lock poisoned")
                .push((*event.metadata().level(), visitor.0));
        }
    }

    /// `lookup_recovery_user` must return `None` when the database lookup
    /// itself fails (not merely when no user exists) — and it must say so
    /// via an `error!`-level log rather than swallowing the error, since a
    /// DB outage on the recovery path would otherwise silently return
    /// success to every caller. See KYO-215.
    #[tokio::test(flavor = "current_thread")]
    async fn lookup_recovery_user_db_error_returns_none_and_logs_error() {
        let db = kyomi_core::DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        match &db {
            kyomi_core::DbPool::Sqlite(pool) => pool.close().await,
            kyomi_core::DbPool::Postgres(_) => panic!("expected sqlite pool"),
        }

        let email = "db-error-recovery-test@example.com";
        let captured: EventLog = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(CaptureLayer(captured.clone()));
        let guard = tracing::subscriber::set_default(subscriber);

        let result = lookup_recovery_user(&db, email).await;

        drop(guard);

        assert!(
            result.is_none(),
            "a DB error must be reported as no user, not surfaced to the caller"
        );

        let events = captured.lock().expect("capture lock poisoned").clone();
        let error_events: Vec<&(Level, String)> =
            events.iter().filter(|(level, _)| *level == Level::ERROR).collect();
        assert!(
            !error_events.is_empty(),
            "a DB error during passkey recovery must emit an error!-level log; captured: {events:?}"
        );
        assert!(
            error_events
                .iter()
                .any(|(_, message)| message.contains("user lookup failed")),
            "expected an error log describing the lookup failure; captured: {events:?}"
        );
        for (_, message) in &error_events {
            assert!(
                !message.contains(email),
                "recovery error logs must never contain the requester's email \
                 (that would reintroduce the enumeration leak through logs); got: {message}"
            );
        }
    }

    /// The absent-user case must NOT log an error — only the DB-failure case
    /// should. Without this test, a broad "always log an error" change would
    /// pass the test above while defeating the entire point of the fix: the
    /// two enumeration-resistant outcomes (no such user vs. DB down) must
    /// stay distinguishable in the logs.
    #[tokio::test(flavor = "current_thread")]
    async fn lookup_recovery_user_absent_user_returns_none_without_error_log() {
        let db = kyomi_core::DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        let email = "absent-recovery-test@example.com";
        let captured: EventLog = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(CaptureLayer(captured.clone()));
        let guard = tracing::subscriber::set_default(subscriber);

        let result = lookup_recovery_user(&db, email).await;

        drop(guard);

        assert!(result.is_none(), "no such user should look up to None");

        let events = captured.lock().expect("capture lock poisoned").clone();
        let error_events: Vec<&(Level, String)> =
            events.iter().filter(|(level, _)| *level == Level::ERROR).collect();
        assert!(
            error_events.is_empty(),
            "a merely-absent user must not log an error — only a DB failure should; captured: {events:?}"
        );
    }

    /// Verify that the JSON format produced by the Alembic migration
    /// (774c005b0f00_migrate_python_passkeys_to_rust_format.py)
    /// can be deserialized by webauthn-rs 0.5.4.
    #[test]
    fn deserialize_migrated_passkey_json() {
        // Use properly-generated base64url-no-pad values (32 bytes each).
        let json = serde_json::json!({
            "cred": {
                "cred_id": "lLemfAbafh8fITA-hRAzYxuk3f6U42wM7-fYnoiodeo",
                "cred": {
                    "type_": "ES256",
                    "key": {
                        "EC_EC2": {
                            "curve": "SECP256R1",
                            "x": "3sfFdW2_SjhozsQJYUIJVFKy3jvMEaCs6IpWhmndx-g",
                            "y": "R34op1BMjd1edprK6zX0ghM6nZODDTNhvDcrN84lQwc"
                        }
                    }
                },
                "counter": 5,
                "transports": null,
                "user_verified": true,
                "backup_eligible": true,
                "backup_state": true,
                "registration_policy": "required",
                "extensions": {
                    "cred_protect": "NotRequested",
                    "hmac_create_secret": "NotRequested",
                    "appid": "NotRequested",
                    "cred_props": "NotRequested"
                },
                "attestation": {
                    "data": "None",
                    "metadata": "None"
                },
                "attestation_format": "none"
            }
        });

        let passkey: Passkey = serde_json::from_value(json)
            .expect("Migration JSON must deserialize into webauthn-rs Passkey");

        assert_eq!(passkey.cred_id().len(), 32);
        assert_eq!(*passkey.cred_algorithm(), webauthn_rs::prelude::COSEAlgorithm::ES256);
    }

    // -----------------------------------------------------------------------
    // Tests — WebAuthn challenge purpose binding (KYO-279/KYO-280)
    // -----------------------------------------------------------------------
    //
    // This is the only REST implementation of the passkey subsystem; every
    // other consumer of `webauthn_challenge_purpose::has_purpose` lives in
    // `kyomi-auth` and is tested there. `require_purpose` is this file's thin
    // wrapper mapping a purpose failure onto the same "invalid or expired
    // challenge" rejection the not-found path already returns — asserting
    // that equality *is* the security property under test, since a
    // distinguishable message would let a caller probe which purpose a given
    // `challenge_id` was minted with.

    /// Assert a purpose-gate rejection: the exact `Error::BadRequest` message
    /// the "challenge not found" path also uses — no distinguishing oracle.
    fn assert_invalid_or_expired_challenge(result: Result<(), kyomi_core::Error>) {
        match result {
            Err(kyomi_core::Error::BadRequest(msg)) => {
                assert_eq!(msg, "Invalid or expired challenge");
            }
            Err(other) => panic!(
                "expected Error::BadRequest(\"Invalid or expired challenge\"), got Err({other})"
            ),
            Ok(()) => {
                panic!("expected Error::BadRequest(\"Invalid or expired challenge\"), got Ok")
            }
        }
    }

    #[test]
    fn require_purpose_accepts_its_own_purpose() {
        let data = serde_json::json!({"purpose": PASSKEY_LOGIN});
        assert!(require_purpose(&data, &[PASSKEY_LOGIN]).is_ok());
    }

    #[test]
    fn require_purpose_accepts_any_purpose_in_a_multi_value_allowlist() {
        let data = serde_json::json!({"purpose": PASSKEY_RECOVERY});
        assert!(require_purpose(&data, &[PASSKEY_SIGNUP, PASSKEY_RECOVERY]).is_ok());
    }

    #[test]
    fn require_purpose_rejects_a_purpose_minted_for_a_different_flow() {
        // Cross-flow confusion in every direction: each real purpose value,
        // checked against every allowlist that does not contain it.
        let all = [
            PASSKEY_LOGIN,
            PASSKEY_SIGNUP,
            PASSKEY_RECOVERY,
            PASSKEY_ADD_DEVICE,
        ];
        for minted in all {
            for allowed in all {
                if minted == allowed {
                    continue;
                }
                let data = serde_json::json!({"purpose": minted});
                assert_invalid_or_expired_challenge(require_purpose(&data, &[allowed]));
            }
        }
    }

    #[test]
    fn require_purpose_rejects_missing_purpose_field() {
        let data = serde_json::json!({"user_id": "u1"});
        assert_invalid_or_expired_challenge(require_purpose(&data, &[PASSKEY_LOGIN]));
    }

    #[test]
    fn require_purpose_rejects_null_purpose_field() {
        let data = serde_json::json!({"purpose": null});
        assert_invalid_or_expired_challenge(require_purpose(&data, &[PASSKEY_LOGIN]));
    }

    #[test]
    fn require_purpose_rejects_unrecognised_purpose_value() {
        let data = serde_json::json!({"purpose": "totally_made_up"});
        assert_invalid_or_expired_challenge(require_purpose(
            &data,
            &[
                PASSKEY_LOGIN,
                PASSKEY_SIGNUP,
                PASSKEY_RECOVERY,
                PASSKEY_ADD_DEVICE,
            ],
        ));
    }
}
