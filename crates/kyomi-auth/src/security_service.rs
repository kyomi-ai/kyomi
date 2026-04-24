// SPDX-License-Identifier: AGPL-3.0-or-later

//! Security service — orchestration for password, TOTP, session, and passkey management.
//!
//! Each function extracts the multi-step business logic that was previously
//! inlined in `kyomi-ui/src/server_fns/security.rs`, leaving those server
//! functions as thin wrappers.
//!
//! All functions take `&DbPool` as their first argument and return
//! `kyomi_core::Result<T>`. Functions that need the KV store also accept
//! `&kyomi_core::KVPool`. Functions that need WebAuthn accept
//! `&webauthn_rs::Webauthn`.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use webauthn_rs::prelude::*;

use kyomi_core::{DbPool, KVPool};

// ---------------------------------------------------------------------------
// Password management
// ---------------------------------------------------------------------------

/// Change a user's password.
///
/// Verifies `current_password` against the stored hash, then replaces it
/// with a bcrypt/argon2 hash of `new_password`. Callers must validate the
/// minimum length before calling this function.
///
/// Returns `Err` if the user has no password set, the current password is
/// wrong, or a database error occurs.
pub async fn change_password(
    pool: &DbPool,
    user_id: &str,
    current_password: &str,
    new_password: &str,
) -> kyomi_core::Result<()> {
    let password_method =
        crate::user_service::get_auth_method(pool, user_id, "password").await?;

    let Some(password_method) = password_method else {
        return Err(kyomi_core::Error::Internal(
            "No password set. Use set-password to create one.".into(),
        ));
    };

    let Some(hash) = password_method
        .auth_data
        .get("hash")
        .and_then(|v| v.as_str())
    else {
        return Err(kyomi_core::Error::Internal(
            "Password auth method corrupted".into(),
        ));
    };

    let valid = crate::password::verify_password(current_password, hash)?;
    if !valid {
        return Err(kyomi_core::Error::Internal(
            "Current password is incorrect".into(),
        ));
    }

    let new_hash = crate::password::hash_password(new_password)?;
    let auth_data = serde_json::json!({"hash": new_hash});
    crate::user_service::upsert_auth_method(pool, user_id, "password", &auth_data).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// TOTP 2FA
// ---------------------------------------------------------------------------

/// Result of `setup_totp_service` — the TOTP secret and QR code data URI.
pub struct TotpSetupResult {
    pub secret: String,
    pub qr_uri: String,
}

/// Begin TOTP 2FA setup for a user.
///
/// Checks that TOTP is not already enabled, generates a new secret and QR
/// code URI, then stores the pending secret in the KV store (10 min TTL)
/// until the user confirms with `enable_totp`.
///
/// Returns `Err` if TOTP is already enabled or the KV store fails.
pub async fn setup_totp(
    pool: &DbPool,
    kv: &KVPool,
    user_id: &str,
    email: &str,
) -> kyomi_core::Result<TotpSetupResult> {
    let enabled = crate::user_service::has_totp_enabled(pool, user_id).await?;
    if enabled {
        return Err(kyomi_core::Error::Internal("2FA is already enabled".into()));
    }

    let secret = crate::totp::generate_secret();
    let qr_uri = crate::totp::generate_qr_code(&secret, email)?;

    crate::redis_ops::store_pending_totp(kv, user_id, &secret).await?;

    Ok(TotpSetupResult { secret, qr_uri })
}

/// Confirm TOTP 2FA setup by verifying a code against the pending secret.
///
/// Atomically retrieves the pending secret from the KV store. On code
/// failure, re-stores the secret so the user can retry. On success,
/// persists the TOTP auth method in the database.
///
/// Returns `Err` if no pending secret exists, the code is wrong, or a
/// database error occurs.
pub async fn enable_totp(
    pool: &DbPool,
    kv: &KVPool,
    user_id: &str,
    code: &str,
) -> kyomi_core::Result<()> {
    let secret = crate::redis_ops::get_pending_totp(kv, user_id).await?;

    let Some(secret) = secret else {
        return Err(kyomi_core::Error::Internal(
            "No pending 2FA setup found. Please start the setup process again.".into(),
        ));
    };

    if !crate::totp::verify_code(&secret, code) {
        // Re-store so the user can retry without restarting the whole flow.
        crate::redis_ops::store_pending_totp(kv, user_id, &secret).await?;
        return Err(kyomi_core::Error::Internal(
            "Invalid verification code. Please try again.".into(),
        ));
    }

    let now = chrono::Utc::now().to_rfc3339();
    let auth_data = serde_json::json!({
        "secret": secret,
        "enabled_at": now,
    });
    crate::user_service::upsert_auth_method(pool, user_id, "totp", &auth_data).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Session management
// ---------------------------------------------------------------------------

/// Get all active sessions for a user, marking the current one.
///
/// `raw_refresh_token` is the raw value of the refresh token cookie (if
/// present). When provided, the session whose token hash matches is flagged
/// as `is_current`.
///
/// Returns `(sessions, current_family_id)` where `current_family_id` is
/// `None` when no matching token exists in the database.
pub async fn get_sessions(
    pool: &DbPool,
    user_id: &str,
    raw_refresh_token: Option<&str>,
) -> kyomi_core::Result<(Vec<crate::token_service::SessionInfo>, Option<String>)> {
    let sessions = crate::token_service::get_user_sessions(pool, user_id).await?;

    let current_family_id = if let Some(raw_token) = raw_refresh_token {
        let hash = crate::token_service::hash_refresh_token(raw_token);

        #[derive(sqlx::FromRow)]
        struct FamilyIdRow {
            family_id: String,
        }

        kyomi_core::db_fetch_optional!(
            pool,
            FamilyIdRow,
            "SELECT family_id FROM refresh_tokens WHERE token_hash = $1 AND is_active = true",
            &hash
        )?
        .map(|r| r.family_id)
    } else {
        None
    };

    Ok((sessions, current_family_id))
}

/// Invalidate the session identified by `raw_refresh_token`.
///
/// Verifies the token and revokes the entire token family so all rotated
/// tokens in the same session are also invalidated. Silently succeeds if
/// the token is already invalid (expired, revoked, or theft-detected).
///
/// Cookie clearing is Leptos-specific and remains in the server function.
pub async fn logout(pool: &DbPool, raw_refresh_token: Option<&str>) -> kyomi_core::Result<()> {
    if let Some(raw_token) = raw_refresh_token {
        match crate::token_service::verify_refresh_token(pool, raw_token).await {
            Ok(
                crate::token_service::RefreshTokenVerifyResult::Valid(data)
                | crate::token_service::RefreshTokenVerifyResult::GracePeriod(data),
            ) => {
                let _ = crate::token_service::revoke_token_family(pool, &data.family_id).await;
            }
            _ => {
                // Token invalid or theft-detected — already revoked, nothing to do.
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Passkey management
// ---------------------------------------------------------------------------

/// Start passkey registration for a user.
///
/// Looks up the user, fetches existing credential IDs to exclude, generates
/// a WebAuthn creation challenge, and stores the registration state in the
/// KV store.
///
/// Returns `(CreationChallengeResponse, challenge_id)` — the server function
/// serializes the response JSON for the browser.
pub async fn start_passkey_registration(
    pool: &DbPool,
    kv: &KVPool,
    webauthn: &Webauthn,
    user_id: &str,
    device_name: &str,
) -> kyomi_core::Result<(CreationChallengeResponse, String)> {
    let db_user = crate::user_service::get_user_by_id(pool, user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("User not found".into()))?;

    let email = &db_user.email;
    let display_name = db_user.name.as_deref().unwrap_or(email);

    // Generate deterministic user handle from email (same as auth_passkeys.rs).
    let user_unique_id = {
        let mut hasher = Sha256::new();
        hasher.update(email.as_bytes());
        let hash = hasher.finalize();
        let bytes: [u8; 16] = hash[..16].try_into().expect("16 bytes");
        Uuid::from_bytes(bytes)
    };

    let creds = crate::user_service::get_passkey_credentials(pool, user_id).await?;

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

    let (ccr, reg_state) = crate::webauthn::start_registration(
        webauthn,
        user_unique_id,
        email,
        display_name,
        exclude_opt,
    )?;

    let challenge_id = crate::redis_ops::generate_token();
    let reg_state_json = serde_json::to_value(&reg_state)
        .map_err(|e| kyomi_core::Error::Internal(format!("Serialize reg state: {e}")))?;

    let challenge_data = serde_json::json!({
        "registration_state": reg_state_json,
        "email": email,
        "user_name": display_name,
        "user_id": user_id,
        "device_name": device_name,
    });
    crate::redis_ops::store_webauthn_challenge(kv, &challenge_id, &challenge_data).await?;

    Ok((ccr, challenge_id))
}

/// Complete passkey registration by verifying the browser credential.
///
/// Retrieves and deletes the challenge from the KV store (preventing replay),
/// verifies the registration state belongs to the authenticated user, runs
/// the WebAuthn finish-registration ceremony, and persists the credential.
///
/// Returns the device name for use in the success message.
pub async fn complete_passkey_registration(
    pool: &DbPool,
    kv: &KVPool,
    webauthn: &Webauthn,
    user_id: &str,
    challenge_id: &str,
    credential: &RegisterPublicKeyCredential,
) -> kyomi_core::Result<String> {
    let challenge_data = crate::redis_ops::get_webauthn_challenge(kv, challenge_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("Invalid or expired challenge".into()))?;

    crate::redis_ops::delete_webauthn_challenge(kv, challenge_id).await?;

    let challenge_user_id = challenge_data["user_id"].as_str().unwrap_or("");
    if challenge_user_id != user_id {
        return Err(kyomi_core::Error::Internal(
            "Challenge does not match authenticated user".into(),
        ));
    }

    let reg_state: PasskeyRegistration =
        serde_json::from_value(challenge_data["registration_state"].clone())
            .map_err(|e| kyomi_core::Error::Internal(format!("Deserialize reg state: {e}")))?;

    let device_name = challenge_data["device_name"]
        .as_str()
        .unwrap_or("Unknown Device")
        .to_string();

    let passkey = crate::webauthn::finish_registration(webauthn, credential, &reg_state)?;

    let cred_id_bytes: &[u8] = passkey.cred_id().as_ref();
    let credential_id_b64 = URL_SAFE_NO_PAD.encode(cred_id_bytes);

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

    crate::user_service::add_passkey_to_user(
        pool,
        user_id,
        &credential_id_b64,
        &public_key_b64,
        initial_counter,
        &device_name,
        &passkey_json,
    )
    .await?;

    Ok(device_name)
}
