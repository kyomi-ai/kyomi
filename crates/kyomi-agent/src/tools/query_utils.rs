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


use arrow_array::{
    Array, BooleanArray, Date32Array, Float64Array, Int32Array, Int64Array, LargeStringArray,
    StringArray, TimestampMicrosecondArray, UInt64Array,
};
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
pub use kyomi_auth::chartml_validation::create_provider_for_datasource;

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

/// Extract `(block_number, sql, datasource_slug)` triples from ChartML blocks
/// in markdown content. Only returns entries that have both `data.query` and
/// `data.datasource`.
pub fn extract_chartml_queries(text: &str) -> Vec<(usize, String, String)> {
    kyomi_core::chartml_validation::markdown_blocks(text).iter().enumerate()
        .filter_map(|(i, b)| kyomi_core::chartml_validation::validate_block(b, i + 1).ok().map(|v| (i + 1, v)))
        .flat_map(|(i, v)| kyomi_core::chartml_validation::sql_sources(&v, i))
        .filter_map(|s| s.datasource.map(|d| (s.block, s.sql, d))).collect()
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
/// validation (or has no SQL source) contributes no entry.
///
/// All unavailable or failed dry-runs produce an error, including credential and connection failures.
pub async fn chartml_sql_block_errors(
    ctx: &QueryContext,
    content: &str,
) -> Vec<(usize, String)> {
    validate_chartml_complete(ctx, &kyomi_core::chartml_validation::markdown_blocks(content)).await
        .into_iter().map(|e| (e.block - 1, e.to_string())).collect()
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

pub async fn validate_chartml_complete(ctx: &QueryContext, blocks: &[&str]) -> Vec<kyomi_core::chartml_validation::Diagnostic> {
    kyomi_core::chartml_validation::validate_blocks(blocks, |source| async move {
        kyomi_auth::chartml_validation::dry_run_datasource_query_detailed(ctx, source.datasource.as_deref().expect("orchestrator checked datasource"), &source.sql).await
    }).await
}

// ---------------------------------------------------------------------------
// Single-query dry-run
// ---------------------------------------------------------------------------

/// Dry-run a SQL query against a datasource to validate syntax.
///
/// Returns `Ok(())` on success or a user-facing error string on failure.
pub use kyomi_auth::chartml_validation::dry_run_datasource_query;

pub(super) use kyomi_auth::chartml_validation::log_dry_run_issue;

/// Preserve the existing internal-error classification while removing
/// connection details from transport errors returned by agent query tools.
pub(super) fn sanitize_query_transport_error(
    error: kyomi_connect_protocol::Error,
) -> kyomi_core::Error {
    tracing::warn!(error = %error, "Datasource query transport failed");
    kyomi_core::Error::Internal(kyomi_core::sanitize_error(&error.to_string()))
}

/// Sanitize both provider validation messages and transport errors before
/// either can reach an agent tool result or its caller.
pub(super) use kyomi_auth::chartml_validation::sanitize_dry_run_result;

#[cfg(test)]
mod dry_run_redaction_tests {
    use super::{sanitize_dry_run_result, sanitize_query_transport_error};

    const URL: &str = "http://clickhouse.example:8123/?database=analytics&password=secret123";

    #[test]
    fn sanitizes_provider_validation_message() {
        let driver = kyomi_datasource_server::DryRunResult::failure(
            format!("ClickHouse request failed for {URL}"),
            Some(2),
            Some(4),
        );
        let result = sanitize_dry_run_result(Ok(driver)).expect("provider returned a result");
        assert!(!result.valid);
        assert_eq!((result.line, result.column), (Some(2), Some(4)));
        assert!(result.message.contains("[connection details redacted]"));
        assert!(!result.message.contains("secret123"));
        assert!(!result.message.contains("password="));
    }

    #[test]
    fn sanitizes_provider_error() {
        let error = kyomi_connect_protocol::Error::Provider(format!(
            "ClickHouse request failed for {URL}"
        ));
        let message = sanitize_dry_run_result(Err(error)).expect_err("provider failed");
        assert!(message.starts_with("Dry-run failed:"));
        assert!(message.contains("[connection details redacted]"));
        assert!(!message.contains("secret123"));
        assert!(!message.contains("password="));
    }

    #[test]
    fn sanitizes_query_transport_error() {
        let error = kyomi_connect_protocol::Error::Provider(format!(
            "ClickHouse request failed for {URL}"
        ));
        let output = sanitize_query_transport_error(error).to_string();
        assert!(output.contains("[connection details redacted]"));
        assert!(!output.contains("secret123"));
        assert!(!output.contains("password="));
    }
}
