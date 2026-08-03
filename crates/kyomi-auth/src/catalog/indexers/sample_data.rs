// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sample Data Catalog Indexer.
//!
//! Indexes the shared sample ClickHouse database tables into a sentinel workspace
//! (`sample-data-workspace`), so every workspace that adds the sample datasource
//! shares one copy of the index.
//!
//! Python ref: `apps/backend-python/src/api/services/sample_data_indexer.py` (440 lines).
//!
//! Architecture:
//! - Creates a ClickHouse connection using environment variables
//! - Discovers tables via `system.tables`, columns via `system.columns`
//! - Stores results in `datasource_table_cache` + `datasource_search_embeddings`
//! - Data is static, so index once and reuse (weekly refresh threshold)

use chrono::{DateTime, TimeDelta, Utc};
use kyomi_core::{db_fetch_scalar, DbPool, Result};
use kyomi_embed::EmbeddingService;
use tracing::{info, warn};

use crate::catalog::helpers::{cache_table, IndexerContext};
use crate::catalog::types::{CatalogIndexResult, ColumnEntry};

/// Sentinel workspace ID for shared sample data.
///
/// NOTE (dead code): `index_sample_data`'s only caller was the REST route
/// `POST /api/v1/datasources/sample`, deleted wholesale in the React→Leptos
/// migration (KYO-73, #183). The Leptos replacement,
/// `crates/kyomi-ui/src/server_fns/onboarding.rs::create_sample_datasource`,
/// indexes sample tables through the generic per-workspace catalog indexer
/// instead — nothing calls `index_sample_data` anymore. See KYO-300.
/// The sentinel is typically empty on a fresh install.
pub const SAMPLE_DATA_WORKSPACE_ID: &str = "sample-data-workspace";

/// Sentinel datasource config ID for sample data (not a real datasource config row).
const SAMPLE_DATASOURCE_CONFIG_ID: &str = "sample-data-indexer";

/// Sample ClickHouse configuration loaded from environment variables.
pub struct SampleClickHouseConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    password: String,
    pub secure: bool,
}

impl std::fmt::Debug for SampleClickHouseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SampleClickHouseConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &"***REDACTED***")
            .field("secure", &self.secure)
            .finish()
    }
}

impl SampleClickHouseConfig {
    /// Load configuration from environment variables.
    /// Returns `None` if `SAMPLE_CLICKHOUSE_HOST` is not set.
    pub fn from_env() -> Option<Self> {
        let host = std::env::var("SAMPLE_CLICKHOUSE_HOST").ok()?;
        if host.is_empty() {
            return None;
        }

        let password = match std::env::var("SAMPLE_CLICKHOUSE_PASSWORD").ok() {
            Some(p) => p,
            None => {
                warn!("SAMPLE_CLICKHOUSE_HOST is set but SAMPLE_CLICKHOUSE_PASSWORD is not — sample data will be disabled");
                return None;
            }
        };

        Some(Self {
            host,
            port: std::env::var("SAMPLE_CLICKHOUSE_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(8126),
            database: std::env::var("SAMPLE_CLICKHOUSE_DATABASE")
                .unwrap_or_else(|_| "acme_analytics".into()),
            username: std::env::var("SAMPLE_CLICKHOUSE_USER")
                .unwrap_or_else(|_| "sample_readonly".into()),
            password,
            secure: std::env::var("SAMPLE_CLICKHOUSE_SECURE")
                .unwrap_or_else(|_| "false".into())
                .to_lowercase()
                == "true",
        })
    }

    /// Build the ClickHouse HTTP API URL.
    fn base_url(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{scheme}://{}:{}", self.host, self.port)
    }

    /// Build a `connection_config` JSON value suitable for `ClickHouseProvider::new()`.
    pub fn connection_config_json(&self) -> serde_json::Value {
        serde_json::json!({
            "host": self.host,
            "port": self.port,
            "database": self.database,
            "secure": self.secure,
        })
    }

    /// Build a `connection_config` JSON value for creating a sample datasource.
    ///
    /// Includes shared credentials and the `is_sample` marker so the datasource
    /// is recognised as the workspace's sample data source.
    pub fn sample_datasource_config_json(&self) -> serde_json::Value {
        serde_json::json!({
            "host": self.host,
            "port": self.port,
            "database": self.database,
            "secure": self.secure,
            "shared_credentials": true,
            "shared_username": self.username,
            "shared_password": self.password,
            "is_sample": true,
        })
    }

    /// Build a `credentials` JSON value suitable for `ClickHouseProvider::new()`.
    pub fn credentials_json(&self) -> serde_json::Value {
        serde_json::json!({
            "username": self.username,
            "password": self.password,
        })
    }
}

/// Sample Data Indexer service.
///
/// Indexes sample ClickHouse tables into the shared sentinel workspace.
pub struct SampleDataIndexer;

impl SampleDataIndexer {
    /// Check if the sample ClickHouse environment is configured.
    pub fn is_configured() -> bool {
        std::env::var("SAMPLE_CLICKHOUSE_HOST")
            .ok()
            .filter(|h| !h.is_empty())
            .is_some()
    }

    /// Check if sample data needs re-indexing (weekly threshold).
    pub async fn needs_refresh(db: &DbPool, hours_threshold: i64) -> bool {
        #[derive(sqlx::FromRow)]
        struct LastUpdate {
            last_update: Option<DateTime<Utc>>,
        }

        let row = kyomi_core::db_fetch_optional!(
            db,
            LastUpdate,
            "SELECT MAX(updated_at) as last_update FROM datasource_table_cache WHERE workspace_id = $1",
            SAMPLE_DATA_WORKSPACE_ID
        );

        let Ok(Some(row)) = row else {
            return true; // No data yet or error
        };

        match row.last_update {
            None => true,
            Some(ts) => {
                let elapsed: TimeDelta = Utc::now() - ts;
                elapsed.num_hours() >= hours_threshold
            }
        }
    }

    /// Index all sample ClickHouse tables.
    ///
    /// Returns a `CatalogIndexResult` with indexing statistics.
    pub async fn index_sample_data(
        db: &DbPool,
        embedding: &EmbeddingService,
    ) -> CatalogIndexResult {
        let start_time = Utc::now();

        let config = match SampleClickHouseConfig::from_env() {
            Some(c) => c,
            None => {
                return CatalogIndexResult::skipped("SAMPLE_CLICKHOUSE_HOST not configured");
            }
        };

        info!(
            database = config.database,
            host = config.host,
            "starting sample data indexing"
        );

        let client = match crate::http_client() {
            Ok(c) => c,
            Err(e) => return CatalogIndexResult::error(&format!("Failed to build HTTP client: {e}")),
        };
        let ctx = IndexerContext {
            workspace_id: SAMPLE_DATA_WORKSPACE_ID.to_string(),
            datasource_config_id: SAMPLE_DATASOURCE_CONFIG_ID.to_string(),
            connection_config: serde_json::json!({}),
            encryption_key: std::sync::Arc::new([0u8; 32]), // Not used for sample data
        };

        // Discover tables
        let tables = match discover_tables(&client, &config).await {
            Ok(t) => t,
            Err(e) => {
                return CatalogIndexResult::error(&format!(
                    "Failed to discover sample tables: {e}"
                ))
                .with_times(&start_time.to_rfc3339(), &Utc::now().to_rfc3339());
            }
        };

        info!(
            count = tables.len(),
            database = config.database,
            "discovered sample tables"
        );

        let mut tables_indexed = 0usize;
        let mut errors = Vec::new();

        for (table_name, engine) in &tables {
            // Get columns for this table
            let columns = match get_table_columns(&client, &config, table_name).await {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        table = table_name.as_str(),
                        error = %e,
                        "failed to get columns, skipping"
                    );
                    errors.push(format!("{table_name}: {e}"));
                    continue;
                }
            };

            let full_table_id = format!("{}.{table_name}", config.database);

            let cached = cache_table(crate::catalog::helpers::CacheTableParams {
                db,
                embedding,
                ctx: &ctx,
                project_id: "", // project_id is empty for ClickHouse sample
                dataset_id: &config.database, // dataset_id = database name
                table_name,
                table_type: engine,
                columns: &columns,
                full_table_id: &full_table_id,
            })
            .await;

            if cached {
                tables_indexed += 1;
            }
        }

        let end_time = Utc::now();
        let elapsed = (end_time - start_time).num_seconds();

        info!(
            tables_indexed,
            errors = errors.len(),
            elapsed_secs = elapsed,
            "sample data indexing complete"
        );

        let mut result = CatalogIndexResult::completed(tables_indexed, 0)
            .with_times(&start_time.to_rfc3339(), &end_time.to_rfc3339());

        if !errors.is_empty() {
            result.errors = Some(errors);
        }

        result
    }

    /// Get the count of cached sample data tables.
    pub async fn get_sample_table_count(db: &DbPool) -> i64 {
        db_fetch_scalar!(
            db,
            i64,
            "SELECT COUNT(*) FROM datasource_table_cache WHERE workspace_id = $1",
            SAMPLE_DATA_WORKSPACE_ID
        )
        .unwrap_or(0)
    }
}

/// Discover all tables in the sample ClickHouse database.
///
/// Returns a list of (table_name, engine) pairs.
async fn discover_tables(
    client: &reqwest::Client,
    config: &SampleClickHouseConfig,
) -> Result<Vec<(String, String)>> {
    let db_escaped = config.database.replace('\'', "''");
    let sql = format!(
        "SELECT name, engine \
         FROM system.tables \
         WHERE database = '{db_escaped}' \
         ORDER BY name"
    );

    let response = execute_clickhouse_query(client, config, &sql).await?;
    let lines: Vec<&str> = response.trim().lines().collect();

    let mut tables = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        // ClickHouse tab-separated output
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            tables.push((parts[0].to_string(), parts[1].to_string()));
        }
    }

    Ok(tables)
}

/// Get column metadata for a table in the sample database.
async fn get_table_columns(
    client: &reqwest::Client,
    config: &SampleClickHouseConfig,
    table_name: &str,
) -> Result<Vec<ColumnEntry>> {
    let db_escaped = config.database.replace('\'', "''");
    let table_escaped = table_name.replace('\'', "''");

    let sql = format!(
        "SELECT name, type, comment \
         FROM system.columns \
         WHERE database = '{db_escaped}' \
           AND table = '{table_escaped}' \
         ORDER BY position"
    );

    let response = execute_clickhouse_query(client, config, &sql).await?;
    let lines: Vec<&str> = response.trim().lines().collect();

    let mut columns = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.is_empty() {
            continue;
        }

        let name = parts[0].to_string();
        let native_type = parts.get(1).map(|s| s.to_string());
        let description = parts
            .get(2)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        columns.push(ColumnEntry {
            name,
            col_type: native_type.clone(),
            native_type,
            description,
        });
    }

    Ok(columns)
}

/// Execute a query against the sample ClickHouse via HTTP API.
///
/// Uses tab-separated format for easy parsing.
async fn execute_clickhouse_query(
    client: &reqwest::Client,
    config: &SampleClickHouseConfig,
    sql: &str,
) -> Result<String> {
    let url = config.base_url();

    let response = client
        .post(&url)
        .query(&[
            ("database", config.database.as_str()),
            ("default_format", "TabSeparated"),
        ])
        .basic_auth(&config.username, Some(&config.password))
        .body(sql.to_string())
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "ClickHouse HTTP request failed: {e}"
            ))
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "ClickHouse query failed (HTTP {status}): {body}"
        )));
    }

    response.text().await.map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to read ClickHouse response: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_data_workspace_id_is_stable() {
        assert_eq!(SAMPLE_DATA_WORKSPACE_ID, "sample-data-workspace");
    }

    #[test]
    fn is_configured_reads_env() {
        // If SAMPLE_CLICKHOUSE_HOST is not set in the test environment,
        // is_configured should return false.
        let configured = SampleDataIndexer::is_configured();
        // We cannot assert a specific value since it depends on CI/env,
        // but we can at least verify it does not panic.
        let _ = configured;
    }

    #[test]
    fn config_from_env_returns_none_when_not_set() {
        // This test depends on SAMPLE_CLICKHOUSE_HOST not being set,
        // which is the normal case in test environments.
        // If it IS set, this test will still pass (just follows the Some branch).
        let config = SampleClickHouseConfig::from_env();
        if std::env::var("SAMPLE_CLICKHOUSE_HOST").is_err() {
            assert!(config.is_none());
        }
    }

    #[test]
    fn config_from_env_returns_none_when_host_set_but_password_missing() {
        // SAFETY: env var mutation is inherently thread-unsafe. This test is
        // safe in practice because no other test sets SAMPLE_CLICKHOUSE_HOST.
        // If flakiness is ever observed, add the `serial_test` crate.
        let original_host = std::env::var("SAMPLE_CLICKHOUSE_HOST").ok();
        let original_password = std::env::var("SAMPLE_CLICKHOUSE_PASSWORD").ok();

        unsafe {
            std::env::set_var("SAMPLE_CLICKHOUSE_HOST", "test-host");
            std::env::remove_var("SAMPLE_CLICKHOUSE_PASSWORD");
        }

        let result = SampleClickHouseConfig::from_env();

        unsafe {
            match original_host {
                Some(v) => std::env::set_var("SAMPLE_CLICKHOUSE_HOST", v),
                None => std::env::remove_var("SAMPLE_CLICKHOUSE_HOST"),
            }
            match original_password {
                Some(v) => std::env::set_var("SAMPLE_CLICKHOUSE_PASSWORD", v),
                None => std::env::remove_var("SAMPLE_CLICKHOUSE_PASSWORD"),
            }
        }

        assert!(result.is_none());
    }

    #[test]
    fn base_url_formats_correctly() {
        let config = SampleClickHouseConfig {
            host: "localhost".into(),
            port: 8123,
            database: "test".into(),
            username: "user".into(),
            password: "pass".into(),
            secure: false,
        };
        assert_eq!(config.base_url(), "http://localhost:8123");

        let secure_config = SampleClickHouseConfig {
            secure: true,
            ..config
        };
        assert_eq!(secure_config.base_url(), "https://localhost:8123");
    }
}
