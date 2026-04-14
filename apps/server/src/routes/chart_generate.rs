// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart generation endpoint — rule-based ChartML from SQL query results.
//!
//! Wire-compatible with Python's `routers/chart_generate.py`.
//! Only the rule-based path is implemented; AI generation (user_context != null)
//! is rejected with 400.

use std::collections::HashSet;

use axum::{routing::post, Json, Router};
use serde::{Deserialize, Serialize};

use kyomi_auth::middleware::AuthUser;

use crate::state::AppState;

/// Build the chart generation router.
///
/// Mounted under `/api/v1/chart` so the full path is:
/// - `POST /api/v1/chart/generate`
pub fn routes() -> Router<AppState> {
    Router::new().route("/generate", post(generate_chart))
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct ChartGenerateRequest {
    sql_text: String,
    columns: Vec<String>,
    rows: Vec<Vec<serde_json::Value>>,
    user_context: Option<String>,
    datasource_slug: Option<String>,
    datasource_type: Option<String>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct ChartGenerateResponse {
    chart_yaml: String,
}

// ---------------------------------------------------------------------------
// Column analysis
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ColumnAnalysis {
    name: String,
    is_numeric: bool,
    is_date: bool,
    cardinality: usize,
}

/// Check whether a JSON value looks like a date/datetime string.
fn is_date_value(v: &serde_json::Value) -> bool {
    let s = match v.as_str() {
        Some(s) => s,
        None => return false,
    };
    // Try common date/datetime formats
    if chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        return true;
    }
    if chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").is_ok() {
        return true;
    }
    if chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").is_ok() {
        return true;
    }
    // RFC 3339 / ISO 8601 with timezone
    if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
        return true;
    }
    false
}

/// Analyze a single column across the provided rows.
fn analyze_column(
    col_name: &str,
    rows: &[Vec<serde_json::Value>],
    columns: &[String],
) -> ColumnAnalysis {
    let col_index = match columns.iter().position(|c| c == col_name) {
        Some(i) => i,
        None => {
            return ColumnAnalysis {
                name: col_name.to_string(),
                is_numeric: false,
                is_date: false,
                cardinality: 0,
            }
        }
    };

    let values: Vec<&serde_json::Value> = rows
        .iter()
        .filter_map(|row| row.get(col_index))
        .filter(|v| !v.is_null())
        .collect();

    if values.is_empty() {
        return ColumnAnalysis {
            name: col_name.to_string(),
            is_numeric: false,
            is_date: false,
            cardinality: 0,
        };
    }

    let is_numeric = values.iter().all(|v| v.is_number());
    let is_date = values.iter().all(|v| is_date_value(v));
    let cardinality = values
        .iter()
        .map(|v| v.to_string())
        .collect::<HashSet<_>>()
        .len();

    ColumnAnalysis {
        name: col_name.to_string(),
        is_numeric,
        is_date,
        cardinality,
    }
}

// ---------------------------------------------------------------------------
// Chart type / axis inference
// ---------------------------------------------------------------------------

/// Infer the best chart type from column analyses.
fn infer_chart_type(analyses: &[ColumnAnalysis]) -> &'static str {
    // Time series → line chart
    if analyses.iter().any(|a| a.is_date) {
        return "line";
    }
    // Categories with reasonable cardinality → bar chart
    if let Some(cat) = analyses
        .iter()
        .find(|a| !a.is_numeric && !a.is_date)
        && cat.cardinality <= 20 {
            return "bar";
        }
    // Default to table
    "table"
}

/// Infer x and y axes from column analyses.
fn infer_axes<'a>(analyses: &'a [ColumnAnalysis], columns: &'a [String]) -> (&'a str, &'a str) {
    // X-axis: first date column, else first non-numeric, else first column
    let x_col = analyses
        .iter()
        .find(|a| a.is_date)
        .or_else(|| analyses.iter().find(|a| !a.is_numeric))
        .or_else(|| analyses.first());

    let x_name = x_col.map(|a| a.name.as_str()).unwrap_or(&columns[0]);

    // Y-axis: first numeric column that isn't x, else second column, else first
    let y_col = analyses
        .iter()
        .find(|a| a.is_numeric && a.name != x_name)
        .or_else(|| {
            if analyses.len() > 1 {
                Some(&analyses[1])
            } else {
                analyses.first()
            }
        });

    let y_name = y_col.map(|a| a.name.as_str()).unwrap_or_else(|| {
        if columns.len() > 1 {
            &columns[1]
        } else {
            &columns[0]
        }
    });

    (x_name, y_name)
}

// ---------------------------------------------------------------------------
// ChartML spec builders
// ---------------------------------------------------------------------------

/// Build the `data:` section of the spec.
fn build_data_section(
    sql_text: &str,
    datasource_slug: Option<&str>,
    datasource_type: Option<&str>,
) -> serde_yaml::Mapping {
    let mut data = serde_yaml::Mapping::new();
    data.insert(y("query"), y(sql_text));

    // Cache
    let mut cache = serde_yaml::Mapping::new();
    cache.insert(y("ttl"), y("24h"));
    data.insert(y("cache"), serde_yaml::Value::Mapping(cache));

    // Datasource reference
    if let Some(slug) = datasource_slug {
        data.insert(y("datasource"), y(slug));
    }
    if let Some(dt) = datasource_type {
        data.insert(y("provider"), y(dt));
    } else if datasource_slug.is_none() {
        data.insert(y("provider"), y("bigquery"));
    }

    data
}

/// Generate a metric card for single-value results.
fn generate_metric_card(
    column_name: &str,
    sql_text: &str,
    datasource_slug: Option<&str>,
    datasource_type: Option<&str>,
) -> String {
    let mut spec = serde_yaml::Mapping::new();
    spec.insert(y("type"), y("chart"));
    spec.insert(y("version"), serde_yaml::Value::Number(1.into()));
    spec.insert(y("title"), y(column_name));
    spec.insert(
        y("data"),
        serde_yaml::Value::Mapping(build_data_section(
            sql_text,
            datasource_slug,
            datasource_type,
        )),
    );

    let mut vis = serde_yaml::Mapping::new();
    vis.insert(y("type"), y("metric"));
    vis.insert(y("value"), y(column_name));
    vis.insert(y("label"), y(column_name));
    spec.insert(y("visualize"), serde_yaml::Value::Mapping(vis));

    serde_yaml::to_string(&spec).unwrap_or_default()
}

/// Generate a table fallback.
fn generate_table_fallback(
    sql_text: &str,
    columns: &[String],
    datasource_slug: Option<&str>,
    datasource_type: Option<&str>,
) -> String {
    let mut spec = serde_yaml::Mapping::new();
    spec.insert(y("type"), y("chart"));
    spec.insert(y("version"), serde_yaml::Value::Number(1.into()));
    spec.insert(y("title"), y("Query Results"));
    spec.insert(
        y("data"),
        serde_yaml::Value::Mapping(build_data_section(
            sql_text,
            datasource_slug,
            datasource_type,
        )),
    );

    let mut vis = serde_yaml::Mapping::new();
    vis.insert(y("type"), y("table"));
    vis.insert(
        y("columns"),
        serde_yaml::Value::Sequence(columns.iter().map(|c| y(c)).collect()),
    );
    spec.insert(y("visualize"), serde_yaml::Value::Mapping(vis));

    serde_yaml::to_string(&spec).unwrap_or_default()
}

/// Shorthand: create a `serde_yaml::Value::String`.
fn y(s: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(s.to_string())
}

// ---------------------------------------------------------------------------
// Rule-based generation
// ---------------------------------------------------------------------------

fn generate_with_rules(
    sql_text: &str,
    columns: &[String],
    rows: &[Vec<serde_json::Value>],
    datasource_slug: Option<&str>,
    datasource_type: Option<&str>,
) -> Result<String, kyomi_core::Error> {
    if columns.is_empty() {
        return Err(kyomi_core::Error::BadRequest(
            "No columns provided".to_string(),
        ));
    }

    let analyses: Vec<ColumnAnalysis> = columns
        .iter()
        .map(|col| analyze_column(col, rows, columns))
        .collect();

    // Single value → metric card
    if columns.len() == 1 && rows.len() == 1 {
        return Ok(generate_metric_card(
            &columns[0],
            sql_text,
            datasource_slug,
            datasource_type,
        ));
    }

    let chart_type = infer_chart_type(&analyses);
    let (x_axis, mut y_axis) = infer_axes(&analyses, columns);

    // Same column for both axes → try picking a different y
    if x_axis == y_axis && columns.len() > 1 {
        if let Some(alt) = analyses
            .iter()
            .find(|a| a.name != x_axis && a.is_numeric)
            .or_else(|| analyses.iter().find(|a| a.name != x_axis))
        {
            y_axis = &alt.name;
        } else {
            return Ok(generate_table_fallback(
                sql_text,
                columns,
                datasource_slug,
                datasource_type,
            ));
        }
    }

    // Build the full spec
    let title = format!("{y_axis} by {x_axis}");

    let mut spec = serde_yaml::Mapping::new();
    spec.insert(y("type"), y("chart"));
    spec.insert(y("version"), serde_yaml::Value::Number(1.into()));
    spec.insert(y("title"), y(&title));
    spec.insert(
        y("data"),
        serde_yaml::Value::Mapping(build_data_section(
            sql_text,
            datasource_slug,
            datasource_type,
        )),
    );

    let mut vis = serde_yaml::Mapping::new();
    vis.insert(y("type"), y(chart_type));
    vis.insert(y("columns"), y(x_axis));
    vis.insert(y("rows"), y(y_axis));
    spec.insert(y("visualize"), serde_yaml::Value::Mapping(vis));

    Ok(serde_yaml::to_string(&spec).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// POST /chart/generate
// ---------------------------------------------------------------------------

async fn generate_chart(
    _user: AuthUser,
    Json(request): Json<ChartGenerateRequest>,
) -> Result<Json<ChartGenerateResponse>, kyomi_core::Error> {
    // Reject AI path — only rule-based generation is supported in Rust
    if request.user_context.is_some() {
        return Err(kyomi_core::Error::BadRequest(
            "AI-based chart generation is not supported in this endpoint. \
             Omit user_context to use rule-based generation."
                .to_string(),
        ));
    }

    let chart_yaml = generate_with_rules(
        &request.sql_text,
        &request.columns,
        &request.rows,
        request.datasource_slug.as_deref(),
        request.datasource_type.as_deref(),
    )?;

    tracing::info!("Generated chart via rules");

    Ok(Json(ChartGenerateResponse { chart_yaml }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- analyze_column ---------------------------------------------------

    #[test]
    fn analyze_numeric_column() {
        let columns = vec!["amount".to_string()];
        let rows = vec![
            vec![json!(10)],
            vec![json!(20.5)],
            vec![json!(30)],
        ];
        let analysis = analyze_column("amount", &rows, &columns);
        assert!(analysis.is_numeric);
        assert!(!analysis.is_date);
        assert_eq!(analysis.cardinality, 3);
    }

    #[test]
    fn analyze_date_column() {
        let columns = vec!["created_at".to_string()];
        let rows = vec![
            vec![json!("2024-01-01")],
            vec![json!("2024-01-02")],
            vec![json!("2024-01-03")],
        ];
        let analysis = analyze_column("created_at", &rows, &columns);
        assert!(!analysis.is_numeric);
        assert!(analysis.is_date);
        assert_eq!(analysis.cardinality, 3);
    }

    #[test]
    fn analyze_datetime_with_timezone() {
        let columns = vec!["ts".to_string()];
        let rows = vec![
            vec![json!("2024-06-15T10:30:00+00:00")],
            vec![json!("2024-06-16T12:00:00Z")],
        ];
        let analysis = analyze_column("ts", &rows, &columns);
        assert!(analysis.is_date);
    }

    #[test]
    fn analyze_category_column() {
        let columns = vec!["status".to_string()];
        let rows = vec![
            vec![json!("active")],
            vec![json!("inactive")],
            vec![json!("active")],
        ];
        let analysis = analyze_column("status", &rows, &columns);
        assert!(!analysis.is_numeric);
        assert!(!analysis.is_date);
        assert_eq!(analysis.cardinality, 2);
    }

    #[test]
    fn analyze_missing_column() {
        let columns = vec!["a".to_string()];
        let rows = vec![vec![json!(1)]];
        let analysis = analyze_column("nonexistent", &rows, &columns);
        assert!(!analysis.is_numeric);
        assert!(!analysis.is_date);
        assert_eq!(analysis.cardinality, 0);
    }

    #[test]
    fn analyze_all_null_values() {
        let columns = vec!["x".to_string()];
        let rows = vec![vec![json!(null)], vec![json!(null)]];
        let analysis = analyze_column("x", &rows, &columns);
        assert!(!analysis.is_numeric);
        assert!(!analysis.is_date);
        assert_eq!(analysis.cardinality, 0);
    }

    // -- infer_chart_type --------------------------------------------------

    #[test]
    fn infer_line_for_date_column() {
        let analyses = vec![
            ColumnAnalysis { name: "date".into(), is_numeric: false, is_date: true, cardinality: 10 },
            ColumnAnalysis { name: "value".into(), is_numeric: true, is_date: false, cardinality: 10 },
        ];
        assert_eq!(infer_chart_type(&analyses), "line");
    }

    #[test]
    fn infer_bar_for_low_cardinality_category() {
        let analyses = vec![
            ColumnAnalysis { name: "region".into(), is_numeric: false, is_date: false, cardinality: 5 },
            ColumnAnalysis { name: "revenue".into(), is_numeric: true, is_date: false, cardinality: 5 },
        ];
        assert_eq!(infer_chart_type(&analyses), "bar");
    }

    #[test]
    fn infer_table_for_high_cardinality() {
        let analyses = vec![
            ColumnAnalysis { name: "id".into(), is_numeric: false, is_date: false, cardinality: 100 },
            ColumnAnalysis { name: "value".into(), is_numeric: true, is_date: false, cardinality: 50 },
        ];
        assert_eq!(infer_chart_type(&analyses), "table");
    }

    #[test]
    fn infer_table_for_all_numeric() {
        let analyses = vec![
            ColumnAnalysis { name: "x".into(), is_numeric: true, is_date: false, cardinality: 50 },
            ColumnAnalysis { name: "y".into(), is_numeric: true, is_date: false, cardinality: 50 },
        ];
        assert_eq!(infer_chart_type(&analyses), "table");
    }

    // -- infer_axes --------------------------------------------------------

    #[test]
    fn infer_axes_date_and_numeric() {
        let analyses = vec![
            ColumnAnalysis { name: "date".into(), is_numeric: false, is_date: true, cardinality: 10 },
            ColumnAnalysis { name: "sales".into(), is_numeric: true, is_date: false, cardinality: 10 },
        ];
        let columns = vec!["date".to_string(), "sales".to_string()];
        let (x, y) = infer_axes(&analyses, &columns);
        assert_eq!(x, "date");
        assert_eq!(y, "sales");
    }

    #[test]
    fn infer_axes_category_and_numeric() {
        let analyses = vec![
            ColumnAnalysis { name: "region".into(), is_numeric: false, is_date: false, cardinality: 5 },
            ColumnAnalysis { name: "count".into(), is_numeric: true, is_date: false, cardinality: 5 },
        ];
        let columns = vec!["region".to_string(), "count".to_string()];
        let (x, y) = infer_axes(&analyses, &columns);
        assert_eq!(x, "region");
        assert_eq!(y, "count");
    }

    #[test]
    fn infer_axes_all_numeric_uses_first_and_second() {
        let analyses = vec![
            ColumnAnalysis { name: "a".into(), is_numeric: true, is_date: false, cardinality: 10 },
            ColumnAnalysis { name: "b".into(), is_numeric: true, is_date: false, cardinality: 10 },
        ];
        let columns = vec!["a".to_string(), "b".to_string()];
        let (x, y) = infer_axes(&analyses, &columns);
        // x = first (since no date/non-numeric), y = second numeric != x
        assert_eq!(x, "a");
        assert_eq!(y, "b");
    }

    // -- generate_with_rules (integration) ---------------------------------

    #[test]
    fn rules_single_value_produces_metric() {
        let yaml = generate_with_rules(
            "SELECT count(*) as total FROM users",
            &["total".to_string()],
            &[vec![json!(42)]],
            Some("my-db"),
            None,
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let vis = &parsed["visualize"];
        assert_eq!(vis["type"].as_str().unwrap(), "metric");
        assert_eq!(vis["value"].as_str().unwrap(), "total");
    }

    #[test]
    fn rules_date_numeric_produces_line() {
        let yaml = generate_with_rules(
            "SELECT date, revenue FROM sales",
            &["date".to_string(), "revenue".to_string()],
            &[
                vec![json!("2024-01-01"), json!(100)],
                vec![json!("2024-01-02"), json!(200)],
            ],
            None,
            Some("postgres"),
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed["visualize"]["type"].as_str().unwrap(), "line");
        assert_eq!(parsed["visualize"]["columns"].as_str().unwrap(), "date");
        assert_eq!(parsed["visualize"]["rows"].as_str().unwrap(), "revenue");
    }

    #[test]
    fn rules_category_numeric_produces_bar() {
        let yaml = generate_with_rules(
            "SELECT region, count FROM stats",
            &["region".to_string(), "count".to_string()],
            &[
                vec![json!("US"), json!(50)],
                vec![json!("EU"), json!(30)],
            ],
            Some("prod-pg"),
            None,
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed["visualize"]["type"].as_str().unwrap(), "bar");
    }

    #[test]
    fn rules_empty_columns_returns_error() {
        let result = generate_with_rules("SELECT 1", &[], &[], None, None);
        assert!(result.is_err());
    }

    #[test]
    fn rules_same_axis_fallback() {
        // Single non-numeric column with multiple rows — x and y would both pick "name"
        // but should resolve to table fallback since there's no second column
        let yaml = generate_with_rules(
            "SELECT name FROM users",
            &["name".to_string()],
            &[
                vec![json!("alice")],
                vec![json!("bob")],
            ],
            None,
            None,
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        // Single column, both axes would be "name" — same x/y path,
        // but columns.len() == 1, so we skip the dedup branch.
        // With 1 column we just get x=name, y=name (the Python behavior).
        assert_eq!(parsed["visualize"]["columns"].as_str().unwrap(), "name");
    }

    #[test]
    fn rules_two_columns_same_axis_picks_different_y() {
        // Two non-numeric columns — x picks first, y would also pick first (non-numeric),
        // but should resolve to second
        let yaml = generate_with_rules(
            "SELECT city, country FROM locations",
            &["city".to_string(), "country".to_string()],
            &[
                vec![json!("NYC"), json!("US")],
                vec![json!("London"), json!("UK")],
            ],
            None,
            None,
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let x = parsed["visualize"]["columns"].as_str().unwrap();
        let y = parsed["visualize"]["rows"].as_str().unwrap();
        assert_ne!(x, y, "x and y should be different columns");
    }

    #[test]
    fn rules_datasource_slug_in_data() {
        let yaml = generate_with_rules(
            "SELECT 1 as n",
            &["n".to_string()],
            &[vec![json!(1)]],
            Some("my-warehouse"),
            None,
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed["data"]["datasource"].as_str().unwrap(), "my-warehouse");
    }

    #[test]
    fn rules_no_datasource_defaults_to_bigquery() {
        let yaml = generate_with_rules(
            "SELECT 1 as n",
            &["n".to_string()],
            &[vec![json!(1)]],
            None,
            None,
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed["data"]["provider"].as_str().unwrap(), "bigquery");
    }

    // -- request deserialization -------------------------------------------

    #[test]
    fn request_deserializes_minimal() {
        let json = json!({
            "sql_text": "SELECT 1",
            "columns": ["a"],
            "rows": [[1]],
        });
        let req: ChartGenerateRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.sql_text, "SELECT 1");
        assert!(req.user_context.is_none());
        assert!(req.datasource_slug.is_none());
    }

    #[test]
    fn request_deserializes_full() {
        let json = json!({
            "sql_text": "SELECT * FROM t",
            "columns": ["a", "b"],
            "rows": [[1, 2]],
            "user_context": null,
            "datasource_slug": "prod-pg",
            "datasource_type": "postgres",
        });
        let req: ChartGenerateRequest = serde_json::from_value(json).unwrap();
        assert!(req.user_context.is_none());
        assert_eq!(req.datasource_slug.as_deref(), Some("prod-pg"));
    }

    #[test]
    fn response_serializes() {
        let resp = ChartGenerateResponse {
            chart_yaml: "type: chart\n".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["chart_yaml"], "type: chart\n");
    }
}
