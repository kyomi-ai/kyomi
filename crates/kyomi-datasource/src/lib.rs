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

/// Collect a query stream into a single QueryResult.
pub async fn collect_stream_to_result(
    stream: kyomi_connect_protocol::stream::QueryStream,
) -> kyomi_core::Result<QueryResult> {
    kyomi_datasource_drivers::collect_stream_to_result(stream)
        .await
        .map_err(Into::into)
}

/// Convert a QueryResult into a query stream.
pub fn query_result_to_stream(
    result: QueryResult,
) -> kyomi_core::Result<kyomi_connect_protocol::stream::QueryStream> {
    kyomi_datasource_drivers::query_result_to_stream(result).map_err(Into::into)
}
