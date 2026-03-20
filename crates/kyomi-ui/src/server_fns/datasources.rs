// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for datasource management.
//!
//! These replace the REST API calls for datasource settings:
//! - `GET /datasources` + `GET /datasources/credential-status` → `list_datasources()`
//! - `GET /datasources/types` → `get_datasource_types()`
//! - `POST /datasources/{id}/toggle` → `toggle_datasource()`
//! - `DELETE /datasources/{id}` → `delete_datasource()`
//!
//! Calls the same service-layer code as `apps/server/src/routes/datasources.rs`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, workspace_id};

// ─── Types ──────────────────────────────────────────────────────────────────

/// A datasource with its credential status, returned by the list server function.
///
/// Combines data from the datasource config and the credential status endpoint
/// into a single struct so the UI can render everything in one pass.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatasourceInfo {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub datasource_type: String,
    pub type_display_name: String,
    pub active: bool,
    pub connection_type: String,
    /// User's credential status: "valid", "shared", "missing", "expired"
    pub credential_status: String,
    /// Auth method: "oauth", "password", "connect"
    pub auth_method: String,
    /// Whether the user has this datasource enabled
    pub user_enabled: bool,
    /// Whether the user can enable this datasource
    pub can_enable: bool,
    /// Whether this is a sample datasource
    pub is_sample: bool,
    /// Whether the catalog needs attention (no tables, no index, or stale)
    pub needs_catalog_attention: bool,
}

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

    // Fetch active datasources
    let datasources =
        kyomi_auth::datasource_service::list_datasources(&ctx.db, ws_id, false)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    // Fetch all user credentials in one query
    let user_credentials = kyomi_core::db_fetch_all!(
        &ctx.db,
        kyomi_core::models::datasource::UserDatasourceCredential,
        "SELECT id, user_id, datasource_config_id, workspace_id, credentials, \
         enabled, created_at, updated_at \
         FROM user_datasource_credentials \
         WHERE user_id = $1 AND workspace_id = $2",
        &auth.user_id,
        ws_id
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let creds_by_ds: std::collections::HashMap<&str, &kyomi_core::models::datasource::UserDatasourceCredential> =
        user_credentials
            .iter()
            .map(|c| (c.datasource_config_id.as_str(), c))
            .collect();

    // Fetch user preferences for shared-auth datasources
    let user_preferences = kyomi_core::db_fetch_all!(
        &ctx.db,
        kyomi_core::models::datasource::UserDatasourcePreference,
        "SELECT id, user_id, datasource_config_id, enabled, \
         created_at, updated_at \
         FROM user_datasource_preferences \
         WHERE user_id = $1",
        &auth.user_id
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let prefs_by_ds: std::collections::HashMap<&str, &kyomi_core::models::datasource::UserDatasourcePreference> =
        user_preferences
            .iter()
            .map(|p| (p.datasource_config_id.as_str(), p))
            .collect();

    // Fetch catalog status for each datasource
    let catalog_statuses = fetch_catalog_statuses(&ctx.db, &datasources).await;

    // Get encryption key for credential status checks
    let encryption_key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    let mut result = Vec::with_capacity(datasources.len());

    for ds in &datasources {
        let connection_config = &ds.connection_config;
        let is_connect = ds.connection_type == "connect";

        // Compute credential status (same logic as REST handler)
        let (cred_result, user_enabled, can_enable) = if is_connect {
            let pref = prefs_by_ds.get(ds.id.as_str()).copied();
            let enabled = pref.is_none_or(|p| p.enabled);
            let status = kyomi_auth::datasource_auth_service::CredentialStatusResult {
                credential_status: "shared".to_string(),
                auth_method: "connect".to_string(),
                oauth_provider: None,
            };
            (status, enabled, true)
        } else {
            let user_cred = creds_by_ds.get(ds.id.as_str()).copied();
            let result = kyomi_auth::datasource_auth_service::check_credential_status(
                ds.datasource_type.as_ref(),
                connection_config,
                user_cred,
                encryption_key,
            );

            let user_enabled = kyomi_auth::datasource_auth_service::get_user_enabled(
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

        // Look up display name from registry
        let type_display_name = kyomi_core::datasource_registry::get_metadata_by_str(
            ds.datasource_type.as_ref(),
        )
        .map(|m| m.display_name.to_string())
        .unwrap_or_else(|| ds.datasource_type.to_string());

        let is_sample = ds
            .connection_config
            .get("is_sample")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let needs_catalog_attention =
            catalog_needs_attention(&catalog_statuses, &ds.id);

        result.push(DatasourceInfo {
            id: ds.id.clone(),
            name: ds.name.clone(),
            slug: ds.slug.clone(),
            datasource_type: ds.datasource_type.to_string(),
            type_display_name,
            active: ds.active,
            connection_type: ds.connection_type.clone(),
            credential_status: cred_result.credential_status,
            auth_method: cred_result.auth_method,
            user_enabled,
            can_enable,
            is_sample,
            needs_catalog_attention,
        });
    }

    Ok(result)
}

/// Fetch catalog status for all datasources (table count + last indexed).
#[cfg(feature = "ssr")]
async fn fetch_catalog_statuses(
    db: &kyomi_core::DbPool,
    datasources: &[kyomi_core::models::datasource::DatasourceConfig],
) -> std::collections::HashMap<String, (i64, Option<chrono::DateTime<chrono::Utc>>)> {
    let mut result = std::collections::HashMap::new();

    for ds in datasources {
        let table_count: i64 = kyomi_core::db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM datasource_tables WHERE datasource_config_id = $1",
            &ds.id
        )
        .unwrap_or(0);

        result.insert(ds.id.clone(), (table_count, ds.last_catalog_refresh));
    }

    result
}

/// Check if a datasource's catalog needs attention.
///
/// Matches the React `needsAttention` function:
/// - No tables indexed
/// - No last_indexed timestamp
/// - Last indexed > 7 days ago
#[cfg(feature = "ssr")]
fn catalog_needs_attention(
    catalog_statuses: &std::collections::HashMap<String, (i64, Option<chrono::DateTime<chrono::Utc>>)>,
    ds_id: &str,
) -> bool {
    let Some((table_count, last_indexed)) = catalog_statuses.get(ds_id) else {
        return false;
    };
    if *table_count == 0 {
        return true;
    }
    let Some(last_indexed) = last_indexed else {
        return true;
    };
    let days_since = (chrono::Utc::now() - *last_indexed).num_days();
    days_since > 7
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

    let ds = kyomi_auth::datasource_service::get_datasource(&ctx.db, &datasource_id, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Datasource not found"))?;

    if !ds.active {
        return Err(ServerFnError::new("Datasource is not active"));
    }

    let connection_config = &ds.connection_config;
    let ds_type_str = ds.datasource_type.as_ref();
    let is_shared =
        kyomi_auth::datasource_auth_service::is_shared_auth(ds_type_str, connection_config);
    let is_connect = ds.connection_type == "connect";

    let user_cred =
        kyomi_auth::datasource_service::get_user_credential(&ctx.db, &auth.user_id, &ds.id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    if enabled {
        if is_shared || is_connect {
            // Shared auth or Connect — always allow enabling via preference
            kyomi_auth::datasource_service::upsert_user_preference(
                &ctx.db,
                &auth.user_id,
                &ds.id,
                true,
            )
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        } else {
            // Personal auth — check credential status before enabling
            let encryption_key = ctx
                .encryption_key
                .as_deref()
                .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

            let result = kyomi_auth::datasource_auth_service::check_credential_status(
                ds_type_str,
                connection_config,
                user_cred.as_ref(),
                encryption_key,
            );

            if result.credential_status != "valid" && result.credential_status != "shared" {
                return Err(ServerFnError::new(
                    "Connect your credentials first to enable this datasource",
                ));
            }

            // Update credential enabled flag
            if let Some(cred) = &user_cred {
                let sql = format!(
                    "UPDATE user_datasource_credentials \
                     SET enabled = true, updated_at = {} \
                     WHERE id = $1",
                    kyomi_core::sql_compat::now(ctx.db.is_postgres())
                );
                kyomi_core::db_execute!(&ctx.db, &sql, &cred.id)
                    .map_err(|e| ServerFnError::new(e.to_string()))?;
            }
        }
    } else {
        // Disabling — always allowed
        if is_shared || is_connect {
            kyomi_auth::datasource_service::upsert_user_preference(
                &ctx.db,
                &auth.user_id,
                &ds.id,
                false,
            )
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        } else if let Some(cred) = &user_cred {
            let sql = format!(
                "UPDATE user_datasource_credentials \
                 SET enabled = false, updated_at = {} \
                 WHERE id = $1",
                kyomi_core::sql_compat::now(ctx.db.is_postgres())
            );
            kyomi_core::db_execute!(&ctx.db, &sql, &cred.id)
                .map_err(|e| ServerFnError::new(e.to_string()))?;
        }
    }

    Ok(())
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
        .map_err(|e| ServerFnError::new(e.to_string()))?;

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
