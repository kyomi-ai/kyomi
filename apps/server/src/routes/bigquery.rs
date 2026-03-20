// SPDX-License-Identifier: AGPL-3.0-or-later

//! BigQuery endpoints — catalog browsing, search, info, access tokens, and Arrow streaming.
//!
//! Wire-compatible with Python's `/api/v1/bigquery/*` endpoints.
//!
//! ## Endpoints
//!
//! - `POST /request-access-token` — Short-lived access token for direct BigQuery API calls
//! - `POST /read-arrow` — BigQuery Arrow streaming (REST→Arrow IPC conversion)
//! - `GET /catalog` — Full catalog tree from cache
//! - `GET /catalog/projects` — Project list with dataset counts
//! - `GET /catalog/{project}/datasets` — Datasets in a project
//! - `GET /catalog/{project}/{dataset}/tables` — Tables in a dataset
//! - `GET /catalog/{project}/{dataset}/{table}/columns` — Column details
//! - `GET /catalog/status` — Refresh status
//! - `POST /catalog/refresh` — Trigger catalog refresh (non-blocking)
//! - `POST /catalog/projects/add` — Add projects to catalog config
//! - `POST /catalog/projects/remove` — Remove project from catalog
//! - `POST /catalog/settings` — Deprecated endpoint (returns 400)
//! - `GET /projects/listAccessible` — List GCP projects user has BigQuery access to
//! - `POST /search` — Semantic vector search across cached tables
//! - `POST /info` — Table metadata from cache

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use kyomi_auth::middleware::AuthUser;
use kyomi_core::models::table_cache::DatasourceTableCache;

use crate::state::AppState;

/// Build the `/bigquery` router.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Phase 6F — token + arrow
        .route("/request-access-token", post(request_access_token))
        .route("/read-arrow", post(read_arrow))
        // Phase 7F — catalog browsing
        .route("/catalog", get(get_catalog))
        .route("/catalog/projects", get(get_catalog_projects))
        .route("/catalog/status", get(get_catalog_status))
        .route("/catalog/refresh", post(catalog_refresh))
        .route("/catalog/projects/add", post(catalog_projects_add))
        .route("/catalog/projects/remove", post(catalog_projects_remove))
        .route("/catalog/settings", post(catalog_settings_deprecated))
        .route(
            "/catalog/{project}/datasets",
            get(get_catalog_datasets),
        )
        .route(
            "/catalog/{project}/{dataset}/tables",
            get(get_catalog_tables),
        )
        .route(
            "/catalog/{project}/{dataset}/{table}/columns",
            get(get_catalog_columns),
        )
        // Phase 7F — search + info
        .route("/search", post(search_tables))
        .route("/info", post(get_table_info))
        // Phase 7F — GCP projects
        .route("/projects/listAccessible", get(list_accessible_projects))
}

// ===========================================================================
// Constants
// ===========================================================================

/// BigQuery on-demand pricing: $6.25 per TB (10^12 bytes).
const BIGQUERY_COST_PER_BYTE: f64 = 6.25 / 1_000_000_000_000.0;

/// BigQuery REST API base URL.
const BIGQUERY_API_BASE: &str = "https://bigquery.googleapis.com/bigquery/v2";

// ===========================================================================
// Helpers
// ===========================================================================

/// Calculate a BigQuery cost estimate from bytes processed.
///
/// Uses on-demand pricing of $6.25 per TB (10^12 bytes).
fn calculate_cost_estimate(bytes_processed: i64) -> CostEstimate {
    let estimated_cost_usd = bytes_processed as f64 * BIGQUERY_COST_PER_BYTE;
    CostEstimate {
        bytes_processed,
        estimated_cost_usd,
    }
}

/// Perform a BigQuery dry run via the REST API and return the cost estimate.
///
/// Calls `POST /bigquery/v2/projects/{project}/queries` with `dryRun: true`.
/// Extracts `totalBytesProcessed` from the response and computes an estimated
/// cost in USD.
async fn bigquery_dry_run_cost(
    access_token: &str,
    billing_project: &str,
    sql: &str,
) -> Result<CostEstimate, kyomi_core::Error> {
    let client = kyomi_datasource_server::http_client()?;
    let url = format!("{BIGQUERY_API_BASE}/projects/{billing_project}/queries");

    let body = serde_json::json!({
        "query": sql,
        "dryRun": true,
        "useLegacySql": false,
    });

    let response = tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_DRY_RUN,
        client
            .post(&url)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .json(&body)
            .send(),
    )
    .await
    .map_err(|_| {
        kyomi_core::Error::Internal("BigQuery dry run timed out".into())
    })?
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("BigQuery dry run HTTP request failed: {e}"))
    })?;

    let status_code = response.status();
    let response_body: serde_json::Value = response.json().await.map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to parse BigQuery dry run response: {e}"))
    })?;

    if status_code.is_client_error() || status_code.is_server_error() {
        let msg = response_body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("BigQuery dry run failed");
        return Err(kyomi_core::Error::Internal(format!(
            "BigQuery dry run error: {msg}"
        )));
    }

    // The queries API returns totalBytesProcessed at the top level for dry runs.
    let bytes_processed = response_body
        .get("totalBytesProcessed")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .or_else(|| v.as_i64())
        })
        .unwrap_or(0);

    Ok(calculate_cost_estimate(bytes_processed))
}

/// Extract workspace_id from user, or return 400.
fn get_workspace_id(user: &AuthUser) -> Result<&str, kyomi_core::Error> {
    user.workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest("User not associated with a workspace".into())
        })
}

/// Fetch all non-archived table cache rows for a workspace, optionally filtered
/// by a specific datasource_config_id.
async fn fetch_cached_tables(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    datasource_config_id: Option<&str>,
) -> Result<Vec<DatasourceTableCache>, kyomi_core::Error> {
    let tables: Vec<DatasourceTableCache> = if let Some(ds_id) = datasource_config_id {
        kyomi_core::db_fetch_all!(
            db, DatasourceTableCache,
            "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
             table_metadata, column_descriptions, \
             created_at, updated_at, \
             structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
             FROM datasource_table_cache \
             WHERE workspace_id = $1 AND datasource_config_id = $2 AND is_archived = false",
            workspace_id,
            ds_id
        )?
    } else {
        kyomi_core::db_fetch_all!(
            db, DatasourceTableCache,
            "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
             table_metadata, column_descriptions, \
             created_at, updated_at, \
             structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
             FROM datasource_table_cache \
             WHERE workspace_id = $1 AND is_archived = false",
            workspace_id
        )?
    };
    Ok(tables)
}

/// Find the first active BigQuery datasource in a workspace, returning its
/// config ID and connection_config.
async fn find_bq_datasource(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
) -> Result<
    Option<kyomi_core::models::datasource::DatasourceConfig>,
    kyomi_core::Error,
> {
    let ds = kyomi_core::db_fetch_optional!(
        db, kyomi_core::models::datasource::DatasourceConfig,
        "SELECT id, workspace_id, name, slug, \
         datasource_type, connection_config, \
         active, connection_type, connect_token_jti, \
         created_at, updated_at, \
         last_catalog_refresh, auto_refresh_allowed \
         FROM datasource_configs \
         WHERE workspace_id = $1 AND datasource_type = 'bigquery' AND active = true \
         LIMIT 1",
        workspace_id
    )?;
    Ok(ds)
}

/// Extract description from table_metadata, trying both `table_description`
/// and `description` keys (matching Python behavior).
fn extract_description(metadata: &Value) -> String {
    metadata
        .get("table_description")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            metadata
                .get("description")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("")
        .to_string()
}

// ===========================================================================
// Shared auth helpers — used by both /request-access-token and /read-arrow
// ===========================================================================

/// Resolved BigQuery access context: token + billing project + expiry.
struct BqAccessContext {
    access_token: String,
    billing_project: String,
    expires_at: String,
}

/// Resolve the BigQuery access token and billing project for a user.
///
/// Supports all three auth modes (kyomi_oauth, enterprise_oauth, service_account).
/// This is the single source of truth for BigQuery token resolution, used by both
/// the `/request-access-token` and `/read-arrow` endpoints.
///
/// For OAuth modes, this function checks token expiry and transparently refreshes
/// expired tokens (matching Python's `get_oauth_credentials()` behavior):
/// - `kyomi_oauth`: refreshes using the app's Google OAuth client credentials
/// - `enterprise_oauth`: refreshes using per-datasource OAuth client credentials
async fn resolve_bq_access(
    db: &kyomi_core::DbPool,
    user: &AuthUser,
    ds: &kyomi_core::models::datasource::DatasourceConfig,
    encryption_key: &[u8; 32],
    config: &kyomi_core::Config,
) -> Result<BqAccessContext, kyomi_core::Error> {
    let connection_config = &ds.connection_config;
    let auth_mode = connection_config
        .get("auth_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("kyomi_oauth");

    // Resolve user-level credentials for billing project override
    let user_cred =
        kyomi_auth::datasource_service::get_user_credential(db, &user.user_id, &ds.id).await?;

    let user_cred_data = if let Some(ref cred) = user_cred {
        kyomi_auth::credential_service::decrypt_credentials(&cred.credentials, encryption_key).ok()
    } else {
        None
    };

    let (access_token, billing_project, expires_at) = match auth_mode {
        "kyomi_oauth" => {
            let client_id = config.google_oauth_client_id.as_deref().ok_or_else(|| {
                kyomi_core::Error::Internal("GOOGLE_OAUTH_CLIENT_ID not configured".into())
            })?;
            let client_secret = config.google_oauth_client_secret.as_deref().ok_or_else(|| {
                kyomi_core::Error::Internal("GOOGLE_OAUTH_CLIENT_SECRET not configured".into())
            })?;

            // Centralized token resolution: reads DB, checks expiry, refreshes, persists
            let tokens = kyomi_auth::google_oauth::ensure_valid_google_token(
                db,
                &user.user_id,
                encryption_key,
                client_id,
                client_secret,
            )
            .await?;

            let token = tokens.access_token.clone();

            let bp = kyomi_datasource_server::providers::bigquery::resolve_billing_project(
                connection_config,
                user_cred_data.as_ref().unwrap_or(&serde_json::json!({})),
                None,
            )
            .unwrap_or_default();

            let expires = tokens.expires_at.unwrap_or_else(|| {
                (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339()
            });

            (token, bp, expires)
        }
        "enterprise_oauth" => {
            let cred_data = user_cred_data.ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "No credentials found for this datasource. Please configure OAuth.".into(),
                )
            })?;

            // Refresh enterprise OAuth token if expired (uses per-datasource credentials)
            let ds_type = kyomi_core::datasource_registry::DatasourceType::BigQuery;
            let cred_data =
                match kyomi_datasource_server::ensure_valid_oauth_credentials(
                    &cred_data,
                    connection_config,
                    &ds_type,
                )
                .await
                {
                    Ok(refreshed) if refreshed != cred_data => {
                        // Persist refreshed token back to DB
                        if let Some(ref cred) = user_cred {
                            if let Err(e) = kyomi_auth::datasource_service::save_user_credential(
                                db,
                                encryption_key,
                                &user.user_id,
                                &ds.id,
                                &cred.workspace_id,
                                &refreshed,
                            )
                            .await
                            {
                                tracing::warn!(
                                    datasource_id = %ds.id,
                                    "Failed to persist refreshed enterprise OAuth token: {e}"
                                );
                            } else {
                                tracing::info!(
                                    user_id = %user.user_id,
                                    datasource_id = %ds.id,
                                    "Enterprise BigQuery OAuth token refreshed and persisted"
                                );
                            }
                        }
                        refreshed
                    }
                    Ok(unchanged) => unchanged,
                    Err(e) => {
                        tracing::warn!(
                            datasource_id = %ds.id,
                            "Enterprise OAuth refresh failed, using existing token: {e}"
                        );
                        cred_data
                    }
                };

            let token = cred_data
                .get("oauth_access_token")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    kyomi_core::Error::BadRequest(
                        "No OAuth access token found in credentials".into(),
                    )
                })?
                .to_string();

            let bp = kyomi_datasource_server::providers::bigquery::resolve_billing_project(
                connection_config,
                &cred_data,
                None,
            )
            .unwrap_or_default();

            let expires = cred_data
                .get("oauth_token_expiry")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    // No expiry stored — this shouldn't happen after a refresh,
                    // but provide a fallback
                    ""
                })
                .to_string();

            let expires = if expires.is_empty() {
                (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339()
            } else {
                expires
            };

            (token, bp, expires)
        }
        "service_account" => {
            let client = kyomi_datasource_server::http_client()?;
            let (token, project_id) =
                kyomi_datasource_server::providers::bigquery::exchange_service_account_jwt(
                    &client,
                    connection_config,
                )
                .await?;

            let bp = kyomi_datasource_server::providers::bigquery::resolve_billing_project(
                connection_config,
                user_cred_data.as_ref().unwrap_or(&serde_json::json!({})),
                Some(&project_id),
            )
            .unwrap_or(project_id);

            let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();

            (token, bp, expires)
        }
        other => {
            return Err(kyomi_core::Error::BadRequest(format!(
                "Unsupported BigQuery auth mode: {other}"
            )));
        }
    };

    if billing_project.is_empty() {
        return Err(kyomi_core::Error::BadRequest(
            "No billing project configured. Please set a billing project in datasource settings."
                .into(),
        ));
    }

    Ok(BqAccessContext {
        access_token,
        billing_project,
        expires_at,
    })
}

/// Look up a BigQuery datasource by slug and validate it is type 'bigquery'.
async fn lookup_bq_datasource(
    db: &kyomi_core::DbPool,
    datasource_slug: &str,
    workspace_id: &str,
) -> Result<kyomi_core::models::datasource::DatasourceConfig, kyomi_core::Error> {
    let ds = kyomi_auth::datasource_service::get_datasource_by_slug(
        db,
        datasource_slug,
        workspace_id,
    )
    .await?
    .ok_or_else(|| {
        kyomi_core::Error::NotFound(format!(
            "BigQuery datasource '{}' not found",
            datasource_slug
        ))
    })?;

    if ds.datasource_type != kyomi_core::DatasourceType::Bigquery {
        return Err(kyomi_core::Error::BadRequest(format!(
            "Datasource '{}' is type '{}', not 'bigquery'",
            datasource_slug, ds.datasource_type
        )));
    }

    Ok(ds)
}

// ===========================================================================
// Request / Response Types — Phase 6F (token + arrow)
// ===========================================================================

#[derive(Deserialize)]
struct AccessTokenRequest {
    datasource_slug: String,
    /// Optional SQL query — when provided, the endpoint performs a BigQuery
    /// dry run and returns a `cost_estimate` alongside the access token.
    #[serde(default)]
    query: Option<String>,
}

/// Cost estimate returned from a BigQuery dry run.
///
/// Matches the Python response schema: `{ bytes_processed: i64, estimated_cost_usd: f64 }`.
#[derive(Debug, Clone, Serialize, PartialEq)]
struct CostEstimate {
    bytes_processed: i64,
    estimated_cost_usd: f64,
}

#[derive(Serialize)]
struct AccessTokenResponse {
    access_token: String,
    expires_at: String,
    billing_project: String,
    /// Empty string for backward compatibility with Python.
    query_hash: String,
    /// Cost estimate from BigQuery dry run (populated when `query` is provided).
    cost_estimate: Option<CostEstimate>,
}

// ===========================================================================
// Request / Response Types — Phase 7F (catalog, search, info)
// ===========================================================================

#[derive(Deserialize)]
struct SearchRequest {
    query: String,
    #[serde(default = "default_search_limit")]
    limit: usize,
    #[serde(default)]
    datasource: Option<String>,
    /// Whether to include BigQuery public datasets in search results.
    /// If `None`, determined from datasource config (defaults to true).
    #[serde(default)]
    include_public: Option<bool>,
}

fn default_search_limit() -> usize {
    20
}

#[derive(Deserialize)]
struct InfoRequest {
    table_id: String,
}

#[derive(Deserialize)]
struct AddProjectsRequest {
    project_ids: Vec<String>,
}

#[derive(Deserialize)]
struct RemoveProjectRequest {
    project_id: String,
}

#[derive(Deserialize, Default)]
struct RefreshRequest {
    #[serde(default)]
    force: bool,
}

// ===========================================================================
// Phase 6F — POST /request-access-token
// ===========================================================================

async fn request_access_token(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<AccessTokenRequest>,
) -> Result<Json<AccessTokenResponse>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;
    let ds = lookup_bq_datasource(&state.db, &request.datasource_slug, workspace_id).await?;

    let ctx = resolve_bq_access(&state.db, &user, &ds, &state.encryption_key, &state.config).await?;

    // If a query was provided, perform a dry run to estimate cost.
    // Errors are logged but do not fail the token request — the access token
    // is still returned with cost_estimate: null.
    let cost_estimate = if let Some(ref sql) = request.query {
        match bigquery_dry_run_cost(&ctx.access_token, &ctx.billing_project, sql).await {
            Ok(estimate) => {
                tracing::info!(
                    bytes_processed = estimate.bytes_processed,
                    estimated_cost_usd = estimate.estimated_cost_usd,
                    "BigQuery dry run cost estimate"
                );
                Some(estimate)
            }
            Err(e) => {
                tracing::warn!("BigQuery dry run failed (non-fatal): {e}");
                None
            }
        }
    } else {
        None
    };

    tracing::info!(
        "Issued BigQuery access token for user {} on datasource {}",
        user.user_id,
        ds.slug
    );

    Ok(Json(AccessTokenResponse {
        access_token: ctx.access_token,
        expires_at: ctx.expires_at,
        billing_project: ctx.billing_project,
        query_hash: String::new(),
        cost_estimate,
    }))
}

// ===========================================================================
// POST /read-arrow — BigQuery Arrow streaming (REST→Arrow IPC)
// ===========================================================================

/// Request body for the `/read-arrow` endpoint.
///
/// The frontend sends the `job_id` of an already-completed BigQuery query.
/// The backend fetches the query results via REST and converts them to Arrow
/// IPC streaming format for efficient DuckDB-WASM ingestion.
#[derive(Deserialize)]
struct ReadArrowRequest {
    /// BigQuery job ID from a completed query.
    job_id: String,
    /// Optional GCP project ID. If not provided, resolved from the
    /// first active BigQuery datasource in the workspace.
    #[serde(default)]
    project_id: Option<String>,
}

/// BigQuery Arrow streaming endpoint.
///
/// Wire-compatible with Python's `POST /api/v1/bigquery/read-arrow`.
///
/// ## How it works
///
/// 1. The frontend has already executed a query via the BigQuery REST API
///    (using the access token from `/request-access-token`).
/// 2. The frontend sends the completed `job_id` to this endpoint.
/// 3. This endpoint fetches the job metadata to get the total row count and schema.
/// 4. It fetches ALL query results from BigQuery REST API (with pagination).
/// 5. It converts the JSON results to Apache Arrow IPC streaming format.
/// 6. It returns the binary IPC stream as `application/octet-stream`.
///
/// The frontend loads this directly into DuckDB-WASM via
/// `conn.insertArrowFromIPCStream()`.
///
/// ## Why REST→Arrow instead of BigQuery Storage API (gRPC)?
///
/// The BigQuery Storage Read API requires `tonic` + protobuf compilation for
/// the BQ Storage v1 protos, which is an extremely heavy dependency chain.
/// The REST→Arrow conversion adds only the well-maintained `arrow` Rust crate
/// and produces the exact IPC streaming format that DuckDB-WASM consumes.
/// Since the query has already completed, we only need to fetch and convert
/// the results — no gRPC streaming needed.
async fn read_arrow(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<ReadArrowRequest>,
) -> Result<Response, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    tracing::info!(
        job_id = %request.job_id,
        user_id = %user.user_id,
        "Arrow data request"
    );

    // Resolve the BigQuery access token. We need to find a BigQuery datasource
    // in the workspace. The frontend doesn't send a datasource_slug here — it
    // uses the first active BigQuery datasource (matching Python behavior).
    let ds = find_bq_datasource(&state.db, workspace_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest(
                "No BigQuery datasource configured in this workspace".into(),
            )
        })?;

    let bq_ctx = resolve_bq_access(&state.db, &user, &ds, &state.encryption_key, &state.config).await?;

    // Use provided project_id or fall back to billing project
    let project_id = request
        .project_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&bq_ctx.billing_project);

    let client = kyomi_datasource_server::http_client()?;

    // Step 1: Fetch the completed job to get metadata (schema, total rows,
    // destination table, execution time).
    let job_url = format!(
        "{BIGQUERY_API_BASE}/projects/{project_id}/jobs/{}",
        request.job_id
    );
    let job_response = tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT,
        client.get(&job_url).bearer_auth(&bq_ctx.access_token).send(),
    )
    .await
    .map_err(|_| kyomi_core::Error::Internal("BigQuery get-job request timed out".into()))?
    .map_err(|e| {
        kyomi_core::Error::Internal(format!("BigQuery get-job request failed: {e}"))
    })?;

    let job_status_code = job_response.status();
    let job_body: Value = job_response.json().await.map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to parse BigQuery job response: {e}"))
    })?;

    if job_status_code.is_client_error() || job_status_code.is_server_error() {
        let msg = job_body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("Failed to fetch BigQuery job");
        return Err(kyomi_core::Error::Internal(format!(
            "BigQuery job fetch failed: {msg}"
        )));
    }

    // Verify job is complete
    let job_state = job_body
        .get("status")
        .and_then(|s| s.get("state"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    if job_state != "DONE" {
        return Err(kyomi_core::Error::BadRequest(format!(
            "BigQuery job {} is not complete (state: {job_state})",
            request.job_id
        )));
    }

    // Check for job errors
    if let Some(err) = job_body
        .get("status")
        .and_then(|s| s.get("errorResult"))
    {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("BigQuery job failed");
        return Err(kyomi_core::Error::Internal(format!(
            "BigQuery job error: {msg}"
        )));
    }

    // Extract execution time from job statistics
    let execution_time_ms = extract_execution_time_ms(&job_body);

    // Step 2: Fetch all query results with pagination.
    // We use the queries/{jobId} endpoint which returns JSON results.
    let (schema_fields, all_rows, total_rows) = fetch_all_query_results(
        &client,
        &bq_ctx.access_token,
        project_id,
        &request.job_id,
    )
    .await?;

    tracing::info!(
        job_id = %request.job_id,
        total_rows = total_rows,
        schema_fields = schema_fields.len(),
        "Fetched all BigQuery results, converting to Arrow IPC"
    );

    // Step 3: Convert BigQuery JSON results to Arrow IPC streaming format.
    let arrow_bytes =
        bigquery_json_to_arrow_ipc(&schema_fields, &all_rows)?;

    tracing::info!(
        job_id = %request.job_id,
        total_rows = total_rows,
        arrow_bytes = arrow_bytes.len(),
        "Arrow IPC conversion complete"
    );

    // Step 4: Build the binary response with metadata headers.
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/octet-stream")
        .header("x-estimated-rows", total_rows.to_string())
        .header("x-stream-count", "1");

    if let Some(ms) = execution_time_ms {
        response = response.header("x-execution-time-ms", ms.to_string());
    }

    response
        .body(axum::body::Body::from(arrow_bytes))
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("Failed to build Arrow response: {e}"))
        })
}

// ===========================================================================
// BigQuery REST helpers for read-arrow
// ===========================================================================

/// Extract execution time in milliseconds from a BigQuery job response.
fn extract_execution_time_ms(job_body: &Value) -> Option<i64> {
    let start_ms = job_body
        .get("statistics")
        .and_then(|s| s.get("startTime"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i64>().ok())?;
    let end_ms = job_body
        .get("statistics")
        .and_then(|s| s.get("endTime"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i64>().ok())?;
    Some(end_ms - start_ms)
}

/// Fetch all query results from a completed BigQuery job, handling pagination.
///
/// Returns `(schema_fields, all_rows, total_rows)` where:
/// - `schema_fields` is the array of `{"name":..., "type":..., "mode":...}` objects
/// - `all_rows` is the vector of all row data from BigQuery's `rows[].f[].v` format
/// - `total_rows` is the reported total row count
async fn fetch_all_query_results(
    client: &reqwest::Client,
    access_token: &str,
    project_id: &str,
    job_id: &str,
) -> Result<(Vec<Value>, Vec<Value>, u64), kyomi_core::Error> {
    let mut all_rows: Vec<Value> = Vec::new();
    let mut schema_fields: Vec<Value> = Vec::new();
    let mut total_rows: u64 = 0;
    let mut page_token: Option<String> = None;
    let max_results_per_page = 10_000u32;

    loop {
        let mut url = format!(
            "{BIGQUERY_API_BASE}/projects/{project_id}/queries/{job_id}?maxResults={max_results_per_page}"
        );
        if let Some(ref token) = page_token {
            url.push_str(&format!("&pageToken={token}"));
        }

        let response = tokio::time::timeout(
            kyomi_datasource_server::DATASOURCE_TIMEOUT_QUERY,
            client.get(&url).bearer_auth(access_token).send(),
        )
        .await
        .map_err(|_| {
            kyomi_core::Error::Internal("BigQuery query results request timed out".into())
        })?
        .map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "BigQuery query results request failed: {e}"
            ))
        })?;

        let status_code = response.status();
        let body: Value = response.json().await.map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "Failed to parse BigQuery query results: {e}"
            ))
        })?;

        if status_code.is_client_error() || status_code.is_server_error() {
            let msg = body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Failed to fetch query results");
            return Err(kyomi_core::Error::Internal(format!(
                "BigQuery results error: {msg}"
            )));
        }

        // Extract schema from first page
        if schema_fields.is_empty() {
            if let Some(fields) = body
                .get("schema")
                .and_then(|s| s.get("fields"))
                .and_then(|f| f.as_array())
            {
                schema_fields = fields.clone();
            }

            // Extract total rows from first page
            if let Some(tr) = body.get("totalRows").and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .or_else(|| v.as_u64())
            }) {
                total_rows = tr;
            }
        }

        // Collect rows
        if let Some(rows) = body.get("rows").and_then(|r| r.as_array()) {
            all_rows.extend(rows.iter().cloned());
        }

        // Check for next page
        match body.get("pageToken").and_then(|t| t.as_str()) {
            Some(token) if !token.is_empty() => {
                page_token = Some(token.to_string());
            }
            _ => break,
        }
    }

    Ok((schema_fields, all_rows, total_rows))
}

/// Convert BigQuery JSON results to Apache Arrow IPC streaming format.
///
/// Takes the BigQuery schema fields and row data (in BigQuery's `rows[].f[].v`
/// format) and produces a complete Arrow IPC stream that DuckDB-WASM can
/// ingest via `insertArrowFromIPCStream()`.
///
/// ## Type mapping
///
/// | BigQuery Type | Arrow Type |
/// |--------------|------------|
/// | STRING, BYTES, GEOGRAPHY, JSON | Utf8 |
/// | INT64, INTEGER | Int64 |
/// | FLOAT64, FLOAT, NUMERIC, BIGNUMERIC | Float64 |
/// | BOOL, BOOLEAN | Boolean |
/// | DATE | Utf8 (ISO format string) |
/// | TIME | Utf8 (ISO format string) |
/// | DATETIME | Utf8 (ISO format string) |
/// | TIMESTAMP | Utf8 (ISO format string) |
/// | STRUCT, RECORD, ARRAY | Utf8 (JSON serialized) |
///
/// Date/time types are kept as strings rather than Arrow temporal types because:
/// 1. BigQuery returns them as formatted strings in JSON mode
/// 2. DuckDB-WASM can parse date/time strings efficiently
/// 3. Avoids timezone and precision conversion complexity
fn bigquery_json_to_arrow_ipc(
    schema_fields: &[Value],
    rows: &[Value],
) -> Result<Vec<u8>, kyomi_core::Error> {
    use arrow_array::{ArrayRef, RecordBatch};
    use arrow_ipc::writer::StreamWriter;
    use arrow_schema::{Field, Schema};

    // Build Arrow schema from BigQuery schema fields
    let fields: Vec<Field> = schema_fields
        .iter()
        .map(|field| {
            let name = field
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("unknown")
                .to_string();
            let bq_type = field
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("STRING");
            let mode = field
                .get("mode")
                .and_then(|m| m.as_str())
                .unwrap_or("NULLABLE");
            let nullable = mode != "REQUIRED";

            let data_type = bq_type_to_arrow_type(bq_type);
            Field::new(name, data_type, nullable)
        })
        .collect();

    let schema = Arc::new(Schema::new(fields));
    let num_columns = schema.fields().len();

    // Build column arrays from BigQuery row data
    let arrays: Vec<ArrayRef> = (0..num_columns)
        .map(|col_idx| {
            let data_type = schema.field(col_idx).data_type();
            build_arrow_column(rows, col_idx, data_type)
        })
        .collect();

    // Create a RecordBatch
    let batch = RecordBatch::try_new(schema.clone(), arrays).map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to create Arrow RecordBatch: {e}"))
    })?;

    // Write as IPC stream
    let mut buf = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut buf, &schema).map_err(|e| {
            kyomi_core::Error::Internal(format!("Failed to create Arrow IPC writer: {e}"))
        })?;

        writer.write(&batch).map_err(|e| {
            kyomi_core::Error::Internal(format!("Failed to write Arrow batch: {e}"))
        })?;

        writer.finish().map_err(|e| {
            kyomi_core::Error::Internal(format!("Failed to finish Arrow IPC stream: {e}"))
        })?;
    }

    Ok(buf)
}

/// Map a BigQuery type name to an Arrow DataType.
fn bq_type_to_arrow_type(bq_type: &str) -> arrow_schema::DataType {
    use arrow_schema::DataType;

    match bq_type.to_uppercase().as_str() {
        "INT64" | "INTEGER" => DataType::Int64,
        "FLOAT64" | "FLOAT" | "NUMERIC" | "BIGNUMERIC" => DataType::Float64,
        "BOOL" | "BOOLEAN" => DataType::Boolean,
        // Date/time types are kept as strings — BigQuery REST API returns them
        // as formatted strings, and DuckDB can parse them efficiently.
        "DATE" | "TIME" | "DATETIME" | "TIMESTAMP" => DataType::Utf8,
        // Complex types are JSON-serialized strings
        "STRUCT" | "RECORD" | "ARRAY" | "RANGE" => DataType::Utf8,
        // String types (STRING, BYTES, GEOGRAPHY, JSON) and anything unknown
        _ => DataType::Utf8,
    }
}

/// Extract a cell value from a BigQuery row at the given column index.
///
/// BigQuery JSON format: `row.f[col_idx].v` where `v` is the value (string,
/// null, or nested for RECORD/ARRAY types).
fn extract_bq_cell(row: &Value, col_idx: usize) -> Option<&Value> {
    row.get("f")
        .and_then(|f| f.as_array())
        .and_then(|cells| cells.get(col_idx))
        .and_then(|cell| cell.get("v"))
}

/// Build an Arrow column array from BigQuery rows for the given column index.
fn build_arrow_column(
    rows: &[Value],
    col_idx: usize,
    data_type: &arrow_schema::DataType,
) -> arrow_array::ArrayRef {
    use arrow_array::{ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray};
    use arrow_schema::DataType;

    match data_type {
        DataType::Int64 => {
            let values: Vec<Option<i64>> = rows
                .iter()
                .map(|row| {
                    extract_bq_cell(row, col_idx).and_then(|v| {
                        if v.is_null() {
                            None
                        } else {
                            // BigQuery returns integers as strings in JSON
                            v.as_str()
                                .and_then(|s| s.parse::<i64>().ok())
                                .or_else(|| v.as_i64())
                        }
                    })
                })
                .collect();
            Arc::new(Int64Array::from(values)) as ArrayRef
        }
        DataType::Float64 => {
            let values: Vec<Option<f64>> = rows
                .iter()
                .map(|row| {
                    extract_bq_cell(row, col_idx).and_then(|v| {
                        if v.is_null() {
                            None
                        } else {
                            v.as_str()
                                .and_then(|s| s.parse::<f64>().ok())
                                .or_else(|| v.as_f64())
                        }
                    })
                })
                .collect();
            Arc::new(Float64Array::from(values)) as ArrayRef
        }
        DataType::Boolean => {
            let values: Vec<Option<bool>> = rows
                .iter()
                .map(|row| {
                    extract_bq_cell(row, col_idx).and_then(|v| {
                        if v.is_null() {
                            None
                        } else {
                            v.as_str()
                                .map(|s| s == "true")
                                .or_else(|| v.as_bool())
                        }
                    })
                })
                .collect();
            Arc::new(BooleanArray::from(values)) as ArrayRef
        }
        // Utf8 covers STRING, DATE, TIME, DATETIME, TIMESTAMP, STRUCT, etc.
        _ => {
            let values: Vec<Option<String>> = rows
                .iter()
                .map(|row| {
                    extract_bq_cell(row, col_idx).and_then(|v| {
                        if v.is_null() {
                            None
                        } else if let Some(s) = v.as_str() {
                            Some(s.to_string())
                        } else {
                            // For complex types (STRUCT, ARRAY), serialize as JSON
                            Some(v.to_string())
                        }
                    })
                })
                .collect();
            Arc::new(StringArray::from(values)) as ArrayRef
        }
    }
}

// ===========================================================================
// Phase 7F — GET /catalog
// ===========================================================================

/// Full catalog tree from cache.
///
/// Returns all cached tables organized by project -> dataset -> tables.
/// Response shape: `{status: "success", total_tables: N, catalog: {project: {dataset: [tables]}}}`
async fn get_catalog(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Get all cached tables for the workspace
    let cached_tables = fetch_cached_tables(&state.db, workspace_id, None).await?;

    // Organize into tree structure: {project: {dataset: [tables]}}
    let mut catalog: BTreeMap<String, BTreeMap<String, Vec<Value>>> = BTreeMap::new();

    for table in &cached_tables {
        let project_id = &table.project_id;
        let dataset_id = &table.dataset_id;
        let table_id = &table.table_id;

        let description = extract_description(&table.table_metadata);

        let full_table_id = format!("{project_id}.{dataset_id}.{table_id}");

        let table_entry = json!({
            "table_id": table_id,
            "full_table_id": full_table_id,
            "description": description,
            "updated_at": table.updated_at.to_rfc3339(),
        });

        catalog
            .entry(project_id.clone())
            .or_default()
            .entry(dataset_id.clone())
            .or_default()
            .push(table_entry);
    }

    Ok(Json(json!({
        "status": "success",
        "total_tables": cached_tables.len(),
        "catalog": catalog,
    })))
}

// ===========================================================================
// Phase 7F — GET /catalog/projects
// ===========================================================================

/// Project list with dataset counts.
///
/// Response shape: `{status: "success", projects: [{project_id, dataset_count}]}`
async fn get_catalog_projects(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let cached_tables = fetch_cached_tables(&state.db, workspace_id, None).await?;

    // Count unique datasets per project
    let mut project_datasets: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    for table in &cached_tables {
        project_datasets
            .entry(table.project_id.clone())
            .or_default()
            .insert(table.dataset_id.clone());
    }

    let projects: Vec<Value> = project_datasets
        .into_iter()
        .map(|(project_id, datasets)| {
            json!({
                "project_id": project_id,
                "dataset_count": datasets.len(),
            })
        })
        .collect();

    Ok(Json(json!({
        "status": "success",
        "projects": projects,
    })))
}

// ===========================================================================
// Phase 7F — GET /catalog/{project}/datasets
// ===========================================================================

/// Datasets in a project with table counts.
///
/// Response shape: `{status: "success", datasets: [{dataset_id, table_count}]}`
async fn get_catalog_datasets(
    State(state): State<AppState>,
    user: AuthUser,
    Path(project): Path<String>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let cached_tables = fetch_cached_tables(&state.db, workspace_id, None).await?;

    // Count tables per dataset for this project only
    let mut dataset_counts: BTreeMap<String, usize> = BTreeMap::new();
    for table in &cached_tables {
        if table.project_id == project {
            *dataset_counts.entry(table.dataset_id.clone()).or_default() += 1;
        }
    }

    let datasets: Vec<Value> = dataset_counts
        .into_iter()
        .map(|(dataset_id, table_count)| {
            json!({
                "dataset_id": dataset_id,
                "table_count": table_count,
            })
        })
        .collect();

    Ok(Json(json!({
        "status": "success",
        "datasets": datasets,
    })))
}

// ===========================================================================
// Phase 7F — GET /catalog/{project}/{dataset}/tables
// ===========================================================================

/// Tables in a dataset with column counts.
///
/// Response shape: `{status: "success", tables: [{table_id, full_table_id, description, column_count, updated_at}]}`
async fn get_catalog_tables(
    State(state): State<AppState>,
    user: AuthUser,
    Path((project, dataset)): Path<(String, String)>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let cached_tables = fetch_cached_tables(&state.db, workspace_id, None).await?;

    let mut tables: Vec<Value> = Vec::new();
    for table in &cached_tables {
        if table.project_id != project || table.dataset_id != dataset {
            continue;
        }

        let description = extract_description(&table.table_metadata);
        let column_count = table
            .table_metadata
            .get("columns")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);

        let full_table_id = format!("{}.{}.{}", project, dataset, table.table_id);

        tables.push(json!({
            "table_id": table.table_id,
            "full_table_id": full_table_id,
            "description": description,
            "column_count": column_count,
            "updated_at": table.updated_at.to_rfc3339(),
        }));
    }

    Ok(Json(json!({
        "status": "success",
        "tables": tables,
    })))
}

// ===========================================================================
// Phase 7F — GET /catalog/{project}/{dataset}/{table}/columns
// ===========================================================================

/// Column details for a specific table.
///
/// Response shape: `{status: "success", columns: [{name, type, mode, description}]}`
async fn get_catalog_columns(
    State(state): State<AppState>,
    user: AuthUser,
    Path((project, dataset, table_name)): Path<(String, String, String)>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    let cached_tables = fetch_cached_tables(&state.db, workspace_id, None).await?;

    let mut columns: Vec<Value> = Vec::new();

    for table in &cached_tables {
        if table.project_id == project
            && table.dataset_id == dataset
            && table.table_id == table_name
        {
            if let Some(cols_arr) = table.table_metadata.get("columns").and_then(|v| v.as_array())
            {
                for col in cols_arr {
                    if let Some(obj) = col.as_object() {
                        columns.push(json!({
                            "name": obj.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                            "type": obj.get("type").and_then(|v| v.as_str())
                                .or_else(|| obj.get("field_type").and_then(|v| v.as_str()))
                                .unwrap_or(""),
                            "mode": obj.get("mode").and_then(|v| v.as_str()).unwrap_or(""),
                            "description": obj.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                        }));
                    }
                }
            }
            break;
        }
    }

    Ok(Json(json!({
        "status": "success",
        "columns": columns,
    })))
}

// ===========================================================================
// Phase 7F — GET /catalog/status
// ===========================================================================

/// Catalog refresh status.
///
/// Response shape:
/// ```json
/// {
///   "indexed_projects": [{project_id, dataset_count, last_indexed}],
///   "current_status": "idle"
/// }
/// ```
async fn get_catalog_status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Find BigQuery datasource to get catalog_projects
    let bq_ds = find_bq_datasource(&state.db, workspace_id).await?;

    let catalog_projects: Vec<String> = bq_ds
        .as_ref()
        .and_then(|ds| ds.connection_config.get("catalog_projects"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // Get cached tables
    let cached_tables = fetch_cached_tables(&state.db, workspace_id, None).await?;

    // Build indexed_projects with dataset counts
    let mut indexed_projects: Vec<Value> = Vec::new();
    for project_id in &catalog_projects {
        // Count distinct datasets
        let datasets: HashSet<&str> = cached_tables
            .iter()
            .filter(|t| &t.project_id == project_id)
            .map(|t| t.dataset_id.as_str())
            .collect();

        // Find most recent update time
        let last_indexed = cached_tables
            .iter()
            .filter(|t| &t.project_id == project_id)
            .map(|t| t.updated_at)
            .max()
            .map(|dt| dt.to_rfc3339());

        indexed_projects.push(json!({
            "project_id": project_id,
            "dataset_count": datasets.len(),
            "last_indexed": last_indexed,
        }));
    }

    Ok(Json(json!({
        "indexed_projects": indexed_projects,
        "current_status": "idle",
    })))
}

// ===========================================================================
// Phase 7F — POST /catalog/refresh
// ===========================================================================

/// Trigger catalog refresh.
///
/// Response shape: `{status: "started"|"error"|"already_running", message: "..."}`
async fn catalog_refresh(
    State(state): State<AppState>,
    user: AuthUser,
    body: Option<Json<RefreshRequest>>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Find BigQuery datasource
    let bq_ds = find_bq_datasource(&state.db, workspace_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest(
                "No BigQuery datasource configured. Please configure one in Settings first."
                    .into(),
            )
        })?;

    let force = body.as_ref().map(|b| b.force).unwrap_or(false);

    // Delegate to the shared catalog refresh logic
    let result =
        super::catalog::execute_catalog_refresh(&state, &user, bq_ds, workspace_id, force)
            .await?;

    Ok(Json(json!({
        "status": result.status,
        "message": result.message,
    })))
}

// ===========================================================================
// Phase 7F — POST /catalog/projects/add
// ===========================================================================

/// Add projects to the catalog config and trigger refresh.
///
/// Response shape: `{status: "indexing"|"no_change", message, indexed_projects}`
async fn catalog_projects_add(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<AddProjectsRequest>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Find BigQuery datasource
    let bq_ds = find_bq_datasource(&state.db, workspace_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest(
                "No BigQuery datasource configured. Please configure one in Settings first."
                    .into(),
            )
        })?;

    // Get current catalog_projects
    let mut connection_config = bq_ds.connection_config.clone();
    let current_projects: Vec<String> = connection_config
        .get("catalog_projects")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // Add new projects (avoid duplicates)
    let new_projects: Vec<String> = request
        .project_ids
        .iter()
        .filter(|pid| !current_projects.contains(pid))
        .cloned()
        .collect();

    if new_projects.is_empty() {
        return Ok(Json(json!({
            "status": "no_change",
            "message": "All specified projects are already indexed",
            "indexed_projects": current_projects,
        })));
    }

    let mut updated_projects = current_projects;
    updated_projects.extend(new_projects.iter().cloned());

    // Update connection_config with new projects
    if let Some(obj) = connection_config.as_object_mut() {
        obj.insert(
            "catalog_projects".to_string(),
            json!(updated_projects),
        );
    }

    // Persist the updated connection_config
    kyomi_auth::datasource_service::update_datasource(
        &state.db,
        &bq_ds.id,
        workspace_id,
        None,
        None,
        Some(connection_config),
        None,
        None,
    )
    .await?;

    tracing::info!(
        "Added {} projects to catalog for workspace {}",
        new_projects.len(),
        workspace_id
    );

    // Re-fetch the datasource with the updated connection_config for the refresh.
    let updated_ds = find_bq_datasource(&state.db, workspace_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Internal(
                "BigQuery datasource not found after update".into(),
            )
        })?;

    // Spawn background catalog refresh so the endpoint returns immediately.
    // The user just added projects — force=true to skip the rate limit check.
    let bg_state = state.clone();
    let bg_user = user.clone();
    let bg_workspace_id = workspace_id.to_string();
    tokio::spawn(async move {
        match super::catalog::execute_catalog_refresh(
            &bg_state,
            &bg_user,
            updated_ds,
            &bg_workspace_id,
            true,
        )
        .await
        {
            Ok(result) => {
                tracing::info!(
                    status = %result.status,
                    message = %result.message,
                    "background catalog refresh after project add completed"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "background catalog refresh after project add failed"
                );
            }
        }
    });

    Ok(Json(json!({
        "status": "indexing",
        "message": format!("Added {} project(s) to catalog. Indexing started in background.", new_projects.len()),
        "indexed_projects": updated_projects,
    })))
}

// ===========================================================================
// Phase 7F — POST /catalog/projects/remove
// ===========================================================================

/// Remove a project from the catalog config.
///
/// Response shape: `{status: "removed"|"not_found", message, indexed_projects}`
async fn catalog_projects_remove(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<RemoveProjectRequest>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Find BigQuery datasource
    let bq_ds = find_bq_datasource(&state.db, workspace_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest(
                "No BigQuery datasource configured. Please configure one in Settings first."
                    .into(),
            )
        })?;

    // Get current catalog_projects
    let mut connection_config = bq_ds.connection_config.clone();
    let current_projects: Vec<String> = connection_config
        .get("catalog_projects")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    if !current_projects.contains(&request.project_id) {
        return Ok(Json(json!({
            "status": "not_found",
            "message": format!("Project '{}' is not in the catalog", request.project_id),
            "indexed_projects": current_projects,
        })));
    }

    // Remove the project
    let updated_projects: Vec<String> = current_projects
        .into_iter()
        .filter(|pid| pid != &request.project_id)
        .collect();

    // Update connection_config
    if let Some(obj) = connection_config.as_object_mut() {
        obj.insert(
            "catalog_projects".to_string(),
            json!(updated_projects),
        );
    }

    // Persist
    kyomi_auth::datasource_service::update_datasource(
        &state.db,
        &bq_ds.id,
        workspace_id,
        None,
        None,
        Some(connection_config),
        None,
        None,
    )
    .await?;

    tracing::info!(
        "Removed project '{}' from catalog for workspace {}",
        request.project_id,
        workspace_id
    );

    Ok(Json(json!({
        "status": "removed",
        "message": format!("Project '{}' removed from catalog. Run a full refresh to remove its data from search.", request.project_id),
        "indexed_projects": updated_projects,
    })))
}

// ===========================================================================
// Phase 7F — POST /catalog/settings (deprecated)
// ===========================================================================

/// Deprecated endpoint — returns 400.
async fn catalog_settings_deprecated(
    _user: AuthUser,
) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "detail": "Workspace-level catalog settings removed. Configure include_public_datasets per-datasource via PATCH /api/v1/datasources/{id}"
        })),
    )
}

// ===========================================================================
// Phase 7F — GET /projects/listAccessible
// ===========================================================================

/// Query parameters for the listAccessible endpoint.
#[derive(Deserialize)]
struct ListProjectsQuery {
    /// When true, count datasets per project via BigQuery API (slower).
    #[serde(default)]
    include_dataset_counts: bool,
}

/// List GCP projects the user has BigQuery access to.
///
/// Requires a BigQuery datasource configured in the workspace.
/// Uses the user's Google OAuth token to call the Cloud Resource Manager API.
///
/// When `include_dataset_counts=true`, additionally calls the BigQuery datasets
/// API per project to populate `dataset_count` (matching Python behavior).
async fn list_accessible_projects(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ListProjectsQuery>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Ensure there is a BigQuery datasource
    let _bq_ds = find_bq_datasource(&state.db, workspace_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest(
                "No BigQuery datasource configured in this workspace".into(),
            )
        })?;

    // Load user's Google OAuth token
    let db_user = kyomi_auth::user_service::get_user_by_id(&state.db, &user.user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))?;

    let oauth_data = kyomi_auth::google_oauth::parse_oauth_data(
        db_user.oauth_data.as_deref(),
        &state.encryption_key,
    )?
    .ok_or_else(|| {
        kyomi_core::Error::BadRequest(
            "No Google OAuth data found. Please connect your Google account first.".into(),
        )
    })?;

    let tokens = oauth_data.google_oauth_tokens.ok_or_else(|| {
        kyomi_core::Error::BadRequest(
            "No BigQuery tokens found. Please connect with BigQuery scopes.".into(),
        )
    })?;

    let access_token = &tokens.access_token;

    // Call Google Cloud Resource Manager API
    let client = kyomi_datasource_server::http_client()?;
    let resp = client
        .get(kyomi_auth::google_oauth::GOOGLE_PROJECTS_URI)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("Failed to call GCP projects API: {e}"))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "GCP projects API returned {status}: {body}"
        )));
    }

    let body: Value = resp.json().await.map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to parse GCP projects response: {e}"))
    })?;

    // Extract active projects
    let mut projects: Vec<Value> = body
        .get("projects")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|p| {
                    p.get("lifecycleState")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "ACTIVE")
                        .unwrap_or(false)
                })
                .map(|p| {
                    let project_id = p
                        .get("projectId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    json!({
                        "project_id": project_id,
                        "name": name,
                        "display_name": name,
                        "can_be_billing_project": true,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    tracing::info!(
        "Found {} BigQuery projects for user {} (include_counts={})",
        projects.len(),
        user.user_id,
        params.include_dataset_counts,
    );

    // Optionally count datasets per project (lazy load, matching Python behavior)
    if params.include_dataset_counts {
        for project in &mut projects {
            let project_id = project
                .get("project_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if project_id.is_empty() {
                continue;
            }

            let dataset_count =
                fetch_dataset_count(&client, access_token, project_id).await;

            match dataset_count {
                Ok(count) => {
                    tracing::info!(
                        "[listAccessible] Project {}: {} datasets",
                        project_id,
                        count,
                    );
                    project
                        .as_object_mut()
                        .unwrap()
                        .insert("dataset_count".to_string(), json!(count));
                }
                Err(e) => {
                    tracing::warn!(
                        "[listAccessible] Failed to count datasets in {}: {}",
                        project_id,
                        e,
                    );
                    // Don't block on error — set to 0 like Python
                    project
                        .as_object_mut()
                        .unwrap()
                        .insert("dataset_count".to_string(), json!(0));
                }
            }
        }
    }

    Ok(Json(json!(projects)))
}

/// Fetch the number of datasets in a BigQuery project using the REST API.
///
/// Calls `GET {BIGQUERY_API_BASE}/projects/{project_id}/datasets`
/// and counts the returned `datasets` array length.
async fn fetch_dataset_count(
    client: &reqwest::Client,
    access_token: &str,
    project_id: &str,
) -> Result<usize, String> {
    let url = format!("{BIGQUERY_API_BASE}/projects/{project_id}/datasets");

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("BigQuery datasets API returned {status}: {body}"));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse datasets response: {e}"))?;

    let count = body
        .get("datasets")
        .and_then(|v| v.as_array())
        .map(|arr| arr.len())
        .unwrap_or(0);

    Ok(count)
}

// ===========================================================================
// Phase 7F — POST /search
// ===========================================================================

/// Semantic vector search across cached tables.
///
/// Uses pgvector cosine similarity with weight-adjusted scoring.
/// Returns empty results for empty query (no embedding needed).
///
/// Response shape: `{status: "success", query, results_count, results: [...]}`
async fn search_tables(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<SearchRequest>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Empty query -> empty results
    if request.query.trim().is_empty() {
        return Ok(Json(json!({
            "status": "success",
            "query": request.query,
            "results_count": 0,
            "results": [],
        })));
    }

    // Resolve optional datasource slug -> config ID
    let datasource_config_id: Option<String> = if let Some(ref slug) = request.datasource {
        let ds = kyomi_auth::datasource_service::get_datasource_by_slug(
            &state.db,
            slug,
            workspace_id,
        )
        .await?;
        ds.map(|d| d.id)
    } else {
        None
    };

    // Determine whether to include public datasets in search results.
    // Matches Python's logic: request.include_public overrides, then datasource config,
    // then first active BigQuery datasource in workspace. Default: true.
    let include_public = {
        let mut val = request.include_public.unwrap_or(true);

        // Check datasource config — if include_public_datasets is false, override
        if val {
            if let Some(ref slug) = request.datasource {
                // Check the specific datasource
                if let Some(ds) = kyomi_auth::datasource_service::get_datasource_by_slug(
                    &state.db,
                    slug,
                    workspace_id,
                )
                .await?
                {
                    if ds.datasource_type == kyomi_core::DatasourceType::Bigquery {
                        let setting = ds
                            .connection_config
                            .get("include_public_datasets")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        if !setting {
                            val = false;
                        }
                    }
                }
            } else {
                // No specific datasource — check first active BigQuery datasource
                #[derive(sqlx::FromRow)]
                struct BqConfigRow { connection_config: serde_json::Value }
                let bq_ds = kyomi_core::db_fetch_optional!(
                    &state.db, BqConfigRow,
                    "SELECT connection_config \
                     FROM datasource_configs \
                     WHERE workspace_id = $1 \
                       AND datasource_type = 'bigquery' \
                       AND active = true \
                     LIMIT 1",
                    workspace_id
                )?;

                if let Some(bq_ds) = bq_ds {
                    let setting = bq_ds
                        .connection_config
                        .get("include_public_datasets")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    if !setting {
                        val = false;
                    }
                }
            }
        }

        val
    };

    // Encode query with embedding model (BGE query prefix for asymmetric retrieval)
    let query_embedding = state.embedding.wait_ready().await?.embed_query(&request.query)?;

    // Fetch limit (overfetch for deduplication)
    let fetch_limit = (request.limit * 5) as i64;

    // Build the workspace filter conditionally based on include_public setting.
    // The sentinel workspace IDs are hardcoded constants (not user input).
    let public_clause = if include_public {
        " OR e.workspace_id = 'public-data-workspace'"
    } else {
        ""
    };

    // Use parameterized pgvector cosine similarity query.
    // pgvector `<=>` operator is Postgres-only; SQLite uses the VectorSearch trait instead.
    let rows: Vec<SearchRow> = match &state.db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let query_vector = pgvector::Vector::from(query_embedding);
            if let Some(ref ds_id) = datasource_config_id {
                let sql = format!(
                    "SELECT \
                        e.project_id, e.dataset_id, e.table_id, e.entry_type, e.text, \
                        e.weight, \
                        (1 - (e.embedding <=> $1::vector)) AS similarity, \
                        ((1 - (e.embedding <=> $1::vector)) * e.weight) AS weighted_score, \
                        tc.table_metadata, \
                        dc.slug AS datasource_slug, \
                        dc.datasource_type AS datasource_type, \
                        dc.name AS datasource_name \
                     FROM datasource_search_embeddings e \
                     JOIN datasource_table_cache tc ON e.table_cache_id = tc.id \
                     LEFT JOIN datasource_configs dc ON e.datasource_config_id = dc.id \
                     WHERE (e.workspace_id = $2{public_clause} \
                            OR e.workspace_id = 'sample-data-workspace') \
                       AND tc.is_archived = false \
                       AND e.datasource_config_id = $3 \
                     ORDER BY weighted_score DESC \
                     LIMIT $4",
                );
                sqlx::query_as::<_, SearchRow>(&sql)
                    .bind(&query_vector)
                    .bind(workspace_id)
                    .bind(ds_id)
                    .bind(fetch_limit)
                    .fetch_all(pg)
                    .await?
            } else {
                let sql = format!(
                    "SELECT \
                        e.project_id, e.dataset_id, e.table_id, e.entry_type, e.text, \
                        e.weight, \
                        (1 - (e.embedding <=> $1::vector)) AS similarity, \
                        ((1 - (e.embedding <=> $1::vector)) * e.weight) AS weighted_score, \
                        tc.table_metadata, \
                        dc.slug AS datasource_slug, \
                        dc.datasource_type AS datasource_type, \
                        dc.name AS datasource_name \
                     FROM datasource_search_embeddings e \
                     JOIN datasource_table_cache tc ON e.table_cache_id = tc.id \
                     LEFT JOIN datasource_configs dc ON e.datasource_config_id = dc.id \
                     WHERE (e.workspace_id = $2{public_clause} \
                            OR e.workspace_id = 'sample-data-workspace') \
                       AND tc.is_archived = false \
                     ORDER BY weighted_score DESC \
                     LIMIT $3",
                );
                sqlx::query_as::<_, SearchRow>(&sql)
                    .bind(&query_vector)
                    .bind(workspace_id)
                    .bind(fetch_limit)
                    .fetch_all(pg)
                    .await?
            }
        }
        kyomi_core::db::DbPool::Sqlite(_sq) => {
            // SQLite: vector search handled by VectorSearch trait; return empty here
            vec![]
        }
    };

    // Deduplicate by full_table_id, keeping highest score
    let mut table_best: HashMap<String, &SearchRow> = HashMap::new();
    for row in &rows {
        let full_table_id = if row.project_id.is_empty() {
            format!("{}.{}", row.dataset_id, row.table_id)
        } else {
            format!("{}.{}.{}", row.project_id, row.dataset_id, row.table_id)
        };

        let is_better = table_best
            .get(&full_table_id)
            .map(|existing| row.weighted_score > existing.weighted_score)
            .unwrap_or(true);

        if is_better {
            table_best.insert(full_table_id, row);
        }
    }

    // Sort by weighted_score descending
    let mut sorted: Vec<(&String, &&SearchRow)> = table_best.iter().collect();
    sorted.sort_by(|a, b| {
        b.1.weighted_score
            .partial_cmp(&a.1.weighted_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Adaptive quality filtering
    let high_quality: Vec<_> = sorted
        .iter()
        .filter(|(_, row)| row.similarity >= 0.5)
        .collect();

    let use_high_quality = high_quality.len() >= std::cmp::min(10, request.limit);

    let final_results: Vec<Value> = if use_high_quality {
        high_quality
            .into_iter()
            .take(request.limit)
            .map(|(full_table_id, row)| row.to_result_json(full_table_id))
            .collect()
    } else {
        sorted
            .iter()
            .take(request.limit)
            .map(|(full_table_id, row)| row.to_result_json(full_table_id))
            .collect()
    };

    let results_count = final_results.len();

    Ok(Json(json!({
        "status": "success",
        "query": request.query,
        "results_count": results_count,
        "results": final_results,
    })))
}

/// Internal row type for the search query result.
#[derive(Debug, sqlx::FromRow)]
struct SearchRow {
    project_id: String,
    dataset_id: String,
    table_id: String,
    entry_type: String,
    text: String,
    weight: f64,
    similarity: f64,
    weighted_score: f64,
    table_metadata: Value,
    datasource_slug: Option<String>,
    datasource_type: Option<String>,
    datasource_name: Option<String>,
}

impl SearchRow {
    fn to_result_json(&self, full_table_id: &str) -> Value {
        let description = self
            .table_metadata
            .get("table_description")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let table_type = self
            .table_metadata
            .get("table_type")
            .and_then(|v| v.as_str())
            .unwrap_or("TABLE");

        let columns_count = self
            .table_metadata
            .get("columns")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);

        let matched_text = if self.text.len() > 100 {
            format!("{}...", &self.text[..100])
        } else {
            self.text.clone()
        };

        json!({
            "project_id": self.project_id,
            "dataset_id": self.dataset_id,
            "table_name": self.table_id,
            "table_id": full_table_id,
            "table_description": description,
            "table_type": table_type,
            "similarity_score": self.similarity,
            "weighted_score": self.weighted_score,
            "weight": self.weight,
            "entry_type": self.entry_type,
            "matched_text": matched_text,
            "columns_count": columns_count,
            "datasource": self.datasource_slug,
            "datasource_type": self.datasource_type,
            "datasource_name": self.datasource_name,
        })
    }
}

// ===========================================================================
// Phase 7F — POST /info
// ===========================================================================

/// Table metadata from cache.
///
/// Parses table_id as "project.dataset.table" (3 parts).
/// Returns full metadata including columns from table_metadata JSON.
///
/// Response shape (success): `{status: "success", metadata: {...}}`
/// Response shape (not found): `{status: "error", error: "...", error_type: "not_found"}`
async fn get_table_info(
    State(state): State<AppState>,
    user: AuthUser,
    Json(request): Json<InfoRequest>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let workspace_id = get_workspace_id(&user)?;

    // Parse table_id as "project.dataset.table"
    let parts: Vec<&str> = request.table_id.splitn(3, '.').collect();
    if parts.len() != 3 {
        return Ok(Json(json!({
            "status": "error",
            "error": format!("Invalid table_id format: '{}'. Expected 'project.dataset.table'.", request.table_id),
            "error_type": "invalid_format",
        })));
    }
    let (project_id, dataset_id, table_name) = (parts[0], parts[1], parts[2]);

    // Look up in datasource_table_cache
    let table: Option<DatasourceTableCache> = kyomi_core::db_fetch_optional!(
        &state.db, DatasourceTableCache,
        "SELECT id, workspace_id, datasource_config_id, project_id, dataset_id, table_id, \
         table_metadata, column_descriptions, \
         created_at, updated_at, \
         structure_refreshed_at, descriptions_refreshed_at, is_archived, last_verified \
         FROM datasource_table_cache \
         WHERE workspace_id = $1 AND project_id = $2 AND dataset_id = $3 AND table_id = $4 \
           AND is_archived = false \
         LIMIT 1",
        workspace_id,
        project_id,
        dataset_id,
        table_name
    )?;

    let Some(table) = table else {
        return Ok(Json(json!({
            "status": "error",
            "error": format!("Table '{}' not found in catalog cache", request.table_id),
            "error_type": "not_found",
        })));
    };

    let description = extract_description(&table.table_metadata);
    let columns = table
        .table_metadata
        .get("columns")
        .cloned()
        .unwrap_or(json!([]));

    Ok(Json(json!({
        "status": "success",
        "metadata": {
            "table_id": request.table_id,
            "project_id": project_id,
            "dataset_id": dataset_id,
            "table_name": table_name,
            "description": description,
            "columns": columns,
        },
    })))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::Array;

    // --- Cost estimation ---

    #[test]
    fn cost_estimate_zero_bytes() {
        let estimate = calculate_cost_estimate(0);
        assert_eq!(estimate.bytes_processed, 0);
        assert_eq!(estimate.estimated_cost_usd, 0.0);
    }

    #[test]
    fn cost_estimate_one_terabyte() {
        // 1 TB = 10^12 bytes at $6.25/TB = $6.25
        let estimate = calculate_cost_estimate(1_000_000_000_000);
        assert_eq!(estimate.bytes_processed, 1_000_000_000_000);
        assert!((estimate.estimated_cost_usd - 6.25).abs() < 1e-10);
    }

    #[test]
    fn cost_estimate_100_gigabytes() {
        // 100 GB = 10^11 bytes at $6.25/TB = $0.625
        let estimate = calculate_cost_estimate(100_000_000_000);
        assert_eq!(estimate.bytes_processed, 100_000_000_000);
        assert!((estimate.estimated_cost_usd - 0.625).abs() < 1e-10);
    }

    #[test]
    fn cost_estimate_small_query() {
        // 10 MB = 10_000_000 bytes
        let estimate = calculate_cost_estimate(10_000_000);
        assert_eq!(estimate.bytes_processed, 10_000_000);
        let expected = 10_000_000.0 * 6.25 / 1_000_000_000_000.0;
        assert!((estimate.estimated_cost_usd - expected).abs() < 1e-15);
    }

    #[test]
    fn cost_estimate_serializes_correctly() {
        let estimate = calculate_cost_estimate(500_000_000_000); // 500 GB
        let json = serde_json::to_value(&estimate).expect("serialize");
        assert_eq!(json["bytes_processed"], 500_000_000_000_i64);
        // 500 GB at $6.25/TB = $3.125
        let cost = json["estimated_cost_usd"].as_f64().expect("f64");
        assert!((cost - 3.125).abs() < 1e-10);
    }

    #[test]
    fn cost_estimate_constant_matches_pricing() {
        // Verify the constant: $6.25 per 10^12 bytes
        let expected = 6.25 / 1_000_000_000_000.0_f64;
        assert!((BIGQUERY_COST_PER_BYTE - expected).abs() < 1e-30);
    }

    // --- Request deserialization ---

    #[test]
    fn access_token_request_without_query() {
        let json = r#"{"datasource_slug": "my-bq"}"#;
        let req: AccessTokenRequest = serde_json::from_str(json).expect("deserialize");
        assert_eq!(req.datasource_slug, "my-bq");
        assert!(req.query.is_none());
    }

    #[test]
    fn access_token_request_with_query() {
        let json = r#"{"datasource_slug": "my-bq", "query": "SELECT 1"}"#;
        let req: AccessTokenRequest = serde_json::from_str(json).expect("deserialize");
        assert_eq!(req.datasource_slug, "my-bq");
        assert_eq!(req.query.as_deref(), Some("SELECT 1"));
    }

    // --- Response serialization ---

    #[test]
    fn access_token_response_without_cost_estimate() {
        let response = AccessTokenResponse {
            access_token: "ya29.test".into(),
            expires_at: "2025-01-01T00:00:00Z".into(),
            billing_project: "my-project".into(),
            query_hash: String::new(),
            cost_estimate: None,
        };
        let json = serde_json::to_value(&response).expect("serialize");
        assert_eq!(json["access_token"], "ya29.test");
        assert!(json["cost_estimate"].is_null());
    }

    #[test]
    fn access_token_response_with_cost_estimate() {
        let response = AccessTokenResponse {
            access_token: "ya29.test".into(),
            expires_at: "2025-01-01T00:00:00Z".into(),
            billing_project: "my-project".into(),
            query_hash: String::new(),
            cost_estimate: Some(CostEstimate {
                bytes_processed: 1_000_000_000_000,
                estimated_cost_usd: 6.25,
            }),
        };
        let json = serde_json::to_value(&response).expect("serialize");
        let cost = &json["cost_estimate"];
        assert_eq!(cost["bytes_processed"], 1_000_000_000_000_i64);
        assert_eq!(cost["estimated_cost_usd"], 6.25);
    }

    // --- ListProjectsQuery deserialization ---

    #[test]
    fn list_projects_query_defaults_to_false() {
        // Simulate empty query string: no parameters provided
        let query: ListProjectsQuery =
            serde_urlencoded::from_str("").expect("deserialize empty query");
        assert!(!query.include_dataset_counts);
    }

    #[test]
    fn list_projects_query_include_dataset_counts_true() {
        let query: ListProjectsQuery =
            serde_urlencoded::from_str("include_dataset_counts=true")
                .expect("deserialize true");
        assert!(query.include_dataset_counts);
    }

    #[test]
    fn list_projects_query_include_dataset_counts_false() {
        let query: ListProjectsQuery =
            serde_urlencoded::from_str("include_dataset_counts=false")
                .expect("deserialize false");
        assert!(!query.include_dataset_counts);
    }

    // ===========================================================================
    // read-arrow: ReadArrowRequest deserialization
    // ===========================================================================

    #[test]
    fn read_arrow_request_minimal() {
        let json = r#"{"job_id": "job_abc123"}"#;
        let req: ReadArrowRequest = serde_json::from_str(json).expect("deserialize");
        assert_eq!(req.job_id, "job_abc123");
        assert!(req.project_id.is_none());
    }

    #[test]
    fn read_arrow_request_with_project_id() {
        let json = r#"{"job_id": "job_abc123", "project_id": "my-project"}"#;
        let req: ReadArrowRequest = serde_json::from_str(json).expect("deserialize");
        assert_eq!(req.job_id, "job_abc123");
        assert_eq!(req.project_id.as_deref(), Some("my-project"));
    }

    // ===========================================================================
    // read-arrow: BigQuery type → Arrow type mapping
    // ===========================================================================

    #[test]
    fn bq_type_to_arrow_int64() {
        use arrow_schema::DataType;
        assert_eq!(bq_type_to_arrow_type("INT64"), DataType::Int64);
        assert_eq!(bq_type_to_arrow_type("INTEGER"), DataType::Int64);
        assert_eq!(bq_type_to_arrow_type("int64"), DataType::Int64);
    }

    #[test]
    fn bq_type_to_arrow_float64() {
        use arrow_schema::DataType;
        assert_eq!(bq_type_to_arrow_type("FLOAT64"), DataType::Float64);
        assert_eq!(bq_type_to_arrow_type("FLOAT"), DataType::Float64);
        assert_eq!(bq_type_to_arrow_type("NUMERIC"), DataType::Float64);
        assert_eq!(bq_type_to_arrow_type("BIGNUMERIC"), DataType::Float64);
    }

    #[test]
    fn bq_type_to_arrow_boolean() {
        use arrow_schema::DataType;
        assert_eq!(bq_type_to_arrow_type("BOOL"), DataType::Boolean);
        assert_eq!(bq_type_to_arrow_type("BOOLEAN"), DataType::Boolean);
    }

    #[test]
    fn bq_type_to_arrow_string_types() {
        use arrow_schema::DataType;
        assert_eq!(bq_type_to_arrow_type("STRING"), DataType::Utf8);
        assert_eq!(bq_type_to_arrow_type("BYTES"), DataType::Utf8);
        assert_eq!(bq_type_to_arrow_type("GEOGRAPHY"), DataType::Utf8);
        assert_eq!(bq_type_to_arrow_type("JSON"), DataType::Utf8);
    }

    #[test]
    fn bq_type_to_arrow_datetime_as_string() {
        use arrow_schema::DataType;
        // Date/time types are kept as strings for DuckDB to parse
        assert_eq!(bq_type_to_arrow_type("DATE"), DataType::Utf8);
        assert_eq!(bq_type_to_arrow_type("TIME"), DataType::Utf8);
        assert_eq!(bq_type_to_arrow_type("DATETIME"), DataType::Utf8);
        assert_eq!(bq_type_to_arrow_type("TIMESTAMP"), DataType::Utf8);
    }

    #[test]
    fn bq_type_to_arrow_complex_as_string() {
        use arrow_schema::DataType;
        assert_eq!(bq_type_to_arrow_type("STRUCT"), DataType::Utf8);
        assert_eq!(bq_type_to_arrow_type("RECORD"), DataType::Utf8);
        assert_eq!(bq_type_to_arrow_type("ARRAY"), DataType::Utf8);
    }

    #[test]
    fn bq_type_to_arrow_unknown_defaults_to_string() {
        use arrow_schema::DataType;
        assert_eq!(bq_type_to_arrow_type("UNKNOWN_TYPE_XYZ"), DataType::Utf8);
    }

    // ===========================================================================
    // read-arrow: Cell extraction from BigQuery row format
    // ===========================================================================

    #[test]
    fn extract_bq_cell_returns_value() {
        let row = json!({"f": [{"v": "hello"}, {"v": "42"}, {"v": null}]});
        assert_eq!(extract_bq_cell(&row, 0), Some(&json!("hello")));
        assert_eq!(extract_bq_cell(&row, 1), Some(&json!("42")));
        assert_eq!(extract_bq_cell(&row, 2), Some(&Value::Null));
    }

    #[test]
    fn extract_bq_cell_out_of_bounds_returns_none() {
        let row = json!({"f": [{"v": "hello"}]});
        assert_eq!(extract_bq_cell(&row, 5), None);
    }

    #[test]
    fn extract_bq_cell_missing_f_returns_none() {
        let row = json!({"x": "y"});
        assert_eq!(extract_bq_cell(&row, 0), None);
    }

    // ===========================================================================
    // read-arrow: Full JSON→Arrow IPC conversion
    // ===========================================================================

    #[test]
    fn arrow_ipc_empty_result() {
        let schema = vec![
            json!({"name": "id", "type": "INT64", "mode": "REQUIRED"}),
            json!({"name": "name", "type": "STRING", "mode": "NULLABLE"}),
        ];
        let rows: Vec<Value> = vec![];

        let ipc_bytes = bigquery_json_to_arrow_ipc(&schema, &rows)
            .expect("should convert empty result");

        // Should produce valid IPC with schema header + EOS marker
        assert!(!ipc_bytes.is_empty(), "IPC bytes should not be empty");
        // The IPC stream should be at least a few bytes (schema + empty batch + EOS)
        assert!(ipc_bytes.len() > 8, "IPC bytes should contain schema");
    }

    #[test]
    fn arrow_ipc_string_columns() {
        let schema = vec![
            json!({"name": "city", "type": "STRING", "mode": "NULLABLE"}),
        ];
        let rows = vec![
            json!({"f": [{"v": "New York"}]}),
            json!({"f": [{"v": "London"}]}),
            json!({"f": [{"v": null}]}),
        ];

        let ipc_bytes = bigquery_json_to_arrow_ipc(&schema, &rows)
            .expect("should convert string columns");

        // Verify by reading back the IPC stream
        let batch = read_ipc_to_batch(&ipc_bytes);
        assert_eq!(batch.num_rows(), 3);
        assert_eq!(batch.num_columns(), 1);
        assert_eq!(batch.schema().field(0).name(), "city");
    }

    #[test]
    fn arrow_ipc_int64_columns() {
        let schema = vec![
            json!({"name": "count", "type": "INT64", "mode": "NULLABLE"}),
        ];
        // BigQuery returns integers as strings in JSON
        let rows = vec![
            json!({"f": [{"v": "42"}]}),
            json!({"f": [{"v": "0"}]}),
            json!({"f": [{"v": "-100"}]}),
            json!({"f": [{"v": null}]}),
        ];

        let ipc_bytes = bigquery_json_to_arrow_ipc(&schema, &rows)
            .expect("should convert int64 columns");

        let batch = read_ipc_to_batch(&ipc_bytes);
        assert_eq!(batch.num_rows(), 4);

        // Verify values
        let col = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::Int64Array>()
            .expect("should be Int64Array");
        assert_eq!(col.value(0), 42);
        assert_eq!(col.value(1), 0);
        assert_eq!(col.value(2), -100);
        assert!(col.is_null(3));
    }

    #[test]
    fn arrow_ipc_float64_columns() {
        let schema = vec![
            json!({"name": "price", "type": "FLOAT64", "mode": "NULLABLE"}),
        ];
        let rows = vec![
            json!({"f": [{"v": "3.14"}]}),
            json!({"f": [{"v": "0.0"}]}),
            json!({"f": [{"v": null}]}),
        ];

        let ipc_bytes = bigquery_json_to_arrow_ipc(&schema, &rows)
            .expect("should convert float64 columns");

        let batch = read_ipc_to_batch(&ipc_bytes);
        assert_eq!(batch.num_rows(), 3);

        let col = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::Float64Array>()
            .expect("should be Float64Array");
        assert!((col.value(0) - 3.14).abs() < 1e-10);
        assert!((col.value(1) - 0.0).abs() < 1e-10);
        assert!(col.is_null(2));
    }

    #[test]
    fn arrow_ipc_boolean_columns() {
        let schema = vec![
            json!({"name": "active", "type": "BOOL", "mode": "NULLABLE"}),
        ];
        // BigQuery returns booleans as "true"/"false" strings in JSON
        let rows = vec![
            json!({"f": [{"v": "true"}]}),
            json!({"f": [{"v": "false"}]}),
            json!({"f": [{"v": null}]}),
        ];

        let ipc_bytes = bigquery_json_to_arrow_ipc(&schema, &rows)
            .expect("should convert boolean columns");

        let batch = read_ipc_to_batch(&ipc_bytes);
        assert_eq!(batch.num_rows(), 3);

        let col = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::BooleanArray>()
            .expect("should be BooleanArray");
        assert!(col.value(0));
        assert!(!col.value(1));
        assert!(col.is_null(2));
    }

    #[test]
    fn arrow_ipc_mixed_columns() {
        // Test a realistic BigQuery result with multiple column types
        let schema = vec![
            json!({"name": "id", "type": "INT64", "mode": "REQUIRED"}),
            json!({"name": "name", "type": "STRING", "mode": "NULLABLE"}),
            json!({"name": "revenue", "type": "FLOAT64", "mode": "NULLABLE"}),
            json!({"name": "is_active", "type": "BOOLEAN", "mode": "NULLABLE"}),
            json!({"name": "created_at", "type": "TIMESTAMP", "mode": "NULLABLE"}),
        ];
        let rows = vec![
            json!({"f": [
                {"v": "1"},
                {"v": "Alice"},
                {"v": "1234.56"},
                {"v": "true"},
                {"v": "2024-01-15 10:30:00 UTC"}
            ]}),
            json!({"f": [
                {"v": "2"},
                {"v": "Bob"},
                {"v": null},
                {"v": "false"},
                {"v": "2024-02-20 14:00:00 UTC"}
            ]}),
        ];

        let ipc_bytes = bigquery_json_to_arrow_ipc(&schema, &rows)
            .expect("should convert mixed columns");

        let batch = read_ipc_to_batch(&ipc_bytes);
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.num_columns(), 5);

        // Verify schema field names
        assert_eq!(batch.schema().field(0).name(), "id");
        assert_eq!(batch.schema().field(1).name(), "name");
        assert_eq!(batch.schema().field(2).name(), "revenue");
        assert_eq!(batch.schema().field(3).name(), "is_active");
        assert_eq!(batch.schema().field(4).name(), "created_at");

        // Verify a few values
        let id_col = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::Int64Array>()
            .expect("id should be Int64Array");
        assert_eq!(id_col.value(0), 1);
        assert_eq!(id_col.value(1), 2);

        let name_col = batch
            .column(1)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .expect("name should be StringArray");
        assert_eq!(name_col.value(0), "Alice");
        assert_eq!(name_col.value(1), "Bob");

        // TIMESTAMP is stored as Utf8 string
        let ts_col = batch
            .column(4)
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .expect("created_at should be StringArray");
        assert_eq!(ts_col.value(0), "2024-01-15 10:30:00 UTC");
    }

    #[test]
    fn arrow_ipc_date_types_as_strings() {
        // Verify that all date/time types are preserved as strings for DuckDB parsing
        let schema = vec![
            json!({"name": "date_col", "type": "DATE", "mode": "NULLABLE"}),
            json!({"name": "time_col", "type": "TIME", "mode": "NULLABLE"}),
            json!({"name": "datetime_col", "type": "DATETIME", "mode": "NULLABLE"}),
            json!({"name": "timestamp_col", "type": "TIMESTAMP", "mode": "NULLABLE"}),
        ];
        let rows = vec![json!({"f": [
            {"v": "2024-03-15"},
            {"v": "14:30:00"},
            {"v": "2024-03-15T14:30:00"},
            {"v": "1.710512e+09"}
        ]})];

        let ipc_bytes = bigquery_json_to_arrow_ipc(&schema, &rows)
            .expect("should convert date/time columns");

        let batch = read_ipc_to_batch(&ipc_bytes);
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 4);

        // All should be Utf8
        for i in 0..4 {
            assert_eq!(
                *batch.schema().field(i).data_type(),
                arrow_schema::DataType::Utf8,
                "column {} should be Utf8",
                i
            );
        }
    }

    // ===========================================================================
    // read-arrow: execution time extraction
    // ===========================================================================

    #[test]
    fn extract_execution_time_from_job() {
        let job = json!({
            "statistics": {
                "startTime": "1710500000000",
                "endTime": "1710500002500"
            }
        });
        assert_eq!(extract_execution_time_ms(&job), Some(2500));
    }

    #[test]
    fn extract_execution_time_missing_statistics() {
        let job = json!({"status": {"state": "DONE"}});
        assert_eq!(extract_execution_time_ms(&job), None);
    }

    #[test]
    fn extract_execution_time_missing_end_time() {
        let job = json!({
            "statistics": {
                "startTime": "1710500000000"
            }
        });
        assert_eq!(extract_execution_time_ms(&job), None);
    }

    // ===========================================================================
    // read-arrow: IPC stream validation helper
    // ===========================================================================

    /// Helper: read an Arrow IPC stream back into a RecordBatch for verification.
    fn read_ipc_to_batch(ipc_bytes: &[u8]) -> arrow_array::RecordBatch {
        use arrow_ipc::reader::StreamReader;
        use std::io::Cursor;

        let cursor = Cursor::new(ipc_bytes);
        let mut reader = StreamReader::try_new(cursor, None)
            .expect("should parse IPC stream");
        reader
            .next()
            .expect("should have at least one batch")
            .expect("batch should be valid")
    }
}
