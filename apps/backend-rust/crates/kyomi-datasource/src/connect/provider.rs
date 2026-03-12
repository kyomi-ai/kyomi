// SPDX-License-Identifier: AGPL-3.0-or-later

//! ConnectProvider — DatasourceProvider implementation that routes queries
//! through a Kyomi Connect instance via WebSocket.
//!
//! Instead of connecting directly to a database, ConnectProvider sends commands
//! through the [`ConnectRegistry`] which forwards them over the WebSocket to the
//! customer-deployed Connect binary. The Connect binary executes the command
//! against the local database and returns the result.
//!
//! This enables Kyomi to query databases that are behind firewalls or VPNs
//! without requiring inbound network access.

use std::time::Duration;

use kyomi_core::connect_protocol::{
    CatalogResult, ConnectOp, ConnectRequest, ConnectResponse, ConnectResponseBody, DryRunParams,
    QueryParams,
};
use kyomi_connect_protocol::stream::{QueryStream, QueryStreamEvent};

use crate::provider::{
    DatasourceProvider, DiscoveryResult, DryRunResult, QueryResult,
};

use super::registry::ConnectRegistry;

/// Default timeout for Connect commands (60 seconds — queries can be slow).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// A [`DatasourceProvider`] that routes all operations through a Kyomi Connect
/// instance registered in the [`ConnectRegistry`].
///
/// The provider does not hold a direct database connection. Instead, it
/// serializes each operation as a [`ConnectRequest`], sends it through the
/// registry (which routes it over WebSocket to the Connect binary), and
/// deserializes the [`ConnectResponse`] into the appropriate result type.
pub struct ConnectProvider {
    registry: ConnectRegistry,
    datasource_config_id: String,
    timeout: Duration,
}

impl ConnectProvider {
    /// Create a new ConnectProvider for the given datasource.
    ///
    /// Uses the default timeout of 60 seconds.
    pub fn new(registry: ConnectRegistry, datasource_config_id: String) -> Self {
        Self {
            registry,
            datasource_config_id,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Create a new ConnectProvider with a custom timeout.
    pub fn with_timeout(
        registry: ConnectRegistry,
        datasource_config_id: String,
        timeout: Duration,
    ) -> Self {
        Self {
            registry,
            datasource_config_id,
            timeout,
        }
    }

    /// Discover the datasource's full catalog (schemas, tables, columns) via
    /// the `discover_catalog` command.
    ///
    /// Uses a longer timeout (120s) than normal queries because catalog
    /// discovery can be slow for large databases.
    pub async fn discover_catalog(&self) -> kyomi_core::Result<CatalogResult> {
        let request = Self::build_request(ConnectOp::DiscoverCatalog, None);
        let result = self.send_and_unwrap(request).await.map_err(core_error)?;
        serde_json::from_value::<CatalogResult>(result).map_err(|e| {
            kyomi_core::Error::Internal(format!("Failed to deserialize CatalogResult: {e}"))
        })
    }

    /// Build a [`ConnectRequest`] with a unique ID, the given operation, and
    /// optional parameters.
    fn build_request(op: ConnectOp, params: Option<serde_json::Value>) -> ConnectRequest {
        ConnectRequest {
            id: uuid::Uuid::new_v4().to_string(),
            op,
            params,
            streaming: false,
        }
    }

    /// Send a request through the registry and extract the result value from
    /// the response body.
    ///
    /// Returns `Ok(Value)` for success responses, or maps error responses to
    /// `kyomi_connect_protocol::Error::Internal` (errors from Connect are
    /// database-level errors which are user-facing, not infrastructure failures).
    async fn send_and_unwrap(
        &self,
        request: ConnectRequest,
    ) -> kyomi_connect_protocol::Result<serde_json::Value> {
        let request_id = request.id.clone();

        let response: ConnectResponse = self
            .registry
            .send_command(&self.datasource_config_id, request, self.timeout)
            .await
            .map_err(|e| kyomi_connect_protocol::Error::Internal(e.to_string()))?;

        // Validate response ID matches request ID (defensive check against
        // misrouted responses in cross-replica pub/sub)
        if response.id != request_id {
            return Err(kyomi_connect_protocol::Error::Internal(format!(
                "Connect response ID mismatch: expected '{}', got '{}'",
                request_id, response.id
            )));
        }

        match response.body {
            ConnectResponseBody::Result { result } => Ok(result),
            ConnectResponseBody::Error { error } => {
                // Errors from Connect are database-level (e.g. SQL syntax error,
                // relation not found) — these are user-facing, not infrastructure
                // failures.
                Err(kyomi_connect_protocol::Error::Provider(error))
            }
            // Streaming variants are not expected in this code path — the
            // non-streaming ConnectProvider always receives a single Result/Error.
            // Streaming will be handled by a separate code path in Phase 4.
            _ => Err(kyomi_connect_protocol::Error::Internal(
                "Unexpected streaming response from Connect agent".into(),
            )),
        }
    }
}

/// Convert a [`kyomi_connect_protocol::Error`] into a [`kyomi_core::Error`].
fn core_error(e: kyomi_connect_protocol::Error) -> kyomi_core::Error {
    kyomi_core::Error::from(e)
}

#[async_trait::async_trait]
impl DatasourceProvider for ConnectProvider {
    async fn test_connection(&self) -> kyomi_connect_protocol::Result<bool> {
        let request = Self::build_request(ConnectOp::TestConnection, None);
        let result = self.send_and_unwrap(request).await?;

        serde_json::from_value::<bool>(result).map_err(|e| {
            kyomi_connect_protocol::Error::Internal(format!(
                "Failed to deserialize test_connection result: {e}"
            ))
        })
    }

    async fn execute_query(
        &self,
        sql: &str,
        limit: Option<u32>,
        offset: Option<u32>,
        include_total: bool,
    ) -> kyomi_connect_protocol::Result<QueryResult> {
        let params = QueryParams {
            sql: sql.to_string(),
            limit,
            offset,
            include_total,
        };
        let params_value = serde_json::to_value(&params)?;

        let request = Self::build_request(ConnectOp::ExecuteQuery, Some(params_value));
        let result = self.send_and_unwrap(request).await?;

        serde_json::from_value::<QueryResult>(result).map_err(|e| {
            kyomi_connect_protocol::Error::Internal(format!(
                "Failed to deserialize QueryResult: {e}"
            ))
        })
    }

    async fn execute_query_stream(
        &self,
        sql: &str,
        limit: Option<u32>,
        offset: Option<u32>,
        include_total: bool,
        _chunk_size: Option<u32>,
    ) -> kyomi_connect_protocol::Result<QueryStream> {
        let params = QueryParams {
            sql: sql.to_string(),
            limit,
            offset,
            include_total,
        };
        let params_value = serde_json::to_value(&params)?;

        let mut request = Self::build_request(ConnectOp::ExecuteQuery, Some(params_value));
        request.streaming = true;

        let rx = self
            .registry
            .send_command_streaming(&self.datasource_config_id, request, self.timeout)
            .await
            .map_err(|e| kyomi_connect_protocol::Error::Internal(e.to_string()))?;

        // Wrap the mpsc receiver into a QueryStream that maps ConnectResponseBody
        // variants back to QueryStreamEvent.
        let stream = async_stream(rx);
        Ok(Box::pin(stream))
    }

    async fn dry_run(&self, sql: &str) -> kyomi_connect_protocol::Result<DryRunResult> {
        let params = DryRunParams {
            sql: sql.to_string(),
        };
        let params_value = serde_json::to_value(&params)?;

        let request = Self::build_request(ConnectOp::DryRun, Some(params_value));
        let result = self.send_and_unwrap(request).await?;

        serde_json::from_value::<DryRunResult>(result).map_err(|e| {
            kyomi_connect_protocol::Error::Internal(format!(
                "Failed to deserialize DryRunResult: {e}"
            ))
        })
    }

    // Discovery methods return defaults — catalog discovery for Connect uses
    // the `discover_catalog` command, not these trait methods.

    async fn list_databases(&self) -> DiscoveryResult {
        DiscoveryResult {
            items: vec![],
            error: Some(
                "Database listing is not supported via Connect. Use catalog discovery instead."
                    .into(),
            ),
        }
    }

    async fn list_schemas(&self) -> DiscoveryResult {
        DiscoveryResult {
            items: vec![],
            error: Some(
                "Schema listing is not supported via Connect. Use catalog discovery instead.".into(),
            ),
        }
    }

    async fn close(&self) {
        // No-op — WebSocket lifecycle is managed by the ConnectRegistry.
        // The Connect binary's connection persists across provider instances.
    }
}

// ---------------------------------------------------------------------------
// Stream adapter
// ---------------------------------------------------------------------------

/// Convert an `mpsc::Receiver<ConnectResponse>` into a `QueryStream`.
///
/// Maps each `ConnectResponseBody` variant to the corresponding `QueryStreamEvent`.
/// Handles both streaming variants (StreamHeader, StreamChunk, StreamComplete)
/// and the legacy single-Result response (splits it into Header + Chunk + Complete).
fn async_stream(
    rx: tokio::sync::mpsc::Receiver<ConnectResponse>,
) -> impl futures_util::Stream<Item = kyomi_connect_protocol::Result<QueryStreamEvent>> {
    futures_util::stream::unfold(rx, |mut rx| async move {
        let response = rx.recv().await?;
        let event = map_response_to_event(response);
        Some((event, rx))
    })
}

/// Map a single ConnectResponse to a QueryStreamEvent.
fn map_response_to_event(
    response: ConnectResponse,
) -> kyomi_connect_protocol::Result<QueryStreamEvent> {
    match response.body {
        ConnectResponseBody::StreamHeader {
            columns,
            total_rows,
        } => Ok(QueryStreamEvent::Header {
            columns,
            total_rows,
        }),
        ConnectResponseBody::StreamChunk { rows, chunk_index } => {
            Ok(QueryStreamEvent::Chunk { rows, chunk_index })
        }
        ConnectResponseBody::StreamComplete {
            execution_time_ms,
            bytes_processed,
            total_chunks,
            total_rows_returned,
        } => Ok(QueryStreamEvent::Complete {
            execution_time_ms,
            bytes_processed,
            total_chunks,
            total_rows_returned,
        }),
        ConnectResponseBody::Result { result } => {
            // Legacy single-result response from an agent that doesn't support
            // streaming. This path is unlikely since our agent sends streaming for
            // large queries and buffered for small ones (which use execute_query).
            let _ = result;
            Err(kyomi_connect_protocol::Error::Internal(
                "Unexpected single Result response on streaming channel".into(),
            ))
        }
        ConnectResponseBody::Error { error } => Err(kyomi_connect_protocol::Error::Provider(error)),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use kyomi_core::connect_protocol::ConnectResponseBody;
    use tokio::sync::{mpsc, oneshot};

    use super::super::registry::{CommandPayload, ConnectRegistry, ResponseChannel};

    /// Helper to create a test registry with Redis and register a mock
    /// Connect handler that uses the provided callback to produce responses.
    async fn setup_mock_connect<F>(
        dsid: &str,
        handler_fn: F,
    ) -> (ConnectRegistry, u64, tokio::task::JoinHandle<()>)
    where
        F: FnOnce(ConnectRequest, ResponseChannel) + Send + 'static,
    {
        let config = kyomi_core::Config::test_config();
        let redis = kyomi_core::redis::create_pool(&config.redis_url)
            .await
            .expect("test Redis");
        let registry = ConnectRegistry::new(redis, config.redis_url);

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<CommandPayload>(16);
        let conn_id = registry.register(dsid, cmd_tx).await;

        let handle = tokio::spawn(async move {
            if let Some((request, response_tx)) = cmd_rx.recv().await {
                handler_fn(request, response_tx);
            }
        });

        (registry, conn_id, handle)
    }

    // -----------------------------------------------------------------------
    // test_connection
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_connection_success() {
        let dsid = "ds-provider-tc-ok";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            assert_eq!(request.op, ConnectOp::TestConnection);
            assert!(request.params.is_none());
            let response = ConnectResponse {
                id: request.id,
                body: ConnectResponseBody::Result {
                    result: serde_json::json!(true),
                },
            };
            let _ = response_tx.send(response);
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider.test_connection().await.expect("should succeed");
        assert!(result);

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    #[tokio::test]
    async fn test_connection_returns_false() {
        let dsid = "ds-provider-tc-false";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            let response = ConnectResponse {
                id: request.id,
                body: ConnectResponseBody::Result {
                    result: serde_json::json!(false),
                },
            };
            let _ = response_tx.send(response);
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider.test_connection().await.expect("should succeed");
        assert!(!result);

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    // -----------------------------------------------------------------------
    // execute_query
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn execute_query_serializes_params_and_deserializes_response() {
        let dsid = "ds-provider-eq";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            // Verify the wire format
            assert_eq!(request.op, ConnectOp::ExecuteQuery);
            let params: QueryParams =
                serde_json::from_value(request.params.unwrap()).expect("valid QueryParams");
            assert_eq!(params.sql, "SELECT id, name FROM users");
            assert_eq!(params.limit, Some(50));
            assert_eq!(params.offset, Some(10));
            assert!(params.include_total);

            // Send back a QueryResult
            let query_result = QueryResult {
                status: crate::provider::QueryStatus::Success,
                columns: Some(vec![
                    crate::provider::ColumnInfo {
                        name: "id".into(),
                        col_type: crate::provider::SimpleType::Number,
                    },
                    crate::provider::ColumnInfo {
                        name: "name".into(),
                        col_type: crate::provider::SimpleType::String,
                    },
                ]),
                rows: Some(vec![
                    vec![serde_json::json!(1), serde_json::json!("Alice")],
                    vec![serde_json::json!(2), serde_json::json!("Bob")],
                ]),
                total_rows: Some(100),
                has_more: true,
                bytes_processed: None,
                execution_time_ms: Some(42),
                error: None,
            };

            let response = ConnectResponse {
                id: request.id,
                body: ConnectResponseBody::Result {
                    result: serde_json::to_value(&query_result).unwrap(),
                },
            };
            let _ = response_tx.send(response);
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider
            .execute_query("SELECT id, name FROM users", Some(50), Some(10), true)
            .await
            .expect("should succeed");

        assert_eq!(
            result.status,
            crate::provider::QueryStatus::Success
        );
        assert_eq!(result.columns.as_ref().unwrap().len(), 2);
        assert_eq!(result.columns.as_ref().unwrap()[0].name, "id");
        assert_eq!(result.rows.as_ref().unwrap().len(), 2);
        assert_eq!(result.total_rows, Some(100));
        assert!(result.has_more);
        assert_eq!(result.execution_time_ms, Some(42));

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    // -----------------------------------------------------------------------
    // dry_run
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn dry_run_serializes_params_and_deserializes_response() {
        let dsid = "ds-provider-dr";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            // Verify the wire format
            assert_eq!(request.op, ConnectOp::DryRun);
            let params: DryRunParams =
                serde_json::from_value(request.params.unwrap()).expect("valid DryRunParams");
            assert_eq!(params.sql, "SELECT * FORM users");

            // Send back a DryRunResult (validation failure)
            let dry_run_result = DryRunResult::failure("Syntax error near 'FORM'", Some(1), Some(10));

            let response = ConnectResponse {
                id: request.id,
                body: ConnectResponseBody::Result {
                    result: serde_json::to_value(&dry_run_result).unwrap(),
                },
            };
            let _ = response_tx.send(response);
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider
            .dry_run("SELECT * FORM users")
            .await
            .expect("should succeed");

        assert!(!result.valid);
        assert_eq!(result.message, "Syntax error near 'FORM'");
        assert_eq!(result.line, Some(1));
        assert_eq!(result.column, Some(10));

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    #[tokio::test]
    async fn dry_run_success_response() {
        let dsid = "ds-provider-dr-ok";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            let dry_run_result = DryRunResult::success("Query validated successfully");
            let response = ConnectResponse {
                id: request.id,
                body: ConnectResponseBody::Result {
                    result: serde_json::to_value(&dry_run_result).unwrap(),
                },
            };
            let _ = response_tx.send(response);
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider.dry_run("SELECT 1").await.expect("should succeed");

        assert!(result.valid);
        assert_eq!(result.message, "Query validated successfully");

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    // -----------------------------------------------------------------------
    // discover_catalog
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn discover_catalog_deserializes_full_result() {
        use kyomi_core::connect_protocol::{
            CatalogColumn, CatalogContainer, CatalogResult, CatalogTable,
        };

        let dsid = "ds-provider-dc";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            assert_eq!(request.op, ConnectOp::DiscoverCatalog);
            assert!(request.params.is_none());

            let catalog = CatalogResult {
                containers: vec![CatalogContainer {
                    name: "public".into(),
                    tables: vec![CatalogTable {
                        name: "users".into(),
                        native_type: Some("BASE TABLE".into()),
                        columns: vec![
                            CatalogColumn {
                                name: "id".into(),
                                native_type: "int4".into(),
                                description: Some("Primary key".into()),
                            },
                            CatalogColumn {
                                name: "email".into(),
                                native_type: "varchar(255)".into(),
                                description: None,
                            },
                        ],
                    }],
                }],
            };

            let response = ConnectResponse {
                id: request.id,
                body: ConnectResponseBody::Result {
                    result: serde_json::to_value(&catalog).unwrap(),
                },
            };
            let _ = response_tx.send(response);
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider
            .discover_catalog()
            .await
            .expect("should succeed");

        assert_eq!(result.containers.len(), 1);
        assert_eq!(result.containers[0].name, "public");
        assert_eq!(result.containers[0].tables.len(), 1);
        assert_eq!(result.containers[0].tables[0].name, "users");
        assert_eq!(
            result.containers[0].tables[0].native_type.as_deref(),
            Some("BASE TABLE")
        );
        assert_eq!(result.containers[0].tables[0].columns.len(), 2);
        assert_eq!(result.containers[0].tables[0].columns[0].name, "id");
        assert_eq!(
            result.containers[0].tables[0].columns[0]
                .description
                .as_deref(),
            Some("Primary key")
        );
        assert_eq!(result.containers[0].tables[0].columns[1].name, "email");
        assert!(result.containers[0].tables[0].columns[1].description.is_none());

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    #[tokio::test]
    async fn discover_catalog_empty_result() {
        use kyomi_core::connect_protocol::CatalogResult;

        let dsid = "ds-provider-dc-empty";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            let catalog = CatalogResult {
                containers: vec![],
            };
            let response = ConnectResponse {
                id: request.id,
                body: ConnectResponseBody::Result {
                    result: serde_json::to_value(&catalog).unwrap(),
                },
            };
            let _ = response_tx.send(response);
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider.discover_catalog().await.expect("should succeed");
        assert!(result.containers.is_empty());

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    #[tokio::test]
    async fn discover_catalog_error_propagated() {
        let dsid = "ds-provider-dc-err";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            let response = ConnectResponse {
                id: request.id,
                body: ConnectResponseBody::Error {
                    error: "permission denied for schema information_schema".into(),
                },
            };
            let _ = response_tx.send(response);
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider.discover_catalog().await;

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("permission denied"),
            "expected error message preserved, got: {err_msg}"
        );

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    // -----------------------------------------------------------------------
    // Error response propagation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn error_response_propagated_as_provider_error() {
        let dsid = "ds-provider-err";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            let response = ConnectResponse {
                id: request.id,
                body: ConnectResponseBody::Error {
                    error: "Connection refused: port 5432".into(),
                },
            };
            let _ = response_tx.send(response);
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider.test_connection().await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = format!("{err}");
        assert!(
            err_msg.contains("Connection refused: port 5432"),
            "expected error message preserved, got: {err_msg}"
        );

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    #[tokio::test]
    async fn error_response_on_execute_query() {
        let dsid = "ds-provider-eq-err";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            let response = ConnectResponse {
                id: request.id,
                body: ConnectResponseBody::Error {
                    error: "relation \"nonexistent\" does not exist".into(),
                },
            };
            let _ = response_tx.send(response);
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider
            .execute_query("SELECT * FROM nonexistent", None, None, false)
            .await;

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("nonexistent"),
            "expected table name in error, got: {err_msg}"
        );

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    // -----------------------------------------------------------------------
    // Offline Connect
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn offline_connect_returns_service_unavailable() {
        let config = kyomi_core::Config::test_config();
        let redis = kyomi_core::redis::create_pool(&config.redis_url)
            .await
            .expect("test Redis");
        let registry = ConnectRegistry::new(redis, config.redis_url);

        let provider = ConnectProvider::new(registry, "ds-not-connected".to_string());
        let result = provider.test_connection().await;

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("offline"),
            "expected 'offline' in error, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn offline_connect_on_execute_query() {
        let config = kyomi_core::Config::test_config();
        let redis = kyomi_core::redis::create_pool(&config.redis_url)
            .await
            .expect("test Redis");
        let registry = ConnectRegistry::new(redis, config.redis_url);

        let provider = ConnectProvider::new(registry, "ds-offline-eq".to_string());
        let result = provider
            .execute_query("SELECT 1", None, None, false)
            .await;

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("offline"),
            "expected 'offline' in error, got: {err_msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Custom timeout
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn timeout_propagates_through_provider() {
        let dsid = "ds-provider-timeout";
        // Register a connection but the handler never responds (drops the oneshot)
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |_request, _response_tx| {
            // Intentionally drop response_tx without sending — simulates a slow Connect
        })
        .await;

        let provider = ConnectProvider::with_timeout(
            registry.clone(),
            dsid.to_string(),
            Duration::from_millis(100), // Very short timeout
        );

        let result = provider.test_connection().await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("dropped the response channel"),
            "expected timeout or channel drop error, got: {err_msg}"
        );

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    // -----------------------------------------------------------------------
    // Discovery methods return sensible defaults
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_databases_returns_not_supported() {
        let config = kyomi_core::Config::test_config();
        let redis = kyomi_core::redis::create_pool(&config.redis_url)
            .await
            .expect("test Redis");
        let registry = ConnectRegistry::new(redis, config.redis_url);

        let provider = ConnectProvider::new(registry, "ds-discovery".to_string());
        let result = provider.list_databases().await;

        assert!(result.items.is_empty());
        assert!(result.error.is_some());
        assert!(result.error.unwrap().contains("catalog discovery"));
    }

    #[tokio::test]
    async fn list_schemas_returns_not_supported() {
        let config = kyomi_core::Config::test_config();
        let redis = kyomi_core::redis::create_pool(&config.redis_url)
            .await
            .expect("test Redis");
        let registry = ConnectRegistry::new(redis, config.redis_url);

        let provider = ConnectProvider::new(registry, "ds-discovery-schemas".to_string());
        let result = provider.list_schemas().await;

        assert!(result.items.is_empty());
        assert!(result.error.is_some());
        assert!(result.error.unwrap().contains("catalog discovery"));
    }
}
