// SPDX-License-Identifier: AGPL-3.0-or-later

//! Transparent access-token auto-refresh middleware.
//!
//! Sits in front of protected routes. If the `access_token` cookie is missing
//! or expired but a valid `refresh_token` cookie is present, silently mints a
//! new access token and rotates the refresh token so the downstream
//! `AuthUser` extractor sees a fresh credential. The browser gets the new
//! cookies via `Set-Cookie` on the response.
//!
//! Silently no-ops on refresh failure — the canonical 401 from the downstream
//! extractor is what the frontend is already wired to handle.
//!
//! Concurrency: no locks. When multiple concurrent requests from the same
//! tab/session hit this middleware with an expired access token, each one
//! independently mints a new access token and rotates the refresh token.
//! [`kyomi_auth::token_service::verify_refresh_token`]'s grace-period +
//! theft-detection semantics make this safe — all concurrent refreshes see
//! either `Valid` or `GracePeriod` and succeed. Accept the small
//! token-rotation churn; locking here would cascade into user-visible latency
//! spikes on every session resume.

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, Request},
    middleware::Next,
    response::Response,
};

use crate::state::AppState;

/// Axum middleware: transparently refresh an expired access token using the
/// refresh-token cookie, then forward the request with the new access token
/// in the `Cookie` header so the downstream `AuthUser` extractor sees it.
pub async fn auth_refresh_middleware(
    State(state): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let cookie_names = &kyomi_core::constants::get().cookies;
    let access_cookie_name = cookie_names.access_token_name.as_str();
    let refresh_cookie_name = cookie_names.refresh_token_name.as_str();

    // Never refresh tokens on logout — the handler will clear them.
    // Leptos #[server] appends a hash suffix to function names, so we use
    // prefix matching to cover both logout and logout_all_sessions.
    let path = req.uri().path();
    if path.starts_with("/leptos-api/logout") {
        return next.run(req).await;
    }

    // Fast path: if access_token is present AND valid, pass through with no
    // work. This is the common case on every request and must stay cheap.
    let access_ok = extract_cookie(req.headers(), access_cookie_name)
        .map(|tok| kyomi_auth::jwt::validate_token(&tok, &state.config.jwt_secret).is_ok())
        .unwrap_or(false);

    if access_ok {
        return next.run(req).await;
    }

    // Slow path: attempt to refresh using the refresh-token cookie.
    let refresh_cookie = extract_cookie(req.headers(), refresh_cookie_name);

    let refreshed = if let Some(refresh_value) = refresh_cookie {
        let device = crate::helpers::extract_device_info(req.headers());
        match kyomi_auth::token_refresh::refresh_tokens(
            &state.db,
            &state.config.jwt_secret,
            &refresh_value,
            &device,
        )
        .await
        {
            Ok(tokens) => Some(tokens),
            Err(err) => {
                // Do not propagate the error — let the downstream 401 surface
                // canonically (frontend is already wired to handle it).
                tracing::debug!(
                    error = %err,
                    "auth_refresh_middleware: refresh attempt failed, passing through"
                );
                None
            }
        }
    } else {
        None
    };

    if let Some(ref tokens) = refreshed {
        // Rewrite the Cookie header in-place so AuthUser extractor finds the
        // new access_token value.
        rewrite_access_token_cookie(req.headers_mut(), access_cookie_name, &tokens.access_token);
    }

    let mut response = next.run(req).await;

    if let Some(tokens) = refreshed {
        kyomi_auth::cookies::set_token_cookies(
            response.headers_mut(),
            Some(&tokens.access_token),
            Some(&tokens.raw_refresh_token),
        );
    }

    response
}

/// Look up a single cookie value in the request `Cookie` header.
///
/// Returns the raw value verbatim — no URL decoding, no trimming beyond
/// whitespace between cookie pairs. Caller is responsible for whatever
/// encoding the cookie was set with.
pub(crate) fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie_header = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    let prefix = format!("{name}=");
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        if let Some(value) = pair.strip_prefix(&prefix) {
            return Some(value.to_string());
        }
    }
    None
}

/// Rewrite the `Cookie` request header so the named cookie has the new value.
///
/// Preserves all other cookies verbatim. If the named cookie is not present,
/// it is appended. If the `Cookie` header itself is missing, a new one is
/// inserted containing just the named cookie.
pub(crate) fn rewrite_access_token_cookie(
    headers: &mut HeaderMap,
    name: &str,
    new_value: &str,
) {
    let existing = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let new_pair = format!("{name}={new_value}");

    let rebuilt = match existing {
        Some(cookie_header) => {
            let prefix = format!("{name}=");
            let mut pairs: Vec<String> = cookie_header
                .split(';')
                .map(str::trim)
                .filter(|p| !p.is_empty() && !p.starts_with(&prefix))
                .map(|p| p.to_string())
                .collect();
            pairs.push(new_pair);
            pairs.join("; ")
        }
        None => new_pair,
    };

    match HeaderValue::from_str(&rebuilt) {
        Ok(v) => {
            headers.insert(axum::http::header::COOKIE, v);
        }
        Err(e) => {
            // Should never happen — cookie names/values already passed
            // through JWT validation and DB round-trip. Log and skip.
            tracing::warn!(
                error = %e,
                "auth_refresh_middleware: failed to build rewritten Cookie header"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    fn headers_with_cookie(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    #[test]
    fn extract_cookie_finds_named_value() {
        let h = headers_with_cookie("a=1; access_token=abc.def.ghi; b=2");
        assert_eq!(
            extract_cookie(&h, "access_token"),
            Some("abc.def.ghi".to_string())
        );
        assert_eq!(extract_cookie(&h, "a"), Some("1".to_string()));
        assert_eq!(extract_cookie(&h, "missing"), None);
    }

    #[test]
    fn extract_cookie_returns_none_when_header_missing() {
        let h = HeaderMap::new();
        assert_eq!(extract_cookie(&h, "access_token"), None);
    }

    #[test]
    fn rewrite_replaces_existing_cookie_preserving_others() {
        let mut h = headers_with_cookie("a=1; access_token=old; b=2");
        rewrite_access_token_cookie(&mut h, "access_token", "new-token");
        let cookie = h
            .get(axum::http::header::COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        // Other cookies must survive; access_token must be the new value.
        assert!(cookie.contains("a=1"));
        assert!(cookie.contains("b=2"));
        assert!(cookie.contains("access_token=new-token"));
        assert!(!cookie.contains("access_token=old"));
    }

    #[test]
    fn rewrite_appends_when_cookie_absent() {
        let mut h = headers_with_cookie("a=1; b=2");
        rewrite_access_token_cookie(&mut h, "access_token", "fresh");
        let cookie = h
            .get(axum::http::header::COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("a=1"));
        assert!(cookie.contains("b=2"));
        assert!(cookie.contains("access_token=fresh"));
    }

    #[test]
    fn rewrite_inserts_header_when_none_exists() {
        let mut h = HeaderMap::new();
        rewrite_access_token_cookie(&mut h, "access_token", "solo");
        assert_eq!(
            h.get(axum::http::header::COOKIE).unwrap().to_str().unwrap(),
            "access_token=solo"
        );
    }
}
