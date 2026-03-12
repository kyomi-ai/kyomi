// SPDX-License-Identifier: AGPL-3.0-or-later

//! Middleware stack — CORS, security headers, request logging.
//!
//! All configuration values are read from `shared/constants.toml` via
//! `kyomi_core::constants`. Nothing is hardcoded here.

use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, Method, Request},
    middleware::Next,
    response::Response,
};
use tower_http::cors::{AllowHeaders, CorsLayer};

/// Build the CORS layer from shared constants.
///
/// Origins, methods, and credentials are read from `shared/constants.toml`
/// so both backends use identical configuration.
pub fn cors_layer() -> CorsLayer {
    let constants = kyomi_core::constants::get();
    let cors = &constants.cors;

    let origins: Vec<HeaderValue> = cors
        .allowed_origins
        .iter()
        .map(|o| {
            o.parse::<HeaderValue>()
                .unwrap_or_else(|_| panic!("invalid CORS origin in constants.toml: {o}"))
        })
        .collect();

    let methods: Vec<Method> = cors
        .allowed_methods
        .iter()
        .map(|m| {
            m.parse::<Method>()
                .unwrap_or_else(|_| panic!("invalid CORS method in constants.toml: {m}"))
        })
        .collect();

    let mut layer = CorsLayer::new()
        .allow_origin(origins)
        .allow_methods(methods)
        .allow_headers(AllowHeaders::mirror_request());

    if cors.allow_credentials {
        layer = layer.allow_credentials(true);
    }

    layer
}

/// Security headers middleware.
///
/// Adds defense-in-depth headers to every response. Header values are read
/// from `shared/constants.toml`.
pub async fn security_headers(
    request: Request<Body>,
    next: Next,
) -> Response {
    let demo_mode = request
        .extensions()
        .get::<DemoModeFlag>()
        .is_some_and(|f| f.0);

    let is_api = request.uri().path().starts_with("/api/")
        || request.uri().path().starts_with("/ws/")
        || request.uri().path().starts_with("/mcp/")
        || request.uri().path().starts_with("/connect/");

    let mut response = next.run(request).await;

    let sh = &kyomi_core::constants::get().security_headers;
    let headers = response.headers_mut();

    // These are read at every request from the global singleton (cheap — just pointer derefs).
    // Using HeaderValue::from_str because the values come from the TOML file, not static strings.
    if let Ok(v) = HeaderValue::from_str(&sh.x_frame_options) {
        headers.insert(HeaderName::from_static("x-frame-options"), v);
    }
    if let Ok(v) = HeaderValue::from_str(&sh.x_content_type_options) {
        headers.insert(HeaderName::from_static("x-content-type-options"), v);
    }
    if let Ok(v) = HeaderValue::from_str(&sh.x_xss_protection) {
        headers.insert(HeaderName::from_static("x-xss-protection"), v);
    }

    if !demo_mode
        && !sh.hsts.is_empty()
        && let Ok(v) = HeaderValue::from_str(&sh.hsts)
    {
        headers.insert(HeaderName::from_static("strict-transport-security"), v);
    }

    // Apply strict CSP only to API/WS routes. Frontend routes need a permissive
    // policy to load scripts, styles, images, and fonts from the same origin.
    if is_api {
        if let Ok(v) = HeaderValue::from_str(&sh.content_security_policy) {
            headers.insert(HeaderName::from_static("content-security-policy"), v);
        }
    } else {
        headers.insert(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(
                "default-src 'self'; \
                 script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; \
                 style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
                 font-src 'self' https://fonts.gstatic.com; \
                 img-src 'self' data: blob:; \
                 connect-src 'self' ws: wss:; \
                 worker-src 'self' blob:; \
                 manifest-src 'self'; \
                 frame-ancestors 'none'",
            ),
        );
    }

    response
}

/// Marker type inserted into request extensions so the security headers
/// middleware can check demo mode without needing full AppState.
#[derive(Clone, Copy)]
pub struct DemoModeFlag(pub bool);
