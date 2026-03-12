// SPDX-License-Identifier: AGPL-3.0-or-later

//! TOTP (Time-based One-Time Password) service for 2FA.
//!
//! Provides generation, verification, and QR code rendering using the `totp-rs` crate.

use totp_rs::{Algorithm, Secret, TOTP};

const ISSUER: &str = "Kyomi";
const DIGITS: usize = 6;
const STEP: u64 = 30;
const SKEW: u8 = 1; // +-1 window for clock drift

/// Generate a random base32-encoded TOTP secret.
pub fn generate_secret() -> String {
    let secret = Secret::generate_secret();
    secret.to_encoded().to_string()
}

/// Generate the otpauth:// provisioning URI for QR code scanning.
pub fn provisioning_uri(secret: &str, email: &str) -> kyomi_core::Result<String> {
    let totp = build_totp(secret, email)?;
    Ok(totp.get_url())
}

/// Generate a QR code as a data URI (`data:image/png;base64,...`).
pub fn generate_qr_code(secret: &str, email: &str) -> kyomi_core::Result<String> {
    let totp = build_totp(secret, email)?;
    let png_bytes = totp
        .get_qr_png()
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to generate QR code: {e}")))?;
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    Ok(format!("data:image/png;base64,{b64}"))
}

/// Verify a 6-digit TOTP code against the secret.
///
/// Allows +-1 time step for clock drift.
pub fn verify_code(secret: &str, code: &str) -> bool {
    // account_name is only used in the provisioning URI, not during verification
    let Ok(totp) = build_totp(secret, "") else {
        return false;
    };
    totp.check_current(code).unwrap_or(false)
}

fn build_totp(secret: &str, account_name: &str) -> kyomi_core::Result<TOTP> {
    let secret_bytes = Secret::Encoded(secret.to_string())
        .to_bytes()
        .map_err(|e| kyomi_core::Error::Internal(format!("Invalid TOTP secret: {e}")))?;

    TOTP::new(
        Algorithm::SHA1,
        DIGITS,
        SKEW,
        STEP,
        secret_bytes,
        Some(ISSUER.to_string()),
        account_name.to_string(),
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("Failed to create TOTP: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_secret_returns_valid_base32() {
        let secret = generate_secret();
        assert!(!secret.is_empty());
        // Should be decodable
        let bytes = Secret::Encoded(secret).to_bytes();
        assert!(bytes.is_ok());
    }

    #[test]
    fn provisioning_uri_contains_issuer() {
        let secret = generate_secret();
        let uri = provisioning_uri(&secret, "test@example.com").unwrap();
        assert!(uri.starts_with("otpauth://totp/"));
        assert!(uri.contains("Kyomi"));
        assert!(uri.contains("test%40example.com") || uri.contains("test@example.com"));
    }

    #[test]
    fn qr_code_returns_data_uri() {
        let secret = generate_secret();
        let qr = generate_qr_code(&secret, "test@example.com").unwrap();
        assert!(qr.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn verify_code_rejects_invalid() {
        let secret = generate_secret();
        assert!(!verify_code(&secret, "000000"));
    }

    #[test]
    fn verify_code_accepts_current() {
        let secret = generate_secret();
        let totp = build_totp(&secret, "test@example.com").unwrap();
        let code = totp.generate_current().unwrap();
        assert!(verify_code(&secret, &code));
    }
}
