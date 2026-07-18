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
}
