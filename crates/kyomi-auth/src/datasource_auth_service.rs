// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource authentication service — auth checks and credential status.
//!
//! Centralizes ALL authentication logic for datasources. All decisions are
//! derived from the `DatasourceTypeRegistry` (no hardcoded type checks).
//!
//! Mirrors Python's `services/auth_service.py::DatasourceAuthService` class,
//! ported as stateless functions following the Rust service pattern.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use kyomi_core::datasource_registry::{self, AuthModeConfig};
use kyomi_core::models::datasource::{UserDatasourceCredential, UserDatasourcePreference};
use serde_json::Value;

use crate::encryption;

// ---------------------------------------------------------------------------
// Result type for credential status
// ---------------------------------------------------------------------------

/// Result of checking a user's credential status for a datasource.
///
/// Returned by [`check_credential_status`].
#[derive(Debug, Clone)]
pub struct CredentialStatusResult {
    /// Authentication method (e.g., `"oauth"`, `"password"`, `"token"`, `"shared"`).
    pub credential_status: String,

    /// Authentication method identifier.
    pub auth_method: String,

    /// OAuth provider name if applicable (e.g., `"google"`, `"snowflake"`).
    pub oauth_provider: Option<String>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Get the active [`AuthModeConfig`] for a datasource based on its connection config.
///
/// Looks up the `auth_mode` field from `connection_config` and returns the
/// matching `AuthModeConfig` from the registry. Falls back to the default
/// auth mode if not specified.
///
/// Returns `Ok(None)` if `ds_type` is not a known datasource type. Returns
/// `Err` if `auth_mode` names a retired mode (KYO-704) — see
/// [`datasource_registry::RETIRED_AUTH_MODES`].
pub fn get_active_auth_mode<'a>(
    ds_type: &str,
    connection_config: &Value,
) -> kyomi_core::Result<Option<&'a AuthModeConfig>> {
    let Some(meta) = datasource_registry::get_metadata_by_str(ds_type) else {
        return Ok(None);
    };
    let config_map = value_to_hashmap(connection_config);
    meta.get_active_auth_mode(&config_map)
}

/// Check if a datasource uses shared/workspace-level authentication.
///
/// Shared auth means:
/// - Users do not provide individual credentials
/// - User's enabled/disabled preference is tracked in `UserDatasourcePreference`
/// - Examples: `service_account`, `kyomi_oauth`, `shared_credentials` mode
///
/// Non-shared (personal) auth means:
/// - Users provide their own credentials
/// - User's enabled/disabled preference is tracked in `UserDatasourceCredential.enabled`
/// - Examples: `password`, `enterprise_oauth`, `token`
///
/// KYO-704: `kyomi_oauth` remains in the legacy-mode-id list below and in
/// [`datasource_registry::DatasourceTypeMetadata::is_shared_auth`]'s own
/// legacy check, deliberately, even though it was retired from BigQuery's
/// `auth_modes`. This function only classifies credential *shape* — a
/// `kyomi_oauth` row was, and still is, a workspace-wide credential in
/// shape, so it stays shared here. Removing it would make a retired-mode
/// row behave like *personal* auth (per-user preference tracking) instead,
/// which is a second wrong answer, not a fix: the actionable error this
/// ticket adds lives in `get_active_auth_mode`'s `Result`, not here.
pub fn is_shared_auth(ds_type: &str, connection_config: &Value) -> bool {
    let meta = datasource_registry::get_metadata_by_str(ds_type);
    let config_map = value_to_hashmap(connection_config);

    if let Some(meta) = meta {
        return meta.is_shared_auth(&config_map);
    }

    // If type is unknown, check explicit shared_credentials flag and legacy modes
    if config_map.get("shared_credentials") == Some(&Value::Bool(true)) {
        return true;
    }
    if let Some(Value::String(mode_id)) = config_map.get("auth_mode") {
        return mode_id == "service_account" || mode_id == "kyomi_oauth";
    }

    false
}

/// Check credential status for a datasource, returning the auth method and validity.
///
/// This is the registry-driven replacement for hardcoded type checks.
///
/// For `oauth_global` modes (BigQuery kyomi_oauth): we cannot check the global
/// OAuth status here since we don't have access to `User.oauth_data` from the
/// credential alone. The route handler must pass user OAuth data separately.
/// In the Rust service, for `oauth_global` we return `"shared"` status, matching
/// the Python behavior for credential_status endpoint (kyomi_oauth is treated as
/// shared for the purposes of the status check — the actual OAuth validation
/// happens at query time).
///
/// For `oauth_per_datasource` modes: checks the encrypted credential for OAuth
/// tokens and their expiry.
///
/// A retired auth mode (KYO-704: `kyomi_oauth`) resolves to a distinct
/// `credential_status` of `"retired_auth_mode"` rather than silently being
/// treated as valid/shared or as an unrecognized-legacy row. This function's
/// signature stays infallible — the named, actionable error for a retired
/// row lives on `DatasourceTypeMetadata::get_active_auth_mode`'s `Err` (see
/// that function's doc); this function surfaces it via a `tracing::warn!`
/// here rather than propagating, so it isn't lost.
///
/// This function has four external call sites, and each reads
/// `"retired_auth_mode"` on its own terms — this function guarantees only
/// that the value is reported distinctly and by name, not that every caller
/// fails closed on it uniformly:
/// - `datasource_service::get_datasource_settings_detail`'s `cred_result`
///   feeds `bigquery_oauth_status`'s catch-all arm, which denies BigQuery
///   scopes for anything but `"valid"`/`"shared"` — so `"retired_auth_mode"`
///   is denied there without needing to know the value's name.
/// - `datasource_service::toggle_datasource_enabled`'s personal-auth branch
///   (the non-shared arm of its `enabled` case) denies the same way. Its
///   shared-auth arm does *not* consult this function for the retired
///   check at all — `is_shared_auth` classifies a retired mode as shared by
///   credential shape (deliberately; see its own doc), so that arm calls
///   `get_active_auth_mode` directly and propagates its `Err`. See that
///   function's doc.
/// - `datasource_service::list_datasources_with_status`'s non-Connect
///   branch reads `"retired_auth_mode"` explicitly to force `can_enable` to
///   `false`. Its `has_credentials` gate alone (`"valid"`/`"shared"` only)
///   is not sufficient for that: `can_enable` also falls back to
///   `user_enabled`, which defaults to `true` when no preference row exists
///   yet, independent of `credential_status` — so the explicit check is
///   load-bearing, not redundant with `has_credentials`.
/// - `onboarding_service`'s `needs_action_for` is the one allowlist (rather
///   than denylist) consumer: `"retired_auth_mode"` had to be (and now is)
///   added to it by name — a future new `credential_status` value is NOT
///   automatically flagged there and needs the same treatment.
pub fn check_credential_status(
    ds_type: &str,
    connection_config: &Value,
    user_credential: Option<&UserDatasourceCredential>,
    encryption_key: &[u8; 32],
) -> CredentialStatusResult {
    let config_map = value_to_hashmap(connection_config);

    // Get active auth mode from registry
    let meta = datasource_registry::get_metadata_by_str(ds_type);
    let active_mode = match meta.map(|m| m.get_active_auth_mode(&config_map)) {
        Some(Ok(mode)) => mode,
        Some(Err(e)) => {
            tracing::warn!(
                ds_type,
                error = %e,
                "retired auth mode encountered while checking credential status"
            );
            return CredentialStatusResult {
                credential_status: "retired_auth_mode".to_string(),
                auth_method: "retired".to_string(),
                oauth_provider: None,
            };
        }
        None => None,
    };

    let Some(mode) = active_mode else {
        // Unknown type or no auth modes — treat as password
        return check_credential_status_legacy(&config_map, user_credential);
    };

    check_credential_status_with_mode(mode, user_credential, &config_map, encryption_key)
}

/// Check if a datasource requires Google OAuth authentication.
///
/// Returns `true` if the active auth mode uses global Google OAuth. As of
/// KYO-704, no BigQuery auth mode does this anymore — `kyomi_oauth` (the
/// only mode that ever set `oauth_global: true`) was retired and removed
/// from the registry, so this now always returns `false`. It is kept (rather
/// than deleted) because the registry-driven check below is still the
/// correct shape for a future `oauth_global` mode, should one ever exist for
/// another provider.
pub fn requires_google_oauth(ds_type: &str, connection_config: &Value) -> bool {
    match get_active_auth_mode(ds_type, connection_config) {
        Ok(Some(mode)) => {
            mode.requires_oauth()
                && mode.oauth_provider.as_deref() == Some("google")
                && mode.oauth_global
        }
        // Ok(None): unknown ds_type or no auth modes. Err: a retired mode
        // (kyomi_oauth) — which must never re-trigger the Google OAuth flow
        // it was removed to stop escalating. KYO-704: this replaces a
        // legacy fallback that defaulted an unset `auth_mode` to
        // "kyomi_oauth" — a null/absent `auth_mode` is now `service_account`
        // (KYO-442), which never requires Google OAuth, so there is nothing
        // left for a fallback to special-case.
        Ok(None) | Err(_) => false,
    }
}

/// Check if a user can access a datasource.
///
/// Combines enabled preference checking with credential validation.
///
/// When `check_credentials` is `false`, skips credential validation and returns
/// `"enabled_no_cred_check"` for any enabled datasource (used by the resolver
/// for performance when only checking enabled/disabled status).
///
/// Returns `(can_access, reason)` where reason is one of:
/// - `"disabled"` — user explicitly disabled the datasource
/// - `"enabled_no_cred_check"` — enabled without credential validation
/// - `"valid_credentials"` — access granted with valid credentials
/// - `"shared_auth"` — access granted via shared auth
/// - `"no_credentials"` — missing credentials
/// - `"expired_credentials"` — OAuth credentials expired
pub fn check_user_access(
    ds_type: &str,
    connection_config: &Value,
    user_credential: Option<&UserDatasourceCredential>,
    user_pref: Option<&UserDatasourcePreference>,
    encryption_key: &[u8; 32],
    check_credentials: bool,
) -> (bool, String) {
    let shared = is_shared_auth(ds_type, connection_config);

    if shared {
        // For shared-auth: check user preference (default to enabled)
        if let Some(pref) = user_pref
            && !pref.enabled
        {
            return (false, "disabled".to_string());
        }

        if !check_credentials {
            return (true, "enabled_no_cred_check".to_string());
        }

        // Check credential status
        let result = check_credential_status(
            ds_type,
            connection_config,
            user_credential,
            encryption_key,
        );

        match result.credential_status.as_str() {
            "shared" => (true, "shared_auth".to_string()),
            "valid" => (true, "valid_credentials".to_string()),
            "expired" => (false, "expired_credentials".to_string()),
            _ => (false, "no_credentials".to_string()),
        }
    } else {
        // For personal-auth: check credential enabled flag
        if let Some(cred) = user_credential
            && !cred.enabled
        {
            return (false, "disabled".to_string());
        }

        if !check_credentials {
            return (true, "enabled_no_cred_check".to_string());
        }

        // Check credential status
        let result = check_credential_status(
            ds_type,
            connection_config,
            user_credential,
            encryption_key,
        );

        match result.credential_status.as_str() {
            "valid" => (true, "valid_credentials".to_string()),
            "expired" => (false, "expired_credentials".to_string()),
            _ => (false, "no_credentials".to_string()),
        }
    }
}

/// Get whether a user has a datasource enabled.
///
/// - For shared-auth: checks `UserDatasourcePreference.enabled` (default `true` if no record)
/// - For personal-auth: checks `UserDatasourceCredential.enabled` (default `true` if exists,
///   `false` if no credential at all)
pub fn get_user_enabled(
    ds_type: &str,
    connection_config: &Value,
    user_credential: Option<&UserDatasourceCredential>,
    user_pref: Option<&UserDatasourcePreference>,
) -> bool {
    let shared = is_shared_auth(ds_type, connection_config);

    if shared {
        // Shared-auth: check preference (default true if no preference record)
        user_pref.is_none_or(|p| p.enabled)
    } else {
        // Personal-auth: check credential enabled (false if no credential)
        user_credential.is_some_and(|c| c.enabled)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check credential status using an [`AuthModeConfig`].
///
/// This is the registry-driven implementation.
fn check_credential_status_with_mode(
    mode: &AuthModeConfig,
    user_credential: Option<&UserDatasourceCredential>,
    config_map: &HashMap<String, Value>,
    encryption_key: &[u8; 32],
) -> CredentialStatusResult {
    let credential_type = mode.credential_type.as_str();

    // Check for shared_credentials flag first (workspace-level shared auth)
    if config_map.get("shared_credentials") == Some(&Value::Bool(true))
        && mode.supports_shared_credentials
    {
        return CredentialStatusResult {
            credential_status: "shared".to_string(),
            auth_method: "shared".to_string(),
            oauth_provider: None,
        };
    }

    match credential_type {
        "none" => CredentialStatusResult {
            credential_status: "valid".to_string(),
            auth_method: "none".to_string(),
            oauth_provider: None,
        },

        "service_account" => CredentialStatusResult {
            credential_status: "shared".to_string(),
            auth_method: "shared".to_string(),
            oauth_provider: None,
        },

        "oauth_global" => {
            // Global OAuth (e.g., BigQuery kyomi_oauth).
            // We cannot check the user's global OAuth data from here — that lives
            // in User.oauth_data, not in UserDatasourceCredential.
            // The route handler checks global OAuth status separately.
            // For the credential_status endpoint, kyomi_oauth is treated as "shared".
            CredentialStatusResult {
                credential_status: "shared".to_string(),
                auth_method: "oauth".to_string(),
                oauth_provider: mode.oauth_provider.clone(),
            }
        }

        "oauth_per_datasource" => {
            // Per-datasource OAuth — check user_cred for OAuth tokens
            let Some(cred) = user_credential else {
                return CredentialStatusResult {
                    credential_status: "missing".to_string(),
                    auth_method: "oauth".to_string(),
                    oauth_provider: mode.oauth_provider.clone(),
                };
            };

            let status = check_oauth_token_status_from_credential(cred, encryption_key);

            CredentialStatusResult {
                credential_status: status,
                auth_method: "oauth".to_string(),
                oauth_provider: mode.oauth_provider.clone(),
            }
        }

        "password" => {
            let status = if user_credential.is_some() {
                "valid"
            } else {
                "missing"
            };
            CredentialStatusResult {
                credential_status: status.to_string(),
                auth_method: "password".to_string(),
                oauth_provider: None,
            }
        }

        "token" => {
            let status = if user_credential.is_some() {
                "valid"
            } else {
                "missing"
            };
            CredentialStatusResult {
                credential_status: status.to_string(),
                auth_method: "token".to_string(),
                oauth_provider: None,
            }
        }

        "keypair" => {
            let status = if user_credential.is_some() {
                "valid"
            } else {
                "missing"
            };
            CredentialStatusResult {
                credential_status: status.to_string(),
                auth_method: "keypair".to_string(),
                oauth_provider: None,
            }
        }

        _ => {
            tracing::warn!("Unknown credential_type: {credential_type}");
            CredentialStatusResult {
                credential_status: "missing".to_string(),
                auth_method: "unknown".to_string(),
                oauth_provider: None,
            }
        }
    }
}

/// Legacy credential status check for datasources without rich auth modes.
fn check_credential_status_legacy(
    config_map: &HashMap<String, Value>,
    user_credential: Option<&UserDatasourceCredential>,
) -> CredentialStatusResult {
    // Check for shared credentials
    if config_map.get("shared_credentials") == Some(&Value::Bool(true)) {
        return CredentialStatusResult {
            credential_status: "shared".to_string(),
            auth_method: "shared".to_string(),
            oauth_provider: None,
        };
    }

    // Default: password auth
    let status = if user_credential.is_some() {
        "valid"
    } else {
        "missing"
    };
    CredentialStatusResult {
        credential_status: status.to_string(),
        auth_method: "password".to_string(),
        oauth_provider: None,
    }
}

/// Check OAuth token status from an encrypted credential.
///
/// Decrypts the credential and checks `oauth_access_token` and `oauth_token_expiry`.
///
/// Returns `"valid"`, `"expired"`, or `"missing"`.
fn check_oauth_token_status_from_credential(
    cred: &UserDatasourceCredential,
    encryption_key: &[u8; 32],
) -> String {
    // Decrypt credentials
    let credentials = match encryption::decrypt_json(&cred.credentials, encryption_key) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("Failed to decrypt credentials for datasource_config_id={}: {e}", cred.datasource_config_id);
            return "missing".to_string();
        }
    };

    let access_token = credentials.get("oauth_access_token").and_then(|v| v.as_str());
    let refresh_token = credentials.get("oauth_refresh_token").and_then(|v| v.as_str());
    let expires_at = credentials.get("oauth_token_expiry").and_then(|v| v.as_str());

    // No access token at all
    let Some(access_token) = access_token else {
        return "missing".to_string();
    };
    if access_token.is_empty() {
        return "missing".to_string();
    }

    // No expiry info — assume valid
    let Some(expires_at_str) = expires_at else {
        return "valid".to_string();
    };
    if expires_at_str.is_empty() {
        return "valid".to_string();
    }

    // Parse the expiry timestamp
    let expiry_dt = parse_token_expiry(expires_at_str);
    let Some(expiry_dt) = expiry_dt else {
        // Could not parse — assume valid
        tracing::warn!("Could not parse oauth_token_expiry: {expires_at_str}");
        return "valid".to_string();
    };

    let now = Utc::now();
    if now >= expiry_dt {
        // Expired — but if we have a refresh token, we can refresh on demand
        if refresh_token.is_some_and(|t| !t.is_empty()) {
            return "valid".to_string();
        }
        return "expired".to_string();
    }

    "valid".to_string()
}

/// Parse an OAuth token expiry string.
///
/// Supports:
/// - ISO 8601 datetime (e.g., `"2025-01-15T10:30:00Z"`)
/// - ISO 8601 with offset (e.g., `"2025-01-15T10:30:00+00:00"`)
fn parse_token_expiry(s: &str) -> Option<DateTime<Utc>> {
    // Try parsing as RFC 3339 / ISO 8601
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }

    // Try with 'Z' replaced for compatibility
    let normalized = if let Some(stripped) = s.strip_suffix('Z') {
        format!("{stripped}+00:00")
    } else {
        s.to_string()
    };

    if let Ok(dt) = DateTime::parse_from_rfc3339(&normalized) {
        return Some(dt.with_timezone(&Utc));
    }

    // Try parsing as a naive datetime and assume UTC
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(dt.and_utc());
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(dt.and_utc());
    }

    None
}

/// Convert a `serde_json::Value` (expected to be an object) to a `HashMap<String, Value>`.
///
/// Returns an empty map if the value is not an object. This bridges the
/// `serde_json::Value` API used by models with the `HashMap<String, Value>`
/// API used by the registry's `get_active_auth_mode` and `is_shared_auth`.
fn value_to_hashmap(value: &Value) -> HashMap<String, Value> {
    match value.as_object() {
        Some(map) => map
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        None => HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- is_shared_auth tests --

    #[test]
    fn shared_auth_bigquery_kyomi_oauth() {
        let config = json!({"auth_mode": "kyomi_oauth"});
        assert!(is_shared_auth("bigquery", &config));
    }

    #[test]
    fn shared_auth_bigquery_service_account() {
        let config = json!({"auth_mode": "service_account"});
        assert!(is_shared_auth("bigquery", &config));
    }

    #[test]
    fn not_shared_auth_bigquery_enterprise_oauth() {
        let config = json!({"auth_mode": "enterprise_oauth"});
        assert!(!is_shared_auth("bigquery", &config));
    }

    #[test]
    fn shared_auth_explicit_flag() {
        let config = json!({"shared_credentials": true});
        assert!(is_shared_auth("postgres", &config));
    }

    #[test]
    fn not_shared_auth_postgres_default() {
        let config = json!({});
        assert!(!is_shared_auth("postgres", &config));
    }

    #[test]
    fn not_shared_auth_clickhouse_default() {
        let config = json!({});
        assert!(!is_shared_auth("clickhouse", &config));
    }

    #[test]
    fn shared_auth_clickhouse_with_flag() {
        let config = json!({"shared_credentials": true});
        assert!(is_shared_auth("clickhouse", &config));
    }

    // -- requires_google_oauth tests --

    #[test]
    fn not_requires_google_oauth_bigquery_retired_kyomi_oauth() {
        // KYO-704: kyomi_oauth is retired — a row still naming it must NOT
        // re-trigger the Google OAuth flow it was removed to stop
        // escalating. This used to assert `true` ("Default BigQuery auth
        // mode is kyomi_oauth"); that default no longer exists.
        let config = json!({"auth_mode": "kyomi_oauth"});
        assert!(!requires_google_oauth("bigquery", &config));
    }

    #[test]
    fn not_requires_google_oauth_bigquery_default() {
        // KYO-442/KYO-704: default BigQuery auth mode is now
        // service_account, which never requires Google OAuth.
        let config = json!({});
        assert!(!requires_google_oauth("bigquery", &config));
    }

    #[test]
    fn not_requires_google_oauth_bigquery_service_account() {
        let config = json!({"auth_mode": "service_account"});
        assert!(!requires_google_oauth("bigquery", &config));
    }

    #[test]
    fn requires_google_oauth_bigquery_enterprise_oauth() {
        // KYO-704 A3: enterprise_oauth is untouched by the kyomi_oauth
        // retirement — it still requires *a* Google OAuth flow, just not
        // the global/kyomi_oauth one (`oauth_global` stays false for it).
        let config = json!({"auth_mode": "enterprise_oauth"});
        assert!(!requires_google_oauth("bigquery", &config));
    }

    #[test]
    fn not_requires_google_oauth_postgres() {
        let config = json!({});
        assert!(!requires_google_oauth("postgres", &config));
    }

    // -- check_credential_status tests --

    fn test_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        key[..16].copy_from_slice(b"test-key-1234567");
        key[16..].copy_from_slice(b"8901234567890123");
        key
    }

    #[test]
    fn credential_status_shared_bigquery_service_account() {
        let key = test_key();
        let config = json!({"auth_mode": "service_account"});
        let result = check_credential_status("bigquery", &config, None, &key);
        assert_eq!(result.credential_status, "shared");
        assert_eq!(result.auth_method, "shared");
    }

    #[test]
    fn credential_status_bigquery_retired_kyomi_oauth() {
        // KYO-704: kyomi_oauth is retired and no longer resolves to an
        // oauth_global AuthModeConfig at all — check_credential_status must
        // report a distinct "retired_auth_mode" status, not the old
        // "shared"/"oauth"/google-provider triple (which implied the row
        // was a normal, working oauth_global credential).
        let key = test_key();
        let config = json!({"auth_mode": "kyomi_oauth"});
        let result = check_credential_status("bigquery", &config, None, &key);
        assert_eq!(result.credential_status, "retired_auth_mode");
        assert_eq!(result.auth_method, "retired");
        assert_eq!(result.oauth_provider, None);
    }

    #[test]
    fn credential_status_bigquery_null_auth_mode_resolves_to_service_account() {
        // KYO-442/KYO-704 acceptance criterion: a null/absent auth_mode
        // resolves to service_account, not the retired kyomi_oauth default.
        let key = test_key();
        let config = json!({});
        let result = check_credential_status("bigquery", &config, None, &key);
        assert_eq!(result.credential_status, "shared");
        assert_eq!(result.auth_method, "shared");
    }

    #[test]
    fn credential_status_missing_password() {
        let key = test_key();
        let config = json!({});
        let result = check_credential_status("postgres", &config, None, &key);
        assert_eq!(result.credential_status, "missing");
        assert_eq!(result.auth_method, "password");
    }

    #[test]
    fn credential_status_shared_with_flag() {
        let key = test_key();
        let config = json!({"shared_credentials": true});
        let result = check_credential_status("postgres", &config, None, &key);
        assert_eq!(result.credential_status, "shared");
    }

    // -- get_user_enabled tests --

    #[test]
    fn user_enabled_shared_auth_no_pref() {
        let config = json!({"auth_mode": "kyomi_oauth"});
        assert!(get_user_enabled("bigquery", &config, None, None));
    }

    #[test]
    fn user_enabled_shared_auth_disabled() {
        let config = json!({"auth_mode": "kyomi_oauth"});
        let pref = make_test_preference(false);
        assert!(!get_user_enabled("bigquery", &config, None, Some(&pref)));
    }

    #[test]
    fn user_enabled_personal_auth_no_cred() {
        let config = json!({});
        assert!(!get_user_enabled("postgres", &config, None, None));
    }

    #[test]
    fn user_enabled_personal_auth_with_cred() {
        let config = json!({});
        let cred = make_test_credential(true);
        assert!(get_user_enabled("postgres", &config, Some(&cred), None));
    }

    #[test]
    fn user_enabled_personal_auth_disabled_cred() {
        let config = json!({});
        let cred = make_test_credential(false);
        assert!(!get_user_enabled("postgres", &config, Some(&cred), None));
    }

    // -- check_user_access tests --

    #[test]
    fn access_shared_auth_default_enabled() {
        let key = test_key();
        let config = json!({"auth_mode": "service_account"});
        let (can_access, reason) =
            check_user_access("bigquery", &config, None, None, &key, true);
        assert!(can_access);
        assert_eq!(reason, "shared_auth");
    }

    #[test]
    fn access_shared_auth_explicitly_disabled() {
        let key = test_key();
        let config = json!({"auth_mode": "service_account"});
        let pref = make_test_preference(false);
        let (can_access, reason) =
            check_user_access("bigquery", &config, None, Some(&pref), &key, true);
        assert!(!can_access);
        assert_eq!(reason, "disabled");
    }

    #[test]
    fn access_shared_auth_skip_credential_check() {
        let key = test_key();
        let config = json!({"auth_mode": "service_account"});
        let (can_access, reason) =
            check_user_access("bigquery", &config, None, None, &key, false);
        assert!(can_access);
        assert_eq!(reason, "enabled_no_cred_check");
    }

    #[test]
    fn access_personal_auth_no_credentials() {
        let key = test_key();
        let config = json!({});
        let (can_access, reason) =
            check_user_access("postgres", &config, None, None, &key, true);
        assert!(!can_access);
        assert_eq!(reason, "no_credentials");
    }

    #[test]
    fn access_personal_auth_disabled_credential() {
        let key = test_key();
        let config = json!({});
        let cred = make_test_credential(false);
        let (can_access, reason) =
            check_user_access("postgres", &config, Some(&cred), None, &key, true);
        assert!(!can_access);
        assert_eq!(reason, "disabled");
    }

    #[test]
    fn access_personal_auth_skip_credential_check() {
        let key = test_key();
        let config = json!({});
        let (can_access, reason) =
            check_user_access("postgres", &config, None, None, &key, false);
        assert!(can_access);
        assert_eq!(reason, "enabled_no_cred_check");
    }

    // -- parse_token_expiry tests --

    #[test]
    fn parse_expiry_rfc3339() {
        let dt = parse_token_expiry("2025-06-15T10:30:00+00:00");
        assert!(dt.is_some());
    }

    #[test]
    fn parse_expiry_z_suffix() {
        let dt = parse_token_expiry("2025-06-15T10:30:00Z");
        assert!(dt.is_some());
    }

    #[test]
    fn parse_expiry_naive() {
        let dt = parse_token_expiry("2025-06-15T10:30:00");
        assert!(dt.is_some());
    }

    #[test]
    fn parse_expiry_invalid() {
        let dt = parse_token_expiry("not-a-date");
        assert!(dt.is_none());
    }

    // -- Test helpers --

    fn make_test_credential(enabled: bool) -> UserDatasourceCredential {
        UserDatasourceCredential {
            id: 1,
            user_id: "user-test".to_string(),
            datasource_config_id: "ds-test".to_string(),
            workspace_id: "ws-test".to_string(),
            credentials: "encrypted-placeholder".to_string(),
            enabled,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn make_test_preference(enabled: bool) -> UserDatasourcePreference {
        UserDatasourcePreference {
            id: 1,
            user_id: "user-test".to_string(),
            datasource_config_id: "ds-test".to_string(),
            enabled,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
}
