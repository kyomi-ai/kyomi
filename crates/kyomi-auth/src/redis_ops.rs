// SPDX-License-Identifier: AGPL-3.0-or-later

//! Redis operations for auth flows — OAuth state, pending signups, WebAuthn challenges.
//!
//! All key patterns and TTLs come from `shared/constants.toml`.
//! Operations use SETEX (store), GET (peek), and GETDEL (consume) patterns.

use kyomi_core::{KVPool, kv_consume_json, kv_peek_json, kv_store_json};

// ---------------------------------------------------------------------------
// OAuth state (Google login + Google account linking)
// ---------------------------------------------------------------------------

/// Store OAuth state in Redis. TTL from constants.toml `redis.ttls.oauth_state` (300s).
pub async fn store_oauth_state(
    kv: &KVPool,
    provider: &str,
    state: &str,
    data: &serde_json::Value,
) -> kyomi_core::Result<()> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .oauth_state
        .replace("{provider}", provider)
        .replace("{state}", state);
    kv_store_json(kv, &key, data, constants.redis.ttls.oauth_state).await
}

/// Atomically get and delete OAuth state (GETDEL). Returns None if expired or absent.
pub async fn verify_oauth_state(
    kv: &KVPool,
    provider: &str,
    state: &str,
) -> kyomi_core::Result<Option<serde_json::Value>> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .oauth_state
        .replace("{provider}", provider)
        .replace("{state}", state);
    kv_consume_json(kv, &key).await
}

// ---------------------------------------------------------------------------
// Pending signup (new user from OAuth — needs terms acceptance)
// ---------------------------------------------------------------------------

/// Store pending signup data. TTL from constants.toml `redis.ttls.pending_signup` (3600s).
pub async fn store_pending_signup(
    kv: &KVPool,
    token: &str,
    data: &serde_json::Value,
) -> kyomi_core::Result<()> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .pending_signup
        .replace("{token}", token);
    kv_store_json(kv, &key, data, constants.redis.ttls.pending_signup).await
}

/// Atomically get and delete pending signup data (GETDEL).
pub async fn get_pending_signup(
    kv: &KVPool,
    token: &str,
) -> kyomi_core::Result<Option<serde_json::Value>> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .pending_signup
        .replace("{token}", token);
    kv_consume_json(kv, &key).await
}

// ---------------------------------------------------------------------------
// Pending terms (existing user from OAuth — needs updated terms)
// ---------------------------------------------------------------------------

/// Store pending terms data. TTL same as pending_signup (3600s).
pub async fn store_pending_terms(
    kv: &KVPool,
    token: &str,
    data: &serde_json::Value,
) -> kyomi_core::Result<()> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .pending_terms
        .replace("{token}", token);
    // pending_terms uses same TTL as pending_signup
    kv_store_json(kv, &key, data, constants.redis.ttls.pending_signup).await
}

/// Atomically get and delete pending terms data (GETDEL).
pub async fn get_pending_terms(
    kv: &KVPool,
    token: &str,
) -> kyomi_core::Result<Option<serde_json::Value>> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .pending_terms
        .replace("{token}", token);
    kv_consume_json(kv, &key).await
}

// ---------------------------------------------------------------------------
// WebAuthn challenges (passkey registration + authentication)
// ---------------------------------------------------------------------------

/// Store WebAuthn challenge data. TTL from constants.toml `redis.ttls.webauthn_challenge` (300s).
///
/// Unlike OAuth state, challenges are NOT consumed on read (allows retry on
/// verification failure). They are explicitly deleted after successful verification.
pub async fn store_webauthn_challenge(
    kv: &KVPool,
    challenge_id: &str,
    data: &serde_json::Value,
) -> kyomi_core::Result<()> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .webauthn_challenge
        .replace("{challenge_id}", challenge_id);
    kv_store_json(kv, &key, data, constants.redis.ttls.webauthn_challenge).await
}

/// Get WebAuthn challenge data (non-destructive read).
///
/// Returns `None` if the challenge does not exist or has expired.
/// Unlike OAuth state, challenges are NOT consumed on read — call
/// `delete_webauthn_challenge` after successful verification.
pub async fn get_webauthn_challenge(
    kv: &KVPool,
    challenge_id: &str,
) -> kyomi_core::Result<Option<serde_json::Value>> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .webauthn_challenge
        .replace("{challenge_id}", challenge_id);
    kv_peek_json(kv, &key).await
}

/// Delete WebAuthn challenge after successful verification (replay prevention).
pub async fn delete_webauthn_challenge(kv: &KVPool, challenge_id: &str) -> kyomi_core::Result<()> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .webauthn_challenge
        .replace("{challenge_id}", challenge_id);
    kv.del(&key).await
}

// ---------------------------------------------------------------------------
// Pending TOTP setup (2FA enrollment — secret stored while user confirms)
// ---------------------------------------------------------------------------

/// Store pending TOTP secret during setup. TTL from constants.toml `redis.ttls.totp_setup` (600s).
///
/// The secret is stored temporarily while the user scans the QR code and enters
/// a verification code. Once confirmed, the secret moves to `user_auth_methods`.
pub async fn store_pending_totp(kv: &KVPool, user_id: &str, secret: &str) -> kyomi_core::Result<()> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .totp_setup
        .replace("{user_id}", user_id);
    kv.set(&key, secret, Some(constants.redis.ttls.totp_setup)).await
}

/// Atomically get and delete pending TOTP secret (GETDEL). Returns None if expired or absent.
pub async fn get_pending_totp(kv: &KVPool, user_id: &str) -> kyomi_core::Result<Option<String>> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .totp_setup
        .replace("{user_id}", user_id);
    kv.getdel(&key).await
}

// ---------------------------------------------------------------------------
// Recovery sessions (unified account recovery — password reset)
// ---------------------------------------------------------------------------

/// Store a recovery session in Redis. TTL from constants.toml `redis.ttls.recovery_session` (900s).
///
/// The session maps a random session_id to a user_id, allowing the user to
/// set a new password after verifying their email via a recovery token.
pub async fn store_recovery_session(
    kv: &KVPool,
    session_id: &str,
    user_id: &str,
) -> kyomi_core::Result<()> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .recovery_session
        .replace("{session_id}", session_id);
    kv.set(&key, user_id, Some(constants.redis.ttls.recovery_session)).await
}

/// Read a recovery session without consuming it. Returns the user_id if valid.
///
/// Use this when you need to validate the session but may reject the request
/// (e.g., same-password check). Call `delete_recovery_session` after success.
pub async fn peek_recovery_session(
    kv: &KVPool,
    session_id: &str,
) -> kyomi_core::Result<Option<String>> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .recovery_session
        .replace("{session_id}", session_id);
    kv.get(&key).await
}

/// Delete a recovery session after successful use.
pub async fn delete_recovery_session(kv: &KVPool, session_id: &str) -> kyomi_core::Result<()> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .recovery_session
        .replace("{session_id}", session_id);
    kv.del(&key).await
}

/// Atomically get and delete a recovery session (GETDEL). Returns the user_id if valid.
///
/// Single-use: the session is consumed on read to prevent replay.
pub async fn get_recovery_session(
    kv: &KVPool,
    session_id: &str,
) -> kyomi_core::Result<Option<String>> {
    let constants = kyomi_core::constants::get();
    let key = constants
        .redis
        .key_prefixes
        .recovery_session
        .replace("{session_id}", session_id);
    kv.getdel(&key).await
}

/// Generate a cryptographically random token (URL-safe, 32 bytes).
pub fn generate_token() -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use rand::Rng;

    let random_bytes: [u8; 32] = rand::rng().random();
    URL_SAFE_NO_PAD.encode(random_bytes)
}
