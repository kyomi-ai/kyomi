// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared query execution utilities for agent tools.
//!
//! Extracts the common pattern of:
//! 1. Resolve datasource by slug
//! 2. Get/decrypt credentials
//! 3. Create provider
//! 4. Execute query
//! 5. Convert rows to dict format
//!
//! Used by `forecast_data`, `render_chart`, and dashboard tools.

use std::sync::OnceLock;

use arrow_array::{
    Array, BooleanArray, Date32Array, Float64Array, Int32Array, Int64Array, LargeStringArray,
    StringArray, TimestampMicrosecondArray, UInt64Array,
};
use regex::Regex;
use serde_json::Value;
use tracing;

use super::QueryContext;

// ---------------------------------------------------------------------------
// Arrow → JSON conversion
// ---------------------------------------------------------------------------

/// Convert an Arrow [`RecordBatch`] to a row-major `Vec<Vec<Value>>`.
///
/// This is the single authoritative place where Arrow columnar data is
/// converted to JSON values for the tool→LLM text boundary. It handles all
/// concrete array types that datasource providers produce. Unknown types fall
/// back to `Value::Null` rather than panicking.
///
/// **Only call this at the tool output boundary.** The Arrow pipeline must
/// stay intact through query execution; JSON conversion belongs here, not
/// inside providers or the query executor.
pub fn record_batch_to_rows(
    batch: &arrow_array::RecordBatch,
) -> Vec<Vec<Value>> {
    let num_rows = batch.num_rows();
    let num_cols = batch.num_columns();
    let mut rows = Vec::with_capacity(num_rows);
    for row_idx in 0..num_rows {
        let mut row = Vec::with_capacity(num_cols);
        for col_idx in 0..num_cols {
            let col = batch.column(col_idx);
            let val = arrow_cell_to_json(col.as_ref(), row_idx);
            row.push(val);
        }
        rows.push(row);
    }
    rows
}

/// Convert a single cell from an Arrow array to a [`serde_json::Value`].
fn arrow_cell_to_json(col: &dyn Array, row_idx: usize) -> Value {
    if col.is_null(row_idx) {
        return Value::Null;
    }
    if let Some(arr) = col.as_any().downcast_ref::<Float64Array>() {
        return serde_json::Number::from_f64(arr.value(row_idx))
            .map(Value::Number)
            .unwrap_or(Value::Null);
    }
    if let Some(arr) = col.as_any().downcast_ref::<Int64Array>() {
        return Value::Number(arr.value(row_idx).into());
    }
    if let Some(arr) = col.as_any().downcast_ref::<Int32Array>() {
        return Value::Number(arr.value(row_idx).into());
    }
    if let Some(arr) = col.as_any().downcast_ref::<UInt64Array>() {
        return Value::Number(arr.value(row_idx).into());
    }
    if let Some(arr) = col.as_any().downcast_ref::<StringArray>() {
        return Value::String(arr.value(row_idx).to_string());
    }
    if let Some(arr) = col.as_any().downcast_ref::<LargeStringArray>() {
        return Value::String(arr.value(row_idx).to_string());
    }
    if let Some(arr) = col.as_any().downcast_ref::<BooleanArray>() {
        return Value::Bool(arr.value(row_idx));
    }
    if let Some(arr) = col.as_any().downcast_ref::<Date32Array>() {
        // Date32 stores days since Unix epoch (1970-01-01)
        let days = arr.value(row_idx);
        let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid epoch");
        let date = epoch + chrono::Duration::days(i64::from(days));
        return Value::String(date.format("%Y-%m-%d").to_string());
    }
    if let Some(arr) = col.as_any().downcast_ref::<TimestampMicrosecondArray>() {
        let micros = arr.value(row_idx);
        let secs = micros.div_euclid(1_000_000);
        let sub_micros = micros.rem_euclid(1_000_000);
        let nsec = (sub_micros * 1_000) as u32;
        if let Some(dt) = chrono::DateTime::from_timestamp(secs, nsec) {
            return Value::String(dt.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string());
        }
        return Value::Null;
    }
    Value::Null
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum rows returned for chart data queries.
pub const CHART_QUERY_MAX_ROWS: u32 = 5000;

// ---------------------------------------------------------------------------
// Provider creation
// ---------------------------------------------------------------------------

/// Create a [`DatasourceProvider`] for a datasource, handling both direct and
/// Connect connection types.
///
/// For `connection_type == "connect"`: creates a [`ConnectProvider`] that routes
/// queries through the Kyomi Connect WebSocket agent.
///
/// For `connection_type == "direct"` (or any other value): resolves credentials,
/// then creates a direct provider via the datasource factory.
///
/// This is the single source of truth for provider creation in agent tools.
/// All agent tool code should use this instead of calling
/// `kyomi_datasource_server::factory::create_provider()` directly.
pub async fn create_provider_for_datasource(
    ctx: &QueryContext,
    ds: &kyomi_core::models::datasource::DatasourceConfig,
) -> Result<Box<dyn kyomi_datasource_server::DatasourceProvider>, String> {
    if ds.connection_type == "connect" {
        // Connect datasources route through the WebSocket registry —
        // no credentials needed (the Connect agent has direct DB access).
        let registry = ctx
            .connect_registry
            .as_ref()
            .ok_or_else(|| {
                format!(
                    "Datasource '{}' uses Kyomi Connect but the Connect registry is not available",
                    ds.slug
                )
            })?;
        Ok(Box::new(kyomi_datasource_server::ConnectProvider::new(
            registry.clone(),
            ds.id.clone(),
        )))
    } else {
        // Direct datasources: resolve credentials and create provider via factory.
        let ds_type: kyomi_core::datasource_registry::DatasourceType = ds.datasource_type.into();
        let credentials = super::resolve_credentials(ctx, ds, &ds_type)
            .await
            .map_err(|e| format!("Failed to resolve credentials for '{}': {e}", ds.slug))?;

        let user_context = kyomi_datasource_server::factory::UserContext {
            oauth_data: credentials.get("oauth_data").cloned(),
            user_email: String::new(),
            workspace_id: ctx.workspace_id.clone(),
        };

        // `ds.connection_config` came straight from the database and may
        // hold encrypted `COMMON_SENSITIVE` fields — every driver needs
        // plaintext.
        let decrypted_config = kyomi_auth::credential_service::decrypt_connection_config_secrets(
            &ds.connection_config,
            &ctx.encryption_key,
        )
        .map_err(|e| format!("Failed to decrypt connection_config for '{}': {e}", ds.slug))?;

        kyomi_datasource_server::factory::create_provider(
            &ds_type,
            &decrypted_config,
            &credentials,
            Some(&user_context),
        )
        .await
        .map_err(|e| format!("Failed to create provider for '{}': {e}", ds.slug))
    }
}

// ---------------------------------------------------------------------------
// Query execution
// ---------------------------------------------------------------------------

/// Result of executing a datasource query: column names + rows as dicts.
pub struct QueryRows {
    /// Column names in order.
    pub columns: Vec<String>,
    /// Each row as a `{column_name: value}` dict.
    pub rows: Vec<serde_json::Map<String, Value>>,
}

/// Execute a SQL query against a datasource and return structured results.
///
/// Handles the full lifecycle: resolve datasource → decrypt credentials →
/// create provider → execute → close provider.
///
/// # Errors
///
/// Returns a user-facing error string (not a `kyomi_core::Error`) so callers
/// can include it directly in tool responses.
pub async fn execute_datasource_query(
    ctx: &QueryContext,
    datasource_slug: &str,
    sql: &str,
    max_rows: Option<u32>,
) -> Result<QueryRows, String> {
    // 1. Resolve datasource
    let ds = kyomi_auth::datasource_service::resolve_datasource(
        &ctx.db,
        datasource_slug,
        &ctx.workspace_id,
        false,
    )
    .await
    .map_err(|e| format!("Failed to resolve datasource '{datasource_slug}': {e}"))?;

    // 2. Create provider (handles both direct and Connect datasources)
    let provider = create_provider_for_datasource(ctx, &ds).await?;

    // 3. Execute query
    let limit = max_rows.unwrap_or(CHART_QUERY_MAX_ROWS);
    let result = provider
        .execute_query(sql, Some(limit), None, false, None)
        .await
        .map_err(|e| {
            tracing::warn!(raw_error = %e, "datasource query error (sanitized for caller)");
            format!("Query execution failed: {}", kyomi_core::sanitize_error(&e.to_string()))
        })?;
    provider.close().await;

    // 4. Check status
    match result.status {
        kyomi_datasource_server::provider::QueryStatus::Error => {
            let msg = result.error.unwrap_or_else(|| "Unknown error".into());
            tracing::warn!(raw_error = %msg, "datasource query status error (sanitized for caller)");
            return Err(format!("Query failed: {}", kyomi_core::sanitize_error(&msg)));
        }
        kyomi_datasource_server::provider::QueryStatus::Success => {}
    }

    // 5. Convert to column names + dict rows
    let columns = result.columns.unwrap_or_default();
    let col_names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();

    let positional_rows = result
        .record_batch
        .as_ref()
        .map(record_batch_to_rows)
        .unwrap_or_default();

    let dict_rows: Vec<serde_json::Map<String, Value>> = positional_rows
        .into_iter()
        .map(|row| {
            let mut map = serde_json::Map::new();
            for (col_name, value) in col_names.iter().zip(row) {
                map.insert(col_name.clone(), value);
            }
            map
        })
        .collect();

    tracing::debug!(
        datasource = %datasource_slug,
        rows = dict_rows.len(),
        cols = col_names.len(),
        "Query executed successfully"
    );

    Ok(QueryRows {
        columns: col_names,
        rows: dict_rows,
    })
}

// ---------------------------------------------------------------------------
// ChartML SQL validation
// ---------------------------------------------------------------------------

/// Compiled regex for extracting ChartML fenced code blocks.
static CHARTML_RE: OnceLock<Regex> = OnceLock::new();

fn chartml_re() -> &'static Regex {
    CHARTML_RE
        .get_or_init(|| Regex::new(r"```chartml\s*\n([\s\S]*?)\n```").expect("valid regex literal"))
}

/// Extract `(block_number, sql, datasource_slug)` triples from ChartML blocks
/// in markdown content. Only returns entries that have both `data.query` and
/// `data.datasource`.
pub fn extract_chartml_queries(text: &str) -> Vec<(usize, String, String)> {
    let re = chartml_re();
    let mut queries = Vec::new();
    for (i, cap) in re.captures_iter(text).enumerate() {
        let block_content = &cap[1];
        let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(block_content) else {
            continue; // YAML parse errors are caught separately
        };
        let Some(data) = value.get("data") else {
            continue;
        };
        let query = data
            .get("query")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let datasource = data
            .get("datasource")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if let (Some(sql), Some(slug)) = (query, datasource) {
            queries.push((i + 1, sql, slug));
        }
    }
    queries
}

/// Per-block SQL dry-run validation errors.
///
/// The primitive [`validate_chartml_sql`] (aggregate) wraps. Each entry's
/// `usize` is the block's **0-based position** in `chartml_re()`'s
/// capture-iteration order — `extract_chartml_queries`'s 1-based
/// `block_number` minus one. This is the same 0-based indexing contract
/// `agent.rs`'s `chartml_block_errors` and `strip_chartml_blocks` use; the
/// `Block N` text embedded in each message stays 1-based for readability,
/// but the index used for stripping is always 0-based. A block that passes
/// validation (or is skipped as an infra error, or has no query/datasource
/// pair to dry-run) contributes no entry.
///
/// Datasource resolution or credential errors are logged but do not produce
/// an entry — only actual SQL syntax errors do.
pub async fn chartml_sql_block_errors(
    ctx: &QueryContext,
    content: &str,
) -> Vec<(usize, String)> {
    let queries = extract_chartml_queries(content);
    let mut errors = Vec::new();

    for (block_num, sql, slug) in &queries {
        match dry_run_datasource_query(ctx, slug, sql).await {
            Ok(()) => {} // valid
            Err(e) => {
                // Distinguish infra errors (datasource not found, no creds)
                // from actual SQL errors. Only report SQL errors to the user.
                if e.starts_with("Failed to resolve") || e.starts_with("Failed to create") {
                    tracing::warn!(
                        block = block_num,
                        slug = %slug,
                        error = %e,
                        "ChartML SQL validation: infra error, skipping block"
                    );
                } else {
                    // `block_num` is 1-based (see `extract_chartml_queries`);
                    // the index carried alongside the message is 0-based, per
                    // this function's doc comment.
                    errors.push((block_num - 1, format!("Block {block_num}: SQL error: {e}")));
                }
            }
        }
    }

    errors
}

/// Validate all SQL queries inside ChartML blocks via dry-run.
///
/// Returns `None` if all queries are valid (or there are no queries).
/// Returns `Some(error_message)` with details of any invalid SQL. Thin
/// aggregate wrapper over [`chartml_sql_block_errors`] — see that function
/// for the per-block primitive.
pub async fn validate_chartml_sql(
    ctx: &QueryContext,
    content: &str,
) -> Option<String> {
    let errors = chartml_sql_block_errors(ctx, content).await;
    if errors.is_empty() {
        None
    } else {
        let message = errors.iter().map(|(_, msg)| msg.as_str()).collect::<Vec<_>>().join("; ");
        Some(message)
    }
}

// ---------------------------------------------------------------------------
// Single-query dry-run
// ---------------------------------------------------------------------------

/// Dry-run a SQL query against a datasource to validate syntax.
///
/// Returns `Ok(())` on success or a user-facing error string on failure.
pub async fn dry_run_datasource_query(
    ctx: &QueryContext,
    datasource_slug: &str,
    sql: &str,
) -> Result<(), String> {
    let ds = kyomi_auth::datasource_service::resolve_datasource(
        &ctx.db,
        datasource_slug,
        &ctx.workspace_id,
        false,
    )
    .await
    .map_err(|e| format!("Failed to resolve datasource '{datasource_slug}': {e}"))?;

    let provider = create_provider_for_datasource(ctx, &ds).await?;

    let result = provider.dry_run(sql).await.map_err(|e| format!("Dry-run failed: {e}"))?;
    provider.close().await;

    if result.valid {
        Ok(())
    } else {
        Err(result.message)
    }
}
