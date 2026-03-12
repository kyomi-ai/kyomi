// SPDX-License-Identifier: AGPL-3.0-or-later

//! AES-256-GCM encryption for credentials at rest.
//!
//! Wire-compatible with the Python backend (`apps/backend-python/src/api/auth/encryption.py`).
//!
//! Binary format: `base64url(version_byte + nonce_12bytes + ciphertext_and_tag)`
//! - Version `0x02` = AES-256-GCM
//! - Nonce: 12 bytes (96-bit), fresh per encryption
//! - Tag: 128-bit, appended to ciphertext by AES-GCM

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, AeadCore, Nonce};
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;

/// Version byte for AES-256-GCM (matches Python's VERSION_AES256_GCM).
const VERSION_AES256_GCM: u8 = 0x02;

/// Nonce size in bytes (96-bit).
const NONCE_SIZE: usize = 12;

/// Encrypt plaintext using AES-256-GCM.
///
/// Returns base64url-encoded `version + nonce + ciphertext_and_tag`.
/// Wire-compatible with the Python backend.
pub fn encrypt(plaintext: &str, key: &[u8; 32]) -> kyomi_core::Result<String> {
    let cipher = Aes256Gcm::new(key.into());
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext_and_tag = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|_| kyomi_core::Error::Internal("encryption failed".into()))?;

    // Format: version (1 byte) + nonce (12 bytes) + ciphertext + tag
    let mut data = Vec::with_capacity(1 + NONCE_SIZE + ciphertext_and_tag.len());
    data.push(VERSION_AES256_GCM);
    data.extend_from_slice(&nonce);
    data.extend_from_slice(&ciphertext_and_tag);

    Ok(URL_SAFE.encode(&data))
}

/// Decrypt a base64url string produced by [`encrypt`] (or the Python backend).
pub fn decrypt(encrypted: &str, key: &[u8; 32]) -> kyomi_core::Result<String> {
    let data = URL_SAFE
        .decode(encrypted)
        .map_err(|e| kyomi_core::Error::BadRequest(format!("bad base64: {e}")))?;

    if data.len() < 1 + NONCE_SIZE + 16 {
        return Err(kyomi_core::Error::BadRequest("encrypted data too short".into()));
    }

    if data[0] != VERSION_AES256_GCM {
        return Err(kyomi_core::Error::BadRequest(format!(
            "unsupported encryption version: 0x{:02x}",
            data[0]
        )));
    }

    let nonce = Nonce::from_slice(&data[1..1 + NONCE_SIZE]);
    let ciphertext_and_tag = &data[1 + NONCE_SIZE..];

    let cipher = Aes256Gcm::new(key.into());

    let plaintext = cipher
        .decrypt(nonce, ciphertext_and_tag)
        .map_err(|_| kyomi_core::Error::Unauthorized("decryption failed".into()))?;

    String::from_utf8(plaintext)
        .map_err(|_| kyomi_core::Error::Internal("decrypted data is not valid text".into()))
}

/// Encrypt a JSON value: serialize to string, then AES-256-GCM encrypt.
pub fn encrypt_json(value: &serde_json::Value, key: &[u8; 32]) -> kyomi_core::Result<String> {
    let json_str = serde_json::to_string(value)
        .map_err(|e| kyomi_core::Error::Internal(format!("JSON serialization failed: {e}")))?;
    encrypt(&json_str, key)
}

/// Decrypt to a JSON value: AES-256-GCM decrypt, then parse JSON.
pub fn decrypt_json(encrypted: &str, key: &[u8; 32]) -> kyomi_core::Result<serde_json::Value> {
    let json_str = decrypt(encrypted, key)?;
    serde_json::from_str(&json_str)
        .map_err(|e| kyomi_core::Error::Internal(format!("JSON deserialization failed: {e}")))
}

/// Derive the 32-byte encryption key from the base64url-encoded env var.
pub fn derive_key(encryption_key_b64: &str) -> kyomi_core::Result<[u8; 32]> {
    let bytes = URL_SAFE
        .decode(encryption_key_b64)
        .map_err(|e| kyomi_core::Error::Internal(format!("bad encryption key base64: {e}")))?;

    if bytes.len() != 32 {
        return Err(kyomi_core::Error::Internal(format!(
            "encryption key must be 32 bytes, got {}",
            bytes.len()
        )));
    }

    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

// ─── Slack token helpers ─────────────────────────────────────────────────────
//
// Thin wrappers around `encrypt` / `decrypt` for Slack OAuth tokens
// (bot token, user token, user refresh token). These follow the same
// AES-256-GCM format used by `credential_service` for datasource credentials.

/// Decrypt a Slack token stored as AES-256-GCM ciphertext in the database.
///
/// Returns the plaintext token string. Tokens are stored encrypted in
/// `workspace_integrations.config` and `workspace_user_integrations.config`
/// JSONB fields.
pub fn decrypt_slack_token(encrypted: &str, encryption_key: &[u8; 32]) -> kyomi_core::Result<String> {
    decrypt(encrypted, encryption_key)
}

/// Encrypt a plaintext Slack token for database storage.
///
/// Returns AES-256-GCM ciphertext as a base64url string. Encrypted tokens
/// are stored in `workspace_integrations.config` and
/// `workspace_user_integrations.config` JSONB fields.
pub fn encrypt_slack_token(plaintext: &str, encryption_key: &[u8; 32]) -> kyomi_core::Result<String> {
    encrypt(plaintext, encryption_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        key[..16].copy_from_slice(b"test-key-1234567");
        key[16..].copy_from_slice(b"8901234567890123");
        key
    }

    #[test]
    fn roundtrip() {
        let key = test_key();
        let plaintext = "my-database-password";
        let encrypted = encrypt(plaintext, &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn roundtrip_empty_string() {
        let key = test_key();
        let encrypted = encrypt("", &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn roundtrip_unicode() {
        let key = test_key();
        let plaintext = "pässwörd-日本語-🔑";
        let encrypted = encrypt(plaintext, &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn each_encryption_produces_different_ciphertext() {
        let key = test_key();
        let plaintext = "same-plaintext";
        let enc1 = encrypt(plaintext, &key).unwrap();
        let enc2 = encrypt(plaintext, &key).unwrap();
        assert_ne!(enc1, enc2, "nonces must differ, producing different ciphertext");

        // But both decrypt to the same value
        assert_eq!(decrypt(&enc1, &key).unwrap(), plaintext);
        assert_eq!(decrypt(&enc2, &key).unwrap(), plaintext);
    }

    #[test]
    fn encrypted_starts_with_version_byte() {
        let key = test_key();
        let encrypted = encrypt("test", &key).unwrap();
        let data = URL_SAFE.decode(&encrypted).unwrap();
        assert_eq!(data[0], VERSION_AES256_GCM);
    }

    #[test]
    fn wrong_key_fails() {
        let key_a = test_key();
        let mut key_b = test_key();
        key_b[0] ^= 0xFF;

        let encrypted = encrypt("secret", &key_a).unwrap();
        let result = decrypt(&encrypted, &key_b);
        assert!(result.is_err());
    }

    #[test]
    fn truncated_ciphertext_rejected() {
        let key = test_key();
        let encrypted = encrypt("test", &key).unwrap();
        let data = URL_SAFE.decode(&encrypted).unwrap();

        // Truncate to just version + partial nonce (too short)
        let truncated = URL_SAFE.encode(&data[..5]);
        let result = decrypt(&truncated, &key);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_base64_rejected() {
        let key = test_key();
        let result = decrypt("not-valid-base64!!!", &key);
        assert!(result.is_err());
    }

    #[test]
    fn wrong_version_byte_rejected() {
        let key = test_key();
        let encrypted = encrypt("test", &key).unwrap();
        let mut data = URL_SAFE.decode(&encrypted).unwrap();

        // Change version byte to unsupported value
        data[0] = 0xFF;
        let tampered = URL_SAFE.encode(&data);

        let result = decrypt(&tampered, &key);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("unsupported encryption version"));
    }

    #[test]
    fn derive_key_correct_length() {
        // A valid 32-byte key encoded as base64url
        let key_bytes = [42u8; 32];
        let encoded = URL_SAFE.encode(key_bytes);
        let derived = derive_key(&encoded).unwrap();
        assert_eq!(derived, key_bytes);
    }

    #[test]
    fn derive_key_wrong_length_rejected() {
        // 16 bytes instead of 32
        let short_key = URL_SAFE.encode([0u8; 16]);
        let result = derive_key(&short_key);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("must be 32 bytes"));
    }

    #[test]
    fn derive_key_invalid_base64_rejected() {
        let result = derive_key("not-valid-base64!!!");
        assert!(result.is_err());
    }

    #[test]
    fn roundtrip_json() {
        let key = test_key();
        let value = serde_json::json!({"model": "claude", "thinking_events": [1, 2, 3]});
        let encrypted = encrypt_json(&value, &key).unwrap();
        let decrypted = decrypt_json(&encrypted, &key).unwrap();
        assert_eq!(decrypted, value);
    }

    #[test]
    fn roundtrip_json_null() {
        let key = test_key();
        let value = serde_json::Value::Null;
        let encrypted = encrypt_json(&value, &key).unwrap();
        let decrypted = decrypt_json(&encrypted, &key).unwrap();
        assert_eq!(decrypted, value);
    }

    #[test]
    fn roundtrip_slack_bot_token() {
        let key = test_key();
        let token = "xoxb-1234567890-abcdefghijklmnop";
        let encrypted = encrypt_slack_token(token, &key).unwrap();
        let decrypted = decrypt_slack_token(&encrypted, &key).unwrap();
        assert_eq!(decrypted, token);
    }

    #[test]
    fn roundtrip_slack_user_token() {
        let key = test_key();
        let token = "xoxp-9876543210-zyxwvutsrqponml";
        let encrypted = encrypt_slack_token(token, &key).unwrap();
        let decrypted = decrypt_slack_token(&encrypted, &key).unwrap();
        assert_eq!(decrypted, token);
    }

    #[test]
    fn slack_token_wrong_key_fails() {
        let key_a = test_key();
        let mut key_b = test_key();
        key_b[0] ^= 0xFF;

        let encrypted = encrypt_slack_token("xoxb-secret", &key_a).unwrap();
        let result = decrypt_slack_token(&encrypted, &key_b);
        assert!(result.is_err());
    }

    #[test]
    fn slack_token_interoperable_with_raw_encrypt() {
        // Verify that encrypt_slack_token produces output that decrypt() can read,
        // and vice versa — they use the same AES-256-GCM format.
        let key = test_key();
        let token = "xoxb-interop-test";

        let from_slack_fn = encrypt_slack_token(token, &key).unwrap();
        assert_eq!(decrypt(&from_slack_fn, &key).unwrap(), token);

        let from_raw_fn = encrypt(token, &key).unwrap();
        assert_eq!(decrypt_slack_token(&from_raw_fn, &key).unwrap(), token);
    }
}
