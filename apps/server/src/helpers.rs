// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared route helpers used across multiple route modules.

use axum::http::HeaderMap;
use kyomi_auth::token_service::DeviceInfo;
use std::net::IpAddr;

/// Extract device info (user agent, IP, country) from request headers.
///
/// Used by login and signup endpoints that need to record device info for sessions.
pub fn extract_device_info(headers: &HeaderMap) -> DeviceInfo {
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let ip_address = extract_client_ip(headers, None);

    let country_code = headers
        .get("cf-ipcountry")
        .and_then(|v| v.to_str().ok())
        .filter(|s| *s != "XX")
        .map(|s| s.to_uppercase());

    DeviceInfo {
        user_agent,
        ip_address: Some(ip_address),
        country_code,
        oauth_client_id: None,
    }
}

/// Default value for `#[serde(default)]` on boolean fields that should default to `true`.
pub fn default_true() -> bool {
    true
}

/// Send a verification email in a background task (fire-and-forget).
pub fn spawn_verification_email(email: String, name: String, url: String) {
    tokio::spawn(async move {
        let email_svc = kyomi_auth::email_service::EmailService::from_env();
        let sent = email_svc
            .send_verification_email(&email, &name, &url)
            .await;
        if sent {
            tracing::info!("Verification email sent to {email}");
        } else {
            tracing::warn!("Failed to send verification email to {email}");
        }
    });
}

/// Extract client IP from request headers with optional TCP peer address fallback.
///
/// **Trust model** (behind nginx reverse proxy):
///
/// 1. **`X-Real-IP`** (preferred) — nginx sets this from `$remote_addr`, the actual TCP
///    connection source. It **cannot** be spoofed by the client because nginx overwrites
///    any client-supplied value.
///
/// 2. **`X-Forwarded-For`** (fallback, first entry) — nginx **appends** to any existing
///    value via `$proxy_add_x_forwarded_for`. A client can inject a fake first entry
///    (e.g. `X-Forwarded-For: 8.8.8.8`), so this is less reliable. Acceptable as a
///    fallback for dev environments without nginx.
///
/// 3. **Peer address** — the direct TCP socket address. Only useful in local dev without
///    a reverse proxy (in production, this is always the nginx pod IP).
///
/// All header-sourced values are validated with `IpAddr::parse()` to ensure only valid
/// IPv4/IPv6 addresses are accepted (defense-in-depth against malformed headers).
///
/// Returns `"unknown"` if no valid IP can be determined.
///
/// # Examples
///
/// ```
/// # use axum::http::HeaderMap;
/// # use kyomi_server::helpers::extract_client_ip;
/// let mut headers = HeaderMap::new();
/// headers.insert("x-real-ip", "1.2.3.4".parse().unwrap());
/// headers.insert("x-forwarded-for", "8.8.8.8, 1.2.3.4".parse().unwrap());
/// // X-Real-IP wins even though XFF is also present:
/// assert_eq!(extract_client_ip(&headers, None), "1.2.3.4");
/// ```
pub fn extract_client_ip(headers: &HeaderMap, peer_addr: Option<std::net::SocketAddr>) -> String {
    // 1. X-Real-IP — trustworthy: set by nginx from TCP peer ($remote_addr).
    if let Some(real_ip) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        let ip = real_ip.trim();
        if !ip.is_empty() && ip.parse::<IpAddr>().is_ok() {
            return ip.to_string();
        }
    }

    // 2. X-Forwarded-For — less reliable: nginx appends but doesn't replace,
    //    so clients can inject fake first entries. Use first entry as fallback.
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
        && let Some(first_ip) = xff.split(',').next() {
            let ip = first_ip.trim();
            if !ip.is_empty() && ip.parse::<IpAddr>().is_ok() {
                return ip.to_string();
            }
        }

    // 3. TCP peer address — direct connection (local dev without reverse proxy).
    if let Some(addr) = peer_addr {
        return addr.ip().to_string();
    }

    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn prefers_x_real_ip_over_xff() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "10.0.0.1".parse().unwrap());
        headers.insert("x-forwarded-for", "8.8.8.8, 10.0.0.1".parse().unwrap());

        assert_eq!(extract_client_ip(&headers, None), "10.0.0.1");
    }

    #[test]
    fn falls_back_to_xff_when_no_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8".parse().unwrap());

        assert_eq!(extract_client_ip(&headers, None), "1.2.3.4");
    }

    #[test]
    fn falls_back_to_peer_addr() {
        let headers = HeaderMap::new();
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)), 12345);

        assert_eq!(extract_client_ip(&headers, Some(peer)), "192.168.1.42");
    }

    #[test]
    fn returns_unknown_when_nothing_available() {
        let headers = HeaderMap::new();
        assert_eq!(extract_client_ip(&headers, None), "unknown");
    }

    #[test]
    fn ignores_empty_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "  ".parse().unwrap());
        headers.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());

        assert_eq!(extract_client_ip(&headers, None), "1.2.3.4");
    }

    #[test]
    fn trims_whitespace_from_xff() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", " 1.2.3.4 , 5.6.7.8".parse().unwrap());

        assert_eq!(extract_client_ip(&headers, None), "1.2.3.4");
    }

    #[test]
    fn ignores_empty_xff() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "  ".parse().unwrap());
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);

        assert_eq!(extract_client_ip(&headers, Some(peer)), "127.0.0.1");
    }

    #[test]
    fn rejects_non_ip_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "not-an-ip".parse().unwrap());
        headers.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());

        // Falls through to XFF because X-Real-IP is not a valid IP
        assert_eq!(extract_client_ip(&headers, None), "1.2.3.4");
    }

    #[test]
    fn rejects_non_ip_xff() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "malicious-string, 1.2.3.4".parse().unwrap());
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 8080);

        // First XFF entry is not a valid IP, falls through to peer
        assert_eq!(extract_client_ip(&headers, Some(peer)), "10.0.0.1");
    }

    #[test]
    fn accepts_ipv6_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "2001:db8::1".parse().unwrap());

        assert_eq!(extract_client_ip(&headers, None), "2001:db8::1");
    }

    #[test]
    fn rejects_all_invalid_returns_unknown() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "garbage".parse().unwrap());
        headers.insert("x-forwarded-for", "also-garbage".parse().unwrap());

        assert_eq!(extract_client_ip(&headers, None), "unknown");
    }
}
