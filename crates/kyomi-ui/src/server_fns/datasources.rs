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
//! - `POST /datasources/discover` → `discover_datasource_resources()`
//!
//! Each function calls directly into `kyomi_auth::datasource_service` — the
//! REST route handlers that predated this module were deleted wholesale in
//! the React→Leptos migration (KYO-73, #183).

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};
#[cfg(feature = "ssr")]
use kyomi_auth::datasource_service::{
    DiscoveryConnectionInputs, DiscoveryConnectionRequest, DiscoveryPrepError,
};
#[cfg(feature = "ssr")]
use kyomi_types::Permission;

// ─── Types ──────────────────────────────────────────────────────────────────

pub use kyomi_types::DatasourceInfo;

/// A single auth mode, with everything a client needs to render it carried
/// alongside the id so the client never has to re-derive (or hardcode) the
/// display name, description, or "recommended" status.
///
/// `description` and `is_default` are used by the four connection Auth Mode
/// Selectors (`*AuthModeSection` in `pages/settings/datasources.rs`, KYO-274);
/// `is_default` is also read there to render a "(Recommended)" suffix on the
/// default mode's label — that suffix is a presentation affordance, not part
/// of `display_name`, so it stays out of this wire type and every other
/// registry consumer (e.g. a future API response) isn't stuck with it baked
/// into the name.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthModeOption {
    pub mode_id: String,
    pub display_name: String,
    pub description: String,
    pub is_default: bool,
}

/// A datasource type from the registry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatasourceTypeInfo {
    pub type_id: String,
    pub display_name: String,
    /// Auth modes usable for headless catalog-indexing credentials — a
    /// strict subset of the type's full (connection) auth modes. Interactive
    /// OAuth modes and credential-less modes (e.g. flaredb's `none`) are
    /// excluded server-side; see
    /// `kyomi_core::datasource_registry::DatasourceTypeMetadata::indexing_auth_modes`.
    /// Empty means the indexing-credentials selector should stay hidden for
    /// this type.
    pub indexing_auth_modes: Vec<AuthModeOption>,
    /// The type's full set of connection auth modes — every mode a user can
    /// select to connect to (not just headless-index) this datasource type.
    /// Drives the four `*AuthModeSection` Authentication Mode selectors
    /// (KYO-274). Unlike `indexing_auth_modes`, this is unfiltered: it is
    /// exactly `DatasourceTypeMetadata::auth_modes` for the type.
    pub connection_auth_modes: Vec<AuthModeOption>,
}

/// A freshly generated SSH keypair for a datasource's SSH tunnel.
pub use kyomi_types::GeneratedSshKey;

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
            indexing_auth_modes: meta
                .indexing_auth_modes()
                .map(|m| AuthModeOption {
                    mode_id: m.mode_id.clone(),
                    display_name: m.display_name.clone(),
                    description: m.description.clone(),
                    is_default: m.is_default,
                })
                .collect(),
            connection_auth_modes: meta
                .auth_modes
                .iter()
                .map(|m| AuthModeOption {
                    mode_id: m.mode_id.clone(),
                    display_name: m.display_name.clone(),
                    description: m.description.clone(),
                    is_default: m.is_default,
                })
                .collect(),
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

    ac.require(Permission::ManageDatasources, "Workspace admin access required")?;

    kyomi_auth::datasource_service::delete_datasource(ac.db(), &datasource_id, &ac.ws_id)
        .await
        .into_sfn()?;

    Ok(())
}

/// Generate a new Ed25519 SSH keypair for a datasource's SSH tunnel (workspace
/// admin only). The private key comes back in plaintext — it is encrypted
/// with the workspace encryption key only when the datasource is actually
/// saved (see `finalize_connection_config_secrets`), same as any other
/// `connection_config` secret the user types into the form.
///
/// No REST counterpart: the datasources REST router was removed (PR #183);
/// this is served exclusively through the `/leptos-api/{*fn_name}` catch-all.
#[server(prefix = "/leptos-api")]
pub async fn generate_ssh_key() -> Result<GeneratedSshKey, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    ac.require(Permission::ManageDatasources, "Workspace admin access required")?;

    let generated = kyomi_auth::ssh_keygen::generate_ssh_keypair().into_sfn()?;

    Ok(generated)
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
    /// Per-key discovery failures, keyed the same as `resources` — a key
    /// present here means that specific `list_*` call failed and is
    /// therefore absent from `resources` (never both, never silently
    /// dropped with no trace of either). Distinct from `success: false`:
    /// this fn already reports `success: false` for connection-level
    /// failures (bad credentials, timeout, `test_connection()` returning
    /// `false`) before any discovery call runs, so by the time
    /// `resource_errors` can be non-empty the connection itself is known
    /// good — only a specific resource listing failed. Some providers
    /// return more than one pair (e.g. `databases` + `schemas`), and one
    /// failing must not blank the ones that succeeded (KYO-466) — that is
    /// the reason this is a per-key map rather than a single flag.
    /// Sanitized for client display the same way `message` is
    /// (`kyomi_core::sanitize_error`); the raw reason is logged
    /// server-side in `discover_datasource_resources` before sanitizing.
    pub resource_errors: std::collections::HashMap<String, String>,
    pub message: String,
}

/// Catalog statistics for a datasource.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogStatsResult {
    pub table_count: i64,
    pub schema_count: i64,
    pub last_indexed: Option<String>,
    /// True if this datasource's most recent catalog refresh failed
    /// (KYO-126, made genuinely per-datasource by KYO-267 —
    /// `catalog_refresh_status` now lives on `datasource_configs`, so no
    /// cross-datasource attribution step is needed to know whose failure
    /// this is). Deliberately a plain `bool`, not the typed
    /// `kyomi_core::enums::CatalogRefreshStatus` — `kyomi-ui` compiles to
    /// wasm32 and `kyomi-core` is an `ssr`-only dependency (see
    /// `docs/CODING_STANDARDS.md`).
    pub refresh_failed: bool,
    /// Human-readable reason for the failure, when one was recorded.
    /// `None` even when `refresh_failed` is `true` means no specific reason
    /// was available (falls back to a generic message in the UI).
    pub refresh_failure_reason: Option<String>,
    /// Per-container/per-table discovery errors from the most recent
    /// refresh, when that refresh *succeeded overall* but one or more
    /// containers/schemas could not be read (KYO-327) — e.g. tables were
    /// found and cached, but a permission-denied schema was skipped. Always
    /// empty when `refresh_failed` is `true`: a hard failure already has its
    /// own red error Alert (`refresh_failure_reason`), and showing both
    /// would double-report the same underlying denial. Populated from the
    /// persisted envelope's top-level `"warnings"` array (see
    /// `kyomi_auth::catalog::helpers::update_datasource_status`), never by
    /// parsing `refresh_failure_reason`'s collapsed text.
    pub refresh_warnings: Vec<String>,
}

/// Create a new datasource (admin only).
///
/// Mirrors `POST /api/v1/datasources` + optionally `POST /api/v1/datasources/{id}/credentials`.
///
/// `input = server_fn::codec::Json` (KYO-428): `connection_config` and
/// `credentials` are `serde_json::Value` — a self-describing type. Under the
/// default `PostUrl` encoding, `serde_qs` deserializes every leaf of a
/// self-describing target as a JSON string, so a numeric `port` or a boolean
/// `secure`/`encrypt`/`trust_server_certificate` field arrives as
/// `Value::String("5434")`/`Value::String("true")` instead of
/// `Value::Number`/`Value::Bool`. The driver-side `.as_u64()`/`.as_bool()`
/// then returns `None` and silently falls back to the provider default —
/// dropping the user's port, or worse, a TLS setting. JSON preserves the
/// original types on the wire, matching `update_dashboard` (dashboards.rs).
#[server(prefix = "/leptos-api", input = server_fn::codec::Json)]
pub async fn create_datasource_modal(
    name: String,
    slug: String,
    datasource_type: String,
    connection_config: serde_json::Value,
    credentials: serde_json::Value,
) -> Result<DatasourceResult, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    ac.require(Permission::ManageDatasources, "Workspace admin access required")?;

    let slug_opt = if slug.is_empty() { None } else { Some(slug.as_str()) };
    let encryption_key = ac.encryption_key()?;

    let ds = kyomi_auth::datasource_service::create_datasource(
        ac.db(),
        kyomi_auth::datasource_service::CreateDatasourceParams {
            workspace_id: &ac.ws_id,
            name: &name,
            slug: slug_opt,
            ds_type: &datasource_type,
            connection_config,
            connection_type: Some("direct"),
            encryption_key: &encryption_key,
        },
    )
    .await
    .into_sfn()?;

    // Save credentials if provided
    let has_creds = credentials.as_object().map(|o| !o.is_empty()).unwrap_or(false);
    if has_creds {
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
///
/// `input = server_fn::codec::Json` (KYO-428) — see `create_datasource_modal`
/// for why a `serde_json::Value` argument needs the JSON codec rather than
/// the default `PostUrl`.
#[server(prefix = "/leptos-api", input = server_fn::codec::Json)]
pub async fn update_datasource_settings(
    datasource_id: String,
    name: String,
    slug: String,
    connection_config: serde_json::Value,
) -> Result<DatasourceResult, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    ac.require(Permission::ManageDatasources, "Workspace admin access required")?;

    let slug_opt = if slug.is_empty() { None } else { Some(slug.as_str()) };
    let name_opt = if name.is_empty() { None } else { Some(name.as_str()) };
    let encryption_key = ac.encryption_key()?;

    let updated = kyomi_auth::datasource_service::update_datasource(
        ac.db(),
        &datasource_id,
        &ac.ws_id,
        name_opt,
        slug_opt,
        Some(connection_config),
        None,
        None,
        &encryption_key,
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
///
/// `input = server_fn::codec::Json` (KYO-428): today every leaf
/// `build_credentials()` (pages/settings/datasources.rs) puts in this map is
/// a string, so decoding it via the default `PostUrl` codec happens to
/// produce the same `serde_json::Value` a JSON codec would — there is no
/// live bug here today. But `create_datasource_modal` accepts the *same*
/// credentials map (built by the same caller, over the same wire type) and
/// now decodes it as JSON per this ticket's fix. Leaving this function on
/// `PostUrl` would make the map's decoded shape depend on which of the two
/// call sites sent it — silently correct only as long as no credential
/// field is ever a number or boolean. Matching the codec here removes that
/// path-dependent trap instead of leaving it for the next field to trip over.
#[server(prefix = "/leptos-api", input = server_fn::codec::Json)]
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

    let is_admin = ac.has(Permission::ManageDatasources);

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

/// Overlay caller-supplied credential fields on top of the stored ones.
/// Fields present in `provided` win; absent fields fall back to `stored`.
/// Keeps Test & Discover working with the saved password when the user
/// leaves the field blank, while still honoring a newly-typed password.
///
/// Unlike the save-path `merge_credentials` (kyomi-auth), this deliberately
/// overlays *every* provided key with no OAuth-field exclusion. That is safe
/// here because this path is per-user (creds are looked up by the caller's own
/// `user_id`) and discover-only — nothing is persisted, so a caller can only
/// overlay their own already-fully-controlled credential map. If this path ever
/// gains persistence, mirror `merge_credentials`'s `OAUTH_FIELDS` protection.
#[cfg(feature = "ssr")]
fn overlay_credentials(stored: serde_json::Value, provided: &serde_json::Value) -> serde_json::Value {
    match (stored.as_object(), provided.as_object()) {
        (Some(s), Some(p)) => {
            let mut merged = s.clone();
            for (k, v) in p {
                merged.insert(k.clone(), v.clone());
            }
            serde_json::Value::Object(merged)
        }
        // If either side isn't an object, prefer provided when it's non-null, else stored.
        _ => {
            if provided.is_null() { stored } else { provided.clone() }
        }
    }
}

/// Discover available resources (databases, schemas, warehouses, etc.) for a datasource.
///
/// Uses provider-specific list methods (list_databases, list_schemas, list_warehouses, etc.),
/// the same mapping the REST route's `discover_all_resources()` used before that
/// route (`catalog.rs`) was deleted wholesale in the React→Leptos migration
/// (KYO-73, #182).
///
/// `input = server_fn::codec::Json` (KYO-428) — see `create_datasource_modal`
/// for why `connection_config` (a `serde_json::Value`) needs the JSON codec
/// rather than the default `PostUrl`.
#[server(prefix = "/leptos-api", input = server_fn::codec::Json)]
pub async fn discover_datasource_resources(
    datasource_type: String,
    connection_config: serde_json::Value,
    credentials: serde_json::Value,
    datasource_slug: Option<String>,
) -> Result<DiscoverResourcesResult, ServerFnError> {
    use std::str::FromStr as _;

    // `auth_mode` lives inside `connection_config` for every provider that
    // has more than one (BigQuery, Snowflake, Databricks, Synapse) — see
    // `build_connection_config` in `pages/settings/datasources.rs`, which
    // sets it under the `"auth_mode"` key before calling this fn. Providers
    // with a single mode (Postgres, MySQL, ClickHouse, Redshift, SQL
    // Server, FlareDB) never set it, so this stays `"none"` for them rather
    // than being invented (KYO-469).
    let auth_mode = connection_config
        .get("auth_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("none")
        .to_string();
    let has_slug = datasource_slug.is_some();

    tracing::info!(
        datasource_type = %datasource_type,
        auth_mode = %auth_mode,
        has_slug,
        "discover_datasource_resources: request received"
    );

    let ac = match AuthenticatedContext::extract().await {
        Ok(ac) => ac,
        Err(e) => {
            tracing::warn!(
                datasource_type = %datasource_type,
                auth_mode = %auth_mode,
                has_slug,
                error = %e,
                "discover_datasource_resources: failed to authenticate request"
            );
            return Err(e);
        }
    };

    let ds_type = match kyomi_core::datasource_registry::DatasourceType::from_str(&datasource_type)
        .into_sfn()
    {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(
                datasource_type = %datasource_type,
                auth_mode = %auth_mode,
                has_slug,
                error = %e,
                "discover_datasource_resources: unrecognized datasource type"
            );
            return Err(e);
        }
    };

    let encryption_key = match ac.encryption_key() {
        Ok(k) => k,
        Err(e) => {
            tracing::warn!(
                datasource_type = %datasource_type,
                auth_mode = %auth_mode,
                has_slug,
                error = %e,
                "discover_datasource_resources: failed to resolve encryption key"
            );
            return Err(e);
        }
    };

    // Resolve the stored-credential lookup, connection-config/credential
    // decrypt, and OAuth `UserContext` build in one service call — this
    // function can run before the datasource is persisted (the Connection
    // tab's "Validate & Discover Projects"), so it goes through
    // `resolve_discovery_connection_inputs` rather than
    // `build_provider_for_datasource`, which requires an already-resolved
    // `&DatasourceConfig` (KYO-445). Extracted into kyomi-auth (rather than
    // inlined here) to keep this fn's service-layer callout count under
    // `check-server-fns.sh`'s Rule B threshold.
    let DiscoveryConnectionInputs {
        connection_config,
        stored_creds,
        user_context,
    } = match kyomi_auth::datasource_service::resolve_discovery_connection_inputs(
        ac.db(),
        DiscoveryConnectionRequest {
            user_id: &ac.auth.user_id,
            ws_id: &ac.ws_id,
            datasource_slug: datasource_slug.as_deref(),
            connection_config: &connection_config,
            encryption_key: &encryption_key,
            google_client_id: ac.ctx.config.google_oauth_client_id.as_deref(),
            google_client_secret: ac.ctx.config.google_oauth_client_secret.as_deref(),
            user_email: ac.auth.email.clone(),
        },
    )
    .await
    {
        Ok(inputs) => inputs,
        Err(DiscoveryPrepError::Decrypt(e)) => {
            tracing::warn!(
                datasource_type = %datasource_type,
                auth_mode = %auth_mode,
                has_slug,
                error = %e,
                "discover_datasource_resources: credential decrypt failed before resource discovery"
            );
            return Ok(DiscoverResourcesResult {
                success: false,
                resources: std::collections::HashMap::new(),
                resource_errors: std::collections::HashMap::new(),
                message: e.to_string(),
            });
        }
        Err(DiscoveryPrepError::UserContext(e)) => {
            tracing::warn!(
                datasource_type = %datasource_type,
                auth_mode = %auth_mode,
                has_slug,
                raw_error = %e,
                "discover_datasource_resources: failed to build user context for datasource discovery"
            );
            return Ok(DiscoverResourcesResult {
                success: false,
                resources: std::collections::HashMap::new(),
                resource_errors: std::collections::HashMap::new(),
                message: format!("Failed to connect: {}", kyomi_core::sanitize_error(&e.to_string())),
            });
        }
    };
    let resolved_creds = overlay_credentials(stored_creds, &credentials);

    let provider = match tokio::time::timeout(
        kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT,
        kyomi_datasource_server::create_provider(
            &ds_type,
            &connection_config,
            &resolved_creds,
            user_context.as_ref(),
        ),
    )
    .await
    {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            tracing::warn!(
                datasource_type = %datasource_type,
                auth_mode = %auth_mode,
                has_slug,
                raw_error = %e,
                "discover_datasource_resources: datasource connection error (sanitized for client)"
            );
            return Ok(DiscoverResourcesResult {
                success: false,
                resources: std::collections::HashMap::new(),
                resource_errors: std::collections::HashMap::new(),
                message: format!("Failed to connect: {}", kyomi_core::sanitize_error(&e.to_string())),
            });
        }
        Err(_) => {
            tracing::warn!(
                datasource_type = %datasource_type,
                auth_mode = %auth_mode,
                has_slug,
                "discover_datasource_resources: create_provider timed out"
            );
            return Ok(DiscoverResourcesResult {
                success: false,
                resources: std::collections::HashMap::new(),
                resource_errors: std::collections::HashMap::new(),
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
        tracing::warn!(
            datasource_type = %datasource_type,
            auth_mode = %auth_mode,
            has_slug,
            "discover_datasource_resources: test_connection reported failure"
        );
        provider.close().await;
        return Ok(DiscoverResourcesResult {
            success: false,
            resources: std::collections::HashMap::new(),
            resource_errors: std::collections::HashMap::new(),
            message: "Connection test failed — check your credentials".to_string(),
        });
    }

    // Discover all resources using the same mapping as catalog.rs `discover_all_resources()`
    let type_str = ds_type.as_str();

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
        "bigquery" => {
            // Unlike the other providers' `list_*` methods above, BigQuery's
            // `list_projects()` returns a plain `Result<Vec<String>, _>` (it
            // hits GCP's Resource Manager API, a genuinely different surface
            // from the SQL-backed `list_databases`/`list_schemas` calls).
            // Adapt it into the same `DiscoveryResult` shape the loop below
            // expects. Many service accounts (e.g. anything scoped to
            // "BigQuery Job User") lack `resourcemanager.projects.list` —
            // that must become a captured `DiscoveryResult` error, not a
            // propagated one, so the loop below reports it as a per-key
            // discovery failure (KYO-466) instead of failing discovery as a
            // whole.
            let projects = bigquery_projects_discovery_result(provider.list_projects().await);
            vec![("projects", projects)]
        }
        _ => vec![],
    };

    let (resources_map, resource_errors) = build_resources_map(discovery_pairs);

    // Log every per-key discovery failure before sanitizing — this is the
    // trace KYO-466 found completely absent from production: the original
    // report could not be diagnosed because a failing `list_projects()` left
    // no server-side record at all. Kept inside this fn (rather than inside
    // `build_resources_map`) so it carries the same three attribution fields
    // as every other `tracing::warn!` here (KYO-469's shape).
    for (key, reason) in &resource_errors {
        tracing::warn!(
            datasource_type = %datasource_type,
            auth_mode = %auth_mode,
            has_slug,
            resource_key = %key,
            error = %reason,
            "discover_datasource_resources: failed to list resources for one discovery key"
        );
    }

    provider.close().await;

    // Sanitized for the client the same way every other message in this fn
    // is (KYO-448) — the raw reason was already logged above.
    let resource_errors: std::collections::HashMap<String, String> = resource_errors
        .into_iter()
        .map(|(key, reason)| (key, kyomi_core::sanitize_error(&reason)))
        .collect();

    let message = discovery_outcome_message(&resources_map, &resource_errors);

    Ok(DiscoverResourcesResult {
        success: true,
        resources: resources_map,
        resource_errors,
        message,
    })
}

/// Choose the client-facing message for a completed discovery call, given
/// the final resources/errors split `build_resources_map` produced.
///
/// KYO-466: `"Connection successful and resources discovered"` was returned
/// unconditionally, which is false whenever nothing was actually
/// discovered. Extracted as a pure function (rather than inlined in
/// `discover_datasource_resources`, which needs a live provider connection
/// to exercise at all) specifically so the three outcomes it distinguishes
/// can be unit tested directly:
/// - at least one resource came back → resources were genuinely discovered
/// - nothing came back, but nothing errored either → a real empty result
///   (e.g. a BigQuery account with zero listable projects)
/// - nothing came back and at least one key errored → discovery could not
///   complete, and `resource_errors` carries the per-key reason(s)
///
/// Neither current client reads this string on the success path (both
/// `resources: HashMap<String, Vec<String>>` consumers in
/// `pages/settings/datasources.rs` derive their own copy instead —
/// `EditModeCatalogTab`'s Catalog-tab Effect builds a per-provider-noun
/// message via `catalog_item_label_for_type`, and `test_action`'s
/// `ConnectionTestResultBadge` never renders `TestConnectionResult.message`
/// at all when `success` is `true`, only its `success_label` prop). That is
/// deliberate, not a sign this fn is dead: this is still the field every
/// non-Leptos caller of `discover_datasource_resources` (a script, a future
/// API consumer, a log line) sees, and the three-outcome distinction it
/// computes is exactly what `discovery_outcome_message_tests` below pins.
/// Wiring it into either client would only *replace* a more specific,
/// per-provider-noun string with a generic one — not add information.
#[cfg(feature = "ssr")]
fn discovery_outcome_message(
    resources: &std::collections::HashMap<String, Vec<String>>,
    resource_errors: &std::collections::HashMap<String, String>,
) -> String {
    let discovered_any = resources.values().any(|items| !items.is_empty());
    if discovered_any {
        "Connection successful and resources discovered".to_string()
    } else if !resource_errors.is_empty() {
        "Connected, but some resources could not be listed — see details below".to_string()
    } else {
        "Connected, but no resources were found".to_string()
    }
}

/// Convert BigQuery's `list_projects()` result into the `DiscoveryResult`
/// shape every other discovery call already returns natively. An `Err`
/// (typically a service account missing `resourcemanager.projects.list`)
/// becomes a `DiscoveryResult` carrying the error message with empty items —
/// never a propagated `Result::Err` — so it can flow through the same
/// per-key handling in [`build_resources_map`] as every other discovery
/// pair.
#[cfg(feature = "ssr")]
fn bigquery_projects_discovery_result(
    result: kyomi_connect_protocol::Result<Vec<String>>,
) -> kyomi_datasource_server::DiscoveryResult {
    match result {
        Ok(items) => kyomi_datasource_server::DiscoveryResult { items, error: None },
        Err(e) => kyomi_datasource_server::DiscoveryResult {
            items: vec![],
            error: Some(e.to_string()),
        },
    }
}

/// Split a set of `(key, DiscoveryResult)` pairs into successfully
/// discovered items and per-key discovery errors, rather than surfacing one
/// erroring pair as an overall discovery failure.
///
/// A key that errored (e.g. BigQuery's `list_projects()` for a service
/// account without Resource Manager access) lands in the *second* returned
/// map, keyed the same way, and is absent from the first — never both,
/// never dropped with no trace of either (KYO-466: silently dropping the
/// key with no error channel is exactly what made "no projects" and
/// "couldn't list projects" indistinguishable to the client). This is the
/// mechanism that lets `discover_datasource_resources` keep reporting
/// `success: true` — the connection itself worked — while still telling the
/// client which specific resource list it could not enumerate, and why.
/// Some providers return more than one pair (e.g. `databases` + `schemas`),
/// and one failing must not blank the pairs that succeeded — the reason
/// this is a per-key map rather than a single flag.
#[cfg(feature = "ssr")]
fn build_resources_map(
    pairs: Vec<(&str, kyomi_datasource_server::DiscoveryResult)>,
) -> (
    std::collections::HashMap<String, Vec<String>>,
    std::collections::HashMap<String, String>,
) {
    let mut resources_map = std::collections::HashMap::new();
    let mut errors_map = std::collections::HashMap::new();
    for (key, result) in pairs {
        match result.error {
            None => {
                resources_map.insert(
                    key.to_string(),
                    result.items, // items is already Vec<String>
                );
            }
            Some(reason) => {
                errors_map.insert(key.to_string(), reason);
            }
        }
    }
    (resources_map, errors_map)
}

// ---------------------------------------------------------------------------
// Shared helper: create a query provider from a resolved datasource
// ---------------------------------------------------------------------------

/// Resolve a datasource by slug and create a provider ready for query execution.
///
/// Used by the dry-run and catalog server functions. Resolves the
/// datasource and checks the encryption key FIRST — in that order — so a
/// bad slug still surfaces as "not found" rather than being masked by an
/// unrelated "encryption key not configured" error, exactly as before this
/// helper existed. Per-user credential decryption, the lazy `UserContext`
/// build, connection config decryption, and provider construction are then
/// delegated to the shared
/// `kyomi_auth::datasource_service::build_provider_for_datasource` helper —
/// mapping its raw `kyomi_core::Error` into a `ServerFnError` via `into_sfn()`.
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
    // Resolve datasource by slug (or UUID) FIRST.
    // `include_inactive = false` enforces the active constraint at the SQL level.
    let ds = kyomi_auth::datasource_service::resolve_datasource(
        &ctx.db,
        datasource_slug,
        workspace_id,
        false,
    )
    .await
    .into_sfn()?;

    // Encryption key check AFTER resolve: even Connect-type datasources
    // (which skip per-user credential decryption) still need it to decrypt
    // `connection_config` secrets below, so this unwrap is unconditional —
    // but it must not run before the resolve above, or a bad slug would be
    // masked by this error instead of surfacing as "not found".
    let encryption_key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    let provider = kyomi_auth::datasource_service::build_provider_for_datasource(
        &ctx.db,
        &auth.user_id,
        &ds,
        encryption_key,
        || async {
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
        },
        ctx.connect_registry.as_ref(),
    )
    .await
    .into_sfn()?;

    Ok((ds, provider))
}

/// Extract the structured `"warnings"` array from a datasource's persisted
/// `catalog_refresh_progress` envelope (see
/// `kyomi_auth::catalog::helpers::update_datasource_status`), for a
/// non-failed refresh only.
///
/// KYO-327: when `refresh_failed` is `true`, always returns an empty vec —
/// a hard failure already has its own reason surfaced via
/// `refresh_failure_reason`/the settings page's red error Alert, so
/// returning warnings too would double-report the same underlying denial
/// through a second Alert. Extracted as a pure function (mirrors
/// `overlay_credentials` above) so it's unit-testable without standing up
/// the `#[server]` fn's request-extracted `AuthenticatedContext`.
///
/// `ssr`-only, same as `overlay_credentials`: its sole caller is
/// `get_catalog_stats`' server-side body, so under the `hydrate`/wasm32
/// build it would otherwise be dead code and fail the `-D warnings` clippy
/// gate.
#[cfg(feature = "ssr")]
fn extract_refresh_warnings(
    refresh_failed: bool,
    progress: Option<&serde_json::Value>,
) -> Vec<String> {
    if refresh_failed {
        return Vec::new();
    }

    progress
        .and_then(|progress| progress.get("warnings"))
        .and_then(|w| w.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
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

    // KYO-267: catalog_refresh_status/progress now live directly on this
    // datasource's own row, so no cross-datasource attribution step is
    // needed — a failure recorded here can only ever belong to this
    // datasource.
    #[derive(sqlx::FromRow)]
    struct DatasourceRefreshRow {
        catalog_refresh_status: Option<kyomi_core::enums::CatalogRefreshStatus>,
        catalog_refresh_progress: Option<serde_json::Value>,
    }
    let refresh_row = kyomi_core::db_fetch_optional!(
        ac.db(),
        DatasourceRefreshRow,
        "SELECT catalog_refresh_status, catalog_refresh_progress FROM datasource_configs \
         WHERE id = $1 AND workspace_id = $2",
        &datasource_id,
        &ac.ws_id
    )
    .into_sfn()?;

    let refresh_failed = refresh_row
        .as_ref()
        .is_some_and(|row| row.catalog_refresh_status == Some(kyomi_core::enums::CatalogRefreshStatus::Failed));

    // Extracted once so both branches below can read the same envelope
    // without a double-move of `refresh_row`.
    let refresh_progress = refresh_row.and_then(|row| row.catalog_refresh_progress);

    let refresh_failure_reason = if refresh_failed {
        refresh_progress
            .as_ref()
            .and_then(|progress| progress.get("error").and_then(|v| v.as_str()).map(str::to_string))
    } else {
        None
    };

    let refresh_warnings = extract_refresh_warnings(refresh_failed, refresh_progress.as_ref());

    Ok(CatalogStatsResult {
        table_count,
        schema_count,
        last_indexed,
        refresh_failed,
        refresh_failure_reason,
        refresh_warnings,
    })
}

#[cfg(all(test, feature = "ssr"))]
mod overlay_credentials_tests {
    use super::overlay_credentials;
    use serde_json::json;

    #[test]
    fn blank_password_in_provided_keeps_stored_password() {
        let stored = json!({ "username": "alice", "password": "s3cr3t" });
        // `build_credentials` only inserts non-empty fields, so a blank
        // password field means "password" is absent from `provided`.
        let provided = json!({ "username": "alice" });
        let merged = overlay_credentials(stored, &provided);
        assert_eq!(merged["password"], json!("s3cr3t"));
        assert_eq!(merged["username"], json!("alice"));
    }

    #[test]
    fn typed_password_in_provided_overrides_stored() {
        let stored = json!({ "username": "alice", "password": "old-password" });
        let provided = json!({ "username": "alice", "password": "new-password" });
        let merged = overlay_credentials(stored, &provided);
        assert_eq!(merged["password"], json!("new-password"));
    }

    #[test]
    fn empty_provided_object_leaves_stored_unchanged() {
        let stored = json!({ "username": "alice", "password": "s3cr3t" });
        let provided = json!({});
        let merged = overlay_credentials(stored.clone(), &provided);
        assert_eq!(merged, stored);
    }

    #[test]
    fn non_object_stored_falls_back_to_provided() {
        // Defensive edge case: `stored` isn't a JSON object at all (e.g.
        // `serde_json::Value::default()`, which is `Null`). Since it can't
        // be merged into, the caller-supplied fields should be used as-is.
        let stored = serde_json::Value::default();
        let provided = json!({ "username": "bob", "password": "typed-password" });
        let merged = overlay_credentials(stored, &provided);
        assert_eq!(merged, provided);
    }
}

// ── extract_refresh_warnings (KYO-327) ────────────────────────────────────

#[cfg(all(test, feature = "ssr"))]
mod extract_refresh_warnings_tests {
    use super::extract_refresh_warnings;
    use serde_json::json;

    /// Idle-with-warnings case: a partial run (tables found, some
    /// containers denied) resolves to `"idle"` (`refresh_failed == false`)
    /// but its envelope carries a non-empty `"warnings"` array — those must
    /// reach the caller.
    #[test]
    fn non_failed_run_with_warnings_returns_them() {
        let progress = json!({
            "warnings": [
                "Failed to list tables in schema 'restricted': permission denied"
            ],
        });
        let warnings = extract_refresh_warnings(false, Some(&progress));
        assert_eq!(
            warnings,
            vec!["Failed to list tables in schema 'restricted': permission denied".to_string()]
        );
    }

    /// Clean run: no errors at all during discovery, so the persisted
    /// envelope's `"warnings"` array is empty — must round-trip as an empty
    /// vec, not `None`/a missing field.
    #[test]
    fn clean_run_returns_empty_vec() {
        let progress = json!({ "warnings": [] });
        let warnings = extract_refresh_warnings(false, Some(&progress));
        assert!(warnings.is_empty());
    }

    /// A refresh row with no persisted progress at all (e.g. a pre-KYO-327
    /// row, or a datasource that has never been refreshed) must not panic
    /// and must report no warnings.
    #[test]
    fn missing_progress_returns_empty_vec() {
        let warnings = extract_refresh_warnings(false, None);
        assert!(warnings.is_empty());
    }

    /// Failed case: even when the envelope carries warnings, a hard failure
    /// must suppress them — `refresh_failure_reason` already reports the
    /// cause, and showing both would double-report the same denial.
    #[test]
    fn failed_run_suppresses_warnings_even_if_present() {
        let progress = json!({
            "warnings": ["Failed to list tables in schema 'a': permission denied"],
        });
        let warnings = extract_refresh_warnings(true, Some(&progress));
        assert!(
            warnings.is_empty(),
            "a failed run must not surface warnings alongside its own failure reason"
        );
    }
}

// ── BigQuery project discovery (KYO-405) ──────────────────────────────────

#[cfg(all(test, feature = "ssr"))]
mod bigquery_discovery_tests {
    use super::{bigquery_projects_discovery_result, build_resources_map};

    /// A successful `list_projects()` call becomes a `DiscoveryResult` with
    /// the project ids as items and no error.
    #[test]
    fn ok_projects_become_a_discovery_result_with_no_error() {
        let result: kyomi_connect_protocol::Result<Vec<String>> =
            Ok(vec!["proj-a".to_string(), "proj-b".to_string()]);
        let discovery = bigquery_projects_discovery_result(result);
        assert_eq!(
            discovery.items,
            vec!["proj-a".to_string(), "proj-b".to_string()]
        );
        assert!(discovery.error.is_none());
    }

    /// A `list_projects()` failure (the common case: a service account
    /// scoped to "BigQuery Job User" lacks `resourcemanager.projects.list`)
    /// must be captured on the `DiscoveryResult`, not propagated as a
    /// `Result::Err` out of `bigquery_projects_discovery_result` — that is
    /// what lets the caller keep going instead of failing discovery outright.
    #[test]
    fn permission_denied_error_is_captured_not_propagated() {
        let result: kyomi_connect_protocol::Result<Vec<String>> = Err(
            kyomi_connect_protocol::Error::Provider(
                "permission denied: resourcemanager.projects.list".to_string(),
            ),
        );
        let discovery = bigquery_projects_discovery_result(result);
        assert!(discovery.items.is_empty());
        assert_eq!(
            discovery.error.as_deref(),
            Some("permission denied: resourcemanager.projects.list")
        );
    }

    /// The exact regression this ticket fixes: a service account without
    /// Resource Manager access must not turn the whole "Validate & Discover
    /// Projects" call into a failure — `Next` still has to enable. That
    /// depends on `build_resources_map` omitting the errored "projects" pair
    /// from `resources` entirely rather than inserting an empty vec (which
    /// would be indistinguishable from a real empty project list) or
    /// aborting. KYO-466 adds the second half of this: the reason must not
    /// simply vanish either — it belongs in the returned error map.
    #[test]
    fn errored_projects_pair_is_dropped_from_resources_map_not_fatal() {
        let projects = bigquery_projects_discovery_result(Err(
            kyomi_connect_protocol::Error::Provider("permission denied".to_string()),
        ));
        let (resources, errors) = build_resources_map(vec![("projects", projects)]);
        assert!(
            !resources.contains_key("projects"),
            "an errored discovery item must be omitted from resources, not surfaced as an \
             empty vec"
        );
        assert_eq!(
            errors.get("projects").map(String::as_str),
            Some("permission denied"),
            "KYO-466: the error must reach the caller via the per-key error channel instead \
             of vanishing with no trace"
        );
    }

    /// A genuinely empty-but-successful project list (e.g. a fresh service
    /// account with no projects yet) is a real result and must still appear
    /// in the resources map with no corresponding entry in the error map —
    /// distinguishing it from the dropped-on-error case above is the whole
    /// point of KYO-466.
    #[test]
    fn empty_but_successful_projects_list_is_present_in_resources_map() {
        let projects = bigquery_projects_discovery_result(Ok(vec![]));
        let (resources, errors) = build_resources_map(vec![("projects", projects)]);
        assert_eq!(resources.get("projects"), Some(&Vec::<String>::new()));
        assert!(
            errors.is_empty(),
            "a successful (even empty) discovery must not also appear in the error map"
        );
    }
}

// ── build_resources_map: multi-pair error isolation (KYO-466) ─────────────

#[cfg(all(test, feature = "ssr"))]
mod build_resources_map_multi_pair_tests {
    //! `bigquery_discovery_tests` above covers the single-pair BigQuery
    //! case. Several other providers return *two* discovery pairs in one
    //! call (postgres/redshift/sqlserver/synapse: `databases` + `schemas`;
    //! snowflake: `warehouses` + `databases`) — this is the property the
    //! ticket's per-key error channel exists to protect: one pair failing
    //! must not blank the pair that succeeded.

    use super::build_resources_map;
    use kyomi_datasource_server::DiscoveryResult;

    /// The core regression this ticket guards against: an erroring
    /// `schemas` pair must not blank a succeeding `databases` pair.
    #[test]
    fn one_erroring_pair_does_not_blank_a_succeeding_pair() {
        let databases = DiscoveryResult {
            items: vec!["prod".to_string(), "staging".to_string()],
            error: None,
        };
        let schemas = DiscoveryResult {
            items: vec![],
            error: Some("permission denied listing schemas".to_string()),
        };
        let (resources, errors) =
            build_resources_map(vec![("databases", databases), ("schemas", schemas)]);

        assert_eq!(
            resources.get("databases"),
            Some(&vec!["prod".to_string(), "staging".to_string()]),
            "the succeeding pair must render fully even though a sibling pair failed"
        );
        assert!(
            !resources.contains_key("schemas"),
            "the failing pair's key must not appear in resources — an empty vec there would \
             be indistinguishable from a real empty schema list"
        );
        assert_eq!(
            errors.get("schemas").map(String::as_str),
            Some("permission denied listing schemas"),
            "the failing pair's reason must reach the caller via the error channel"
        );
        assert!(
            !errors.contains_key("databases"),
            "a succeeding pair must never appear in the error map"
        );
    }

    /// The reverse ordering — first pair fails, second succeeds — guards
    /// against an implementation that only isolates the failure correctly
    /// when it happens to come last in the `Vec`.
    #[test]
    fn failure_in_the_first_pair_still_preserves_the_second() {
        let warehouses = DiscoveryResult {
            items: vec![],
            error: Some("warehouse listing timed out".to_string()),
        };
        let databases = DiscoveryResult {
            items: vec!["analytics".to_string()],
            error: None,
        };
        let (resources, errors) =
            build_resources_map(vec![("warehouses", warehouses), ("databases", databases)]);

        assert_eq!(
            resources.get("databases"),
            Some(&vec!["analytics".to_string()])
        );
        assert!(!resources.contains_key("warehouses"));
        assert_eq!(
            errors.get("warehouses").map(String::as_str),
            Some("warehouse listing timed out")
        );
    }

    /// Both pairs failing must surface both reasons — neither error may
    /// overwrite or suppress the other.
    #[test]
    fn both_pairs_failing_surfaces_both_errors() {
        let databases = DiscoveryResult {
            items: vec![],
            error: Some("db error".to_string()),
        };
        let schemas = DiscoveryResult {
            items: vec![],
            error: Some("schema error".to_string()),
        };
        let (resources, errors) =
            build_resources_map(vec![("databases", databases), ("schemas", schemas)]);

        assert!(resources.is_empty());
        assert_eq!(errors.get("databases").map(String::as_str), Some("db error"));
        assert_eq!(errors.get("schemas").map(String::as_str), Some("schema error"));
    }
}

// ── discovery_outcome_message: three distinguishable outcomes (KYO-466) ───

#[cfg(all(test, feature = "ssr"))]
mod discovery_outcome_message_tests {
    //! The whole point of KYO-466: a discovery call that found resources,
    //! one that legitimately found none, and one that could not list at
    //! all must each produce a different client-facing message — before
    //! this fix all three said "Connection successful and resources
    //! discovered", which is only true of the first.

    use super::discovery_outcome_message;
    use std::collections::HashMap;

    /// At least one key came back with items → the original unconditional
    /// message is accurate and stays as-is.
    #[test]
    fn non_empty_resources_report_success() {
        let mut resources = HashMap::new();
        resources.insert("projects".to_string(), vec!["proj-a".to_string()]);
        let errors = HashMap::new();

        let message = discovery_outcome_message(&resources, &errors);
        assert_eq!(message, "Connection successful and resources discovered");
    }

    /// Nothing came back, and nothing errored either — a real empty
    /// result (e.g. a BigQuery account with zero listable projects). Must
    /// read differently from both the success case above and the error
    /// case below, or the client still cannot distinguish "no projects"
    /// from "couldn't list projects".
    #[test]
    fn empty_success_is_distinguishable_from_non_empty_success() {
        let resources = HashMap::new();
        let errors = HashMap::new();

        let message = discovery_outcome_message(&resources, &errors);
        assert_ne!(message, "Connection successful and resources discovered");
        assert_eq!(message, "Connected, but no resources were found");
    }

    /// Nothing came back and a key errored — must read differently from
    /// the empty-but-successful case above; this is the "couldn't list
    /// projects" outcome the ticket is about.
    #[test]
    fn discovery_error_is_distinguishable_from_empty_success() {
        let resources = HashMap::new();
        let mut errors = HashMap::new();
        errors.insert("projects".to_string(), "permission denied".to_string());

        let message = discovery_outcome_message(&resources, &errors);
        assert_ne!(message, "Connection successful and resources discovered");
        assert_ne!(message, "Connected, but no resources were found");
        assert_eq!(
            message,
            "Connected, but some resources could not be listed — see details below"
        );
    }

    /// A key with an empty-but-present `Vec` alongside another key that
    /// errored must still report success — `values().any(...)` looks
    /// across every key, not just the one that failed, matching
    /// `build_resources_map`'s guarantee that a succeeding pair is never
    /// blanked by a sibling failure.
    #[test]
    fn one_key_succeeding_reports_success_even_if_another_key_errored() {
        let mut resources = HashMap::new();
        resources.insert("databases".to_string(), vec!["prod".to_string()]);
        let mut errors = HashMap::new();
        errors.insert("schemas".to_string(), "permission denied".to_string());

        let message = discovery_outcome_message(&resources, &errors);
        assert_eq!(message, "Connection successful and resources discovered");
    }
}

// ── discover_datasource_resources builds a UserContext (KYO-445) ──────────

#[cfg(all(test, feature = "ssr"))]
mod discover_user_context_tests {
    //! KYO-445: `discover_datasource_resources` used to pass a literal
    //! `None` for `user_context`, so every BigQuery `kyomi_oauth` "Discover
    //! Available" call (and, since this function also runs the connection
    //! test, every "test connection" call on the same datasource) failed
    //! with "BigQuery kyomi_oauth mode requires user context with OAuth
    //! data" (`resolve_kyomi_oauth_token`, kyomi-connect) — the
    //! `UserContext` was never constructed on this path at all, unlike the
    //! three other callers of a provider-construction function
    //! (`server_fns/sql_editor.rs`, `apps/server/src/routes/query_arrow.rs`,
    //! `kyomi_auth::datasource_service`'s manual catalog refresh).
    //!
    //! The fix builds the `UserContext` via
    //! `kyomi_auth::datasource_service::resolve_discovery_connection_inputs`
    //! rather than inline in this fn — extracted so this server_fn's
    //! service-layer callout count stays under `check-server-fns.sh`'s Rule
    //! B threshold (the credential/decrypt lookups already here plus an
    //! inline `build_datasource_user_context` call tipped it to 4). The
    //! sibling test in `kyomi-auth/src/datasource_service.rs`'s `mod tests`
    //! (`resolve_discovery_connection_inputs_threads_build_datasource_user_context_through`)
    //! pins the other half of the chain: that
    //! `resolve_discovery_connection_inputs` itself calls
    //! `build_datasource_user_context` and returns the result rather than
    //! dropping it. Together the two tests pin the full chain this test
    //! alone used to pin in one file.
    //!
    //! Exercising the real path end-to-end needs a live, refreshable Google
    //! OAuth token, which this test environment doesn't have. Following the
    //! source-assertion precedent `pages/settings/datasources.rs`'s
    //! `mod tests` establishes for exactly this situation, this pins the fix
    //! at the source level instead.
    //!
    //! Not covered by this test: the live Google token refresh, and the
    //! BigQuery-provider-side branching on `auth_mode` (that lives in
    //! kyomi-connect, a separate repo/crate, and is exercised by its own
    //! `resolve_kyomi_oauth_token` unit tests).

    const SRC: &str = include_str!("datasources.rs");

    #[test]
    fn discover_datasource_resources_threads_a_built_user_context_into_create_provider() {
        let fn_start = SRC
            .find("pub async fn discover_datasource_resources(")
            .expect("discover_datasource_resources not found in datasources.rs");
        let fn_end = SRC[fn_start..]
            .find("// Test connection first")
            .map(|i| fn_start + i)
            .unwrap_or_else(|| {
                panic!("\"// Test connection first\" marker not found after discover_datasource_resources")
            });
        let connect_phase = &SRC[fn_start..fn_end];

        assert!(
            connect_phase.contains(
                "kyomi_auth::datasource_service::resolve_discovery_connection_inputs("
            ),
            "discover_datasource_resources must resolve its connection \
             config, credentials, and UserContext via \
             resolve_discovery_connection_inputs before creating the \
             provider — without this call the BigQuery kyomi_oauth path \
             has no OAuth data to construct a UserContext from"
        );
        assert!(
            connect_phase.contains("} = match kyomi_auth::datasource_service::resolve_discovery_connection_inputs("),
            "the resolved user_context (destructured from \
             DiscoveryConnectionInputs) must come from \
             resolve_discovery_connection_inputs's return value, not be \
             shadowed or reassigned afterward"
        );
        assert!(
            connect_phase.contains("&resolved_creds,\n            user_context.as_ref(),"),
            "create_provider must receive the UserContext resolved above \
             (user_context.as_ref()), not a hardcoded value"
        );
        assert!(
            !connect_phase.contains("&resolved_creds,\n            None,"),
            "regression guard: this is the exact KYO-445 bug — a literal \
             None passed as user_context makes every BigQuery kyomi_oauth \
             \"Discover Available\" and \"test connection\" call fail with \
             \"BigQuery kyomi_oauth mode requires user context with OAuth data\""
        );
    }
}

// ── JSON input codec on serde_json::Value server fns (KYO-428) ────────────

#[cfg(all(test, feature = "ssr"))]
mod json_input_codec_tests {
    //! KYO-428: `create_datasource_modal`, `update_datasource_settings`,
    //! `discover_datasource_resources`, and `save_datasource_credentials`
    //! all take a `serde_json::Value` argument built from typed
    //! `connection_config`/`credentials` maps (`build_connection_config`,
    //! `build_credentials` in `pages/settings/datasources.rs`) that include
    //! non-string JSON leaves — numbers (`port`) and booleans (`secure`,
    //! `encrypt`, `trust_server_certificate`).
    //!
    //! `serde_json::Value` is self-describing, so its `Deserialize` impl
    //! defers entirely to the format doing the decoding. Under the `#[server]`
    //! macro's default input codec (`PostUrl` — `serde_qs` over
    //! `application/x-www-form-urlencoded`), every leaf decodes as a JSON
    //! *string*, because `serde_qs` has no type information beyond "this
    //! looks like text in a query string". A driver reading `port` with
    //! `.as_u64()` then gets `None` and silently falls back to the
    //! provider's default port; a driver reading `secure`/`encrypt` with
    //! `.as_bool()` silently falls back to its default TLS posture.
    //!
    //! The fix is `#[server(prefix = "/leptos-api", input =
    //! server_fn::codec::Json)]` on all four functions (matching the
    //! existing precedent at `dashboards.rs`'s `update_dashboard`), which
    //! decodes the body as real JSON and preserves `Value::Number`/
    //! `Value::Bool` leaves as such.
    //!
    //! This test does not build a local struct or assert a decoding truth
    //! table — either would keep passing if the `input = ...` attribute
    //! were deleted, since nothing would exercise the macro-generated wire
    //! type at all. Instead it inspects the `server_fn::ServerFn::Protocol`
    //! associated type the `#[server]` macro actually generates for each of
    //! these four functions, and asserts the *input* side of that protocol
    //! is not `PostUrl` — the one property that flips if the attribute is
    //! ever removed or edited back to the default.
    //!
    //! Verified by mutation: temporarily deleting `input =
    //! server_fn::codec::Json` from `create_datasource_modal` turns its
    //! `Protocol`'s input slot back into `server_fn::codec::url::PostUrl`,
    //! and `create_datasource_modal_uses_the_json_input_codec` fails with
    //! exactly the message below. The attribute was restored immediately
    //! after and this test file was confirmed to pass again.

    use super::{
        CreateDatasourceModal, DiscoverDatasourceResources, SaveDatasourceCredentials,
        UpdateDatasourceSettings,
    };
    use leptos::server_fn::ServerFn;

    /// Extract the type name of the *first* generic argument of
    /// `server_fn::Http<Input, Output>` from a full `type_name::<Protocol>()`
    /// string — i.e. the input encoding. Splitting on the first top-level
    /// comma is sufficient here because neither `PostUrl` nor
    /// `Post<JsonEncoding>` (what `server_fn::codec::Json` expands to)
    /// contains a comma of its own.
    fn input_encoding_of(protocol_type_name: &str) -> &str {
        protocol_type_name
            .split_once("Http<")
            .and_then(|(_, rest)| rest.split_once(','))
            .map(|(input, _)| input)
            .unwrap_or_else(|| {
                panic!(
                    "expected `{protocol_type_name}` to be a server_fn::Http<Input, Output> \
                     protocol with a comma-separated generic argument list"
                )
            })
    }

    /// Assert a server_fn's `Protocol::Input` side is the JSON codec, not
    /// the default `PostUrl` form codec. `T` is one of the PascalCase
    /// structs the `#[server]` macro generates for each function below
    /// (`CreateDatasourceModal`, etc.) — asserting against the real
    /// macro-generated type, not a hand-rolled stand-in, is what makes this
    /// load-bearing against the attribute actually being removed.
    fn assert_json_input_codec<T: ServerFn>() {
        let protocol = std::any::type_name::<T::Protocol>();
        let input_encoding = input_encoding_of(protocol);
        assert!(
            !input_encoding.contains("PostUrl"),
            "expected a JSON input codec, but {protocol} still uses the \
             default form-urlencoded PostUrl codec — this is the exact \
             KYO-428 regression: serde_json::Value leaves that are numbers \
             or booleans (port, secure, encrypt, trust_server_certificate) \
             silently decode as strings under PostUrl, and the driver's \
             .as_u64()/.as_bool() then falls back to its default instead \
             of erroring"
        );
        assert!(
            input_encoding.contains("JsonEncoding"),
            "expected the input encoding to be server_fn::codec::Json \
             (JsonEncoding), got {input_encoding} in protocol {protocol}"
        );
    }

    #[test]
    fn create_datasource_modal_uses_the_json_input_codec() {
        assert_json_input_codec::<CreateDatasourceModal>();
    }

    #[test]
    fn update_datasource_settings_uses_the_json_input_codec() {
        assert_json_input_codec::<UpdateDatasourceSettings>();
    }

    #[test]
    fn discover_datasource_resources_uses_the_json_input_codec() {
        assert_json_input_codec::<DiscoverDatasourceResources>();
    }

    #[test]
    fn save_datasource_credentials_uses_the_json_input_codec() {
        assert_json_input_codec::<SaveDatasourceCredentials>();
    }
}

// ── discover_datasource_resources observability (KYO-469) ────────────────

#[cfg(all(test, feature = "ssr"))]
mod discover_datasource_resources_logging_tests {
    //! KYO-469, half 2: every terminal path in `discover_datasource_resources`
    //! must leave a trace. Before this fix, three early `?` returns (auth
    //! context extraction, unknown datasource type, missing encryption key)
    //! surfaced as bare 500s indistinguishable from any other endpoint's
    //! failure in `tower_http::trace`'s generic "response failed" log line,
    //! and two terminal `Ok(DiscoverResourcesResult { success: false, .. })`
    //! paths — `create_provider` timing out, and `test_connection()`
    //! returning `false` — returned a client-facing message but logged
    //! nothing server-side at all. That silence is exactly why the original
    //! KYO-469 investigation could not tell, from production logs alone,
    //! whether a BigQuery service-account failure even reached this
    //! endpoint.
    //!
    //! Following the source-assertion precedent `discover_user_context_tests`
    //! (above, in this same file) already established — exercising this
    //! function for real needs a live, connectable datasource this test
    //! environment doesn't have — this pins the fix at the source level:
    //! every log call the fix added or enriched is asserted present, by
    //! its exact message text, inside the function body specifically (not
    //! just anywhere in the file).

    const SRC: &str = include_str!("datasources.rs");

    /// The full source of `discover_datasource_resources`, from its
    /// signature to the start of the next top-level fn
    /// (`bigquery_projects_discovery_result`).
    fn fn_body() -> &'static str {
        let start = SRC
            .find("pub async fn discover_datasource_resources(")
            .expect("discover_datasource_resources not found in datasources.rs");
        let end = SRC[start..]
            .find("fn bigquery_projects_discovery_result(")
            .map(|i| start + i)
            .unwrap_or_else(|| {
                panic!(
                    "fn bigquery_projects_discovery_result( marker not found after \
                     discover_datasource_resources"
                )
            });
        &SRC[start..end]
    }

    /// The entry log — its absence today is indistinguishable from "the
    /// request never arrived", which was a real ambiguity in the original
    /// KYO-469 investigation. Must carry `datasource_type`, `auth_mode`,
    /// and `has_slug` so it identifies the request, not just that *a*
    /// request happened.
    #[test]
    fn entry_log_records_datasource_type_auth_mode_and_has_slug() {
        let body = fn_body();
        let start = body
            .find("tracing::info!(")
            .expect("no tracing::info!( call found in discover_datasource_resources");
        let end = body[start..]
            .find(");")
            .map(|i| start + i)
            .expect("tracing::info!( call in discover_datasource_resources has no closing );");
        let call = &body[start..end];
        assert!(
            call.contains("discover_datasource_resources: request received"),
            "expected an entry log recording the request; found: {call}"
        );
        assert!(call.contains("datasource_type = %datasource_type"));
        assert!(call.contains("auth_mode = %auth_mode"));
        assert!(call.contains("has_slug"));
    }

    /// `AuthenticatedContext::extract()` failing was a bare `?` return —
    /// a 500 with nothing tying it to this endpoint.
    #[test]
    fn authentication_failure_is_logged() {
        assert!(
            fn_body().contains("discover_datasource_resources: failed to authenticate request"),
            "AuthenticatedContext::extract() failing must log before returning — this was one \
             of the three bare `?` returns the ticket required converting to an explicit, \
             logged match"
        );
    }

    /// `DatasourceType::from_str` failing was a bare `?` return.
    #[test]
    fn unrecognized_datasource_type_is_logged() {
        assert!(
            fn_body().contains("discover_datasource_resources: unrecognized datasource type"),
            "DatasourceType::from_str failing must log before returning — this was one of the \
             three bare `?` returns the ticket required converting to an explicit, logged match"
        );
    }

    /// `ac.encryption_key()` failing was a bare `?` return.
    #[test]
    fn missing_encryption_key_is_logged() {
        assert!(
            fn_body().contains("discover_datasource_resources: failed to resolve encryption key"),
            "ac.encryption_key() failing must log before returning — this was one of the three \
             bare `?` returns the ticket required converting to an explicit, logged match"
        );
    }

    /// `DiscoveryPrepError::Decrypt` already warned before this fix — this
    /// guards its message stays present and endpoint-attributed.
    #[test]
    fn credential_decrypt_failure_is_logged() {
        assert!(fn_body().contains(
            "discover_datasource_resources: credential decrypt failed before resource discovery"
        ));
    }

    /// `DiscoveryPrepError::UserContext` already warned before this fix —
    /// this guards its message stays present and endpoint-attributed.
    #[test]
    fn user_context_build_failure_is_logged() {
        assert!(fn_body().contains(
            "discover_datasource_resources: failed to build user context for datasource discovery"
        ));
    }

    /// `create_provider` returning `Err` already warned before this fix —
    /// this guards its message stays present and endpoint-attributed.
    #[test]
    fn create_provider_error_is_logged() {
        assert!(fn_body().contains(
            "discover_datasource_resources: datasource connection error (sanitized for client)"
        ));
    }

    /// **The point of KYO-469, half 2, bolded row 1.** Before this fix,
    /// `create_provider` timing out returned "Connection timed out" to the
    /// client and logged nothing server-side.
    #[test]
    fn create_provider_timeout_is_logged() {
        assert!(
            fn_body().contains("discover_datasource_resources: create_provider timed out"),
            "create_provider timing out must log — before this fix it returned \"Connection \
             timed out\" to the client and logged nothing at all"
        );
    }

    /// **The point of KYO-469, half 2, bolded row 2.** Before this fix,
    /// `test_connection()` returning `false` returned "Connection test
    /// failed — check your credentials" to the client and logged nothing
    /// server-side.
    #[test]
    fn test_connection_failure_is_logged() {
        assert!(
            fn_body().contains("discover_datasource_resources: test_connection reported failure"),
            "test_connection() returning false must log — before this fix it returned a \
             client-facing message and logged nothing at all"
        );
    }

    /// KYO-466: a per-key discovery failure (e.g. BigQuery's
    /// `list_projects()` returning `Err`) must be logged before the reason
    /// is sanitized and handed to the client — its total absence from
    /// production logs is why the original report needed a code read
    /// instead of a log search. Unlike the other warns in this module, this
    /// one is not a terminal `?`/early-return path — discovery continues
    /// for any other key in the same request — so it is asserted
    /// separately here rather than folded into the timeout/auth-failure
    /// list above.
    #[test]
    fn per_key_discovery_failure_is_logged() {
        assert!(
            fn_body().contains(
                "discover_datasource_resources: failed to list resources for one discovery key"
            ),
            "a discovery pair whose DiscoveryResult carried an error must be logged with the \
             failing key and reason before discover_datasource_resources returns — this is the \
             log KYO-466 added so a failing list_projects() leaves a trace"
        );
    }

    /// Cross-cutting requirement: every `tracing::warn!` in this function —
    /// not just the two previously-silent terminal paths above, and not
    /// just the KYO-466 per-key warn — must carry all three attribution
    /// fields. A warn without them is exactly as unattributable to *this*
    /// endpoint as no warn at all once two datasources are failing
    /// discovery concurrently.
    ///
    /// Splits on `tracing::warn!(` rather than depending on exact
    /// indentation (several of the nine call sites sit one nesting level
    /// shallower than the rest), so this stays robust to reformatting.
    #[test]
    fn every_warn_carries_datasource_type_auth_mode_and_has_slug() {
        let body = fn_body();
        let mut warn_count = 0;
        for chunk in body.split("tracing::warn!(").skip(1) {
            warn_count += 1;
            let end = chunk.find(");").unwrap_or_else(|| {
                panic!(
                    "tracing::warn!( call #{warn_count} in discover_datasource_resources has \
                     no closing );"
                )
            });
            let call = &chunk[..end];
            assert!(
                call.contains("datasource_type = %datasource_type")
                    && call.contains("auth_mode = %auth_mode")
                    && call.contains("has_slug"),
                "tracing::warn!( call #{warn_count} in discover_datasource_resources is \
                 missing one of datasource_type / auth_mode / has_slug — every failure path \
                 must be attributable to this endpoint:\n{call}"
            );
        }
        assert_eq!(
            warn_count, 9,
            "expected exactly 9 tracing::warn!( calls in discover_datasource_resources (the \
             eight terminal failure paths from KYO-469, plus the KYO-466 per-key discovery \
             failure warn) — found {warn_count}. If a new failure path was added, give it the \
             same three attribution fields and update this count."
        );
    }
}
