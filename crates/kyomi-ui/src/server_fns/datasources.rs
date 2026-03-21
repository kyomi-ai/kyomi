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

// ─── Modal Server Functions ─────────────────────────────────────────────────

/// Result of creating or saving a datasource.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatasourceResult {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub datasource_type: String,
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

/// Datasource settings loaded for edit modal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatasourceSettingsResult {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub datasource_type: String,
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
    .map_err(|e| ServerFnError::new(e.to_string()))?;

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
        .map_err(|e| ServerFnError::new(e.to_string()))?;
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
    .map_err(|e| ServerFnError::new(e.to_string()))?;

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
    .map_err(|e| ServerFnError::new(e.to_string()))?;

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

    let ds = kyomi_auth::datasource_service::get_datasource(&ctx.db, &datasource_id, ws_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("Datasource not found"))?;

    // Non-admins can only view active datasources
    if !is_admin && !ds.active {
        return Err(ServerFnError::new("Datasource not found"));
    }

    let encryption_key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    let user_cred =
        kyomi_auth::datasource_service::get_user_credential(&ctx.db, &auth.user_id, &ds.id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    let user_settings = match &user_cred {
        Some(cred) => kyomi_auth::credential_service::decrypt_credentials(
            &cred.credentials,
            encryption_key,
        )
        .unwrap_or(serde_json::json!({})),
        None => serde_json::json!({}),
    };

    let connection_config = &ds.connection_config;
    let cred_result = kyomi_auth::datasource_auth_service::check_credential_status(
        ds.datasource_type.as_ref(),
        connection_config,
        user_cred.as_ref(),
        encryption_key,
    );

    let shared_credentials = connection_config
        .get("shared_credentials")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let has_username = user_settings
        .get("username")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    let has_password = user_settings
        .get("password")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    let auth_mode = connection_config
        .get("auth_mode")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let service_account_email = if auth_mode.as_deref() == Some("service_account") {
        connection_config
            .get("service_account_json")
            .and_then(|v| v.as_str())
            .and_then(|json_str| serde_json::from_str::<serde_json::Value>(json_str).ok())
            .and_then(|v| v.get("client_email").and_then(|e| e.as_str()).map(|s| s.to_string()))
    } else {
        None
    };

    // BigQuery OAuth status
    let (has_oauth, oauth_email, has_bigquery_scopes, needs_bigquery_connect) =
        if ds.datasource_type.as_ref() == "bigquery" {
            match auth_mode.as_deref() {
                Some("service_account") => (true, None, true, false),
                Some("enterprise_oauth") => {
                    let has_o = user_settings
                        .get("auth_type")
                        .and_then(|v| v.as_str())
                        == Some("oauth");
                    let o_email = if has_o {
                        user_settings
                            .get("oauth_email")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    } else {
                        None
                    };
                    (has_o, o_email, has_o, !has_o)
                }
                _ => {
                    // kyomi_oauth — use global OAuth status from cred_result
                    let has_o = cred_result.credential_status == "valid"
                        || cred_result.credential_status == "shared";
                    (has_o, None, has_o, !has_o)
                }
            }
        } else {
            (false, None, false, false)
        };

    // Mask connection config (don't return secrets)
    let masked_config = kyomi_auth::credential_service::mask_connection_config(
        connection_config,
        ds.datasource_type.as_ref(),
    );

    Ok(DatasourceSettingsResult {
        id: ds.id,
        name: ds.name,
        slug: ds.slug,
        datasource_type: ds.datasource_type.to_string(),
        connection_config: masked_config,
        user_settings,
        has_oauth,
        oauth_email,
        has_bigquery_scopes,
        needs_bigquery_connect,
        auth_mode,
        service_account_email,
        shared_credentials,
        credential_status: cred_result.credential_status,
        has_username,
        has_password,
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
        .map_err(|e| ServerFnError::new(e.to_string()))?;

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
        .map_err(|e| ServerFnError::new(e.to_string()))?
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
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    let credentials = if let Some(ref cred) = user_cred {
        kyomi_auth::credential_service::decrypt_credentials(&cred.credentials, encryption_key)
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
        .map_err(|e| ServerFnError::new(e.to_string()))?;

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
                        kyomi_auth::credential_service::decrypt_credentials(
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
