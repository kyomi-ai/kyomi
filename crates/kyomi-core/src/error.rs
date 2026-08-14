// SPDX-License-Identifier: AGPL-3.0-or-later

//! Unified error types for the Kyomi backend.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Application-wide error type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("too many requests: {0}")]
    TooManyRequests(String, u64),

    #[error("not implemented: {0}")]
    NotImplemented(String),

    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),

    /// A datasource connection or validation failure whose message is already
    /// a complete, user-facing sentence (e.g. "Connection test failed — check
    /// datasource credentials and connectivity"). Unlike the other variants,
    /// its `Display` is prefix-free so the message surfaces verbatim in a
    /// client toast instead of reading as `internal: <message>`.
    #[error("{0}")]
    DatasourceConnection(String),

    /// A stored credential field looked like Kyomi's ciphertext format
    /// (passed `credential_service::looks_encrypted`) but failed to decrypt —
    /// a rotated/mismatched `DATASOURCE_ENCRYPTION_KEY`, or corrupted/tampered
    /// data. This is a **server configuration problem**, never something the
    /// request caller did wrong, and it must never be confused with the
    /// external datasource rejecting the (would-be) credential — so its
    /// message is deliberately worded to point at the encryption key rather
    /// than reading like an authentication failure. `Display` is prefix-free,
    /// same rationale as [`Error::DatasourceConnection`].
    #[error("{0}")]
    CredentialDecryptionFailed(String),

    #[error("internal: {0}")]
    Internal(String),

    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),

    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error(transparent)]
    Redis(#[from] redis::RedisError),

    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
}

impl From<kyomi_connect_protocol::Error> for Error {
    fn from(e: kyomi_connect_protocol::Error) -> Self {
        Error::Internal(e.to_string())
    }
}

impl Error {
    /// Returns `true` if this error is transient and the operation may succeed
    /// on a subsequent attempt.
    ///
    /// Transient errors are those caused by temporary server-side conditions:
    /// rate limiting, gateway errors, and service unavailability. Permanent
    /// errors (authentication failures, bad requests, not-found) must not be
    /// retried because repeating them will produce the same result.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Error::TooManyRequests(_, _) | Error::ServiceUnavailable(_)
        )
    }

    /// Returns the message that may be shown to a user, with no variant tag.
    ///
    /// `Display` (via `#[error(...)]` above) is the **log** representation —
    /// it deliberately includes the variant tag (`"internal: {0}"`,
    /// `"not found: {0}"`, ...) so a log line identifies which branch fired
    /// without needing the source location. `user_message()` is the **user**
    /// representation: the inner message alone, so callers building a
    /// user-facing sentence (e.g. `format!("I encountered an error while
    /// processing your request: {}", err.user_message())`) don't leak that
    /// tag into copy a person reads. The two must never be swapped — logging
    /// `user_message()` loses the variant, and showing `Display` to a user
    /// reads as `"...: internal: <message>"`.
    ///
    /// For the four `#[error(transparent)]` variants (`Sqlx`, `Migrate`,
    /// `Redis`, `SerdeJson`), this returns the same fixed
    /// `"internal server error"` string that `IntoResponse` already returns
    /// for them (see the `match` below) — raw database/cache/serialization
    /// detail is never appropriate to show a user, so there is no inner
    /// message to strip a tag from.
    pub fn user_message(&self) -> &str {
        match self {
            Error::NotFound(msg)
            | Error::Unauthorized(msg)
            | Error::Forbidden(msg)
            | Error::BadRequest(msg)
            | Error::Conflict(msg)
            | Error::TooManyRequests(msg, _)
            | Error::NotImplemented(msg)
            | Error::ServiceUnavailable(msg)
            | Error::DatasourceConnection(msg)
            | Error::CredentialDecryptionFailed(msg)
            | Error::Internal(msg) => msg,
            Error::Sqlx(_) | Error::Migrate(_) | Error::Redis(_) | Error::SerdeJson(_) => {
                "internal server error"
            }
        }
    }
}

/// Convenience alias used throughout the codebase.
pub type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------------------
// Axum integration — convert Error into HTTP responses
// ---------------------------------------------------------------------------

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Error::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            Error::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            Error::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            Error::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            Error::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            Error::TooManyRequests(msg, retry_after) => {
                let body = serde_json::json!({ "detail": msg });
                let mut response = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
                if let Ok(val) = retry_after.to_string().parse() {
                    response.headers_mut().insert("retry-after", val);
                }
                return response;
            }
            Error::NotImplemented(msg) => (StatusCode::NOT_IMPLEMENTED, msg.clone()),
            Error::ServiceUnavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg.clone()),
            // Client-actionable (bad credentials / wrong host / unreachable):
            // surface the full message so the caller can fix the config.
            Error::DatasourceConnection(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            // Server misconfiguration (wrong/rotated encryption key), not a
            // client mistake — 500, but the message is already logged at the
            // decrypt call site (`error!`), so it's surfaced here verbatim
            // rather than re-logged or swallowed like `Error::Internal`.
            Error::CredentialDecryptionFailed(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, msg.clone())
            }
            Error::Internal(msg) => {
                tracing::error!("internal error: {msg}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error".into())
            }
            Error::Sqlx(e) => {
                tracing::error!("database error: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error".into())
            }
            Error::Redis(e) => {
                tracing::error!("redis error: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error".into())
            }
            Error::SerdeJson(e) => {
                tracing::error!("serialization error: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error".into())
            }
            Error::Migrate(e) => {
                tracing::error!("migration error: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error".into())
            }
        };

        let body = serde_json::json!({ "detail": message });
        (status, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datasource_connection_display_is_prefix_free() {
        // KYO-145: this variant exists specifically so its message surfaces
        // verbatim to the user (no `internal: ` / `bad request: ` prefix).
        // Guards against a future refactor reintroducing a prefix.
        let err = Error::DatasourceConnection(
            "Connection test failed — check datasource credentials and connectivity".into(),
        );
        assert_eq!(
            err.to_string(),
            "Connection test failed — check datasource credentials and connectivity"
        );
    }

    #[test]
    fn credential_decryption_failed_display_is_prefix_free() {
        // KYO-221: this variant's whole purpose is to read as a distinct,
        // operator-legible message ("check the encryption key") rather than
        // an authentication failure — a `credential_decryption_failed: `
        // prefix would blur that distinction, so `Display` must not add one.
        let err = Error::CredentialDecryptionFailed(
            "credential could not be decrypted — check the encryption key (field: shared_password)"
                .into(),
        );
        assert_eq!(
            err.to_string(),
            "credential could not be decrypted — check the encryption key (field: shared_password)"
        );
    }

    #[test]
    fn internal_display_still_carries_the_log_prefix() {
        // The log representation must not regress — Display is what shows up
        // in tracing output and must keep identifying the variant.
        let err = Error::Internal("the tool-use budget was exhausted".into());
        assert_eq!(
            err.to_string(),
            "internal: the tool-use budget was exhausted"
        );
    }

    #[test]
    fn internal_user_message_strips_the_log_prefix() {
        // KYO-350: user_message() is the user representation — no "internal: "
        // tag, just the inner message.
        let err = Error::Internal("the tool-use budget was exhausted".into());
        assert!(!err.user_message().contains("internal:"));
        assert_eq!(err.user_message(), "the tool-use budget was exhausted");
    }

    #[test]
    fn transparent_variant_user_message_does_not_leak_raw_detail() {
        // KYO-350: transparent variants (Sqlx, Migrate, Redis, SerdeJson) must
        // never show their raw wrapped error to a user — mirrors the same
        // policy IntoResponse already applies to them (see the `match` in
        // `IntoResponse::into_response` above).
        let serde_err = serde_json::from_str::<i32>("x").unwrap_err();
        let err = Error::SerdeJson(serde_err);
        assert_eq!(err.user_message(), "internal server error");
        assert!(!err.user_message().contains("expected"));
    }
}
