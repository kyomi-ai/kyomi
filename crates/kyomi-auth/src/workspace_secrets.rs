// SPDX-License-Identifier: AGPL-3.0-or-later

//! AES-256-GCM encryption for workspace-level secrets (BYOK API keys).
//!
//! This module is separate from [`crate::encryption`] because:
//!
//! * It uses a **distinct master key** (`WORKSPACE_SECRETS_KEY`) so that leaking
//!   one key does not compromise the other (principle of key separation).
//! * Its wire format is minimal: `base64(nonce[12B] || ciphertext || tag[16B])`
//!   — no version byte, no Python interop. AES-GCM authentication is provided
//!   by the appended 16-byte tag (which `aes-gcm` writes inside
//!   `ciphertext_and_tag`).
//!
//! # Deployment
//!
//! * **SaaS mode** (`config.self_hosted == false`): `WORKSPACE_SECRETS_KEY` is
//!   **required**. The server refuses to start without it — see the startup
//!   check in `apps/server/src/main.rs`.
//! * **Self-hosted mode**: the env var is optional. If absent,
//!   [`is_available`] returns `false` and the BYOK system is disabled. Call
//!   sites must check [`is_available`] before offering BYOK UI or accepting
//!   BYOK configuration writes.
//!
//! # Key generation
//!
//! ```text
//! openssl rand -base64 32
//! ```

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

/// Env var holding the base64-encoded 32-byte master key.
const MASTER_KEY_ENV: &str = "WORKSPACE_SECRETS_KEY";

/// AES-GCM nonce size in bytes (96-bit).
const NONCE_SIZE: usize = 12;

/// AES-GCM authentication tag size in bytes (128-bit). The `aes-gcm` crate
/// appends this tag to `ciphertext_and_tag`, so the minimum valid ciphertext
/// length is `NONCE_SIZE + TAG_SIZE`.
const TAG_SIZE: usize = 16;

/// Errors returned by the workspace secrets module.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceSecretError {
    /// `WORKSPACE_SECRETS_KEY` is not set in the environment.
    #[error("WORKSPACE_SECRETS_KEY is not configured — BYOK is disabled")]
    NotConfigured,

    /// The master key env var is set but does not decode to 32 bytes of base64.
    #[error("WORKSPACE_SECRETS_KEY is malformed: {0}")]
    MalformedKey(String),

    /// The stored ciphertext is malformed (bad base64, too short, truncated nonce).
    #[error("workspace secret ciphertext is malformed: {0}")]
    MalformedCiphertext(String),

    /// AES-GCM encryption failed. Effectively unreachable for valid inputs.
    #[error("workspace secret encryption failed")]
    EncryptionFailed,

    /// AES-GCM authentication failed — either the master key is wrong or the
    /// ciphertext has been tampered with.
    #[error("workspace secret decryption failed — key mismatch or tampering")]
    DecryptionFailed,
}

/// Returns true if `WORKSPACE_SECRETS_KEY` is set and decodes to a valid 32-byte key.
///
/// Call sites should use this to feature-gate BYOK functionality. In SaaS
/// mode, the server's startup check guarantees this returns true (or the
/// server never starts). In self-hosted mode, this may return false.
pub fn is_available() -> bool {
    load_master_key().is_ok()
}

/// Encrypt a plaintext secret for storage.
///
/// Returns `base64(nonce[12B] || ciphertext_and_tag)` where `ciphertext_and_tag`
/// ends with the 16-byte GCM authentication tag appended by `aes-gcm`.
///
/// Each call uses a fresh random nonce, so repeated encryption of the same
/// plaintext yields different ciphertexts — a property relied on by tests.
pub fn encrypt_secret(plaintext: &str) -> Result<String, WorkspaceSecretError> {
    let key = load_master_key()?;
    let cipher = Aes256Gcm::new((&key).into());
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext_and_tag = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|_| WorkspaceSecretError::EncryptionFailed)?;

    let mut buf = Vec::with_capacity(NONCE_SIZE + ciphertext_and_tag.len());
    buf.extend_from_slice(nonce.as_slice());
    buf.extend_from_slice(&ciphertext_and_tag);

    Ok(BASE64_STANDARD.encode(&buf))
}

/// Decrypt a secret previously produced by [`encrypt_secret`].
///
/// Returns [`WorkspaceSecretError::DecryptionFailed`] if the master key is
/// wrong or the ciphertext has been tampered with (AEAD integrity failure).
pub fn decrypt_secret(encoded: &str) -> Result<String, WorkspaceSecretError> {
    let key = load_master_key()?;

    let data = BASE64_STANDARD
        .decode(encoded)
        .map_err(|e| WorkspaceSecretError::MalformedCiphertext(format!("bad base64: {e}")))?;

    if data.len() < NONCE_SIZE + TAG_SIZE {
        return Err(WorkspaceSecretError::MalformedCiphertext(format!(
            "payload too short: {} bytes (need >= {})",
            data.len(),
            NONCE_SIZE + TAG_SIZE
        )));
    }

    let (nonce_bytes, ciphertext_and_tag) = data.split_at(NONCE_SIZE);
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new((&key).into());

    let plaintext = cipher
        .decrypt(nonce, ciphertext_and_tag)
        .map_err(|_| WorkspaceSecretError::DecryptionFailed)?;

    String::from_utf8(plaintext).map_err(|_| {
        WorkspaceSecretError::MalformedCiphertext("plaintext is not valid UTF-8".into())
    })
}

/// Load and validate the 32-byte master key from `WORKSPACE_SECRETS_KEY`.
///
/// Kept private — callers should not handle the raw key. All encryption /
/// decryption happens inside this module.
fn load_master_key() -> Result<[u8; 32], WorkspaceSecretError> {
    let encoded =
        std::env::var(MASTER_KEY_ENV).map_err(|_| WorkspaceSecretError::NotConfigured)?;

    if encoded.is_empty() {
        return Err(WorkspaceSecretError::NotConfigured);
    }

    let bytes = BASE64_STANDARD
        .decode(encoded.trim())
        .map_err(|e| WorkspaceSecretError::MalformedKey(format!("bad base64: {e}")))?;

    if bytes.len() != 32 {
        return Err(WorkspaceSecretError::MalformedKey(format!(
            "expected 32 bytes, got {}",
            bytes.len()
        )));
    }

    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

#[cfg(test)]
mod tests {
    //! # Env var discipline
    //!
    //! These tests mutate the process-global env var `WORKSPACE_SECRETS_KEY`.
    //! `cargo test` runs tests in parallel across a single binary, so we
    //! serialize all tests in this module through `ENV_LOCK`. Each test
    //! acquires the mutex for its whole duration and restores the env var
    //! before returning (via `EnvGuard::drop`).
    //!
    //! Tests in this module must NOT call each other's helpers across the
    //! mutex boundary, and must NOT spawn tasks that touch the env var.

    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// RAII guard that sets `WORKSPACE_SECRETS_KEY` for the duration of a
    /// test and clears it on drop. Holds the global mutex so tests are
    /// serialized.
    struct EnvGuard {
        _lock: MutexGuard<'static, ()>,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn with_key(key_b64: &str) -> Self {
            // Poisoned mutex is fine — we just want mutual exclusion.
            let lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var(MASTER_KEY_ENV).ok();
            // SAFETY: tests are single-threaded w.r.t. this env var because
            // `lock` is held for the entire test body.
            unsafe { std::env::set_var(MASTER_KEY_ENV, key_b64) };
            Self { _lock: lock, prev }
        }

        fn without_key() -> Self {
            let lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var(MASTER_KEY_ENV).ok();
            // SAFETY: tests are single-threaded w.r.t. this env var because
            // `lock` is held for the entire test body.
            unsafe { std::env::remove_var(MASTER_KEY_ENV) };
            Self { _lock: lock, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: tests are single-threaded w.r.t. this env var because
            // `_lock` is held for the entire test body and released only after
            // this drop completes.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var(MASTER_KEY_ENV, v),
                    None => std::env::remove_var(MASTER_KEY_ENV),
                }
            }
        }
    }

    fn key_a() -> String {
        BASE64_STANDARD.encode([0xA5u8; 32])
    }

    fn key_b() -> String {
        BASE64_STANDARD.encode([0x5Au8; 32])
    }

    #[test]
    fn roundtrip() {
        let _g = EnvGuard::with_key(&key_a());
        let plaintext = "sk-ant-api03-super-secret-key-value";
        let encrypted = encrypt_secret(plaintext).unwrap();
        let decrypted = decrypt_secret(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn roundtrip_empty_string() {
        let _g = EnvGuard::with_key(&key_a());
        let encrypted = encrypt_secret("").unwrap();
        let decrypted = decrypt_secret(&encrypted).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn fresh_nonce_per_encryption() {
        let _g = EnvGuard::with_key(&key_a());
        let enc1 = encrypt_secret("same").unwrap();
        let enc2 = encrypt_secret("same").unwrap();
        assert_ne!(enc1, enc2, "nonces must differ");
        assert_eq!(decrypt_secret(&enc1).unwrap(), "same");
        assert_eq!(decrypt_secret(&enc2).unwrap(), "same");
    }

    #[test]
    fn wrong_master_key_fails() {
        // Encrypt with key A, then swap the env to key B and attempt to decrypt.
        let encrypted = {
            let _g = EnvGuard::with_key(&key_a());
            encrypt_secret("top-secret").unwrap()
        };

        let _g = EnvGuard::with_key(&key_b());
        let err = decrypt_secret(&encrypted).unwrap_err();
        assert!(
            matches!(err, WorkspaceSecretError::DecryptionFailed),
            "expected DecryptionFailed, got {err:?}"
        );
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let _g = EnvGuard::with_key(&key_a());
        let encrypted = encrypt_secret("tamper-me").unwrap();

        // Flip a byte in the middle of the payload (past the nonce, inside
        // the ciphertext-or-tag region).
        let mut bytes = BASE64_STANDARD.decode(&encrypted).unwrap();
        let mid = bytes.len() / 2;
        assert!(mid > NONCE_SIZE, "payload too small to tamper safely");
        bytes[mid] ^= 0xFF;
        let tampered = BASE64_STANDARD.encode(&bytes);

        let err = decrypt_secret(&tampered).unwrap_err();
        assert!(
            matches!(err, WorkspaceSecretError::DecryptionFailed),
            "expected DecryptionFailed, got {err:?}"
        );
    }

    #[test]
    fn is_available_false_when_env_missing() {
        let _g = EnvGuard::without_key();
        assert!(!is_available());
        let err = encrypt_secret("x").unwrap_err();
        assert!(matches!(err, WorkspaceSecretError::NotConfigured));
    }

    #[test]
    fn is_available_true_when_env_set() {
        let _g = EnvGuard::with_key(&key_a());
        assert!(is_available());
    }

    #[test]
    fn malformed_master_key_fails() {
        // 16 bytes, not 32.
        let short = BASE64_STANDARD.encode([0u8; 16]);
        let _g = EnvGuard::with_key(&short);
        assert!(!is_available());
        let err = encrypt_secret("x").unwrap_err();
        assert!(matches!(err, WorkspaceSecretError::MalformedKey(_)));
    }

    #[test]
    fn malformed_ciphertext_rejected() {
        let _g = EnvGuard::with_key(&key_a());
        // Valid base64 but shorter than nonce+tag.
        let too_short = BASE64_STANDARD.encode([0u8; 10]);
        let err = decrypt_secret(&too_short).unwrap_err();
        assert!(matches!(err, WorkspaceSecretError::MalformedCiphertext(_)));
    }
}
