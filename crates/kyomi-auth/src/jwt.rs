// SPDX-License-Identifier: AGPL-3.0-or-later

//! JWT token creation and validation.
//!
//! Wire-compatible with the Python backend — same secret, same claims shape.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, TokenData, Validation, errors::ErrorKind};
use rand::Rng;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Claims embedded in every JWT access token.
///
/// Must match the Python backend's JWT payload exactly for cross-service
/// compatibility during the migration.
///
/// Standard claims: `sub`, `exp`, `iat`, `jti`.
///
/// Python also puts `user_id`, `email`, `name`, `roles`, and workspace
/// context into the JWT. The `extra` field captures these so Rust can
/// decode Python-created tokens without losing data.
///
/// Note: Python's refresh tokens are opaque strings (`rt_<base64url>`), NOT JWTs.
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject — the user's UUID.
    pub sub: String,
    /// Expiry (Unix timestamp).
    pub exp: i64,
    /// Issued-at (Unix timestamp).
    pub iat: i64,
    /// JWT ID — unique per token, used for revocation tracking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
    /// Extra fields from the Python backend (user_id, email, name, roles, etc.).
    ///
    /// Rust-created tokens won't include these, but Python-created tokens will.
    /// Captured here for wire compatibility during the migration period.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

/// Create a JWT access token for the given user (UUID version, for backwards compat).
///
/// Generates a unique `jti` for revocation tracking, matching the Python
/// backend's `secrets.token_urlsafe(16)` pattern.
pub fn create_access_token(
    user_id: Uuid,
    secret: &str,
    expires_minutes: i64,
) -> kyomi_core::Result<String> {
    create_access_token_str(&user_id.to_string(), secret, expires_minutes, Default::default())
}

/// Create a JWT access token with extra claims (matches Python's token creation).
///
/// The `extra` map should contain: `user_id`, `email`, `name`, `roles`,
/// and optionally `workspace_id`, `workspace_roles`, etc.
pub fn create_access_token_str(
    sub: &str,
    secret: &str,
    expires_minutes: i64,
    extra: std::collections::HashMap<String, serde_json::Value>,
) -> kyomi_core::Result<String> {
    let now = Utc::now();
    let jti = generate_jti();
    let claims = Claims {
        sub: sub.to_string(),
        iat: now.timestamp(),
        exp: (now + Duration::minutes(expires_minutes)).timestamp(),
        jti: Some(jti),
        extra,
    };

    jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("jwt encode: {e}")))
}

/// Create an opaque refresh token.
///
/// Refresh tokens are NOT JWTs — they are opaque strings with a prefix
/// for identification, matching Python's `f"rt_{secrets.token_urlsafe(32)}"`.
pub fn create_refresh_token() -> String {
    let prefix = &kyomi_core::constants::get().jwt.refresh_token_prefix;
    let random_bytes: [u8; 32] = rand::rng().random();
    let encoded = URL_SAFE_NO_PAD.encode(random_bytes);
    format!("{prefix}{encoded}")
}

/// Generate a JWT ID (jti) matching Python's `secrets.token_urlsafe(16)`.
fn generate_jti() -> String {
    let random_bytes: [u8; 16] = rand::rng().random();
    URL_SAFE_NO_PAD.encode(random_bytes)
}

/// Validate a token and return its claims.
///
/// Returns specific error messages for different failure modes:
/// - Expired tokens → "token expired"
/// - Malformed tokens → "malformed token: ..."
/// - Wrong signature → "invalid token signature"
/// - Missing required claims → "token missing required claims: ..."
pub fn validate_token(token: &str, secret: &str) -> kyomi_core::Result<TokenData<Claims>> {
    let mut validation = Validation::default();
    validation.validate_exp = true;

    jsonwebtoken::decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|e| {
        let message = match e.kind() {
            ErrorKind::ExpiredSignature => "token expired".to_string(),
            ErrorKind::InvalidSignature => "invalid token signature".to_string(),
            ErrorKind::InvalidToken => format!("malformed token: {e}"),
            ErrorKind::Base64(_) => format!("malformed token: {e}"),
            ErrorKind::Json(json_err) => format!("malformed token payload: {json_err}"),
            ErrorKind::MissingRequiredClaim(claim) => {
                format!("token missing required claim: {claim}")
            }
            _ => format!("invalid token: {e}"),
        };
        kyomi_core::Error::Unauthorized(message)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_access_token() {
        let secret = "test-secret-key";
        let user_id = Uuid::new_v4();

        let token = create_access_token(user_id, secret, 15).unwrap();
        let decoded = validate_token(&token, secret).unwrap();

        assert_eq!(decoded.claims.sub, user_id.to_string());
        assert!(decoded.claims.jti.is_some(), "access tokens must have jti");
        assert!(decoded.claims.extra.is_empty(), "Rust-created tokens have no extra fields");
    }

    #[test]
    fn refresh_token_is_opaque() {
        // Load constants (disk if available, embedded fallback otherwise).
        // Idempotent — repeat calls are a no-op via OnceLock.
        let _ = kyomi_core::constants::load_with_fallback();

        let token = create_refresh_token();
        assert!(token.starts_with("rt_"), "refresh token must start with rt_ prefix");
        assert!(token.len() >= 40, "refresh token must be at least 40 chars");
    }

    #[test]
    fn wrong_secret_rejects() {
        let user_id = Uuid::new_v4();
        let token = create_access_token(user_id, "secret-a", 15).unwrap();
        let result = validate_token(&token, "secret-b");
        assert!(result.is_err());
        // Should specifically identify signature mismatch
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("invalid token signature"),
            "expected signature error, got: {err_msg}"
        );
    }

    #[test]
    fn expired_token_rejected_with_specific_error() {
        let secret = "test-secret-key";
        let user_id = Uuid::new_v4();

        // Create a token that expired 5 minutes ago (well past the default
        // 60-second leeway that jsonwebtoken allows).
        let token = create_access_token(user_id, secret, -5).unwrap();
        let result = validate_token(&token, secret);

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("token expired"),
            "expected 'token expired' error, got: {err_msg}"
        );
    }

    #[test]
    fn malformed_token_rejected() {
        let result = validate_token("not-a-jwt", "secret");
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("malformed token") || err_msg.contains("invalid token"),
            "expected malformed/invalid error, got: {err_msg}"
        );
    }

    #[test]
    fn empty_token_rejected() {
        let result = validate_token("", "secret");
        assert!(result.is_err());
    }

    #[test]
    fn decode_python_shaped_jwt_with_extra_fields() {
        // Simulate a JWT created by the Python backend which includes
        // extra fields: user_id, email, name, roles.
        let secret = "test-secret-key";

        // Build a Python-shaped payload manually
        let now = Utc::now();
        let mut payload = serde_json::Map::new();
        payload.insert("sub".into(), serde_json::json!("550e8400-e29b-41d4-a716-446655440000"));
        payload.insert("exp".into(), serde_json::json!((now + Duration::minutes(15)).timestamp()));
        payload.insert("iat".into(), serde_json::json!(now.timestamp()));
        payload.insert("jti".into(), serde_json::json!("abc123def456"));
        // Extra fields that Python includes
        payload.insert("user_id".into(), serde_json::json!("550e8400-e29b-41d4-a716-446655440000"));
        payload.insert("email".into(), serde_json::json!("user@example.com"));
        payload.insert("name".into(), serde_json::json!("Test User"));
        payload.insert("roles".into(), serde_json::json!(["admin"]));
        payload.insert("workspace_id".into(), serde_json::json!("ws-123"));

        let token = jsonwebtoken::encode(
            &Header::default(),
            &payload,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();

        // Rust should decode this successfully and preserve extra fields
        let decoded = validate_token(&token, secret).unwrap();
        assert_eq!(decoded.claims.sub, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(decoded.claims.jti, Some("abc123def456".to_string()));

        // Extra fields are captured
        assert_eq!(
            decoded.claims.extra.get("email"),
            Some(&serde_json::json!("user@example.com"))
        );
        assert_eq!(
            decoded.claims.extra.get("name"),
            Some(&serde_json::json!("Test User"))
        );
        assert_eq!(
            decoded.claims.extra.get("roles"),
            Some(&serde_json::json!(["admin"]))
        );
        assert_eq!(
            decoded.claims.extra.get("workspace_id"),
            Some(&serde_json::json!("ws-123"))
        );
        assert_eq!(
            decoded.claims.extra.get("user_id"),
            Some(&serde_json::json!("550e8400-e29b-41d4-a716-446655440000"))
        );
    }

    #[test]
    fn jti_is_unique_per_token() {
        let secret = "test-secret-key";
        let user_id = Uuid::new_v4();

        let token1 = create_access_token(user_id, secret, 15).unwrap();
        let token2 = create_access_token(user_id, secret, 15).unwrap();

        let decoded1 = validate_token(&token1, secret).unwrap();
        let decoded2 = validate_token(&token2, secret).unwrap();

        assert_ne!(
            decoded1.claims.jti, decoded2.claims.jti,
            "each token must have a unique jti"
        );
    }

    #[test]
    fn refresh_tokens_are_unique() {
        // Load constants (disk if available, embedded fallback otherwise).
        let _ = kyomi_core::constants::load_with_fallback();

        let token1 = create_refresh_token();
        let token2 = create_refresh_token();
        assert_ne!(token1, token2, "refresh tokens must be unique");
    }
}
