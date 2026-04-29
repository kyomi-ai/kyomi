// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for datasource management.
//!
//! These replace the REST API calls for datasource settings:
//! - `GET /datasources` + `GET /datasources/credential-status` → `list_datasources()`
//! - `GET /datasources/types` → `get_datasource_types()`
//! - `POST /datasources/{id}/toggle` → `toggle_datasource()`
//! - `DELETE /datasources/{id}` → `delete_datasource()`
//! - `POST /datasources` → `create_datasource_modal()`
//! - `PUT /datasources/{id}` → `update_datasource_settings()`
//! - `POST /datasources/{id}/credentials` → `save_datasource_credentials()`
//! - `GET /datasources/{id}/settings` → `get_datasource_settings()`
//! - `POST /datasources/test-connection` → `test_datasource_standalone()`
//! - `POST /datasources/{id}/test` → `test_existing_datasource()`
//! - `POST /datasources/discover` → `discover_datasource_resources()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/datasources.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, workspace_id, IntoServerFnError};

// ─── Types ──────────────────────────────────────────────────────────────────

pub use kyomi_types::DatasourceInfo;

/// A datasource type from the registry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatasourceTypeInfo {
    pub type_id: String,
    pub display_name: String,
}

// ─── Server Functions ───────────────────────────────────────────────────────

/// List all datasources with credential status for the current user.
///
/// Combines the list and credential-status endpoints into a single call.
/// Mirrors `GET /api/v1/datasources` + `GET /api/v1/datasources/credential-status`.
#[server(prefix = "/leptos-api")]
pub async fn list_datasources() -> Result<Vec<DatasourceInfo>, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let encryption_key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    let items = kyomi_auth::datasource_service::list_datasources_with_status(
        &ctx.db,
        ws_id,
        &auth.user_id,
        encryption_key,
    )
    .await
    .into_sfn()?;

    Ok(items)
}

/// Get all registered datasource types.
///
/// Mirrors `GET /api/v1/datasources/types` (simplified for the list view).
#[server(prefix = "/leptos-api")]
pub async fn get_datasource_types() -> Result<Vec<DatasourceTypeInfo>, ServerFnError> {
    let all_meta = kyomi_core::datasource_registry::all_metadata();

    let types: Vec<DatasourceTypeInfo> = all_meta
        .into_iter()
        .map(|(_, meta)| DatasourceTypeInfo {
            type_id: meta.type_id.to_string(),
            display_name: meta.display_name.to_string(),
        })
        .collect();

    Ok(types)
}

/// Toggle a datasource enabled/disabled for the current user.
///
/// Mirrors `POST /api/v1/datasources/{id}/toggle`.
#[server(prefix = "/leptos-api")]
pub async fn toggle_datasource(
    datasource_id: String,
    enabled: bool,
) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let encryption_key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    kyomi_auth::datasource_service::toggle_datasource_enabled(
        &ctx.db,
        &datasource_id,
        ws_id,
        &auth.user_id,
        enabled,
        encryption_key,
    )
    .await
    .into_sfn()
}

/// Delete a datasource (workspace admin only).
///
/// Mirrors `DELETE /api/v1/datasources/{id}`.
#[server(prefix = "/leptos-api")]
pub async fn delete_datasource(datasource_id: String) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    require_workspace_admin(&auth)?;

    kyomi_auth::datasource_service::delete_datasource(&ctx.db, &datasource_id, ws_id)
        .await
        .into_sfn()?;

    Ok(())
}

// ─── Helpers (server-only) ──────────────────────────────────────────────────

/// Reject non-workspace-admin users.
#[cfg(feature = "ssr")]
fn require_workspace_admin(
    auth: &kyomi_auth::middleware::AuthUser,
) -> Result<(), ServerFnError> {
    if !auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
    {
        return Err(ServerFnError::new("Workspace admin access required"));
    }
    Ok(())
}

// ─── Modal Server Functions ─────────────────────────────────────────────────

/// Result of creating or saving a datasource.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatasourceResult {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub datasource_type: String,
}

/// Datasource settings loaded for edit modal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatasourceSettingsResult {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub datasource_type: String,
    /// `"direct"` for standard provider connections, `"connect"` for Kyomi
    /// Connect agent datasources. Drives the edit-mode branch that swaps the
    /// connection/auth form for the `ConnectStatusPanel`.
    pub connection_type: String,
    pub connection_config: serde_json::Value,
    pub user_settings: serde_json::Value,
    pub has_oauth: bool,
    pub oauth_email: Option<String>,
    pub has_bigquery_scopes: bool,
    pub needs_bigquery_connect: bool,
    pub auth_mode: Option<String>,
    pub service_account_email: Option<String>,
    pub shared_credentials: bool,
    pub credential_status: String,
    pub has_username: bool,
    pub has_password: bool,
}

/// Result of executing a SQL query and converting to Arrow IPC format.
///
/// The IPC bytes are base64-encoded for transport over the Leptos server
/// function boundary. The browser decodes and loads them into Arrow.js
/// for zero-copy chartml-rs rendering.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryArrowResult {
    /// Base64-encoded Arrow IPC stream bytes containing the RecordBatch.
    pub ipc_base64: String,
    /// Number of rows in the result.
    pub num_rows: usize,
    /// Execution time in milliseconds.
    pub execution_time_ms: Option<i64>,
}

/// Connection test result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestConnectionResult {
    pub success: bool,
    pub message: String,
}

/// Discover resources result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoverResourcesResult {
    pub success: bool,
    pub resources: std::collections::HashMap<String, Vec<String>>,
    pub message: String,
}

/// Create a new datasource (admin only).
///
/// Mirrors `POST /api/v1/datasources` + optionally `POST /api/v1/datasources/{id}/credentials`.
#[server(prefix = "/leptos-api")]
pub async fn create_datasource_modal(
    name: String,
    slug: String,
    datasource_type: String,
    connection_config: serde_json::Value,
    credentials: serde_json::Value,
) -> Result<DatasourceResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    require_workspace_admin(&auth)?;

    let slug_opt = if slug.is_empty() { None } else { Some(slug.as_str()) };

    let ds = kyomi_auth::datasource_service::create_datasource(
        &ctx.db,
        ws_id,
        &name,
        slug_opt,
        &datasource_type,
        connection_config,
        Some("direct"),
    )
    .await
    .into_sfn()?;

    // Save credentials if provided
    let has_creds = credentials.as_object().map(|o| !o.is_empty()).unwrap_or(false);
    if has_creds {
        let encryption_key = ctx
            .encryption_key
            .as_deref()
            .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

        kyomi_auth::datasource_service::save_user_credential(
            &ctx.db,
            encryption_key,
            &auth.user_id,
            &ds.id,
            ws_id,
            &credentials,
        )
        .await
        .into_sfn()?;
    }

    // Kick off catalog indexing in the background so tables show up
    // without waiting for the hourly scheduler tick. Fire-and-forget —
    // failures are logged and picked up on the next scheduled refresh.
    // Credential resolution (dedicated → shared → workspace owner) is
    // handled inside spawn_post_create.
    if let Some(encryption_key) = ctx.encryption_key.clone() {
        kyomi_agent::catalog::indexing_service::CatalogIndexingService::spawn_post_create(
            ctx.db.clone(),
            encryption_key,
            ctx.embedding.clone(),
            ws_id.to_string(),
            ds.id.clone(),
        );
    } else {
        tracing::warn!(
            workspace_id = %ws_id,
            datasource_id = %ds.id,
            "Encryption key not configured — skipping initial catalog index"
        );
    }

    Ok(DatasourceResult {
        id: ds.id,
        slug: ds.slug,
        name: ds.name,
        datasource_type: ds.datasource_type.to_string(),
    })
}

/// Update an existing datasource's connection config and name (admin only).
///
/// Mirrors `PUT /api/v1/datasources/{id}`.
#[server(prefix = "/leptos-api")]
pub async fn update_datasource_settings(
    datasource_id: String,
    name: String,
    slug: String,
    connection_config: serde_json::Value,
) -> Result<DatasourceResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    require_workspace_admin(&auth)?;

    let slug_opt = if slug.is_empty() { None } else { Some(slug.as_str()) };
    let name_opt = if name.is_empty() { None } else { Some(name.as_str()) };

    let updated = kyomi_auth::datasource_service::update_datasource(
        &ctx.db,
        &datasource_id,
        ws_id,
        name_opt,
        slug_opt,
        Some(connection_config),
        None,
        None,
    )
    .await
    .into_sfn()?;

    Ok(DatasourceResult {
        id: updated.id,
        slug: updated.slug,
        name: updated.name,
        datasource_type: updated.datasource_type.to_string(),
    })
}

/// Save user credentials for an existing datasource.
///
/// Mirrors `POST /api/v1/datasources/{id}/credentials`.
#[server(prefix = "/leptos-api")]
pub async fn save_datasource_credentials(
    datasource_id: String,
    credentials: serde_json::Value,
) -> Result<(), ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let encryption_key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    kyomi_auth::datasource_service::save_user_credential(
        &ctx.db,
        encryption_key,
        &auth.user_id,
        &datasource_id,
        ws_id,
        &credentials,
    )
    .await
    .into_sfn()?;

    Ok(())
}

/// Load full settings for the edit modal.
///
/// Mirrors `GET /api/v1/datasources/{id}/settings`.
#[server(prefix = "/leptos-api")]
pub async fn get_datasource_settings(
    datasource_id: String,
) -> Result<DatasourceSettingsResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let is_admin = auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
        || auth.workspace.is_owner;

    let encryption_key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    let d = kyomi_auth::datasource_service::get_datasource_settings_detail(
        &ctx.db,
        &datasource_id,
        ws_id,
        &auth.user_id,
        is_admin,
        encryption_key,
    )
    .await
    .into_sfn()?;

    Ok(DatasourceSettingsResult {
        id: d.id,
        name: d.name,
        slug: d.slug,
        datasource_type: d.datasource_type,
        connection_type: d.connection_type,
        connection_config: d.connection_config,
        user_settings: d.user_settings,
        has_oauth: d.has_oauth,
        oauth_email: d.oauth_email,
        has_bigquery_scopes: d.has_bigquery_scopes,
        needs_bigquery_connect: d.needs_bigquery_connect,
        auth_mode: d.auth_mode,
        service_account_email: d.service_account_email,
        shared_credentials: d.shared_credentials,
        credential_status: d.credential_status,
        has_username: d.has_username,
        has_password: d.has_password,
    })
}

/// Test a new connection (create mode) without an existing datasource record.
///
/// Mirrors `POST /api/v1/datasources/test-connection`.
#[server(prefix = "/leptos-api")]
pub async fn test_datasource_standalone(
    datasource_type: String,
    connection_config: serde_json::Value,
    credentials: serde_json::Value,
) -> Result<TestConnectionResult, ServerFnError> {
    use std::str::FromStr as _;
    let _auth = extract_auth().await?;

    let ds_type = kyomi_core::datasource_registry::DatasourceType::from_str(&datasource_type)
        .into_sfn()?;

    let provider = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT,
        kyomi_datasource_server::create_provider(
            &ds_type,
            &connection_config,
            &credentials,
            None,
        ),
    )
    .await
    {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            return Ok(TestConnectionResult {
                success: false,
                message: format!("Failed to connect: {e}"),
            });
        }
        Err(_) => {
            return Ok(TestConnectionResult {
                success: false,
                message: "Connection timed out".to_string(),
            });
        }
    };

    let result = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT,
        provider.test_connection(),
    )
    .await
    {
        Ok(Ok(true)) => TestConnectionResult {
            success: true,
            message: "Connection successful".to_string(),
        },
        Ok(Ok(false)) => TestConnectionResult {
            success: false,
            message: "Connection test returned false".to_string(),
        },
        Ok(Err(e)) => TestConnectionResult {
            success: false,
            message: format!("Connection failed: {e}"),
        },
        Err(_) => TestConnectionResult {
            success: false,
            message: "Connection test timed out".to_string(),
        },
    };

    provider.close().await;
    Ok(result)
}

/// Test an existing datasource's connection.
///
/// Mirrors `POST /api/v1/datasources/{id}/test`.
#[server(prefix = "/leptos-api")]
pub async fn test_existing_datasource(
    datasource_id: String,
) -> Result<TestConnectionResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let ds = kyomi_auth::datasource_service::get_datasource(&ctx.db, &datasource_id, ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Datasource not found"))?;

    if !ds.active {
        return Err(ServerFnError::new("Datasource is not active"));
    }

    let encryption_key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    let ds_type: kyomi_core::datasource_registry::DatasourceType = ds.datasource_type.into();

    let user_cred =
        kyomi_auth::datasource_service::get_user_credential(&ctx.db, &auth.user_id, &ds.id)
            .await
            .into_sfn()?;

    let credentials = if let Some(ref cred) = user_cred {
        kyomi_auth::encryption::decrypt_json(&cred.credentials, encryption_key)
            .unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let provider = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT,
        kyomi_datasource_server::create_provider(
            &ds_type,
            &ds.connection_config,
            &credentials,
            None,
        ),
    )
    .await
    {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            return Ok(TestConnectionResult {
                success: false,
                message: format!("Failed to connect: {e}"),
            });
        }
        Err(_) => {
            return Ok(TestConnectionResult {
                success: false,
                message: "Connection timed out".to_string(),
            });
        }
    };

    let result = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT,
        provider.test_connection(),
    )
    .await
    {
        Ok(Ok(true)) => TestConnectionResult {
            success: true,
            message: "Connection successful".to_string(),
        },
        Ok(Ok(false)) => TestConnectionResult {
            success: false,
            message: "Connection test returned false".to_string(),
        },
        Ok(Err(e)) => TestConnectionResult {
            success: false,
            message: format!("Connection failed: {e}"),
        },
        Err(_) => TestConnectionResult {
            success: false,
            message: "Connection test timed out".to_string(),
        },
    };

    provider.close().await;
    Ok(result)
}

/// Discover available resources (databases, schemas, warehouses, etc.) for a datasource.
///
/// Mirrors `POST /api/v1/datasources/discover` from catalog.rs.
/// Uses provider-specific list methods (list_databases, list_schemas, list_warehouses, etc.)
/// matching `discover_all_resources()` in `apps/server/src/routes/catalog.rs`.
#[server(prefix = "/leptos-api")]
pub async fn discover_datasource_resources(
    datasource_type: String,
    connection_config: serde_json::Value,
    credentials: serde_json::Value,
    datasource_slug: Option<String>,
) -> Result<DiscoverResourcesResult, ServerFnError> {
    use std::str::FromStr as _;
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let ds_type = kyomi_core::datasource_registry::DatasourceType::from_str(&datasource_type)
        .into_sfn()?;

    // If slug provided, look up stored credentials (for OAuth datasources)
    let resolved_creds = if let Some(ref slug) = datasource_slug {
        let encryption_key = ctx
            .encryption_key
            .as_deref()
            .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

        match kyomi_auth::datasource_service::get_datasource_by_slug(&ctx.db, slug, ws_id)
            .await
        {
            Ok(Some(ds)) => {
                match kyomi_auth::datasource_service::get_user_credential(
                    &ctx.db,
                    &auth.user_id,
                    &ds.id,
                )
                .await
                {
                    Ok(Some(cred)) => {
                        kyomi_auth::encryption::decrypt_json(
                            &cred.credentials,
                            encryption_key,
                        )
                        .unwrap_or(credentials.clone())
                    }
                    _ => credentials.clone(),
                }
            }
            _ => credentials.clone(),
        }
    } else {
        credentials.clone()
    };

    let provider = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT,
        kyomi_datasource_server::create_provider(
            &ds_type,
            &connection_config,
            &resolved_creds,
            None,
        ),
    )
    .await
    {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            return Ok(DiscoverResourcesResult {
                success: false,
                resources: std::collections::HashMap::new(),
                message: format!("Failed to connect: {e}"),
            });
        }
        Err(_) => {
            return Ok(DiscoverResourcesResult {
                success: false,
                resources: std::collections::HashMap::new(),
                message: "Connection timed out".to_string(),
            });
        }
    };

    // Test connection first
    let connected = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT,
        provider.test_connection(),
    )
    .await
    {
        Ok(Ok(ok)) => ok,
        _ => false,
    };

    if !connected {
        provider.close().await;
        return Ok(DiscoverResourcesResult {
            success: false,
            resources: std::collections::HashMap::new(),
            message: "Connection test failed — check your credentials".to_string(),
        });
    }

    // Discover all resources using the same mapping as catalog.rs `discover_all_resources()`
    let type_str = ds_type.as_str();
    let mut resources_map: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    let discovery_pairs: Vec<(&str, kyomi_datasource_server::DiscoveryResult)> = match type_str {
        "postgres" | "redshift" => {
            let dbs = provider.list_databases().await;
            let schemas = provider.list_schemas().await;
            vec![("databases", dbs), ("schemas", schemas)]
        }
        "mysql" | "clickhouse" => {
            let dbs = provider.list_databases().await;
            vec![("databases", dbs)]
        }
        "snowflake" => {
            let wh = provider.list_warehouses().await;
            let dbs = provider.list_databases().await;
            vec![("warehouses", wh), ("databases", dbs)]
        }
        "databricks" => {
            let catalogs = provider.list_catalogs().await;
            vec![("catalogs", catalogs)]
        }
        "sqlserver" | "synapse" => {
            let dbs = provider.list_databases().await;
            let schemas = provider.list_schemas().await;
            vec![("databases", dbs), ("schemas", schemas)]
        }
        _ => vec![],
    };

    for (key, result) in discovery_pairs {
        if result.error.is_none() {
            resources_map.insert(
                key.to_string(),
                result.items, // items is already Vec<String>
            );
        }
    }

    provider.close().await;

    Ok(DiscoverResourcesResult {
        success: true,
        resources: resources_map,
        message: "Connection successful and resources discovered".to_string(),
    })
}

// ─── Arrow Query ─────────────────────────────────────────────────────────────

/// Execute a SQL query and return results as Arrow IPC bytes (base64-encoded).
///
/// This is the chartml-rs data path — preserves full type fidelity (timestamps,
/// dates, decimals) by converting to Arrow format on the server before sending
/// to the browser, avoiding JSON type loss.
#[server(prefix = "/leptos-api")]
pub async fn query_datasource_arrow(
    datasource_slug: String,
    sql: String,
    limit: Option<i32>,
) -> Result<QueryArrowResult, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let (_ds, provider) = create_query_provider(&ctx, &auth, ws_id, &datasource_slug).await?;

    // Execute query
    let query_limit = limit.map(|l| l.clamp(1, 10_000) as u32);
    let result = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_QUERY,
        provider.execute_query(&sql, query_limit, None, false),
    )
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            provider.close().await;
            return Err(ServerFnError::new(format!("Query failed: {e}")));
        }
        Err(_) => {
            provider.close().await;
            return Err(ServerFnError::new("Query timed out"));
        }
    };

    provider.close().await;

    // Check for query errors
    if result.status == kyomi_datasource_server::QueryStatus::Error {
        return Err(ServerFnError::new(
            result
                .error
                .unwrap_or_else(|| "Query execution failed".to_string()),
        ));
    }

    let execution_time_ms = result.execution_time_ms;

    // Arrow-native path: serialize the RecordBatch directly into IPC bytes.
    // All Connect providers populate record_batch; non-Connect providers do not
    // support this endpoint (they expose JSON-only rows for the SQL editor path).
    let batch = result.record_batch.ok_or_else(|| {
        ServerFnError::new(
            "Provider returned no Arrow RecordBatch — this datasource does not support the Arrow query endpoint".to_string(),
        )
    })?;
    let num_rows = batch.num_rows();
    let ipc_bytes = {
        use arrow_ipc::writer::StreamWriter;
        let mut buf = Vec::new();
        let mut writer = StreamWriter::try_new(&mut buf, &batch.schema())
            .map_err(|e| ServerFnError::new(format!("Arrow writer init failed: {e}")))?;
        writer
            .write(&batch)
            .map_err(|e| ServerFnError::new(format!("Arrow write failed: {e}")))?;
        writer
            .finish()
            .map_err(|e| ServerFnError::new(format!("Arrow writer finish failed: {e}")))?;
        buf
    };

    let ipc_base64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &ipc_bytes,
    );

    Ok(QueryArrowResult {
        ipc_base64,
        num_rows,
        execution_time_ms,
    })
}

/// Convert an Arrow `RecordBatch` into JSON rows for display in the SQL editor.
///
/// This is a display-layer conversion: the SQL editor table component expects
/// `Vec<Vec<serde_json::Value>>` (one inner `Vec` per row), so data that arrived
/// as Arrow must be translated back to JSON for rendering. Each Arrow column type
/// is mapped to the most natural JSON representation:
///
/// - Numeric types → `json!(value)`
/// - Boolean → `json!(value)`
/// - String/Large-string → `json!(value)`
/// - Date32 → `"YYYY-MM-DD"` string
/// - Time64Microsecond → `"HH:MM:SS"` string
/// - TimestampMicrosecond → ISO 8601 string
/// - Decimal128 → decimal string (preserves precision, avoids float rounding)
/// - Null → `Value::Null`
/// - Unknown/unsupported type → `Value::Null`
#[cfg(feature = "ssr")]
pub(crate) fn record_batch_to_json_rows(batch: &arrow_array::RecordBatch) -> Vec<Vec<serde_json::Value>> {
    use arrow_array::Array;
    use arrow_array::cast::AsArray;
    use arrow_schema::DataType;

    let num_rows = batch.num_rows();
    let num_cols = batch.num_columns();
    let mut result = vec![Vec::with_capacity(num_cols); num_rows];

    for col_idx in 0..num_cols {
        let col = batch.column(col_idx);
        let data_type = col.data_type();

        for (row_idx, row) in result.iter_mut().enumerate() {
            let value = if col.is_null(row_idx) {
                serde_json::Value::Null
            } else {
                match data_type {
                    DataType::Float64 => {
                        let v = col.as_primitive::<arrow_array::types::Float64Type>().value(row_idx);
                        serde_json::json!(v)
                    }
                    DataType::Float32 => {
                        let v = col.as_primitive::<arrow_array::types::Float32Type>().value(row_idx);
                        serde_json::json!(v)
                    }
                    DataType::Int64 => {
                        let v = col.as_primitive::<arrow_array::types::Int64Type>().value(row_idx);
                        serde_json::json!(v)
                    }
                    DataType::Int32 => {
                        let v = col.as_primitive::<arrow_array::types::Int32Type>().value(row_idx);
                        serde_json::json!(v)
                    }
                    DataType::Int16 => {
                        let v = col.as_primitive::<arrow_array::types::Int16Type>().value(row_idx);
                        serde_json::json!(v)
                    }
                    DataType::Int8 => {
                        let v = col.as_primitive::<arrow_array::types::Int8Type>().value(row_idx);
                        serde_json::json!(v)
                    }
                    DataType::UInt64 => {
                        let v = col.as_primitive::<arrow_array::types::UInt64Type>().value(row_idx);
                        serde_json::json!(v)
                    }
                    DataType::UInt32 => {
                        let v = col.as_primitive::<arrow_array::types::UInt32Type>().value(row_idx);
                        serde_json::json!(v)
                    }
                    DataType::UInt16 => {
                        let v = col.as_primitive::<arrow_array::types::UInt16Type>().value(row_idx);
                        serde_json::json!(v)
                    }
                    DataType::UInt8 => {
                        let v = col.as_primitive::<arrow_array::types::UInt8Type>().value(row_idx);
                        serde_json::json!(v)
                    }
                    DataType::Boolean => {
                        let v = col.as_boolean().value(row_idx);
                        serde_json::json!(v)
                    }
                    DataType::Utf8 => {
                        let v = col.as_string::<i32>().value(row_idx);
                        serde_json::json!(v)
                    }
                    DataType::LargeUtf8 => {
                        let v = col.as_string::<i64>().value(row_idx);
                        serde_json::json!(v)
                    }
                    DataType::Date32 => {
                        let days = col.as_primitive::<arrow_array::types::Date32Type>().value(row_idx);
                        // Days since Unix epoch (1970-01-01)
                        let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default();
                        let date = epoch.checked_add_signed(chrono::Duration::days(days as i64));
                        match date {
                            Some(d) => serde_json::json!(d.format("%Y-%m-%d").to_string()),
                            None => serde_json::Value::Null,
                        }
                    }
                    DataType::Time64(arrow_schema::TimeUnit::Microsecond) => {
                        let micros = col.as_primitive::<arrow_array::types::Time64MicrosecondType>().value(row_idx);
                        let secs = micros / 1_000_000;
                        let h = secs / 3600;
                        let m = (secs % 3600) / 60;
                        let s = secs % 60;
                        serde_json::json!(format!("{h:02}:{m:02}:{s:02}"))
                    }
                    DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, tz) => {
                        let micros = col.as_primitive::<arrow_array::types::TimestampMicrosecondType>().value(row_idx);
                        match chrono::DateTime::from_timestamp_micros(micros) {
                            Some(d) => {
                                let suffix = if tz.is_some() { "Z" } else { "" };
                                serde_json::json!(format!("{}{suffix}", d.format("%Y-%m-%dT%H:%M:%S%.6f")))
                            }
                            None => serde_json::Value::Null,
                        }
                    }
                    DataType::Decimal128(_precision, scale) => {
                        let raw = col.as_primitive::<arrow_array::types::Decimal128Type>().value(row_idx);
                        // Format as a decimal string to preserve full precision.
                        // `scale` is the number of fractional digits; use it as
                        // the zero-pad width for the fractional part.
                        let sign = if raw < 0 { "-" } else { "" };
                        let abs_raw = raw.unsigned_abs();
                        let s = if *scale <= 0 {
                            let shifted = abs_raw * 10u128.pow((-*scale) as u32);
                            format!("{sign}{shifted}")
                        } else {
                            let scale_u32 = *scale as u32;
                            let divisor = 10u128.pow(scale_u32);
                            let int_part = abs_raw / divisor;
                            let frac_part = abs_raw % divisor;
                            format!("{sign}{int_part}.{frac_part:0>width$}", width = scale_u32 as usize)
                        };
                        serde_json::json!(s)
                    }
                    _ => {
                        // Unsupported type — return null rather than
                        // attempting lossy display formatting
                        serde_json::Value::Null
                    }
                }
            };
            row.push(value);
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Shared helper: create a query provider from a resolved datasource
// ---------------------------------------------------------------------------

/// Resolve a datasource by slug and create a provider ready for query execution.
///
/// This consolidates the common pattern used by `query_datasource_arrow` and the
/// SQL editor server functions. It:
/// 1. Resolves the datasource from slug within the workspace
/// 2. Handles Connect vs direct provider creation
/// 3. Decrypts user credentials and refreshes OAuth tokens
/// 4. Applies connection timeout
///
/// Returns the resolved datasource row alongside the provider so callers can
/// access metadata (e.g., `datasource_type`, `slug`).
#[cfg(feature = "ssr")]
pub(crate) async fn create_query_provider(
    ctx: &super::ServerContext,
    auth: &kyomi_auth::middleware::AuthUser,
    workspace_id: &str,
    datasource_slug: &str,
) -> Result<
    (
        kyomi_core::models::datasource::DatasourceConfig,
        Box<dyn kyomi_datasource_server::DatasourceProvider>,
    ),
    ServerFnError,
> {
    // Resolve datasource by slug (or UUID).
    // `include_inactive = false` enforces the active constraint at the SQL level.
    let ds = kyomi_auth::datasource_service::resolve_datasource(
        &ctx.db,
        datasource_slug,
        workspace_id,
        false,
    )
    .await
    .into_sfn()?;

    let provider: Box<dyn kyomi_datasource_server::DatasourceProvider> =
        if ds.connection_type == "connect" {
            let registry = ctx
                .connect_registry
                .as_ref()
                .ok_or_else(|| ServerFnError::new("Connect registry not available"))?;
            Box::new(kyomi_datasource_server::ConnectProvider::new(
                registry.clone(),
                ds.id.clone(),
            ))
        } else {
            let encryption_key = ctx
                .encryption_key
                .as_deref()
                .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

            let ds_type: kyomi_core::datasource_registry::DatasourceType =
                ds.datasource_type.into();

            let user_cred = kyomi_auth::datasource_service::get_user_credential(
                &ctx.db,
                &auth.user_id,
                &ds.id,
            )
            .await
            .into_sfn()?;

            let credentials = if let Some(ref cred) = user_cred {
                kyomi_auth::encryption::decrypt_json(
                    &cred.credentials,
                    encryption_key,
                )
                .unwrap_or(serde_json::json!({}))
            } else {
                serde_json::json!({})
            };

            let credentials =
                kyomi_datasource_server::ensure_valid_oauth_credentials(
                    &credentials,
                    &ds.connection_config,
                    &ds_type,
                )
                .await
                .into_sfn()?;

            // Build user context for BigQuery OAuth (kyomi_oauth auth mode).
            let user_context = build_user_context(ctx, auth).await?;
            let user_context_ref = user_context.as_ref();

            match tokio::time::timeout(
                kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT,
                kyomi_datasource_server::create_provider(
                    &ds_type,
                    &ds.connection_config,
                    &credentials,
                    user_context_ref,
                ),
            )
            .await
            {
                Ok(Ok(p)) => p,
                Ok(Err(e)) => {
                    return Err(ServerFnError::new(format!(
                        "Failed to connect to datasource: {e}"
                    )));
                }
                Err(_) => {
                    return Err(ServerFnError::new("Connection timed out"));
                }
            }
        };

    Ok((ds, provider))
}

/// Build a `UserContext` for BigQuery provider creation.
///
/// Loads the user's Google OAuth tokens from the DB, refreshes if expired,
/// and returns a `UserContext` with the valid tokens. If the user has no
/// Google OAuth data (e.g., service_account auth mode), `oauth_data` will
/// be `None` — BigQuery will fall back to other auth modes.
#[cfg(feature = "ssr")]
pub(crate) async fn build_user_context(
    ctx: &super::ServerContext,
    auth: &kyomi_auth::middleware::AuthUser,
) -> Result<Option<kyomi_datasource_server::UserContext>, ServerFnError> {
    // Use centralized token resolution: reads DB, checks expiry, refreshes, persists.
    let oauth_data = if let (Some(client_id), Some(client_secret)) = (
        ctx.config.google_oauth_client_id.as_deref(),
        ctx.config.google_oauth_client_secret.as_deref(),
    ) {
        let encryption_key = ctx
            .encryption_key
            .as_ref()
            .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

        match kyomi_auth::google_oauth::ensure_valid_google_token(
            &ctx.db,
            &auth.user_id,
            encryption_key,
            client_id,
            client_secret,
        )
        .await
        {
            Ok(tokens) => {
                let data = kyomi_auth::google_oauth::OAuthData {
                    google_oauth_tokens: Some(tokens),
                    ..Default::default()
                };
                serde_json::to_value(data).ok()
            }
            Err(_) => None,
        }
    } else {
        None
    };

    let workspace_id = auth
        .workspace
        .workspace_id
        .clone()
        .unwrap_or_default();

    Ok(Some(kyomi_datasource_server::UserContext {
        oauth_data,
        user_email: auth.email.clone(),
        workspace_id,
    }))
}
