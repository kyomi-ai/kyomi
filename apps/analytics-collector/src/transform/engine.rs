use tracing::{error, info, warn};

use crate::transform::definition::{load_transforms_from_dir, TransformDef};

/// ClickHouse HTTP API connection info.
#[derive(Clone)]
pub struct ChHttpConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

impl ChHttpConfig {
    pub fn from_env() -> Self {
        Self {
            host: std::env::var("ANALYTICS_CLICKHOUSE_HOST").unwrap_or_else(|_| "localhost".into()),
            port: std::env::var("ANALYTICS_CLICKHOUSE_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8126),
            user: std::env::var("ANALYTICS_CLICKHOUSE_USER").unwrap_or_else(|_| "default".into()),
            password: std::env::var("ANALYTICS_CLICKHOUSE_PASSWORD").unwrap_or_default(),
        }
    }

    fn url(&self) -> String {
        let user = url::form_urlencoded::byte_serialize(self.user.as_bytes()).collect::<String>();
        let password = url::form_urlencoded::byte_serialize(self.password.as_bytes()).collect::<String>();
        format!(
            "http://{}:{}/?user={}&password={}",
            self.host, self.port, user, password
        )
    }
}

/// Execute a DDL statement against ClickHouse.
pub async fn execute_ddl(http: &reqwest::Client, config: &ChHttpConfig, sql: &str) -> Result<(), String> {
    let resp = http
        .post(&config.url())
        .body(sql.to_string())
        .send()
        .await
        .map_err(|e| format!("ClickHouse DDL request failed: {e}"))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("ClickHouse DDL error: {body}"));
    }
    Ok(())
}

/// Query ClickHouse and return the response body as a string.
pub async fn execute_query(http: &reqwest::Client, config: &ChHttpConfig, sql: &str) -> Result<String, String> {
    let resp = http
        .post(&config.url())
        .body(sql.to_string())
        .send()
        .await
        .map_err(|e| format!("ClickHouse query failed: {e}"))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("ClickHouse query error: {body}"));
    }
    resp.text().await.map_err(|e| format!("Failed to read response: {e}"))
}

/// The transform engine manages transform definitions and schema lifecycle.
/// With the MV-based architecture, the engine only handles schema management —
/// ClickHouse Materialized Views aggregate data automatically on INSERT.
pub struct TransformEngine {
    definitions: Vec<TransformDef>,
    http: reqwest::Client,
    /// Per-pod performance cache to avoid DDL round-trips on every event.
    /// NOT shared across replicas — each pod independently warms this set.
    /// All DDL uses IF NOT EXISTS / CREATE OR REPLACE, so redundant DDL from
    /// multiple replicas is safe (idempotent). This is purely an optimization.
    initialized_databases: tokio::sync::Mutex<std::collections::HashSet<String>>,
}

impl TransformEngine {
    /// Load transform definitions from YAML files.
    pub fn new(transforms_dir: &std::path::Path) -> Result<Self, String> {
        let definitions = load_transforms_from_dir(transforms_dir)?;
        if definitions.is_empty() {
            warn!("No transform definitions found in {}", transforms_dir.display());
        }

        info!(count = definitions.len(), "Transform engine initialized");

        Ok(Self {
            definitions,
            http: reqwest::Client::new(),
            initialized_databases: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        })
    }

    /// Ensure transform schemas exist for a database (idempotent, runs once per database).
    /// Only marks the database as initialized if all transforms succeed, so that
    /// transient ClickHouse failures are retried on the next event.
    pub async fn ensure_database_schemas(&self, ch_config: &ChHttpConfig, database: &str) {
        {
            let dbs = self.initialized_databases.lock().await;
            if dbs.contains(database) {
                return;
            }
        }

        let success = ensure_schemas_for_database(&self.http, &self.definitions, ch_config, database).await;

        if success {
            let mut dbs = self.initialized_databases.lock().await;
            dbs.insert(database.to_string());
        }
    }
}

/// Ensure all transform MVs and views exist for a database.
/// Handles migration from old ReplacingMergeTree tables to new MVs.
/// Returns true only if ALL transforms succeeded.
async fn ensure_schemas_for_database(
    http: &reqwest::Client,
    definitions: &[TransformDef],
    ch_config: &ChHttpConfig,
    database: &str,
) -> bool {
    let mut all_ok = true;
    for def in definitions {
        if let Err(e) = ensure_transform_schema(http, ch_config, database, def).await {
            error!(
                error = %e,
                database = %database,
                transform = %def.name,
                "Failed to ensure transform schema"
            );
            all_ok = false;
        }
    }
    all_ok
}

/// Ensure MV and view exist for a single transform in a database.
///
/// Logic:
/// 1. Check if the old ReplacingMergeTree `_{name}` exists → migrate (drop + create MV + backfill)
/// 2. If MV doesn't exist → create MV + view + backfill
/// 3. If MV exists → compare columns, recreate if mismatched, otherwise just refresh view
async fn ensure_transform_schema(
    http: &reqwest::Client,
    config: &ChHttpConfig,
    database: &str,
    def: &TransformDef,
) -> Result<(), String> {
    use crate::transform::ddl;

    let hidden_name = format!("_{}", def.name);

    // Check what currently exists for this name
    let check_sql = format!(
        "SELECT engine FROM system.tables WHERE database = '{}' AND name = '{}' FORMAT TSVRaw",
        database, hidden_name,
    );
    let engine_result = execute_query(http, config, &check_sql).await?;
    let engine_str = engine_result.trim();

    if engine_str.contains("ReplacingMergeTree") {
        // Old ReplacingMergeTree table found — migrate to MV
        info!(database = %database, transform = %def.name, "Migrating from ReplacingMergeTree to MaterializedView");

        // Drop old table and view
        execute_ddl(http, config, &format!("DROP TABLE IF EXISTS {database}.{hidden_name}")).await?;
        execute_ddl(http, config, &format!("DROP VIEW IF EXISTS {database}.{}", def.name)).await?;

        // Create new MV + view
        execute_ddl(http, config, &ddl::create_mv_ddl(database, def)).await?;
        execute_ddl(http, config, &ddl::create_view_ddl(database, def)).await?;

        // Backfill from events
        let backfill = ddl::backfill_sql(database, def);
        if let Err(e) = execute_ddl(http, config, &backfill).await {
            error!(error = %e, database = %database, transform = %def.name, "Backfill failed (MV still active for new events)");
        } else {
            info!(database = %database, transform = %def.name, "Backfill complete after migration");
        }

        return Ok(());
    }

    if engine_str.is_empty() {
        // Nothing exists — create fresh MV + view + backfill
        execute_ddl(http, config, &ddl::create_mv_ddl(database, def)).await?;
        execute_ddl(http, config, &ddl::create_view_ddl(database, def)).await?;
        info!(database = %database, transform = %def.name, "Created transform MV and view");

        // Backfill from existing events (if any)
        let backfill = ddl::backfill_sql(database, def);
        if let Err(e) = execute_ddl(http, config, &backfill).await {
            // Backfill failure is non-fatal for new MVs (events table might be empty)
            warn!(error = %e, database = %database, transform = %def.name, "Backfill had no data or failed");
        }

        return Ok(());
    }

    // MV exists — check if columns and types match
    let cols_sql = format!(
        "SELECT name, type FROM system.columns WHERE database = '{}' AND table = '{}' FORMAT TSVRaw",
        database, hidden_name,
    );
    let cols_result = execute_query(http, config, &cols_sql).await?;
    let existing_cols: std::collections::HashSet<String> = cols_result
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    // Build desired set as "name\ttype" pairs to match TSVRaw format
    let desired_cols: std::collections::HashSet<String> = std::iter::once(
        format!("{}\tString", def.key)
    ).chain(def.columns.iter().map(|c| {
        let col_type = ddl::mv_column_type(&c.strategy, &c.ch_type, &def.order_by_type);
        format!("{}\t{}", c.name, col_type)
    })).collect();

    if existing_cols != desired_cols {
        // Schema mismatch — drop and recreate MV + view + backfill
        info!(
            database = %database,
            transform = %def.name,
            "MV column mismatch, recreating"
        );
        execute_ddl(http, config, &format!("DROP TABLE IF EXISTS {database}._{}", def.name)).await?;
        execute_ddl(http, config, &format!("DROP VIEW IF EXISTS {database}.{}", def.name)).await?;

        execute_ddl(http, config, &ddl::create_mv_ddl(database, def)).await?;
        execute_ddl(http, config, &ddl::create_view_ddl(database, def)).await?;

        let backfill = ddl::backfill_sql(database, def);
        if let Err(e) = execute_ddl(http, config, &backfill).await {
            error!(error = %e, database = %database, transform = %def.name, "Backfill failed after schema change");
        }
    } else {
        // Columns match — just recreate the view (cheap, ensures it stays current)
        execute_ddl(http, config, &ddl::create_view_ddl(database, def)).await?;
    }

    Ok(())
}
