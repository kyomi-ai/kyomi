// SPDX-License-Identifier: AGPL-3.0-or-later

//! ClickHouse admin client for analytics tenant isolation.
//!
//! Creates per-site ClickHouse databases and users so each analytics site
//! gets structural isolation — no row policies needed.
//!
//! Each site gets a database named `site_{site_id}`.
//! The per-site ClickHouse user gets `SELECT` on their database only.
//! The collector writes to per-site databases using the `default` admin user
//! (configured via `ANALYTICS_CLICKHOUSE_USER`). It routes events to the correct
//! database via the pre-computed `database` field in each `BatchEntry`.
//!
//! Uses the ClickHouse HTTP API with the `default` admin user — same
//! approach as our existing ClickHouse provider.

use rand::Rng;
use reqwest::Client as HttpClient;

// ─── Database name generation ─────────────────────────────────────────────────

/// Generate the ClickHouse database name for a site.
/// Format: `site_{site_id}`
///
/// NOTE: The collector (`apps/analytics-collector/src/collector.rs`) has an
/// independent copy of this function — it cannot import from kyomi-auth.
/// **This copy is authoritative.** If the formula changes, update both and
/// keep their unit tests in sync.
pub fn database_name(site_id: &str) -> String {
    assert!(
        !site_id.is_empty() && site_id.chars().all(|c| c.is_ascii_hexdigit()),
        "site_id must be non-empty hex digits"
    );
    format!("site_{site_id}")
}

// ─── DDL generation ──────────────────────────────────────────────────────────

fn ch_username(site_id: &str) -> String {
    assert!(
        !site_id.is_empty() && site_id.chars().all(|c| c.is_ascii_hexdigit()),
        "site_id must be non-empty hex digits"
    );
    format!("kyomi_site_{site_id}")
}

/// Generate a random 32-byte hex password for a ClickHouse user.
pub fn generate_ch_password() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    hex::encode(bytes)
}

fn create_user_ddl(site_id: &str, password: &str) -> String {
    assert!(
        !site_id.is_empty() && site_id.chars().all(|c| c.is_ascii_hexdigit()),
        "site_id must be non-empty hex digits"
    );
    assert!(
        !password.is_empty() && password.chars().all(|c| c.is_ascii_hexdigit()),
        "password must be non-empty hex digits"
    );
    let user = ch_username(site_id);
    format!("CREATE USER IF NOT EXISTS {user} IDENTIFIED BY '{password}'")
}

/// Generate DDL to create the per-site ClickHouse database.
pub fn create_database_ddl(database: &str) -> String {
    assert!(
        database.starts_with("site_") && database[5..].chars().all(|c| c.is_ascii_hexdigit()) && database.len() > 5,
        "database must match site_{{hex}}"
    );
    format!("CREATE DATABASE IF NOT EXISTS {database}")
}

/// Generate DDL to create the `events` table inside the per-site database.
pub fn create_events_table_ddl(database: &str) -> String {
    assert!(
        database.starts_with("site_") && database[5..].chars().all(|c| c.is_ascii_hexdigit()) && database.len() > 5,
        "database must match site_{{hex}}"
    );
    format!(
        "CREATE TABLE IF NOT EXISTS {database}.events (\
            visitor_id String, \
            session_id String, \
            user_id String DEFAULT '', \
            timestamp DateTime64(3), \
            event_name LowCardinality(String), \
            hostname LowCardinality(String), \
            pathname String, \
            referrer String, \
            referrer_source LowCardinality(String), \
            utm_source LowCardinality(String), \
            utm_medium LowCardinality(String), \
            utm_campaign LowCardinality(String), \
            utm_term String, \
            utm_content String, \
            country_code LowCardinality(String), \
            region LowCardinality(String), \
            city String, \
            browser LowCardinality(String), \
            browser_version LowCardinality(String), \
            os LowCardinality(String), \
            os_version LowCardinality(String), \
            device_type LowCardinality(String), \
            screen_width UInt16, \
            screen_height UInt16, \
            properties String DEFAULT '{{}}'\
        ) ENGINE = MergeTree() \
        PARTITION BY toYYYYMM(timestamp) \
        ORDER BY (toDate(timestamp), event_name, visitor_id) \
        TTL toDateTime(timestamp) + INTERVAL 2 YEAR"
    )
}

/// Generate DDL to grant SELECT on the per-site database to a ClickHouse user.
pub fn grant_select_ddl(username: &str, database: &str) -> String {
    assert!(
        username.starts_with("kyomi_site_") && username[11..].chars().all(|c| c.is_ascii_hexdigit()) && username.len() > 11,
        "username must match kyomi_site_{{hex}}"
    );
    assert!(
        database.starts_with("site_") && database[5..].chars().all(|c| c.is_ascii_hexdigit()) && database.len() > 5,
        "database must match site_{{hex}}"
    );
    format!("GRANT SELECT ON {database}.* TO {username}")
}

/// Generate DDL to drop the per-site ClickHouse database (removes all tables).
pub fn drop_database_ddl(database: &str) -> String {
    assert!(
        database.starts_with("site_") && database[5..].chars().all(|c| c.is_ascii_hexdigit()) && database.len() > 5,
        "database must match site_{{hex}}"
    );
    format!("DROP DATABASE IF EXISTS {database}")
}

fn drop_user_ddl(site_id: &str) -> String {
    assert!(
        !site_id.is_empty() && site_id.chars().all(|c| c.is_ascii_hexdigit()),
        "site_id must be non-empty hex digits"
    );
    let user = ch_username(site_id);
    format!("DROP USER IF EXISTS {user}")
}

// ─── HTTP execution ──────────────────────────────────────────────────────────

/// Execute a DDL statement against the analytics ClickHouse as the admin user.
async fn execute_ch_ddl(
    http: &HttpClient,
    host: &str,
    port: u16,
    admin_password: &str,
    sql: &str,
    secure: bool,
) -> kyomi_core::Result<()> {
    let scheme = if secure { "https" } else { "http" };
    let url = format!("{scheme}://{host}:{port}/");
    let resp = http
        .post(&url)
        .header("X-ClickHouse-User", "default")
        .header("X-ClickHouse-Key", admin_password)
        .body(sql.to_string())
        .send()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("ClickHouse DDL failed: {e}")))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "ClickHouse DDL error: {body}"
        )));
    }
    Ok(())
}

/// Execute a query and return the response body as text.
async fn query_ch(
    http: &HttpClient,
    host: &str,
    port: u16,
    admin_password: &str,
    sql: &str,
    secure: bool,
) -> kyomi_core::Result<String> {
    let scheme = if secure { "https" } else { "http" };
    let url = format!("{scheme}://{host}:{port}/");
    let resp = http
        .post(&url)
        .header("X-ClickHouse-User", "default")
        .header("X-ClickHouse-Key", admin_password)
        .body(sql.to_string())
        .send()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("ClickHouse query failed: {e}")))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "ClickHouse query error: {body}"
        )));
    }
    resp.text()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("ClickHouse response read error: {e}")))
}

// ─── Properties column migration (Map → String) ─────────────────────────────

/// Migrate the `properties` column from `Map(String, String)` to `String` (JSON)
/// for a single site database. Idempotent — skips if already migrated.
///
/// Steps:
/// 1. Check current column type via system.columns
/// 2. If already String, do nothing
/// 3. Add temporary `properties_v2 String DEFAULT '{}'`
/// 4. Populate via synchronous mutation: `toJSONString(properties)`
/// 5. Drop old `properties` column
/// 6. Rename `properties_v2` to `properties`
async fn migrate_properties_column(
    http: &HttpClient,
    host: &str,
    port: u16,
    admin_password: &str,
    database: &str,
    secure: bool,
) -> kyomi_core::Result<()> {
    // Check current column type
    let type_query = format!(
        "SELECT type FROM system.columns WHERE database = '{database}' AND table = 'events' AND name = 'properties'"
    );
    let col_type = query_ch(http, host, port, admin_password, &type_query, secure).await?;
    let col_type = col_type.trim();

    if col_type.is_empty() {
        // No properties column at all — table might not exist yet, skip
        return Ok(());
    }

    if col_type == "String" {
        // Already migrated
        return Ok(());
    }

    tracing::info!(database = %database, column_type = %col_type, "Migrating properties column from Map to String");

    // Step 1: Add temporary column
    execute_ch_ddl(
        http, host, port, admin_password,
        &format!("ALTER TABLE {database}.events ADD COLUMN IF NOT EXISTS properties_v2 String DEFAULT '{{}}'"),
        secure,
    ).await?;

    // Step 2: Populate from old Map column (synchronous mutation)
    execute_ch_ddl(
        http, host, port, admin_password,
        &format!(
            "ALTER TABLE {database}.events UPDATE properties_v2 = toJSONString(properties) WHERE 1 SETTINGS mutations_sync = 2"
        ),
        secure,
    ).await?;

    // Step 3: Drop old column
    execute_ch_ddl(
        http, host, port, admin_password,
        &format!("ALTER TABLE {database}.events DROP COLUMN IF EXISTS properties"),
        secure,
    ).await?;

    // Step 4: Rename new column
    execute_ch_ddl(
        http, host, port, admin_password,
        &format!("ALTER TABLE {database}.events RENAME COLUMN properties_v2 TO properties"),
        secure,
    ).await?;

    tracing::info!(database = %database, "Properties column migration complete");
    Ok(())
}

/// Migrate all analytics site databases from Map(String, String) properties to String (JSON).
/// Queries PostgreSQL for all analytics sites, then migrates each ClickHouse database.
/// Idempotent — safe to run on every startup.
pub async fn migrate_all_properties_columns(
    pg: &sqlx::PgPool,
    ch_host: &str,
    ch_port: u16,
    ch_admin_password: &str,
    ch_secure: bool,
) -> kyomi_core::Result<()> {
    let sites = sqlx::query_scalar::<_, String>(
        "SELECT clickhouse_database FROM analytics_sites WHERE clickhouse_database IS NOT NULL"
    )
    .fetch_all(pg)
    .await
    .map_err(|e| kyomi_core::Error::Internal(format!("Failed to list analytics sites: {e}")))?;

    if sites.is_empty() {
        return Ok(());
    }

    let http = HttpClient::new();
    let mut migrated = 0u32;

    for database in &sites {
        match migrate_properties_column(&http, ch_host, ch_port, ch_admin_password, database, ch_secure).await {
            Ok(()) => migrated += 1,
            Err(e) => {
                tracing::warn!(database = %database, error = %e, "Failed to migrate properties column — will retry on next startup");
            }
        }
    }

    tracing::info!(total = sites.len(), migrated = migrated, "Analytics properties column migration check complete");
    Ok(())
}

/// Create a ClickHouse user, per-site database, events table, and grant SELECT.
/// Returns (username, password).
pub async fn create_site_user(
    host: &str,
    port: u16,
    admin_password: &str,
    site_id: &str,
    database: &str,
    secure: bool,
) -> kyomi_core::Result<(String, String)> {
    let http = HttpClient::new();
    let password = generate_ch_password();
    let username = ch_username(site_id);

    execute_ch_ddl(&http, host, port, admin_password, &create_user_ddl(site_id, &password), secure).await?;
    execute_ch_ddl(&http, host, port, admin_password, &create_database_ddl(database), secure).await?;
    execute_ch_ddl(&http, host, port, admin_password, &create_events_table_ddl(database), secure).await?;
    execute_ch_ddl(&http, host, port, admin_password, &grant_select_ddl(&username, database), secure).await?;

    Ok((username, password))
}

/// Drop the per-site ClickHouse database and user for the given site_id.
pub async fn delete_site_user(
    host: &str,
    port: u16,
    admin_password: &str,
    site_id: &str,
    database: &str,
    secure: bool,
) -> kyomi_core::Result<()> {
    let http = HttpClient::new();
    // Drop database first (removes all tables), then drop user
    execute_ch_ddl(&http, host, port, admin_password, &drop_database_ddl(database), secure).await?;
    execute_ch_ddl(&http, host, port, admin_password, &drop_user_ddl(site_id), secure).await?;
    Ok(())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── database_name tests ──

    #[test]
    fn test_database_name() {
        assert_eq!(database_name("deadbeef01234567"), "site_deadbeef01234567");
    }

    // ── DDL generation tests ──

    #[test]
    fn test_ch_username_format() {
        assert_eq!(ch_username("abc123def456"), "kyomi_site_abc123def456");
    }

    #[test]
    fn test_generate_ch_password_length() {
        let pw = generate_ch_password();
        assert_eq!(pw.len(), 64); // 32 bytes hex-encoded
    }

    #[test]
    fn test_create_user_sql() {
        let sql = create_user_ddl("abc123def456", "deadbeef01234567");
        assert!(sql.contains("CREATE USER IF NOT EXISTS kyomi_site_abc123def456"));
        assert!(sql.contains("IDENTIFIED BY 'deadbeef01234567'"));
    }

    #[test]
    fn test_create_database_ddl() {
        let ddl = create_database_ddl("site_deadbeef01234567");
        assert!(ddl.contains("CREATE DATABASE IF NOT EXISTS site_deadbeef01234567"));
    }

    #[test]
    fn test_create_events_table_ddl_no_site_id() {
        let ddl = create_events_table_ddl("site_deadbeef01234567");
        assert!(ddl.contains("site_deadbeef01234567.events"));
        assert!(!ddl.contains("site_id")); // No site_id column
        assert!(ddl.contains("visitor_id"));
        assert!(ddl.contains("MergeTree"));
        // Properties column stores JSON as a String (not Map)
        assert!(ddl.contains("properties String DEFAULT '{}'"));
    }

    #[test]
    fn test_grant_ddl_scoped_to_database() {
        let ddl = grant_select_ddl("kyomi_site_abc456def012", "site_deadbeef01234567");
        assert!(ddl.contains("GRANT SELECT ON site_deadbeef01234567.*"));
        assert!(ddl.contains("TO kyomi_site_abc456def012"));
    }

    #[test]
    fn test_drop_database_ddl() {
        let ddl = drop_database_ddl("site_deadbeef01234567");
        assert!(ddl.contains("DROP DATABASE IF EXISTS site_deadbeef01234567"));
    }

    #[test]
    fn test_drop_user_sql() {
        let sql = drop_user_ddl("abc123def456");
        assert!(sql.contains("DROP USER IF EXISTS kyomi_site_abc123def456"));
    }

    // ── Validation rejection tests ──

    #[test]
    #[should_panic(expected = "site_id must be non-empty hex digits")]
    fn test_create_user_ddl_rejects_non_hex_site_id() {
        create_user_ddl("abc; DROP TABLE--", "deadbeef01234567");
    }

    #[test]
    #[should_panic(expected = "password must be non-empty hex digits")]
    fn test_create_user_ddl_rejects_non_hex_password() {
        create_user_ddl("deadbeef01234567", "s3cret!");
    }

    #[test]
    #[should_panic(expected = "site_id must be non-empty hex digits")]
    fn test_create_user_ddl_rejects_empty_site_id() {
        create_user_ddl("", "deadbeef01234567");
    }

    #[test]
    #[should_panic(expected = "database must match site_{hex}")]
    fn test_create_database_ddl_rejects_invalid_format() {
        create_database_ddl("site_ws123_abc456");
    }

    #[test]
    #[should_panic(expected = "database must match site_{hex}")]
    fn test_create_database_ddl_rejects_missing_prefix() {
        create_database_ddl("deadbeef01234567");
    }

    #[test]
    #[should_panic(expected = "database must match site_{hex}")]
    fn test_drop_database_ddl_rejects_invalid_format() {
        drop_database_ddl("site_ws123_abc456");
    }

    #[test]
    #[should_panic(expected = "username must match kyomi_site_{hex}")]
    fn test_grant_select_ddl_rejects_invalid_username() {
        grant_select_ddl("kyomi_site_bad!user", "site_deadbeef01234567");
    }

    #[test]
    #[should_panic(expected = "database must match site_{hex}")]
    fn test_grant_select_ddl_rejects_invalid_database() {
        grant_select_ddl("kyomi_site_deadbeef01234567", "site_bad_db!");
    }
}
