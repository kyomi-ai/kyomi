// SPDX-License-Identifier: AGPL-3.0-or-later

//! Middleware stack — CORS, security headers, request logging,
//! transparent access-token auto-refresh.
//!
//! All configuration values are read from `data/constants.toml` via
//! `kyomi_core::constants`. Nothing is hardcoded here.

pub mod auth_refresh;

pub use auth_refresh::auth_refresh_middleware;

use axum::{
    body::{Body, to_bytes},
    http::{HeaderName, HeaderValue, Method, Request, header},
    middleware::Next,
    response::Response,
};
use tower_http::cors::{AllowHeaders, CorsLayer};

/// Marker replaced in the embedded frontend shell with its per-response CSP nonce.
pub const CSP_NONCE_PLACEHOLDER: &str = "__KYOMI_CSP_NONCE__";

/// Nonce shared by the security middleware and Leptos SSR for one request.
#[derive(Clone)]
pub struct CspNonce(pub leptos::nonce::Nonce);

/// Build the CORS layer from shared constants.
///
/// Origins, methods, and credentials are read from `data/constants.toml`
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
/// from `data/constants.toml`.
pub async fn security_headers(
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let demo_mode = request
        .extensions()
        .get::<DemoModeFlag>()
        .is_some_and(|f| f.0);

    let is_consent_get = request.method() == Method::GET
        && matches!(
            request.uri().path(),
            "/api/v1/oauth/authorize" | "/api/v1/oauth/authorize/continue"
        );
    let is_api = request.uri().path().starts_with("/api/")
        || request.uri().path().starts_with("/ws/")
        || request.uri().path().starts_with("/mcp/")
        || request.uri().path().starts_with("/connect/");

    // Frontend shell and SSR scripts share one unpredictable nonce. API and
    // scriptless OAuth consent responses do not need one.
    let csp_nonce = (!is_api && !is_consent_get).then(leptos::nonce::Nonce::new);
    if let Some(nonce) = csp_nonce.as_ref() {
        request.extensions_mut().insert(CspNonce(nonce.clone()));
    }

    let mut response = next.run(request).await;

    // Trunk embeds the shell once at build time. Substitute the request
    // nonce before returning HTML so its inline scripts match the header.
    if let Some(nonce) = csp_nonce.as_ref()
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .is_some_and(|value| value.as_bytes().starts_with(b"text/html"))
    {
        // login_ssr_handler already caps rendered app output at 2 MiB; 4 MiB
        // leaves room for the shell. If this bound is exceeded, return a
        // non-HTML error instead of delivering scripts without a matching nonce.
        let body = std::mem::replace(response.body_mut(), Body::empty());
        match to_bytes(body, 4 * 1024 * 1024).await {
            Ok(bytes) => {
                let html = String::from_utf8_lossy(&bytes).replace(
                    &format!("nonce=\"{CSP_NONCE_PLACEHOLDER}\""),
                    &format!("nonce=\"{nonce}\""),
                );
                *response.body_mut() = Body::from(html);
                response.headers_mut().remove(header::CONTENT_LENGTH);
            }
            Err(error) => {
                tracing::error!(%error, "frontend HTML exceeded CSP nonce rewrite limit");
                *response.status_mut() = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
                *response.body_mut() = Body::from("Unable to prepare frontend security policy");
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/plain; charset=utf-8"),
                );
                response.headers_mut().remove(header::CONTENT_LENGTH);
            }
        }
    }

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

    // Consent pages get a scriptless policy, API/WS routes use their configured
    // deny-all policy, and frontend routes use nonce-bound scripts plus the
    // measured external resources and style requirements.
    if is_consent_get
        && headers
            .get("content-type")
            .is_some_and(|value| value.as_bytes().starts_with(b"text/html"))
    {
        headers.insert(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(
                "default-src 'none'; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; font-src https://fonts.gstatic.com; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
            ),
        );
    } else if is_api {
        if let Ok(v) = HeaderValue::from_str(&sh.content_security_policy) {
            headers.insert(HeaderName::from_static("content-security-policy"), v);
        }
    } else {
        let nonce = csp_nonce
            .as_ref()
            .expect("frontend requests receive a CSP nonce");

        headers.insert(
            HeaderName::from_static("content-security-policy"),
            frontend_content_security_policy(nonce),
        );
    }

    response
}

/// Requirements measured from the Trunk shell, WASM output, and SSR renderer.
/// `wasm-unsafe-eval` is needed by the Rust/WASM hydration runtime; Stripe.js
/// and Cloudflare Insights are existing external integrations. Style inline
/// remains for the shell's critical CSS and 54 `style=` attributes inventoried
/// in `crates/kyomi-ui` (including runtime chart, editor, and layout styles).
fn frontend_content_security_policy(nonce: &leptos::nonce::Nonce) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "default-src 'self'; \
         script-src 'self' 'nonce-{nonce}' 'wasm-unsafe-eval' https://js.stripe.com https://static.cloudflareinsights.com; \
         style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
         font-src 'self' https://fonts.gstatic.com; \
         img-src 'self' data: blob: https://*.stripe.com; \
         connect-src 'self' ws: wss: https://api.stripe.com https://cloudflareinsights.com; \
         frame-src https://js.stripe.com https://hooks.stripe.com; \
         worker-src 'self' blob:; \
         manifest-src 'self'; \
         frame-ancestors 'none'"
    ))
    .expect("CSP nonce is a valid header value")
}

/// Marker type inserted into request extensions so the security headers
/// middleware can check demo mode without needing full AppState.
#[derive(Clone, Copy)]
pub struct DemoModeFlag(pub bool);

#[cfg(test)]
mod tests {
    use super::frontend_content_security_policy;

    #[test]
    fn frontend_csp_limits_scripts_to_nonce_and_measured_dependencies() {
        let nonce = leptos::nonce::Nonce::new();
        let policy = frontend_content_security_policy(&nonce);
        let policy = policy.to_str().expect("CSP should be valid ASCII");
        let script_src = policy
            .split(';')
            .find(|directive| directive.trim_start().starts_with("script-src "))
            .expect("frontend CSP should define script-src");

        assert!(script_src.contains(&format!("'nonce-{nonce}'")));
        assert!(script_src.contains("'wasm-unsafe-eval'"));
        assert!(script_src.contains("https://js.stripe.com"));
        assert!(script_src.contains("https://static.cloudflareinsights.com"));
        assert!(!script_src.contains("'unsafe-inline'"));
        assert!(!script_src.contains("'unsafe-eval'"));
    }
}
