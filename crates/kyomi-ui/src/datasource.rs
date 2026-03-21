// SPDX-License-Identifier: AGPL-3.0-or-later

//! Kyomi datasource implementation for chartml-rs.
//!
//! Implements the `DataSource` trait from chartml-core, calling the
//! `query_datasource_arrow` server function to fetch data as Arrow IPC
//! and returning a `DataTable`.

use chartml_core::data::DataTable;
use chartml_core::error::ChartError;
use chartml_core::plugin::data_source::{DataSource, DataSpec, FetchOptions};

use crate::server_fns::datasources::query_datasource_arrow;

/// Kyomi platform datasource for chartml-rs charts.
///
/// Fetches data from kyomi datasources via server functions, receiving
/// Arrow IPC bytes that are deserialized into a `DataTable` with full
/// type fidelity (timestamps, dates, decimals preserved).
///
/// On WASM (hydrate), `query_datasource_arrow` makes an HTTP call to
/// the server function endpoint. On SSR, it executes directly.
///
// TODO: kyomi-ui uses arrow v57 for IPC serialisation while chartml-core
// uses arrow v54 (pinned by datafusion v45) for deserialisation. Arrow IPC
// is a stable wire format so the v54/v57 mismatch is safe for interchange,
// but the versions should be aligned once datafusion upgrades to arrow v57+.
pub struct KyomiDataSource;

#[async_trait::async_trait]
impl DataSource for KyomiDataSource {
    async fn fetch(&self, spec: &DataSpec, _options: &FetchOptions) -> Result<DataTable, ChartError> {
        let datasource_slug = spec
            .endpoint
            .as_ref()
            .ok_or_else(|| ChartError::DataError("DataSpec.endpoint (datasource slug) is required".into()))?;

        let sql = spec
            .url
            .as_ref()
            .ok_or_else(|| ChartError::DataError("DataSpec.url (SQL query) is required".into()))?;

        let result = query_datasource_arrow(datasource_slug.clone(), sql.clone(), None)
            .await
            .map_err(|e| ChartError::DataError(format!("Server function error: {e}")))?;

        // Decode base64 IPC bytes
        use base64::Engine;
        let ipc_bytes = base64::engine::general_purpose::STANDARD
            .decode(&result.ipc_base64)
            .map_err(|e| ChartError::DataError(format!("Base64 decode error: {e}")))?;

        // Deserialize Arrow IPC to DataTable
        DataTable::from_ipc_bytes(&ipc_bytes)
    }
}
