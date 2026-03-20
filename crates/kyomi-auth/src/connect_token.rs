// SPDX-License-Identifier: AGPL-3.0-or-later

//! JWT token generation and verification for Kyomi Connect.
//!
//! Uses ES256 (ECDSA with P-256) asymmetric signing — separate from the
//! HMAC-SHA256 tokens in `jwt.rs` used for user authentication.
//!
//! Kyomi holds the private key (signs tokens). The Connect binary verifies
//! tokens using the public key fetched from the `/.well-known/jwks.json`
//! endpoint.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::pkcs8::{DecodePrivateKey, EncodePublicKey, LineEnding};
use rand::Rng;
use serde::{Deserialize, Serialize};

/// Claims embedded in every Kyomi Connect JWT.
///
/// Connect tokens use ES256 and do NOT expire — revocation is handled
/// by replacing the `jti` stored in `datasource_configs.connect_token_jti`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ConnectTokenClaims {
    /// Issuer — base URL for JWKS discovery (`{iss}/.well-known/jwks.json`).
    pub iss: String,
    /// Unique token ID for revocation.
    pub jti: String,
    /// Datasource config UUID.
    pub dsid: String,
    /// Workspace UUID.
    pub wid: String,
    /// Datasource type ("postgres", "mysql", etc.).
    pub db: String,
    /// WebSocket endpoint URL.
    pub url: String,
    /// Issued at (Unix timestamp).
    pub iat: i64,
}

/// Service for generating and verifying Kyomi Connect JWT tokens (ES256).
pub struct ConnectTokenService {
    /// ES256 private key for signing tokens.
    private_key: EncodingKey,
    /// ES256 public key for backend-side verification.
    public_key: DecodingKey,
    /// Key ID included in both JWT headers and the JWKS entry.
    key_id: String,
    /// WebSocket URL embedded in generated tokens.
    connect_url: String,
    /// Base URL used as the JWT `iss` claim (for JWKS discovery).
    base_url: String,
    /// Pre-computed JWKS JSON response (served at `/.well-known/jwks.json`).
    jwks_json: String,
}

impl ConnectTokenService {
    /// Create a new service from a PEM-encoded PKCS#8 private key and Connect
    /// WebSocket URL.
    ///
    /// The PEM must be an EC P-256 key in PKCS#8 format (`BEGIN PRIVATE KEY`).
    /// The public key and JWKS response are derived from the private key at
    /// initialization time.
    pub fn new(private_key_pem: &str, connect_url: &str) -> kyomi_core::Result<Self> {
        // Parse the PEM for jsonwebtoken's EncodingKey
        let encoding_key = EncodingKey::from_ec_pem(private_key_pem.as_bytes())
            .map_err(|e| kyomi_core::Error::Internal(format!("invalid Connect private key: {e}")))?;

        // Parse the PEM with p256 to derive the public key
        let secret_key = p256::SecretKey::from_pkcs8_pem(private_key_pem)
            .map_err(|e| kyomi_core::Error::Internal(format!("failed to parse P-256 key: {e}")))?;

        let public_key_p256 = secret_key.public_key();

        // Derive public key PEM for jsonwebtoken's DecodingKey
        let public_key_pem = public_key_p256
            .to_public_key_pem(LineEnding::LF)
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to encode public key PEM: {e}"))
            })?;

        let decoding_key = DecodingKey::from_ec_pem(public_key_pem.as_bytes())
            .map_err(|e| kyomi_core::Error::Internal(format!("invalid derived public key: {e}")))?;

        // Derive a stable key ID from the first 8 bytes of the x coordinate
        let point = public_key_p256.to_encoded_point(false);
        let x_bytes = point.x().ok_or_else(|| {
            kyomi_core::Error::Internal("failed to extract x coordinate for kid".to_string())
        })?;
        let key_id = URL_SAFE_NO_PAD.encode(&x_bytes[..8]);

        // Build JWKS JSON from the public key coordinates
        let jwks_json = build_jwks_json(&public_key_p256, &key_id)?;

        // Derive base URL from connect_url for the JWT `iss` claim.
        // e.g. "wss://connect.kyomi.ai/v1" → "https://connect.kyomi.ai"
        let base_url = derive_base_url(connect_url)?;

        Ok(Self {
            private_key: encoding_key,
            public_key: decoding_key,
            key_id,
            connect_url: connect_url.to_string(),
            base_url,
            jwks_json,
        })
    }

    /// Generate a new Connect JWT token.
    ///
    /// Returns `(jwt_string, jti)`. The caller must store the `jti` in
    /// `datasource_configs.connect_token_jti` for revocation checks.
    pub fn generate(
        &self,
        datasource_config_id: &str,
        workspace_id: &str,
        datasource_type: &str,
    ) -> kyomi_core::Result<(String, String)> {
        let jti = generate_jti();
        let now = chrono::Utc::now();

        let claims = ConnectTokenClaims {
            iss: self.base_url.clone(),
            jti: jti.clone(),
            dsid: datasource_config_id.to_string(),
            wid: workspace_id.to_string(),
            db: datasource_type.to_string(),
            url: self.connect_url.clone(),
            iat: now.timestamp(),
        };

        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.key_id.clone());
        let token = jsonwebtoken::encode(&header, &claims, &self.private_key)
            .map_err(|e| kyomi_core::Error::Internal(format!("Connect JWT encode: {e}")))?;

        Ok((token, jti))
    }

    /// Verify a Connect JWT token's signature and extract claims.
    ///
    /// This validates the ES256 signature but does NOT check the `jti` against
    /// the stored value — the caller must do that for revocation enforcement.
    pub fn verify(&self, token: &str) -> kyomi_core::Result<ConnectTokenClaims> {
        let mut validation = Validation::new(Algorithm::ES256);
        // Connect tokens don't expire — revocation is via jti replacement
        validation.validate_exp = false;
        validation.required_spec_claims.clear();

        let token_data = jsonwebtoken::decode::<ConnectTokenClaims>(
            token,
            &self.public_key,
            &validation,
        )
        .map_err(|e| {
            let message = match e.kind() {
                jsonwebtoken::errors::ErrorKind::InvalidSignature => {
                    "invalid Connect token signature".to_string()
                }
                jsonwebtoken::errors::ErrorKind::InvalidToken => {
                    format!("malformed Connect token: {e}")
                }
                jsonwebtoken::errors::ErrorKind::Base64(_) => {
                    format!("malformed Connect token: {e}")
                }
                jsonwebtoken::errors::ErrorKind::Json(json_err) => {
                    format!("malformed Connect token payload: {json_err}")
                }
                _ => format!("invalid Connect token: {e}"),
            };
            kyomi_core::Error::Unauthorized(message)
        })?;

        Ok(token_data.claims)
    }

    /// Return pre-computed JWKS JSON for the `/.well-known/jwks.json` endpoint.
    pub fn jwks(&self) -> &str {
        &self.jwks_json
    }
}

/// Generate a unique token ID matching the existing `base64url(16 random bytes)` pattern.
fn generate_jti() -> String {
    let random_bytes: [u8; 16] = rand::rng().random();
    URL_SAFE_NO_PAD.encode(random_bytes)
}

/// Derive the base URL from a Connect WebSocket URL.
///
/// Strips the path and converts the scheme: `wss` → `https`, `ws` → `http`.
/// Returns an error if the URL cannot be parsed or has no host.
///
/// # Examples
/// - `wss://api.kyomi.ai/connect/v1` → `https://api.kyomi.ai`
/// - `ws://localhost:8002/connect/v1` → `http://localhost:8002`
fn derive_base_url(connect_url: &str) -> kyomi_core::Result<String> {
    let parsed = url::Url::parse(connect_url).map_err(|e| {
        kyomi_core::Error::Internal(format!("invalid connect_url '{connect_url}': {e}"))
    })?;

    let scheme = match parsed.scheme() {
        "wss" => "https",
        "ws" => "http",
        other => other,
    };

    let host = parsed.host_str().ok_or_else(|| {
        kyomi_core::Error::Internal(format!("connect_url '{connect_url}' has no host"))
    })?;

    match parsed.port() {
        Some(port) => Ok(format!("{scheme}://{host}:{port}")),
        None => Ok(format!("{scheme}://{host}")),
    }
}

/// Build a JWKS JSON string from a P-256 public key.
///
/// Extracts the uncompressed x/y coordinates and formats them as a JWK
/// with `kty: "EC"`, `crv: "P-256"`, `use: "sig"`, `alg: "ES256"`, `kid`.
fn build_jwks_json(public_key: &p256::PublicKey, kid: &str) -> kyomi_core::Result<String> {
    let point = public_key.to_encoded_point(false); // uncompressed (65 bytes: 0x04 || x || y)

    let x_bytes = point.x().ok_or_else(|| {
        kyomi_core::Error::Internal("failed to extract x coordinate from public key".to_string())
    })?;
    let y_bytes = point.y().ok_or_else(|| {
        kyomi_core::Error::Internal("failed to extract y coordinate from public key".to_string())
    })?;

    let x_b64 = URL_SAFE_NO_PAD.encode(x_bytes);
    let y_b64 = URL_SAFE_NO_PAD.encode(y_bytes);

    let jwks = serde_json::json!({
        "keys": [{
            "kid": kid,
            "kty": "EC",
            "crv": "P-256",
            "use": "sig",
            "alg": "ES256",
            "x": x_b64,
            "y": y_b64,
        }]
    });

    serde_json::to_string(&jwks)
        .map_err(|e| kyomi_core::Error::Internal(format!("failed to serialize JWKS: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a fresh P-256 key pair in PEM format for testing.
    ///
    /// Uses `p256`'s re-exported `OsRng` (rand_core 0.6) because the `p256`
    /// crate depends on rand_core 0.6 while this project uses rand 0.9.
    fn generate_test_key_pem() -> String {
        use p256::elliptic_curve::rand_core::OsRng;
        use p256::pkcs8::EncodePrivateKey;

        let secret_key = p256::SecretKey::random(&mut OsRng);
        let pem = secret_key
            .to_pkcs8_pem(LineEnding::LF)
            .expect("failed to encode test key as PEM");
        pem.to_string()
    }

    fn create_test_service() -> ConnectTokenService {
        let pem = generate_test_key_pem();
        ConnectTokenService::new(&pem, "wss://connect.kyomi.ai/v1").unwrap()
    }

    #[test]
    fn test_generate_token() {
        let service = create_test_service();
        let dsid = "550e8400-e29b-41d4-a716-446655440000";
        let wid = "660e8400-e29b-41d4-a716-446655440001";
        let db = "postgres";

        let (token, jti) = service.generate(dsid, wid, db).unwrap();

        // Token should be a non-empty JWT (3 dot-separated parts)
        assert!(!token.is_empty(), "token must not be empty");
        assert_eq!(token.matches('.').count(), 2, "JWT must have 3 parts");

        // Header should contain kid
        let header_bytes = URL_SAFE_NO_PAD
            .decode(token.split('.').next().unwrap())
            .expect("header must be valid base64url");
        let header: serde_json::Value =
            serde_json::from_slice(&header_bytes).expect("header must be valid JSON");
        assert!(header["kid"].is_string(), "JWT header must contain kid");

        // Decode the payload (middle part) and verify all fields
        let parts: Vec<&str> = token.split('.').collect();
        let payload_bytes = URL_SAFE_NO_PAD
            .decode(parts[1])
            .expect("payload must be valid base64url");
        let claims: ConnectTokenClaims =
            serde_json::from_slice(&payload_bytes).expect("payload must be valid JSON");

        assert_eq!(
            claims.iss, "https://connect.kyomi.ai",
            "iss must be the base URL derived from the connect URL"
        );
        assert_eq!(claims.jti, jti, "jti must match returned value");
        assert_eq!(claims.dsid, dsid, "dsid must match input");
        assert_eq!(claims.wid, wid, "wid must match input");
        assert_eq!(claims.db, db, "db must match input");
        assert_eq!(
            claims.url, "wss://connect.kyomi.ai/v1",
            "url must match service URL"
        );
        assert!(claims.iat > 0, "iat must be a positive timestamp");

        // iat should be recent (within last 10 seconds)
        let now = chrono::Utc::now().timestamp();
        assert!(
            (now - claims.iat).abs() < 10,
            "iat should be close to current time"
        );
    }

    #[test]
    fn test_verify_valid_token() {
        let service = create_test_service();

        let (token, jti) = service
            .generate("ds-123", "ws-456", "mysql")
            .unwrap();

        let claims = service.verify(&token).unwrap();

        assert_eq!(claims.iss, "https://connect.kyomi.ai");
        assert_eq!(claims.jti, jti);
        assert_eq!(claims.dsid, "ds-123");
        assert_eq!(claims.wid, "ws-456");
        assert_eq!(claims.db, "mysql");
        assert_eq!(claims.url, "wss://connect.kyomi.ai/v1");
    }

    #[test]
    fn test_verify_tampered_token() {
        let service = create_test_service();

        let (token, _jti) = service
            .generate("ds-123", "ws-456", "postgres")
            .unwrap();

        // Tamper with the payload: decode, modify, re-encode
        let parts: Vec<&str> = token.split('.').collect();
        let payload_bytes = URL_SAFE_NO_PAD
            .decode(parts[1])
            .expect("payload must be valid base64url");
        let mut claims: serde_json::Value =
            serde_json::from_slice(&payload_bytes).expect("payload must be valid JSON");

        // Change the URL to an attacker's endpoint
        claims["url"] = serde_json::json!("wss://evil.example.com/ws");

        let tampered_payload = serde_json::to_vec(&claims).unwrap();
        let tampered_payload_b64 = URL_SAFE_NO_PAD.encode(&tampered_payload);

        // Reconstruct token with original header and signature but tampered payload
        let tampered_token = format!("{}.{}.{}", parts[0], tampered_payload_b64, parts[2]);

        let result = service.verify(&tampered_token);
        assert!(
            result.is_err(),
            "tampered token must be rejected by signature verification"
        );

        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("invalid Connect token signature"),
            "expected signature error, got: {err_msg}"
        );
    }

    #[test]
    fn test_verify_wrong_jti() {
        // Two tokens for the same datasource should get different jtis,
        // demonstrating that jti-based revocation works (old jti won't match).
        let service = create_test_service();

        let (token1, jti1) = service
            .generate("ds-123", "ws-456", "postgres")
            .unwrap();
        let (token2, jti2) = service
            .generate("ds-123", "ws-456", "postgres")
            .unwrap();

        // Both tokens are cryptographically valid
        let claims1 = service.verify(&token1).unwrap();
        let claims2 = service.verify(&token2).unwrap();

        // But they have different jtis
        assert_ne!(
            claims1.jti, claims2.jti,
            "tokens for same datasource must have different jtis"
        );
        assert_ne!(jti1, jti2, "returned jtis must differ");

        // If the stored jti is jti2, then token1's jti won't match
        // (this is the business logic check the caller would perform)
        let stored_jti = &jti2;
        assert_ne!(
            &claims1.jti, stored_jti,
            "old token's jti must not match the current stored jti"
        );
        assert_eq!(
            &claims2.jti, stored_jti,
            "new token's jti must match the current stored jti"
        );
    }

    #[test]
    fn test_rotate_token() {
        let service = create_test_service();
        let dsid = "ds-rotate-test";
        let wid = "ws-rotate-test";
        let db = "clickhouse";

        // Generate first token
        let (_token1, jti1) = service.generate(dsid, wid, db).unwrap();

        // "Rotate" by generating a new token for the same datasource
        let (_token2, jti2) = service.generate(dsid, wid, db).unwrap();

        assert_ne!(
            jti1, jti2,
            "rotated token must have a different jti than the original"
        );

        // Both jtis should be valid base64url-encoded strings
        assert!(
            URL_SAFE_NO_PAD.decode(&jti1).is_ok(),
            "jti1 must be valid base64url"
        );
        assert!(
            URL_SAFE_NO_PAD.decode(&jti2).is_ok(),
            "jti2 must be valid base64url"
        );

        // Each jti should be 16 random bytes = 22 base64url characters
        assert_eq!(jti1.len(), 22, "jti should be 22 base64url chars (16 bytes)");
        assert_eq!(jti2.len(), 22, "jti should be 22 base64url chars (16 bytes)");
    }

    #[test]
    fn test_jwks_response() {
        let service = create_test_service();

        let jwks_str = service.jwks();

        // Must be valid JSON
        let jwks: serde_json::Value =
            serde_json::from_str(jwks_str).expect("JWKS must be valid JSON");

        // Must have a "keys" array
        let keys = jwks["keys"]
            .as_array()
            .expect("JWKS must have a 'keys' array");
        assert_eq!(keys.len(), 1, "JWKS must contain exactly one key");

        let key = &keys[0];

        // Verify all required JWK fields
        assert_eq!(key["kty"].as_str().unwrap(), "EC", "kty must be 'EC'");
        assert_eq!(key["crv"].as_str().unwrap(), "P-256", "crv must be 'P-256'");
        assert_eq!(key["use"].as_str().unwrap(), "sig", "use must be 'sig'");
        assert_eq!(
            key["alg"].as_str().unwrap(),
            "ES256",
            "alg must be 'ES256'"
        );
        let kid = key["kid"].as_str().expect("kid must be present");
        assert!(!kid.is_empty(), "kid must not be empty");

        // x and y must be present and non-empty base64url strings
        let x = key["x"].as_str().expect("x must be a string");
        let y = key["y"].as_str().expect("y must be a string");

        assert!(!x.is_empty(), "x must not be empty");
        assert!(!y.is_empty(), "y must not be empty");

        // x and y must be valid base64url (32 bytes each = 43 base64url chars)
        let x_bytes = URL_SAFE_NO_PAD
            .decode(x)
            .expect("x must be valid base64url");
        let y_bytes = URL_SAFE_NO_PAD
            .decode(y)
            .expect("y must be valid base64url");

        assert_eq!(x_bytes.len(), 32, "x coordinate must be 32 bytes");
        assert_eq!(y_bytes.len(), 32, "y coordinate must be 32 bytes");
    }
}
