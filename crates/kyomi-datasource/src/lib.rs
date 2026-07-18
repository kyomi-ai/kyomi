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

/// Create a datasource provider from configuration.
pub async fn create_provider(
    ds_type: &kyomi_core::datasource_registry::DatasourceType,
    connection_config: &serde_json::Value,
    credentials: &serde_json::Value,
    user_context: Option<&UserContext>,
) -> kyomi_core::Result<Box<dyn DatasourceProvider>> {
    kyomi_datasource_drivers::create_provider(
        ds_type,
        connection_config,
        credentials,
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

/// Ensure OAuth credentials are valid (refresh if needed).
pub async fn ensure_valid_oauth_credentials(
    credentials: &serde_json::Value,
    connection_config: &serde_json::Value,
    ds_type: &kyomi_core::datasource_registry::DatasourceType,
) -> kyomi_core::Result<serde_json::Value> {
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

