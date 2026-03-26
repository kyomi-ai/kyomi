// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource management endpoints.
//!
//! Wire-compatible with Python's `routers/datasources.py`.
//! All responses use `{"detail": "message"}` format for errors.
//!
//! ## Phase 5 scope
//!
//! 18 endpoints for datasource CRUD, credentials, settings, toggle,
//! test-connection, SSH key generation, and affected-users.
//!
//! Test-connection endpoints return "not yet implemented" — actual providers
//! come in Phase 6.

use std::str::FromStr;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use kyomi_auth::{
    credential_service,
    datasource_auth_service::{self, CredentialStatusResult},
    datasource_service,
    middleware::AuthUser,
    websocket::helpers as ws_helpers,
};
use kyomi_core::stream::QueryStreamEvent;
use kyomi_core::{datasource_registry, DatasourceType, MessageType, WebSocketMessage, WorkspaceRole};

use crate::connect::provider::ConnectProvider;
use crate::state::AppState;

/// Build the `/datasources` router with all datasource management endpoints.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Static paths FIRST (before /{identifier} captures them)
        .route("/credential-status", get(get_credential_status))
        .route("/types", get(list_datasource_types))
        .route("/sample/available", get(check_sample_available))
        .route("/sample", post(create_sample_datasource))
        .route("/test-connection", post(test_connection_standalone))
        .route("/query/execute", post(execute_query))
        .route("/query/stream", post(execute_query_stream))
        // Dynamic path handlers
        .route("/", get(list_datasources).post(create_datasource))
        .route(
            "/{identifier}",
            get(get_datasource)
                .put(update_datasource)
                .delete(delete_datasource_handler),
        )
        .route(
            "/{identifier}/credentials",
            get(get_credentials)
                .post(save_credentials)
                .delete(delete_credentials),
        )
        .route(
            "/{identifier}/settings",
            get(get_settings).put(save_settings),
        )
        .route("/{identifier}/toggle", post(toggle_datasource))
        .route("/{identifier}/test", post(test_datasource_connection))
        .route(
            "/{identifier}/generate-ssh-key",
            post(generate_ssh_key),
        )
        .route("/{identifier}/affected-users", get(get_affected_users))
        // Kyomi Connect management
        .route("/{identifier}/connect/rotate-token", post(rotate_connect_token))
        .route("/{identifier}/connect/disconnect", post(disconnect_connect))
        .route("/{identifier}/connect/status", get(connect_status))
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Reject non-workspace-admin users with 403.
fn require_workspace_admin(user: &AuthUser) -> Result<(), kyomi_core::Error> {
    if !user
        .workspace
        .workspace_roles
        .contains(&WorkspaceRole::WorkspaceAdmin)
    {
        return Err(kyomi_core::Error::Forbidden(
            "Workspace admin access required".into(),
        ));
    }
    Ok(())
}

/// Extract workspace_id from user, or return 400.
fn get_workspace_id(user: &AuthUser) -> Result<&str, kyomi_core::Error> {
    user.workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("User not associated with a workspace".into()))
}

/// Check if user is an admin (without erroring — used for permission filtering).
fn is_admin(user: &AuthUser) -> bool {
    user.workspace
        .workspace_roles
        .contains(&WorkspaceRole::WorkspaceAdmin)
        || user.workspace.is_owner
}

/// Resolve a datasource by identifier (slug or UUID), returning 404 with available
/// slugs on failure. This is the common resolve pattern used by most endpoints.
async fn resolve_or_404(
    db: &kyomi_core::DbPool,
    identifier: &str,
    workspace_id: &str,
    include_inactive: bool,
) -> Result<kyomi_core::models::datasource::DatasourceConfig, kyomi_core::Error> {
    datasource_service::resolve_datasource(db, identifier, workspace_id, include_inactive).await
}

/// Check that a datasource is active, returning 404 if not.
fn require_active(
    ds: &kyomi_core::models::datasource::DatasourceConfig,
) -> Result<(), kyomi_core::Error> {
    if !ds.active {
        return Err(kyomi_core::Error::NotFound(
            "Datasource is inactive".into(),
        ));
    }
    Ok(())
}

/// Preserved sensitive fields constant.
const MASKED_VALUE: &str = "********";

// ===========================================================================
// Request / Response Types
// ===========================================================================

// -- List datasources --

#[derive(Serialize)]
struct DatasourceListResponse {
    id: String,
    slug: String,
    name: String,
    datasource_type: String,
    active: bool,
    created_at: String,
    auto_refresh_allowed: bool,
    is_sample: bool,
    /// "direct" or "connect"
    connection_type: String,
}

// -- Datasource detail --

#[derive(Serialize)]
struct DatasourceResponse {
    id: String,
    slug: String,
    name: String,
    datasource_type: String,
    connection_config: Value,
    active: bool,
    created_at: String,
    updated_at: String,
    has_user_credentials: bool,
    auto_refresh_allowed: bool,
    /// "direct" or "connect"
    connection_type: String,
}

// -- Create / Update --

#[derive(Deserialize)]
struct CreateDatasourceRequest {
    name: String,
    slug: Option<String>,
    datasource_type: String,
    connection_config: Option<Value>,
    /// Connection type: "direct" (default) or "connect" (Kyomi Connect).
    #[serde(default = "default_connection_type")]
    connection_type: String,
}

fn default_connection_type() -> String {
    "direct".to_string()
}

/// Response for datasource creation — extends DatasourceResponse with optional connect_token.
#[derive(Serialize)]
struct CreateDatasourceResponse {
    #[serde(flatten)]
    datasource: DatasourceResponse,
    /// One-time Connect token (only present for `connection_type: "connect"` on creation).
    #[serde(skip_serializing_if = "Option::is_none")]
    connect_token: Option<String>,
}

#[derive(Deserialize)]
struct UpdateDatasourceRequest {
    name: Option<String>,
    slug: Option<String>,
    connection_config: Option<Value>,
    active: Option<bool>,
    auto_refresh_allowed: Option<bool>,
}

// -- Credentials --

#[derive(Deserialize)]
struct SaveCredentialsRequest {
    credentials: Value,
}

#[derive(Serialize)]
struct CredentialsResponse {
    datasource_id: String,
    datasource_slug: String,
    datasource_name: String,
    datasource_type: String,
    has_credentials: bool,
    credentials_preview: Value,
    created_at: Option<String>,
    updated_at: Option<String>,
}

// -- Credential status --

#[derive(Serialize)]
struct DatasourceCredentialStatus {
    id: String,
    slug: String,
    name: String,
    datasource_type: String,
    credential_status: String,
    auth_method: String,
    oauth_provider: Option<String>,
    user_enabled: bool,
    can_enable: bool,
    connection_config: Value,
}

#[derive(Serialize)]
struct CredentialStatusSummary {
    total: i32,
    ready: i32,
    needs_credentials: i32,
    needs_oauth: i32,
    needs_password: i32,
}

#[derive(Serialize)]
struct CredentialStatusResponse {
    datasources: Vec<DatasourceCredentialStatus>,
    summary: CredentialStatusSummary,
}

// -- Types --

#[derive(Serialize)]
struct DatasourceTypeMetadataResponse {
    type_id: String,
    display_name: String,
    description: String,
    default_port: Option<u16>,
    credential_fields: Vec<String>,
    requires_user_credentials: bool,
    accepts_user_context: bool,
    catalog_container_label: String,
    catalog_config_keys: Vec<String>,
    supports_catalog_discovery: bool,
    auth_modes: Vec<String>,
    sensitive_connection_config_fields: Vec<String>,
}

#[derive(Serialize)]
struct DatasourceTypesResponse {
    types: Vec<DatasourceTypeMetadataResponse>,
}

// -- Sample available --

#[derive(Serialize)]
struct SampleAvailableResponse {
    configured: bool,
    already_added: bool,
}

// -- Toggle --

#[derive(Deserialize)]
struct DatasourceToggleRequest {
    enabled: bool,
}

#[derive(Serialize)]
struct DatasourceToggleResponse {
    id: String,
    slug: String,
    enabled: bool,
    message: String,
}

// -- Test connection --

/// Phase 5 stub — fields are deserialised by serde for wire compatibility but
/// not consumed until providers are implemented in Phase 6.
#[derive(Deserialize)]
struct StandaloneTestConnectionRequest {
    datasource_type: String,
    connection_config: Value,
    #[serde(default)]
    credentials: Option<Value>,
}

// TestConnectionRequest body is accepted for wire compatibility but not
// consumed — credentials come from the database for existing datasources.

#[derive(Serialize)]
struct TestConnectionResponse {
    success: bool,
    message: String,
}

// -- Query execute --

#[derive(Deserialize)]
struct ExecuteQueryRequest {
    sql: String,
    datasource: String,
    #[serde(default = "default_query_limit")]
    limit: i32,
    #[serde(default)]
    offset: i32,
    #[serde(default = "default_page_size")]
    page_size: i32,
    #[serde(default)]
    dry_run: bool,
    #[serde(default = "default_true")]
    include_total: bool,
    /// Optional client-generated request ID for streaming queries.
    /// When provided, the backend uses this instead of generating its own.
    /// This avoids a race condition where a WebSocket error message arrives
    /// before the HTTP response that sets the request ID.
    #[serde(default)]
    request_id: Option<String>,
}

fn default_query_limit() -> i32 {
    1000
}

fn default_page_size() -> i32 {
    50
}

fn default_true() -> bool {
    true
}

#[derive(Serialize)]
struct ExecuteQueryResponse {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    columns: Option<Vec<ExecuteQueryColumnInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<Vec<Vec<Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_rows: Option<i64>,
    page_size: i32,
    has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    column: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes_processed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_time_ms: Option<i64>,
}

#[derive(Serialize)]
struct ExecuteQueryColumnInfo {
    name: String,
    #[serde(rename = "type")]
    col_type: String,
}

// -- SSH key --

#[derive(Serialize)]
struct SSHKeyResponse {
    public_key: String,
    key_type: String,
    message: String,
}

// -- Affected users --

#[derive(Serialize)]
struct AffectedUserInfo {
    email: String,
    auth_type: String,
}

#[derive(Serialize)]
struct AffectedUsersResponse {
    affected_count: i32,
    affected_users: Vec<AffectedUserInfo>,
    warning_message: Option<String>,
}

// -- User settings --

#[derive(Serialize)]
struct UserSettingsResponse {
    datasource_id: String,
    datasource_slug: String,
    datasource_name: String,
    datasource_type: String,
    user_settings: Value,
    workspace_defaults: Value,
    effective_settings: Value,
    has_oauth: bool,
    oauth_email: Option<String>,
    has_bigquery_scopes: bool,
    needs_bigquery_connect: bool,
    connection_config: Value,
    shared_credentials: bool,
    credential_status: String,
    auth_method: String,
    oauth_provider: Option<String>,
    has_password: bool,
    has_username: bool,
    has_access_token: bool,
    auth_mode: Option<String>,
    enable_arrow_streaming: Option<bool>,
    service_account_email: Option<String>,
}

// -- Query params --

#[derive(Deserialize)]
struct ListDatasourcesParams {
    #[serde(default)]
    include_inactive: bool,
}

#[derive(Deserialize)]
struct AffectedUsersParams {
    new_auth_mode: String,
}

// ===========================================================================
// Endpoint Handlers
// ===========================================================================

// ---------------------------------------------------------------------------
// GET / — List datasources
// ---------------------------------------------------------------------------

async fn list_datasources(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ListDatasourcesParams>,
) -> Result<Json<Vec<DatasourceListResponse>>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Only admins can see inactive datasources
    let include_inactive = params.include_inactive && is_admin(&user);

    let datasources =
        datasource_service::list_datasources(&state.db, workspace_id, include_inactive).await?;

    let result: Vec<DatasourceListResponse> = datasources
        .into_iter()
        .map(|ds| {
            let is_sample = ds
                .connection_config
                .get("is_sample")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            DatasourceListResponse {
                id: ds.id,
                slug: ds.slug,
                name: ds.name,
                datasource_type: ds.datasource_type.to_string(),
                active: ds.active,
                created_at: ds.created_at.to_rfc3339(),
                auto_refresh_allowed: ds.auto_refresh_allowed,
                is_sample,
                connection_type: ds.connection_type,
            }
        })
        .collect();

    tracing::info!(
        "Listed {} datasources for workspace {}",
        result.len(),
        workspace_id
    );

    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// GET /credential-status — Credential status for all datasources
// ---------------------------------------------------------------------------

async fn get_credential_status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<CredentialStatusResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Get all active datasources
    let datasources =
        datasource_service::list_datasources(&state.db, workspace_id, false).await?;

    // Get user's credentials for all datasources in one query
    let user_credentials = kyomi_core::db_fetch_all!(
        &state.db,
        kyomi_core::models::datasource::UserDatasourceCredential,
        "SELECT id, user_id, datasource_config_id, workspace_id, credentials, \
         enabled, created_at, updated_at \
         FROM user_datasource_credentials \
         WHERE user_id = $1 AND workspace_id = $2",
        &user.user_id,
        workspace_id
    )?;

    // Build lookup
    let creds_by_ds: std::collections::HashMap<&str, &kyomi_core::models::datasource::UserDatasourceCredential> =
        user_credentials
            .iter()
            .map(|c| (c.datasource_config_id.as_str(), c))
            .collect();

    // Get user's preferences for shared-auth datasources
    let user_preferences = kyomi_core::db_fetch_all!(
        &state.db,
        kyomi_core::models::datasource::UserDatasourcePreference,
        "SELECT id, user_id, datasource_config_id, enabled, \
         created_at, updated_at \
         FROM user_datasource_preferences \
         WHERE user_id = $1",
        &user.user_id
    )?;

    let prefs_by_ds: std::collections::HashMap<&str, &kyomi_core::models::datasource::UserDatasourcePreference> =
        user_preferences
            .iter()
            .map(|p| (p.datasource_config_id.as_str(), p))
            .collect();

    let mut statuses = Vec::new();
    let mut summary = CredentialStatusSummary {
        total: 0,
        ready: 0,
        needs_credentials: 0,
        needs_oauth: 0,
        needs_password: 0,
    };

    for ds in &datasources {
        summary.total += 1;
        let connection_config = &ds.connection_config;
        let user_cred = creds_by_ds.get(ds.id.as_str()).copied();
        let is_connect = ds.connection_type == "connect";

        // Connect datasources don't use user credentials — treat like shared auth
        let (result, user_enabled, can_enable) = if is_connect {
            let pref = prefs_by_ds.get(ds.id.as_str()).copied();
            let enabled = pref.is_none_or(|p| p.enabled);
            let status = CredentialStatusResult {
                credential_status: "shared".to_string(),
                auth_method: "connect".to_string(),
                oauth_provider: None,
            };
            (status, enabled, true)
        } else {
            // Check credential status
            let result = datasource_auth_service::check_credential_status(
                ds.datasource_type.as_ref(),
                connection_config,
                user_cred,
                &state.encryption_key,
            );

            // Get user enabled preference
            let user_enabled = datasource_auth_service::get_user_enabled(
                ds.datasource_type.as_ref(),
                connection_config,
                user_cred,
                prefs_by_ds.get(ds.id.as_str()).copied(),
            );

            let has_credentials =
                result.credential_status == "valid" || result.credential_status == "shared";
            let can_enable = has_credentials || user_enabled;
            (result, user_enabled, can_enable)
        };

        // Update summary
        match result.credential_status.as_str() {
            "valid" | "shared" => summary.ready += 1,
            "missing" => {
                summary.needs_credentials += 1;
                if result.auth_method == "oauth" {
                    summary.needs_oauth += 1;
                } else if result.auth_method == "password" {
                    summary.needs_password += 1;
                }
            }
            "expired" => {
                summary.needs_oauth += 1;
            }
            _ => {}
        }

        statuses.push(DatasourceCredentialStatus {
            id: ds.id.clone(),
            slug: ds.slug.clone(),
            name: ds.name.clone(),
            datasource_type: ds.datasource_type.to_string(),
            credential_status: result.credential_status,
            auth_method: result.auth_method,
            oauth_provider: result.oauth_provider,
            user_enabled,
            can_enable,
            connection_config: connection_config.clone(),
        });
    }

    tracing::info!(
        "Credential status for workspace {}: {} ready, {} need credentials",
        workspace_id,
        summary.ready,
        summary.needs_credentials
    );

    Ok(Json(CredentialStatusResponse {
        datasources: statuses,
        summary,
    }))
}

// ---------------------------------------------------------------------------
// GET /types — List datasource type metadata
// ---------------------------------------------------------------------------

async fn list_datasource_types() -> Result<Json<DatasourceTypesResponse>, kyomi_core::Error> {
    let all_meta = datasource_registry::all_metadata();

    let mut types: Vec<DatasourceTypeMetadataResponse> = all_meta
        .into_iter()
        .map(|(_, meta)| DatasourceTypeMetadataResponse {
            type_id: meta.type_id.to_string(),
            display_name: meta.display_name.to_string(),
            description: meta.description.to_string(),
            default_port: meta.default_port,
            credential_fields: meta.credential_fields.iter().map(|s| s.to_string()).collect(),
            requires_user_credentials: meta.requires_user_credentials,
            accepts_user_context: meta.accepts_user_context,
            catalog_container_label: meta.catalog_container_label.to_string(),
            catalog_config_keys: meta.catalog_config_keys.iter().map(|s| s.to_string()).collect(),
            supports_catalog_discovery: meta.supports_catalog_discovery,
            auth_modes: meta
                .auth_modes
                .iter()
                .map(|m| m.mode_id.to_string())
                .collect(),
            sensitive_connection_config_fields: meta
                .sensitive_connection_config_fields
                .iter()
                .map(|s| s.to_string())
                .collect(),
        })
        .collect();

    // Sort by popularity (most commonly used first)
    let popularity_order = |type_id: &str| -> u32 {
        match type_id {
            "postgres" => 0,
            "mysql" => 1,
            "bigquery" => 2,
            "snowflake" => 3,
            "clickhouse" => 4,
            "databricks" => 5,
            "redshift" => 6,
            _ => 99,
        }
    };

    types.sort_by(|a, b| {
        popularity_order(&a.type_id)
            .cmp(&popularity_order(&b.type_id))
            .then(a.display_name.cmp(&b.display_name))
    });

    tracing::info!("Listed {} registered datasource types", types.len());

    Ok(Json(DatasourceTypesResponse { types }))
}

// ---------------------------------------------------------------------------
// GET /sample/available — Check sample datasource availability
// ---------------------------------------------------------------------------

async fn check_sample_available(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<SampleAvailableResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let configured = std::env::var("SAMPLE_CLICKHOUSE_HOST").is_ok();

    let already_added = if configured {
        let datasources =
            datasource_service::list_datasources(&state.db, workspace_id, false).await?;
        datasources.iter().any(|ds| {
            ds.connection_config
                .get("is_sample")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        })
    } else {
        false
    };

    Ok(Json(SampleAvailableResponse {
        configured,
        already_added,
    }))
}

// ---------------------------------------------------------------------------
// POST /sample — Create sample ClickHouse datasource (admin only)
// ---------------------------------------------------------------------------

async fn create_sample_datasource(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<(StatusCode, Json<DatasourceResponse>), kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;

    // Check if sample ClickHouse is configured
    let ch_config =
        kyomi_auth::catalog::indexers::sample_data::SampleClickHouseConfig::from_env()
            .ok_or_else(|| {
                kyomi_core::Error::Internal(
                    "Sample database is not configured on this server".into(),
                )
            })?;

    // Check if workspace already has a sample datasource
    let datasources =
        datasource_service::list_datasources(&state.db, workspace_id, true).await?;
    let has_sample = datasources.iter().any(|ds| {
        ds.connection_config
            .get("is_sample")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    });

    if has_sample {
        return Err(kyomi_core::Error::Conflict(
            "This workspace already has a sample datasource".into(),
        ));
    }

    let connection_config = ch_config.sample_datasource_config_json();

    let ds = datasource_service::create_datasource(
        &state.db,
        workspace_id,
        "Acme Analytics (Sample)",
        Some("acme-analytics-sample"),
        "clickhouse",
        connection_config,
        None, // direct connection
    )
    .await?;

    tracing::info!(
        "Created sample datasource '{}' (id: {}) for workspace {} by user {}",
        ds.name,
        ds.id,
        workspace_id,
        user.user_id
    );

    // Trigger sample data indexing if the sentinel workspace has no cached tables.
    // Runs as a background task so the HTTP response is not delayed.
    {
        let db = state.db.clone();
        let embedding = state.embedding.clone();
        tokio::spawn(async move {
            use kyomi_auth::catalog::indexers::SampleDataIndexer;

            tracing::info!("Sample data indexing background task started");
            let count = SampleDataIndexer::get_sample_table_count(&db).await;
            if count == 0 {
                tracing::info!("Sample data index empty — triggering indexing");
                let emb = match embedding.wait_ready().await {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::error!(error = %e, "Embedding not available for sample data indexing");
                        return;
                    }
                };
                let result = SampleDataIndexer::index_sample_data(&db, emb).await;
                tracing::info!(
                    status = ?result.status,
                    tables = result.tables_indexed,
                    "sample data indexing finished"
                );
            } else {
                tracing::info!(count, "sample data already indexed — skipping");
            }
        });
    }

    let masked_config =
        credential_service::mask_connection_config(&ds.connection_config, ds.datasource_type.as_ref());

    Ok((
        StatusCode::CREATED,
        Json(DatasourceResponse {
            id: ds.id,
            slug: ds.slug,
            name: ds.name,
            datasource_type: ds.datasource_type.to_string(),
            connection_config: masked_config,
            active: ds.active,
            created_at: ds.created_at.to_rfc3339(),
            updated_at: ds.updated_at.to_rfc3339(),
            has_user_credentials: false,
            auto_refresh_allowed: ds.auto_refresh_allowed,
            connection_type: ds.connection_type,
        }),
    ))
}

// ---------------------------------------------------------------------------
// POST / — Create datasource (admin only)
// ---------------------------------------------------------------------------

async fn create_datasource(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<CreateDatasourceRequest>,
) -> Result<(StatusCode, Json<CreateDatasourceResponse>), kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;

    // Validate connection_type
    let is_connect = match request.connection_type.as_str() {
        "direct" => false,
        "connect" => true,
        other => {
            return Err(kyomi_core::Error::BadRequest(format!(
                "Invalid connection_type: '{other}'. Must be 'direct' or 'connect'."
            )));
        }
    };

    // Validate type is supported
    if !datasource_registry::is_supported_type(&request.datasource_type) {
        return Err(kyomi_core::Error::BadRequest(format!(
            "Unsupported datasource type: {}",
            request.datasource_type
        )));
    }

    // Connect datasources require the ConnectTokenService to be configured
    if is_connect {
        if state.connect_token.is_none() {
            return Err(kyomi_core::Error::BadRequest(
                "Kyomi Connect is not configured on this server".into(),
            ));
        }
        // OAuth datasources (BigQuery, Snowflake, Databricks) don't use Connect —
        // they already have a trust model via OAuth provider authorization
        match request.datasource_type.as_str() {
            "bigquery" | "snowflake" | "databricks" => {
                return Err(kyomi_core::Error::BadRequest(format!(
                    "Kyomi Connect is not supported for {} — use OAuth authentication instead",
                    request.datasource_type
                )));
            }
            _ => {}
        }
    }

    let conn_config = request.connection_config.unwrap_or(json!({}));

    // Validate required fields for SQL Server / Synapse (direct connections only)
    if !is_connect
        && (request.datasource_type == "sqlserver" || request.datasource_type == "synapse")
        && conn_config.get("database").and_then(|v| v.as_str()).unwrap_or("").is_empty()
    {
        return Err(kyomi_core::Error::BadRequest(
            "Database is required for SQL Server/Synapse. Please select a database from the discovery list.".into(),
        ));
    }

    let ds = datasource_service::create_datasource(
        &state.db,
        workspace_id,
        &request.name,
        request.slug.as_deref(),
        &request.datasource_type,
        conn_config,
        Some(&request.connection_type),
    )
    .await?;

    // For connect datasources, generate a JWT token and store the jti
    let connect_token = if is_connect {
        let service = state.connect_token.as_ref().unwrap(); // safe: checked above
        let (token, jti) = service.generate(
            &ds.id,
            workspace_id,
            ds.datasource_type.as_ref(),
        )?;
        datasource_service::update_connect_jti(&state.db, &ds.id, &jti).await?;
        Some(token)
    } else {
        None
    };

    tracing::info!(
        "Created datasource '{}' (slug: {}, id: {}, connection: {}) for workspace {} by user {}",
        ds.name,
        ds.slug,
        ds.id,
        request.connection_type,
        workspace_id,
        user.user_id
    );

    // Skip catalog indexing (Phase 7) and MCP notification (Phase 11)

    let masked_config =
        credential_service::mask_connection_config(&ds.connection_config, ds.datasource_type.as_ref());

    Ok((
        StatusCode::CREATED,
        Json(CreateDatasourceResponse {
            datasource: DatasourceResponse {
                id: ds.id,
                slug: ds.slug,
                name: ds.name,
                datasource_type: ds.datasource_type.to_string(),
                connection_config: masked_config,
                active: ds.active,
                created_at: ds.created_at.to_rfc3339(),
                updated_at: ds.updated_at.to_rfc3339(),
                has_user_credentials: false,
                auto_refresh_allowed: ds.auto_refresh_allowed,
                connection_type: ds.connection_type,
            },
            connect_token,
        }),
    ))
}

// ---------------------------------------------------------------------------
// GET /{identifier} — Get single datasource
// ---------------------------------------------------------------------------

async fn get_datasource(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
) -> Result<Json<DatasourceResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Admins can view disabled datasources to manage them
    let ds = resolve_or_404(&state.db, &identifier, workspace_id, is_admin(&user)).await?;

    // Check if user has credentials for this datasource
    let user_cred =
        datasource_service::get_user_credential(&state.db, &user.user_id, &ds.id).await?;

    let masked_config =
        credential_service::mask_connection_config(&ds.connection_config, ds.datasource_type.as_ref());

    Ok(Json(DatasourceResponse {
        id: ds.id,
        slug: ds.slug,
        name: ds.name,
        datasource_type: ds.datasource_type.to_string(),
        connection_config: masked_config,
        active: ds.active,
        created_at: ds.created_at.to_rfc3339(),
        updated_at: ds.updated_at.to_rfc3339(),
        has_user_credentials: user_cred.is_some(),
        auto_refresh_allowed: ds.auto_refresh_allowed,
        connection_type: ds.connection_type,
    }))
}

// ---------------------------------------------------------------------------
// PUT /{identifier} — Update datasource (admin only)
// ---------------------------------------------------------------------------

async fn update_datasource(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
    Json(request): Json<UpdateDatasourceRequest>,
) -> Result<Json<DatasourceResponse>, kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;

    // Admins can update disabled datasources
    let ds = resolve_or_404(&state.db, &identifier, workspace_id, true).await?;

    // Guard: sample datasource connection config cannot be modified
    let is_sample = ds
        .connection_config
        .get("is_sample")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if is_sample && request.connection_config.is_some() {
        return Err(kyomi_core::Error::BadRequest(
            "Sample datasource configuration cannot be modified".into(),
        ));
    }

    // Validate required fields for SQL Server / Synapse on connection_config update
    if let Some(ref new_config) = request.connection_config
        && (ds.datasource_type == DatasourceType::Sqlserver || ds.datasource_type == DatasourceType::Synapse)
        && new_config.get("database").and_then(|v| v.as_str()).unwrap_or("").is_empty()
    {
        return Err(kyomi_core::Error::BadRequest(
            "Database is required for SQL Server/Synapse. Please select a database from the discovery list.".into(),
        ));
    }

    // Process connection_config: merge sensitive fields
    let final_connection_config = if let Some(mut new_config) = request.connection_config {
        let existing_config = &ds.connection_config;

        // Get sensitive fields from registry + common
        let common_sensitive: &[&str] = &["shared_password", "ssh_private_key"];
        let type_specific = datasource_registry::get_metadata_by_str(ds.datasource_type.as_ref())
            .map(|m| m.sensitive_connection_config_fields)
            .unwrap_or(&[]);

        let mut all_sensitive: Vec<&str> = common_sensitive.to_vec();
        all_sensitive.extend_from_slice(type_specific);
        all_sensitive.sort_unstable();
        all_sensitive.dedup();

        // Preserve existing sensitive values if new is empty or masked
        if let Some(new_obj) = new_config.as_object_mut() {
            for field in &all_sensitive {
                let new_value = new_obj.get(*field);
                let should_preserve = match new_value {
                    None => true,
                    Some(Value::String(s)) => s.is_empty() || s == MASKED_VALUE,
                    _ => false,
                };

                if should_preserve
                    && let Some(existing_val) = existing_config.get(*field)
                    // Only preserve if existing is not itself masked
                    && existing_val.as_str() != Some(MASKED_VALUE)
                {
                    new_obj.insert(field.to_string(), existing_val.clone());
                }
            }
        }

        Some(new_config)
    } else {
        None
    };

    let updated = datasource_service::update_datasource(
        &state.db,
        &ds.id,
        workspace_id,
        request.name.as_deref(),
        request.slug.as_deref(),
        final_connection_config,
        request.active,
        request.auto_refresh_allowed,
    )
    .await?;

    tracing::info!(
        "Updated datasource '{}' (id: {}) by user {}",
        updated.name,
        updated.id,
        user.user_id
    );

    // Check if user has credentials
    let user_cred =
        datasource_service::get_user_credential(&state.db, &user.user_id, &updated.id).await?;

    let masked_config = credential_service::mask_connection_config(
        &updated.connection_config,
        updated.datasource_type.as_ref(),
    );

    Ok(Json(DatasourceResponse {
        id: updated.id,
        slug: updated.slug,
        name: updated.name,
        datasource_type: updated.datasource_type.to_string(),
        connection_config: masked_config,
        active: updated.active,
        created_at: updated.created_at.to_rfc3339(),
        updated_at: updated.updated_at.to_rfc3339(),
        has_user_credentials: user_cred.is_some(),
        auto_refresh_allowed: updated.auto_refresh_allowed,
        connection_type: updated.connection_type,
    }))
}

// ---------------------------------------------------------------------------
// DELETE /{identifier} — Delete datasource (admin only)
// ---------------------------------------------------------------------------

async fn delete_datasource_handler(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
) -> Result<StatusCode, kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;

    // Admins can delete disabled datasources
    let ds = resolve_or_404(&state.db, &identifier, workspace_id, true).await?;

    let ds_id = ds.id.clone();

    datasource_service::delete_datasource(&state.db, &ds.id, workspace_id).await?;

    tracing::info!(
        "Deleted datasource '{}' (id: {}) from workspace {} by user {}",
        ds.name,
        ds_id,
        workspace_id,
        user.user_id
    );

    // No graph cleanup needed — cascade deletes handle this

    // Skip MCP notification (Phase 11)

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// POST /{identifier}/credentials — Save user credentials
// ---------------------------------------------------------------------------

async fn save_credentials(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
    Json(request): Json<SaveCredentialsRequest>,
) -> Result<Json<CredentialsResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let ds = resolve_or_404(&state.db, &identifier, workspace_id, false).await?;
    require_active(&ds)?;

    // Save credentials (service handles merge with existing OAuth fields + encryption)
    let cred = datasource_service::save_user_credential(
        &state.db,
        &state.encryption_key,
        &user.user_id,
        &ds.id,
        workspace_id,
        &request.credentials,
    )
    .await?;

    tracing::info!(
        "Saved credentials for user {} on datasource {}",
        user.user_id,
        ds.id
    );

    // Send credential_status_changed WebSocket notification
    let ws_manager = state.ws_manager.clone();
    let ws_user_id = user.user_id.clone();
    let ws_workspace_id = workspace_id.to_string();
    let ws_slug = ds.slug.clone();
    let ws_ds_type = ds.datasource_type.to_string();
    tokio::spawn(async move {
        ws_helpers::send_credential_status_changed(
            &ws_manager,
            &ws_user_id,
            &ws_workspace_id,
            &ws_slug,
            "connected",
            &ws_ds_type,
        )
        .await;
    });

    let masked_preview =
        credential_service::mask_credentials(&request.credentials, ds.datasource_type.as_ref());

    Ok(Json(CredentialsResponse {
        datasource_id: ds.id,
        datasource_slug: ds.slug,
        datasource_name: ds.name,
        datasource_type: ds.datasource_type.to_string(),
        has_credentials: true,
        credentials_preview: masked_preview,
        created_at: Some(cred.created_at.to_rfc3339()),
        updated_at: Some(cred.updated_at.to_rfc3339()),
    }))
}

// ---------------------------------------------------------------------------
// GET /{identifier}/credentials — Get user credentials (masked)
// ---------------------------------------------------------------------------

async fn get_credentials(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
) -> Result<Json<CredentialsResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let ds = resolve_or_404(&state.db, &identifier, workspace_id, false).await?;
    require_active(&ds)?;

    let credential =
        datasource_service::get_user_credential(&state.db, &user.user_id, &ds.id).await?;

    match credential {
        Some(cred) => {
            // Decrypt and mask credentials
            let decrypted =
                credential_service::decrypt_credentials(&cred.credentials, &state.encryption_key)?;
            let masked = credential_service::mask_credentials(&decrypted, ds.datasource_type.as_ref());

            Ok(Json(CredentialsResponse {
                datasource_id: ds.id,
                datasource_slug: ds.slug,
                datasource_name: ds.name,
                datasource_type: ds.datasource_type.to_string(),
                has_credentials: true,
                credentials_preview: masked,
                created_at: Some(cred.created_at.to_rfc3339()),
                updated_at: Some(cred.updated_at.to_rfc3339()),
            }))
        }
        None => Ok(Json(CredentialsResponse {
            datasource_id: ds.id,
            datasource_slug: ds.slug,
            datasource_name: ds.name,
            datasource_type: ds.datasource_type.to_string(),
            has_credentials: false,
            credentials_preview: json!({}),
            created_at: None,
            updated_at: None,
        })),
    }
}

// ---------------------------------------------------------------------------
// DELETE /{identifier}/credentials — Delete user credentials
// ---------------------------------------------------------------------------

async fn delete_credentials(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
) -> Result<StatusCode, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let ds = resolve_or_404(&state.db, &identifier, workspace_id, false).await?;
    require_active(&ds)?;

    // Check credential exists
    let credential =
        datasource_service::get_user_credential(&state.db, &user.user_id, &ds.id).await?;
    if credential.is_none() {
        return Err(kyomi_core::Error::NotFound(
            "No credentials found for this datasource".into(),
        ));
    }

    datasource_service::delete_user_credential(&state.db, &user.user_id, &ds.id).await?;

    tracing::info!(
        "Deleted credentials for user {} on datasource {}",
        user.user_id,
        ds.id
    );

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// GET /{identifier}/settings — Get user settings with workspace defaults
// ---------------------------------------------------------------------------

async fn get_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
) -> Result<Json<UserSettingsResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let ds = resolve_or_404(&state.db, &identifier, workspace_id, is_admin(&user)).await?;

    // Get user credentials
    let user_cred =
        datasource_service::get_user_credential(&state.db, &user.user_id, &ds.id).await?;

    // Decrypt user settings from credentials
    let user_settings = match &user_cred {
        Some(cred) => {
            credential_service::decrypt_credentials(&cred.credentials, &state.encryption_key)
                .unwrap_or(json!({}))
        }
        None => json!({}),
    };

    let connection_config = &ds.connection_config;

    // Determine credential status
    let cred_result = datasource_auth_service::check_credential_status(
        ds.datasource_type.as_ref(),
        connection_config,
        user_cred.as_ref(),
        &state.encryption_key,
    );

    // Type-specific settings
    let (workspace_defaults, effective, has_oauth, oauth_email, has_bigquery_scopes,
         needs_bigquery_connect, auth_mode, enable_arrow_streaming, service_account_email) =
        match ds.datasource_type {
            DatasourceType::Bigquery => {
                let defaults = json!({
                    "default_billing_project": connection_config.get("default_billing_project"),
                    "default_project": connection_config.get("default_project"),
                });

                // Resolve effective BigQuery settings (workspace config wins)
                let billing_project =
                    kyomi_datasource_server::providers::bigquery::resolve_billing_project(
                        connection_config,
                        &user_settings,
                        None,
                    );
                let default_project = user_settings
                    .get("default_project")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        connection_config
                            .get("default_project")
                            .and_then(|v| v.as_str())
                    });

                let effective = json!({
                    "billing_project": billing_project,
                    "default_project": default_project,
                });

                let auth_mode_val = connection_config
                    .get("auth_mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("kyomi_oauth")
                    .to_string();
                let enable_arrow = connection_config
                    .get("enable_arrow_streaming")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let mut sa_email: Option<String> = None;

                let (has_o, o_email, bq_scopes, needs_connect) = match auth_mode_val.as_str() {
                    "service_account" => {
                        // Extract service account email
                        if let Some(sa_json) = connection_config.get("service_account_json").and_then(|v| v.as_str())
                            && let Ok(sa_data) = serde_json::from_str::<Value>(sa_json)
                        {
                            sa_email = sa_data.get("client_email").and_then(|v| v.as_str()).map(|s| s.to_string());
                        }
                        (true, None, true, false)
                    }
                    "enterprise_oauth" => {
                        let has_o = user_settings.get("auth_type").and_then(|v| v.as_str()) == Some("oauth");
                        let o_email = if has_o {
                            user_settings.get("oauth_email").and_then(|v| v.as_str()).map(|s| s.to_string())
                        } else {
                            None
                        };
                        (has_o, o_email, has_o, !has_o)
                    }
                    _ => {
                        // kyomi_oauth — read the user's global Google OAuth data
                        // from users.oauth_data (same as Python's _build_user_context).
                        let db_user = kyomi_auth::user_service::get_user_by_id(
                            &state.db,
                            &user.user_id,
                        )
                        .await?;

                        let oauth_data = db_user
                            .as_ref()
                            .and_then(|u| u.oauth_data.as_deref())
                            .and_then(|encrypted| {
                                kyomi_auth::google_oauth::parse_oauth_data(
                                    Some(encrypted),
                                    &state.encryption_key,
                                )
                                .ok()
                                .flatten()
                            });

                        let google_tokens = oauth_data
                            .as_ref()
                            .and_then(|o| o.google_oauth_tokens.as_ref());

                        match google_tokens {
                            Some(t) => {
                                let has_o = !t.access_token.is_empty();
                                let o_email = t.email.clone();
                                let bq_scopes =
                                    kyomi_auth::google_oauth::has_bigquery_scopes(&t.scope);
                                let has_refresh = t.refresh_token.is_some();
                                let needs_connect = !bq_scopes || !has_refresh;
                                (has_o, o_email, bq_scopes, needs_connect)
                            }
                            None => (false, None, false, true),
                        }
                    }
                };

                (
                    defaults,
                    effective,
                    has_o,
                    o_email,
                    bq_scopes,
                    needs_connect,
                    Some(auth_mode_val),
                    Some(enable_arrow),
                    sa_email,
                )
            }
            DatasourceType::Snowflake | DatasourceType::Databricks | DatasourceType::Synapse => {
                let has_o = user_settings.get("auth_type").and_then(|v| v.as_str()) == Some("oauth");
                let o_email_field = if ds.datasource_type == DatasourceType::Snowflake {
                    "oauth_username"
                } else {
                    "oauth_email"
                };
                let o_email = if has_o {
                    user_settings.get(o_email_field).and_then(|v| v.as_str()).map(|s| s.to_string())
                } else {
                    None
                };

                (
                    json!({}),
                    user_settings.clone(),
                    has_o,
                    o_email,
                    false,
                    false,
                    None,
                    None,
                    None,
                )
            }
            _ => {
                // No special handling for other types
                (
                    json!({}),
                    user_settings.clone(),
                    false,
                    None,
                    false,
                    false,
                    None,
                    None,
                    None,
                )
            }
        };

    // Mask sensitive connection config fields
    let safe_connection_config =
        credential_service::mask_connection_config(connection_config, ds.datasource_type.as_ref());

    // Credential presence flags
    let has_password = user_settings.get("password").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty());
    let has_username = user_settings.get("username").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty());
    let has_access_token = user_settings.get("access_token").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty());

    // Create safe user settings — remove sensitive credential values
    let safe_user_settings = {
        let sensitive_keys = [
            "password",
            "access_token",
            "private_key",
            "private_key_passphrase",
            "oauth_access_token",
            "oauth_refresh_token",
            "client_secret",
        ];
        match user_settings.as_object() {
            Some(obj) => {
                let filtered: serde_json::Map<String, Value> = obj
                    .iter()
                    .filter(|(k, _)| !sensitive_keys.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                Value::Object(filtered)
            }
            None => user_settings.clone(),
        }
    };

    let shared_credentials = connection_config
        .get("shared_credentials")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Ok(Json(UserSettingsResponse {
        datasource_id: ds.id,
        datasource_slug: ds.slug,
        datasource_name: ds.name,
        datasource_type: ds.datasource_type.to_string(),
        user_settings: safe_user_settings,
        workspace_defaults,
        effective_settings: effective,
        has_oauth,
        oauth_email,
        has_bigquery_scopes,
        needs_bigquery_connect,
        connection_config: safe_connection_config,
        shared_credentials,
        credential_status: cred_result.credential_status,
        auth_method: cred_result.auth_method,
        oauth_provider: cred_result.oauth_provider,
        has_password,
        has_username,
        has_access_token,
        auth_mode,
        enable_arrow_streaming,
        service_account_email,
    }))
}

// ---------------------------------------------------------------------------
// PUT /{identifier}/settings — Save user settings (alias for credentials)
// ---------------------------------------------------------------------------

async fn save_settings(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
    Json(request): Json<SaveCredentialsRequest>,
) -> Result<Json<CredentialsResponse>, kyomi_core::Error> {
    // Delegate to the existing credentials save logic
    save_credentials(State(state), user, Path(identifier), Json(request)).await
}

// ---------------------------------------------------------------------------
// POST /{identifier}/toggle — Enable/disable datasource for user
// ---------------------------------------------------------------------------

async fn toggle_datasource(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
    Json(request): Json<DatasourceToggleRequest>,
) -> Result<Json<DatasourceToggleResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let ds = resolve_or_404(&state.db, &identifier, workspace_id, false).await?;
    require_active(&ds)?;

    let connection_config = &ds.connection_config;
    let ds_type_str = ds.datasource_type.as_ref();
    let is_shared = datasource_auth_service::is_shared_auth(ds_type_str, connection_config);

    // Get user's existing credential
    let user_cred =
        datasource_service::get_user_credential(&state.db, &user.user_id, &ds.id).await?;

    if request.enabled {
        // When enabling, verify credentials are valid
        // Exception: Kyomi Connect datasources don't require user credentials (they use the Connect token)
        let is_connect = ds.connection_type == "connect";

        if is_shared || is_connect {
            // For shared auth that requires Google OAuth, we cannot fully check
            // here (no access to User.oauth_data). But the status endpoint handles this.
            // For now, always allow enabling shared auth datasources.
            //
            // Kyomi Connect datasources don't have user credentials — they use the Connect token
            // instead, so they can always be enabled.

            datasource_service::upsert_user_preference(
                &state.db,
                &user.user_id,
                &ds.id,
                true,
            )
            .await?;

            tracing::info!(
                "User {} enabled {} datasource {} (slug: {})",
                user.user_id,
                if is_connect { "connect" } else { "shared-auth" },
                ds.id,
                ds.slug
            );
        } else {
            // Personal auth — check credential status before enabling
            let result = datasource_auth_service::check_credential_status(
                ds_type_str,
                connection_config,
                user_cred.as_ref(),
                &state.encryption_key,
            );

            if result.credential_status != "valid" && result.credential_status != "shared" {
                let detail = match result.credential_status.as_str() {
                    "missing" => {
                        if result.auth_method == "oauth" {
                            format!(
                                "Cannot enable '{}': Please connect your {} account first",
                                ds.name,
                                result.oauth_provider.as_deref().unwrap_or("OAuth")
                            )
                        } else {
                            format!(
                                "Cannot enable '{}': Please enter your credentials first",
                                ds.name
                            )
                        }
                    }
                    "expired" => {
                        format!(
                            "Cannot enable '{}': Your {} connection has expired. Please reconnect.",
                            ds.name,
                            result.oauth_provider.as_deref().unwrap_or("OAuth")
                        )
                    }
                    _ => {
                        format!(
                            "Cannot enable '{}': Invalid credential status",
                            ds.name
                        )
                    }
                };
                return Err(kyomi_core::Error::BadRequest(detail));
            }

            // Update credential enabled flag
            if let Some(cred) = &user_cred {
                let sql = format!(
                    "UPDATE user_datasource_credentials \
                     SET enabled = true, updated_at = {} \
                     WHERE id = $1",
                    kyomi_core::sql_compat::now(state.db.is_postgres())
                );
                kyomi_core::db_execute!(&state.db, &sql, &cred.id)?;

                tracing::info!(
                    "User {} enabled personal-auth datasource {} (slug: {})",
                    user.user_id,
                    ds.id,
                    ds.slug
                );
            } else {
                return Err(kyomi_core::Error::BadRequest(format!(
                    "Cannot enable '{}': No credential record found",
                    ds.name
                )));
            }
        }
    } else {
        // When disabling, always succeed
        if is_shared {
            datasource_service::upsert_user_preference(
                &state.db,
                &user.user_id,
                &ds.id,
                false,
            )
            .await?;

            tracing::info!(
                "User {} disabled shared-auth datasource {} (slug: {})",
                user.user_id,
                ds.id,
                ds.slug
            );
        } else if let Some(cred) = &user_cred {
            let sql = format!(
                "UPDATE user_datasource_credentials \
                 SET enabled = false, updated_at = {} \
                 WHERE id = $1",
                kyomi_core::sql_compat::now(state.db.is_postgres())
            );
            kyomi_core::db_execute!(&state.db, &sql, &cred.id)?;

            tracing::info!(
                "User {} disabled personal-auth datasource {} (slug: {})",
                user.user_id,
                ds.id,
                ds.slug
            );
        } else {
            // No credential record — use preference for tracking
            datasource_service::upsert_user_preference(
                &state.db,
                &user.user_id,
                &ds.id,
                false,
            )
            .await?;

            tracing::info!(
                "User {} disabled datasource {} (created preference record for tracking)",
                user.user_id,
                ds.id
            );
        }
    }

    let action = if request.enabled { "enabled" } else { "disabled" };
    Ok(Json(DatasourceToggleResponse {
        id: ds.id,
        slug: ds.slug,
        enabled: request.enabled,
        message: format!("Datasource '{}' has been {action}", ds.name),
    }))
}

// ---------------------------------------------------------------------------
// POST /test-connection — Standalone test connection (no saved datasource)
// ---------------------------------------------------------------------------

async fn test_connection_standalone(
    _state: State<AppState>,
    _user: AuthUser,
    Json(request): Json<StandaloneTestConnectionRequest>,
) -> Result<Json<TestConnectionResponse>, kyomi_core::Error> {
    // Validate datasource type
    let ds_type = datasource_registry::DatasourceType::from_str(&request.datasource_type)?;

    let credentials = request.credentials.unwrap_or(json!({}));

    // Create provider and test connection with timeout
    let provider = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT,
        kyomi_datasource_server::create_provider(
            &ds_type,
            &request.connection_config,
            &credentials,
            None,
        ),
    )
    .await
    {
        Ok(Ok(provider)) => provider,
        Ok(Err(e)) => {
            tracing::warn!(
                datasource_type = %request.datasource_type,
                error = %e,
                "Failed to create provider for standalone test connection"
            );
            return Ok(Json(TestConnectionResponse {
                success: false,
                message: "Failed to connect — check your connection settings".into(),
            }));
        }
        Err(_) => {
            return Ok(Json(TestConnectionResponse {
                success: false,
                message: "Connection timed out".into(),
            }));
        }
    };

    let result = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT,
        provider.test_connection(),
    )
    .await
    {
        Ok(Ok(true)) => TestConnectionResponse {
            success: true,
            message: "Connection successful".into(),
        },
        Ok(Ok(false)) => TestConnectionResponse {
            success: false,
            message: "Connection test returned false".into(),
        },
        Ok(Err(e)) => {
            tracing::warn!(
                datasource_type = %request.datasource_type,
                error = %e,
                "Connection test failed for standalone test"
            );
            TestConnectionResponse {
                success: false,
                message: "Connection failed — check your credentials and network access".into(),
            }
        }
        Err(_) => TestConnectionResponse {
            success: false,
            message: "Connection test timed out".into(),
        },
    };

    provider.close().await;

    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// POST /{identifier}/test — Test existing datasource connection
// ---------------------------------------------------------------------------

async fn test_datasource_connection(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
    Json(_request): Json<Value>,
) -> Result<Json<TestConnectionResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let ds = resolve_or_404(&state.db, &identifier, workspace_id, false).await?;
    require_active(&ds)?;

    // Create provider — Connect datasources skip credential decryption
    let provider: Box<dyn kyomi_datasource_server::provider::DatasourceProvider> =
        if ds.connection_type == "connect" {
            Box::new(ConnectProvider::new(
                state.connect_registry.clone(),
                ds.id.clone(),
            ))
        } else {
            let ds_type: datasource_registry::DatasourceType = ds.datasource_type.into();

            // Get user credentials
            let user_cred =
                datasource_service::get_user_credential(&state.db, &user.user_id, &ds.id).await?;

            let credentials = if let Some(ref cred) = user_cred {
                credential_service::decrypt_credentials(&cred.credentials, &state.encryption_key)?
            } else {
                // For shared-credentials datasources, empty creds is fine (resolved in factory)
                json!({})
            };

            // Refresh OAuth if needed
            let credentials = kyomi_datasource_server::ensure_valid_oauth_credentials(
                &credentials,
                &ds.connection_config,
                &ds_type,
            )
            .await?;

            // Build user context for BigQuery
            let user_context = build_user_context(&state, &user).await?;
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
                Ok(Ok(provider)) => provider,
                Ok(Err(e)) => {
                    tracing::warn!(
                        datasource_id = %ds.id,
                        error = %e,
                        "Failed to create provider for existing datasource test"
                    );
                    return Ok(Json(TestConnectionResponse {
                        success: false,
                        message: "Failed to connect — check your connection settings and credentials".into(),
                    }));
                }
                Err(_) => {
                    return Ok(Json(TestConnectionResponse {
                        success: false,
                        message: "Connection timed out".into(),
                    }));
                }
            }
        };

    let result = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT,
        provider.test_connection(),
    )
    .await
    {
        Ok(Ok(true)) => TestConnectionResponse {
            success: true,
            message: "Connection successful".into(),
        },
        Ok(Ok(false)) => TestConnectionResponse {
            success: false,
            message: "Connection test returned false".into(),
        },
        Ok(Err(e)) => {
            tracing::warn!(
                datasource_id = %ds.id,
                error = %e,
                "Connection test failed for existing datasource"
            );
            TestConnectionResponse {
                success: false,
                message: "Connection failed — check your credentials and network access".into(),
            }
        }
        Err(_) => TestConnectionResponse {
            success: false,
            message: "Connection test timed out".into(),
        },
    };

    provider.close().await;

    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Shared: create a DatasourceProvider for query execution
// ---------------------------------------------------------------------------

/// Create a DatasourceProvider for a resolved datasource. Handles Connect vs
/// credential-based providers, OAuth token refresh, and connection timeouts.
async fn create_query_provider(
    state: &AppState,
    user: &AuthUser,
    ds: &kyomi_core::models::datasource::DatasourceConfig,
) -> Result<Box<dyn kyomi_datasource_server::provider::DatasourceProvider>, kyomi_core::Error> {
    if ds.connection_type == "connect" {
        return Ok(Box::new(ConnectProvider::new(
            state.connect_registry.clone(),
            ds.id.clone(),
        )));
    }

    let ds_type: datasource_registry::DatasourceType = ds.datasource_type.into();

    let user_cred =
        datasource_service::get_user_credential(&state.db, &user.user_id, &ds.id).await?;

    let credentials = if let Some(ref cred) = user_cred {
        credential_service::decrypt_credentials(&cred.credentials, &state.encryption_key)?
    } else {
        json!({})
    };

    let credentials = kyomi_datasource_server::ensure_valid_oauth_credentials(
        &credentials,
        &ds.connection_config,
        &ds_type,
    )
    .await?;

    let user_context = build_user_context(state, user).await?;
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
        Ok(Ok(p)) => Ok(p),
        Ok(Err(e)) => Err(kyomi_core::Error::Internal(format!(
            "Failed to connect to datasource: {e}"
        ))),
        Err(_) => Err(kyomi_core::Error::Internal("Connection timed out".into())),
    }
}

// ---------------------------------------------------------------------------
// POST /query/execute — Execute a SQL query against a datasource
// ---------------------------------------------------------------------------

async fn execute_query(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<ExecuteQueryRequest>,
) -> Result<Json<ExecuteQueryResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    tracing::info!(
        user = %user.user_id,
        datasource = %request.datasource,
        dry_run = request.dry_run,
        "Executing query"
    );

    // Validate limits
    let limit = request.limit.clamp(1, 10000);
    let offset = request.offset.max(0);
    let page_size = request.page_size.clamp(1, 1000);

    // Resolve datasource (by slug or UUID)
    let ds = match resolve_or_404(&state.db, &request.datasource, workspace_id, false).await {
        Ok(ds) => ds,
        Err(_) => {
            // Get available datasources for the error message
            let available =
                datasource_service::list_datasources(&state.db, workspace_id, false).await?;
            let slugs: Vec<&str> = available.iter().map(|d| d.slug.as_str()).collect();
            return Err(kyomi_core::Error::NotFound(format!(
                "Datasource '{}' not found. Available: {}",
                request.datasource,
                slugs.join(", ")
            )));
        }
    };

    // Ensure datasource is active
    if !ds.active {
        return Ok(Json(ExecuteQueryResponse {
            status: "error".into(),
            columns: None,
            rows: None,
            total_rows: None,
            page_size,
            has_more: false,
            error: Some(format!("Datasource '{}' is disabled", ds.slug)),
            message: None,
            line: None,
            column: None,
            bytes_processed: None,
            execution_time_ms: None,
        }));
    }

    // Create provider
    let provider = match create_query_provider(&state, &user, &ds).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                datasource = %request.datasource,
                error = %e,
                "Failed to create provider for query execution"
            );
            return Ok(Json(ExecuteQueryResponse {
                status: "error".into(),
                columns: None,
                rows: None,
                total_rows: None,
                page_size,
                has_more: false,
                error: Some(e.to_string()),
                message: None,
                line: None,
                column: None,
                bytes_processed: None,
                execution_time_ms: None,
            }));
        }
    };

    // Execute query or dry run
    let response = if request.dry_run {
        // Dry run — validate SQL without executing
        match tokio::time::timeout(
            kyomi_datasource_server::DATASOURCE_TIMEOUT_DRY_RUN,
            provider.dry_run(&request.sql),
        )
        .await
        {
            Ok(Ok(dr)) => {
                if dr.valid {
                    ExecuteQueryResponse {
                        status: "success".into(),
                        columns: None,
                        rows: None,
                        total_rows: None,
                        page_size,
                        has_more: false,
                        error: None,
                        message: Some(dr.message),
                        line: None,
                        column: None,
                        bytes_processed: None,
                        execution_time_ms: None,
                    }
                } else {
                    ExecuteQueryResponse {
                        status: "error".into(),
                        columns: None,
                        rows: None,
                        total_rows: None,
                        page_size,
                        has_more: false,
                        error: Some(dr.message.clone()),
                        message: Some(dr.message),
                        line: dr.line.map(|l| l as i64),
                        column: dr.column.map(|c| c as i64),
                        bytes_processed: None,
                        execution_time_ms: None,
                    }
                }
            }
            Ok(Err(e)) => {
                let msg = format!("Validation failed: {e}");
                ExecuteQueryResponse {
                    status: "error".into(),
                    columns: None,
                    rows: None,
                    total_rows: None,
                    page_size,
                    has_more: false,
                    error: Some(msg.clone()),
                    message: Some(msg),
                    line: None,
                    column: None,
                    bytes_processed: None,
                    execution_time_ms: None,
                }
            }
            Err(_) => {
                let msg = "SQL validation timed out".to_string();
                ExecuteQueryResponse {
                    status: "error".into(),
                    columns: None,
                    rows: None,
                    total_rows: None,
                    page_size,
                    has_more: false,
                    error: Some(msg.clone()),
                    message: Some(msg),
                    line: None,
                    column: None,
                    bytes_processed: None,
                    execution_time_ms: None,
                }
            }
        }
    } else {
        // Execute query
        match tokio::time::timeout(
            kyomi_datasource_server::DATASOURCE_TIMEOUT_QUERY,
            provider.execute_query(
                &request.sql,
                Some(limit as u32),
                Some(offset as u32),
                request.include_total,
            ),
        )
        .await
        {
            Ok(Ok(qr)) => {
                let columns = qr.columns.map(|cols| {
                    cols.into_iter()
                        .map(|c| ExecuteQueryColumnInfo {
                            name: c.name,
                            col_type: c.col_type.as_str().to_string(),
                        })
                        .collect()
                });

                ExecuteQueryResponse {
                    status: qr.status.as_str().into(),
                    columns,
                    rows: qr.rows,
                    total_rows: qr.total_rows,
                    page_size,
                    has_more: qr.has_more,
                    error: qr.error,
                    message: None,
                    line: None,
                    column: None,
                    bytes_processed: qr.bytes_processed,
                    execution_time_ms: qr.execution_time_ms,
                }
            }
            Ok(Err(e)) => ExecuteQueryResponse {
                status: "error".into(),
                columns: None,
                rows: None,
                total_rows: None,
                page_size,
                has_more: false,
                error: Some(format!("Query execution failed: {e}")),
                message: None,
                line: None,
                column: None,
                bytes_processed: None,
                execution_time_ms: None,
            },
            Err(_) => ExecuteQueryResponse {
                status: "error".into(),
                columns: None,
                rows: None,
                total_rows: None,
                page_size,
                has_more: false,
                error: Some(format!(
                    "Query timed out after {} seconds",
                    kyomi_datasource_server::DATASOURCE_TIMEOUT_QUERY.as_secs()
                )),
                message: None,
                line: None,
                column: None,
                bytes_processed: None,
                execution_time_ms: None,
            },
        }
    };

    provider.close().await;

    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// POST /query/stream — Execute a SQL query with streaming results via WebSocket
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct StreamQueryResponse {
    request_id: String,
    status: String,
}

async fn execute_query_stream(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<ExecuteQueryRequest>,
) -> Result<Json<StreamQueryResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    tracing::info!(
        user = %user.user_id,
        datasource = %request.datasource,
        "Streaming query"
    );

    // Validate limits
    let limit = request.limit.clamp(1, 10000);
    let offset = request.offset.max(0);

    // Resolve datasource
    let ds = match resolve_or_404(&state.db, &request.datasource, workspace_id, false).await {
        Ok(ds) => ds,
        Err(_) => {
            let available =
                datasource_service::list_datasources(&state.db, workspace_id, false).await?;
            let slugs: Vec<&str> = available.iter().map(|d| d.slug.as_str()).collect();
            return Err(kyomi_core::Error::NotFound(format!(
                "Datasource '{}' not found. Available: {}",
                request.datasource,
                slugs.join(", ")
            )));
        }
    };

    if !ds.active {
        return Err(kyomi_core::Error::BadRequest(format!(
            "Datasource '{}' is disabled",
            ds.slug
        )));
    }

    // Create provider
    let provider = create_query_provider(&state, &user, &ds).await?;

    // Use client-provided request ID if available, otherwise generate one.
    let request_id = request
        .request_id
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Spawn the streaming task — iterates the query stream and sends WS messages
    let user_id = user.user_id.clone();
    let ws_manager = state.ws_manager.clone();
    let sql = request.sql.clone();
    let rid = request_id.clone();

    tokio::spawn(async move {
        let start = std::time::Instant::now();

        let mut stream = match provider
            .execute_query_stream(
                &sql,
                Some(limit as u32),
                Some(offset as u32),
                request.include_total,
                None,
            )
            .await
        {
            Ok(s) => s,
            Err(e) => {
                let msg = WebSocketMessage::new(MessageType::QueryStreamError)
                    .with_data(json!({
                        "request_id": rid,
                        "error": e.to_string(),
                    }));
                ws_manager.send_to_user(&user_id, msg).await;
                return;
            }
        };

        // Per-event timeout: if no event arrives within 120s the datasource
        // is likely disconnected or the query is hung.
        let event_timeout = std::time::Duration::from_secs(120);
        let mut completed = false;

        loop {
            let next = tokio::time::timeout(event_timeout, stream.next()).await;

            let event = match next {
                Ok(Some(event)) => event,
                Ok(None) => break, // Stream ended
                Err(_) => {
                    // Timeout waiting for next event
                    let msg = WebSocketMessage::new(MessageType::QueryStreamError)
                        .with_data(json!({
                            "request_id": rid,
                            "error": "Query timed out — the datasource may be disconnected or the query is taking too long",
                        }));
                    ws_manager.send_to_user(&user_id, msg).await;
                    break;
                }
            };

            match event {
                Ok(QueryStreamEvent::Header { columns, total_rows }) => {
                    let cols: Vec<Value> = columns
                        .iter()
                        .map(|c| json!({ "name": c.name, "type": c.col_type.as_str() }))
                        .collect();

                    let msg = WebSocketMessage::new(MessageType::QueryStreamHeader)
                        .with_data(json!({
                            "request_id": rid,
                            "columns": cols,
                            "total_rows": total_rows,
                        }));
                    ws_manager.send_to_user(&user_id, msg).await;
                }
                Ok(QueryStreamEvent::Chunk { rows, chunk_index }) => {
                    let msg = WebSocketMessage::new(MessageType::QueryStreamChunk)
                        .with_data(json!({
                            "request_id": rid,
                            "rows": rows,
                            "chunk_index": chunk_index,
                        }));
                    ws_manager.send_to_user(&user_id, msg).await;
                }
                Ok(QueryStreamEvent::Complete {
                    execution_time_ms,
                    bytes_processed,
                    total_chunks,
                    total_rows_returned,
                }) => {
                    completed = true;
                    let elapsed = start.elapsed().as_millis() as i64;
                    let msg = WebSocketMessage::new(MessageType::QueryStreamComplete)
                        .with_data(json!({
                            "request_id": rid,
                            "execution_time_ms": execution_time_ms.unwrap_or(elapsed),
                            "bytes_processed": bytes_processed,
                            "total_chunks": total_chunks,
                            "total_rows_returned": total_rows_returned,
                        }));
                    ws_manager.send_to_user(&user_id, msg).await;
                    break;
                }
                Err(e) => {
                    let msg = WebSocketMessage::new(MessageType::QueryStreamError)
                        .with_data(json!({
                            "request_id": rid,
                            "error": e.to_string(),
                        }));
                    ws_manager.send_to_user(&user_id, msg).await;
                    break;
                }
            }
        }

        // If the stream ended without a Complete event (e.g., datasource
        // disconnected mid-query, handler dropped the channel), notify the
        // frontend so it doesn't stay stuck in "streaming" state.
        if !completed {
            let msg = WebSocketMessage::new(MessageType::QueryStreamError)
                .with_data(json!({
                    "request_id": rid,
                    "error": "Query stream ended unexpectedly — the datasource may have disconnected",
                }));
            ws_manager.send_to_user(&user_id, msg).await;
        }

        provider.close().await;
    });

    Ok(Json(StreamQueryResponse {
        request_id,
        status: "streaming".into(),
    }))
}

// ---------------------------------------------------------------------------
// Helper: Build UserContext for BigQuery
// ---------------------------------------------------------------------------

/// Build a `UserContext` for BigQuery provider creation.
///
/// Loads the user's `oauth_data` from the DB and decrypts it.
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

// ---------------------------------------------------------------------------
// POST /{identifier}/generate-ssh-key — Generate SSH keypair (admin, postgres only)
// ---------------------------------------------------------------------------

async fn generate_ssh_key(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
) -> Result<Json<SSHKeyResponse>, kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;

    let ds = resolve_or_404(&state.db, &identifier, workspace_id, false).await?;

    // Only supported for PostgreSQL
    if ds.datasource_type != DatasourceType::Postgres {
        return Err(kyomi_core::Error::BadRequest(
            "SSH key generation is only supported for PostgreSQL datasources".into(),
        ));
    }

    // Generate Ed25519 keypair from random bytes
    let secret_bytes: [u8; 32] = rand::random();
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
    let verifying_key = signing_key.verifying_key();

    // Format private key as OpenSSH PEM
    let private_key_bytes = signing_key.to_bytes();
    let public_key_bytes = verifying_key.to_bytes();

    // Build OpenSSH-format public key: "ssh-ed25519 <base64> kyomi-generated"
    // The public key blob is: string "ssh-ed25519" + string <32-byte key>
    let mut pubkey_blob = Vec::new();
    let key_type = b"ssh-ed25519";
    pubkey_blob.extend_from_slice(&(key_type.len() as u32).to_be_bytes());
    pubkey_blob.extend_from_slice(key_type);
    pubkey_blob.extend_from_slice(&(public_key_bytes.len() as u32).to_be_bytes());
    pubkey_blob.extend_from_slice(&public_key_bytes);

    let public_key = format!(
        "ssh-ed25519 {} kyomi-generated",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &pubkey_blob)
    );

    // Build OpenSSH-format private key (PEM format)
    // This matches what paramiko generates in Python
    let private_key_pem = build_openssh_ed25519_private_key(&private_key_bytes, &public_key_bytes);

    // Store keys in connection_config
    let mut config = ds.connection_config.clone();
    if let Some(obj) = config.as_object_mut() {
        obj.insert("ssh_private_key".to_string(), json!(private_key_pem));
        obj.insert("ssh_public_key".to_string(), json!(public_key));
    }

    // Update datasource
    datasource_service::update_datasource(
        &state.db,
        &ds.id,
        workspace_id,
        None,
        None,
        Some(config),
        None,
        None,
    )
    .await?;

    tracing::info!(
        "Generated SSH keypair for datasource '{}' (id: {}) by user {}",
        ds.name,
        ds.id,
        user.user_id
    );

    Ok(Json(SSHKeyResponse {
        public_key,
        key_type: "ed25519".to_string(),
        message: "Add this public key to your bastion server's ~/.ssh/authorized_keys file. \
                  Then configure the SSH host, port, and username in the datasource settings."
            .to_string(),
    }))
}

/// Build an OpenSSH-format PEM private key for Ed25519.
///
/// This produces output compatible with `paramiko.Ed25519Key.generate()`
/// and `ssh-keygen -t ed25519`.
fn build_openssh_ed25519_private_key(private_bytes: &[u8; 32], public_bytes: &[u8; 32]) -> String {
    use base64::Engine;

    let auth_magic = b"openssh-key-v1\0";
    let cipher_name = b"none";
    let kdf_name = b"none";
    let kdf_options = b"";
    let num_keys: u32 = 1;

    // Build the public key section
    let key_type = b"ssh-ed25519";
    let mut pubkey_section = Vec::new();
    pubkey_section.extend_from_slice(&(key_type.len() as u32).to_be_bytes());
    pubkey_section.extend_from_slice(key_type);
    pubkey_section.extend_from_slice(&(public_bytes.len() as u32).to_be_bytes());
    pubkey_section.extend_from_slice(public_bytes);

    // Build the private key section
    // checkint (random, repeated twice for verification)
    let check: u32 = rand::random();
    let mut privkey_section = Vec::new();
    privkey_section.extend_from_slice(&check.to_be_bytes());
    privkey_section.extend_from_slice(&check.to_be_bytes());
    // key type
    privkey_section.extend_from_slice(&(key_type.len() as u32).to_be_bytes());
    privkey_section.extend_from_slice(key_type);
    // public key
    privkey_section.extend_from_slice(&(public_bytes.len() as u32).to_be_bytes());
    privkey_section.extend_from_slice(public_bytes);
    // private key (64 bytes: 32 private + 32 public concatenated, per OpenSSH format)
    let mut combined_key = Vec::with_capacity(64);
    combined_key.extend_from_slice(private_bytes);
    combined_key.extend_from_slice(public_bytes);
    privkey_section.extend_from_slice(&(combined_key.len() as u32).to_be_bytes());
    privkey_section.extend_from_slice(&combined_key);
    // comment (empty)
    privkey_section.extend_from_slice(&0u32.to_be_bytes());
    // padding (1, 2, 3, ... up to block size alignment — block size for "none" cipher is 8)
    let block_size = 8;
    let padding_len = (block_size - (privkey_section.len() % block_size)) % block_size;
    for i in 0..padding_len {
        privkey_section.push((i + 1) as u8);
    }

    // Assemble the full key blob
    let mut blob = Vec::new();
    blob.extend_from_slice(auth_magic);
    // cipher name
    blob.extend_from_slice(&(cipher_name.len() as u32).to_be_bytes());
    blob.extend_from_slice(cipher_name);
    // kdf name
    blob.extend_from_slice(&(kdf_name.len() as u32).to_be_bytes());
    blob.extend_from_slice(kdf_name);
    // kdf options
    blob.extend_from_slice(&(kdf_options.len() as u32).to_be_bytes());
    blob.extend_from_slice(kdf_options);
    // number of keys
    blob.extend_from_slice(&num_keys.to_be_bytes());
    // public key section
    blob.extend_from_slice(&(pubkey_section.len() as u32).to_be_bytes());
    blob.extend_from_slice(&pubkey_section);
    // private key section
    blob.extend_from_slice(&(privkey_section.len() as u32).to_be_bytes());
    blob.extend_from_slice(&privkey_section);

    // Encode as base64 and wrap in PEM
    let b64 = base64::engine::general_purpose::STANDARD.encode(&blob);

    // Wrap at 70 chars per line
    let wrapped: Vec<&str> = b64
        .as_bytes()
        .chunks(70)
        .map(|chunk| std::str::from_utf8(chunk).expect("base64 is ASCII"))
        .collect();

    format!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
        wrapped.join("\n")
    )
}

// ---------------------------------------------------------------------------
// GET /{identifier}/affected-users — Check affected users for auth mode change
// ---------------------------------------------------------------------------

async fn get_affected_users(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
    Query(params): Query<AffectedUsersParams>,
) -> Result<Json<AffectedUsersResponse>, kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;

    // Validate auth mode
    let valid_auth_modes = [
        "kyomi_oauth",
        "enterprise_oauth",
        "oauth",
        "service_account",
        "password",
        "sql",
        "token",
        "service_principal",
        "keypair",
        "none",
    ];

    if !valid_auth_modes.contains(&params.new_auth_mode.as_str()) {
        return Err(kyomi_core::Error::BadRequest(format!(
            "Invalid auth_mode: {}. Must be one of: {}",
            params.new_auth_mode,
            valid_auth_modes.join(", ")
        )));
    }

    let ds = resolve_or_404(&state.db, &identifier, workspace_id, true).await?;

    let connection_config = &ds.connection_config;
    let current_auth_mode = connection_config
        .get("auth_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // If switching to the same mode, no users affected
    if current_auth_mode == params.new_auth_mode {
        return Ok(Json(AffectedUsersResponse {
            affected_count: 0,
            affected_users: vec![],
            warning_message: None,
        }));
    }

    let new_mode_is_oauth = matches!(
        params.new_auth_mode.as_str(),
        "kyomi_oauth" | "enterprise_oauth" | "oauth"
    );
    let new_mode_is_shared = matches!(params.new_auth_mode.as_str(), "service_account" | "none")
        || connection_config
            .get("shared_credentials")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

    // Query all user credentials for this datasource
    let user_credentials = kyomi_core::db_fetch_all!(
        &state.db,
        kyomi_core::models::datasource::UserDatasourceCredential,
        "SELECT id, user_id, datasource_config_id, workspace_id, credentials, \
         enabled, created_at, updated_at \
         FROM user_datasource_credentials \
         WHERE datasource_config_id = $1",
        &ds.id
    )?;

    let mut affected_users = Vec::new();

    for cred in &user_credentials {
        // Decrypt credentials to check auth_type
        let cred_data = credential_service::decrypt_credentials(
            &cred.credentials,
            &state.encryption_key,
        )
        .unwrap_or(json!({}));

        let cred_auth_type = cred_data
            .get("auth_type")
            .and_then(|v| v.as_str())
            .unwrap_or("password");

        let is_oauth_cred = cred_auth_type == "oauth";

        let affected = if is_oauth_cred {
            // OAuth credential becomes invalid when switching away from OAuth mode
            !new_mode_is_oauth
        } else {
            // Password/token credential becomes invalid when switching to OAuth or shared mode
            new_mode_is_oauth || new_mode_is_shared
        };

        if affected {
            // Get user email
            #[derive(sqlx::FromRow)]
            struct EmailRow { email: String }
            let user_email: Option<String> = kyomi_core::db_fetch_optional!(
                &state.db, EmailRow,
                "SELECT email FROM users WHERE user_id = $1",
                &cred.user_id
            )?
            .map(|r| r.email);

            if let Some(email) = user_email {
                affected_users.push(AffectedUserInfo {
                    email,
                    auth_type: if is_oauth_cred {
                        "oauth".to_string()
                    } else {
                        "password".to_string()
                    },
                });
            }
        }
    }

    let affected_count = affected_users.len() as i32;

    let warning_message = if affected_count > 0 {
        Some(if affected_count == 1 {
            "1 user has existing credentials that will become invalid. They will need to set up new credentials after this change.".to_string()
        } else {
            format!("{affected_count} users have existing credentials that will become invalid. They will need to set up new credentials after this change.")
        })
    } else {
        None
    };

    tracing::info!(
        "Auth mode change check for datasource {}: {} -> {}, {} users affected",
        ds.slug,
        current_auth_mode,
        params.new_auth_mode,
        affected_count
    );

    Ok(Json(AffectedUsersResponse {
        affected_count,
        affected_users,
        warning_message,
    }))
}

// ---------------------------------------------------------------------------
// Kyomi Connect management endpoints
// ---------------------------------------------------------------------------

/// Response for Connect token operations.
#[derive(Serialize)]
struct ConnectTokenResponse {
    token: String,
}

/// Response for Connect status checks.
#[derive(Serialize)]
struct ConnectStatusResponse {
    connected: bool,
    last_seen: Option<String>,
}

// ---------------------------------------------------------------------------
// POST /{identifier}/connect/rotate-token — Rotate Connect token (admin only)
// ---------------------------------------------------------------------------

async fn rotate_connect_token(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
) -> Result<Json<ConnectTokenResponse>, kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;

    let ds = resolve_or_404(&state.db, &identifier, workspace_id, true).await?;

    // Verify this is a Connect datasource
    if ds.connection_type != "connect" {
        return Err(kyomi_core::Error::BadRequest(
            "Token rotation is only available for Connect datasources".into(),
        ));
    }

    let service = state.connect_token.as_ref().ok_or_else(|| {
        kyomi_core::Error::BadRequest("Kyomi Connect is not configured on this server".into())
    })?;

    let (token, jti) = service.generate(
        &ds.id,
        workspace_id,
        ds.datasource_type.as_ref(),
    )?;

    datasource_service::update_connect_jti(&state.db, &ds.id, &jti).await?;

    tracing::info!(
        "Rotated Connect token for datasource '{}' (id: {}) by user {}",
        ds.slug,
        ds.id,
        user.user_id
    );

    Ok(Json(ConnectTokenResponse { token }))
}

// ---------------------------------------------------------------------------
// POST /{identifier}/connect/disconnect — Disconnect Connect (admin only)
// ---------------------------------------------------------------------------

async fn disconnect_connect(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    require_workspace_admin(&user)?;
    let workspace_id = get_workspace_id(&user)?;

    let ds = resolve_or_404(&state.db, &identifier, workspace_id, true).await?;

    // Verify this is a Connect datasource
    if ds.connection_type != "connect" {
        return Err(kyomi_core::Error::BadRequest(
            "Disconnect is only available for Connect datasources".into(),
        ));
    }

    datasource_service::clear_connect_jti(&state.db, &ds.id).await?;

    tracing::info!(
        "Disconnected Connect datasource '{}' (id: {}) by user {}",
        ds.slug,
        ds.id,
        user.user_id
    );

    Ok(Json(json!({ "message": "Connect token revoked" })))
}

// ---------------------------------------------------------------------------
// GET /{identifier}/connect/status — Check Connect agent status
// ---------------------------------------------------------------------------

async fn connect_status(
    State(state): State<AppState>,
    user: AuthUser,
    Path(identifier): Path<String>,
) -> Result<Json<ConnectStatusResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let ds = resolve_or_404(&state.db, &identifier, workspace_id, true).await?;

    // Verify this is a Connect datasource
    if ds.connection_type != "connect" {
        return Err(kyomi_core::Error::BadRequest(
            "Connect status is only available for Connect datasources".into(),
        ));
    }

    // Check Redis for the Connect agent presence key.
    // The key stores a connection_id (u64) with a 60s TTL refreshed by heartbeats.
    // If the key exists, the agent is currently connected.
    // In single-instance mode (no Redis), always report disconnected.
    let Some(mut redis) = state.redis.clone() else {
        return Ok(Json(ConnectStatusResponse {
            connected: false,
            last_seen: None,
        }));
    };
    let redis_key = format!("connect:{}", ds.id);
    let presence: Option<String> = redis::cmd("GET")
        .arg(&redis_key)
        .query_async(&mut redis)
        .await
        .unwrap_or(None);

    let connected = presence.is_some();
    // When connected, the agent was just seen (heartbeat refreshes the key every 30s).
    let last_seen = if connected {
        Some(chrono::Utc::now().to_rfc3339())
    } else {
        None
    };

    Ok(Json(ConnectStatusResponse {
        connected,
        last_seen,
    }))
}

