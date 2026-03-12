use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use clickhouse::Client;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tracing::{error, warn};
use url::Url;

type HmacSha256 = Hmac<Sha256>;

use crate::models::{BatchEntry, EventPayload, EventRow};

/// Maximum payload size (64KB).
const MAX_PAYLOAD_SIZE: usize = 65536;

/// Shared application state loaded once at startup.
#[derive(Clone)]
pub struct AppState {
    pub clickhouse: Arc<Client>,
    pub signing_secret: String,
    pub salt_secret: String,
    pub redis: Option<redis::aio::ConnectionManager>,
    pub batcher: crate::batcher::EventBatcher,
    pub transform_engine: Option<Arc<crate::transform::engine::TransformEngine>>,
    pub ch_http_config: Option<crate::transform::engine::ChHttpConfig>,
}

impl AppState {
    pub fn new(
        clickhouse: Arc<Client>,
        redis: Option<redis::aio::ConnectionManager>,
        batcher: crate::batcher::EventBatcher,
        transform_engine: Option<Arc<crate::transform::engine::TransformEngine>>,
        ch_http_config: Option<crate::transform::engine::ChHttpConfig>,
    ) -> Self {
        Self {
            clickhouse,
            signing_secret: std::env::var("ANALYTICS_SIGNING_SECRET").unwrap_or_default(),
            salt_secret: std::env::var("ANALYTICS_SALT_SECRET").unwrap_or_default(),
            redis,
            batcher,
            transform_engine,
            ch_http_config,
        }
    }
}

/// Compute a deterministic daily salt from a secret and the current day.
/// Uses `SHA256(secret + days_since_epoch)` so all replicas produce the same
/// salt without any shared state.
fn compute_daily_salt(secret: &str) -> String {
    if secret.is_empty() {
        warn!("ANALYTICS_SALT_SECRET not set — visitor_id will be empty");
        return String::new();
    }
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86400;
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.update(days.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

/// Compute a privacy-preserving visitor ID.
/// `SHA256(site_id + ip + user_agent + daily_salt)` truncated to 16 hex chars (64 bits).
/// The IP is never stored — only its hash contribution persists.
fn compute_visitor_id(site_id: &str, ip: &str, user_agent: &str, salt: &str) -> String {
    if salt.is_empty() {
        return String::new();
    }
    let mut hasher = Sha256::new();
    hasher.update(site_id.as_bytes());
    hasher.update(ip.as_bytes());
    hasher.update(user_agent.as_bytes());
    hasher.update(salt.as_bytes());
    let hash = hex::encode(hasher.finalize());
    hash[..16].to_string()
}

/// Decoded payload from a signed analytics key.
#[derive(serde::Deserialize)]
struct KeyPayload {
    /// site_id
    s: String,
    /// workspace_id — used to derive the per-site ClickHouse database name.
    /// `#[serde(default)]` handles old keys that pre-date the `w` field; such
    /// keys produce an empty workspace_id and are rejected at the guard below.
    #[serde(default)]
    w: String,
    /// allowed domains
    d: Vec<String>,
}

/// Verify a signed analytics key and extract its payload.
/// Key format: `base64url(json_payload).base64url(hmac_sha256_signature)`.
fn verify_signed_key(key: &str, secret: &str) -> Option<KeyPayload> {
    let (payload_b64, sig_b64) = key.split_once('.')?;
    if payload_b64.is_empty() || sig_b64.is_empty() {
        return None;
    }

    let provided_sig = URL_SAFE_NO_PAD.decode(sig_b64).ok()?;

    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload_b64.as_bytes());
    let expected_sig = mac.finalize().into_bytes();

    // Constant-time comparison to prevent timing attacks
    if provided_sig.as_slice().ct_eq(&expected_sig).into() {
        let payload_json = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
        serde_json::from_slice(&payload_json).ok()
    } else {
        None
    }
}

/// Extract the client's real IP from proxy headers.
/// Priority: CF-Connecting-IP > X-Forwarded-For (first entry) > empty.
fn extract_client_ip(headers: &HeaderMap) -> String {
    // Cloudflare's single-IP header
    if let Some(cf_ip) = headers.get("cf-connecting-ip").and_then(|v| v.to_str().ok()) {
        let trimmed = cf_ip.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    // X-Forwarded-For: client, proxy1, proxy2
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff.split(',').next() {
            let trimmed = first.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    String::new()
}

/// Parsed user-agent information.
struct UaInfo {
    browser: String,
    browser_version: String,
    os: String,
    os_version: String,
    device_type: String,
    is_bot: bool,
}

/// Parse a User-Agent string into browser, OS, device type, and bot flag.
fn parse_user_agent(ua: &str) -> UaInfo {
    if ua.is_empty() {
        return UaInfo {
            browser: String::new(),
            browser_version: String::new(),
            os: String::new(),
            os_version: String::new(),
            device_type: String::new(),
            is_bot: false,
        };
    }

    let result = woothee::parser::Parser::new().parse(ua);
    match result {
        Some(parsed) => {
            let is_bot = parsed.category == "crawler";

            // Determine device type from OS/category
            let device_type = if is_bot {
                "bot".to_string()
            } else {
                classify_device(ua, &parsed.os)
            };

            UaInfo {
                browser: parsed.name.to_string(),
                browser_version: parsed.version.to_string(),
                os: parsed.os.to_string(),
                os_version: parsed.os_version.to_string(),
                device_type,
                is_bot,
            }
        }
        None => UaInfo {
            browser: String::new(),
            browser_version: String::new(),
            os: String::new(),
            os_version: String::new(),
            device_type: String::new(),
            is_bot: false,
        },
    }
}

/// Classify device type based on OS and UA string.
fn classify_device(ua: &str, os: &str) -> String {
    let ua_lower = ua.to_lowercase();
    match os {
        "Android" => {
            if ua_lower.contains("tablet") || (ua_lower.contains("android") && !ua_lower.contains("mobile")) {
                "tablet".to_string()
            } else {
                "mobile".to_string()
            }
        }
        "iPhone" | "iPod" => "mobile".to_string(),
        "iPad" => "tablet".to_string(),
        _ if ua_lower.contains("mobile") || ua_lower.contains("phone") => "mobile".to_string(),
        _ if ua_lower.contains("tablet") => "tablet".to_string(),
        _ => "desktop".to_string(),
    }
}

/// Add CORS headers to a response.
fn cors_headers(origin: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let origin_val = if origin.is_empty() { "*" } else { origin };
    headers.insert(
        "access-control-allow-origin",
        HeaderValue::from_str(origin_val).unwrap_or_else(|_| HeaderValue::from_static("*")),
    );
    headers.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("POST, OPTIONS"),
    );
    headers.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("Content-Type"),
    );
    headers.insert(
        "access-control-max-age",
        HeaderValue::from_static("86400"),
    );
    headers
}

/// Check whether the request Origin is allowed by the signed key's domain list.
/// Returns `true` if access is allowed, `false` if it should be rejected.
///
/// When `allowed_domains` is `None` (signed key issued with no domain restriction),
/// any origin is allowed. When a domain list is present, Origin MUST match.
fn check_origin_allowed(origin: &str, allowed_domains: &Option<Vec<String>>) -> bool {
    let Some(domains) = allowed_domains else {
        // Signed key has no domain restriction — allow any origin
        return true;
    };

    // Key-based mode requires an Origin header
    if origin == "*" {
        // No Origin header was sent — reject
        return false;
    }

    // Parse origin to extract hostname
    let Some(host) = Url::parse(origin)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
    else {
        // Couldn't parse origin — reject
        return false;
    };

    // Match exact domain or any subdomain (e.g. "example.com" also matches "app.example.com")
    domains.iter().any(|d| d == &host || host.ends_with(&format!(".{d}")))
}

/// CORS preflight handler for OPTIONS /api/collect.
pub async fn collect_preflight(headers: HeaderMap) -> Response {
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("*");

    let mut response = StatusCode::NO_CONTENT.into_response();
    let cors = cors_headers(origin);
    response.headers_mut().extend(cors);
    response
}

/// POST /api/collect — receive an analytics event.
pub async fn collect_event(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("*");

    // Check payload size
    if body.len() > MAX_PAYLOAD_SIZE {
        let mut response = StatusCode::PAYLOAD_TOO_LARGE.into_response();
        response.headers_mut().extend(cors_headers(origin));
        return response;
    }

    // Parse JSON from body (content type may be text/plain to avoid CORS preflight)
    let payload: EventPayload = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => {
            let mut response = StatusCode::BAD_REQUEST.into_response();
            response.headers_mut().extend(cors_headers(origin));
            return response;
        }
    };

    // Determine site_id and workspace_id: signed key only (legacy raw `s` field is rejected)
    let (site_id, workspace_id, allowed_domains) = if !payload.key.is_empty() {
        if state.signing_secret.is_empty() {
            // No secret configured — can't verify signed keys
            let mut response = StatusCode::FORBIDDEN.into_response();
            response.headers_mut().extend(cors_headers(origin));
            return response;
        }
        match verify_signed_key(&payload.key, &state.signing_secret) {
            Some(kp) => (kp.s, kp.w, Some(kp.d)),
            None => {
                let mut response = StatusCode::FORBIDDEN.into_response();
                response.headers_mut().extend(cors_headers(origin));
                return response;
            }
        }
    } else if !payload.s.is_empty() {
        // Legacy mode (raw `s` field) is no longer supported: per-site databases require
        // a signed key to embed the workspace_id. Sites must migrate to `key=<signed_key>`.
        tracing::warn!(site_id = %payload.s, "Rejected legacy event: signed key required for per-site databases");
        let mut response = StatusCode::ACCEPTED.into_response();
        response.headers_mut().extend(cors_headers(origin));
        return response;
    } else {
        let mut response = StatusCode::BAD_REQUEST.into_response();
        response.headers_mut().extend(cors_headers(origin));
        return response;
    };

    // Reject signed keys that pre-date the `w` (workspace_id) field.
    // Without workspace_id the database name cannot be computed, so the event
    // would be routed to a non-existent database and silently dropped at flush time.
    // Return 202 (not 403) to avoid breaking old client deployments with a hard error.
    if workspace_id.is_empty() {
        tracing::warn!(
            site_id = %site_id,
            "Signed key missing workspace_id (w field) — old key format, event dropped. Regenerate the site key."
        );
        let mut response = StatusCode::ACCEPTED.into_response();
        response.headers_mut().extend(cors_headers(origin));
        return response;
    }

    // Validate site_id and URL length
    if site_id.len() > 128 || payload.u.len() > 2048 {
        let mut response = StatusCode::BAD_REQUEST.into_response();
        response.headers_mut().extend(cors_headers(origin));
        return response;
    }

    // Validate Origin against allowed domains (key-based mode requires Origin)
    if !check_origin_allowed(origin, &allowed_domains) {
        let mut response = StatusCode::FORBIDDEN.into_response();
        response.headers_mut().extend(cors_headers(origin));
        return response;
    }

    // Check quota (fail-open: if Redis unavailable, events flow freely)
    if let Some(ref redis) = state.redis {
        let mut conn = redis.clone();
        match crate::quota::check_quota(&mut conn, &workspace_id).await {
            crate::quota::QuotaResult::Blocked => {
                let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
                response.headers_mut().extend(cors_headers(origin));
                return response;
            }
            _ => {} // Allowed or GracePeriod — continue
        }
    }

    // Parse User-Agent and reject bots early
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ua_info = parse_user_agent(user_agent);
    if ua_info.is_bot {
        // Return 202 but don't insert — bots should not pollute analytics
        let mut response = StatusCode::ACCEPTED.into_response();
        response.headers_mut().extend(cors_headers(origin));
        return response;
    }

    // Compute privacy-preserving visitor ID
    let client_ip = extract_client_ip(&headers);
    let salt = compute_daily_salt(&state.salt_secret);
    let visitor_id = compute_visitor_id(&site_id, &client_ip, user_agent, &salt);

    // Parse the page URL
    let (hostname, pathname, utms) = parse_url(&payload.u);

    // Extract referrer source domain
    let referrer_source = parse_referrer_source(&payload.r);

    // Get country from Cloudflare header (free geo-IP)
    let country_code = headers
        .get("cf-ipcountry")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Current timestamp as epoch milliseconds
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    // Compute the per-site ClickHouse database name.
    let database = database_name(&site_id);

    // Resolve session ID server-side via Redis (30-minute sliding window).
    // Replaces the old client-side sessionStorage approach which fragmented
    // sessions across tabs.
    let session_id = resolve_session_id(&state.redis, &site_id, &visitor_id).await;

    // Resolve user ID server-side via Redis.
    // Fills in user_id for events where k.js hasn't called identify() yet
    // (e.g., initial pageview on page load/refresh before React boots).
    let user_id = resolve_user_id(
        &state.redis,
        &site_id,
        &session_id,
        &payload.uid,
        payload.i == 1,
    )
    .await;

    let row = EventRow {
        visitor_id,
        session_id,
        user_id,
        timestamp,
        event_name: payload.n,
        hostname,
        pathname,
        referrer: payload.r,
        referrer_source,
        utm_source: utms.get("utm_source").cloned().unwrap_or_default(),
        utm_medium: utms.get("utm_medium").cloned().unwrap_or_default(),
        utm_campaign: utms.get("utm_campaign").cloned().unwrap_or_default(),
        utm_term: utms.get("utm_term").cloned().unwrap_or_default(),
        utm_content: utms.get("utm_content").cloned().unwrap_or_default(),
        country_code,
        region: String::new(),
        city: String::new(),
        browser: ua_info.browser,
        browser_version: ua_info.browser_version,
        os: ua_info.os,
        os_version: ua_info.os_version,
        device_type: ua_info.device_type,
        screen_width: payload.w,
        screen_height: payload.h,
        properties: match payload.p {
            Some(v) if v.is_object() => serde_json::to_string(&v).unwrap_or_else(|_| "{}".into()),
            _ => "{}".into(),
        },
    };

    // Ensure transform MVs exist for this database (idempotent, runs once per database)
    if let (Some(engine), Some(ch_config)) = (&state.transform_engine, &state.ch_http_config) {
        engine.ensure_database_schemas(ch_config, &database).await;
    }

    // Submit to batcher — returns immediately (non-blocking)
    if state.batcher.submit(BatchEntry { database, row }).is_err() {
        // Channel full — backpressure
        let mut response = StatusCode::SERVICE_UNAVAILABLE.into_response();
        response.headers_mut().extend(cors_headers(origin));
        return response;
    }

    let mut response = StatusCode::ACCEPTED.into_response();
    response.headers_mut().extend(cors_headers(origin));
    response
}

/// Parse a URL into (hostname, pathname, utm_params).
fn parse_url(raw: &str) -> (String, String, HashMap<String, String>) {
    let Ok(parsed) = Url::parse(raw) else {
        return (String::new(), "/".to_string(), HashMap::new());
    };

    let hostname = parsed.host_str().unwrap_or("").to_string();
    let pathname = if parsed.path().is_empty() {
        "/".to_string()
    } else {
        parsed.path().to_string()
    };

    let mut utms = HashMap::new();
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "utm_source" | "utm_medium" | "utm_campaign" | "utm_term" | "utm_content" => {
                utms.insert(key.into_owned(), value.into_owned());
            }
            _ => {}
        }
    }

    (hostname, pathname, utms)
}

/// Extract the domain from a referrer URL as the source.
fn parse_referrer_source(referrer: &str) -> String {
    if referrer.is_empty() {
        return String::new();
    }
    Url::parse(referrer)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default()
}

/// Session TTL: 30 minutes of inactivity resets the session.
const SESSION_TTL_SECS: u64 = 1800;

/// Resolve a user ID for a session using Redis.
///
/// Three cases based on the `identified` flag (whether k.js has called identify()):
///
/// 1. `uid` is non-empty: use it, and cache the mapping in Redis.
/// 2. `uid` is empty AND `identified` is true: user explicitly signed out — clear cache, return empty.
/// 3. `uid` is empty AND `identified` is false: k.js hasn't called identify() yet — look up cache.
///
/// If Redis is unavailable, falls back to the client-sent `uid` (same as today).
async fn resolve_user_id(
    redis: &Option<redis::aio::ConnectionManager>,
    site_id: &str,
    session_id: &str,
    client_uid: &str,
    identified: bool,
) -> String {
    // Case 1: client sent a user_id — authoritative, cache it
    if !client_uid.is_empty() {
        if let Some(redis) = redis {
            let key = format!("user:{site_id}:{session_id}");
            let mut conn = redis.clone();
            let _: Result<(), _> = redis::cmd("SET")
                .arg(&key)
                .arg(client_uid)
                .arg("EX")
                .arg(SESSION_TTL_SECS)
                .query_async(&mut conn)
                .await;
        }
        return client_uid.to_string();
    }

    // Case 2: explicit sign-out (identify("") was called) — clear cache
    if identified {
        if let Some(redis) = redis {
            let key = format!("user:{site_id}:{session_id}");
            let mut conn = redis.clone();
            let _: Result<(), _> = redis::cmd("DEL")
                .arg(&key)
                .query_async(&mut conn)
                .await;
        }
        return String::new();
    }

    // Case 3: k.js hasn't called identify() yet — look up cache
    let Some(redis) = redis else {
        return String::new();
    };
    let key = format!("user:{site_id}:{session_id}");
    let mut conn = redis.clone();
    match redis::cmd("GET")
        .arg(&key)
        .query_async::<Option<String>>(&mut conn)
        .await
    {
        Ok(Some(uid)) => uid,
        Ok(None) => String::new(),
        Err(e) => {
            warn!(error = %e, "Redis user_id lookup failed — falling back to client uid");
            String::new()
        }
    }
}

/// Resolve a session ID for a visitor using Redis.
///
/// Uses an atomic Lua script: GET the key, if it exists return the value and refresh TTL;
/// if not, generate a new session ID and SET with TTL.
/// Single Redis round trip, no race conditions.
///
/// If Redis is unavailable, returns a deterministic session ID derived from the
/// visitor_id so events from the same visitor still group together. This is
/// degraded (no 30-minute sliding window) but avoids silent session fragmentation.
async fn resolve_session_id(
    redis: &Option<redis::aio::ConnectionManager>,
    site_id: &str,
    visitor_id: &str,
) -> String {
    let Some(redis) = redis else {
        warn!("Redis unavailable — session tracking degraded (no sliding window)");
        return deterministic_session_id(site_id, visitor_id);
    };

    let key = format!("session:{site_id}:{visitor_id}");
    let mut conn = redis.clone();

    // Lua script: atomic GET-or-SET with TTL refresh
    // Returns the existing session ID (refreshed TTL) or sets a new one
    let script = redis::Script::new(
        r#"
        local existing = redis.call('GET', KEYS[1])
        if existing then
            redis.call('EXPIRE', KEYS[1], ARGV[2])
            return existing
        end
        redis.call('SET', KEYS[1], ARGV[1], 'EX', ARGV[2])
        return ARGV[1]
        "#,
    );

    let new_id = generate_random_session_id();

    match script
        .key(&key)
        .arg(&new_id)
        .arg(SESSION_TTL_SECS)
        .invoke_async::<String>(&mut conn)
        .await
    {
        Ok(session_id) => session_id,
        Err(e) => {
            error!(error = %e, "Redis session lookup failed — session tracking degraded");
            deterministic_session_id(site_id, visitor_id)
        }
    }
}

/// Produce a deterministic session ID when Redis is unavailable.
/// Uses SHA256(site_id + visitor_id + day) truncated to 16 hex chars.
/// Same visitor on the same day always gets the same session ID, avoiding
/// fragmentation. The trade-off: no 30-minute sliding window boundary.
fn deterministic_session_id(site_id: &str, visitor_id: &str) -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86400;
    let mut hasher = Sha256::new();
    hasher.update(b"session:");
    hasher.update(site_id.as_bytes());
    hasher.update(b":");
    hasher.update(visitor_id.as_bytes());
    hasher.update(b":");
    hasher.update(days.to_string().as_bytes());
    let hash = hex::encode(hasher.finalize());
    hash[..16].to_string()
}

/// Generate a random 16-character hex session ID.
fn generate_random_session_id() -> String {
    let mut buf = [0u8; 8];
    getrandom::getrandom(&mut buf).expect("getrandom failed");
    hex::encode(buf)
}

/// Compute the per-site ClickHouse database name from workspace and site IDs.
///
/// Format: `site_{site_id}`
///
/// NOTE: This MUST stay in sync with `analytics_clickhouse::database_name()` in kyomi-auth,
/// which is the **authoritative copy**. Both binaries independently compute the same name —
/// the collector writes events there, the backend provisions the database. If the formula
/// ever changes, update both copies and keep their unit tests in sync.
fn database_name(site_id: &str) -> String {
    format!("site_{site_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_visitor_id_deterministic() {
        let id1 = compute_visitor_id("site1", "1.2.3.4", "Mozilla/5.0", "salt123");
        let id2 = compute_visitor_id("site1", "1.2.3.4", "Mozilla/5.0", "salt123");
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 16);
    }

    #[test]
    fn test_compute_visitor_id_different_inputs() {
        let id1 = compute_visitor_id("site1", "1.2.3.4", "Mozilla/5.0", "salt123");
        let id2 = compute_visitor_id("site1", "5.6.7.8", "Mozilla/5.0", "salt123");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_compute_visitor_id_different_salt() {
        let id1 = compute_visitor_id("site1", "1.2.3.4", "Mozilla/5.0", "salt_day1");
        let id2 = compute_visitor_id("site1", "1.2.3.4", "Mozilla/5.0", "salt_day2");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_compute_visitor_id_empty_salt() {
        let id = compute_visitor_id("site1", "1.2.3.4", "Mozilla/5.0", "");
        assert_eq!(id, "");
    }

    #[test]
    fn test_extract_client_ip_cf_header() {
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.1"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("10.0.0.1, 10.0.0.2"));
        assert_eq!(extract_client_ip(&headers), "203.0.113.1");
    }

    #[test]
    fn test_extract_client_ip_xff() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.5, 10.0.0.1"));
        assert_eq!(extract_client_ip(&headers), "203.0.113.5");
    }

    #[test]
    fn test_extract_client_ip_xff_single() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("192.168.1.1"));
        assert_eq!(extract_client_ip(&headers), "192.168.1.1");
    }

    #[test]
    fn test_extract_client_ip_none() {
        let headers = HeaderMap::new();
        assert_eq!(extract_client_ip(&headers), "");
    }

    #[test]
    fn test_parse_user_agent_chrome() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
        let info = parse_user_agent(ua);
        assert_eq!(info.browser, "Chrome");
        assert!(!info.is_bot);
        assert_eq!(info.device_type, "desktop");
    }

    #[test]
    fn test_parse_user_agent_googlebot() {
        let ua = "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";
        let info = parse_user_agent(ua);
        assert!(info.is_bot);
        assert_eq!(info.device_type, "bot");
    }

    #[test]
    fn test_parse_user_agent_mobile() {
        let ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
        let info = parse_user_agent(ua);
        assert!(!info.is_bot);
        assert_eq!(info.device_type, "mobile");
    }

    #[test]
    fn test_parse_user_agent_empty() {
        let info = parse_user_agent("");
        assert_eq!(info.browser, "");
        assert!(!info.is_bot);
    }

    #[test]
    fn test_classify_device_android_mobile() {
        assert_eq!(classify_device("Mozilla/5.0 (Linux; Android 13; Pixel 7) Mobile Safari", "Android"), "mobile");
    }

    #[test]
    fn test_classify_device_android_tablet() {
        assert_eq!(classify_device("Mozilla/5.0 (Linux; Android 13; SM-X200) Safari", "Android"), "tablet");
    }

    #[test]
    fn test_classify_device_ipad() {
        assert_eq!(classify_device("Mozilla/5.0 (iPad; CPU OS 17_0)", "iPad"), "tablet");
    }

    #[test]
    fn test_classify_device_windows() {
        assert_eq!(classify_device("Mozilla/5.0 (Windows NT 10.0; Win64; x64)", "Windows"), "desktop");
    }

    #[test]
    fn test_parse_url_with_utms() {
        let (host, path, utms) = parse_url("https://example.com/page?utm_source=google&utm_medium=cpc");
        assert_eq!(host, "example.com");
        assert_eq!(path, "/page");
        assert_eq!(utms.get("utm_source").unwrap(), "google");
        assert_eq!(utms.get("utm_medium").unwrap(), "cpc");
    }

    #[test]
    fn test_parse_referrer_source_domain() {
        assert_eq!(parse_referrer_source("https://www.google.com/search?q=test"), "www.google.com");
    }

    #[test]
    fn test_parse_referrer_source_empty() {
        assert_eq!(parse_referrer_source(""), "");
    }

    #[test]
    fn test_verify_signed_key_roundtrip() {
        // Generate a key the same way the backend does
        let payload = serde_json::json!({"s": "abcd1234", "w": "ws_test", "d": ["example.com"]});
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        let payload_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload_bytes);

        use hmac::Mac;
        let mut mac = HmacSha256::new_from_slice(b"test-secret").unwrap();
        mac.update(payload_b64.as_bytes());
        let sig = mac.finalize().into_bytes();
        let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&sig);

        let key = format!("{payload_b64}.{sig_b64}");
        let result = verify_signed_key(&key, "test-secret");
        assert!(result.is_some());
        let kp = result.unwrap();
        assert_eq!(kp.s, "abcd1234");
        assert_eq!(kp.d, vec!["example.com"]);
    }

    #[test]
    fn test_verify_signed_key_wrong_secret() {
        let payload = serde_json::json!({"s": "abcd1234", "w": "ws_test", "d": ["example.com"]});
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        let payload_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload_bytes);

        use hmac::Mac;
        let mut mac = HmacSha256::new_from_slice(b"correct-secret").unwrap();
        mac.update(payload_b64.as_bytes());
        let sig = mac.finalize().into_bytes();
        let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&sig);

        let key = format!("{payload_b64}.{sig_b64}");
        assert!(verify_signed_key(&key, "wrong-secret").is_none());
    }

    #[test]
    fn test_verify_signed_key_malformed() {
        assert!(verify_signed_key("no-dot-separator", "secret").is_none());
        assert!(verify_signed_key("", "secret").is_none());
        assert!(verify_signed_key(".", "secret").is_none());
    }

    // --- check_origin_allowed tests ---

    #[test]
    fn test_origin_allowed_no_domain_restriction() {
        // allowed_domains = None means the key was issued with no domain restriction
        assert!(check_origin_allowed("*", &None));
        assert!(check_origin_allowed("https://anything.com", &None));
    }

    #[test]
    fn test_origin_allowed_matching_domain() {
        let domains = Some(vec!["example.com".to_string(), "other.com".to_string()]);
        assert!(check_origin_allowed("https://example.com", &domains));
        assert!(check_origin_allowed("https://other.com", &domains));
    }

    #[test]
    fn test_origin_allowed_subdomain() {
        let domains = Some(vec!["example.com".to_string()]);
        assert!(check_origin_allowed("https://app.example.com", &domains));
        assert!(check_origin_allowed("https://www.example.com", &domains));
        assert!(check_origin_allowed("https://deep.sub.example.com", &domains));
    }

    #[test]
    fn test_origin_rejected_non_matching_domain() {
        let domains = Some(vec!["example.com".to_string()]);
        assert!(!check_origin_allowed("https://evil.com", &domains));
        // Must not match suffix without dot boundary (evil-example.com != example.com)
        assert!(!check_origin_allowed("https://evil-example.com", &domains));
    }

    #[test]
    fn test_origin_rejected_missing_origin_key_mode() {
        // Key-based mode with no Origin header (sentinel "*") — must reject
        let domains = Some(vec!["example.com".to_string()]);
        assert!(!check_origin_allowed("*", &domains));
    }

    #[test]
    fn test_origin_rejected_unparseable() {
        let domains = Some(vec!["example.com".to_string()]);
        assert!(!check_origin_allowed("not-a-url", &domains));
        assert!(!check_origin_allowed("", &domains));
    }

    // --- database_name tests ---
    // These MUST use the same inputs as the equivalent tests in
    // analytics_clickhouse::database_name() in kyomi-auth to guarantee
    // the two copies produce identical output for real-world values.

    #[test]
    fn test_database_name() {
        assert_eq!(database_name("deadbeef01234567"), "site_deadbeef01234567");
    }

    // --- generate_random_session_id tests ---

    #[test]
    fn test_generate_random_session_id_format() {
        let id = generate_random_session_id();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_random_session_id_unique() {
        let id1 = generate_random_session_id();
        let id2 = generate_random_session_id();
        assert_ne!(id1, id2);
    }

    // --- resolve_session_id tests ---

    #[tokio::test]
    async fn test_resolve_session_id_no_redis_returns_deterministic() {
        let id1 = resolve_session_id(&None, "site1", "visitor1").await;
        let id2 = resolve_session_id(&None, "site1", "visitor1").await;
        // Without Redis, same visitor gets the same deterministic session ID
        assert_eq!(id1.len(), 16);
        assert_eq!(id1, id2);
    }

    #[tokio::test]
    async fn test_resolve_session_id_no_redis_different_visitors() {
        let id1 = resolve_session_id(&None, "site1", "visitor1").await;
        let id2 = resolve_session_id(&None, "site1", "visitor2").await;
        // Different visitors get different session IDs
        assert_ne!(id1, id2);
    }

    // --- deterministic_session_id tests ---

    #[test]
    fn test_deterministic_session_id_stable() {
        let id1 = deterministic_session_id("site1", "visitor1");
        let id2 = deterministic_session_id("site1", "visitor1");
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 16);
    }

    #[test]
    fn test_deterministic_session_id_varies_by_visitor() {
        let id1 = deterministic_session_id("site1", "visitor1");
        let id2 = deterministic_session_id("site1", "visitor2");
        assert_ne!(id1, id2);
    }

    // --- resolve_user_id tests ---

    #[tokio::test]
    async fn test_resolve_user_id_identified_with_uid_returns_uid() {
        // Client sent uid="user-abc" with identified=true → use uid, don't look up Redis
        let result = resolve_user_id(&None, "site1", "session1", "user-abc", true).await;
        assert_eq!(result, "user-abc");
    }

    #[tokio::test]
    async fn test_resolve_user_id_identified_empty_uid_returns_empty() {
        // Client sent uid="" with identified=true (explicit sign-out) → use empty
        let result = resolve_user_id(&None, "site1", "session1", "", true).await;
        assert_eq!(result, "");
    }

    #[tokio::test]
    async fn test_resolve_user_id_not_identified_no_redis_returns_empty() {
        // Client hasn't called identify(), no Redis available → empty string
        let result = resolve_user_id(&None, "site1", "session1", "", false).await;
        assert_eq!(result, "");
    }

    #[tokio::test]
    async fn test_resolve_user_id_not_identified_with_uid_returns_uid() {
        // Edge case: uid present but identified=false (shouldn't happen, but be safe)
        let result = resolve_user_id(&None, "site1", "session1", "user-abc", false).await;
        assert_eq!(result, "user-abc");
    }
}
