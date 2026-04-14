// SPDX-License-Identifier: AGPL-3.0-or-later

//! Catalog management endpoints for datasource catalog browsing, discovery, and refresh.
//!
//! Wire-compatible with Python's `routers/catalog.py`.
//!
//! ## Endpoints
//!
//! - `POST /discover-catalog` — Discover catalog items (projects, databases, schemas)
//! - `POST /discover` — Discover ALL resources for datasource setup
//! - `GET /{identifier}/catalog/tree` — Hierarchical catalog tree from cache
//! - `GET /{identifier}/catalog/status` — Indexing status and statistics
//! - `GET /{identifier}/schemas` — Live schema list from datasource
//! - `POST /{identifier}/catalog/refresh` — Trigger manual catalog refresh

use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use kyomi_auth::{
    credential_service, datasource_service,
    middleware::AuthUser,
};
use kyomi_core::datasource_registry;
use kyomi_core::enums::WorkspaceRole;
use kyomi_core::models::table_cache::DatasourceTableCache;

use crate::state::AppState;

/// Build the catalog router.
///
/// These routes are nested under `/api/v1/datasources` alongside the existing
/// datasource management routes. Static paths (`/discover-catalog`, `/discover`)
/// go first; dynamic paths (`/{identifier}/...`) follow.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Static paths — MUST come before /{identifier} captures
        .route("/discover-catalog", post(discover_catalog))
        .route("/discover", post(discover_resources))
        // Dynamic path handlers
        .route("/{identifier}/catalog/tree", get(get_catalog_tree))
        .route("/{identifier}/catalog/status", get(get_catalog_status))
        .route("/{identifier}/schemas", get(list_schemas))
        .route(
            "/{identifier}/catalog/refresh",
            post(refresh_catalog),
        )
}

// ===========================================================================
// Helpers (reuse patterns from datasources.rs)
// ===========================================================================

/// Extract workspace_id from user, or return 400.
fn get_workspace_id(user: &AuthUser) -> Result<&str, kyomi_core::Error> {
    user.workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("User not associated with a workspace".into()))
}

/// Check if user is an admin.
fn is_admin(user: &AuthUser) -> bool {
    user.workspace
        .workspace_roles
        .contains(&WorkspaceRole::WorkspaceAdmin)
        || user.workspace.is_owner
}

/// Resolve a datasource by identifier (slug or UUID), returning 404 with available
/// slugs on failure.
async fn resolve_or_404(
    db: &kyomi_core::DbPool,
    identifier: &str,
    workspace_id: &str,
    include_inactive: bool,
) -> Result<kyomi_core::models::datasource::DatasourceConfig, kyomi_core::Error> {
    datasource_service::resolve_datasource(db, identifier, workspace_id, include_inactive).await
}

/// Build a `UserContext` for BigQuery provider creation.
async fn build_user_context(
    state: &AppState,
    user: &AuthUser,
) -> Result<Option<kyomi_datasource_server::UserContext>, kyomi_core::Error> {
    // Use centralized token resolution: reads DB, checks expiry, refreshes, persists.
    // If the user has no Google OAuth data (e.g. service_account auth), this returns
    // Err which we map to None — that's fine, BigQuery will use other auth modes.
    let oauth_data = if let (Some(client_id), Some(client_secret)) = (
        state.config.google_oauth_client_id.as_deref(),
        state.config.google_oauth_client_secret.as_deref(),
    ) {
        match kyomi_auth::google_oauth::ensure_valid_google_token(
            &state.db,
            &user.user_id,
            &state.encryption_key,
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

    Ok(Some(kyomi_datasource_server::UserContext {
        oauth_data,
        user_email: user.email.clone(),
        workspace_id: user
            .workspace
            .workspace_id
            .clone()
            .unwrap_or_default(),
    }))
}

// ===========================================================================
// Request / Response Types
// ===========================================================================

// -- Discover Catalog --

#[derive(Deserialize)]
struct DiscoverCatalogRequest {
    datasource_type: String,
    #[serde(default)]
    connection_config: Value,
    #[serde(default)]
    credentials: Value,
    datasource_slug: Option<String>,
}

#[derive(Serialize)]
struct CatalogItem {
    name: String,
    #[serde(rename = "type")]
    item_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Serialize)]
struct DiscoverCatalogResponse {
    success: bool,
    items: Vec<CatalogItem>,
    item_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

// -- Discover Resources --

#[derive(Deserialize)]
struct DiscoverResourcesRequest {
    datasource_type: String,
    #[serde(default)]
    connection_config: Value,
    #[serde(default)]
    credentials: Value,
    datasource_slug: Option<String>,
}

#[derive(Serialize)]
struct DiscoverResourcesResponse {
    success: bool,
    resources: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

// -- Catalog Tree --

#[derive(Deserialize)]
struct CatalogTreeParams {
    #[serde(default)]
    include_columns: bool,
}

#[derive(Clone, Serialize)]
struct CatalogTreeNode {
    id: String,
    name: String,
    #[serde(rename = "type")]
    node_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<CatalogTreeNode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
}

#[derive(Serialize)]
struct CatalogTreeResponse {
    datasource_id: String,
    datasource_name: String,
    datasource_type: String,
    tree: Vec<CatalogTreeNode>,
    table_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_indexed: Option<String>,
}

// -- Catalog Status --

#[derive(Serialize)]
struct CatalogStatusResponse {
    datasource_id: String,
    datasource_name: String,
    datasource_type: String,
    table_count: usize,
    schema_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_indexed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    indexing_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    indexing_progress: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog_config: Option<Value>,
}

// -- Schema List --

#[derive(Serialize)]
struct SchemaListResponse {
    datasource_id: String,
    datasource_type: String,
    schemas: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

// -- Catalog Refresh --

#[derive(Deserialize, Default)]
struct CatalogRefreshRequest {
    #[serde(default)]
    force: bool,
}

#[derive(Serialize)]
pub(crate) struct CatalogRefreshResponse {
    pub status: String,
    pub message: String,
    pub datasource_id: String,
}

// ===========================================================================
// Discovery helpers — delegate to provider trait methods
// ===========================================================================

/// Call the primary discovery method for a datasource type.
///
/// Matches Python's `meta.discovery_method` registry field:
/// - BigQuery → `list_projects()`
/// - PostgreSQL, Redshift, SQL Server, Synapse → `list_schemas()`
/// - MySQL, ClickHouse, Snowflake → `list_databases()`
/// - Databricks → `list_catalogs()`
async fn discover_primary(
    provider: &dyn kyomi_datasource_server::DatasourceProvider,
    ds_type: &datasource_registry::DatasourceType,
) -> kyomi_datasource_server::DiscoveryResult {
    match ds_type.as_str() {
        "postgres" | "redshift" | "sqlserver" | "synapse" => provider.list_schemas().await,
        "mysql" | "clickhouse" | "snowflake" => provider.list_databases().await,
        "databricks" => provider.list_catalogs().await,
        // BigQuery handled separately via list_projects()
        _ => kyomi_datasource_server::DiscoveryResult {
            items: vec![],
            error: Some(format!(
                "Discovery not available for datasource type '{}'",
                ds_type.as_str()
            )),
        },
    }
}

/// Discover ALL resources for a datasource (for the universal setup flow).
///
/// Matches Python's `hasattr(provider, 'list_xxx')` checks in `discover_resources`.
/// Returns a list of (resource_type, DiscoveryResult) tuples.
async fn discover_all_resources(
    provider: &dyn kyomi_datasource_server::DatasourceProvider,
    ds_type: &datasource_registry::DatasourceType,
) -> Vec<(&'static str, kyomi_datasource_server::DiscoveryResult)> {
    match ds_type.as_str() {
        "postgres" => vec![
            ("databases", provider.list_databases().await),
            ("schemas", provider.list_schemas().await),
        ],
        "redshift" => vec![("schemas", provider.list_schemas().await)],
        "mysql" => vec![("databases", provider.list_databases().await)],
        "clickhouse" => vec![("databases", provider.list_databases().await)],
        "snowflake" => vec![
            ("warehouses", provider.list_warehouses().await),
            ("databases", provider.list_databases().await),
        ],
        "databricks" => vec![("catalogs", provider.list_catalogs().await)],
        "sqlserver" | "synapse" => vec![
            ("databases", provider.list_databases().await),
            ("schemas", provider.list_schemas().await),
        ],
        // BigQuery handled separately
        _ => vec![],
    }
}

// SQL helper functions (escape_sql_literal, escape_sql_identifier,
// get_tables_in_container_sql, get_columns_sql) are imported from
// kyomi_auth::catalog::sql_helpers at the top of this file.

// ===========================================================================
// Endpoint Handlers
// ===========================================================================

// ---------------------------------------------------------------------------
// POST /discover-catalog — Discover catalog items (projects, databases, schemas)
// ---------------------------------------------------------------------------

async fn discover_catalog(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<DiscoverCatalogRequest>,
) -> Result<Json<DiscoverCatalogResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Build credentials — may need to look up OAuth tokens from database
    let mut credentials = request.credentials.clone();
    let connection_config = if request.connection_config.is_null() {
        json!({})
    } else {
        request.connection_config.clone()
    };

    let auth_mode = connection_config
        .get("auth_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_oauth_mode = auth_mode == "oauth" || auth_mode == "enterprise_oauth";
    let has_oauth_token = credentials
        .get("oauth_access_token")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());

    // For OAuth mode, look up credentials from database if datasource_slug provided
    if let Some(ref slug) = request.datasource_slug
        && is_oauth_mode && !has_oauth_token
        && let Ok(datasource) = datasource_service::resolve_datasource(
            &state.db,
            slug,
            workspace_id,
            false,
        )
        .await
    {
            let user_cred = datasource_service::get_user_credential(
                &state.db,
                &user.user_id,
                &datasource.id,
            )
            .await
            .ok()
            .flatten();

            if let Some(cred) = user_cred
                && let Ok(db_creds) =
                    credential_service::decrypt_credentials(&cred.credentials, &state.encryption_key)
            {
                    // Merge OAuth tokens from database
                    if let Some(obj) = db_creds.as_object() {
                        if let Some(cred_obj) = credentials.as_object_mut() {
                            for (k, v) in obj {
                                cred_obj.insert(k.clone(), v.clone());
                            }
                        } else {
                            credentials = db_creds;
                        }
                    }
                    tracing::info!(
                        "[discover-catalog] Loaded OAuth credentials from database for {slug}"
                    );
                }
        }

    // Validate datasource type
    let meta = match datasource_registry::get_metadata_by_str(&request.datasource_type) {
        Some(m) => m,
        None => {
            return Ok(Json(DiscoverCatalogResponse {
                success: false,
                items: vec![],
                item_type: "unknown".into(),
                message: Some(format!(
                    "unsupported datasource type: '{}'",
                    request.datasource_type
                )),
            }));
        }
    };

    let item_type = format!("{}s", meta.catalog_container_label);

    // Create provider and discover
    let ds_type = match datasource_registry::DatasourceType::from_str(&request.datasource_type) {
        Ok(t) => t,
        Err(e) => {
            return Ok(Json(DiscoverCatalogResponse {
                success: false,
                items: vec![],
                item_type,
                message: Some(e.to_string()),
            }));
        }
    };

    let user_context = build_user_context(&state, &user).await?;
    let user_context_ref = user_context.as_ref();

    let provider = match kyomi_datasource_server::create_provider(
        &ds_type,
        &connection_config,
        &credentials,
        user_context_ref,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                datasource_type = %request.datasource_type,
                error = %e,
                "Failed to create provider for catalog discovery"
            );
            return Ok(Json(DiscoverCatalogResponse {
                success: false,
                items: vec![],
                item_type,
                message: Some("Failed to connect to datasource".into()),
            }));
        }
    };

    // Discover containers using provider discovery methods.
    //
    // Each provider implements list_databases(), list_schemas(), list_catalogs(),
    // or list_projects() — matching Python's provider methods exactly.
    // BigQuery uses list_projects() which calls REST API (not SQL).
    let discovery = if ds_type == datasource_registry::DatasourceType::BigQuery {
        // BigQuery uses REST API for project discovery
        match provider.list_projects().await {
            Ok(projects) => kyomi_datasource_server::DiscoveryResult {
                items: projects,
                error: None,
            },
            Err(e) => kyomi_datasource_server::DiscoveryResult {
                items: vec![],
                error: Some(e.to_string()),
            },
        }
    } else {
        discover_primary(provider.as_ref(), &ds_type).await
    };
    provider.close().await;

    if let Some(err) = discovery.error {
        Ok(Json(DiscoverCatalogResponse {
            success: false,
            items: vec![],
            item_type,
            message: Some(err),
        }))
    } else {
        let items: Vec<CatalogItem> = discovery
            .items
            .into_iter()
            .map(|name| CatalogItem {
                name,
                item_type: meta.catalog_container_label.to_string(),
                description: None,
            })
            .collect();
        let count = items.len();
        Ok(Json(DiscoverCatalogResponse {
            success: true,
            items,
            item_type,
            message: Some(format!("Found {count} {}", meta.catalog_container_label)),
        }))
    }
}

// ---------------------------------------------------------------------------
// POST /discover — Discover ALL resources for datasource setup
// ---------------------------------------------------------------------------

async fn discover_resources(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<DiscoverResourcesRequest>,
) -> Result<Json<DiscoverResourcesResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Start with request credentials
    let mut credentials = if request.credentials.is_null() {
        json!({})
    } else {
        request.credentials.clone()
    };
    let mut connection_config = if request.connection_config.is_null() {
        json!({})
    } else {
        request.connection_config.clone()
    };
    let mut _user_cred_id: Option<i32> = None;

    // For existing datasources (edit mode), load stored credentials first
    if let Some(ref slug) = request.datasource_slug
        && let Ok(datasource) = datasource_service::resolve_datasource(
            &state.db,
            slug,
            workspace_id,
            false,
        )
        .await
    {
            // Merge connection_config: database has secrets, request has current UI values
            let db_config = &datasource.connection_config;
            let request_config = &request.connection_config;

            // Start with DB config as base
            let mut merged_config = db_config.clone();

            // Always use request's auth_mode if provided
            if let Some(am) = request_config.get("auth_mode")
                && let Some(obj) = merged_config.as_object_mut() {
                    obj.insert("auth_mode".to_string(), am.clone());
                }

            // For service_account mode, use request's service_account_json if it looks like real JSON
            if let Some(sa_json) = request_config.get("service_account_json").and_then(|v| v.as_str())
                && sa_json.trim().starts_with('{')
                && let Some(obj) = merged_config.as_object_mut() {
                    obj.insert("service_account_json".to_string(), json!(sa_json));
                }

            // Handle OAuth client credentials with mask awareness
            const MASKED_VALUE: &str = "********";
            if let Some(oci) = request_config.get("oauth_client_id")
                && let Some(obj) = merged_config.as_object_mut() {
                    obj.insert("oauth_client_id".to_string(), oci.clone());
                }
            if let Some(ocs) = request_config.get("oauth_client_secret").and_then(|v| v.as_str())
                && ocs != MASKED_VALUE
                && let Some(obj) = merged_config.as_object_mut() {
                    obj.insert("oauth_client_secret".to_string(), json!(ocs));
                }

            connection_config = merged_config;

            // Look up user's OAuth credentials for this datasource
            if let Ok(Some(user_cred)) = datasource_service::get_user_credential(
                &state.db,
                &user.user_id,
                &datasource.id,
            )
            .await
                && let Ok(db_creds) =
                    credential_service::decrypt_credentials(&user_cred.credentials, &state.encryption_key)
            {
                    credentials = db_creds;
                    _user_cred_id = Some(user_cred.id);
                    tracing::info!(
                        "[discover] Loaded stored credentials for {slug}: keys={:?}",
                        credentials.as_object().map(|o| o.keys().collect::<Vec<_>>())
                    );

                    // Override stored credentials with non-empty request values
                    let request_creds = if request.credentials.is_null() {
                        &json!({})
                    } else {
                        &request.credentials
                    };
                    if let Some(req_obj) = request_creds.as_object()
                        && let Some(cred_obj) = credentials.as_object_mut() {
                            for (key, value) in req_obj {
                                if !value.is_null()
                                    && value.as_str().map(|s| !s.is_empty()).unwrap_or(true)
                                {
                                    cred_obj.insert(key.clone(), value.clone());
                                }
                            }
                        }
                }
        }

    // Validate datasource type
    let ds_type = match datasource_registry::DatasourceType::from_str(&request.datasource_type) {
        Ok(t) => t,
        Err(e) => {
            return Ok(Json(DiscoverResourcesResponse {
                success: false,
                resources: json!({}),
                message: Some(e.to_string()),
            }));
        }
    };

    // Refresh OAuth tokens if needed (e.g., Snowflake, Databricks, Synapse)
    let credentials = match kyomi_datasource_server::ensure_valid_oauth_credentials(
        &credentials,
        &connection_config,
        &ds_type,
    )
    .await
    {
        Ok(refreshed) => refreshed,
        Err(e) => {
            tracing::warn!(
                datasource_type = %request.datasource_type,
                error = %e,
                "OAuth token refresh failed during resource discovery"
            );
            return Ok(Json(DiscoverResourcesResponse {
                success: false,
                resources: json!({}),
                message: Some("OAuth token refresh failed. Please reconnect your account.".into()),
            }));
        }
    };

    let user_context = build_user_context(&state, &user).await?;
    let user_context_ref = user_context.as_ref();

    let provider = match kyomi_datasource_server::create_provider(
        &ds_type,
        &connection_config,
        &credentials,
        user_context_ref,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                datasource_type = %request.datasource_type,
                error = %e,
                "Failed to create provider for resource discovery"
            );
            return Ok(Json(DiscoverResourcesResponse {
                success: false,
                resources: json!({}),
                message: Some("Failed to connect to datasource".into()),
            }));
        }
    };

    let mut resources = serde_json::Map::new();
    let mut errors: Vec<String> = Vec::new();

    // Discover resources using provider discovery methods.
    //
    // Each provider implements the appropriate methods (list_databases,
    // list_schemas, list_warehouses, list_catalogs) matching Python exactly.
    if ds_type == datasource_registry::DatasourceType::BigQuery {
        // BigQuery uses REST API for project discovery
        match provider.list_projects().await {
            Ok(projects) if !projects.is_empty() => {
                resources.insert("projects".to_string(), json!(projects));
            }
            Ok(_) => {} // empty list — not an error
            Err(e) => {
                tracing::warn!(error = %e, "Failed to list BigQuery projects during discovery");
                errors.push("projects: failed to list".into());
            }
        }
    } else {
        let discovery_results =
            discover_all_resources(provider.as_ref(), &ds_type).await;

        for (resource_type, result) in discovery_results {
            if let Some(err) = result.error {
                tracing::warn!(
                    resource_type,
                    error = %err,
                    "Discovery failed for resource type"
                );
                errors.push(format!("{resource_type}: discovery failed"));
            } else if !result.items.is_empty() {
                resources.insert(resource_type.to_string(), json!(result.items));
            }
        }
    }

    provider.close().await;

    // Build response
    if resources.is_empty() {
        if !errors.is_empty() {
            return Ok(Json(DiscoverResourcesResponse {
                success: false,
                resources: json!({}),
                message: Some(format!("Discovery failed: {}", errors.join("; "))),
            }));
        }
        return Ok(Json(DiscoverResourcesResponse {
            success: false,
            resources: json!({}),
            message: Some("No discoverable resources found for this datasource type".into()),
        }));
    }

    // Build success message
    let mut summaries: Vec<String> = Vec::new();
    for (resource_type, items) in &resources {
        let count = items.as_array().map(|a| a.len()).unwrap_or(0);
        summaries.push(format!("{count} {resource_type}"));
    }
    let mut message = format!("Discovered: {}", summaries.join(", "));
    if !errors.is_empty() {
        message += &format!(" (partial failures: {})", errors.join("; "));
    }

    Ok(Json(DiscoverResourcesResponse {
        success: true,
        resources: Value::Object(resources),
        message: Some(message),
    }))
}

// ---------------------------------------------------------------------------
// GET /{identifier}/catalog/tree — Hierarchical catalog tree from cache
// ---------------------------------------------------------------------------

async fn get_catalog_tree(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
    Query(params): Query<CatalogTreeParams>,
) -> Result<Json<CatalogTreeResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    let datasource = resolve_or_404(&state.db, &identifier, workspace_id, false).await?;

    // Get all cached tables for this datasource (exclude archived).
    //
    // Note: sample datasources used to live in a shared sentinel workspace
    // (`SAMPLE_DATA_WORKSPACE_ID`) and required a special branch here, but
    // they now index into the user's workspace via the generic per-workspace
    // indexer (see `kyomi_ui::server_fns::onboarding::add_sample_datasource`).
    // Query by `datasource_config_id` uniformly.
    let is_pg = state.db.is_postgres();
    let bf = kyomi_core::sql_compat::bool_false(is_pg);
    let mut cached_tables: Vec<DatasourceTableCache> = kyomi_core::db_fetch_all!(
        &state.db, DatasourceTableCache,
        &format!(
            "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
             table_metadata, column_descriptions, created_at, updated_at, \
             structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
             FROM datasource_table_cache \
             WHERE datasource_config_id = $1 AND is_archived = {bf}"
        ),
        &datasource.id
    )?;

    // SPECIAL CASE: BigQuery public datasets.
    // Public datasets are stored in a sentinel workspace and shared across all workspaces.
    // Include them when the datasource has include_public_datasets enabled (defaults to true).
    if datasource.datasource_type == kyomi_core::DatasourceType::Bigquery {
        let include_public = datasource
            .connection_config
            .get("include_public_datasets")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        if include_public {
            let public_tables: Vec<DatasourceTableCache> = kyomi_core::db_fetch_all!(
                &state.db, DatasourceTableCache,
                &format!(
                    "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
                     table_metadata, column_descriptions, created_at, updated_at, \
                     structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
                     FROM datasource_table_cache \
                     WHERE workspace_id = $1 AND is_archived = {bf}"
                ),
                kyomi_auth::catalog::indexers::bigquery_public::PUBLIC_DATA_WORKSPACE_ID
            )?;
            cached_tables.extend(public_tables);
        }
    }

    let table_count = cached_tables.len();

    // Build tree: {project_id: {dataset_id: [table_nodes]}}
    // Use BTreeMap for sorted output
    let mut tree_dict: BTreeMap<String, BTreeMap<String, Vec<CatalogTreeNode>>> = BTreeMap::new();
    let mut last_indexed: Option<String> = None;

    for table in &cached_tables {
        let project = &table.project_id;
        let dataset = &table.dataset_id;
        let table_name = &table.table_id;

        let project_map = tree_dict.entry(project.clone()).or_default();
        let table_list = project_map.entry(dataset.clone()).or_default();

        // Build table node ID: skip project prefix if empty
        let table_id = if project.is_empty() {
            format!("{dataset}.{table_name}")
        } else {
            format!("{project}.{dataset}.{table_name}")
        };

        let metadata = {
            let description = table
                .table_metadata
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| json!(s))
                .unwrap_or(Value::Null);
            let row_count = table
                .table_metadata
                .get("row_count")
                .cloned()
                .unwrap_or(Value::Null);
            json!({
                "description": description,
                "row_count": row_count,
            })
        };

        let children = if params.include_columns {
            let columns = table
                .table_metadata
                .get("columns")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            let col_nodes: Vec<CatalogTreeNode> = columns
                .iter()
                .map(|col| {
                    let col_name = col
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let col_id = if project.is_empty() {
                        format!("{dataset}.{table_name}.{col_name}")
                    } else {
                        format!("{project}.{dataset}.{table_name}.{col_name}")
                    };
                    let col_meta = json!({
                        "data_type": col.get("type"),
                        "description": col.get("description"),
                    });
                    CatalogTreeNode {
                        id: col_id,
                        name: col_name.to_string(),
                        node_type: "column".into(),
                        children: None,
                        metadata: Some(col_meta),
                    }
                })
                .collect();

            if col_nodes.is_empty() {
                None
            } else {
                Some(col_nodes)
            }
        } else {
            None
        };

        table_list.push(CatalogTreeNode {
            id: table_id,
            name: table_name.clone(),
            node_type: "table".into(),
            children,
            metadata: Some(metadata),
        });

        // Track last indexed time — use last_verified (set on every refresh check)
        // with fallback to updated_at (only set on schema changes).
        let checked_at = table.last_verified.unwrap_or(table.updated_at);
        let checked_str = checked_at.to_rfc3339();
        if last_indexed.as_ref().is_none_or(|li| &checked_str > li) {
            last_indexed = Some(checked_str);
        }
    }

    // Convert tree_dict to CatalogTreeNode structure using registry metadata
    let meta = datasource_registry::get_metadata_by_str(datasource.datasource_type.as_ref())
        .ok_or_else(|| {
            kyomi_core::Error::Internal(format!(
                "Unknown datasource type: '{}'",
                datasource.datasource_type
            ))
        })?;

    let level1_type = meta.tree_level1_type;
    let level2_type = meta.tree_level2_type;

    let mut tree: Vec<CatalogTreeNode> = Vec::new();

    for (project_id, datasets) in &tree_dict {
        let mut dataset_nodes: Vec<CatalogTreeNode> = Vec::new();

        for (dataset_id, tables) in datasets {
            let ds_id = if project_id.is_empty() {
                dataset_id.clone()
            } else {
                format!("{project_id}.{dataset_id}")
            };

            let mut sorted_tables = tables.clone();
            sorted_tables.sort_by(|a, b| a.name.cmp(&b.name));

            dataset_nodes.push(CatalogTreeNode {
                id: ds_id,
                name: dataset_id.clone(),
                node_type: level2_type.to_string(),
                children: Some(sorted_tables),
                metadata: None,
            });
        }

        // Sort dataset nodes by name
        dataset_nodes.sort_by(|a, b| a.name.cmp(&b.name));

        // Determine if we should skip the level1 wrapper
        let skip_wrapper = (meta.skip_empty_project_wrapper && project_id.is_empty())
            || (meta.skip_single_project_wrapper && tree_dict.len() == 1);

        if skip_wrapper {
            tree = dataset_nodes;
        } else {
            tree.push(CatalogTreeNode {
                id: project_id.clone(),
                name: project_id.clone(),
                node_type: level1_type.to_string(),
                children: Some(dataset_nodes),
                metadata: None,
            });
        }
    }

    Ok(Json(CatalogTreeResponse {
        datasource_id: datasource.id,
        datasource_type: datasource.datasource_type.to_string(),
        datasource_name: datasource.name,
        tree,
        table_count,
        last_indexed,
    }))
}

// ---------------------------------------------------------------------------
// GET /{identifier}/catalog/status — Indexing status and statistics
// ---------------------------------------------------------------------------

async fn get_catalog_status(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
) -> Result<Json<CatalogStatusResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Admins can view catalog status for disabled datasources
    let datasource =
        resolve_or_404(&state.db, &identifier, workspace_id, is_admin(&user)).await?;

    // Get cached table stats for this datasource (exclude archived).
    // Sample datasources now index into the user's workspace like any other
    // datasource — the old sentinel branch has been removed.
    let is_pg = state.db.is_postgres();
    let bf = kyomi_core::sql_compat::bool_false(is_pg);
    let cached_tables: Vec<DatasourceTableCache> = kyomi_core::db_fetch_all!(
        &state.db, DatasourceTableCache,
        &format!(
            "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
             table_metadata, column_descriptions, created_at, updated_at, \
             structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
             FROM datasource_table_cache \
             WHERE datasource_config_id = $1 AND is_archived = {bf}"
        ),
        &datasource.id
    )?;

    let table_count = cached_tables.len();

    // Count unique schemas/datasets and track last indexed
    let mut schemas: HashSet<String> = HashSet::new();
    let mut last_indexed: Option<String> = None;

    for table in &cached_tables {
        if !table.dataset_id.is_empty() {
            schemas.insert(table.dataset_id.clone());
        }
        // Use last_verified (set on every refresh check) with fallback to updated_at
        let checked_at = table.last_verified.unwrap_or(table.updated_at);
        let checked_str = checked_at.to_rfc3339();
        if last_indexed.as_ref().is_none_or(|li| &checked_str > li) {
            last_indexed = Some(checked_str);
        }
    }

    let schema_count = schemas.len();

    // Get indexing status from workspace.
    // The catalog_refresh_status column is VARCHAR(50) — may contain:
    //   - A simple string: "idle", "running", "failed"
    //   - A JSON-encoded dict: {"datasource_id": "...", "status": "...", "progress": N}
    let indexing_status: String;
    let mut indexing_progress: Option<i32> = None;

    #[derive(sqlx::FromRow)]
    struct RefreshStatusRow { catalog_refresh_status: Option<String> }
    let refresh_status_str = kyomi_core::db_fetch_optional!(
        &state.db, RefreshStatusRow,
        "SELECT catalog_refresh_status FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?
    .and_then(|r| r.catalog_refresh_status);

    if let Some(status_str) = refresh_status_str {
        // Try to parse as JSON object (advanced status with per-datasource tracking)
        if let Ok(parsed) = serde_json::from_str::<Value>(&status_str) {
            if parsed.is_object()
                && parsed.get("datasource_id").and_then(|v| v.as_str()) == Some(&datasource.id)
            {
                indexing_status = parsed
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("idle")
                    .to_string();
                indexing_progress =
                    parsed.get("progress").and_then(|v| v.as_i64()).map(|v| v as i32);
            } else {
                indexing_status = "idle".into();
            }
        } else {
            // Simple string value — use directly if it looks like a known status,
            // otherwise default to "idle"
            indexing_status = match status_str.as_str() {
                "idle" | "running" | "failed" => status_str,
                _ => "idle".into(),
            };
        }
    } else {
        indexing_status = "idle".into();
    }

    // Extract catalog config using registry metadata
    let meta = datasource_registry::get_metadata_by_str(datasource.datasource_type.as_ref())
        .ok_or_else(|| {
            kyomi_core::Error::Internal(format!(
                "Unknown datasource type: '{}'",
                datasource.datasource_type
            ))
        })?;

    let conn_config = &datasource.connection_config;
    let mut catalog_config = serde_json::Map::new();

    for key in meta.catalog_config_keys {
        if *key == "include_public_datasets" {
            // Boolean flag (BigQuery-specific)
            let val = conn_config
                .get(*key)
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            catalog_config.insert(key.to_string(), json!(val));
        } else {
            // List value (catalog_schemas, catalog_databases, etc.)
            let val = conn_config
                .get(*key)
                .cloned()
                .unwrap_or(json!([]));
            catalog_config.insert(key.to_string(), val);
        }
    }

    Ok(Json(CatalogStatusResponse {
        datasource_id: datasource.id,
        datasource_name: datasource.name,
        datasource_type: datasource.datasource_type.to_string(),
        table_count,
        schema_count,
        last_indexed,
        indexing_status: Some(indexing_status),
        indexing_progress,
        catalog_config: Some(Value::Object(catalog_config)),
    }))
}

// ---------------------------------------------------------------------------
// GET /{identifier}/schemas — Live schema list from datasource
// ---------------------------------------------------------------------------

async fn list_schemas(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
) -> Result<Json<SchemaListResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    let datasource = resolve_or_404(&state.db, &identifier, workspace_id, false).await?;

    // Get registry metadata
    let meta = datasource_registry::get_metadata_by_str(datasource.datasource_type.as_ref())
        .ok_or_else(|| {
            kyomi_core::Error::Internal(format!(
                "Unknown datasource type: '{}'",
                datasource.datasource_type
            ))
        })?;

    // Check if discovery is supported
    if !meta.supports_catalog_discovery {
        let msg = format!(
            "Schema discovery not supported for {}",
            datasource.datasource_type
        );
        return Ok(Json(SchemaListResponse {
            datasource_id: datasource.id,
            datasource_type: datasource.datasource_type.to_string(),
            schemas: vec![],
            message: Some(msg),
        }));
    }

    let ds_type: datasource_registry::DatasourceType = datasource.datasource_type.into();

    // --- Connect datasources: route through the Connect agent ---
    if datasource.connection_type == "connect" {
        use crate::connect::provider::ConnectProvider;

        let provider = ConnectProvider::with_timeout(
            state.connect_registry.clone(),
            datasource.id.clone(),
            std::time::Duration::from_secs(30),
        );

        let discovery = match provider.discover_catalog().await {
            Ok(catalog) => {
                let items: Vec<String> = catalog
                    .containers
                    .into_iter()
                    .map(|c| c.name)
                    .collect();
                kyomi_datasource_server::DiscoveryResult {
                    items,
                    error: None,
                }
            }
            Err(e) => {
                tracing::warn!(
                    datasource_id = %datasource.id,
                    error = %e,
                    "Connect catalog discovery failed for schema listing"
                );
                kyomi_datasource_server::DiscoveryResult {
                    items: vec![],
                    error: Some(format!(
                        "Failed to list schemas via Connect — is the agent running? ({e})"
                    )),
                }
            }
        };

        return Ok(Json(SchemaListResponse {
            datasource_id: datasource.id,
            datasource_type: datasource.datasource_type.to_string(),
            schemas: discovery.items,
            message: discovery.error,
        }));
    }

    // --- Direct datasources: connect using server-side credentials ---

    // Get user credentials
    let user_cred =
        datasource_service::get_user_credential(&state.db, &user.user_id, &datasource.id)
            .await?;

    let credentials = if let Some(ref cred) = user_cred {
        credential_service::decrypt_credentials(&cred.credentials, &state.encryption_key)?
    } else {
        json!({})
    };

    // Refresh OAuth if needed
    let credentials = match kyomi_datasource_server::ensure_valid_oauth_credentials(
        &credentials,
        &datasource.connection_config,
        &ds_type,
    )
    .await
    {
        Ok(refreshed) if refreshed != credentials => {
            if let Some(ref cred) = user_cred
                && let Err(e) = datasource_service::save_user_credential(
                    &state.db,
                    &state.encryption_key,
                    &user.user_id,
                    &datasource.id,
                    &cred.workspace_id,
                    &refreshed,
                )
                .await
            {
                    tracing::warn!(
                        datasource_id = %datasource.id,
                        "Failed to persist refreshed OAuth token: {e}"
                    );
                }
            refreshed
        }
        Ok(unchanged) => unchanged,
        Err(_) => credentials,
    };

    let user_context = build_user_context(&state, &user).await?;
    let user_context_ref = user_context.as_ref();

    let provider = match kyomi_datasource_server::create_provider(
        &ds_type,
        &datasource.connection_config,
        &credentials,
        user_context_ref,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                datasource_id = %datasource.id,
                error = %e,
                "Failed to create provider for schema listing"
            );
            return Ok(Json(SchemaListResponse {
                datasource_id: datasource.id,
                datasource_type: datasource.datasource_type.to_string(),
                schemas: vec![],
                message: Some("Failed to connect to datasource".into()),
            }));
        }
    };

    // Use provider discovery methods matching Python's approach.
    let discovery = if ds_type == datasource_registry::DatasourceType::BigQuery {
        match provider.list_projects().await {
            Ok(projects) => kyomi_datasource_server::DiscoveryResult {
                items: projects,
                error: None,
            },
            Err(e) => kyomi_datasource_server::DiscoveryResult {
                items: vec![],
                error: Some(e.to_string()),
            },
        }
    } else {
        discover_primary(provider.as_ref(), &ds_type).await
    };
    provider.close().await;

    Ok(Json(SchemaListResponse {
        datasource_id: datasource.id,
        datasource_type: datasource.datasource_type.to_string(),
        schemas: discovery.items,
        message: discovery.error,
    }))
}

// ---------------------------------------------------------------------------
// POST /{identifier}/catalog/refresh — Trigger manual catalog refresh
// ---------------------------------------------------------------------------

async fn refresh_catalog(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
    body: Option<Json<CatalogRefreshRequest>>,
) -> Result<Json<CatalogRefreshResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Check admin permission
    if !is_admin(&user) {
        return Err(kyomi_core::Error::Forbidden(
            "Only workspace admins can trigger catalog refresh".into(),
        ));
    }

    let datasource = resolve_or_404(&state.db, &identifier, workspace_id, false).await?;

    // (The old "sample datasources cannot be refreshed manually" gate was
    // valid when samples lived in a shared sentinel workspace. Samples are
    // now indexed into the user's workspace by the generic per-workspace
    // indexer, so the normal refresh path works for them too.)

    let force = body.map(|b| b.force).unwrap_or(false);

    execute_catalog_refresh(&state, &user, datasource, workspace_id, force).await
        .map(Json)
}

/// Core catalog refresh logic, shared by the generic endpoint and bigquery-specific endpoint.
///
/// Delegates to [`kyomi_ui::server_fns::catalog_refresh::execute_catalog_refresh`]
/// which is the single source of truth for catalog refresh orchestration.
pub(crate) async fn execute_catalog_refresh(
    state: &AppState,
    user: &AuthUser,
    datasource: kyomi_core::models::datasource::DatasourceConfig,
    workspace_id: &str,
    force: bool,
) -> Result<CatalogRefreshResponse, kyomi_core::Error> {
    // Resolve and decrypt user credentials.
    let user_cred =
        datasource_service::get_user_credential(&state.db, &user.user_id, &datasource.id)
            .await?;

    let credentials = if let Some(ref cred) = user_cred {
        credential_service::decrypt_credentials(&cred.credentials, &state.encryption_key)
            .unwrap_or(json!({}))
    } else {
        json!({})
    };

    let ds_type: datasource_registry::DatasourceType = datasource.datasource_type.into();

    // Refresh OAuth credentials if needed.
    let credentials = match kyomi_datasource_server::ensure_valid_oauth_credentials(
        &credentials,
        &datasource.connection_config,
        &ds_type,
    )
    .await
    {
        Ok(refreshed) if refreshed != credentials => {
            // Persist refreshed token.
            if let Some(ref cred) = user_cred {
                let _ = datasource_service::save_user_credential(
                    &state.db,
                    &state.encryption_key,
                    &user.user_id,
                    &datasource.id,
                    &cred.workspace_id,
                    &refreshed,
                )
                .await;
            }
            refreshed
        }
        Ok(unchanged) => unchanged,
        Err(_) => credentials,
    };

    let user_context = build_user_context(state, user).await?;

    let embedding = state.embedding.wait_ready().await?;

    let params = kyomi_ui::server_fns::catalog_refresh::CatalogRefreshParams {
        db: &state.db,
        embedding,
        encryption_key: &state.encryption_key,
        datasource,
        workspace_id,
        user_id: &user.user_id,
        force,
        connect_registry: Some(&state.connect_registry),
        user_context,
        credentials,
    };

    let result =
        kyomi_ui::server_fns::catalog_refresh::execute_catalog_refresh(params).await?;

    Ok(CatalogRefreshResponse {
        status: result.status,
        message: result.message,
        datasource_id: result.datasource_id,
    })
}

// Unit tests for SQL helpers have been moved to kyomi_auth::catalog::sql_helpers.
