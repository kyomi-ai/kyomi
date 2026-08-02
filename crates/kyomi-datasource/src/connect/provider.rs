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

use arrow_ipc::reader::StreamReader as ArrowStreamReader;
use arrow_ipc::writer::StreamWriter as ArrowStreamWriter;
use arrow_select::concat::concat_batches;

use kyomi_connect_protocol::ArrowStreamEvent;
use kyomi_core::connect_protocol::{
    CatalogResult, ConnectOp, ConnectRequest, ConnectResponse, ConnectResponseBody,
    DiscoverCatalogParams, DryRunParams, QueryParams,
};
use kyomi_connect_protocol::stream::QueryFormat;

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
    pub async fn discover_catalog(
        &self,
        params: DiscoverCatalogParams,
    ) -> kyomi_core::Result<CatalogResult> {
        // A default (unscoped) request serializes to `{}`. Send `None` in that
        // case so the wire message is byte-identical to the pre-scope protocol
        // — older agents never see an unexpected params object.
        //
        // `to_value` on this plain Option/Vec/bool struct cannot fail; `.expect`
        // makes that invariant explicit and surfaces a developer error loudly
        // rather than silently degrading to an unscoped discovery.
        let value = serde_json::to_value(&params)
            .expect("DiscoverCatalogParams is always serializable");
        let params_value =
            (!matches!(&value, serde_json::Value::Object(m) if m.is_empty())).then_some(value);
        let request = Self::build_request(ConnectOp::DiscoverCatalog, params_value);
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
        job_id: Option<&str>,
    ) -> kyomi_connect_protocol::Result<QueryResult> {
        // Request Arrow format so the Connect binary sends IPC bytes instead of
        // JSON rows. Falls back gracefully: if the Connect binary is older and
        // doesn't honour the format field it will send JSON streaming or a
        // buffered Result, both of which we still handle below.
        let params = QueryParams {
            sql: sql.to_string(),
            limit,
            offset,
            include_total,
            format: QueryFormat::Arrow,
            job_id: job_id.map(str::to_string),
        };
        let params_value = serde_json::to_value(&params)?;

        let mut request = Self::build_request(ConnectOp::ExecuteQuery, Some(params_value));
        request.streaming = true;

        let mut rx = self
            .registry
            .send_command_streaming(&self.datasource_config_id, request, self.timeout)
            .await
            .map_err(|e| kyomi_connect_protocol::Error::Internal(e.to_string()))?;

        // Peek at the first message to decide the response format.
        let first = rx.recv().await.ok_or_else(|| {
            kyomi_connect_protocol::Error::Internal(
                "Connect channel closed without a response".into(),
            )
        })?;

        match first.body {
            // (a) Buffered: deserialize the single Result directly.
            ConnectResponseBody::Result { result } => {
                serde_json::from_value::<QueryResult>(result).map_err(|e| {
                    kyomi_connect_protocol::Error::Internal(format!(
                        "Failed to deserialize QueryResult: {e}"
                    ))
                })
            }
            ConnectResponseBody::Error { error } => {
                Err(kyomi_connect_protocol::Error::Provider(error))
            }
            // (c) Arrow IPC path: ArrowHeader → ArrowBatch* → ArrowComplete.
            ConnectResponseBody::ArrowHeader {
                columns,
                total_rows,
                schema_ipc: _, // schema is embedded per-batch in each ArrowBatch IPC stream
            } => {
                // Collect all ArrowBatch messages until ArrowComplete signals end of stream.
                let mut all_batches: Vec<arrow_array::RecordBatch> = Vec::new();
                let mut schema: Option<std::sync::Arc<arrow_schema::Schema>> = None;

                let (execution_time_ms, bytes_processed, result_job_id) = loop {
                    let msg = rx.recv().await.ok_or_else(|| {
                        kyomi_connect_protocol::Error::Internal(
                            "Connect channel closed before ArrowComplete".into(),
                        )
                    })?;

                    match msg.body {
                        ConnectResponseBody::ArrowBatch { ipc_bytes, .. } => {
                            let reader =
                                ArrowStreamReader::try_new(std::io::Cursor::new(ipc_bytes), None)
                                    .map_err(|e| {
                                        kyomi_connect_protocol::Error::Internal(format!(
                                            "Failed to create Arrow StreamReader: {e}"
                                        ))
                                    })?;
                            if schema.is_none() {
                                schema = Some(reader.schema());
                            }
                            for batch_result in reader {
                                let batch = batch_result.map_err(|e| {
                                    kyomi_connect_protocol::Error::Internal(format!(
                                        "Failed to decode Arrow RecordBatch: {e}"
                                    ))
                                })?;
                                all_batches.push(batch);
                            }
                        }
                        ConnectResponseBody::ArrowComplete {
                            execution_time_ms,
                            bytes_processed,
                            job_id: complete_job_id,
                            ..
                        } => {
                            break (execution_time_ms, bytes_processed, complete_job_id);
                        }
                        ConnectResponseBody::Error { error } => {
                            return Err(kyomi_connect_protocol::Error::Provider(error));
                        }
                        other => {
                            return Err(kyomi_connect_protocol::Error::Internal(format!(
                                "Unexpected message in Arrow stream: {other:?}"
                            )));
                        }
                    }
                };

                // Merge multiple batches into one using concat_batches.
                let record_batch = if all_batches.is_empty() {
                    None
                } else if all_batches.len() == 1 {
                    all_batches.into_iter().next()
                } else {
                    let batch_schema = schema.ok_or_else(|| {
                        kyomi_connect_protocol::Error::Internal(
                            "Arrow schema missing when concatenating batches".into(),
                        )
                    })?;
                    let merged = concat_batches(&batch_schema, &all_batches).map_err(|e| {
                        kyomi_connect_protocol::Error::Internal(format!(
                            "Failed to concat Arrow batches: {e}"
                        ))
                    })?;
                    Some(merged)
                };

                Ok(QueryResult {
                    status: crate::provider::QueryStatus::Success,
                    columns: Some(columns),
                    total_rows,
                    has_more: false,
                    bytes_processed,
                    execution_time_ms,
                    error: None,
                    record_batch,
                    job_id: result_job_id,
                })
            }
            other => {
                Err(kyomi_connect_protocol::Error::Internal(format!(
                    "Unexpected first response from Connect agent: {other:?}"
                )))
            }
        }
    }

    async fn execute_query_stream_arrow(
        &self,
        sql: &str,
        limit: Option<u32>,
        offset: Option<u32>,
        include_total: bool,
        chunk_size: Option<u32>,
    ) -> kyomi_connect_protocol::Result<kyomi_connect_protocol::ArrowStream> {
        let params = QueryParams {
            sql: sql.to_string(),
            limit,
            offset,
            include_total,
            format: QueryFormat::Arrow,
            job_id: None,
        };
        let params_value = serde_json::to_value(&params)?;

        let mut request = Self::build_request(ConnectOp::ExecuteQuery, Some(params_value));
        request.streaming = true;

        let mut rx = self
            .registry
            .send_command_streaming(&self.datasource_config_id, request, self.timeout)
            .await
            .map_err(|e| kyomi_connect_protocol::Error::Internal(e.to_string()))?;

        // Peek at first message to handle non-Arrow fallback paths.
        let first = rx.recv().await.ok_or_else(|| {
            kyomi_connect_protocol::Error::Internal(
                "Connect channel closed without a response".into(),
            )
        })?;

        // Create an mpsc channel to bridge ConnectResponse messages into ArrowStreamEvents.
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<kyomi_connect_protocol::Result<ArrowStreamEvent>>(64);

        match first.body {
            ConnectResponseBody::Error { error } => {
                return Err(kyomi_connect_protocol::Error::Provider(error));
            }
            // Buffered result path: wrap in a single-batch stream.
            ConnectResponseBody::Result { result } => {
                let query_result =
                    serde_json::from_value::<QueryResult>(result).map_err(|e| {
                        kyomi_connect_protocol::Error::Internal(format!(
                            "Failed to deserialize QueryResult: {e}"
                        ))
                    })?;
                return crate::stream::query_result_to_arrow_stream(query_result);
            }
            // Arrow IPC path: forward each message as an ArrowStreamEvent.
            ConnectResponseBody::ArrowHeader {
                schema_ipc,
                columns,
                total_rows,
            } => {
                let schema_event = Ok(ArrowStreamEvent::Schema {
                    schema_ipc,
                    columns,
                    total_rows,
                });
                // If the channel is already closed, just return an error stream.
                if event_tx.send(schema_event).await.is_err() {
                    return Err(kyomi_connect_protocol::Error::Internal(
                        "Arrow stream consumer closed before schema was sent".into(),
                    ));
                }

                // Spawn a task that forwards the remaining messages into the channel.
                tokio::spawn(async move {
                    let _ = chunk_size; // chunk_size is advisory; the Connect binary controls batch sizing
                    loop {
                        let msg = match rx.recv().await {
                            Some(m) => m,
                            None => {
                                let _ = event_tx
                                    .send(Err(kyomi_connect_protocol::Error::Internal(
                                        "Connect channel closed before ArrowComplete".into(),
                                    )))
                                    .await;
                                break;
                            }
                        };

                        match msg.body {
                            ConnectResponseBody::ArrowBatch {
                                ipc_bytes,
                                chunk_index,
                            } => {
                                // Decode then re-encode each batch as a standalone IPC stream
                                // so downstream readers get self-contained IPC bytes per batch.
                                let event = decode_and_reencode_batch(ipc_bytes, chunk_index);
                                let done = event.is_err();
                                if event_tx.send(event).await.is_err() || done {
                                    break;
                                }
                            }
                            ConnectResponseBody::ArrowComplete {
                                execution_time_ms,
                                bytes_processed,
                                total_chunks,
                                total_rows_returned,
                                ..
                            } => {
                                let _ = event_tx
                                    .send(Ok(ArrowStreamEvent::Complete {
                                        execution_time_ms,
                                        bytes_processed,
                                        total_chunks,
                                        total_rows_returned,
                                    }))
                                    .await;
                                break;
                            }
                            ConnectResponseBody::Error { error } => {
                                let _ = event_tx
                                    .send(Err(kyomi_connect_protocol::Error::Provider(error)))
                                    .await;
                                break;
                            }
                            other => {
                                let _ = event_tx
                                    .send(Err(kyomi_connect_protocol::Error::Internal(format!(
                                        "Unexpected message in Arrow stream: {other:?}"
                                    ))))
                                    .await;
                                break;
                            }
                        }
                    }
                });
            }
            other => {
                return Err(kyomi_connect_protocol::Error::Internal(format!(
                    "Unexpected first response from Connect agent: {other:?}"
                )));
            }
        }

        let stream = futures_util::stream::unfold(event_rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
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
// Arrow IPC re-encode helper
// ---------------------------------------------------------------------------

/// Decode one Arrow IPC stream (schema + one batch) and re-encode the batch as
/// a fresh self-contained IPC stream.
///
/// This ensures the downstream consumer of `execute_query_stream_arrow` receives
/// standalone IPC bytes per batch that include the schema, regardless of how the
/// Connect binary structured its messages.
fn decode_and_reencode_batch(
    ipc_bytes: Vec<u8>,
    chunk_index: u32,
) -> kyomi_connect_protocol::Result<ArrowStreamEvent> {
    let reader = ArrowStreamReader::try_new(std::io::Cursor::new(ipc_bytes), None).map_err(
        |e| {
            kyomi_connect_protocol::Error::Internal(format!(
                "Failed to create Arrow StreamReader: {e}"
            ))
        },
    )?;
    let schema = reader.schema();

    let mut out_buf = Vec::new();
    let mut writer = ArrowStreamWriter::try_new(&mut out_buf, &schema).map_err(|e| {
        kyomi_connect_protocol::Error::Internal(format!(
            "Failed to create Arrow StreamWriter: {e}"
        ))
    })?;

    for batch_result in reader {
        let batch = batch_result.map_err(|e| {
            kyomi_connect_protocol::Error::Internal(format!(
                "Failed to decode Arrow RecordBatch: {e}"
            ))
        })?;
        writer.write(&batch).map_err(|e| {
            kyomi_connect_protocol::Error::Internal(format!(
                "Failed to write Arrow RecordBatch: {e}"
            ))
        })?;
    }

    writer.finish().map_err(|e| {
        kyomi_connect_protocol::Error::Internal(format!("Failed to finish Arrow stream: {e}"))
    })?;

    Ok(ArrowStreamEvent::Batch {
        ipc_bytes: out_buf,
        chunk_index,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use kyomi_core::connect_protocol::ConnectResponseBody;
    use tokio::sync::mpsc;

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
        let redis_url = config.redis_url.unwrap_or_else(|| "redis://localhost:6380".into());
        let redis = kyomi_core::redis::create_pool(&redis_url)
            .await
            .expect("test Redis");
        let registry = ConnectRegistry::new(redis, redis_url);

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

    /// Connect agent sends a single buffered Result for small queries.
    #[tokio::test]
    async fn execute_query_handles_buffered_result() {
        let dsid = "ds-provider-eq-buffered";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            assert_eq!(request.op, ConnectOp::ExecuteQuery);

            // Connect agent sends a single buffered Result (small query path)
            let query_result = QueryResult {
                status: crate::provider::QueryStatus::Success,
                columns: Some(vec![crate::provider::ColumnInfo {
                    name: "id".into(),
                    col_type: crate::provider::SimpleType::Number,
                }]),
                total_rows: Some(1),
                has_more: false,
                bytes_processed: None,
                execution_time_ms: Some(5),
                error: None,
                record_batch: None,
                job_id: None,
            };

            match response_tx {
                ResponseChannel::Stream(tx) => {
                    let _ = tx.try_send(ConnectResponse {
                        id: request.id,
                        body: ConnectResponseBody::Result {
                            result: serde_json::to_value(&query_result).unwrap(),
                        },
                    });
                }
                _ => panic!("expected Stream channel"),
            }
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider
            .execute_query("SELECT 1", None, None, false, None)
            .await
            .expect("should succeed");

        assert_eq!(result.status, crate::provider::QueryStatus::Success);
        assert_eq!(result.execution_time_ms, Some(5));

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
            .discover_catalog(DiscoverCatalogParams::default())
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
        let result = provider.discover_catalog(DiscoverCatalogParams::default()).await.expect("should succeed");
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
        let result = provider.discover_catalog(DiscoverCatalogParams::default()).await;

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
            match response_tx {
                ResponseChannel::Stream(tx) => {
                    let _ = tx.try_send(ConnectResponse {
                        id: request.id,
                        body: ConnectResponseBody::Error {
                            error: "relation \"nonexistent\" does not exist".into(),
                        },
                    });
                }
                _ => panic!("expected Stream channel"),
            }
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider
            .execute_query("SELECT * FROM nonexistent", None, None, false, None)
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
        let redis_url = config.redis_url.unwrap_or_else(|| "redis://localhost:6380".into());
        let redis = kyomi_core::redis::create_pool(&redis_url)
            .await
            .expect("test Redis");
        let registry = ConnectRegistry::new(redis, redis_url);

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
        let redis_url = config.redis_url.unwrap_or_else(|| "redis://localhost:6380".into());
        let redis = kyomi_core::redis::create_pool(&redis_url)
            .await
            .expect("test Redis");
        let registry = ConnectRegistry::new(redis, redis_url);

        let provider = ConnectProvider::new(registry, "ds-offline-eq".to_string());
        let result = provider
            .execute_query("SELECT 1", None, None, false, None)
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
        let redis_url = config.redis_url.unwrap_or_else(|| "redis://localhost:6380".into());
        let redis = kyomi_core::redis::create_pool(&redis_url)
            .await
            .expect("test Redis");
        let registry = ConnectRegistry::new(redis, redis_url);

        let provider = ConnectProvider::new(registry, "ds-discovery".to_string());
        let result = provider.list_databases().await;

        assert!(result.items.is_empty());
        assert!(result.error.is_some());
        assert!(result.error.unwrap().contains("catalog discovery"));
    }

    #[tokio::test]
    async fn list_schemas_returns_not_supported() {
        let config = kyomi_core::Config::test_config();
        let redis_url = config.redis_url.unwrap_or_else(|| "redis://localhost:6380".into());
        let redis = kyomi_core::redis::create_pool(&redis_url)
            .await
            .expect("test Redis");
        let registry = ConnectRegistry::new(redis, redis_url);

        let provider = ConnectProvider::new(registry, "ds-discovery-schemas".to_string());
        let result = provider.list_schemas().await;

        assert!(result.items.is_empty());
        assert!(result.error.is_some());
        assert!(result.error.unwrap().contains("catalog discovery"));
    }

    // -----------------------------------------------------------------------
    // execute_query — Arrow IPC path
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn execute_query_handles_arrow_ipc_response() {
        use arrow_array::{Int64Array, StringArray, RecordBatch};
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;

        let dsid = "ds-provider-eq-arrow";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            assert_eq!(request.op, ConnectOp::ExecuteQuery);
            let params: QueryParams =
                serde_json::from_value(request.params.unwrap()).expect("valid QueryParams");
            assert_eq!(params.format, kyomi_connect_protocol::stream::QueryFormat::Arrow);

            let schema = Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("name", DataType::Utf8, false),
            ]));
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Int64Array::from(vec![1, 2])),
                    Arc::new(StringArray::from(vec!["Alice", "Bob"])),
                ],
            )
            .unwrap();

            let mut ipc_buf = Vec::new();
            {
                let mut writer =
                    arrow_ipc::writer::StreamWriter::try_new(&mut ipc_buf, &schema).unwrap();
                writer.write(&batch).unwrap();
                writer.finish().unwrap();
            }

            let schema_buf = {
                let mut buf = Vec::new();
                let mut w = arrow_ipc::writer::StreamWriter::try_new(&mut buf, &schema).unwrap();
                w.finish().unwrap();
                buf
            };

            let id = request.id;
            match response_tx {
                ResponseChannel::Stream(tx) => {
                    let _ = tx.try_send(ConnectResponse {
                        id: id.clone(),
                        body: ConnectResponseBody::ArrowHeader {
                            schema_ipc: schema_buf,
                            columns: vec![
                                kyomi_connect_protocol::stream::ColumnInfo {
                                    name: "id".into(),
                                    col_type: kyomi_connect_protocol::stream::SimpleType::Number,
                                },
                                kyomi_connect_protocol::stream::ColumnInfo {
                                    name: "name".into(),
                                    col_type: kyomi_connect_protocol::stream::SimpleType::String,
                                },
                            ],
                            total_rows: Some(2),
                        },
                    });
                    let _ = tx.try_send(ConnectResponse {
                        id: id.clone(),
                        body: ConnectResponseBody::ArrowBatch {
                            ipc_bytes: ipc_buf,
                            chunk_index: 0,
                        },
                    });
                    let _ = tx.try_send(ConnectResponse {
                        id,
                        body: ConnectResponseBody::ArrowComplete {
                            execution_time_ms: Some(15),
                            bytes_processed: None,
                            total_chunks: 1,
                            total_rows_returned: 2,
                            job_id: None,
                        },
                    });
                }
                _ => panic!("expected Stream channel"),
            }
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider
            .execute_query("SELECT id, name FROM users", Some(50), None, true, None)
            .await
            .expect("should succeed");

        assert_eq!(result.status, crate::provider::QueryStatus::Success);
        assert!(result.record_batch.is_some());
        let batch = result.record_batch.unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.num_columns(), 2);
        assert_eq!(result.total_rows, Some(2));
        assert_eq!(result.execution_time_ms, Some(15));
        assert_eq!(result.job_id, None);

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    #[tokio::test]
    async fn execute_query_passes_job_id_in_params() {
        let dsid = "ds-provider-eq-job-id";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            assert_eq!(request.op, ConnectOp::ExecuteQuery);
            let params: QueryParams =
                serde_json::from_value(request.params.unwrap()).expect("valid QueryParams");
            // Verify job_id is forwarded in the wire params.
            assert_eq!(params.job_id.as_deref(), Some("bq-job-abc123"));

            match response_tx {
                ResponseChannel::Stream(tx) => {
                    let _ = tx.try_send(ConnectResponse {
                        id: request.id.clone(),
                        body: ConnectResponseBody::ArrowHeader {
                            schema_ipc: {
                                use arrow_schema::{DataType, Field, Schema};
                                use std::sync::Arc;
                                let schema = Arc::new(Schema::new(vec![
                                    Field::new("n", DataType::Int64, false),
                                ]));
                                let mut buf = Vec::new();
                                let mut w = arrow_ipc::writer::StreamWriter::try_new(&mut buf, &schema).unwrap();
                                w.finish().unwrap();
                                buf
                            },
                            columns: vec![],
                            total_rows: None,
                        },
                    });
                    let _ = tx.try_send(ConnectResponse {
                        id: request.id.clone(),
                        body: ConnectResponseBody::ArrowComplete {
                            execution_time_ms: None,
                            bytes_processed: None,
                            total_chunks: 0,
                            total_rows_returned: 0,
                            job_id: Some("bq-job-abc123".into()),
                        },
                    });
                }
                _ => panic!("expected Stream channel"),
            }
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider
            .execute_query("SELECT 1", None, None, false, Some("bq-job-abc123"))
            .await
            .expect("should succeed");

        // job_id from ArrowComplete is propagated back to the caller.
        assert_eq!(result.job_id.as_deref(), Some("bq-job-abc123"));

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }

    #[tokio::test]
    async fn execute_query_merges_multiple_arrow_batches() {
        use arrow_array::{Int64Array, RecordBatch};
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;

        let dsid = "ds-provider-eq-multi-batch";
        let (registry, conn_id, handle) = setup_mock_connect(dsid, |request, response_tx| {
            let schema = Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
            ]));

            let make_ipc = |values: Vec<i64>| {
                let batch = RecordBatch::try_new(
                    schema.clone(),
                    vec![Arc::new(Int64Array::from(values))],
                )
                .unwrap();
                let mut buf = Vec::new();
                let mut w = arrow_ipc::writer::StreamWriter::try_new(&mut buf, &schema).unwrap();
                w.write(&batch).unwrap();
                w.finish().unwrap();
                buf
            };

            let schema_buf = {
                let mut buf = Vec::new();
                let mut w = arrow_ipc::writer::StreamWriter::try_new(&mut buf, &schema).unwrap();
                w.finish().unwrap();
                buf
            };

            let id = request.id.clone();
            match response_tx {
                ResponseChannel::Stream(tx) => {
                    let _ = tx.try_send(ConnectResponse {
                        id: id.clone(),
                        body: ConnectResponseBody::ArrowHeader {
                            schema_ipc: schema_buf,
                            columns: vec![kyomi_connect_protocol::stream::ColumnInfo {
                                name: "id".into(),
                                col_type: kyomi_connect_protocol::stream::SimpleType::Number,
                            }],
                            total_rows: Some(4),
                        },
                    });
                    let _ = tx.try_send(ConnectResponse {
                        id: id.clone(),
                        body: ConnectResponseBody::ArrowBatch {
                            ipc_bytes: make_ipc(vec![1, 2]),
                            chunk_index: 0,
                        },
                    });
                    let _ = tx.try_send(ConnectResponse {
                        id: id.clone(),
                        body: ConnectResponseBody::ArrowBatch {
                            ipc_bytes: make_ipc(vec![3, 4]),
                            chunk_index: 1,
                        },
                    });
                    let _ = tx.try_send(ConnectResponse {
                        id,
                        body: ConnectResponseBody::ArrowComplete {
                            execution_time_ms: Some(20),
                            bytes_processed: None,
                            total_chunks: 2,
                            total_rows_returned: 4,
                            job_id: None,
                        },
                    });
                }
                _ => panic!("expected Stream channel"),
            }
        })
        .await;

        let provider = ConnectProvider::new(registry.clone(), dsid.to_string());
        let result = provider
            .execute_query("SELECT id FROM t", None, None, false, None)
            .await
            .expect("should succeed");

        assert_eq!(result.status, crate::provider::QueryStatus::Success);
        let batch = result.record_batch.expect("should have a record batch");
        // concat_batches merges the two 2-row batches into one 4-row batch.
        assert_eq!(batch.num_rows(), 4);
        assert_eq!(batch.num_columns(), 1);

        handle.await.unwrap();
        registry.unregister(dsid, conn_id).await;
    }
}
