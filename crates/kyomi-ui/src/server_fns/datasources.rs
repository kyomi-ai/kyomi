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
#[server(prefix = "/leptos-api")]
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

    let encryption_key = ac.encryption_key()?;

    // If slug provided, look up any stored per-user credential blob (e.g.
    // OAuth) to overlay caller-provided `credentials` on top of.
    let stored_cred_str: Option<String> = if let Some(ref slug) = datasource_slug {
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
                    Ok(Some(cred)) => Some(cred.credentials),
                    _ => None,
                }
            }
            _ => None,
        }
    } else {
        None
    };

    // `connection_config` may be freshly-typed plaintext (create mode) or an
    // already-persisted config fetched by the caller (edit mode) — decrypt
    // defensively; non-ciphertext values pass through unchanged. The stored
    // per-user credential blob (if any) needs the same treatment before being
    // overlaid with caller-provided `credentials` (missing/undecryptable
    // stored credentials yield an empty object, so the overlay falls back to
    // whatever the caller provided).
    let (connection_config, stored_creds) = match kyomi_auth::credential_service::decrypt_provider_secrets(
        &connection_config,
        stored_cred_str.as_deref(),
        &encryption_key,
    ) {
        Ok(pair) => pair,
        Err(e) => {
            tracing::warn!(error = %e, "credential decrypt failed before resource discovery");
            return Ok(DiscoverResourcesResult {
                success: false,
                resources: std::collections::HashMap::new(),
                message: e.to_string(),
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
    let refresh_failure_reason = if refresh_failed {
        refresh_row
            .and_then(|row| row.catalog_refresh_progress)
            .and_then(|progress| progress.get("error").and_then(|v| v.as_str()).map(str::to_string))
    } else {
        None
    };

    Ok(CatalogStatsResult {
        table_count,
        schema_count,
        last_indexed,
        refresh_failed,
        refresh_failure_reason,
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

