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
use kyomi_core::db::in_clause_placeholders;
use kyomi_core::models::datasource::{
    DatasourceConfig, UserDatasourceCredential, UserDatasourcePreference,
};
use kyomi_core::sql_compat;
use kyomi_core::DbPool;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

use crate::{credential_service, encryption};

pub use kyomi_types::DatasourceInfo;

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

/// Parameters for [`create_datasource`].
///
/// Bundled into a struct (rather than individual arguments) to stay under
/// clippy's `too_many_arguments` threshold now that an encryption key is
/// required alongside the existing fields.
pub struct CreateDatasourceParams<'a> {
    pub workspace_id: &'a str,
    pub name: &'a str,
    pub slug: Option<&'a str>,
    pub ds_type: &'a str,
    pub connection_config: Value,
    /// Defaults to `"direct"` when `None`.
    pub connection_type: Option<&'a str>,
    /// Used to encrypt any freshly-provided `COMMON_SENSITIVE` field in
    /// `connection_config` (e.g. a brand-new `shared_password` or
    /// `ssh_private_key`) before it is persisted. See
    /// [`credential_service::finalize_connection_config_secrets`].
    pub encryption_key: &'a [u8; 32],
}

/// Create a new datasource configuration.
///
/// Checks for duplicate name and slug within the workspace before inserting.
/// Generates a datasource ID automatically. Encrypts any `COMMON_SENSITIVE`
/// `connection_config` field (see [`credential_service::finalize_connection_config_secrets`])
/// before it is written to the database — there is no existing stored config
/// to restore from on create, so any real value provided is treated as fresh
/// plaintext.
pub async fn create_datasource(
    pool: &DbPool,
    params: CreateDatasourceParams<'_>,
) -> kyomi_core::Result<DatasourceConfig> {
    let CreateDatasourceParams {
        workspace_id,
        name,
        slug,
        ds_type,
        mut connection_config,
        connection_type,
        encryption_key,
    } = params;

    credential_service::finalize_connection_config_secrets(
        &mut connection_config,
        None,
        encryption_key,
    )?;

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
    encryption_key: &[u8; 32],
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

    // Sensitive `connection_config` fields (e.g. `ssh_private_key`,
    // `shared_password`) are masked on read and therefore never round-trip
    // through the frontend as real values. Restore them from the stored
    // config (already ciphertext, never re-encrypted) here so a wholesale
    // replace doesn't clobber or drop the real secret when the caller
    // resubmits the masked placeholder or omits the field entirely. Any
    // genuinely new plaintext value the caller does provide is encrypted
    // before being persisted.
    let final_config = match connection_config {
        Some(mut cfg) => {
            credential_service::finalize_connection_config_secrets(
                &mut cfg,
                Some(&existing.connection_config),
                encryption_key,
            )?;
            cfg
        }
        None => existing.connection_config.clone(),
    };
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
        &final_config,
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
    let existing = encryption::decrypt_json(encrypted, encryption_key)?;

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
    let encrypted = encryption::encrypt_json(&merged, encryption_key)?;

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
// KYO-485: startup retype sweep for string-typed scalar credential leaves
// ---------------------------------------------------------------------------

/// `user_datasource_credentials.credentials` leaves that must be a real
/// `Value::Bool`, never a `Value::String`, for the driver that reads them.
///
/// KYO-485 traced every non-string-typed read (`.as_bool()`, `.as_u64()`,
/// `.as_i64()`, `.as_f64()`, and any `Value::Bool`/`Value::Number` pattern
/// match) of the decrypted per-user credentials blob, across this repo and
/// `kyomi-connect`'s `crates/kyomi-datasource` (every provider's `new()`,
/// plus `factory.rs` and `oauth_refresh.rs`), and found exactly one: `iam`,
/// read via `.and_then(|v| v.as_bool()).unwrap_or(false)` in
/// `providers/redshift.rs::RedshiftProvider::new` to choose between IAM and
/// username/password Redshift authentication. Every other credential leaf
/// on every provider (`username`, `password`, `private_key`,
/// `oauth_access_token`, `client_id`, `cluster_identifier`, `region`,
/// `db_user`, `access_key_id`, `secret_access_key`, `billing_project`,
/// `tenant_id`, `auth_type`, …) is read exclusively via `.as_str()`, so a
/// string-vs-string leaf can never be corrupted by this bug.
///
/// Before KYO-428, `save_datasource_credentials` decoded its request body
/// with the default `serde_qs` codec, which flattens every JSON scalar to
/// text. A credentials blob written or re-saved through that path before
/// the fix may still hold `"iam": "true"` (a string) instead of
/// `"iam": true` (a bool) — `.as_bool()` returns `None` for the string,
/// `.unwrap_or(false)` fires, and the driver silently uses the
/// username/password branch even when IAM auth was intended.
///
/// Extend this list only after repeating that same trace for the new leaf
/// — see
/// `docs/standards/data-state-management/verify-config-keys-against-the-driver-that-reads-them.md`.
const BOOLEAN_CREDENTIAL_LEAVES: &[&str] = &["iam"];

/// Row shape for the startup retype sweep — just enough to decrypt, retype,
/// and write back.
#[derive(sqlx::FromRow)]
struct CredentialRetypeCandidate {
    id: i32,
    credentials: String,
}

/// One-shot repair sweep for `user_datasource_credentials.credentials` rows
/// whose scalar leaves were flattened to JSON strings by the pre-KYO-428,
/// `serde_qs`-decoded `save_datasource_credentials`.
///
/// Unlike `datasource_configs.connection_config` — repaired in place by the
/// KYO-460 SQL migration
/// (`apps/server/migrations/20260823000000_retype_connection_config_scalars.sql`)
/// — `credentials` is a single AES-256-GCM-encrypted blob (see
/// `encryption::encrypt_json`/`decrypt_json`): a SQL `UPDATE` cannot see
/// inside it. This sweep decrypts, retypes only
/// [`BOOLEAN_CREDENTIAL_LEAVES`], and re-encrypts, following the same
/// structure as `kyomi_auth::push_service::purge_invalid_subscriptions` —
/// invoked once at every boot from `apps/server/src/main.rs`, immediately
/// after migrations run.
///
/// Detection is by JSON type, never by value: a leaf is only ever touched
/// when it is exactly `Value::String("true")` or `Value::String("false")`
/// (case-sensitive, matching `serde_qs`'s lowercase `Display` output for
/// Rust `bool` — the only casing this bug could ever have produced).
/// Anything else — already a `Value::Bool`, absent, or a string that is not
/// exactly `"true"`/`"false"` — is left byte-identical; this sweep never
/// destroys or guesses at data.
///
/// Idempotent and safe to run on every boot: a row is only re-encrypted and
/// written back when at least one leaf actually needed retyping. Once a row
/// has been repaired, every leaf in [`BOOLEAN_CREDENTIAL_LEAVES`] is a real
/// `Value::Bool`, so the next boot's pass finds nothing to change for that
/// row and never re-writes it (each `encrypt_json` call draws a fresh nonce,
/// so a needless re-write would still be safe, but it would not be a no-op
/// on disk — the point is that after the first pass there simply is no
/// leaf left to retype). A row that fails to decrypt (wrong/rotated key,
/// corrupted data) is logged and left completely untouched — never dropped,
/// never overwritten with a best-effort guess.
///
/// Returns the number of rows repaired.
pub async fn retype_credential_scalars(
    pool: &DbPool,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<u64> {
    let candidates: Vec<CredentialRetypeCandidate> = kyomi_core::db_fetch_all!(
        pool,
        CredentialRetypeCandidate,
        "SELECT id, credentials FROM user_datasource_credentials"
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!(
            "failed to list user_datasource_credentials for startup retype sweep: {e}"
        ))
    })?;

    let is_pg = pool.is_postgres();
    let mut repaired = 0u64;

    for candidate in candidates {
        let mut plaintext =
            match encryption::decrypt_json(&candidate.credentials, encryption_key) {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        id = candidate.id,
                        error = %e,
                        "user_datasource_credentials row could not be decrypted during \
                         startup retype sweep — left untouched (wrong/rotated key or \
                         corrupted data)"
                    );
                    continue;
                }
            };

        let Some(obj) = plaintext.as_object_mut() else {
            // Not a JSON object (e.g. a legacy non-object payload) — nothing
            // to retype, and nothing this sweep is safe to guess at.
            continue;
        };

        let mut changed_fields: Vec<&'static str> = Vec::new();
        for &field in BOOLEAN_CREDENTIAL_LEAVES {
            let Some(Value::String(s)) = obj.get(field) else {
                // Already correctly typed, or absent — never invented here.
                continue;
            };
            let new_val = match s.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => {
                    // Not unambiguously convertible — leave exactly as
                    // stored rather than guessing.
                    warn!(
                        id = candidate.id,
                        field,
                        value = %s,
                        "credential leaf expected to be boolean holds a string that is \
                         neither \"true\" nor \"false\" — left untouched"
                    );
                    continue;
                }
            };
            obj.insert(field.to_string(), new_val);
            changed_fields.push(field);
        }

        if changed_fields.is_empty() {
            continue;
        }

        let encrypted = match encryption::encrypt_json(&plaintext, encryption_key) {
            Ok(e) => e,
            Err(e) => {
                warn!(
                    id = candidate.id,
                    error = %e,
                    "Failed to re-encrypt retyped credentials during startup sweep — \
                     left untouched"
                );
                continue;
            }
        };

        let sql = format!(
            "UPDATE user_datasource_credentials SET credentials = $1, updated_at = {} \
             WHERE id = $2",
            sql_compat::now(is_pg)
        );

        match kyomi_core::db_execute!(pool, &sql, &encrypted, candidate.id) {
            Ok(_) => {
                repaired += 1;
                info!(
                    id = candidate.id,
                    fields = ?changed_fields,
                    "Retyped string-typed boolean credential leaf(s) at startup"
                );
            }
            Err(e) => {
                warn!(
                    id = candidate.id,
                    error = %e,
                    "Failed to persist retyped credentials during startup sweep"
                );
            }
        }
    }

    Ok(repaired)
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

/// Fetch and decrypt user credentials for a datasource in one step.
///
/// Returns `(raw_record, decrypted_value)`:
/// - `raw_record` is `None` when no credential row exists for this user+datasource.
/// - `decrypted_value` is `serde_json::json!({})` when `raw_record` is `None`.
pub async fn get_decrypted_user_credentials(
    pool: &DbPool,
    user_id: &str,
    datasource_config_id: &str,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<(Option<UserDatasourceCredential>, Value)> {
    let user_cred = get_user_credential(pool, user_id, datasource_config_id).await?;
    let credentials = if let Some(ref cred) = user_cred {
        encryption::decrypt_json(&cred.credentials, encryption_key)
            .unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    Ok((user_cred, credentials))
}

// ---------------------------------------------------------------------------
// Shared helper: decrypt credentials, build provider for an already-resolved
// datasource
// ---------------------------------------------------------------------------

/// Build a ready-to-query provider for an already-resolved datasource.
///
/// This is the shared core of the "decrypt per-user credentials -> build
/// user context -> decrypt connection config -> build provider" sequence
/// that was previously duplicated between `apps/server/src/routes/query_arrow.rs`
/// and `crates/kyomi-ui/src/server_fns/datasources.rs::create_query_provider`.
///
/// Datasource resolution is deliberately NOT part of this helper — callers
/// must call `resolve_datasource` themselves first. Folding resolution in
/// here would force the encryption-key check and the (potentially
/// network-calling, token-refreshing) `build_user_context` closure to run
/// before the resolve outcome is known, changing both side-effect timing
/// (e.g. triggering a Google OAuth refresh for a slug that doesn't even
/// exist) and error precedence (masking a 403/not-found behind an
/// unrelated "encryption key not configured" error) relative to today's
/// behavior. See KYO-138 code review.
///
/// `build_user_context` is taken as a lazy closure rather than a
/// pre-computed value so callers can preserve their *exact* original call
/// order: this helper invokes it after the per-user credential decrypt and
/// before the connection-config decrypt, matching both original call
/// sites' sequencing precisely.
///
/// Steps:
/// 1. Decrypt per-user credentials (skipped for `"connect"`-type datasources).
/// 2. Invoke `build_user_context` (lazy — only evaluated here).
/// 3. Decrypt `COMMON_SENSITIVE` fields in `connection_config`.
/// 4. Delegate Connect-vs-direct branching, OAuth refresh, and connection
///    timeout to `kyomi_datasource_server::create_provider_from_parts`.
///
/// # Errors
///
/// Returns the raw `kyomi_core::Error` from whichever step failed — this
/// helper does not log or translate errors. Callers own all error
/// presentation (HTTP status codes, `ServerFnError` messages, etc.).
pub async fn build_provider_for_datasource<F, Fut>(
    db: &kyomi_core::DbPool,
    user_id: &str,
    ds: &DatasourceConfig,
    encryption_key: &[u8; 32],
    build_user_context: F,
    connect_registry: Option<&kyomi_datasource_server::ConnectRegistry>,
) -> kyomi_core::Result<Box<dyn kyomi_datasource_server::DatasourceProvider>>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = kyomi_core::Result<Option<kyomi_datasource_server::UserContext>>>,
{
    // Decrypt per-user credentials (skipped for Connect-type datasources).
    let credentials = if ds.connection_type != "connect" {
        let user_cred = get_user_credential(db, user_id, &ds.id).await?;
        if let Some(ref cred) = user_cred {
            encryption::decrypt_json(&cred.credentials, encryption_key)
                .unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        }
    } else {
        serde_json::json!({})
    };

    // Build user context for BigQuery OAuth (kyomi_oauth auth mode). Invoked
    // here — after the credential decrypt, before the config decrypt — to
    // preserve the original call order at both call sites exactly.
    let user_context = build_user_context().await?;

    // `ds.connection_config` came straight from the database and may hold
    // encrypted `COMMON_SENSITIVE` fields (e.g. `ssh_private_key`) — the
    // driver always needs plaintext.
    let decrypted_config =
        credential_service::decrypt_connection_config_secrets(&ds.connection_config, encryption_key)?;

    let ds_type: kyomi_core::datasource_registry::DatasourceType = ds.datasource_type.into();
    let provider = kyomi_datasource_server::create_provider_from_parts(
        &ds.id,
        &ds.connection_type,
        &decrypted_config,
        ds_type,
        credentials,
        user_context,
        connect_registry,
    )
    .await?;

    Ok(provider)
}

// ---------------------------------------------------------------------------
// Discovery / test-connection input resolution (KYO-445)
// ---------------------------------------------------------------------------

/// Resolved inputs for a discovery/test-connection provider: the decrypted
/// connection config, the decrypted stored per-user credential blob (not
/// yet overlaid with caller-provided credentials — that overlay is JSON
/// merge logic the caller owns, not a service-layer concern), and the
/// OAuth `UserContext` BigQuery `kyomi_oauth` mode needs.
pub struct DiscoveryConnectionInputs {
    pub connection_config: Value,
    pub stored_creds: Value,
    pub user_context: Option<kyomi_datasource_server::UserContext>,
}

/// Which step of [`resolve_discovery_connection_inputs`] failed. Kept
/// distinct (rather than collapsing to a single `kyomi_core::Error`) so
/// callers can preserve their own per-step error-message shape — the two
/// existing callers report a plain decrypt failure differently from a
/// "Failed to connect" / sanitized OAuth failure.
pub enum DiscoveryPrepError {
    Decrypt(kyomi_core::Error),
    UserContext(kyomi_core::Error),
}

/// Inputs for [`resolve_discovery_connection_inputs`], bundled into one
/// struct rather than taken as separate parameters — the function has
/// several cohesive-but-distinct pieces (who's asking, what datasource,
/// what secrets) that read better named at the call site than as a long
/// positional argument list.
pub struct DiscoveryConnectionRequest<'a> {
    pub user_id: &'a str,
    pub ws_id: &'a str,
    pub datasource_slug: Option<&'a str>,
    pub connection_config: &'a Value,
    pub encryption_key: &'a [u8; 32],
    pub google_client_id: Option<&'a str>,
    pub google_client_secret: Option<&'a str>,
    pub user_email: String,
}

/// Resolve everything a discovery/test-connection caller needs before
/// calling `create_provider`, **without** requiring an already-persisted
/// `DatasourceConfig` row.
///
/// This is deliberately NOT built on [`build_provider_for_datasource`]:
/// that helper takes an already-resolved `&DatasourceConfig`, but this path
/// can run pre-create (the Connection tab's "Validate & Discover Projects"
/// runs before the datasource is saved, with a slug that may not resolve to
/// a row). See KYO-445.
///
/// Steps, in order — this order matters, see
/// `docs/standards/code-organization/preserve-side-effect-and-error-ordering.md`:
/// 1. If `req.datasource_slug` resolves to a persisted row, look up any
///    stored per-user credential blob (e.g. OAuth) for it. Best-effort: any
///    lookup failure (no such datasource, no stored credential) silently
///    yields `None` here, matching pre-KYO-445 behavior — a fresh
///    pre-create discovery has no stored credential to find anyway.
/// 2. Decrypt `req.connection_config` and the stored credential blob (if
///    any). A decrypt failure surfaces as [`DiscoveryPrepError::Decrypt`]
///    *before* the OAuth call below runs, so a bad connection config isn't
///    masked by an unrelated OAuth-refresh failure.
/// 3. Build the OAuth `UserContext` (BigQuery `kyomi_oauth` mode reads its
///    access token from here — see `resolve_kyomi_oauth_token` in
///    kyomi-connect; every other provider/auth-mode ignores it).
pub async fn resolve_discovery_connection_inputs(
    db: &DbPool,
    req: DiscoveryConnectionRequest<'_>,
) -> Result<DiscoveryConnectionInputs, DiscoveryPrepError> {
    let stored_cred_str: Option<String> = if let Some(slug) = req.datasource_slug {
        match get_datasource_by_slug(db, slug, req.ws_id).await {
            Ok(Some(ds)) => match get_user_credential(db, req.user_id, &ds.id).await {
                Ok(Some(cred)) => Some(cred.credentials),
                _ => None,
            },
            _ => None,
        }
    } else {
        None
    };

    let (connection_config, stored_creds) = credential_service::decrypt_provider_secrets(
        req.connection_config,
        stored_cred_str.as_deref(),
        req.encryption_key,
    )
    .map_err(DiscoveryPrepError::Decrypt)?;

    let user_context = crate::google_oauth::build_datasource_user_context(
        db,
        req.user_id,
        Some(req.encryption_key),
        req.google_client_id,
        req.google_client_secret,
        req.user_email,
        req.ws_id.to_string(),
    )
    .await
    .map_err(DiscoveryPrepError::UserContext)?;

    Ok(DiscoveryConnectionInputs {
        connection_config,
        stored_creds,
        user_context,
    })
}

// ---------------------------------------------------------------------------
// Manual catalog refresh orchestration (KYO-143)
// ---------------------------------------------------------------------------

/// Outcome of [`prepare_manual_catalog_refresh`].
pub enum ManualRefreshDecision {
    /// A refresh is genuinely in flight —
    /// `datasource_configs.catalog_refresh_status` is `running` AND the
    /// start stamp is still within the guard window. The caller should not
    /// spawn a duplicate background index.
    AlreadyRunning,
    /// Validation passed. `credentials` are the OAuth-refreshed credentials,
    /// ready for the caller to hand to the background `index_datasource` call.
    Ready { credentials: Value },
}

/// Arguments for [`prepare_manual_catalog_refresh`].
pub struct PrepareManualRefreshParams<'a> {
    pub db: &'a kyomi_core::DbPool,
    pub user_id: &'a str,
    pub email: String,
    pub ws_id: &'a str,
    pub datasource: &'a DatasourceConfig,
    pub encryption_key: &'a [u8; 32],
    pub connect_registry: Option<&'a kyomi_datasource_server::ConnectRegistry>,
    pub google_oauth_client_id: Option<&'a str>,
    pub google_oauth_client_secret: Option<&'a str>,
    /// Concurrency-guard window, in minutes. Passed in rather than
    /// referenced from `kyomi_agent` — this crate must not depend on
    /// `kyomi-agent` (the indexing service depends on `kyomi-auth`, not the
    /// other way around). Callers pass
    /// `kyomi_agent::catalog::indexing_service::CONCURRENT_RUN_GUARD_MINUTES`.
    pub guard_minutes: i64,
    /// Timeout for provider construction and `test_connection()`. Callers
    /// pass `kyomi_datasource_server::DATASOURCE_TIMEOUT_CONNECT`.
    pub connect_timeout: std::time::Duration,
}

/// Prepare a manual catalog refresh: concurrency guard → decrypt/refresh
/// credentials → live connection validation.
///
/// This absorbs everything the `refresh_catalog` server fn used to run
/// inline before backgrounding the slow table-by-table indexing:
///
/// 1. **Concurrency guard.** `datasource_configs.catalog_refresh_status ==
///    running` AND `last_index_started_at` within `guard_minutes` — returns
///    [`ManualRefreshDecision::AlreadyRunning`] without touching credentials
///    or the network if so. `catalog_refresh_status` alone isn't checked
///    without the stamp because a crash could leave it stuck at `running`
///    forever; the stamp bounds how long that can block a re-click.
/// 2. **Credentials.** Decrypt the user's stored credentials, refresh OAuth
///    if needed, and persist the refreshed token — best-effort, matching
///    the original inline behavior (a failed persist doesn't fail the
///    refresh; the refreshed token is still used for this run).
/// 3. **Validation.** Build the provider the same way the shared indexing
///    pipeline will ([`build_provider_for_datasource`], so Connect-type
///    datasources and BigQuery `kyomi_oauth` mode validate correctly) and
///    call `test_connection()`.
///
/// # Errors
///
/// Returns `Err` for the same failure cases the old synchronous
/// `refresh_catalog` body did — bad/expired credentials, unreachable
/// datasource, or a validation timeout — so the caller's error message to
/// the user is unchanged. Only the slow table-by-table indexing itself
/// stays out of this function; the caller backgrounds that afterward using
/// the returned `credentials`.
pub async fn prepare_manual_catalog_refresh(
    p: PrepareManualRefreshParams<'_>,
) -> kyomi_core::Result<ManualRefreshDecision> {
    #[derive(sqlx::FromRow)]
    struct DatasourceRefreshStatusRow {
        catalog_refresh_status: Option<kyomi_core::enums::CatalogRefreshStatus>,
    }

    // KYO-267: scoped to this datasource, not the workspace — filtering on
    // `workspace_id` too (not just `p.datasource.id`) is a tenant-isolation
    // boundary, not redundant, even though the id is already unique.
    let status_row = kyomi_core::db_fetch_optional!(
        p.db,
        DatasourceRefreshStatusRow,
        "SELECT catalog_refresh_status FROM datasource_configs WHERE id = $1 AND workspace_id = $2",
        &p.datasource.id,
        &p.ws_id
    )
    .map_err(|e| {
        kyomi_core::Error::Internal(format!(
            "failed to load datasource catalog refresh status: {e}"
        ))
    })?;

    let is_running = status_row
        .and_then(|row| row.catalog_refresh_status)
        .is_some_and(|s| s == kyomi_core::enums::CatalogRefreshStatus::Running);

    if is_running
        && crate::catalog::helpers::index_started_within(
            p.db,
            &p.datasource.id,
            p.guard_minutes,
        )
        .await
    {
        return Ok(ManualRefreshDecision::AlreadyRunning);
    }

    // Fetch and decrypt credentials in one service call.
    let (user_cred, credentials) =
        get_decrypted_user_credentials(p.db, p.user_id, &p.datasource.id, p.encryption_key).await?;

    let ds_type: kyomi_core::datasource_registry::DatasourceType =
        p.datasource.datasource_type.into();

    // Refresh OAuth credentials if needed. This runs directly here (before
    // `build_provider_for_datasource` below), so an OAuth-refresh failure
    // surfaces from this call rather than from `create_provider_from_parts`.
    // Remap its `Internal` to `DatasourceConnection` — a re-authorization
    // requirement is user-actionable and must reach the "Refresh Now" toast
    // prefix-free, exactly like the connect/timeout failures.
    let credentials = kyomi_datasource_server::ensure_valid_oauth_credentials(
        &credentials,
        &p.datasource.connection_config,
        &ds_type,
    )
    .await
    .map_err(|e| match e {
        kyomi_core::Error::Internal(msg) => kyomi_core::Error::DatasourceConnection(msg),
        other => other,
    })?;

    // Persist refreshed token if it changed.
    if let Some(ref cred) = user_cred {
        let _ = save_user_credential(
            p.db,
            p.encryption_key,
            p.user_id,
            &p.datasource.id,
            &cred.workspace_id,
            &credentials,
        )
        .await;
    }

    // Synchronous validation: build the provider the same way the shared
    // indexing pipeline will and confirm it can actually connect before
    // telling the caller it's safe to background the slow indexing.
    //
    // `build_provider_for_datasource`'s internal `get_user_credential` read
    // happens after the `save_user_credential` call above, so it picks up
    // the OAuth-refreshed token we just persisted rather than the stale one.
    let provider = tokio::time::timeout(
        p.connect_timeout,
        build_provider_for_datasource(
            p.db,
            p.user_id,
            p.datasource,
            p.encryption_key,
            || {
                crate::google_oauth::build_datasource_user_context(
                    p.db,
                    p.user_id,
                    Some(p.encryption_key),
                    p.google_oauth_client_id,
                    p.google_oauth_client_secret,
                    p.email.clone(),
                    p.ws_id.to_string(),
                )
            },
            p.connect_registry,
        ),
    )
    .await
    .map_err(|_| kyomi_core::Error::DatasourceConnection("Connection validation timed out".into()))??;

    // `provider.close()` must run regardless of whether test_connection
    // succeeded, timed out, or errored — capture the result first, close
    // unconditionally, then propagate.
    let connected_result = tokio::time::timeout(p.connect_timeout, provider.test_connection())
        .await
        .map_err(|_| kyomi_core::Error::DatasourceConnection("Connection validation timed out".into()));

    provider.close().await;

    if !connected_result?? {
        return Err(kyomi_core::Error::DatasourceConnection(
            "Connection test failed — check datasource credentials and connectivity".into(),
        ));
    }

    Ok(ManualRefreshDecision::Ready { credentials })
}

// ---------------------------------------------------------------------------
// Enriched view types (used by list/settings orchestration below)
// ---------------------------------------------------------------------------

/// Full datasource settings for the edit modal.
///
/// Returned by [`get_datasource_settings_detail`]. Combines the datasource
/// config with per-user credential details.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatasourceSettingsDetail {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub datasource_type: String,
    /// `"direct"` for standard provider connections, `"connect"` for Kyomi
    /// Connect agent datasources.
    pub connection_type: String,
    pub connection_config: Value,
    pub user_settings: Value,
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

// ---------------------------------------------------------------------------
// List datasources with per-user credential + catalog status
// ---------------------------------------------------------------------------

/// List all active datasources for a workspace, enriched with per-user
/// credential status and catalog attention flags.
///
/// Mirrors the combined logic of `GET /api/v1/datasources` +
/// `GET /api/v1/datasources/credential-status` as previously inlined in the
/// `list_datasources` server_fn.
pub async fn list_datasources_with_status(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<Vec<DatasourceInfo>> {
    // Fetch active datasources
    let datasources = list_datasources(pool, workspace_id, false).await?;

    // Fetch all user credentials in one query
    let user_credentials = kyomi_core::db_fetch_all!(
        pool,
        UserDatasourceCredential,
        "SELECT id, user_id, datasource_config_id, workspace_id, credentials, \
         enabled, created_at, updated_at \
         FROM user_datasource_credentials \
         WHERE user_id = $1 AND workspace_id = $2",
        user_id,
        workspace_id
    )?;

    let creds_by_ds: std::collections::HashMap<&str, &UserDatasourceCredential> =
        user_credentials
            .iter()
            .map(|c| (c.datasource_config_id.as_str(), c))
            .collect();

    // Fetch user preferences for shared-auth datasources
    let user_preferences = kyomi_core::db_fetch_all!(
        pool,
        UserDatasourcePreference,
        "SELECT id, user_id, datasource_config_id, enabled, \
         created_at, updated_at \
         FROM user_datasource_preferences \
         WHERE user_id = $1",
        user_id
    )?;

    let prefs_by_ds: std::collections::HashMap<&str, &UserDatasourcePreference> =
        user_preferences
            .iter()
            .map(|p| (p.datasource_config_id.as_str(), p))
            .collect();

    // Fetch catalog status for each datasource
    let catalog_statuses = fetch_catalog_statuses(pool, &datasources).await;

    let mut result = Vec::with_capacity(datasources.len());

    for ds in &datasources {
        let connection_config = &ds.connection_config;
        let is_connect = ds.connection_type == "connect";

        // Compute credential status (mirrors REST handler logic)
        let (cred_result, user_enabled, can_enable) = if is_connect {
            let pref = prefs_by_ds.get(ds.id.as_str()).copied();
            let enabled = pref.is_none_or(|p| p.enabled);
            let status = crate::datasource_auth_service::CredentialStatusResult {
                credential_status: "shared".to_string(),
                auth_method: "connect".to_string(),
                oauth_provider: None,
            };
            (status, enabled, true)
        } else {
            let user_cred = creds_by_ds.get(ds.id.as_str()).copied();
            let cred_result = crate::datasource_auth_service::check_credential_status(
                ds.datasource_type.as_ref(),
                connection_config,
                user_cred,
                encryption_key,
            );

            let user_enabled = crate::datasource_auth_service::get_user_enabled(
                ds.datasource_type.as_ref(),
                connection_config,
                user_cred,
                prefs_by_ds.get(ds.id.as_str()).copied(),
            );

            let has_credentials = cred_result.credential_status == "valid"
                || cred_result.credential_status == "shared";
            let can_enable = has_credentials || user_enabled;
            (cred_result, user_enabled, can_enable)
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

        let is_analytics = ds
            .connection_config
            .get("analytics_site_id")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty());

        let needs_catalog_attention = ds_catalog_needs_attention(&catalog_statuses, &ds.id);

        let auth_mode = ds
            .connection_config
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .map(String::from);

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
            is_analytics,
            needs_catalog_attention,
            auth_mode,
        });
    }

    Ok(result)
}

#[derive(Debug, sqlx::FromRow)]
struct TableCountRow {
    datasource_config_id: String,
    count: i64,
}

/// Fetch cached (non-archived) table counts for a list of datasource ids in
/// a single grouped query, optionally restricted to one schema/dataset.
///
/// Uses `= ANY($1)` on Postgres and individual placeholders on SQLite,
/// mirroring `chat_service::fetch_session_counts`. A datasource with zero
/// matching cached tables is simply absent from the returned rows.
async fn fetch_table_counts_raw(
    db: &DbPool,
    ds_ids: &[String],
    bf: &str,
    schema: Option<&str>,
) -> Result<Vec<TableCountRow>, sqlx::Error> {
    if ds_ids.is_empty() {
        return Ok(Vec::new());
    }

    match db {
        DbPool::Postgres(pg) => {
            let sql = format!(
                "SELECT datasource_config_id, COUNT(*) as count FROM datasource_table_cache \
                 WHERE datasource_config_id = ANY($1) AND is_archived = {bf} \
                   AND ($2 IS NULL OR dataset_id = $2) \
                 GROUP BY datasource_config_id"
            );
            sqlx::query_as::<_, TableCountRow>(&sql)
                .bind(ds_ids)
                .bind(schema)
                .fetch_all(pg)
                .await
        }
        DbPool::Sqlite(sq) => {
            let (in_clause, next_idx) = in_clause_placeholders(ds_ids.len(), 1);
            let sql = format!(
                "SELECT datasource_config_id, COUNT(*) as count FROM datasource_table_cache \
                 WHERE datasource_config_id IN {in_clause} AND is_archived = {bf} \
                   AND (${next_idx} IS NULL OR dataset_id = ${next_idx}) \
                 GROUP BY datasource_config_id"
            );
            let mut query = sqlx::query_as::<_, TableCountRow>(&sql);
            for id in ds_ids {
                query = query.bind(id);
            }
            query = query.bind(schema);
            query.fetch_all(sq).await
        }
    }
}

/// Canonical accessor for "how many non-archived tables does this
/// datasource have cached", batched over one or more datasource ids and
/// optionally restricted to a single schema/dataset.
///
/// This is the single source of truth for a datasource's table count —
/// `ListDatasourcesTool` and `BrowseCatalogTool` (`crates/kyomi-agent/src/tools/`)
/// both call it rather than hand-rolling their own `COUNT(*)` query, so the
/// `is_archived` exclusion can never again be dropped at a new call site
/// (KYO-615). Hides the Postgres/SQLite bool-literal plumbing `fetch_table_counts_raw`
/// needs — callers just get a map keyed by `datasource_config_id`, with
/// zero-count datasources simply absent.
pub async fn fetch_table_counts(
    db: &DbPool,
    ds_ids: &[String],
    schema: Option<&str>,
) -> Result<std::collections::HashMap<String, i64>, sqlx::Error> {
    let bf = sql_compat::bool_false(db.is_postgres());
    let rows = fetch_table_counts_raw(db, ds_ids, bf, schema).await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.datasource_config_id, row.count))
        .collect())
}

/// Canonical accessor for "how many non-archived tables live in this
/// sentinel workspace" — the workspace-keyed sibling of [`fetch_table_counts`]
/// for tables that live under a synthetic workspace (the BigQuery-public or
/// sample-data shared caches) rather than a real `datasource_config_id`.
/// Optionally restricted to a single schema/dataset. Shares the same
/// `is_archived` exclusion discipline; callers are `BrowseCatalogTool`'s
/// public-dataset contribution and the `bigquery_public`/`sample_data`
/// indexers' own table-count helpers (KYO-615).
pub async fn count_tables_for_workspace(
    db: &DbPool,
    workspace_id: &str,
    schema: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let bf = sql_compat::bool_false(db.is_postgres());
    let sql = format!(
        "SELECT COUNT(*) FROM datasource_table_cache \
         WHERE workspace_id = $1 AND is_archived = {bf} AND ($2 IS NULL OR dataset_id = $2)"
    );
    kyomi_core::db_fetch_scalar!(db, i64, &sql, workspace_id, schema)
}

/// Fetch catalog status (table count + last indexed) for all datasources.
///
/// Every datasource gets an entry, including ones with zero cached tables.
/// A count-query failure is non-fatal: it's logged and every datasource
/// falls back to a `0` count (but still keeps its real `last_catalog_refresh`)
/// rather than propagating the error or dropping entries.
async fn fetch_catalog_statuses(
    db: &DbPool,
    datasources: &[DatasourceConfig],
) -> std::collections::HashMap<String, (i64, Option<DateTime<Utc>>)> {
    // Seed every datasource with a 0 count first, so callers always get an
    // entry regardless of whether the grouped query below finds a row or
    // fails outright.
    let mut result: std::collections::HashMap<String, (i64, Option<DateTime<Utc>>)> = datasources
        .iter()
        .map(|ds| (ds.id.clone(), (0i64, ds.last_catalog_refresh)))
        .collect();

    let ds_ids: Vec<String> = datasources.iter().map(|ds| ds.id.clone()).collect();

    match fetch_table_counts(db, &ds_ids, None).await {
        Ok(counts) => {
            for (ds_id, count) in counts {
                if let Some(entry) = result.get_mut(&ds_id) {
                    entry.0 = count;
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "failed to fetch cached table counts for datasources; defaulting all to 0"
            );
        }
    }

    result
}

/// Check if a datasource's catalog needs attention.
///
/// Returns `true` when:
/// - No tables are indexed
/// - No `last_indexed` timestamp exists
/// - Last indexed more than 7 days ago
fn ds_catalog_needs_attention(
    catalog_statuses: &std::collections::HashMap<String, (i64, Option<DateTime<Utc>>)>,
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

// ---------------------------------------------------------------------------
// Toggle datasource enabled/disabled for a user
// ---------------------------------------------------------------------------

/// Toggle a datasource enabled or disabled for a specific user.
///
/// Handles all auth-mode branches:
/// - Shared auth / Connect datasources → upsert user preference
/// - Personal auth (enable) → validates credential status before enabling,
///   then sets `user_datasource_credentials.enabled = true`
/// - Personal auth (disable) → sets `user_datasource_credentials.enabled = false`
///
/// Returns `Err` if the datasource is inactive or the user lacks credentials
/// required to enable it.
pub async fn toggle_datasource_enabled(
    pool: &DbPool,
    datasource_id: &str,
    workspace_id: &str,
    user_id: &str,
    enabled: bool,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<()> {
    let ds = get_datasource(pool, datasource_id, workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Datasource not found".into()))?;

    if !ds.active {
        return Err(kyomi_core::Error::BadRequest(
            "Datasource is not active".into(),
        ));
    }

    let connection_config = &ds.connection_config;
    let ds_type_str = ds.datasource_type.as_ref();
    let is_shared =
        crate::datasource_auth_service::is_shared_auth(ds_type_str, connection_config);
    let is_connect = ds.connection_type == "connect";

    let user_cred = get_user_credential(pool, user_id, &ds.id).await?;

    if enabled {
        if is_shared || is_connect {
            // Shared auth or Connect — always allow enabling via preference
            upsert_user_preference(pool, user_id, &ds.id, true).await?;
        } else {
            // Personal auth — check credential status before enabling
            let result = crate::datasource_auth_service::check_credential_status(
                ds_type_str,
                connection_config,
                user_cred.as_ref(),
                encryption_key,
            );

            if result.credential_status != "valid" && result.credential_status != "shared" {
                return Err(kyomi_core::Error::BadRequest(
                    "Connect your credentials first to enable this datasource".into(),
                ));
            }

            // Update credential enabled flag
            if let Some(cred) = &user_cred {
                let sql = format!(
                    "UPDATE user_datasource_credentials \
                     SET enabled = true, updated_at = {} \
                     WHERE id = $1",
                    sql_compat::now(pool.is_postgres())
                );
                kyomi_core::db_execute!(pool, &sql, &cred.id)?;
            }
        }
    } else {
        // Disabling — always allowed
        if is_shared || is_connect {
            upsert_user_preference(pool, user_id, &ds.id, false).await?;
        } else if let Some(cred) = &user_cred {
            let sql = format!(
                "UPDATE user_datasource_credentials \
                 SET enabled = false, updated_at = {} \
                 WHERE id = $1",
                sql_compat::now(pool.is_postgres())
            );
            kyomi_core::db_execute!(pool, &sql, &cred.id)?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Get datasource settings detail for the edit modal
// ---------------------------------------------------------------------------

/// Non-sensitive per-user credential fields the datasource edit modal needs.
/// Default-deny whitelist — any field NOT listed here (password, private_key,
/// client_secret, ssh_private_key, service_account_json, OAuth tokens, and any
/// future secret) is excluded automatically and never reaches the client.
const CLIENT_SAFE_USER_SETTINGS_FIELDS: &[&str] =
    &["username", "billing_project", "client_id"];

/// Project the fully-decrypted per-user credential blob down to only the
/// client-safe fields (see `CLIENT_SAFE_USER_SETTINGS_FIELDS`). The full blob
/// contains plaintext secrets and must never be sent to the browser.
fn client_safe_user_settings(full: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(obj) = full.as_object() {
        for &k in CLIENT_SAFE_USER_SETTINGS_FIELDS {
            if let Some(v) = obj.get(k) {
                out.insert(k.to_string(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// Load full datasource settings for the edit modal.
///
/// Combines the datasource config with the user's decrypted credential data
/// and BigQuery OAuth status. Non-admins receive a `NotFound` error for
/// inactive datasources.
///
/// Mirrors `GET /api/v1/datasources/{id}/settings` as previously inlined in
/// the `get_datasource_settings` server_fn.
pub async fn get_datasource_settings_detail(
    pool: &DbPool,
    datasource_id: &str,
    workspace_id: &str,
    user_id: &str,
    is_admin: bool,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<DatasourceSettingsDetail> {
    let ds = get_datasource(pool, datasource_id, workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Datasource not found".into()))?;

    // Non-admins can only view active datasources
    if !is_admin && !ds.active {
        return Err(kyomi_core::Error::NotFound("Datasource not found".into()));
    }

    let user_cred = get_user_credential(pool, user_id, &ds.id).await?;

    let user_settings = match &user_cred {
        Some(cred) => {
            encryption::decrypt_json(&cred.credentials, encryption_key)
                .unwrap_or(serde_json::json!({}))
        }
        None => serde_json::json!({}),
    };

    let connection_config = &ds.connection_config;
    let cred_result = crate::datasource_auth_service::check_credential_status(
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
            .and_then(|json_str| serde_json::from_str::<Value>(json_str).ok())
            .and_then(|v| {
                v.get("client_email")
                    .and_then(|e| e.as_str())
                    .map(|s| s.to_string())
            })
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
    let masked_config = credential_service::mask_connection_config(
        connection_config,
        ds.datasource_type.as_ref(),
    );

    // Project the decrypted per-user credential blob down to the client-safe
    // whitelist — the full blob contains plaintext secrets and must never
    // reach the browser.
    let safe_user_settings = client_safe_user_settings(&user_settings);

    Ok(DatasourceSettingsDetail {
        id: ds.id,
        name: ds.name,
        slug: ds.slug,
        datasource_type: ds.datasource_type.to_string(),
        connection_type: ds.connection_type.clone(),
        connection_config: masked_config,
        user_settings: safe_user_settings,
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sqlx::sqlite::SqlitePoolOptions;

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
        let encrypted = encryption::encrypt_json(&existing, &key).unwrap();

        // New credentials try to update billing_project but also include empty OAuth fields
        let new_creds = json!({
            "billing_project": "new-project",
            "client_id": "app-456"
        });

        let merged = merge_credentials(Some(&encrypted), &new_creds, &key).unwrap();

        // Non-OAuth fields should be updated
        assert_eq!(merged["billing_project"], "new-project");
        assert_eq!(merged["client_id"], "app-456");

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
        let encrypted = encryption::encrypt_json(&existing, &key).unwrap();

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

    // -- client_safe_user_settings tests --

    #[test]
    fn client_safe_user_settings_excludes_secrets() {
        let full = serde_json::json!({
            "username": "alice",
            "password": "s3cr3t",
            "private_key": "-----BEGIN...",
            "client_secret": "shhh",
            "oauth_email": "alice@example.com",
            "oauth_access_token": "tok",
            "client_id": "app-123",
            "billing_project": "proj-b",
            "default_project": "proj-d"
        });
        let safe = client_safe_user_settings(&full);
        let obj = safe.as_object().unwrap();
        // included
        assert_eq!(obj.get("username").and_then(|v| v.as_str()), Some("alice"));
        assert_eq!(obj.get("client_id").and_then(|v| v.as_str()), Some("app-123"));
        assert_eq!(
            obj.get("billing_project").and_then(|v| v.as_str()),
            Some("proj-b")
        );
        // excluded secrets
        for k in [
            "password",
            "private_key",
            "client_secret",
            "oauth_email",
            "oauth_access_token",
        ] {
            assert!(!obj.contains_key(k), "leaked secret field: {k}");
        }
        // KYO-415: default_project was removed from
        // CLIENT_SAFE_USER_SETTINGS_FIELDS along with the rest of the dead
        // "Default Project" UI. It was never a secret, but the doc comment
        // on the allowlist says these are the fields "the datasource edit
        // modal needs" — the modal no longer reads default_project, so it
        // must no longer be projected. A legacy row may still carry the
        // key; that's inert as long as it stops here.
        assert!(
            !obj.contains_key("default_project"),
            "default_project must no longer be projected to the client"
        );
        assert_eq!(obj.len(), 3);
    }

    // -- fetch_catalog_statuses (KYO-201: N+1 -> single grouped query) --

    /// Build an in-memory SQLite pool with migrations applied.
    async fn test_pool() -> DbPool {
        let _ = kyomi_core::constants::load_with_fallback();

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .expect("enable foreign keys");

        sqlx::migrate!("../../apps/server/migrations-sqlite")
            .run(&pool)
            .await
            .expect("run sqlite migrations");

        DbPool::Sqlite(pool)
    }

    async fn seed_workspace_and_owner(pool: &DbPool, workspace_id: &str, owner_user_id: &str) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        };
        sqlx::query("INSERT INTO users (user_id, email) VALUES (?1, ?2)")
            .bind(owner_user_id)
            .bind(format!("{owner_user_id}@test.local"))
            .execute(sq)
            .await
            .expect("insert owner user");
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES (?1, ?2, ?3)",
        )
        .bind(workspace_id)
        .bind(format!("Workspace {workspace_id}"))
        .bind(owner_user_id)
        .execute(sq)
        .await
        .expect("insert workspace");
    }

    async fn seed_datasource(
        pool: &DbPool,
        id: &str,
        workspace_id: &str,
        slug: &str,
        last_catalog_refresh: Option<&str>,
    ) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        };
        sqlx::query(
            "INSERT INTO datasource_configs \
             (id, workspace_id, name, datasource_type, connection_config, slug, last_catalog_refresh) \
             VALUES (?1, ?2, ?3, 'postgres', '{}', ?4, ?5)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(format!("Datasource {id}"))
        .bind(slug)
        .bind(last_catalog_refresh)
        .execute(sq)
        .await
        .expect("insert datasource");
    }

    async fn seed_table_cache_row(
        pool: &DbPool,
        datasource_config_id: &str,
        workspace_id: &str,
        table_id: &str,
        is_archived: bool,
    ) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        };
        sqlx::query(
            "INSERT INTO datasource_table_cache \
             (workspace_id, project_id, dataset_id, table_id, table_metadata, is_archived, datasource_config_id) \
             VALUES (?1, 'proj', 'dataset', ?2, '{}', ?3, ?4)",
        )
        .bind(workspace_id)
        .bind(table_id)
        .bind(is_archived)
        .bind(datasource_config_id)
        .execute(sq)
        .await
        .expect("insert table cache row");
    }

    #[tokio::test]
    async fn fetch_catalog_statuses_covers_every_datasource_including_zero_tables() {
        let pool = test_pool().await;
        seed_workspace_and_owner(&pool, "ws-1", "owner-1").await;
        seed_datasource(&pool, "ds-a", "ws-1", "ds-a", Some("2026-01-05 12:00:00")).await;
        seed_datasource(&pool, "ds-b", "ws-1", "ds-b", None).await;

        // ds-a: 2 non-archived + 1 archived (archived must not count).
        seed_table_cache_row(&pool, "ds-a", "ws-1", "table1", false).await;
        seed_table_cache_row(&pool, "ds-a", "ws-1", "table2", false).await;
        seed_table_cache_row(&pool, "ds-a", "ws-1", "table3", true).await;
        // ds-b: no cache rows at all.

        let datasources = list_datasources(&pool, "ws-1", true).await.unwrap();
        assert_eq!(datasources.len(), 2, "sanity: both datasources listed");

        let statuses = fetch_catalog_statuses(&pool, &datasources).await;
        assert_eq!(statuses.len(), 2, "every datasource must get an entry");

        let ds_a = datasources.iter().find(|d| d.id == "ds-a").unwrap();
        let ds_b = datasources.iter().find(|d| d.id == "ds-b").unwrap();

        let (count_a, refresh_a) = statuses.get("ds-a").expect("ds-a entry present");
        assert_eq!(*count_a, 2, "archived row must not be counted");
        assert_eq!(*refresh_a, ds_a.last_catalog_refresh);

        let (count_b, refresh_b) = statuses
            .get("ds-b")
            .expect("ds-b entry present even with zero cached tables");
        assert_eq!(*count_b, 0);
        assert_eq!(*refresh_b, ds_b.last_catalog_refresh);
        assert!(refresh_b.is_none());
    }

    #[tokio::test]
    async fn fetch_catalog_statuses_empty_input_returns_empty_without_erroring() {
        let pool = test_pool().await;
        let statuses = fetch_catalog_statuses(&pool, &[]).await;
        assert!(statuses.is_empty());
    }

    #[tokio::test]
    async fn fetch_catalog_statuses_query_failure_is_non_fatal_and_defaults_to_zero() {
        let pool = test_pool().await;
        seed_workspace_and_owner(&pool, "ws-1", "owner-1").await;
        seed_datasource(&pool, "ds-a", "ws-1", "ds-a", Some("2026-01-05 12:00:00")).await;
        seed_table_cache_row(&pool, "ds-a", "ws-1", "table1", false).await;

        let datasources = list_datasources(&pool, "ws-1", true).await.unwrap();

        // Force the grouped count query to fail by dropping the table it
        // reads from, simulating a transient DB error.
        if let DbPool::Sqlite(sq) = &pool {
            sqlx::query("DROP TABLE datasource_table_cache")
                .execute(sq)
                .await
                .expect("drop table to simulate query failure");
        }

        let statuses = fetch_catalog_statuses(&pool, &datasources).await;

        // Non-fatal: still one entry per datasource, count defaulted to 0,
        // but the real last_catalog_refresh is preserved (it doesn't come
        // from the failed query).
        assert_eq!(statuses.len(), 1);
        let (count, refresh) = statuses.get("ds-a").expect("entry still present on failure");
        assert_eq!(*count, 0);
        assert!(refresh.is_some(), "last_catalog_refresh should survive the query failure");
    }

    // ─── fetch_table_counts / count_tables_for_workspace (KYO-615) ─────────
    //
    // These are the canonical accessors `ListDatasourcesTool` and
    // `BrowseCatalogTool` (crates/kyomi-agent/src/tools/) both call, in
    // place of hand-rolled per-call-site `COUNT(*)` queries.

    /// Insert a table cache row with an explicit `dataset_id`, unlike
    /// `seed_table_cache_row` above which hardcodes `dataset_id = 'dataset'`.
    /// `datasource_config_id` is optional so this can also seed sentinel
    /// rows (BigQuery-public / sample-data) that belong to a workspace but
    /// no real datasource config.
    async fn seed_table_cache_row_with_dataset(
        pool: &DbPool,
        datasource_config_id: Option<&str>,
        workspace_id: &str,
        dataset_id: &str,
        table_id: &str,
        is_archived: bool,
    ) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        };
        sqlx::query(
            "INSERT INTO datasource_table_cache \
             (workspace_id, project_id, dataset_id, table_id, table_metadata, is_archived, datasource_config_id) \
             VALUES (?1, 'proj', ?2, ?3, '{}', ?4, ?5)",
        )
        .bind(workspace_id)
        .bind(dataset_id)
        .bind(table_id)
        .bind(is_archived)
        .bind(datasource_config_id)
        .execute(sq)
        .await
        .expect("insert table cache row");
    }

    #[tokio::test]
    async fn fetch_table_counts_excludes_archived_and_batches_multiple_ids_in_one_call() {
        let pool = test_pool().await;
        seed_workspace_and_owner(&pool, "ws-1", "owner-1").await;
        seed_datasource(&pool, "ds-a", "ws-1", "ds-a", None).await;
        seed_datasource(&pool, "ds-b", "ws-1", "ds-b", None).await;

        seed_table_cache_row(&pool, "ds-a", "ws-1", "table1", false).await;
        seed_table_cache_row(&pool, "ds-a", "ws-1", "table2", false).await;
        seed_table_cache_row(&pool, "ds-a", "ws-1", "table3", true).await; // archived
        seed_table_cache_row(&pool, "ds-b", "ws-1", "table4", false).await;

        // One call with both ids — the "batches N datasources in a single
        // query, not N" structural proof: a single await on one function
        // call returns counts for every id, rather than the caller looping
        // and awaiting once per datasource.
        let counts = fetch_table_counts(&pool, &["ds-a".to_string(), "ds-b".to_string()], None)
            .await
            .expect("fetch_table_counts must succeed");

        assert_eq!(counts.get("ds-a").copied(), Some(2), "archived row must not be counted");
        assert_eq!(counts.get("ds-b").copied(), Some(1));
    }

    #[tokio::test]
    async fn fetch_table_counts_schema_filter_reports_that_schemas_total_not_the_datasources() {
        let pool = test_pool().await;
        seed_workspace_and_owner(&pool, "ws-1", "owner-1").await;
        seed_datasource(&pool, "ds-a", "ws-1", "ds-a", None).await;

        seed_table_cache_row_with_dataset(&pool, Some("ds-a"), "ws-1", "sales", "orders", false)
            .await;
        seed_table_cache_row_with_dataset(&pool, Some("ds-a"), "ws-1", "sales", "refunds", false)
            .await;
        seed_table_cache_row_with_dataset(&pool, Some("ds-a"), "ws-1", "marketing", "leads", false)
            .await;

        let unfiltered = fetch_table_counts(&pool, &["ds-a".to_string()], None)
            .await
            .expect("fetch_table_counts (unfiltered) must succeed");
        assert_eq!(unfiltered.get("ds-a").copied(), Some(3), "sanity: whole-datasource total");

        let sales_only = fetch_table_counts(&pool, &["ds-a".to_string()], Some("sales"))
            .await
            .expect("fetch_table_counts (schema-filtered) must succeed");
        assert_eq!(
            sales_only.get("ds-a").copied(),
            Some(2),
            "a schema filter must report that schema's total, not the whole datasource's"
        );
    }

    #[tokio::test]
    async fn count_tables_for_workspace_excludes_archived_and_respects_schema_filter() {
        let pool = test_pool().await;

        // Sentinel-workspace rows (BigQuery-public/sample-data shape): a
        // real `workspace_id`, no `datasource_config_id`. No `workspaces`
        // row needed — `workspace_id` on this table carries no FK.
        seed_table_cache_row_with_dataset(&pool, None, "public-data-workspace", "hacker_news", "full", false)
            .await;
        seed_table_cache_row_with_dataset(&pool, None, "public-data-workspace", "hacker_news", "stories", false)
            .await;
        seed_table_cache_row_with_dataset(&pool, None, "public-data-workspace", "hacker_news", "old", true)
            .await; // archived
        seed_table_cache_row_with_dataset(&pool, None, "public-data-workspace", "covid19", "cases", false)
            .await;

        let total = count_tables_for_workspace(&pool, "public-data-workspace", None)
            .await
            .expect("count_tables_for_workspace (unfiltered) must succeed");
        assert_eq!(total, 3, "archived row must not be counted");

        let scoped = count_tables_for_workspace(&pool, "public-data-workspace", Some("hacker_news"))
            .await
            .expect("count_tables_for_workspace (schema-filtered) must succeed");
        assert_eq!(scoped, 2, "schema filter must report only that schema's tables");

        let other_workspace = count_tables_for_workspace(&pool, "sample-data-workspace", None)
            .await
            .expect("count_tables_for_workspace for an empty sentinel workspace must succeed");
        assert_eq!(other_workspace, 0);
    }

    // ─── retype_credential_scalars tests (KYO-485) ─────────────────────────
    //
    // `retype_credential_scalars`'s SQL is backend-generic (plain `$1`/`$2`
    // positional binds, no `= ANY($1)` array bind or other Postgres-only
    // shape), so — unlike the KYO-292 section below — these run against the
    // in-memory `sqlite::memory:` pool like every other test in this module
    // and need no live Postgres to exercise the real read-modify-write path.

    fn test_encryption_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        key[..16].copy_from_slice(b"kyo485-test-key-");
        key[16..].copy_from_slice(b"0123456789abcdef");
        key
    }

    /// Insert a `user_datasource_credentials` row with `credentials`
    /// pre-encrypted from `plaintext` — the shape the sweep must operate on.
    /// Returns the exact ciphertext written, so a caller that needs to
    /// prove a row was left byte-identical has something other than a
    /// fresh `encrypt_json` call to compare against (each call draws a new
    /// AES-GCM nonce, so re-encrypting the same plaintext never reproduces
    /// the original ciphertext).
    async fn seed_encrypted_credential(
        pool: &DbPool,
        user_id: &str,
        datasource_config_id: &str,
        workspace_id: &str,
        plaintext: &Value,
        key: &[u8; 32],
    ) -> String {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => panic!("test requires sqlite pool"),
        };
        let encrypted = encryption::encrypt_json(plaintext, key).expect("encrypt fixture credentials");
        sqlx::query(
            "INSERT INTO user_datasource_credentials \
             (user_id, datasource_config_id, workspace_id, credentials) \
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(user_id)
        .bind(datasource_config_id)
        .bind(workspace_id)
        .bind(&encrypted)
        .execute(sq)
        .await
        .expect("insert encrypted credential row");
        encrypted
    }

    /// The core regression test: a row whose `credentials` blob is
    /// *encrypted* (never plaintext JSON — that would not exercise the
    /// decrypt/retype/re-encrypt path this sweep exists for) and whose
    /// `iam` leaf was flattened to the JSON string `"true"` by the
    /// pre-KYO-428 codec must come out with `iam` as a real `Value::Bool`,
    /// with every other leaf preserved untouched, and must still decrypt
    /// with the same key afterward.
    #[tokio::test]
    async fn retype_credential_scalars_fixes_string_typed_iam_bool_in_an_encrypted_row() {
        let pool = test_pool().await;
        seed_workspace_and_owner(&pool, "ws-485", "user-485").await;
        seed_datasource(&pool, "ds-485", "ws-485", "ds-485-slug", None).await;

        let key = test_encryption_key();
        let corrupted = json!({
            "iam": "true",
            "cluster_identifier": "prod-cluster",
            "region": "us-east-1",
            "db_user": "admin",
        });
        seed_encrypted_credential(&pool, "user-485", "ds-485", "ws-485", &corrupted, &key).await;

        let repaired = retype_credential_scalars(&pool, &key)
            .await
            .expect("retype_credential_scalars must succeed");
        assert_eq!(repaired, 1, "the one corrupted row must be repaired");

        let cred = get_user_credential(&pool, "user-485", "ds-485")
            .await
            .expect("fetch repaired credential")
            .expect("credential row must still exist");
        let decrypted = encryption::decrypt_json(&cred.credentials, &key)
            .expect("repaired row must still decrypt with the same key");

        assert_eq!(
            decrypted.get("iam"),
            Some(&Value::Bool(true)),
            "iam must now be a real JSON bool, not the string \"true\": {decrypted:?}"
        );
        assert_eq!(
            decrypted.get("cluster_identifier").and_then(|v| v.as_str()),
            Some("prod-cluster"),
            "unrelated string leaves must be preserved exactly"
        );
        assert_eq!(decrypted.get("region").and_then(|v| v.as_str()), Some("us-east-1"));
        assert_eq!(decrypted.get("db_user").and_then(|v| v.as_str()), Some("admin"));

        // Idempotency: every leaf is now correctly typed, so a second pass
        // must find nothing left to retype and must not re-write the row.
        let repaired_again = retype_credential_scalars(&pool, &key)
            .await
            .expect("second retype pass must succeed");
        assert_eq!(
            repaired_again, 0,
            "a row with no remaining string-typed boolean leaf must not be re-written"
        );
    }

    /// A row whose `iam` leaf is already the correct type must not be
    /// touched — proves the sweep detects by JSON type and does not simply
    /// stamp every row with a fresh nonce (and a bumped `updated_at`) on
    /// every boot.
    #[tokio::test]
    async fn retype_credential_scalars_leaves_an_already_correct_bool_untouched() {
        let pool = test_pool().await;
        seed_workspace_and_owner(&pool, "ws-485b", "user-485b").await;
        seed_datasource(&pool, "ds-485b", "ws-485b", "ds-485b-slug", None).await;

        let key = test_encryption_key();
        let already_correct = json!({ "iam": true, "cluster_identifier": "prod-cluster" });
        seed_encrypted_credential(&pool, "user-485b", "ds-485b", "ws-485b", &already_correct, &key)
            .await;

        let repaired = retype_credential_scalars(&pool, &key)
            .await
            .expect("retype_credential_scalars must succeed");
        assert_eq!(repaired, 0, "an already-correctly-typed row must not be reported as repaired");
    }

    /// Conservative-by-construction: a string value that is neither exactly
    /// `"true"` nor `"false"` is not unambiguously convertible, so it must
    /// be left exactly as stored rather than guessed at.
    #[tokio::test]
    async fn retype_credential_scalars_leaves_a_non_bool_string_value_untouched() {
        let pool = test_pool().await;
        seed_workspace_and_owner(&pool, "ws-485c", "user-485c").await;
        seed_datasource(&pool, "ds-485c", "ws-485c", "ds-485c-slug", None).await;

        let key = test_encryption_key();
        // Not a value this bug could ever have produced (serde_qs's bool
        // Display output is always lowercase "true"/"false") and not
        // unambiguously convertible — must be left alone rather than
        // guessed at.
        let ambiguous = json!({ "iam": "Yes" });
        seed_encrypted_credential(&pool, "user-485c", "ds-485c", "ws-485c", &ambiguous, &key).await;

        let repaired = retype_credential_scalars(&pool, &key)
            .await
            .expect("retype_credential_scalars must succeed");
        assert_eq!(repaired, 0, "an ambiguous string value must not be converted or counted as repaired");

        let cred = get_user_credential(&pool, "user-485c", "ds-485c")
            .await
            .expect("fetch credential")
            .expect("credential row must still exist");
        let decrypted = encryption::decrypt_json(&cred.credentials, &key).expect("must still decrypt");
        assert_eq!(
            decrypted.get("iam"),
            Some(&Value::String("Yes".to_string())),
            "an ambiguous value must be left byte-identical, not coerced to a bool: {decrypted:?}"
        );
    }

    /// A row that cannot be decrypted (wrong/rotated key, corrupted data)
    /// must be left completely untouched and must not abort the sweep for
    /// the rows after it.
    #[tokio::test]
    async fn retype_credential_scalars_skips_an_undecryptable_row_without_aborting() {
        let pool = test_pool().await;
        seed_workspace_and_owner(&pool, "ws-485d", "user-485d").await;
        seed_datasource(&pool, "ds-485d-a", "ws-485d", "ds-485d-a-slug", None).await;
        seed_datasource(&pool, "ds-485d-b", "ws-485d", "ds-485d-b-slug", None).await;

        let key = test_encryption_key();
        let wrong_key = {
            let mut k = key;
            k[0] ^= 0xFF;
            k
        };
        // Row A: encrypted with a different key — cannot be decrypted with `key`.
        let row_a_original_ciphertext = seed_encrypted_credential(
            &pool,
            "user-485d",
            "ds-485d-a",
            "ws-485d",
            &json!({ "iam": "true" }),
            &wrong_key,
        )
        .await;
        // Row B: a genuinely corrupted row the sweep must still repair.
        seed_encrypted_credential(
            &pool,
            "user-485d",
            "ds-485d-b",
            "ws-485d",
            &json!({ "iam": "false" }),
            &key,
        )
        .await;

        let repaired = retype_credential_scalars(&pool, &key)
            .await
            .expect("retype_credential_scalars must not error out on an undecryptable row");
        assert_eq!(repaired, 1, "only the decryptable, genuinely corrupted row must be repaired");

        let cred_a = get_user_credential(&pool, "user-485d", "ds-485d-a")
            .await
            .expect("fetch row A")
            .expect("row A must still exist, untouched");
        assert_eq!(
            cred_a.credentials, row_a_original_ciphertext,
            "an undecryptable row must be left byte-identical, never dropped or overwritten"
        );
        // Row A must still fail to decrypt with the real key — it was never
        // touched, dropped, or overwritten with a best-effort guess.
        assert!(encryption::decrypt_json(&cred_a.credentials, &key).is_err());

        let cred_b = get_user_credential(&pool, "user-485d", "ds-485d-b")
            .await
            .expect("fetch row B")
            .expect("row B must still exist");
        let decrypted_b =
            encryption::decrypt_json(&cred_b.credentials, &key).expect("row B must decrypt");
        assert_eq!(decrypted_b.get("iam"), Some(&Value::Bool(false)));
    }

    // ─── Postgres coverage (KYO-292) ───────────────────────────────────────
    //
    // Every test above runs against `sqlite::memory:`, so `fetch_table_counts`'s
    // Postgres arm (`= ANY($1)` + array bind) is type-checked but never
    // executed by this crate's test suite. This test runs the same function
    // against a real per-worktree Postgres database (see `crate::test_pg`)
    // and skips cleanly — with a visible `SKIP:` line — when Postgres isn't
    // reachable, so `cargo test -p kyomi-auth` with no Postgres available
    // still passes.
    //
    // KYO-615: exercises the pub `fetch_table_counts` accessor (not the
    // private `fetch_table_counts_raw`) — this is the same function
    // `ListDatasourcesTool` and `BrowseCatalogTool` call, so this test's
    // Postgres coverage of the archived-row exclusion applies to both.

    /// Seed the owner user and workspace together — a thin composition of
    /// the two shared `crate::test_pg` fixtures, since every Postgres test
    /// in this module needs both.
    async fn seed_workspace_and_owner_pg(pg: &sqlx::PgPool, workspace_id: &str, owner_user_id: &str) {
        crate::test_pg::seed_user_pg(pg, owner_user_id, &format!("{owner_user_id}@test.local"))
            .await;
        crate::test_pg::seed_workspace_pg(pg, workspace_id, owner_user_id).await;
    }

    async fn seed_datasource_pg(pg: &sqlx::PgPool, id: &str, workspace_id: &str, slug: &str) {
        sqlx::query(
            "INSERT INTO datasource_configs \
             (id, workspace_id, name, datasource_type, connection_config, slug) \
             VALUES ($1, $2, $3, 'postgres', '{}', $4)",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(format!("Datasource {id}"))
        .bind(slug)
        .execute(pg)
        .await
        .expect("insert datasource (postgres)");
    }

    async fn seed_table_cache_row_pg(
        pg: &sqlx::PgPool,
        datasource_config_id: &str,
        workspace_id: &str,
        table_id: &str,
        is_archived: bool,
    ) {
        sqlx::query(
            "INSERT INTO datasource_table_cache \
             (workspace_id, project_id, dataset_id, table_id, table_metadata, is_archived, datasource_config_id) \
             VALUES ($1, 'proj', 'dataset', $2, '{}', $3, $4)",
        )
        .bind(workspace_id)
        .bind(table_id)
        .bind(is_archived)
        .bind(datasource_config_id)
        .execute(pg)
        .await
        .expect("insert table cache row (postgres)");
    }

    /// Delete everything a Postgres test in this module inserted, scoped by
    /// `workspace_id`, so repeated local runs against this worktree's
    /// persistent test database don't accumulate rows. FK order: table
    /// cache -> datasource configs -> workspace -> owner.
    async fn cleanup_pg(pg: &sqlx::PgPool, workspace_id: &str, owner_user_id: &str) {
        sqlx::query("DELETE FROM datasource_table_cache WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(pg)
            .await
            .expect("cleanup datasource_table_cache (postgres)");
        sqlx::query("DELETE FROM datasource_configs WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(pg)
            .await
            .expect("cleanup datasource_configs (postgres)");
        crate::test_pg::cleanup_workspace_and_users_pg(pg, workspace_id, &[owner_user_id]).await;
    }

    #[tokio::test]
    async fn postgres_fetch_table_counts_excludes_archived_rows() {
        let test_name = "postgres_fetch_table_counts_excludes_archived_rows";
        let Some(db) = crate::test_pg::postgres_test_pool_or_skip(test_name).await else {
            return;
        };
        let pg = crate::test_pg::postgres_pool(&db);

        let workspace_id = crate::test_pg::unique_test_id("ws");
        let owner_id = crate::test_pg::unique_test_id("owner");
        let ds_a = crate::test_pg::unique_test_id("ds-a");
        let ds_b = crate::test_pg::unique_test_id("ds-b");

        seed_workspace_and_owner_pg(pg, &workspace_id, &owner_id).await;
        seed_datasource_pg(pg, &ds_a, &workspace_id, "ds-a-slug").await;
        seed_datasource_pg(pg, &ds_b, &workspace_id, "ds-b-slug").await;

        // ds_a: 2 non-archived + 1 archived (archived must not count). ds_b:
        // no cache rows at all, so it must not appear in the result either.
        seed_table_cache_row_pg(pg, &ds_a, &workspace_id, "table1", false).await;
        seed_table_cache_row_pg(pg, &ds_a, &workspace_id, "table2", false).await;
        seed_table_cache_row_pg(pg, &ds_a, &workspace_id, "table3", true).await;

        let counts = fetch_table_counts(&db, &[ds_a.clone(), ds_b.clone()], None)
            .await
            .expect("fetch_table_counts must succeed against a real Postgres pool");

        assert_eq!(
            counts.len(),
            1,
            "only ds_a has cache rows, ds_b must not produce an entry: {counts:?}"
        );
        assert_eq!(
            counts.get(&ds_a).copied(),
            Some(2),
            "the archived row must not be counted"
        );

        cleanup_pg(pg, &workspace_id, &owner_id).await;
    }

    #[tokio::test]
    async fn postgres_fetch_table_counts_respects_schema_filter() {
        let test_name = "postgres_fetch_table_counts_respects_schema_filter";
        let Some(db) = crate::test_pg::postgres_test_pool_or_skip(test_name).await else {
            return;
        };
        let pg = crate::test_pg::postgres_pool(&db);

        let workspace_id = crate::test_pg::unique_test_id("ws");
        let owner_id = crate::test_pg::unique_test_id("owner");
        let ds_a = crate::test_pg::unique_test_id("ds-a");

        seed_workspace_and_owner_pg(pg, &workspace_id, &owner_id).await;
        seed_datasource_pg(pg, &ds_a, &workspace_id, "ds-a-slug").await;

        seed_table_cache_row_pg(pg, &ds_a, &workspace_id, "table1", false).await;
        seed_table_cache_row_pg(pg, &ds_a, &workspace_id, "table2", false).await;

        let unfiltered = fetch_table_counts(&db, std::slice::from_ref(&ds_a), None)
            .await
            .expect("fetch_table_counts (unfiltered) must succeed against a real Postgres pool");
        assert_eq!(unfiltered.get(&ds_a).copied(), Some(2));

        // `seed_table_cache_row_pg` always writes `dataset_id = 'dataset'`,
        // so filtering to that schema must return the same count, and
        // filtering to a schema with no rows must return none at all.
        let filtered = fetch_table_counts(&db, std::slice::from_ref(&ds_a), Some("dataset"))
            .await
            .expect("fetch_table_counts (filtered) must succeed against a real Postgres pool");
        assert_eq!(filtered.get(&ds_a).copied(), Some(2));

        let filtered_out = fetch_table_counts(&db, std::slice::from_ref(&ds_a), Some("other-schema"))
            .await
            .expect("fetch_table_counts (filtered_out) must succeed against a real Postgres pool");
        assert_eq!(
            filtered_out.get(&ds_a),
            None,
            "a schema filter that matches no rows must produce no entry, not a zero-value one"
        );

        cleanup_pg(pg, &workspace_id, &owner_id).await;
    }

    // -- resolve_discovery_connection_inputs builds a UserContext (KYO-445) --

    /// `resolve_discovery_connection_inputs` must build a `UserContext` via
    /// `build_datasource_user_context` and carry the result through to
    /// `DiscoveryConnectionInputs.user_context` — dropping it, or hardcoding
    /// `None`, would reproduce the KYO-445 regression one layer down after
    /// `discover_datasource_resources` (kyomi-ui) was extracted to call
    /// this function instead of building the `UserContext` inline itself
    /// (extracted to satisfy `check-server-fns.sh`'s Rule B service-layer
    /// callout limit — see that function's call site).
    ///
    /// Exercising this end-to-end needs a live Google OAuth token this test
    /// environment doesn't have (see the sibling source-assertion test in
    /// `kyomi-ui/src/server_fns/datasources.rs`, which pins the other half
    /// of the chain: that the caller threads this function's `user_context`
    /// into `create_provider` rather than a literal `None`), so this pins
    /// the wiring at the source level instead.
    #[test]
    fn resolve_discovery_connection_inputs_threads_build_datasource_user_context_through() {
        const SRC: &str = include_str!("datasource_service.rs");
        let fn_start = SRC
            .find("pub async fn resolve_discovery_connection_inputs(")
            .expect("resolve_discovery_connection_inputs not found in datasource_service.rs");
        let fn_end = SRC[fn_start..]
            .find("\n// ---")
            .map(|i| fn_start + i)
            .unwrap_or(SRC.len());
        let body = &SRC[fn_start..fn_end];

        assert!(
            body.contains("crate::google_oauth::build_datasource_user_context("),
            "resolve_discovery_connection_inputs must build a UserContext via \
             build_datasource_user_context — without this call the BigQuery \
             kyomi_oauth path has no OAuth data to construct a UserContext from"
        );
        // Structural, not layout-exact: must find `user_context` (the
        // built value, not a literal) somewhere after the
        // build_datasource_user_context call, inside a `DiscoveryConnectionInputs`
        // construction — without pinning field order or whitespace, which
        // `cargo fmt` or a field reorder could otherwise break for free.
        let after_build = {
            let idx = body
                .find("crate::google_oauth::build_datasource_user_context(")
                .expect("build_datasource_user_context call not found");
            &body[idx..]
        };
        assert!(
            after_build.contains("DiscoveryConnectionInputs {") && after_build.contains("user_context"),
            "the built user_context must be threaded into the returned \
             DiscoveryConnectionInputs, not dropped or hardcoded"
        );
        assert!(
            !body.contains("user_context: None"),
            "regression guard: hardcoding user_context: None here reproduces \
             the KYO-445 bug one layer down"
        );
    }
}
