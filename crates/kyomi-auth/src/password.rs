// SPDX-License-Identifier: AGPL-3.0-or-later

//! Password hashing and verification.
//!
//! Supports both bcrypt (legacy passwords) and argon2id (new passwords).
//! During migration, existing bcrypt hashes are verified but new hashes use
//! argon2id.

use argon2::{
    Argon2,
    PasswordHash as Argon2PasswordHash,
    PasswordHasher,
    PasswordVerifier,
    password_hash::SaltString,
    password_hash::rand_core::OsRng,
};

/// Hash a password using argon2id (recommended for new passwords).
pub fn hash_password(password: &str) -> kyomi_core::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();

    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| kyomi_core::Error::Internal(format!("password hash failed: {e}")))
}

/// Verify a password against a stored hash.
///
/// Automatically detects whether the hash is bcrypt or argon2id.
pub fn verify_password(password: &str, hash: &str) -> kyomi_core::Result<bool> {
    if hash.starts_with("$2b$") || hash.starts_with("$2a$") {
        // Legacy bcrypt hash
        Ok(bcrypt::verify(password, hash)
            .map_err(|e| kyomi_core::Error::Internal(format!("bcrypt verify: {e}")))?)
    } else {
        // argon2id hash
        let parsed = Argon2PasswordHash::new(hash)
            .map_err(|e| kyomi_core::Error::Internal(format!("invalid hash format: {e}")))?;

        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_argon2() {
        let password = "correct-horse-battery-staple";
        let hash = hash_password(password).unwrap();

        assert!(hash.starts_with("$argon2"));
        assert!(verify_password(password, &hash).unwrap());
        assert!(!verify_password("wrong-password", &hash).unwrap());
    }

    #[test]
    fn verify_bcrypt_legacy() {
        let password = "legacy-password";
        // Pre-generate a bcrypt hash (cost=4 for fast tests)
        let hash = bcrypt::hash(password, 4).unwrap();

        assert!(verify_password(password, &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
    }
}
