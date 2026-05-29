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
use super::{extract_auth, AuthenticatedContext, IntoServerFnError};

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
    let ac = AuthenticatedContext::extract().await?;

    let encryption_key = ac.encryption_key()?;

    let items = kyomi_auth::datasource_service::list_datasources_with_status(
        ac.db(),
        &ac.ws_id,
        &ac.auth.user_id,
        &encryption_key,
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
    let ac = AuthenticatedContext::extract().await?;

    let encryption_key = ac.encryption_key()?;

    kyomi_auth::datasource_service::toggle_datasource_enabled(
        ac.db(),
        &datasource_id,
        &ac.ws_id,
        &ac.auth.user_id,
        enabled,
        &encryption_key,
    )
    .await
    .into_sfn()
}

/// Delete a datasource (workspace admin only).
///
/// Mirrors `DELETE /api/v1/datasources/{id}`.
#[server(prefix = "/leptos-api")]
pub async fn delete_datasource(datasource_id: String) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    require_workspace_admin(&ac.auth)?;

    kyomi_auth::datasource_service::delete_datasource(ac.db(), &datasource_id, &ac.ws_id)
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

/// Catalog statistics for a datasource.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogStatsResult {
    pub table_count: i64,
    pub schema_count: i64,
    pub last_indexed: Option<String>,
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
    let ac = AuthenticatedContext::extract().await?;

    require_workspace_admin(&ac.auth)?;

    let slug_opt = if slug.is_empty() { None } else { Some(slug.as_str()) };

    let ds = kyomi_auth::datasource_service::create_datasource(
        ac.db(),
        &ac.ws_id,
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
        let encryption_key = ac.encryption_key()?;

        kyomi_auth::datasource_service::save_user_credential(
            ac.db(),
            &encryption_key,
            &ac.auth.user_id,
            &ds.id,
            &ac.ws_id,
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
    if let Some(encryption_key) = ac.ctx.encryption_key.clone() {
        kyomi_agent::catalog::indexing_service::CatalogIndexingService::spawn_post_create(
            ac.ctx.db.clone(),
            encryption_key,
            ac.ctx.embedding.clone(),
            ac.ws_id.clone(),
            ds.id.clone(),
            ac.ctx.connect_registry.clone(),
        );
    } else {
        tracing::warn!(
            workspace_id = %ac.ws_id,
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
    let ac = AuthenticatedContext::extract().await?;

    require_workspace_admin(&ac.auth)?;

    let slug_opt = if slug.is_empty() { None } else { Some(slug.as_str()) };
    let name_opt = if name.is_empty() { None } else { Some(name.as_str()) };

    let updated = kyomi_auth::datasource_service::update_datasource(
        ac.db(),
        &datasource_id,
        &ac.ws_id,
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
    let ac = AuthenticatedContext::extract().await?;

    let encryption_key = ac.encryption_key()?;

    kyomi_auth::datasource_service::save_user_credential(
        ac.db(),
        &encryption_key,
        &ac.auth.user_id,
        &datasource_id,
        &ac.ws_id,
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
    let ac = AuthenticatedContext::extract().await?;

    let is_admin = ac
        .auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
        || ac.auth.workspace.is_owner;

    let encryption_key = ac.encryption_key()?;

    let d = kyomi_auth::datasource_service::get_datasource_settings_detail(
        ac.db(),
        &datasource_id,
        &ac.ws_id,
        &ac.auth.user_id,
        is_admin,
        &encryption_key,
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
            tracing::warn!(raw_error = %e, "datasource connection error (sanitized for client)");
            return Ok(TestConnectionResult {
                success: false,
                message: format!("Failed to connect: {}", kyomi_core::sanitize_error(&e.to_string())),
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
        Ok(Err(e)) => {
            tracing::warn!(raw_error = %e, "datasource test_connection error (sanitized for client)");
            TestConnectionResult {
                success: false,
                message: format!("Connection failed: {}", kyomi_core::sanitize_error(&e.to_string())),
            }
        }
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
    let ac = AuthenticatedContext::extract().await?;

    let ds = kyomi_auth::datasource_service::get_datasource(ac.db(), &datasource_id, &ac.ws_id)
        .await
        .into_sfn()?
        .ok_or_else(|| ServerFnError::new("Datasource not found"))?;

    if !ds.active {
        return Err(ServerFnError::new("Datasource is not active"));
    }

    let encryption_key = ac.encryption_key()?;

    let ds_type: kyomi_core::datasource_registry::DatasourceType = ds.datasource_type.into();

    let user_cred =
        kyomi_auth::datasource_service::get_user_credential(ac.db(), &ac.auth.user_id, &ds.id)
            .await
            .into_sfn()?;

    let credentials = if let Some(ref cred) = user_cred {
        kyomi_auth::encryption::decrypt_json(&cred.credentials, &encryption_key)
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
            tracing::warn!(raw_error = %e, "datasource connection error (sanitized for client)");
            return Ok(TestConnectionResult {
                success: false,
                message: format!("Failed to connect: {}", kyomi_core::sanitize_error(&e.to_string())),
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
        Ok(Err(e)) => {
            tracing::warn!(raw_error = %e, "datasource test_connection error (sanitized for client)");
            TestConnectionResult {
                success: false,
                message: format!("Connection failed: {}", kyomi_core::sanitize_error(&e.to_string())),
            }
        }
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
    let ac = AuthenticatedContext::extract().await?;

    let ds_type = kyomi_core::datasource_registry::DatasourceType::from_str(&datasource_type)
        .into_sfn()?;

    // If slug provided, look up stored credentials (for OAuth datasources)
    let resolved_creds = if let Some(ref slug) = datasource_slug {
        let encryption_key = ac.encryption_key()?;

        match kyomi_auth::datasource_service::get_datasource_by_slug(ac.db(), slug, &ac.ws_id)
            .await
        {
            Ok(Some(ds)) => {
                match kyomi_auth::datasource_service::get_user_credential(
                    ac.db(),
                    &ac.auth.user_id,
                    &ds.id,
                )
                .await
                {
                    Ok(Some(cred)) => {
                        kyomi_auth::encryption::decrypt_json(
                            &cred.credentials,
                            &encryption_key,
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
            tracing::warn!(raw_error = %e, "datasource connection error (sanitized for client)");
            return Ok(DiscoverResourcesResult {
                success: false,
                resources: std::collections::HashMap::new(),
                message: format!("Failed to connect: {}", kyomi_core::sanitize_error(&e.to_string())),
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
        "flaredb" => {
            let schemas = provider.list_schemas().await;
            vec![("schemas", schemas)]
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

// ---------------------------------------------------------------------------
// Shared helper: create a query provider from a resolved datasource
// ---------------------------------------------------------------------------

/// Resolve a datasource by slug and create a provider ready for query execution.
///
/// Used by the dry-run and catalog server functions. It:
/// 1. Resolves the datasource from slug within the workspace
/// 2. Decrypts per-user credentials
/// 3. Builds a `UserContext` for OAuth-based providers (e.g. BigQuery)
/// 4. Delegates Connect-vs-direct branching, OAuth refresh, and connection
///    timeout to `kyomi_datasource_server::create_provider_from_parts`
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

    // Decrypt per-user credentials (skipped for Connect-type datasources).
    let credentials = if ds.connection_type != "connect" {
        let encryption_key = ctx
            .encryption_key
            .as_deref()
            .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

        let user_cred = kyomi_auth::datasource_service::get_user_credential(
            &ctx.db,
            &auth.user_id,
            &ds.id,
        )
        .await
        .into_sfn()?;

        if let Some(ref cred) = user_cred {
            kyomi_auth::encryption::decrypt_json(&cred.credentials, encryption_key)
                .unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        }
    } else {
        serde_json::json!({})
    };

    // Build user context for BigQuery OAuth (kyomi_oauth auth mode).
    let user_context = build_user_context(ctx, auth).await?;

    let ds_type: kyomi_core::datasource_registry::DatasourceType = ds.datasource_type.into();
    let provider = kyomi_datasource_server::create_provider_from_parts(
        &ds.id,
        &ds.connection_type,
        &ds.connection_config,
        ds_type,
        credentials,
        user_context,
        ctx.connect_registry.as_ref(),
    )
    .await
    .into_sfn()?;

    Ok((ds, provider))
}

/// Return table count, schema count, and last-indexed timestamp for a datasource.
///
/// Used by the datasource settings page to display catalog health at a glance.
#[server(prefix = "/leptos-api")]
pub async fn get_catalog_stats(
    datasource_id: String,
) -> Result<CatalogStatsResult, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    let is_pg = ac.db().is_postgres();
    let bf = kyomi_core::sql_compat::bool_false(is_pg);

    // Verify the datasource belongs to this workspace.
    let ds_count: i64 = kyomi_core::db_fetch_scalar!(
        ac.db(),
        i64,
        "SELECT COUNT(*) FROM datasource_configs WHERE id = $1 AND workspace_id = $2",
        &datasource_id,
        &ac.ws_id
    )
    .map_err(|e| ServerFnError::new(format!("Failed to verify datasource: {e}")))?;
    if ds_count == 0 {
        return Err(ServerFnError::new("Datasource not found"));
    }

    let table_count: i64 = kyomi_core::db_fetch_scalar!(
        ac.db(),
        i64,
        &format!(
            "SELECT COUNT(*) FROM datasource_table_cache \
             WHERE datasource_config_id = $1 AND is_archived = {bf}"
        ),
        &datasource_id
    )
    .map_err(|e| ServerFnError::new(format!("Failed to count tables: {e}")))?;

    let schema_count: i64 = kyomi_core::db_fetch_scalar!(
        ac.db(),
        i64,
        &format!(
            "SELECT COUNT(DISTINCT dataset_id) FROM datasource_table_cache \
             WHERE datasource_config_id = $1 AND is_archived = {bf}"
        ),
        &datasource_id
    )
    .map_err(|e| ServerFnError::new(format!("Failed to count schemas: {e}")))?;

    #[derive(sqlx::FromRow)]
    struct LastIndexedRow {
        last_catalog_refresh: Option<chrono::DateTime<chrono::Utc>>,
    }
    let row = kyomi_core::db_fetch_optional!(
        ac.db(),
        LastIndexedRow,
        "SELECT last_catalog_refresh FROM datasource_configs \
         WHERE id = $1 AND workspace_id = $2",
        &datasource_id,
        &ac.ws_id
    )
    .into_sfn()?;

    let last_indexed = row
        .and_then(|r| r.last_catalog_refresh)
        .map(|dt| dt.to_rfc3339());

    Ok(CatalogStatsResult {
        table_count,
        schema_count,
        last_indexed,
    })
}

/// Build a `UserContext` for BigQuery provider creation.
///
/// Delegates to `kyomi_auth::google_oauth::build_datasource_user_context`.
/// Returns `None` when Google OAuth is not configured or the user has no
/// tokens — providers fall back to other auth modes automatically.
#[cfg(feature = "ssr")]
pub(crate) async fn build_user_context(
    ctx: &super::ServerContext,
    auth: &kyomi_auth::middleware::AuthUser,
) -> Result<Option<kyomi_datasource_server::UserContext>, ServerFnError> {
    let workspace_id = auth.workspace.workspace_id.clone().unwrap_or_default();

    kyomi_auth::google_oauth::build_datasource_user_context(
        &ctx.db,
        &auth.user_id,
        ctx.encryption_key.as_deref(),
        ctx.config.google_oauth_client_id.as_deref(),
        ctx.config.google_oauth_client_secret.as_deref(),
        auth.email.clone(),
        workspace_id,
    )
    .await
    .into_sfn()
}
