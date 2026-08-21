// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small `IntoResponse` error type for REST route handlers.
//!
//! `axum::response::Response` is >= 128 bytes (clippy's default
//! `large-error-threshold`), so a handler signature like
//! `Result<T, Response>` trips `clippy::result_large_err` — every `?`
//! propagation site copies the full `Err` payload by value, even on the
//! common path where no error occurs. `RouteError` stores only what's
//! needed to reconstruct the same response (a status code plus a small
//! owned body), keeping the `Err` variant a few dozen bytes regardless of
//! how large the eventual `Response` is.
//!
//! Build one via `.into()`/`?` from the shapes route handlers already
//! construct errors with — `(StatusCode, Json<Value>)`,
//! `(StatusCode, &'static str)`, and `kyomi_core::Error` — then return
//! `Result<T, RouteError>` and let `?` do the rest. `RouteError` itself
//! never duplicates status/body logic that already lives elsewhere: the
//! `kyomi_core::Error` variant delegates straight to that type's own
//! `IntoResponse` impl.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

/// Small owned error type for REST route handlers — see module docs.
#[derive(Debug)]
pub(crate) enum RouteError {
    /// A JSON error body, e.g.
    /// `(StatusCode::BAD_REQUEST, Json(json!({"error": "..."})))`.
    Json(StatusCode, serde_json::Value),
    /// A plain-text error body, e.g. `(StatusCode::UNAUTHORIZED, "Not logged in")`.
    Text(StatusCode, String),
    /// Delegates to `kyomi_core::Error`'s own status/body mapping — used
    /// wherever the error already comes from the shared application error
    /// type, so its status-code and body logic isn't duplicated here.
    Core(kyomi_core::Error),
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        match self {
            RouteError::Json(status, body) => (status, Json(body)).into_response(),
            RouteError::Text(status, body) => (status, body).into_response(),
            RouteError::Core(err) => err.into_response(),
        }
    }
}

impl From<(StatusCode, Json<serde_json::Value>)> for RouteError {
    fn from((status, Json(body)): (StatusCode, Json<serde_json::Value>)) -> Self {
        RouteError::Json(status, body)
    }
}

impl From<(StatusCode, &'static str)> for RouteError {
    fn from((status, body): (StatusCode, &'static str)) -> Self {
        RouteError::Text(status, body.to_string())
    }
}

impl From<kyomi_core::Error> for RouteError {
    fn from(err: kyomi_core::Error) -> Self {
        RouteError::Core(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::json;

    async fn body_bytes(resp: Response) -> Vec<u8> {
        to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .expect("test body should be readable")
            .to_vec()
    }

    /// Guards the whole point of this type: it must stay well under
    /// `clippy::result_large_err`'s default 128-byte threshold no matter how
    /// large `axum::response::Response` itself grows.
    #[test]
    fn route_error_stays_small() {
        let size = std::mem::size_of::<RouteError>();
        assert!(
            size < 128,
            "RouteError grew to {size} bytes — this reintroduces \
             clippy::result_large_err at every call site that returns it"
        );
    }

    #[tokio::test]
    async fn json_variant_preserves_status_and_exact_body() {
        let err: RouteError =
            (StatusCode::BAD_REQUEST, Json(json!({"error": "bad client_id"}))).into();
        let resp = err.into_response();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_bytes(resp).await;
        assert_eq!(
            body,
            serde_json::to_vec(&json!({"error": "bad client_id"})).unwrap()
        );
    }

    #[tokio::test]
    async fn text_variant_preserves_status_and_exact_body() {
        let err: RouteError = (StatusCode::UNAUTHORIZED, "Not logged in").into();
        let resp = err.into_response();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_bytes(resp).await;
        assert_eq!(body, b"Not logged in");
    }

    #[tokio::test]
    async fn core_variant_delegates_to_kyomi_core_error_response() {
        // The RouteError::Core path must produce byte-identical output to
        // calling `kyomi_core::Error::into_response()` directly — it's a
        // pure delegation, not a reimplementation.
        let direct = kyomi_core::Error::BadRequest("no workspace associated with user".into())
            .into_response();
        let via_route_error: RouteError =
            kyomi_core::Error::BadRequest("no workspace associated with user".into()).into();
        let resp = via_route_error.into_response();

        assert_eq!(resp.status(), direct.status());
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let (direct_body, resp_body) =
            tokio::join!(body_bytes(direct), body_bytes(resp));
        assert_eq!(direct_body, resp_body);
        assert_eq!(
            resp_body,
            serde_json::to_vec(&json!({"detail": "no workspace associated with user"})).unwrap()
        );
    }
}
