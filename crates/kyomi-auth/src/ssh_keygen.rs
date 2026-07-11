// SPDX-License-Identifier: AGPL-3.0-or-later

//! SSH keypair generation for datasource SSH tunnels.
//!
//! Generates an Ed25519 SSH keypair using the `ssh-key` crate (RustCrypto).
//! The private key is emitted in OpenSSH format (unencrypted, no passphrase)
//! so it can be parsed directly by the driver's `russh::keys::decode_secret_key`,
//! then encrypted at rest with the workspace encryption key before storage —
//! the same AES-256-GCM scheme used for all other datasource credentials
//! (see [`crate::encryption`]).

use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};

use crate::encryption;

/// A freshly generated SSH keypair, ready for storage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GeneratedSshKey {
    /// OpenSSH public key line (`ssh-ed25519 AAAA... `), plaintext.
    ///
    /// Shown to the user so they can add it to their server's
    /// `~/.ssh/authorized_keys`.
    pub public_key: String,

    /// OpenSSH private key PEM, AES-256-GCM encrypted with the workspace
    /// encryption key. Never returned in plaintext.
    pub private_key: String,
}

/// Generate a new Ed25519 SSH keypair for an SSH tunnel and encrypt the
/// private key with `encryption_key`.
///
/// The private key is generated without a passphrase — encryption at rest is
/// handled by our own AES-256-GCM scheme (matching every other stored
/// credential), not by OpenSSH's own passphrase protection. The decrypted PEM
/// must be parseable by `russh::keys::decode_secret_key(pem, None)`, which the
/// `ssh-key` crate's OpenSSH Ed25519 output satisfies.
pub fn generate_ssh_keypair(encryption_key: &[u8; 32]) -> kyomi_core::Result<GeneratedSshKey> {
    let mut rng = OsRng;
    let private_key = PrivateKey::random(&mut rng, Algorithm::Ed25519).map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to generate SSH keypair: {e}"))
    })?;

    let public_key = private_key.public_key().to_openssh().map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to encode SSH public key: {e}"))
    })?;

    let private_key_pem = private_key.to_openssh(LineEnding::LF).map_err(|e| {
        kyomi_core::Error::Internal(format!("failed to encode SSH private key: {e}"))
    })?;

    let encrypted_private_key = encryption::encrypt(&private_key_pem, encryption_key)?;

    Ok(GeneratedSshKey {
        public_key,
        private_key: encrypted_private_key,
    })
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
    fn generates_ed25519_keypair_with_recoverable_private_key() {
        let key = test_key();
        let generated = generate_ssh_keypair(&key).expect("keygen should succeed");

        assert!(
            generated.public_key.starts_with("ssh-ed25519 "),
            "public key should be an OpenSSH ed25519 line, got: {}",
            generated.public_key
        );

        let decrypted_pem = encryption::decrypt(&generated.private_key, &key)
            .expect("decrypting the stored private key should succeed");

        assert!(
            decrypted_pem.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"),
            "decrypted private key should be an OpenSSH PEM block, got: {}",
            decrypted_pem
        );

        // The PEM must round-trip through ssh-key's own parser — this is
        // what the russh driver relies on to load the tunnel key.
        let parsed = PrivateKey::from_openssh(&decrypted_pem)
            .expect("decrypted PEM should parse as a valid OpenSSH private key");
        assert_eq!(parsed.algorithm(), Algorithm::Ed25519);
        assert!(
            !parsed.is_encrypted(),
            "private key must be unencrypted (no passphrase) — encryption at rest \
             is handled by our own AES-256-GCM layer, not OpenSSH's"
        );
    }

    #[test]
    fn each_call_generates_a_different_keypair() {
        let key = test_key();
        let a = generate_ssh_keypair(&key).expect("keygen should succeed");
        let b = generate_ssh_keypair(&key).expect("keygen should succeed");
        assert_ne!(a.public_key, b.public_key, "each keypair should be unique");
    }
}
