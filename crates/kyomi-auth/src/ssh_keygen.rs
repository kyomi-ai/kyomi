// SPDX-License-Identifier: AGPL-3.0-or-later

//! SSH keypair generation for datasource SSH tunnels.
//!
//! Generates an Ed25519 SSH keypair using the `ssh-key` crate (RustCrypto).
//! The private key is emitted in OpenSSH format (unencrypted, no passphrase)
//! so it can be parsed directly by the driver's `russh::keys::decode_secret_key`.
//!
//! The generated private key is returned **in plaintext**. Encryption at rest
//! happens uniformly for every `connection_config` secret at the storage
//! layer — see [`crate::credential_service::finalize_connection_config_secrets`] —
//! not here. Keeping keygen encryption-agnostic means the same AES-256-GCM
//! scheme protects a freshly generated key exactly the same way it protects
//! any other secret the user types into the form.

use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};

/// A freshly generated SSH keypair, ready for use.
///
/// Defined in `kyomi_types` because it also crosses into the WASM client as
/// a server_fn response — see `kyomi_types::datasource_contracts`.
pub use kyomi_types::GeneratedSshKey;

/// Generate a new Ed25519 SSH keypair for an SSH tunnel.
///
/// The private key is generated without a passphrase — encryption at rest is
/// handled by the storage-layer AES-256-GCM scheme (matching every other
/// stored credential), not by OpenSSH's own passphrase protection. The
/// plaintext PEM returned here must be parseable by
/// `russh::keys::decode_secret_key(pem, None)`, which the `ssh-key` crate's
/// OpenSSH Ed25519 output satisfies.
pub fn generate_ssh_keypair() -> kyomi_core::Result<GeneratedSshKey> {
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

    Ok(GeneratedSshKey {
        public_key,
        private_key: private_key_pem.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_ed25519_keypair_with_a_directly_parseable_plaintext_private_key() {
        let generated = generate_ssh_keypair().expect("keygen should succeed");

        assert!(
            generated.public_key.starts_with("ssh-ed25519 "),
            "public key should be an OpenSSH ed25519 line, got: {}",
            generated.public_key
        );

        assert!(
            generated.private_key.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"),
            "private key should be a plaintext OpenSSH PEM block, got: {}",
            generated.private_key
        );

        // The PEM must round-trip through ssh-key's own parser directly —
        // no decryption step — which is what the russh driver relies on to
        // load the tunnel key.
        let parsed = PrivateKey::from_openssh(&generated.private_key)
            .expect("plaintext PEM should parse as a valid OpenSSH private key");
        assert_eq!(parsed.algorithm(), Algorithm::Ed25519);
        assert!(
            !parsed.is_encrypted(),
            "private key must be unencrypted (no passphrase) — encryption at rest \
             is handled by the storage-layer AES-256-GCM scheme, not OpenSSH's"
        );
    }

    #[test]
    fn each_call_generates_a_different_keypair() {
        let a = generate_ssh_keypair().expect("keygen should succeed");
        let b = generate_ssh_keypair().expect("keygen should succeed");
        assert_ne!(a.public_key, b.public_key, "each keypair should be unique");
    }
}
