// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared constants loaded from `shared/constants.toml`.
//!
//! This is the Rust side of the cross-backend single source of truth.
//! The Python backend loads the same file. NEVER hardcode values that
//! belong here — if both backends need it, it goes in the TOML file.

use serde::Deserialize;
use std::path::Path;
use std::sync::OnceLock;

/// Embedded fallback for standalone binary mode (no filesystem access needed).
/// Path: from crates/kyomi-core/src/ → repo root/data/constants.toml
const EMBEDDED_CONSTANTS: &str = include_str!("../../../data/constants.toml");

/// Global singleton — loaded once at startup.
static CONSTANTS: OnceLock<SharedConstants> = OnceLock::new();

/// Load constants from the shared TOML file.
///
/// Searches for the file at the given path, or falls back to common
/// locations relative to the workspace root.
pub fn load(path: &Path) -> crate::Result<&'static SharedConstants> {
    let mut parsed = parse_file(path)?;
    apply_runtime_overrides(&mut parsed);
    CONSTANTS
        .set(parsed)
        .map_err(|_| crate::Error::Internal("constants already loaded".into()))?;
    CONSTANTS
        .get()
        .ok_or_else(|| crate::Error::Internal("constants not initialized".into()))
}

/// Load constants from disk, falling back to the embedded copy for standalone mode.
///
/// Tries [`find_constants_file`] first. If the file is not found on disk,
/// parses the compile-time embedded copy of `shared/constants.toml`.
pub fn load_with_fallback() -> crate::Result<&'static SharedConstants> {
    let content = match find_constants_file() {
        Ok(path) => {
            tracing::info!("Loaded shared constants from {}", path.display());
            std::fs::read_to_string(&path).map_err(|e| {
                crate::Error::Internal(format!(
                    "failed to read shared constants from {}: {e}",
                    path.display()
                ))
            })?
        }
        Err(_) => {
            tracing::info!("constants.toml not found on disk, using embedded fallback");
            EMBEDDED_CONSTANTS.to_string()
        }
    };

    let mut parsed: SharedConstants = toml::from_str(&content)
        .map_err(|e| crate::Error::Internal(format!("failed to parse shared constants: {e}")))?;

    apply_runtime_overrides(&mut parsed);

    CONSTANTS
        .set(parsed)
        .map_err(|_| crate::Error::Internal("constants already loaded".into()))?;
    CONSTANTS
        .get()
        .ok_or_else(|| crate::Error::Internal("constants not initialized".into()))
}

/// Get the loaded constants. Panics if [`load`] hasn't been called.
pub fn get() -> &'static SharedConstants {
    CONSTANTS
        .get()
        .expect("shared constants not loaded — call constants::load() at startup")
}

/// Apply runtime overrides to parsed constants based on environment.
///
/// `constants.toml` is the default configuration for the SaaS deployment.
/// At runtime we adjust settings that depend on the deployment context:
///
/// - **Cookie `Secure` flag**: Inferred from `FRONTEND_URL` scheme. When the
///   frontend is served over plain HTTP (standalone binary, local dev), the
///   browser will reject `Secure` cookies. We detect this and disable the flag.
///   Users who put a TLS reverse proxy in front and set `FRONTEND_URL=https://…`
///   get `Secure` cookies automatically.
///
/// - **CORS origins**: Standalone/self-hosted mode adds `FRONTEND_URL` to the
///   allowed origins list so the app works on any host/port combination.
///
/// - **HSTS**: Disabled when serving over HTTP (no point advertising HSTS
///   on a plain HTTP connection, and it can confuse browsers).
fn apply_runtime_overrides(constants: &mut SharedConstants) {
    let frontend_url = std::env::var("FRONTEND_URL").unwrap_or_default();
    let is_https = frontend_url.starts_with("https://");

    // Cookie Secure flag: match the frontend protocol
    if !is_https && constants.cookies.secure {
        tracing::info!(
            "FRONTEND_URL is not HTTPS — disabling Secure flag on cookies"
        );
        constants.cookies.secure = false;
    }

    // HSTS: only meaningful over HTTPS
    if !is_https {
        constants.security_headers.hsts = String::new();
    }

    // CORS: ensure FRONTEND_URL is in allowed origins for self-hosted deployments
    if !frontend_url.is_empty()
        && !constants.cors.allowed_origins.contains(&frontend_url)
    {
        constants.cors.allowed_origins.push(frontend_url);
    }

    // Version: prefer build-time tag injected by CI over the TOML default
    if let Some(v) = option_env!("KYOMI_VERSION") {
        constants.api.version = v.to_string();
    }
}

fn parse_file(path: &Path) -> crate::Result<SharedConstants> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        crate::Error::Internal(format!(
            "failed to read shared constants from {}: {e}",
            path.display()
        ))
    })?;

    toml::from_str(&content).map_err(|e| {
        crate::Error::Internal(format!("failed to parse shared constants: {e}"))
    })
}

/// Find the constants file by checking common locations.
///
/// Searches in this order:
/// 1. `../../shared/constants.toml` — running from `apps/backend-rust/`
/// 2. `shared/constants.toml` — running from repo root
/// 3. `../../../../shared/constants.toml` — running from a nested crate directory
/// 4. `$SHARED_CONSTANTS_PATH` env var — absolute fallback for CI/tests
///
/// Returns the first path that exists, or an error if none are found.
pub fn find_constants_file() -> crate::Result<std::path::PathBuf> {
    // Try common locations
    let candidates = [
        // Running from apps/backend-rust/
        std::path::PathBuf::from("../../shared/constants.toml"),
        // Running from repo root
        std::path::PathBuf::from("shared/constants.toml"),
        // Running from apps/backend-rust/crates/kyomi-api/
        std::path::PathBuf::from("../../../../shared/constants.toml"),
        // Absolute fallback for tests
        std::path::PathBuf::from(
            std::env::var("SHARED_CONSTANTS_PATH")
                .unwrap_or_default(),
        ),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    Err(crate::Error::Internal(
        "shared/constants.toml not found — set SHARED_CONSTANTS_PATH env var".into(),
    ))
}

// ---------------------------------------------------------------------------
// Typed structs matching the TOML structure
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct SharedConstants {
    pub api: ApiConstants,
    pub analytics: AnalyticsConstants,
    pub jwt: JwtConstants,
    pub cookies: CookieConstants,
    pub cors: CorsConstants,
    pub security_headers: SecurityHeaderConstants,
    pub redis: RedisConstants,
    pub rate_limits: RateLimitConstants,
    pub workspace: WorkspaceConstants,
    pub websocket: WebSocketConstants,
}

/// API-level constants (version, etc.).
#[derive(Debug, Clone, Deserialize)]
pub struct ApiConstants {
    pub version: String,
}

/// Analytics collector configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct AnalyticsConstants {
    pub collector_url: String,
}

/// JWT configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct JwtConstants {
    pub algorithm: String,
    pub access_token_expire_minutes: i64,
    pub refresh_token_expire_days: i64,
    pub password_reset_expire_minutes: i64,
    pub email_verification_expire_hours: i64,
    pub refresh_token_prefix: String,
    pub refresh_token_min_length: usize,
    pub refresh_token_grace_period_seconds: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CookieConstants {
    pub access_token_name: String,
    pub refresh_token_name: String,
    pub httponly: bool,
    pub secure: bool,
    pub samesite: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CorsConstants {
    pub allowed_origins: Vec<String>,
    pub allowed_methods: Vec<String>,
    pub allow_credentials: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SecurityHeaderConstants {
    pub x_frame_options: String,
    pub x_content_type_options: String,
    pub x_xss_protection: String,
    pub hsts: String,
    pub content_security_policy: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConstants {
    pub key_prefixes: RedisKeyPrefixes,
    pub ttls: RedisTtls,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisKeyPrefixes {
    pub websocket_user: String,
    pub oauth_state: String,
    pub pending_signup: String,
    pub pending_terms: String,
    pub totp_setup: String,
    pub webauthn_challenge: String,
    pub recovery_session: String,
    pub rate_limit_ip: String,
    pub rate_limit_user: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisTtls {
    pub oauth_state: u64,
    pub pending_signup: u64,
    pub totp_setup: u64,
    pub webauthn_challenge: u64,
    pub recovery_session: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConstants {
    pub login: RateLimitConfig,
    pub register: RateLimitConfig,
    pub refresh: RateLimitConfig,
    pub api_call: RateLimitConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    pub ip_capacity: u32,
    pub ip_refill_interval: u64,
    pub user_capacity: u32,
    pub user_refill_interval: u64,
    pub window_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceConstants {
    pub roles: WorkspaceRoleConstants,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceRoleConstants {
    pub admin: String,
    pub user: String,
    pub viewer: String,
}

/// Map a stored DB workspace-role token (e.g. `workspace_admin`) to a
/// human-readable display label (e.g. `"Admin"`).
///
/// Any value that doesn't match a known role constant falls back to
/// `"Member"`, mirroring the generic non-admin wording the invitation
/// email uses (note: `kyomi-auth::email_service` matches on a separate
/// UI-level role string, so it shares the outcome, not this mechanism).
pub fn humanize_workspace_role(role: &str) -> &'static str {
    let roles = &get().workspace.roles;
    if role == roles.admin {
        "Admin"
    } else if role == roles.viewer {
        "Viewer"
    } else {
        "Member"
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebSocketConstants {
    pub message_types: std::collections::HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_constants_file() {
        let path = match find_constants_file() {
            Ok(p) => p,
            Err(_) => {
                eprintln!("skipping constants test — shared/constants.toml not found");
                return;
            }
        };
        let constants = parse_file(&path).expect("should parse");

        // Spot-check values
        assert_eq!(
            constants.analytics.collector_url,
            "https://analytics.kyomi.ai"
        );
        assert_eq!(constants.jwt.algorithm, "HS256");
        assert_eq!(constants.jwt.access_token_expire_minutes, 15);
        assert_eq!(constants.jwt.refresh_token_expire_days, 7);
        assert_eq!(constants.cookies.access_token_name, "access_token");
        assert_eq!(constants.cookies.samesite, "strict");
        assert_eq!(constants.cors.allowed_origins.len(), 5);
        assert!(constants.cors.allow_credentials);
        assert_eq!(constants.security_headers.x_frame_options, "DENY");
        assert_eq!(
            constants.security_headers.content_security_policy,
            "default-src 'none'; frame-ancestors 'none'"
        );
        assert_eq!(constants.redis.ttls.oauth_state, 300);
        assert_eq!(constants.rate_limits.login.ip_capacity, 10);
        assert_eq!(constants.workspace.roles.admin, "workspace_admin");
        assert_eq!(constants.workspace.roles.user, "workspace_user");
        assert_eq!(constants.workspace.roles.viewer, "workspace_viewer");
        assert_eq!(
            constants.websocket.message_types.get("chat_stream"),
            Some(&"chat_stream".to_string())
        );
        assert_eq!(constants.websocket.message_types.len(), 23);
    }

    #[test]
    fn humanize_workspace_role_maps_known_roles() {
        // Ensure the global singleton is populated (embedded fallback always
        // succeeds, so this is safe regardless of `find_constants_file()`
        // resolving in this test's working directory or test execution order).
        let _ = load_with_fallback();
        let roles = &get().workspace.roles;

        assert_eq!(humanize_workspace_role(&roles.admin), "Admin");
        assert_eq!(humanize_workspace_role(&roles.user), "Member");
        assert_eq!(humanize_workspace_role(&roles.viewer), "Viewer");
    }

    #[test]
    fn humanize_workspace_role_falls_back_to_member_for_unknown_value() {
        let _ = load_with_fallback();

        assert_eq!(humanize_workspace_role("some_unrecognized_role"), "Member");
    }
}
