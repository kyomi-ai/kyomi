// SPDX-License-Identifier: AGPL-3.0-or-later

//! kyomi-datasource-server — Server-side Connect registry and provider.
//!
//! This crate contains the server-side infrastructure for routing queries
//! through customer-deployed Kyomi Connect instances via WebSocket. It also
//! re-exports all types from `kyomi-datasource-drivers` for backwards
//! compatibility so existing `use kyomi_datasource_server::*` imports work.
//!
//! ## Architecture
//!
//! - **`connect::registry`** — Maps `datasource_config_id` to active WebSocket
//!   connections, with cross-replica routing via Redis pub/sub.
//! - **`connect::provider`** — `DatasourceProvider` implementation that routes
//!   queries through the registry to a Connect instance.

pub mod connect;

pub use connect::provider::ConnectProvider;
pub use connect::registry::ConnectRegistry;

// ---------------------------------------------------------------------------
// Re-exports from kyomi-datasource-drivers for backwards compatibility
// ---------------------------------------------------------------------------

// Type re-exports (no error-type issues — these are pure types)
pub use kyomi_datasource_drivers::{
    UserContext,
    DatasourceProvider, QueryResult, DryRunResult, DiscoveryResult,
    QueryStatus, ColumnInfo, SimpleType,
    provider::extract_string_col_from_batch,
};

// Timeout constants
pub use kyomi_datasource_drivers::{
    DATASOURCE_TIMEOUT_CONNECT, DATASOURCE_TIMEOUT_QUERY,
    DATASOURCE_TIMEOUT_DRY_RUN, OAUTH_REFRESH_TIMEOUT,
};

// Re-export sub-modules for code that accesses them directly
// (e.g., `kyomi_datasource_server::provider::DatasourceProvider`)
pub use kyomi_datasource_drivers::provider;
pub use kyomi_datasource_drivers::factory;
pub use kyomi_datasource_drivers::providers;
pub use kyomi_datasource_drivers::stream;
pub use kyomi_datasource_drivers::oauth_refresh;

// ---------------------------------------------------------------------------
// Wrapper functions that bridge kyomi_connect_protocol::Result → kyomi_core::Result
// ---------------------------------------------------------------------------
// The drivers crate returns kyomi_connect_protocol::Result, but monorepo code
// uses kyomi_core::Result. These thin wrappers provide backward compatibility.

/// Build a shared HTTP client with a proper User-Agent header.
pub fn http_client() -> kyomi_core::Result<reqwest::Client> {
    kyomi_datasource_drivers::http_client().map_err(Into::into)
}

/// Select an explicitly shared password identity before provider auth selection.
/// Snowflake prefers OAuth/key-pair fields and Redshift prefers IAM fields, so
/// retaining personal credentials would let them override the shared identity.
fn supported_shared_credentials(
    ds_type: &kyomi_core::datasource_registry::DatasourceType,
    connection_config: &serde_json::Value,
) -> Option<serde_json::Value> {
    if connection_config.get("shared_credentials") != Some(&serde_json::Value::Bool(true)) {
        return None;
    }
    let config = connection_config
        .as_object()?
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mode = kyomi_core::datasource_registry::get_metadata(ds_type)
        .get_active_auth_mode(&config)
        .ok()
        .flatten()?;
    mode.supports_shared_credentials.then(|| {
        kyomi_datasource_drivers::resolve_shared_credentials(
            connection_config,
            &serde_json::json!({}),
        )
    })
}

/// Create a datasource provider from configuration.
pub async fn create_provider(
    ds_type: &kyomi_core::datasource_registry::DatasourceType,
    connection_config: &serde_json::Value,
    credentials: &serde_json::Value,
    user_context: Option<&UserContext>,
) -> kyomi_core::Result<Box<dyn DatasourceProvider>> {
    let shared = supported_shared_credentials(ds_type, connection_config);
    kyomi_datasource_drivers::create_provider(
        ds_type,
        connection_config,
        shared.as_ref().unwrap_or(credentials),
        user_context,
    )
    .await
    .map_err(Into::into)
}

/// Resolve shared credentials from connection config.
pub fn resolve_shared_credentials(
    connection_config: &serde_json::Value,
    credentials: &serde_json::Value,
) -> serde_json::Value {
    kyomi_datasource_drivers::resolve_shared_credentials(connection_config, credentials)
}

/// Ensure personal OAuth credentials are valid (refresh if needed).
///
/// Explicit shared auth bypasses personal refresh and returns the personal blob
/// unchanged. Shared credentials are selected only when constructing a provider;
/// they must never become refresh output that callers persist to a personal row.
pub async fn ensure_valid_oauth_credentials(
    credentials: &serde_json::Value,
    connection_config: &serde_json::Value,
    ds_type: &kyomi_core::datasource_registry::DatasourceType,
) -> kyomi_core::Result<serde_json::Value> {
    if supported_shared_credentials(ds_type, connection_config).is_some() {
        // This API returns personal credentials and may be persisted by callers.
        // Shared identity is selected only during provider construction.
        return Ok(credentials.clone());
    }
    kyomi_datasource_drivers::ensure_valid_oauth_credentials(
        credentials,
        connection_config,
        ds_type,
    )
    .await
    .map_err(Into::into)
}

/// Create a datasource provider from pre-resolved parts.
///
/// Centralises the Connect-vs-direct branching and connection timeout logic
/// that was previously duplicated in `query_arrow.rs` and
/// `server_fns/datasources.rs`.
///
/// # Parameters
///
/// - `datasource_id` — config ID of the datasource (used by `ConnectProvider`)
/// - `connection_type` — `"connect"` routes through the registry; anything else
///   goes through the driver factory
/// - `connection_config` — full JSON connection config from the datasource record
/// - `datasource_type` — resolved datasource type (e.g. BigQuery, ClickHouse)
/// - `credentials` — already-decrypted credentials JSON (pass `json!({})` if none)
/// - `user_context` — pre-built user context for OAuth-based providers (may be
///   `None` when the auth mode doesn't require it)
/// - `connect_registry` — registry for Connect-type datasource routing; only
///   required when `connection_type == "connect"` — pass `None` to signal that
///   Connect is not available (returns an error if the datasource needs it)
///
/// # Errors
///
/// Returns `kyomi_core::Error` on connection failure or timeout. The caller is
/// responsible for mapping this to the appropriate response type
/// (`StatusCode`, `ServerFnError`, etc.).
pub async fn create_provider_from_parts(
    datasource_id: &str,
    connection_type: &str,
    connection_config: &serde_json::Value,
    datasource_type: kyomi_core::datasource_registry::DatasourceType,
    credentials: serde_json::Value,
    user_context: Option<UserContext>,
    connect_registry: Option<&ConnectRegistry>,
) -> kyomi_core::Result<Box<dyn DatasourceProvider>> {
    if connection_type == "connect" {
        // Server-side configuration issue (registry not wired up), not
        // something the user can act on — keep as `Internal`.
        let registry = connect_registry.ok_or_else(|| {
            kyomi_core::Error::Internal("Connect registry not available".into())
        })?;
        return Ok(Box::new(ConnectProvider::new(
            registry.clone(),
            datasource_id.to_string(),
        )));
    }

    // OAuth refresh failure (e.g. re-authorization required) is
    // user-actionable — remap `Internal` to `DatasourceConnection` so it
    // surfaces prefix-free like the connect/timeout failures below.
    let credentials = ensure_valid_oauth_credentials(
        &credentials,
        connection_config,
        &datasource_type,
    )
    .await
    .map_err(|e| match e {
        kyomi_core::Error::Internal(msg) => kyomi_core::Error::DatasourceConnection(msg),
        other => other,
    })?;

    // Provider-build/connection failures below are user-actionable (bad
    // credentials, unreachable host) — use `DatasourceConnection` so the
    // message reaches the client without an `internal: ` prefix.
    match tokio::time::timeout(
        DATASOURCE_TIMEOUT_CONNECT,
        create_provider(
            &datasource_type,
            connection_config,
            &credentials,
            user_context.as_ref(),
        ),
    )
    .await
    {
        Ok(Ok(p)) => Ok(p),
        Ok(Err(e)) => Err(kyomi_core::Error::DatasourceConnection(format!(
            "failed to connect to datasource: {e}"
        ))),
        Err(_) => Err(kyomi_core::Error::DatasourceConnection(
            "datasource connection timed out".into(),
        )),
    }
}


#[cfg(test)]
mod shared_credentials_tests {
    use super::*;
    use kyomi_core::datasource_registry::DatasourceType;
    use serde_json::json;

    #[tokio::test]
    async fn shared_password_identity_excludes_personal_oauth_keypair_and_iam_fields() {
        let personal = json!({"username": "personal", "password": "personal-password",
            "oauth_access_token": "personal-token", "oauth_refresh_token": "personal-refresh",
            "private_key": "personal-key", "iam": true, "oauth_token_expiry": "2000-01-01T00:00:00Z"});
        for ds_type in [DatasourceType::Snowflake, DatasourceType::Redshift] {
            let config = json!({"auth_mode": "password", "shared_credentials": true,
                "shared_username": "workspace-reader", "shared_password": "workspace-secret"});
            let unchanged = ensure_valid_oauth_credentials(&personal, &config, &ds_type)
                .await
                .unwrap();
            assert_eq!(unchanged, personal);
            assert_eq!(
                supported_shared_credentials(&ds_type, &config).unwrap(),
                json!({"username": "workspace-reader", "password": "workspace-secret"})
            );
            let mut disabled = config;
            disabled["shared_credentials"] = json!(false);
            assert!(supported_shared_credentials(&ds_type, &disabled).is_none());
        }
        let keypair = json!({"auth_mode": "keypair", "shared_credentials": true});
        assert!(supported_shared_credentials(&DatasourceType::Snowflake, &keypair).is_none());
    }
}
