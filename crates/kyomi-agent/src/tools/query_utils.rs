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

use regex::Regex;
use serde_json::Value;
use tracing;

use super::QueryContext;

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
        kyomi_datasource_server::factory::create_provider(
            &ds_type,
            &ds.connection_config,
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
        .execute_query(sql, Some(limit), None, false)
        .await
        .map_err(|e| format!("Query execution failed: {e}"))?;
    provider.close().await;

    // 4. Check status
    match result.status {
        kyomi_datasource_server::provider::QueryStatus::Error => {
            let msg = result.error.unwrap_or_else(|| "Unknown error".into());
            return Err(format!("Query failed: {msg}"));
        }
        kyomi_datasource_server::provider::QueryStatus::Success => {}
    }

    // 5. Convert to column names + dict rows
    let columns = result.columns.unwrap_or_default();
    let raw_rows = result.rows.unwrap_or_default();

    let col_names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();

    let dict_rows: Vec<serde_json::Map<String, Value>> = raw_rows
        .into_iter()
        .map(|row| {
            let mut map = serde_json::Map::new();
            for (i, col_name) in col_names.iter().enumerate() {
                let value = row.get(i).cloned().unwrap_or(Value::Null);
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

/// Validate all SQL queries inside ChartML blocks via dry-run.
///
/// Returns `None` if all queries are valid (or there are no queries).
/// Returns `Some(error_message)` with details of any invalid SQL.
///
/// Datasource resolution or credential errors are logged but do not
/// block the save — only actual SQL syntax errors are reported.
pub async fn validate_chartml_sql(
    ctx: &QueryContext,
    content: &str,
) -> Option<String> {
    let queries = extract_chartml_queries(content);
    if queries.is_empty() {
        return None;
    }

    let mut sql_errors = Vec::new();

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
                    sql_errors.push(format!("Block {block_num}: SQL error: {e}"));
                }
            }
        }
    }

    if sql_errors.is_empty() {
        None
    } else {
        Some(sql_errors.join("; "))
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
