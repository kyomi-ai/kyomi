// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart data resolver — executes datasource queries in ChartML specs.
//!
//! The chart renderer (and MCP App) only work with inline data. This module
//! checks if a chart spec has `data.datasource` + `data.query` (needs query
//! execution) and converts it to inline `{ provider: "inline", rows: [...] }`.
//!
//! Ports Python's `chart_data_resolver.py`.

use std::collections::HashSet;

use serde_json::{json, Map, Value};
use tracing;

use super::query_utils::{execute_datasource_query, CHART_QUERY_MAX_ROWS};
use super::QueryContext;

// ---------------------------------------------------------------------------
// Reserved data keys
// ---------------------------------------------------------------------------

/// Keys that indicate an unnamed (single) data source, not named sources.
/// Matches Python's `_RESERVED_DATA_KEYS` and frontend's `RESERVED_DATA_KEYS`.
fn reserved_data_keys() -> HashSet<&'static str> {
    ["datasource", "provider", "query", "rows", "url", "cache"]
        .into_iter()
        .collect()
}

/// Check if spec.data uses named sources (not an unnamed source).
fn is_named_sources(data: &Map<String, Value>) -> bool {
    let reserved = reserved_data_keys();
    !data.keys().any(|k| reserved.contains(k.as_str()))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve ChartML chart data by executing queries if needed.
///
/// Checks the `data` section of a ChartML spec and, if it contains
/// `datasource` + `query`, executes the query and replaces the data
/// with inline rows.
///
/// Handles:
/// - String references (logged, returned as-is)
/// - Already-inline data (returned as-is)
/// - Named sources (each resolved independently)
/// - Unnamed single source (resolved directly)
pub async fn resolve_chart_data(
    spec: &Value,
    ctx: &QueryContext,
) -> Result<Value, String> {
    let mut spec = spec.clone();

    let data = match spec.get("data") {
        Some(d) => d.clone(),
        None => return Ok(spec),
    };

    // Case 1: data is a string (reference to named source) — not supported
    if data.is_string() {
        tracing::warn!(
            source = data.as_str().unwrap_or("?"),
            "ChartML data references named source — not resolved"
        );
        return Ok(spec);
    }

    // Case 2: data is not an object — nothing to do
    let data_obj = match data.as_object() {
        Some(obj) => obj,
        None => return Ok(spec),
    };

    // Case 3: already has inline rows
    if data_obj.get("provider").and_then(|v| v.as_str()) == Some("inline")
        || data_obj.contains_key("rows")
    {
        tracing::debug!("ChartML data already has inline rows");
        return Ok(spec);
    }

    // Case 4: Named sources
    if is_named_sources(data_obj) {
        let resolved_data = resolve_named_sources(data_obj, ctx).await?;
        spec["data"] = Value::Object(resolved_data);
        return Ok(spec);
    }

    // Case 5: Unnamed single source
    let resolved_data = resolve_single_source(data_obj, ctx).await?;
    spec["data"] = resolved_data;
    Ok(spec)
}

// ---------------------------------------------------------------------------
// Named sources
// ---------------------------------------------------------------------------

/// Resolve each named data source individually.
///
/// Named sources look like:
/// ```yaml
/// data:
///   sales:
///     datasource: my-db
///     query: SELECT * FROM sales
///   targets:
///     provider: inline
///     rows: [...]
/// ```
async fn resolve_named_sources(
    data: &Map<String, Value>,
    ctx: &QueryContext,
) -> Result<Map<String, Value>, String> {
    let mut resolved = Map::new();

    for (source_name, source_spec) in data {
        let source_obj = match source_spec.as_object() {
            Some(obj) => obj,
            None => {
                resolved.insert(source_name.clone(), source_spec.clone());
                continue;
            }
        };

        // Already has inline rows — keep as-is
        if source_obj.get("provider").and_then(|v| v.as_str()) == Some("inline")
            || source_obj.contains_key("rows")
        {
            resolved.insert(source_name.clone(), source_spec.clone());
            continue;
        }

        // Needs query execution
        let query = match source_obj.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => {
                resolved.insert(source_name.clone(), source_spec.clone());
                continue;
            }
        };

        let Some(datasource_slug) = source_obj.get("datasource").and_then(|v| v.as_str()) else {
            resolved.insert(source_name.clone(), source_spec.clone());
            continue;
        };

        let rows = execute_datasource_query(
            ctx,
            datasource_slug,
            query,
            Some(CHART_QUERY_MAX_ROWS),
        )
        .await
        .map_err(|e| format!("Failed to resolve named source '{source_name}': {e}"))?;

        let inline_rows: Vec<Value> = rows
            .rows
            .into_iter()
            .map(|m| Value::Object(m))
            .collect();

        tracing::info!(
            source = %source_name,
            rows = inline_rows.len(),
            "Resolved named source"
        );

        resolved.insert(
            source_name.clone(),
            json!({ "provider": "inline", "rows": inline_rows }),
        );
    }

    Ok(resolved)
}

// ---------------------------------------------------------------------------
// Single (unnamed) source
// ---------------------------------------------------------------------------

/// Resolve an unnamed single data source with datasource + query.
async fn resolve_single_source(
    data: &Map<String, Value>,
    ctx: &QueryContext,
) -> Result<Value, String> {
    let query = match data.get("query").and_then(|v| v.as_str()) {
        Some(q) => q,
        None => {
            tracing::debug!("ChartML data has no query to execute");
            return Ok(Value::Object(data.clone()));
        }
    };

    let Some(datasource_slug) = data.get("datasource").and_then(|v| v.as_str()) else {
        tracing::warn!("ChartML data has query but no datasource specified");
        return Ok(Value::Object(data.clone()));
    };

    let rows = execute_datasource_query(
        ctx,
        datasource_slug,
        query,
        Some(CHART_QUERY_MAX_ROWS),
    )
    .await?;

    let inline_rows: Vec<Value> = rows
        .rows
        .into_iter()
        .map(|m| Value::Object(m))
        .collect();

    tracing::info!(rows = inline_rows.len(), "Resolved chart data");

    Ok(json!({ "provider": "inline", "rows": inline_rows }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_keys_match_python() {
        let keys = reserved_data_keys();
        assert!(keys.contains("datasource"));
        assert!(keys.contains("provider"));
        assert!(keys.contains("query"));
        assert!(keys.contains("rows"));
        assert!(keys.contains("url"));
        assert!(keys.contains("cache"));
        assert_eq!(keys.len(), 6);
    }

    #[test]
    fn is_named_sources_detects_named() {
        let data: Map<String, Value> = serde_json::from_value(json!({
            "sales": { "datasource": "db", "query": "SELECT 1" },
            "targets": { "provider": "inline", "rows": [] }
        }))
        .unwrap();
        assert!(is_named_sources(&data));
    }

    #[test]
    fn is_named_sources_detects_unnamed() {
        let data: Map<String, Value> = serde_json::from_value(json!({
            "datasource": "my-db",
            "query": "SELECT 1"
        }))
        .unwrap();
        assert!(!is_named_sources(&data));
    }

    #[test]
    fn is_named_sources_inline_rows() {
        let data: Map<String, Value> = serde_json::from_value(json!({
            "rows": [{"a": 1}]
        }))
        .unwrap();
        assert!(!is_named_sources(&data));
    }

    #[test]
    fn is_named_sources_provider_inline() {
        let data: Map<String, Value> = serde_json::from_value(json!({
            "provider": "inline",
            "rows": [{"x": 1}]
        }))
        .unwrap();
        assert!(!is_named_sources(&data));
    }
}
