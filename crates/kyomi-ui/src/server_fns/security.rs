// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for Security settings.
//!
//! These replace the REST API calls for password, TOTP, session, and passkey management:
//! - `POST /auth/set-password` -> `set_password()`
//! - `POST /auth/change-password` -> `change_password()`
//! - `GET  /auth/2fa/status` -> `get_totp_status()`
//! - `POST /auth/2fa/setup` -> `setup_totp()`
//! - `POST /auth/2fa/enable` -> `enable_totp()`
//! - `POST /auth/2fa/disable` -> `disable_totp()`
//! - `GET  /auth/sessions` -> `get_sessions()`
//! - `DELETE /auth/sessions/{id}` -> `revoke_session()`
//! - `POST /auth/logout-all` -> `logout_all_sessions()`
//! - `GET  /auth/passkeys/list` -> `list_passkeys()`
//! - `POST /auth/passkeys/add/start` -> `start_passkey_registration()`
//! - `POST /auth/passkeys/add/complete` -> `complete_passkey_registration()`
//! - `DELETE /auth/passkeys/{id}` -> `delete_passkey()`
//! - `PATCH /auth/passkeys/{id}` -> `rename_passkey()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/auth_password.rs`,
//! `apps/server/src/routes/auth_totp.rs`, `apps/server/src/routes/auth_passkeys.rs`,
//! and `apps/server/src/routes/auth.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context};

/// TOTP status returned by `get_totp_status()`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TotpStatus {
    pub enabled: bool,
}

/// TOTP setup data returned by `setup_totp()`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TotpSetup {
    pub secret: String,
    pub qr_uri: String,
}

/// Check whether the current user has a password set.
#[server(prefix = "/leptos-api")]
pub async fn has_password() -> Result<bool, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    kyomi_auth::user_service::has_password(&ctx.db, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Set a password for a user who does not yet have one (e.g. OAuth-only users).
///
/// Mirrors the validation in `apps/server/src/routes/auth_password.rs::set_password`:
/// - Password must be at least 8 characters.
/// - User must NOT already have a password.
#[server(prefix = "/leptos-api")]
pub async fn set_password(new_password: String) -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    // Validate password length
    if new_password.len() < 8 {
        return Err(ServerFnError::new(
            "Password must be at least 8 characters",
        ));
    }

    // Check user does NOT already have a password
    let has_pw = kyomi_auth::user_service::has_password(&ctx.db, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if has_pw {
        return Err(ServerFnError::new(
            "Password already set. Use change-password to update it.",
        ));
    }

    // Hash and store
    let hash = kyomi_auth::password::hash_password(&new_password)
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let auth_data = serde_json::json!({"hash": hash});
    kyomi_auth::user_service::upsert_auth_method(&ctx.db, &auth.user_id, "password", &auth_data)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok("Password set successfully".to_string())
}

/// Change password for a user who already has one.
///
/// Mirrors the validation in `apps/server/src/routes/auth_password.rs::change_password`:
/// - New password must be at least 8 characters.
/// - Current password must be verified first.
#[server(prefix = "/leptos-api")]
// lint-allow: server-fn-callouts=pre-existing orchestration drift tracked in KYO-124
pub async fn change_password(
    current_password: String,
    new_password: String,
) -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    // Validate new password length
    if new_password.len() < 8 {
        return Err(ServerFnError::new(
            "New password must be at least 8 characters",
        ));
    }

    // Get existing password auth method
    let password_method =
        kyomi_auth::user_service::get_auth_method(&ctx.db, &auth.user_id, "password")
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    let Some(password_method) = password_method else {
        return Err(ServerFnError::new(
            "No password set. Use set-password to create one.",
        ));
    };

    // Extract and verify current password
    let Some(hash) = password_method
        .auth_data
        .get("hash")
        .and_then(|v| v.as_str())
    else {
        return Err(ServerFnError::new("Password auth method corrupted"));
    };

    let valid = kyomi_auth::password::verify_password(&current_password, hash)
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if !valid {
        return Err(ServerFnError::new("Current password is incorrect"));
    }

    // Hash new password and upsert
    let new_hash = kyomi_auth::password::hash_password(&new_password)
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let auth_data = serde_json::json!({"hash": new_hash});
    kyomi_auth::user_service::upsert_auth_method(&ctx.db, &auth.user_id, "password", &auth_data)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok("Password changed successfully".to_string())
}

// ---------------------------------------------------------------------------
// TOTP 2FA server functions
// ---------------------------------------------------------------------------

/// Check whether the current user has TOTP 2FA enabled.
///
/// Mirrors `GET /auth/2fa/status` in `apps/server/src/routes/auth_totp.rs`.
#[server(prefix = "/leptos-api")]
pub async fn get_totp_status() -> Result<TotpStatus, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let enabled = kyomi_auth::user_service::has_totp_enabled(&ctx.db, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(TotpStatus { enabled })
}

/// Begin 2FA setup: generate a secret and QR code data URI.
///
/// The secret is stored in Redis (10 min TTL) until the user confirms with
/// a verification code via `enable_totp()`.
///
/// Mirrors `POST /auth/2fa/setup` in `apps/server/src/routes/auth_totp.rs`.
#[server(prefix = "/leptos-api")]
// lint-allow: server-fn-callouts=pre-existing orchestration drift tracked in KYO-124
pub async fn setup_totp() -> Result<TotpSetup, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    // Check not already enabled
    let enabled = kyomi_auth::user_service::has_totp_enabled(&ctx.db, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if enabled {
        return Err(ServerFnError::new("2FA is already enabled"));
    }

    let secret = kyomi_auth::totp::generate_secret();
    let qr_uri = kyomi_auth::totp::generate_qr_code(&secret, &auth.email)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Store pending secret in Redis (10 min TTL)
    let kv = ctx
        .kv
        .as_ref()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;
    kyomi_auth::redis_ops::store_pending_totp(kv, &auth.user_id, &secret)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(TotpSetup { secret, qr_uri })
}

/// Confirm 2FA setup by verifying a TOTP code against the pending secret.
///
/// On success the secret is persisted in `user_auth_methods`. On failure the
/// pending secret is re-stored in Redis so the user can retry.
///
/// Mirrors `POST /auth/2fa/enable` in `apps/server/src/routes/auth_totp.rs`.
#[server(prefix = "/leptos-api")]
// lint-allow: server-fn-callouts=pre-existing orchestration drift tracked in KYO-124
pub async fn enable_totp(code: String) -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let kv = ctx
        .kv
        .as_ref()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Get pending secret from Redis (atomic get+delete)
    let secret = kyomi_auth::redis_ops::get_pending_totp(kv, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let Some(secret) = secret else {
        return Err(ServerFnError::new(
            "No pending 2FA setup found. Please start the setup process again.",
        ));
    };

    // Verify the code
    if !kyomi_auth::totp::verify_code(&secret, &code) {
        // Re-store secret so user can retry
        kyomi_auth::redis_ops::store_pending_totp(kv, &auth.user_id, &secret)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        return Err(ServerFnError::new(
            "Invalid verification code. Please try again.",
        ));
    }

    // Persist TOTP auth method
    let now = chrono::Utc::now().to_rfc3339();
    let auth_data = serde_json::json!({
        "secret": secret,
        "enabled_at": now,
    });
    kyomi_auth::user_service::upsert_auth_method(&ctx.db, &auth.user_id, "totp", &auth_data)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok("2FA has been successfully enabled".to_string())
}

/// Disable 2FA for the authenticated user.
///
/// Mirrors `POST /auth/2fa/disable` in `apps/server/src/routes/auth_totp.rs`.
/// The REST handler does not require a TOTP code — it simply removes the auth
/// method for the already-authenticated user.
#[server(prefix = "/leptos-api")]
pub async fn disable_totp() -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let enabled = kyomi_auth::user_service::has_totp_enabled(&ctx.db, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if !enabled {
        return Err(ServerFnError::new("2FA is not currently enabled"));
    }

    kyomi_auth::user_service::remove_auth_method(&ctx.db, &auth.user_id, "totp")
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok("2FA has been successfully disabled".to_string())
}

// ---------------------------------------------------------------------------
// Session management server functions
// ---------------------------------------------------------------------------

/// A single session entry returned by `get_sessions()`.
///
/// Maps to the `SessionInfo` fields from `kyomi_auth::token_service` plus
/// the `is_current` flag computed by comparing the caller's refresh token
/// cookie family against each session's family.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionEntry {
    pub token_id: String,
    pub created_at: String,
    pub last_used: Option<String>,
    pub expires_at: String,
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
    pub country_code: Option<String>,
    pub oauth_client_name: Option<String>,
    pub is_current: bool,
}

/// Get all active sessions for the current user.
///
/// Mirrors `GET /auth/sessions` in `apps/server/src/routes/auth.rs`.
/// Determines the current session by comparing the refresh token cookie's
/// family_id against each session's family_id.
#[server(prefix = "/leptos-api")]
// lint-allow: server-fn-callouts=pre-existing orchestration drift tracked in KYO-124
pub async fn get_sessions() -> Result<Vec<SessionEntry>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let sessions =
        kyomi_auth::token_service::get_user_sessions(&ctx.db, &auth.user_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Determine current session by looking up the family_id of the cookie's
    // refresh token — same approach as the REST handler in auth.rs.
    let headers: axum::http::HeaderMap = leptos_axum::extract().await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;

    let refresh_token_name = &kyomi_core::constants::get().cookies.refresh_token_name;

    #[derive(sqlx::FromRow)]
    struct FamilyIdRow {
        family_id: String,
    }

    let current_family_id: Option<String> =
        if let Some(raw_token) = kyomi_auth::cookies::get_cookie_value(&headers, refresh_token_name)
        {
            let hash = kyomi_auth::token_service::hash_refresh_token(raw_token);
            kyomi_core::db_fetch_optional!(
                &ctx.db,
                FamilyIdRow,
                "SELECT family_id FROM refresh_tokens WHERE token_hash = $1 AND is_active = true",
                &hash
            )
            .ok()
            .flatten()
            .map(|r| r.family_id)
        } else {
            None
        };

    let entries = sessions
        .iter()
        .map(|s| {
            let is_current = current_family_id.as_deref() == Some(&s.family_id);
            SessionEntry {
                token_id: s.token_id.clone(),
                created_at: s.created_at.to_rfc3339(),
                last_used: s.last_used.map(|dt| dt.to_rfc3339()),
                expires_at: s.expires_at.to_rfc3339(),
                user_agent: s.user_agent.clone(),
                ip_address: s.ip_address.clone(),
                country_code: s.country_code.clone(),
                oauth_client_name: s.oauth_client_name.clone(),
                is_current,
            }
        })
        .collect();

    Ok(entries)
}

/// Revoke a specific session by token ID.
///
/// Mirrors `DELETE /auth/sessions/{token_id}` in `apps/server/src/routes/auth.rs`.
/// Revokes the entire token family so rotated tokens in the same session are
/// also invalidated.
#[server(prefix = "/leptos-api")]
pub async fn revoke_session(token_id: String) -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let revoked =
        kyomi_auth::token_service::revoke_user_refresh_token(&ctx.db, &auth.user_id, &token_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    if !revoked {
        return Err(ServerFnError::new("Session not found"));
    }

    Ok("Session revoked successfully".to_string())
}

/// Log out the current session.
///
/// Mirrors `POST /auth/logout` in `apps/server/src/routes/auth.rs`:
/// 1. Reads the refresh token cookie.
/// 2. Revokes the entire token family so rotated tokens are also invalidated.
/// 3. Clears both auth cookies (access_token + refresh_token) via `ResponseOptions`.
///
/// Does NOT require `extract_auth()` — the token may already be invalid
/// (e.g. if the access token expired) but we still want to clear cookies.
#[server(prefix = "/leptos-api")]
// lint-allow: server-fn-callouts=pre-existing orchestration drift tracked in KYO-124
pub async fn logout() -> Result<(), ServerFnError> {
    let ctx = extract_context()?;

    // Extract the raw request headers to read the refresh token cookie.
    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;

    let refresh_token_name = &kyomi_core::constants::get().cookies.refresh_token_name;

    // Revoke the token family if we can find a valid refresh token.
    if let Some(raw_token) =
        kyomi_auth::cookies::get_cookie_value(&headers, refresh_token_name)
    {
        match kyomi_auth::token_service::verify_refresh_token(&ctx.db, raw_token).await {
            Ok(
                kyomi_auth::token_service::RefreshTokenVerifyResult::Valid(data)
                | kyomi_auth::token_service::RefreshTokenVerifyResult::GracePeriod(data),
            ) => {
                let _ =
                    kyomi_auth::token_service::revoke_token_family(&ctx.db, &data.family_id)
                        .await;
            }
            _ => {
                // Token invalid or theft-detected — already revoked, nothing to do.
            }
        }
    }

    // Clear both HTTPOnly cookies so the browser forgets the session.
    let response_options =
        leptos::prelude::expect_context::<leptos_axum::ResponseOptions>();
    let mut cookie_headers = axum::http::HeaderMap::new();
    kyomi_auth::cookies::clear_token_cookies(&mut cookie_headers);
    for (name, value) in cookie_headers.iter() {
        if name == axum::http::header::SET_COOKIE {
            response_options.append_header(name.clone(), value.clone());
        }
    }

    Ok(())
}

/// Log out from all devices by revoking every refresh token for the user.
///
/// Mirrors `POST /auth/logout-all` in `apps/server/src/routes/auth.rs`.
#[server(prefix = "/leptos-api")]
pub async fn logout_all_sessions() -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let revoked_count =
        kyomi_auth::token_service::revoke_all_user_refresh_tokens(&ctx.db, &auth.user_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(format!(
        "Logged out from all devices successfully ({revoked_count} sessions revoked)"
    ))
}

// ---------------------------------------------------------------------------
// Passkey management server functions
// ---------------------------------------------------------------------------

/// A single passkey entry returned by `list_passkeys()`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PasskeyInfo {
    pub credential_id: String,
    pub name: String,
    pub created_at: Option<String>,
    pub last_used: Option<String>,
}

/// List all passkeys for the authenticated user.
///
/// Mirrors `GET /auth/passkeys/list` in `apps/server/src/routes/auth_passkeys.rs`.
#[server(prefix = "/leptos-api")]
pub async fn list_passkeys() -> Result<Vec<PasskeyInfo>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let creds = kyomi_auth::user_service::get_passkey_credentials(&ctx.db, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let passkeys: Vec<PasskeyInfo> = creds
        .iter()
        .map(|(cred_id, data)| PasskeyInfo {
            credential_id: cred_id.clone(),
            name: data
                .get("device_name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unnamed Device")
                .to_string(),
            created_at: data
                .get("created_at")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            last_used: data
                .get("last_used")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        })
        .collect();

    Ok(passkeys)
}

/// Start passkey registration for the authenticated user.
///
/// Returns a JSON string containing the challenge_id and WebAuthn options
/// that the browser needs for `navigator.credentials.create()`.
///
/// Mirrors `POST /auth/passkeys/add/start` in `apps/server/src/routes/auth_passkeys.rs`.
#[server(prefix = "/leptos-api")]
// lint-allow: server-fn-callouts=pre-existing orchestration drift tracked in KYO-124
pub async fn start_passkey_registration(
    device_name: String,
) -> Result<String, ServerFnError> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};
    use webauthn_rs::prelude::*;

    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let webauthn = ctx
        .webauthn
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebAuthn not configured"))?;

    let kv = ctx
        .kv
        .as_ref()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    let device_name = if device_name.trim().is_empty() {
        "Unknown Device".to_string()
    } else {
        device_name.trim().to_string()
    };

    let db_user = kyomi_auth::user_service::get_user_by_id(&ctx.db, &auth.user_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("User not found"))?;

    let email = &db_user.email;
    let display_name = db_user.name.as_deref().unwrap_or(email);

    // Generate deterministic user handle from email (same as auth_passkeys.rs)
    let user_unique_id = {
        let mut hasher = Sha256::new();
        hasher.update(email.as_bytes());
        let hash = hasher.finalize();
        let bytes: [u8; 16] = hash[..16].try_into().expect("16 bytes");
        Uuid::from_bytes(bytes)
    };

    // Get existing credential IDs to exclude
    let creds = kyomi_auth::user_service::get_passkey_credentials(&ctx.db, &auth.user_id)
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
        email,
        display_name,
        exclude_opt,
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let challenge_id = kyomi_auth::redis_ops::generate_token();
    let reg_state_json = serde_json::to_value(&reg_state)
        .map_err(|e| ServerFnError::new(format!("Serialize reg state: {e}")))?;

    let challenge_data = serde_json::json!({
        "registration_state": reg_state_json,
        "email": email,
        "user_name": display_name,
        "user_id": auth.user_id,
        "device_name": device_name,
    });
    kyomi_auth::redis_ops::store_webauthn_challenge(kv, &challenge_id, &challenge_data)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Return JSON with challenge_id and options for the browser
    let result = serde_json::json!({
        "challenge_id": challenge_id,
        "options": ccr,
    });

    serde_json::to_string(&result)
        .map_err(|e| ServerFnError::new(format!("Serialize response: {e}")))
}

/// Complete passkey registration by verifying the browser credential.
///
/// Receives the challenge_id and the PublicKeyCredential JSON from the browser.
///
/// Mirrors `POST /auth/passkeys/add/complete` in `apps/server/src/routes/auth_passkeys.rs`.
#[server(prefix = "/leptos-api")]
// lint-allow: server-fn-callouts=pre-existing orchestration drift tracked in KYO-124
pub async fn complete_passkey_registration(
    credential_json: String,
) -> Result<String, ServerFnError> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use webauthn_rs::prelude::*;

    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let webauthn = ctx
        .webauthn
        .as_ref()
        .ok_or_else(|| ServerFnError::new("WebAuthn not configured"))?;

    let kv = ctx
        .kv
        .as_ref()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    // Parse the incoming JSON which contains challenge_id and credential
    let data: serde_json::Value = serde_json::from_str(&credential_json)
        .map_err(|e| ServerFnError::new(format!("Invalid credential JSON: {e}")))?;

    let challenge_id = data["challenge_id"]
        .as_str()
        .ok_or_else(|| ServerFnError::new("Missing challenge_id"))?;

    let credential: RegisterPublicKeyCredential =
        serde_json::from_value(data["credential"].clone())
            .map_err(|e| ServerFnError::new(format!("Invalid credential: {e}")))?;

    // Get challenge from KV
    let challenge_data =
        kyomi_auth::redis_ops::get_webauthn_challenge(kv, challenge_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .ok_or_else(|| ServerFnError::new("Invalid or expired challenge"))?;

    // Delete challenge (prevent replay)
    kyomi_auth::redis_ops::delete_webauthn_challenge(kv, challenge_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Verify the registration state matches this user
    let challenge_user_id = challenge_data["user_id"].as_str().unwrap_or("");
    if challenge_user_id != auth.user_id {
        return Err(ServerFnError::new(
            "Challenge does not match authenticated user",
        ));
    }

    let reg_state: PasskeyRegistration =
        serde_json::from_value(challenge_data["registration_state"].clone())
            .map_err(|e| ServerFnError::new(format!("Deserialize reg state: {e}")))?;

    let device_name = challenge_data["device_name"]
        .as_str()
        .unwrap_or("Unknown Device");

    // Verify credential
    let passkey = kyomi_auth::webauthn::finish_registration(webauthn, &credential, &reg_state)
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let cred_id_bytes: &[u8] = passkey.cred_id().as_ref();
    let credential_id_b64 = URL_SAFE_NO_PAD.encode(cred_id_bytes);

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

    // Store credential
    kyomi_auth::user_service::add_passkey_to_user(
        &ctx.db,
        &auth.user_id,
        &credential_id_b64,
        &public_key_b64,
        initial_counter,
        device_name,
        &passkey_json,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(format!("Passkey '{}' added successfully", device_name))
}

/// Delete a passkey for the authenticated user.
///
/// Mirrors `DELETE /auth/passkeys/{credential_id}` in `apps/server/src/routes/auth_passkeys.rs`.
#[server(prefix = "/leptos-api")]
pub async fn delete_passkey(credential_id: String) -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    match kyomi_auth::user_service::delete_passkey_from_user(
        &ctx.db,
        &auth.user_id,
        &credential_id,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?
    {
        None => Ok("Passkey deleted successfully".to_string()),
        Some(error_msg) => Err(ServerFnError::new(error_msg)),
    }
}

/// Rename a passkey for the authenticated user.
///
/// Mirrors `PATCH /auth/passkeys/{credential_id}` in `apps/server/src/routes/auth_passkeys.rs`.
#[server(prefix = "/leptos-api")]
pub async fn rename_passkey(
    credential_id: String,
    name: String,
) -> Result<String, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let trimmed = name.trim().to_string();

    if trimmed.is_empty() {
        return Err(ServerFnError::new("Device name cannot be empty"));
    }

    if trimmed.len() > 100 {
        return Err(ServerFnError::new(
            "Device name cannot exceed 100 characters",
        ));
    }

    let updated = kyomi_auth::user_service::update_passkey_device_name(
        &ctx.db,
        &auth.user_id,
        &credential_id,
        &trimmed,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    if !updated {
        return Err(ServerFnError::new("Passkey not found"));
    }

    Ok("Passkey renamed successfully".to_string())
}

