use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// The JS snippet, embedded at compile time.
const SNIPPET_JS: &str = include_str!("static/k.js");

/// GET /api/k.js — serve the analytics JS snippet.
pub async fn serve_snippet() -> Response {
    let mut response = (StatusCode::OK, SNIPPET_JS).into_response();
    let headers = response.headers_mut();
    headers.insert("content-type", HeaderValue::from_static("application/javascript; charset=utf-8"));
    headers.insert("cache-control", HeaderValue::from_static("public, max-age=86400"));
    headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    response
}
