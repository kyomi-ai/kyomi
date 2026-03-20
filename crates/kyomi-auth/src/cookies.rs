// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cookie helpers for setting and clearing HTTPOnly auth cookies.
//!
//! Must match Python's `set_token_cookies` and `clear_auth_cookies` exactly.
//! Configuration from `shared/constants.toml`.

use axum::http::{HeaderMap, HeaderValue, header::SET_COOKIE};

/// Set the access_token and/or refresh_token HTTPOnly cookies on a response.
///
/// Cookie settings from `shared/constants.toml`:
/// - HTTPOnly, Secure, SameSite=Strict, Path=/
/// - access_token max_age: 15 min (from jwt.access_token_expire_minutes)
/// - refresh_token max_age: 7 days (from jwt.refresh_token_expire_days)
pub fn set_token_cookies(
    headers: &mut HeaderMap,
    access_token: Option<&str>,
    refresh_token: Option<&str>,
) {
    let constants = kyomi_core::constants::get();
    let cookie_cfg = &constants.cookies;
    let jwt_cfg = &constants.jwt;

    let samesite = &cookie_cfg.samesite;
    let path = &cookie_cfg.path;
    let secure = if cookie_cfg.secure { "; Secure" } else { "" };
    let httponly = if cookie_cfg.httponly { "; HttpOnly" } else { "" };

    if let Some(token) = access_token {
        let max_age = jwt_cfg.access_token_expire_minutes * 60; // minutes → seconds
        let cookie = format!(
            "{}={token}; Max-Age={max_age}; Path={path}; SameSite={samesite}{secure}{httponly}",
            cookie_cfg.access_token_name,
        );
        if let Ok(val) = HeaderValue::from_str(&cookie) {
            headers.append(SET_COOKIE, val);
        }
    }

    if let Some(token) = refresh_token {
        let max_age = jwt_cfg.refresh_token_expire_days * 24 * 60 * 60; // days → seconds
        let cookie = format!(
            "{}={token}; Max-Age={max_age}; Path={path}; SameSite={samesite}{secure}{httponly}",
            cookie_cfg.refresh_token_name,
        );
        if let Ok(val) = HeaderValue::from_str(&cookie) {
            headers.append(SET_COOKIE, val);
        }
    }
}

/// Clear both access_token and refresh_token cookies.
///
/// Sets Max-Age=0 to tell the browser to delete them immediately.
pub fn clear_token_cookies(headers: &mut HeaderMap) {
    let constants = kyomi_core::constants::get();
    let cookie_cfg = &constants.cookies;

    let samesite = &cookie_cfg.samesite;
    let path = &cookie_cfg.path;
    let secure = if cookie_cfg.secure { "; Secure" } else { "" };
    let httponly = if cookie_cfg.httponly { "; HttpOnly" } else { "" };

    // Set Max-Age=0 to delete the cookie
    let clear_access = format!(
        "{}=; Max-Age=0; Path={path}; SameSite={samesite}{secure}{httponly}",
        cookie_cfg.access_token_name,
    );
    let clear_refresh = format!(
        "{}=; Max-Age=0; Path={path}; SameSite={samesite}{secure}{httponly}",
        cookie_cfg.refresh_token_name,
    );

    if let Ok(val) = HeaderValue::from_str(&clear_access) {
        headers.append(SET_COOKIE, val);
    }
    if let Ok(val) = HeaderValue::from_str(&clear_refresh) {
        headers.append(SET_COOKIE, val);
    }
}

/// Set the recovery_session cookie (limited-scope JWT for passkey recovery).
///
/// Expires in 15 minutes (900s). HTTPOnly, Secure, SameSite=Strict.
pub fn set_recovery_session_cookie(headers: &mut HeaderMap, token: &str) {
    let constants = kyomi_core::constants::get();
    let cookie_cfg = &constants.cookies;

    let samesite = &cookie_cfg.samesite;
    let path = &cookie_cfg.path;
    let secure = if cookie_cfg.secure { "; Secure" } else { "" };
    let httponly = if cookie_cfg.httponly { "; HttpOnly" } else { "" };

    let cookie = format!(
        "recovery_session={token}; Max-Age=900; Path={path}; SameSite={samesite}{secure}{httponly}",
    );
    if let Ok(val) = HeaderValue::from_str(&cookie) {
        headers.append(SET_COOKIE, val);
    }
}

/// Clear the recovery_session cookie.
pub fn clear_recovery_session_cookie(headers: &mut HeaderMap) {
    let constants = kyomi_core::constants::get();
    let cookie_cfg = &constants.cookies;

    let samesite = &cookie_cfg.samesite;
    let path = &cookie_cfg.path;
    let secure = if cookie_cfg.secure { "; Secure" } else { "" };
    let httponly = if cookie_cfg.httponly { "; HttpOnly" } else { "" };

    let cookie = format!(
        "recovery_session=; Max-Age=0; Path={path}; SameSite={samesite}{secure}{httponly}",
    );
    if let Ok(val) = HeaderValue::from_str(&cookie) {
        headers.append(SET_COOKIE, val);
    }
}

/// Extract a cookie value by name from the Cookie header.
pub fn get_cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let cookie_header = headers.get("cookie")?.to_str().ok()?;
    let prefix = format!("{name}=");

    for cookie in cookie_header.split(';') {
        let cookie = cookie.trim();
        if let Some(value) = cookie.strip_prefix(&prefix) {
            return Some(value);
        }
    }

    None
}
