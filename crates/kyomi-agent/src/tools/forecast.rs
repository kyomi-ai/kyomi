// SPDX-License-Identifier: AGPL-3.0-or-later

//! Forecast tool — run time series forecasting on data from a datasource.

use async_trait::async_trait;

use crate::forecast;
use crate::tools::{AgentTool, ToolContext};
use crate::types::ToolAnnotations;

/// Maximum rows for forecast queries. Balances full time series access with
/// query performance.
const FORECAST_QUERY_LIMIT: u32 = 10_000;

// ---------------------------------------------------------------------------
// ForecastDataTool
// ---------------------------------------------------------------------------

/// Run time series forecasting on data from a datasource.
pub struct ForecastDataTool;

#[async_trait]
impl AgentTool for ForecastDataTool {
    fn name(&self) -> &str {
        "forecast_data"
    }

    fn description(&self) -> &str {
        "Run time series forecasting on data from a datasource.\n\n\
         Returns forecast predictions with confidence intervals that you can use \
         to make projections and recommendations. Use this when the user asks \
         about future trends, projections, or predictions.\n\n\
         The tool queries the datasource, fits a forecasting model, and returns \
         predicted values with upper/lower confidence bounds.\n\n\
         Models: 'auto' (recommended - selects best model via cross-validation), \
         'ets' (exponential smoothing), 'linear', 'exponential' (growth curves), \
         'logistic' (S-curves/saturation).\n\n\
         Minimum 4 data points required. Always include ORDER BY on the timestamp \
         column in your SQL query."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "datasource": {
                    "type": "string",
                    "description": "Datasource slug"
                },
                "query": {
                    "type": "string",
                    "description": "SQL query returning timestamp + value columns"
                },
                "timestamp": {
                    "type": "string",
                    "description": "Name of the timestamp column"
                },
                "value": {
                    "type": "string",
                    "description": "Name of the value column to forecast"
                },
                "horizon": {
                    "type": "integer",
                    "description": "Number of periods to forecast ahead",
                    "default": 3
                },
                "confidence_level": {
                    "type": "number",
                    "description": "Confidence interval width (0-1)",
                    "default": 0.95
                },
                "model": {
                    "type": "string",
                    "enum": ["auto", "ets", "linear", "exponential", "logistic"],
                    "default": "auto"
                },
                "group_by": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Columns to group by for per-group forecasts"
                }
            },
            "required": ["datasource", "query", "timestamp", "value"]
        })
    }

    fn annotations(&self) -> Option<ToolAnnotations> {
        Some(ToolAnnotations {
            read_only_hint: Some(true),
            idempotent_hint: Some(true),
            open_world_hint: Some(true),
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let datasource_slug = args
            .get("datasource")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'datasource'".into())
            })?;
        let sql = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'query'".into())
            })?;
        let timestamp_col = args
            .get("timestamp")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'timestamp'".into())
            })?;
        let value_col = args
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::BadRequest("Missing required parameter 'value'".into())
            })?;
        let horizon = args
            .get("horizon")
            .and_then(|v| v.as_u64())
            .unwrap_or(3) as usize;
        let confidence_level = args
            .get("confidence_level")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.95);
        let model = args
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("auto");
        let group_by: Option<Vec<String>> = args.get("group_by").and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str().map(String::from))
                    .collect()
            })
        });

        // Trial mode: return a simplified response
        if ctx.is_trial {
            return Ok(serde_json::json!({
                "error": "Forecast is not available in trial mode. \
                          Connect your own datasource to use forecasting."
            })
            .to_string());
        }

        // Resolve datasource
        let ds = kyomi_auth::datasource_service::resolve_datasource(
            &ctx.db,
            datasource_slug,
            &ctx.workspace_id,
            false,
        )
        .await?;

        // Create provider (handles both direct and Connect datasources)
        let query_ctx = ctx.query_context();
        let provider =
            super::query_utils::create_provider_for_datasource(&query_ctx, &ds)
                .await
                .map_err(kyomi_core::Error::Internal)?;

        let result = provider
            .execute_query(sql, Some(FORECAST_QUERY_LIMIT), None, false)
            .await?;
        provider.close().await;

        match result.status {
            kyomi_datasource_server::provider::QueryStatus::Error => {
                let error_msg = result
                    .error
                    .unwrap_or_else(|| "Unknown query error".to_string());
                return Ok(
                    serde_json::json!({ "error": format!("Query failed: {error_msg}") })
                        .to_string(),
                );
            }
            kyomi_datasource_server::provider::QueryStatus::Success => {}
        }

        let columns = result.columns.unwrap_or_default();
        let rows = result.rows.unwrap_or_default();

        if rows.is_empty() {
            return Ok(serde_json::json!({ "error": "Query returned no data." }).to_string());
        }

        // Build columnar data: { col_name: [values] }
        let col_names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();

        // Find timestamp and value column indices
        let ts_idx = col_names
            .iter()
            .position(|name| name == timestamp_col)
            .ok_or_else(|| {
                let available = col_names.join(", ");
                kyomi_core::Error::BadRequest(format!(
                    "Timestamp column '{timestamp_col}' not found in query results. \
                     Available columns: {available}"
                ))
            })?;
        let val_idx = col_names
            .iter()
            .position(|name| name == value_col)
            .ok_or_else(|| {
                let available = col_names.join(", ");
                kyomi_core::Error::BadRequest(format!(
                    "Value column '{value_col}' not found in query results. \
                     Available columns: {available}"
                ))
            })?;

        // Validate group_by columns exist
        let group_indices: Vec<(String, usize)> = if let Some(ref gb) = group_by {
            let mut indices = Vec::new();
            for col in gb {
                let idx = col_names.iter().position(|name| name == col).ok_or_else(|| {
                    let available = col_names.join(", ");
                    kyomi_core::Error::BadRequest(format!(
                        "Group-by column '{col}' not found in query results. \
                         Available columns: {available}"
                    ))
                })?;
                indices.push((col.clone(), idx));
            }
            indices
        } else {
            Vec::new()
        };

        // Extract data into columns
        let timestamps: Vec<&serde_json::Value> =
            rows.iter().map(|row| &row[ts_idx]).collect();
        let raw_values: Vec<&serde_json::Value> =
            rows.iter().map(|row| &row[val_idx]).collect();

        if group_indices.is_empty() {
            // Single (ungrouped) forecast
            run_single_forecast(
                &timestamps,
                &raw_values,
                timestamp_col,
                value_col,
                horizon,
                confidence_level,
                model,
            )
        } else {
            // Grouped forecast
            run_grouped_forecast(
                &rows,
                &group_indices,
                ts_idx,
                val_idx,
                timestamp_col,
                value_col,
                horizon,
                confidence_level,
                model,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Single (ungrouped) forecast
// ---------------------------------------------------------------------------

fn run_single_forecast(
    timestamps: &[&serde_json::Value],
    raw_values: &[&serde_json::Value],
    timestamp_col: &str,
    value_col: &str,
    horizon: usize,
    confidence_level: f64,
    model: &str,
) -> kyomi_core::Result<String> {
    // Validate timestamp ordering
    if !is_ordered(timestamps) {
        return Ok(serde_json::json!({
            "error": format!(
                "Data is not ordered by timestamp column '{timestamp_col}'. \
                 Add ORDER BY {timestamp_col} to your query."
            )
        })
        .to_string());
    }

    // Convert values to f64
    let float_values = parse_float_values(raw_values, value_col)?;

    let result = forecast::forecast(&float_values, horizon, confidence_level, model, None);

    if let Some(ref error) = result.error {
        return Ok(serde_json::json!({ "error": error }).to_string());
    }

    // Enrich with projected timestamps
    let projected = project_timestamps(timestamps, horizon);
    let enriched_points = enrich_forecast_points(&result.forecast, &projected);

    Ok(serde_json::json!({
        "model_used": result.model_used,
        "data_points": result.data_points,
        "forecast": enriched_points,
        "summary": format!(
            "Forecasted {} periods using {} model with {}% confidence intervals",
            horizon,
            result.model_used,
            (confidence_level * 100.0) as u32
        ),
    })
    .to_string())
}

// ---------------------------------------------------------------------------
// Grouped forecast
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_grouped_forecast(
    rows: &[Vec<serde_json::Value>],
    group_indices: &[(String, usize)],
    ts_idx: usize,
    val_idx: usize,
    _timestamp_col: &str,
    _value_col: &str,
    horizon: usize,
    confidence_level: f64,
    model: &str,
) -> kyomi_core::Result<String> {
    // Group rows by group_by columns
    let mut groups: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();

    for (row_idx, row) in rows.iter().enumerate() {
        let key_parts: Vec<String> = group_indices
            .iter()
            .map(|(_, idx)| json_value_to_string(&row[*idx]))
            .collect();
        let group_key = if key_parts.len() == 1 {
            key_parts.into_iter().next().unwrap_or_default()
        } else {
            key_parts.join(" | ")
        };
        groups.entry(group_key).or_default().push(row_idx);
    }

    let mut group_results = serde_json::Map::new();

    for (group_key, row_indices) in &groups {
        // Extract timestamps and values for this group
        let group_timestamps: Vec<&serde_json::Value> =
            row_indices.iter().map(|&i| &rows[i][ts_idx]).collect();
        let group_raw_values: Vec<&serde_json::Value> =
            row_indices.iter().map(|&i| &rows[i][val_idx]).collect();

        // Convert values to f64
        let float_values = match group_raw_values
            .iter()
            .map(|v| json_value_to_f64(v))
            .collect::<Result<Vec<f64>, _>>()
        {
            Ok(vals) => vals,
            Err(_) => {
                group_results.insert(
                    group_key.clone(),
                    serde_json::json!({
                        "error": format!("Non-numeric values in group '{group_key}'.")
                    }),
                );
                continue;
            }
        };

        let result = forecast::forecast(&float_values, horizon, confidence_level, model, None);

        if let Some(ref error) = result.error {
            group_results.insert(
                group_key.clone(),
                serde_json::json!({ "error": error }),
            );
        } else {
            let projected = project_timestamps(&group_timestamps, horizon);
            let enriched_points = enrich_forecast_points(&result.forecast, &projected);

            group_results.insert(
                group_key.clone(),
                serde_json::json!({
                    "model_used": result.model_used,
                    "data_points": result.data_points,
                    "forecast": enriched_points,
                }),
            );
        }
    }

    let successful = group_results
        .values()
        .filter(|v| v.get("error").is_none())
        .count();
    let failed = group_results.len() - successful;
    let total_data_points: usize = group_results
        .values()
        .filter_map(|v| v.get("data_points").and_then(|dp| dp.as_u64()))
        .sum::<u64>() as usize;

    let mut summary_parts = vec![format!(
        "Forecasted {} of {} groups",
        successful,
        group_results.len()
    )];
    if failed > 0 {
        summary_parts.push(format!("({failed} failed)"));
    }
    summary_parts.push(format!(
        "with {}% confidence intervals",
        (confidence_level * 100.0) as u32
    ));

    Ok(serde_json::json!({
        "groups": group_results,
        "total_groups": group_results.len(),
        "successful_groups": successful,
        "total_data_points": total_data_points,
        "summary": summary_parts.join(" "),
    })
    .to_string())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if a list of JSON values is in non-decreasing order.
fn is_ordered(values: &[&serde_json::Value]) -> bool {
    if values.len() < 2 {
        return true;
    }
    for i in 0..values.len() - 1 {
        let a = json_value_to_string(values[i]);
        let b = json_value_to_string(values[i + 1]);
        if a > b {
            return false;
        }
    }
    true
}

/// Parse JSON values as f64 values.
fn parse_float_values(
    raw: &[&serde_json::Value],
    col_name: &str,
) -> kyomi_core::Result<Vec<f64>> {
    raw.iter()
        .map(|v| {
            json_value_to_f64(v).map_err(|_| {
                kyomi_core::Error::BadRequest(format!(
                    "Value column '{col_name}' contains non-numeric data."
                ))
            })
        })
        .collect()
}

/// Convert a JSON value to f64.
fn json_value_to_f64(v: &serde_json::Value) -> Result<f64, ()> {
    match v {
        serde_json::Value::Number(n) => n.as_f64().ok_or(()),
        serde_json::Value::String(s) => s.parse::<f64>().map_err(|_| ()),
        serde_json::Value::Null => Err(()),
        _ => Err(()),
    }
}

/// Convert a JSON value to a string for display/comparison.
fn json_value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

/// Enrich forecast points with projected timestamp labels.
fn enrich_forecast_points(
    points: &[forecast::ForecastPoint],
    projected: &[String],
) -> Vec<serde_json::Value> {
    points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let ts = projected
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("Step {}", p.step));
            serde_json::json!({
                "step": p.step,
                "timestamp": ts,
                "predicted": p.forecast,
                "lower": p.lower_bound,
                "upper": p.upper_bound,
            })
        })
        .collect()
}

/// Project future timestamps by inferring the interval from original data.
///
/// Parses timestamps as strings, infers the median interval, and projects
/// future values. Falls back to step labels if parsing fails.
fn project_timestamps(
    original: &[&serde_json::Value],
    count: usize,
) -> Vec<String> {
    if original.len() < 2 {
        return (1..=count)
            .map(|i| format!("Step {i}"))
            .collect();
    }

    // Try to parse as dates using chrono
    let parsed: Vec<Option<chrono::NaiveDate>> = original
        .iter()
        .map(|v| {
            let s = json_value_to_string(v);
            // Try common date formats
            chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                .or_else(|_| {
                    // Try datetime format, extract date
                    chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S")
                        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S"))
                        .map(|dt| dt.date())
                })
                .ok()
        })
        .collect();

    // Check if all parsed successfully
    if parsed.iter().any(|d| d.is_none()) {
        // Can't parse dates, fall back to step labels
        let last_ts = json_value_to_string(original.last().unwrap_or(&&serde_json::Value::Null));
        return (1..=count)
            .map(|i| format!("Step +{i} after {last_ts}"))
            .collect();
    }

    let dates: Vec<chrono::NaiveDate> = parsed.into_iter().flatten().collect();

    // Compute median interval in days
    let mut diffs: Vec<i64> = dates
        .windows(2)
        .map(|w| (w[1] - w[0]).num_days())
        .collect();
    diffs.sort();

    let median_days = if diffs.len() % 2 == 1 {
        diffs[diffs.len() / 2]
    } else {
        (diffs[diffs.len() / 2 - 1] + diffs[diffs.len() / 2]) / 2
    };

    if median_days <= 0 {
        let last_ts = json_value_to_string(original.last().unwrap_or(&&serde_json::Value::Null));
        return (1..=count)
            .map(|i| format!("Step +{i} after {last_ts}"))
            .collect();
    }

    let last_date = dates.last().copied().unwrap_or(dates[0]);
    (1..=count)
        .map(|i| {
            let future_date =
                last_date + chrono::Duration::days(median_days * i as i64);
            future_date.format("%Y-%m-%d").to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forecast_data_tool_name() {
        assert_eq!(ForecastDataTool.name(), "forecast_data");
    }

    #[test]
    fn forecast_data_tool_description_not_empty() {
        assert!(!ForecastDataTool.description().is_empty());
    }

    #[test]
    fn forecast_data_tool_schema_has_required_fields() {
        let schema = ForecastDataTool.parameters_schema();
        assert_eq!(schema["type"], "object");
        let required = schema["required"].as_array().expect("required is array");
        assert!(required.contains(&serde_json::json!("datasource")));
        assert!(required.contains(&serde_json::json!("query")));
        assert!(required.contains(&serde_json::json!("timestamp")));
        assert!(required.contains(&serde_json::json!("value")));
    }

    #[test]
    fn forecast_data_tool_schema_has_optional_fields() {
        let schema = ForecastDataTool.parameters_schema();
        let props = schema["properties"]
            .as_object()
            .expect("properties is object");
        assert!(props.contains_key("horizon"));
        assert!(props.contains_key("confidence_level"));
        assert!(props.contains_key("model"));
        assert!(props.contains_key("group_by"));
    }

    #[test]
    fn forecast_data_tool_annotations_read_only() {
        let ann = ForecastDataTool.annotations().expect("has annotations");
        assert_eq!(ann.read_only_hint, Some(true));
        assert!(ann.destructive_hint.is_none());
        assert_eq!(ann.idempotent_hint, Some(true));
        assert_eq!(ann.open_world_hint, Some(true));
    }

    #[test]
    fn forecast_data_tool_not_copilot_only() {
        assert!(!ForecastDataTool.is_copilot_only());
    }

    #[test]
    fn test_is_ordered() {
        let a = serde_json::json!("2024-01-01");
        let b = serde_json::json!("2024-01-02");
        let c = serde_json::json!("2024-01-03");
        assert!(is_ordered(&[&a, &b, &c]));
        assert!(!is_ordered(&[&c, &a, &b]));
    }

    #[test]
    fn test_project_timestamps_dates() {
        let a = serde_json::json!("2024-01-01");
        let b = serde_json::json!("2024-01-02");
        let c = serde_json::json!("2024-01-03");
        let projected = project_timestamps(&[&a, &b, &c], 3);
        assert_eq!(projected.len(), 3);
        assert_eq!(projected[0], "2024-01-04");
        assert_eq!(projected[1], "2024-01-05");
        assert_eq!(projected[2], "2024-01-06");
    }

    #[test]
    fn test_project_timestamps_fallback() {
        let a = serde_json::json!("not-a-date");
        let b = serde_json::json!("also-not");
        let projected = project_timestamps(&[&a, &b], 2);
        assert_eq!(projected.len(), 2);
        assert!(projected[0].starts_with("Step +"));
    }

    #[test]
    fn test_json_value_to_f64() {
        assert_eq!(json_value_to_f64(&serde_json::json!(42.0)), Ok(42.0));
        assert_eq!(json_value_to_f64(&serde_json::json!("3.14")), Ok(3.14));
        assert!(json_value_to_f64(&serde_json::json!(null)).is_err());
        assert!(json_value_to_f64(&serde_json::json!("abc")).is_err());
    }
}
