// SPDX-License-Identifier: AGPL-3.0-or-later

//! Analytics site service — CRUD for analytics sites with HMAC-signed keys.
//!
//! Each analytics site gets a self-contained signed key that embeds the site_id,
//! workspace_id, and allowed domains. The collector can verify it statelessly
//! without any DB lookup.
//!
//! Key design decisions:
//! - Free-function pattern (`&DbPool` first arg) matching other services
//! - Workspace-scoped: all operations filter by workspace_id
//! - HMAC-SHA256 signing with constant-time verification (via `subtle`)
//! - Key format: `base64url(json_payload).base64url(hmac_signature)`
//!
//! ## SQLite compatibility note
//!
//! Analytics sites require ClickHouse infrastructure and use Postgres array types
//! (`text[]` for `allowed_domains`). The CRUD functions in this module only support
//! Postgres at runtime and return `Error::Internal` if called on a SQLite pool.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use kyomi_core::{DbPool, Result};
use rand::Rng;
use serde_json::json;
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::{analytics_clickhouse, datasource_service};

type HmacSha256 = Hmac<Sha256>;

/// Build the `<script>` snippet for an analytics site's signed key.
pub fn snippet_tag(signed_key: &str) -> String {
    let url = &kyomi_core::constants::get().analytics.collector_url;
    format!(
        r#"<script defer data-key="{signed_key}" src="{url}/k.js"></script>"#,
    )
}

// ─── Types ──────────────────────────────────────────────────────────────────

/// Database row for an analytics site (with optional datasource join).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AnalyticsSite {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub site_id: String,
    pub allowed_domains: Vec<String>,
    pub signed_key: String,
    pub datasource_id: Option<String>,
    pub clickhouse_database: Option<String>,
    pub datasource_slug: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Internal row type for Postgres queries — derives FromRow with Vec<String>.
/// Only used in the Postgres arm of match blocks.
#[derive(Debug, sqlx::FromRow)]
struct AnalyticsSitePgRow {
    id: String,
    workspace_id: String,
    name: String,
    site_id: String,
    allowed_domains: Vec<String>,
    signed_key: String,
    datasource_id: Option<String>,
    clickhouse_database: Option<String>,
    datasource_slug: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<AnalyticsSitePgRow> for AnalyticsSite {
    fn from(row: AnalyticsSitePgRow) -> Self {
        Self {
            id: row.id,
            workspace_id: row.workspace_id,
            name: row.name,
            site_id: row.site_id,
            allowed_domains: row.allowed_domains,
            signed_key: row.signed_key,
            datasource_id: row.datasource_id,
            clickhouse_database: row.clickhouse_database,
            datasource_slug: row.datasource_slug,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Payload embedded in the signed key.
#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
pub struct KeyPayload {
    /// site_id
    pub s: String,
    /// workspace_id
    pub w: String,
    /// allowed domains
    pub d: Vec<String>,
}

/// Return an error indicating analytics sites require Postgres.
fn sqlite_not_supported() -> kyomi_core::Error {
    kyomi_core::Error::Internal(
        "Analytics sites require Postgres (uses text[] columns and ClickHouse integration)".into(),
    )
}

// ─── Key generation / verification ──────────────────────────────────────────

/// Generate a random 16-char hex site_id (matches VARCHAR(16) column).
pub fn generate_site_id() -> String {
    let bytes: [u8; 8] = rand::rng().random();
    hex::encode(bytes)
}

/// Generate a signed key: `base64url(json_payload).base64url(hmac_sha256(payload_b64, secret))`
///
/// The resulting key is self-contained — the collector can verify it and extract
/// the site_id, workspace_id, and allowed domains without any database lookup.
pub fn generate_signed_key(
    site_id: &str,
    workspace_id: &str,
    domains: &[String],
    secret: &str,
) -> String {
    let payload = KeyPayload {
        s: site_id.to_string(),
        w: workspace_id.to_string(),
        d: domains.to_vec(),
    };
    let payload_json = serde_json::to_vec(&payload).expect("KeyPayload serialization cannot fail");
    let payload_b64 = URL_SAFE_NO_PAD.encode(&payload_json);

    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload_b64.as_bytes());
    let sig = mac.finalize().into_bytes();
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig);

    format!("{payload_b64}.{sig_b64}")
}

/// Verify a signed key and extract its payload. Uses constant-time comparison.
///
/// Returns `None` if the signature is invalid, the key is malformed, or it is empty.
pub fn verify_signed_key(key: &str, secret: &str) -> Option<KeyPayload> {
    let (payload_b64, sig_b64) = key.split_once('.')?;

    if payload_b64.is_empty() || sig_b64.is_empty() {
        return None;
    }

    let provided_sig = URL_SAFE_NO_PAD.decode(sig_b64).ok()?;

    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload_b64.as_bytes());
    let expected_sig = mac.finalize().into_bytes();

    // Constant-time comparison to prevent timing attacks
    if provided_sig.as_slice().ct_eq(&expected_sig).into() {
        let payload_json = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
        serde_json::from_slice(&payload_json).ok()
    } else {
        None
    }
}

// ─── Helper: Postgres-only query execution ─────────────────────────────────

/// Fetch all analytics site rows (Postgres only).
async fn pg_fetch_all_sites(
    db: &DbPool,
    sql: &str,
    binds: &[&str],
) -> Result<Vec<AnalyticsSite>> {
    match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let mut query = sqlx::query_as::<_, AnalyticsSitePgRow>(sql);
            for b in binds {
                query = query.bind(*b);
            }
            let rows = query.fetch_all(pg).await.map_err(|e| {
                kyomi_core::Error::Internal(format!("failed to query analytics sites: {e}"))
            })?;
            Ok(rows.into_iter().map(AnalyticsSite::from).collect())
        }
        kyomi_core::db::DbPool::Sqlite(_) => Err(sqlite_not_supported()),
    }
}

/// Fetch one optional analytics site row (Postgres only).
async fn pg_fetch_optional_site(
    db: &DbPool,
    sql: &str,
    binds: (&str, &str),
) -> Result<Option<AnalyticsSite>> {
    match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let row = sqlx::query_as::<_, AnalyticsSitePgRow>(sql)
                .bind(binds.0)
                .bind(binds.1)
                .fetch_optional(pg)
                .await
                .map_err(|e| {
                    kyomi_core::Error::Internal(format!("failed to query analytics site: {e}"))
                })?;
            Ok(row.map(AnalyticsSite::from))
        }
        kyomi_core::db::DbPool::Sqlite(_) => Err(sqlite_not_supported()),
    }
}

// ─── CRUD operations ────────────────────────────────────────────────────────

/// Create a new analytics site. Generates site_id and signed_key.
///
/// Also provisions a per-site ClickHouse database, user, and datasource:
/// 1. Creates a ClickHouse user + per-site database for tenant isolation
/// 2. Creates a ClickHouse datasource in the workspace
/// 3. Links the datasource to the analytics site
///
/// On failure, best-effort cleanup removes any ClickHouse objects created
/// before the failure and the orphaned PostgreSQL row so the caller can retry.
pub async fn create_site(
    db: &DbPool,
    workspace_id: &str,
    name: &str,
    domains: &[String],
    secret: &str,
    datasource_slug: Option<&str>,
    ch_host: &str,
    ch_port: u16,
    ch_admin_password: &str,
    ch_secure: bool,
) -> Result<AnalyticsSite> {
    let site_id = generate_site_id();
    let signed_key = generate_signed_key(&site_id, workspace_id, domains, secret);

    let new_id = uuid::Uuid::new_v4().to_string();

    let row: AnalyticsSite = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            let pg_row = sqlx::query_as::<_, AnalyticsSitePgRow>(
                r#"
                INSERT INTO analytics_sites (id, workspace_id, name, site_id, allowed_domains, signed_key)
                VALUES ($1, $2, $3, $4, $5, $6)
                RETURNING id, workspace_id, name, site_id,
                          allowed_domains,
                          signed_key,
                          datasource_id,
                          clickhouse_database,
                          NULL::text AS datasource_slug,
                          created_at,
                          updated_at
                "#,
            )
            .bind(&new_id)
            .bind(workspace_id)
            .bind(name)
            .bind(&site_id)
            .bind(domains)
            .bind(&signed_key)
            .fetch_one(pg)
            .await
            .map_err(|e| kyomi_core::Error::Internal(format!("failed to create analytics site: {e}")))?;
            AnalyticsSite::from(pg_row)
        }
        kyomi_core::db::DbPool::Sqlite(_) => return Err(sqlite_not_supported()),
    };

    // Pre-compute the database name so the Err arm can clean up ClickHouse objects
    // if provisioning fails partway (e.g. CREATE USER succeeded but CREATE DATABASE failed).
    let ch_database = analytics_clickhouse::database_name(&row.site_id);

    // Auto-provision ClickHouse datasource (password may be empty for default user)
    match provision_datasource(db, &row, &ch_database, datasource_slug, ch_host, ch_port, ch_admin_password, ch_secure).await {
        Ok(ds_id) => {
            // Update the site with the datasource_id and clickhouse_database
            kyomi_core::db_execute!(
                db,
                "UPDATE analytics_sites SET datasource_id = $1, clickhouse_database = $2 WHERE id = $3",
                &ds_id,
                &ch_database,
                &row.id
            )
            .map_err(|e| kyomi_core::Error::Internal(format!("failed to link datasource to site: {e}")))?;

            tracing::info!(site_id = %row.site_id, datasource_id = %ds_id, clickhouse_database = %ch_database, "Auto-provisioned analytics datasource");

            let mut site = row;
            site.datasource_id = Some(ds_id);
            site.clickhouse_database = Some(ch_database);
            Ok(site)
        }
        Err(e) => {
            tracing::error!(site_id = %row.site_id, error = %e, "Failed to auto-provision analytics datasource");
            // Best-effort: clean up any ClickHouse objects created before the failure
            if let Err(ch_err) = analytics_clickhouse::delete_site_user(
                ch_host, ch_port, ch_admin_password, &row.site_id, &ch_database, ch_secure,
            ).await {
                tracing::warn!(site_id = %row.site_id, error = %ch_err, "Failed to clean up ClickHouse objects after provisioning failure");
            }
            // Clean up the orphaned PostgreSQL row so the user can retry cleanly.
            if let Err(del_err) = kyomi_core::db_execute!(
                db,
                "DELETE FROM analytics_sites WHERE id = $1",
                &row.id
            ) {
                tracing::error!(site_id = %row.site_id, error = %del_err, "Failed to clean up orphaned analytics_sites row after provisioning failure");
            }
            Err(e)
        }
    }
}

/// Provision a ClickHouse user + per-site database + datasource for an analytics site.
/// `ch_database` is pre-computed by the caller so the Err arm can clean up ClickHouse
/// objects if this function fails partway (e.g. user created, database creation failed).
/// Returns datasource_id on success.
async fn provision_datasource(
    db: &DbPool,
    site: &AnalyticsSite,
    ch_database: &str,
    datasource_slug: Option<&str>,
    ch_host: &str,
    ch_port: u16,
    ch_admin_password: &str,
    ch_secure: bool,
) -> Result<String> {
    // 1. Create ClickHouse user + per-site database + events table + grant
    let (ch_username, ch_password) =
        analytics_clickhouse::create_site_user(ch_host, ch_port, ch_admin_password, &site.site_id, ch_database, ch_secure)
            .await?;

    // 2. Create the datasource with shared credentials pointing to per-site database
    let ds_name = format!("{} Analytics", site.name);
    let connection_config = json!({
        "host": ch_host,
        "port": ch_port,
        "database": ch_database,
        "secure": ch_secure,
        "shared_credentials": true,
        "shared_username": ch_username,
        "shared_password": ch_password,
        "analytics_site_id": site.site_id,
    });

    let ds = datasource_service::create_datasource(
        db,
        &site.workspace_id,
        &ds_name,
        datasource_slug,
        "clickhouse",
        connection_config,
        None, // direct connection
    )
    .await?;

    Ok(ds.id)
}

/// The standard SELECT columns for analytics site queries with datasource JOIN.
const SITE_SELECT_SQL: &str = r#"
    SELECT a.id, a.workspace_id, a.name, a.site_id,
           a.allowed_domains,
           a.signed_key,
           a.datasource_id,
           a.clickhouse_database,
           d.slug AS datasource_slug,
           a.created_at,
           a.updated_at
    FROM analytics_sites a
    LEFT JOIN datasource_configs d ON d.id = a.datasource_id
"#;

/// List all analytics sites for a workspace.
pub async fn list_sites(db: &DbPool, workspace_id: &str) -> Result<Vec<AnalyticsSite>> {
    let sql = format!("{SITE_SELECT_SQL} WHERE a.workspace_id = $1 ORDER BY a.created_at DESC");
    pg_fetch_all_sites(db, &sql, &[workspace_id]).await
}

/// Get a single analytics site by ID, scoped to workspace.
pub async fn get_site(
    db: &DbPool,
    id: &str,
    workspace_id: &str,
) -> Result<Option<AnalyticsSite>> {
    let sql = format!("{SITE_SELECT_SQL} WHERE a.id = $1 AND a.workspace_id = $2");
    pg_fetch_optional_site(db, &sql, (id, workspace_id)).await
}

/// Get a single analytics site by site_id (16-char hex), scoped to workspace.
pub async fn get_site_by_site_id(
    db: &DbPool,
    site_id: &str,
    workspace_id: &str,
) -> Result<Option<AnalyticsSite>> {
    let sql = format!("{SITE_SELECT_SQL} WHERE a.site_id = $1 AND a.workspace_id = $2");
    pg_fetch_optional_site(db, &sql, (site_id, workspace_id)).await
}

/// Update a site's name, domains, and/or datasource slug. Regenerates signed_key when domains change.
pub async fn update_site(
    db: &DbPool,
    id: &str,
    workspace_id: &str,
    name: Option<&str>,
    domains: Option<&[String]>,
    secret: &str,
    datasource_slug: Option<&str>,
) -> Result<AnalyticsSite> {
    // Fetch existing site to get current values for key regeneration
    let existing = get_site(db, id, workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound(format!("Analytics site {id} not found")))?;

    // Determine new values
    let new_name = name.unwrap_or(&existing.name);
    let new_domains = domains.unwrap_or(&existing.allowed_domains);

    // Regenerate signed key if domains changed
    let new_signed_key = if domains.is_some() {
        generate_signed_key(&existing.site_id, workspace_id, new_domains, secret)
    } else {
        existing.signed_key.clone()
    };

    match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            sqlx::query(
                "UPDATE analytics_sites \
                 SET name = $3, allowed_domains = $4, signed_key = $5, updated_at = now() \
                 WHERE id = $1 AND workspace_id = $2",
            )
            .bind(id)
            .bind(workspace_id)
            .bind(new_name)
            .bind(new_domains)
            .bind(&new_signed_key)
            .execute(pg)
            .await
            .map_err(|e| kyomi_core::Error::Internal(format!("failed to update analytics site: {e}")))?;
        }
        kyomi_core::db::DbPool::Sqlite(_) => return Err(sqlite_not_supported()),
    }

    // Update datasource slug if requested and a datasource is linked
    if let Some(new_slug) = datasource_slug
        && let Some(ref ds_id) = existing.datasource_id
    {
        datasource_service::update_datasource(
            db,
            ds_id,
            workspace_id,
            None,
            Some(new_slug),
            None,
            None,
            None,
        )
        .await?;
    }

    // Re-fetch with JOIN to get datasource_slug
    let row = get_site(db, id, workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Internal("Site disappeared after update".into()))?;

    tracing::info!(site_id = %row.site_id, "Updated analytics site");
    Ok(row)
}

/// Delete a site by UUID, scoped to workspace.
///
/// Also tears down the auto-provisioned ClickHouse database, user, and datasource.
pub async fn delete_site(
    db: &DbPool,
    id: &str,
    workspace_id: &str,
    ch_host: &str,
    ch_port: u16,
    ch_admin_password: &str,
    ch_secure: bool,
) -> Result<()> {
    // Fetch site first to get site_id and datasource_id for cleanup
    let site = get_site(db, id, workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound(format!("Analytics site {id} not found")))?;

    // Delete the site row first (FK ON DELETE SET NULL handles datasource_id)
    kyomi_core::db_execute!(
        db,
        "DELETE FROM analytics_sites WHERE id = $1 AND workspace_id = $2",
        id,
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete analytics site: {e}")))?;

    // Clean up ClickHouse database + user (best-effort)
    if let Some(ref ch_db) = site.clickhouse_database
        && let Err(e) =
            analytics_clickhouse::delete_site_user(ch_host, ch_port, ch_admin_password, &site.site_id, ch_db, ch_secure)
                .await
    {
        tracing::error!(site_id = %site.site_id, error = %e, "Failed to delete ClickHouse database/user");
    }

    // Clean up the auto-provisioned datasource
    if let Some(ds_id) = &site.datasource_id
        && let Err(e) = kyomi_core::db_execute!(
            db,
            "DELETE FROM datasource_configs WHERE id = $1",
            ds_id
        )
    {
        tracing::error!(datasource_id = %ds_id, error = %e, "Failed to delete auto-provisioned datasource");
    }

    tracing::info!(site_id = %site.site_id, "Deleted analytics site");
    Ok(())
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &str = "test-hmac-secret-for-unit-tests";

    #[test]
    fn test_generate_site_id() {
        let id = generate_site_id();
        assert_eq!(id.len(), 16, "site_id should be 16 chars");
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "site_id should be all hex chars, got: {id}"
        );
    }

    #[test]
    fn test_generate_and_verify_key() {
        let site_id = "abcd1234";
        let workspace_id = "ws_test123";
        let domains = vec!["example.com".to_string(), "app.example.com".to_string()];

        let key = generate_signed_key(site_id, workspace_id, &domains, TEST_SECRET);

        // Key should contain exactly one dot separator
        assert_eq!(
            key.matches('.').count(),
            1,
            "key should have exactly one dot separator"
        );

        // Verify roundtrip
        let payload = verify_signed_key(&key, TEST_SECRET);
        assert!(payload.is_some(), "valid key should verify successfully");

        let payload = payload.unwrap();
        assert_eq!(payload.s, site_id);
        assert_eq!(payload.w, workspace_id);
        assert_eq!(payload.d, domains);
    }

    #[test]
    fn test_verify_tampered_payload() {
        let key = generate_signed_key("abcd1234", "ws_test", &vec!["a.com".into()], TEST_SECRET);

        // Tamper with the payload portion (before the dot)
        let dot_pos = key.find('.').unwrap();
        let sig = &key[dot_pos..];
        // Replace first char of payload to tamper it
        let tampered_payload = format!("X{}{sig}", &key[1..dot_pos]);

        let result = verify_signed_key(&tampered_payload, TEST_SECRET);
        assert!(result.is_none(), "tampered payload should fail verification");
    }

    #[test]
    fn test_verify_wrong_secret() {
        let key = generate_signed_key("abcd1234", "ws_test", &vec!["a.com".into()], TEST_SECRET);

        let result = verify_signed_key(&key, "wrong-secret");
        assert!(
            result.is_none(),
            "key signed with different secret should fail verification"
        );
    }

    #[test]
    fn test_verify_malformed_key() {
        // No dot separator
        let result = verify_signed_key("nodothere", TEST_SECRET);
        assert!(result.is_none(), "key without dot should fail verification");
    }

    #[test]
    fn test_verify_empty_key() {
        let result = verify_signed_key("", TEST_SECRET);
        assert!(result.is_none(), "empty key should fail verification");
    }
}
