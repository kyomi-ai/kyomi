// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource service — CRUD, resolution, credential management, and preferences.
//!
//! Follows the same function-based pattern as `workspace_service.rs` and
//! `user_service.rs`: stateless `pub async fn` functions that take `&DbPool`.
//!
//! Resolution order:
//! 1. slug: `"production-postgres"` -> lookup by slug
//! 2. id:   `"ds-..."` -> direct UUID lookup
//!
//! Credential merge preserves OAuth fields on update to prevent the frontend
//! from overwriting tokens set by OAuth callbacks.

use chrono::{DateTime, Utc};
use kyomi_core::models::datasource::{
    DatasourceConfig, UserDatasourceCredential, UserDatasourcePreference,
};
use kyomi_core::sql_compat;
use kyomi_core::DbPool;
use serde_json::Value;

use crate::credential_service;

// ---------------------------------------------------------------------------
// ID / Slug generation
// ---------------------------------------------------------------------------

/// Generate a unique datasource ID.
///
/// Format: `"ds-{uuid_hex}"` matching Python's `generate_datasource_id()`.
/// Uses the hex representation of a v4 UUID (32 chars, no hyphens).
pub fn generate_datasource_id() -> String {
    format!("ds-{}", uuid::Uuid::new_v4().simple())
}

/// Generate a URL-safe slug from a display name.
///
/// Rules:
/// - Lowercase
/// - Replace spaces and underscores with hyphens
/// - Remove non-alphanumeric except hyphens
/// - Collapse multiple consecutive hyphens
/// - Strip leading/trailing hyphens
/// - Minimum length 3 (pad with `-db` if needed)
/// - Maximum length 100
///
/// Matches Python's `utils/slug.py::generate_slug()`.
pub fn generate_slug(name: &str) -> String {
    // Lowercase
    let mut slug = name.to_lowercase();

    // Replace spaces and underscores with hyphens
    slug = slug
        .chars()
        .map(|c| if c.is_whitespace() || c == '_' { '-' } else { c })
        .collect();

    // Remove non-alphanumeric except hyphens
    slug.retain(|c| c.is_ascii_alphanumeric() || c == '-');

    // Collapse multiple hyphens
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }

    // Remove leading/trailing hyphens
    slug = slug.trim_matches('-').to_string();

    // Ensure minimum length
    if slug.len() < 3 {
        slug = if slug.is_empty() {
            "datasource".to_string()
        } else {
            format!("{slug}-db")
        };
    }

    // Truncate to max length
    if slug.len() > 100 {
        slug.truncate(100);
    }

    slug
}

// ---------------------------------------------------------------------------
// Datasource CRUD
// ---------------------------------------------------------------------------

/// List all datasources for a workspace.
///
/// When `include_inactive` is false, only active datasources are returned.
/// Results are ordered by name.
pub async fn list_datasources(
    pool: &DbPool,
    workspace_id: &str,
    include_inactive: bool,
) -> kyomi_core::Result<Vec<DatasourceConfig>> {
    let is_pg = pool.is_postgres();
    let datasources = if include_inactive {
        kyomi_core::db_fetch_all!(
            pool,
            DatasourceConfig,
            "SELECT id, workspace_id, name, slug,
                      datasource_type, connection_config,
                      active, connection_type, connect_token_jti,
                      created_at, updated_at,
                      last_catalog_refresh, last_index_started_at, auto_refresh_allowed
             FROM datasource_configs
             WHERE workspace_id = $1
             ORDER BY name",
            workspace_id
        )?
    } else {
        let sql = format!(
            "SELECT id, workspace_id, name, slug,
                      datasource_type, connection_config,
                      active, connection_type, connect_token_jti,
                      created_at, updated_at,
                      last_catalog_refresh, last_index_started_at, auto_refresh_allowed
             FROM datasource_configs
             WHERE workspace_id = $1 AND active = {}
             ORDER BY name",
            sql_compat::bool_true(is_pg)
        );
        kyomi_core::db_fetch_all!(pool, DatasourceConfig, &sql, workspace_id)?
    };

    Ok(datasources)
}

/// Get a datasource by ID within a workspace.
pub async fn get_datasource(
    pool: &DbPool,
    id: &str,
    workspace_id: &str,
) -> kyomi_core::Result<Option<DatasourceConfig>> {
    let ds = kyomi_core::db_fetch_optional!(
        pool,
        DatasourceConfig,
        "SELECT id, workspace_id, name, slug,
                  datasource_type, connection_config,
                  active, connection_type, connect_token_jti,
                  created_at, updated_at,
                  last_catalog_refresh, last_index_started_at, auto_refresh_allowed
         FROM datasource_configs
         WHERE id = $1 AND workspace_id = $2",
        id,
        workspace_id
    )?;

    Ok(ds)
}

/// Get a datasource by slug within a workspace.
pub async fn get_datasource_by_slug(
    pool: &DbPool,
    slug: &str,
    workspace_id: &str,
) -> kyomi_core::Result<Option<DatasourceConfig>> {
    let ds = kyomi_core::db_fetch_optional!(
        pool,
        DatasourceConfig,
        "SELECT id, workspace_id, name, slug,
                  datasource_type, connection_config,
                  active, connection_type, connect_token_jti,
                  created_at, updated_at,
                  last_catalog_refresh, last_index_started_at, auto_refresh_allowed
         FROM datasource_configs
         WHERE slug = $1 AND workspace_id = $2",
        slug,
        workspace_id
    )?;

    Ok(ds)
}

/// Resolve a datasource by identifier (slug or UUID).
///
/// Resolution order:
/// 1. Try slug match first (most common case)
/// 2. If identifier starts with `"ds-"`, try UUID match
/// 3. If not found, return error with available slugs
///
/// When `include_inactive` is false, inactive datasources are excluded.
pub async fn resolve_datasource(
    pool: &DbPool,
    identifier: &str,
    workspace_id: &str,
    include_inactive: bool,
) -> kyomi_core::Result<DatasourceConfig> {
    let is_pg = pool.is_postgres();

    // Try slug first
    let ds = if include_inactive {
        kyomi_core::db_fetch_optional!(
            pool,
            DatasourceConfig,
            "SELECT id, workspace_id, name, slug,
                      datasource_type, connection_config,
                      active, connection_type, connect_token_jti,
                      created_at, updated_at,
                      last_catalog_refresh, last_index_started_at, auto_refresh_allowed
             FROM datasource_configs
             WHERE slug = $1 AND workspace_id = $2",
            identifier,
            workspace_id
        )?
    } else {
        let sql = format!(
            "SELECT id, workspace_id, name, slug,
                      datasource_type, connection_config,
                      active, connection_type, connect_token_jti,
                      created_at, updated_at,
                      last_catalog_refresh, last_index_started_at, auto_refresh_allowed
             FROM datasource_configs
             WHERE slug = $1 AND workspace_id = $2 AND active = {}",
            sql_compat::bool_true(is_pg)
        );
        kyomi_core::db_fetch_optional!(pool, DatasourceConfig, &sql, identifier, workspace_id)?
    };

    if let Some(ds) = ds {
        return Ok(ds);
    }

    // Try UUID if identifier looks like one (starts with "ds-")
    if identifier.starts_with("ds-") {
        let ds = if include_inactive {
            kyomi_core::db_fetch_optional!(
                pool,
                DatasourceConfig,
                "SELECT id, workspace_id, name, slug,
                          datasource_type, connection_config,
                          active, connection_type, connect_token_jti,
                          created_at, updated_at,
                          last_catalog_refresh, last_index_started_at, auto_refresh_allowed
                 FROM datasource_configs
                 WHERE id = $1 AND workspace_id = $2",
                identifier,
                workspace_id
            )?
        } else {
            let sql = format!(
                "SELECT id, workspace_id, name, slug,
                          datasource_type, connection_config,
                          active, connection_type, connect_token_jti,
                          created_at, updated_at,
                          last_catalog_refresh, last_index_started_at, auto_refresh_allowed
                 FROM datasource_configs
                 WHERE id = $1 AND workspace_id = $2 AND active = {}",
                sql_compat::bool_true(is_pg)
            );
            kyomi_core::db_fetch_optional!(
                pool,
                DatasourceConfig,
                &sql,
                identifier,
                workspace_id
            )?
        };

        if let Some(ds) = ds {
            return Ok(ds);
        }
    }

    // Not found — build error with available slugs
    let slugs = list_datasource_slugs(pool, workspace_id).await?;
    let available = if slugs.is_empty() {
        String::new()
    } else {
        format!(" Available: {}", slugs.join(", "))
    };

    Err(kyomi_core::Error::NotFound(format!(
        "Datasource '{identifier}' not found in this workspace.{available}"
    )))
}

/// Resolve a datasource by provider type.
///
/// Auto-resolves when exactly one datasource of the given type exists.
/// Returns an error if zero or multiple matches.
pub async fn resolve_by_provider(
    pool: &DbPool,
    provider_type: &str,
    workspace_id: &str,
) -> kyomi_core::Result<DatasourceConfig> {
    let is_pg = pool.is_postgres();
    let sql = format!(
        "SELECT id, workspace_id, name, slug,
                  datasource_type, connection_config,
                  active, connection_type, connect_token_jti,
                  created_at, updated_at,
                  last_catalog_refresh, last_index_started_at, auto_refresh_allowed
         FROM datasource_configs
         WHERE workspace_id = $1 AND datasource_type = $2 AND active = {}",
        sql_compat::bool_true(is_pg)
    );
    let datasources = kyomi_core::db_fetch_all!(
        pool,
        DatasourceConfig,
        &sql,
        workspace_id,
        provider_type
    )?;

    match datasources.len() {
        0 => Err(kyomi_core::Error::NotFound(format!(
            "No {provider_type} datasource configured in this workspace."
        ))),
        1 => datasources.into_iter().next().ok_or_else(|| {
            kyomi_core::Error::Internal("Vec with len 1 yielded no elements".into())
        }),
        _ => {
            let slugs: Vec<&str> = datasources.iter().map(|ds| ds.slug.as_str()).collect();
            Err(kyomi_core::Error::BadRequest(format!(
                "Multiple {provider_type} datasources configured: {}. \
                 Please specify datasource_id explicitly to disambiguate.",
                slugs.join(", ")
            )))
        }
    }
}

/// Create a new datasource configuration.
///
/// Checks for duplicate name and slug within the workspace before inserting.
/// Generates a datasource ID automatically.
///
/// `connection_type` defaults to `"direct"` when `None`.
pub async fn create_datasource(
    pool: &DbPool,
    workspace_id: &str,
    name: &str,
    slug: Option<&str>,
    ds_type: &str,
    connection_config: Value,
    connection_type: Option<&str>,
) -> kyomi_core::Result<DatasourceConfig> {
    // Check duplicate name
    let existing_name: i64 = kyomi_core::db_fetch_scalar!(
        pool,
        i64,
        "SELECT COUNT(*) FROM datasource_configs \
         WHERE workspace_id = $1 AND name = $2",
        workspace_id,
        name
    )?;

    if existing_name > 0 {
        return Err(kyomi_core::Error::Conflict(format!(
            "A datasource named '{name}' already exists in this workspace"
        )));
    }

    // Generate or use provided slug
    let slug_value = match slug {
        Some(s) => s.to_string(),
        None => generate_slug(name),
    };

    // Check duplicate slug
    let existing_slug: i64 = kyomi_core::db_fetch_scalar!(
        pool,
        i64,
        "SELECT COUNT(*) FROM datasource_configs \
         WHERE workspace_id = $1 AND slug = $2",
        workspace_id,
        &slug_value
    )?;

    if existing_slug > 0 {
        return Err(kyomi_core::Error::Conflict(format!(
            "A datasource with slug '{slug_value}' already exists in this workspace"
        )));
    }

    let id = generate_datasource_id();
    let conn_type = connection_type.unwrap_or("direct");
    let is_pg = pool.is_postgres();
    let sql = format!(
        "INSERT INTO datasource_configs \
         (id, workspace_id, name, slug, datasource_type, connection_config, active, connection_type, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, {}, $7, {now}, {now})",
        sql_compat::bool_true(is_pg),
        now = sql_compat::now(is_pg),
    );
    kyomi_core::db_execute!(
        pool,
        &sql,
        &id,
        workspace_id,
        name,
        &slug_value,
        ds_type,
        &connection_config,
        conn_type
    )?;

    // Fetch and return the created record
    get_datasource(pool, &id, workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("Datasource created but not found".into()))
}

/// Partial update of a datasource configuration.
///
/// Only provided fields are updated. Checks for duplicate name/slug on rename.
/// Returns the updated `DatasourceConfig`.
#[allow(clippy::too_many_arguments)]
pub async fn update_datasource(
    pool: &DbPool,
    id: &str,
    workspace_id: &str,
    name: Option<&str>,
    slug: Option<&str>,
    connection_config: Option<Value>,
    active: Option<bool>,
    auto_refresh_allowed: Option<bool>,
) -> kyomi_core::Result<DatasourceConfig> {
    // Verify the datasource exists
    let existing = get_datasource(pool, id, workspace_id).await?;
    let Some(existing) = existing else {
        return Err(kyomi_core::Error::NotFound(format!(
            "Datasource '{id}' not found"
        )));
    };

    // Check duplicate name if changing
    if let Some(new_name) = name
        && new_name != existing.name
    {
        let dup_name: i64 = kyomi_core::db_fetch_scalar!(
            pool,
            i64,
            "SELECT COUNT(*) FROM datasource_configs \
             WHERE workspace_id = $1 AND name = $2 AND id != $3",
            workspace_id,
            new_name,
            id
        )?;

        if dup_name > 0 {
            return Err(kyomi_core::Error::Conflict(format!(
                "A datasource named '{new_name}' already exists in this workspace"
            )));
        }
    }

    // Check duplicate slug if changing
    if let Some(new_slug) = slug
        && new_slug != existing.slug
    {
        let dup_slug: i64 = kyomi_core::db_fetch_scalar!(
            pool,
            i64,
            "SELECT COUNT(*) FROM datasource_configs \
             WHERE workspace_id = $1 AND slug = $2 AND id != $3",
            workspace_id,
            new_slug,
            id
        )?;

        if dup_slug > 0 {
            return Err(kyomi_core::Error::Conflict(format!(
                "A datasource with slug '{new_slug}' already exists in this workspace"
            )));
        }
    }

    // Build and execute the update
    let final_name = name.unwrap_or(&existing.name);
    let final_slug = slug.unwrap_or(&existing.slug);
    let final_config = connection_config.as_ref().unwrap_or(&existing.connection_config);
    let final_active = active.unwrap_or(existing.active);
    let final_auto_refresh = auto_refresh_allowed.unwrap_or(existing.auto_refresh_allowed);

    let is_pg = pool.is_postgres();

    let sql = format!(
        "UPDATE datasource_configs SET \
         name = $1, slug = $2, connection_config = $3, \
         active = $4, auto_refresh_allowed = $5, updated_at = {} \
         WHERE id = $6 AND workspace_id = $7",
        sql_compat::now(is_pg)
    );
    kyomi_core::db_execute!(
        pool,
        &sql,
        final_name,
        final_slug,
        final_config,
        final_active,
        final_auto_refresh,
        id,
        workspace_id
    )?;

    // Return updated record
    get_datasource(pool, id, workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("Datasource updated but not found".into()))
}

/// Delete a datasource. CASCADE in the database handles related rows.
pub async fn delete_datasource(
    pool: &DbPool,
    id: &str,
    workspace_id: &str,
) -> kyomi_core::Result<()> {
    let result = kyomi_core::db_execute!(
        pool,
        "DELETE FROM datasource_configs WHERE id = $1 AND workspace_id = $2",
        id,
        workspace_id
    )?;

    if result.rows_affected() == 0 {
        return Err(kyomi_core::Error::NotFound(format!(
            "Datasource '{id}' not found"
        )));
    }

    Ok(())
}

/// List all datasource slugs for a workspace (for error messages).
pub async fn list_datasource_slugs(
    pool: &DbPool,
    workspace_id: &str,
) -> kyomi_core::Result<Vec<String>> {
    #[derive(Debug, sqlx::FromRow)]
    struct SlugRow {
        slug: String,
    }

    let is_pg = pool.is_postgres();
    let sql = format!(
        "SELECT slug FROM datasource_configs \
         WHERE workspace_id = $1 AND active = {} \
         ORDER BY slug",
        sql_compat::bool_true(is_pg)
    );
    let rows = kyomi_core::db_fetch_all!(pool, SlugRow, &sql, workspace_id)?;

    Ok(rows.into_iter().map(|r| r.slug).collect())
}

// ---------------------------------------------------------------------------
// Connect token JTI management
// ---------------------------------------------------------------------------

/// Update the Connect token JTI for a datasource.
///
/// Used when generating or rotating a Connect token — the stored JTI is compared
/// against incoming tokens for revocation enforcement.
pub async fn update_connect_jti(
    pool: &DbPool,
    datasource_config_id: &str,
    jti: &str,
) -> kyomi_core::Result<()> {
    let is_pg = pool.is_postgres();
    let sql = format!(
        "UPDATE datasource_configs \
         SET connect_token_jti = $1, updated_at = {} \
         WHERE id = $2 AND connection_type = 'connect'",
        sql_compat::now(is_pg)
    );
    let result = kyomi_core::db_execute!(pool, &sql, jti, datasource_config_id)?;

    if result.rows_affected() == 0 {
        return Err(kyomi_core::Error::BadRequest(
            "Datasource is not a Connect type or does not exist".into(),
        ));
    }

    Ok(())
}

/// Clear the Connect token JTI for a datasource, effectively revoking any active token.
///
/// Used when disconnecting a Connect datasource — any subsequent token verification
/// will fail because there is no matching JTI.
pub async fn clear_connect_jti(
    pool: &DbPool,
    datasource_config_id: &str,
) -> kyomi_core::Result<()> {
    let is_pg = pool.is_postgres();
    let none: Option<&str> = None;
    let sql = format!(
        "UPDATE datasource_configs \
         SET connect_token_jti = $1, updated_at = {} \
         WHERE id = $2 AND connection_type = 'connect'",
        sql_compat::now(is_pg)
    );
    let result = kyomi_core::db_execute!(pool, &sql, &none, datasource_config_id)?;

    if result.rows_affected() == 0 {
        return Err(kyomi_core::Error::BadRequest(
            "Datasource is not a Connect type or does not exist".into(),
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Credential CRUD
// ---------------------------------------------------------------------------

/// Get a user's credential record for a datasource.
pub async fn get_user_credential(
    pool: &DbPool,
    user_id: &str,
    datasource_config_id: &str,
) -> kyomi_core::Result<Option<UserDatasourceCredential>> {
    let cred = kyomi_core::db_fetch_optional!(
        pool,
        UserDatasourceCredential,
        "SELECT id, user_id, datasource_config_id, workspace_id, credentials,
                  enabled, created_at, updated_at
         FROM user_datasource_credentials
         WHERE user_id = $1 AND datasource_config_id = $2",
        user_id,
        datasource_config_id
    )?;

    Ok(cred)
}

/// OAuth fields that must be preserved during credential merge.
///
/// These fields are set exclusively by OAuth callbacks (Google, Snowflake,
/// Databricks, Microsoft) and must not be overwritten when users save other
/// credential fields (e.g., billing_project). If users could overwrite these,
/// they would bypass the OAuth flow by injecting arbitrary tokens.
const OAUTH_FIELDS: &[&str] = &[
    "auth_type",
    "oauth_access_token",
    "oauth_refresh_token",
    "oauth_token_expiry",
    "oauth_scope",
    "oauth_username",
    "oauth_email",
];

/// Merge new credentials with existing ones, preserving OAuth fields.
///
/// The merge logic:
/// - Start with existing credentials (preserves all OAuth fields)
/// - Overlay all keys from `new_credentials` that are NOT OAuth fields
/// - OAuth fields can only be set by the OAuth callback, not by user input
///
/// If `existing_encrypted` is `None`, returns `new_credentials` as-is.
pub fn merge_credentials(
    existing_encrypted: Option<&str>,
    new_credentials: &Value,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<Value> {
    let Some(encrypted) = existing_encrypted else {
        return Ok(new_credentials.clone());
    };

    // Decrypt existing credentials
    let existing = credential_service::decrypt_credentials(encrypted, encryption_key)?;

    let Some(existing_obj) = existing.as_object() else {
        // If existing is not an object, just use new
        return Ok(new_credentials.clone());
    };

    let Some(new_obj) = new_credentials.as_object() else {
        // If new is not an object, just use it
        return Ok(new_credentials.clone());
    };

    // Start with existing (preserves OAuth fields)
    let mut merged = existing_obj.clone();

    // Overlay non-OAuth fields from new credentials
    for (key, value) in new_obj {
        if !OAUTH_FIELDS.contains(&key.as_str()) {
            merged.insert(key.clone(), value.clone());
        }
    }

    Ok(Value::Object(merged))
}

/// Save (upsert) user credentials for a datasource.
///
/// If credentials already exist, merges with existing to preserve OAuth fields,
/// then encrypts and updates. If new, encrypts and inserts.
///
/// Returns the saved credential record.
pub async fn save_user_credential(
    pool: &DbPool,
    encryption_key: &[u8; 32],
    user_id: &str,
    datasource_config_id: &str,
    workspace_id: &str,
    credentials: &Value,
) -> kyomi_core::Result<UserDatasourceCredential> {
    // Check for existing credential to merge OAuth fields
    let existing = get_user_credential(pool, user_id, datasource_config_id).await?;

    let merged = merge_credentials(
        existing.as_ref().map(|c| c.credentials.as_str()),
        credentials,
        encryption_key,
    )?;

    // Encrypt the merged credentials
    let encrypted = credential_service::encrypt_credentials(&merged, encryption_key)?;

    let is_pg = pool.is_postgres();

    // Upsert: insert or update on conflict
    if is_pg {
        let sql = format!(
            "INSERT INTO user_datasource_credentials \
             (user_id, datasource_config_id, workspace_id, credentials, enabled) \
             VALUES ($1, $2, $3, $4, {}) \
             ON CONFLICT (user_id, datasource_config_id) \
             DO UPDATE SET credentials = $4, updated_at = {}",
            sql_compat::bool_true(is_pg),
            sql_compat::now(is_pg),
        );
        kyomi_core::db_execute!(
            pool,
            &sql,
            user_id,
            datasource_config_id,
            workspace_id,
            &encrypted
        )?;
    } else {
        // SQLite: INSERT OR REPLACE with explicit values
        let sql = format!(
            "INSERT INTO user_datasource_credentials \
             (user_id, datasource_config_id, workspace_id, credentials, enabled, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, {}, {now}, {now}) \
             ON CONFLICT (user_id, datasource_config_id) \
             DO UPDATE SET credentials = $4, updated_at = {now}",
            sql_compat::bool_true(is_pg),
            now = sql_compat::now(is_pg),
        );
        kyomi_core::db_execute!(
            pool,
            &sql,
            user_id,
            datasource_config_id,
            workspace_id,
            &encrypted
        )?;
    }

    // Return the saved record
    get_user_credential(pool, user_id, datasource_config_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Internal("Credential saved but not found".into())
        })
}

/// Delete a user's credential record for a datasource.
pub async fn delete_user_credential(
    pool: &DbPool,
    user_id: &str,
    datasource_config_id: &str,
) -> kyomi_core::Result<()> {
    kyomi_core::db_execute!(
        pool,
        "DELETE FROM user_datasource_credentials \
         WHERE user_id = $1 AND datasource_config_id = $2",
        user_id,
        datasource_config_id
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Preference CRUD
// ---------------------------------------------------------------------------

/// Get a user's datasource preference (for shared-auth datasources).
pub async fn get_user_preference(
    pool: &DbPool,
    user_id: &str,
    datasource_config_id: &str,
) -> kyomi_core::Result<Option<UserDatasourcePreference>> {
    let pref = kyomi_core::db_fetch_optional!(
        pool,
        UserDatasourcePreference,
        "SELECT id, user_id, datasource_config_id, enabled,
                  created_at, updated_at
         FROM user_datasource_preferences
         WHERE user_id = $1 AND datasource_config_id = $2",
        user_id,
        datasource_config_id
    )?;

    Ok(pref)
}

/// Upsert a user's datasource preference.
///
/// Creates a new preference record if one does not exist, or updates
/// the existing record's `enabled` flag.
pub async fn upsert_user_preference(
    pool: &DbPool,
    user_id: &str,
    datasource_config_id: &str,
    enabled: bool,
) -> kyomi_core::Result<UserDatasourcePreference> {
    let is_pg = pool.is_postgres();
    let sql = format!(
        "INSERT INTO user_datasource_preferences \
         (user_id, datasource_config_id, enabled, created_at, updated_at) \
         VALUES ($1, $2, $3, {now}, {now}) \
         ON CONFLICT (user_id, datasource_config_id) \
         DO UPDATE SET enabled = $3, updated_at = {now}",
        now = sql_compat::now(is_pg),
    );
    kyomi_core::db_execute!(pool, &sql, user_id, datasource_config_id, enabled)?;

    get_user_preference(pool, user_id, datasource_config_id)
        .await?
        .ok_or_else(|| {
            kyomi_core::Error::Internal("Preference upserted but not found".into())
        })
}

// ---------------------------------------------------------------------------
// Helpers for route handlers (timestamp extraction)
// ---------------------------------------------------------------------------

/// Get the updated_at timestamp for a credential, or None.
pub async fn get_credential_timestamps(
    pool: &DbPool,
    user_id: &str,
    datasource_config_id: &str,
) -> kyomi_core::Result<Option<(DateTime<Utc>, DateTime<Utc>)>> {
    let cred = get_user_credential(pool, user_id, datasource_config_id).await?;
    Ok(cred.map(|c| (c.created_at, c.updated_at)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- generate_slug tests --

    #[test]
    fn slug_from_display_name() {
        assert_eq!(generate_slug("Production BigQuery"), "production-bigquery");
    }

    #[test]
    fn slug_removes_special_chars() {
        assert_eq!(generate_slug("My Database!"), "my-database");
    }

    #[test]
    fn slug_collapses_whitespace() {
        assert_eq!(generate_slug("Test   DB  123"), "test-db-123");
    }

    #[test]
    fn slug_pads_short_names() {
        assert_eq!(generate_slug("A"), "a-db");
    }

    #[test]
    fn slug_handles_empty_name() {
        assert_eq!(generate_slug(""), "datasource");
    }

    #[test]
    fn slug_handles_underscores() {
        assert_eq!(generate_slug("my_database_prod"), "my-database-prod");
    }

    #[test]
    fn slug_truncates_long_names() {
        let long_name = "a".repeat(200);
        let slug = generate_slug(&long_name);
        assert!(slug.len() <= 100);
    }

    #[test]
    fn slug_strips_leading_trailing_hyphens() {
        assert_eq!(generate_slug("--test--"), "test");
    }

    // -- generate_datasource_id tests --

    #[test]
    fn datasource_id_format() {
        let id = generate_datasource_id();
        assert!(id.starts_with("ds-"));
        // "ds-" (3) + 32 hex chars = 35
        assert_eq!(id.len(), 35);
    }

    #[test]
    fn datasource_id_is_unique() {
        let id1 = generate_datasource_id();
        let id2 = generate_datasource_id();
        assert_ne!(id1, id2);
    }

    // -- merge_credentials tests --

    fn test_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        key[..16].copy_from_slice(b"test-key-1234567");
        key[16..].copy_from_slice(b"8901234567890123");
        key
    }

    #[test]
    fn merge_no_existing_returns_new() {
        let key = test_key();
        let new_creds = json!({"username": "admin", "password": "secret"});
        let result = merge_credentials(None, &new_creds, &key).unwrap();
        assert_eq!(result, new_creds);
    }

    #[test]
    fn merge_preserves_oauth_fields() {
        let key = test_key();

        // Simulate existing credential with OAuth tokens
        let existing = json!({
            "billing_project": "old-project",
            "oauth_access_token": "tok-abc",
            "oauth_refresh_token": "ref-xyz",
            "oauth_token_expiry": "2025-01-01T00:00:00Z"
        });
        let encrypted = credential_service::encrypt_credentials(&existing, &key).unwrap();

        // New credentials try to update billing_project but also include empty OAuth fields
        let new_creds = json!({
            "billing_project": "new-project",
            "default_project": "my-project"
        });

        let merged = merge_credentials(Some(&encrypted), &new_creds, &key).unwrap();

        // Non-OAuth fields should be updated
        assert_eq!(merged["billing_project"], "new-project");
        assert_eq!(merged["default_project"], "my-project");

        // OAuth fields should be preserved from existing
        assert_eq!(merged["oauth_access_token"], "tok-abc");
        assert_eq!(merged["oauth_refresh_token"], "ref-xyz");
        assert_eq!(merged["oauth_token_expiry"], "2025-01-01T00:00:00Z");
    }

    #[test]
    fn merge_does_not_allow_overwriting_oauth_fields() {
        let key = test_key();

        let existing = json!({
            "oauth_access_token": "existing-token",
            "billing_project": "old"
        });
        let encrypted = credential_service::encrypt_credentials(&existing, &key).unwrap();

        // Attempt to overwrite OAuth token via new credentials
        let new_creds = json!({
            "billing_project": "new",
            "oauth_access_token": "hacked-token"
        });

        let merged = merge_credentials(Some(&encrypted), &new_creds, &key).unwrap();

        // OAuth field should remain from existing
        assert_eq!(merged["oauth_access_token"], "existing-token");
        assert_eq!(merged["billing_project"], "new");
    }
}
