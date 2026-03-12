// SPDX-License-Identifier: AGPL-3.0-or-later

//! LLM usage tracking REST endpoint.
//!
//! Provides aggregated API usage statistics:
//! - `GET /llm` — query LLM usage with optional grouping and filtering

use axum::{routing::get, Json, Router};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

use kyomi_auth::middleware::AuthUser;

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the `/usage` router.
pub fn routes() -> Router<AppState> {
    Router::new().route("/llm", get(get_llm_usage))
}

// ===========================================================================
// Request / Response Types
// ===========================================================================

#[derive(Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct UsageQuery {
    /// Number of days to query (default: 30).
    #[serde(default = "default_days")]
    days: i64,
    /// Group results by: `component`, `model`, or `date`.
    group_by: Option<String>,
    /// Filter by specific component (e.g. `sql_autocomplete`).
    component: Option<String>,
}

fn default_days() -> i64 {
    30
}

/// Polymorphic usage record — variant depends on `group_by` parameter.
#[derive(Serialize)]
#[cfg_attr(test, derive(Deserialize, Debug))]
#[serde(untagged)]
enum UsageRecord {
    ByComponent {
        component: String,
        input_tokens: i64,
        output_tokens: i64,
        total_tokens: i64,
        cost_estimate: f64,
        request_count: i64,
    },
    ByModel {
        model: String,
        input_tokens: i64,
        output_tokens: i64,
        total_tokens: i64,
        cost_estimate: f64,
        request_count: i64,
    },
    ByDate {
        date: String,
        input_tokens: i64,
        output_tokens: i64,
        total_tokens: i64,
        cost_estimate: f64,
        request_count: i64,
    },
    Ungrouped {
        timestamp: String,
        component: String,
        model: String,
        input_tokens: i32,
        output_tokens: i32,
        total_tokens: i32,
        cost_estimate: f64,
        provider: String,
    },
}

// ===========================================================================
// Endpoint Handler
// ===========================================================================

/// GET /llm — Query LLM usage statistics for the current workspace.
///
/// Supports grouping by component, model, or date. Without grouping,
/// returns individual usage records (up to 1000).
async fn get_llm_usage(
    user: AuthUser,
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Query(params): axum::extract::Query<UsageQuery>,
) -> Result<Json<Vec<UsageRecord>>, kyomi_core::Error> {
    let workspace_id = user
        .workspace
        .workspace_id
        .as_deref()
        .ok_or_else(|| kyomi_core::Error::BadRequest("Workspace context required".into()))?;
    let end_date = Utc::now();
    let start_date = end_date - Duration::days(params.days);

    // Validate group_by if provided
    if let Some(ref gb) = params.group_by
        && !["component", "model", "date"].contains(&gb.as_str())
    {
        return Err(kyomi_core::Error::BadRequest(
            "group_by must be 'component', 'model', or 'date'".into(),
        ));
    }

    let results = match params.group_by.as_deref() {
        Some("component") => {
            query_grouped_by_component(
                &state.db,
                workspace_id,
                start_date,
                end_date,
                params.component.as_deref(),
            )
            .await?
        }
        Some("model") => {
            query_grouped_by_model(
                &state.db,
                workspace_id,
                start_date,
                end_date,
                params.component.as_deref(),
            )
            .await?
        }
        Some("date") => {
            query_grouped_by_date(
                &state.db,
                workspace_id,
                start_date,
                end_date,
                params.component.as_deref(),
            )
            .await?
        }
        _ => {
            query_ungrouped(
                &state.db,
                workspace_id,
                start_date,
                end_date,
                params.component.as_deref(),
            )
            .await?
        }
    };

    Ok(Json(results))
}

// ===========================================================================
// Query helpers
// ===========================================================================

/// Shared row type for grouped usage queries.
#[derive(sqlx::FromRow)]
struct GroupedUsageRow {
    group_key: String,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    total_tokens: Option<i64>,
    cost_estimate: Option<f64>,
    request_count: i64,
}

async fn query_grouped_by_component(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
    component_filter: Option<&str>,
) -> Result<Vec<UsageRecord>, kyomi_core::Error> {
    let is_pg = db.is_postgres();
    let cast_bigint = if is_pg { "::bigint" } else { "" };
    let param_cast = if is_pg { "$4::text" } else { "$4" };
    let sql = format!(
        "SELECT \
            COALESCE(component, 'unknown') AS group_key, \
            SUM(input_tokens){cast_bigint} AS input_tokens, \
            SUM(output_tokens){cast_bigint} AS output_tokens, \
            SUM(total_tokens){cast_bigint} AS total_tokens, \
            SUM(cost_estimate) AS cost_estimate, \
            COUNT(*){cast_bigint} AS request_count \
         FROM api_usage_log \
         WHERE workspace_id = $1 \
           AND timestamp >= $2 \
           AND timestamp <= $3 \
           AND ({param_cast} IS NULL OR component = $4) \
         GROUP BY component"
    );
    let start_str = start.to_rfc3339();
    let end_str = end.to_rfc3339();
    let rows = kyomi_core::db_fetch_all!(
        db, GroupedUsageRow, &sql,
        workspace_id, &start_str, &end_str, &component_filter
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("Failed to query usage: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|r| UsageRecord::ByComponent {
            component: r.group_key,
            input_tokens: r.input_tokens.unwrap_or(0),
            output_tokens: r.output_tokens.unwrap_or(0),
            total_tokens: r.total_tokens.unwrap_or(0),
            cost_estimate: r.cost_estimate.unwrap_or(0.0),
            request_count: r.request_count,
        })
        .collect())
}

async fn query_grouped_by_model(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
    component_filter: Option<&str>,
) -> Result<Vec<UsageRecord>, kyomi_core::Error> {
    let is_pg = db.is_postgres();
    let cast_bigint = if is_pg { "::bigint" } else { "" };
    let param_cast = if is_pg { "$4::text" } else { "$4" };
    let sql = format!(
        "SELECT \
            model AS group_key, \
            SUM(input_tokens){cast_bigint} AS input_tokens, \
            SUM(output_tokens){cast_bigint} AS output_tokens, \
            SUM(total_tokens){cast_bigint} AS total_tokens, \
            SUM(cost_estimate) AS cost_estimate, \
            COUNT(*){cast_bigint} AS request_count \
         FROM api_usage_log \
         WHERE workspace_id = $1 \
           AND timestamp >= $2 \
           AND timestamp <= $3 \
           AND ({param_cast} IS NULL OR component = $4) \
         GROUP BY model"
    );
    let start_str = start.to_rfc3339();
    let end_str = end.to_rfc3339();
    let rows = kyomi_core::db_fetch_all!(
        db, GroupedUsageRow, &sql,
        workspace_id, &start_str, &end_str, &component_filter
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("Failed to query usage: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|r| UsageRecord::ByModel {
            model: r.group_key,
            input_tokens: r.input_tokens.unwrap_or(0),
            output_tokens: r.output_tokens.unwrap_or(0),
            total_tokens: r.total_tokens.unwrap_or(0),
            cost_estimate: r.cost_estimate.unwrap_or(0.0),
            request_count: r.request_count,
        })
        .collect())
}

async fn query_grouped_by_date(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
    component_filter: Option<&str>,
) -> Result<Vec<UsageRecord>, kyomi_core::Error> {
    let is_pg = db.is_postgres();
    let cast_bigint = if is_pg { "::bigint" } else { "" };
    let date_text = if is_pg { "DATE(timestamp)::text" } else { "DATE(timestamp)" };
    let param_cast = if is_pg { "$4::text" } else { "$4" };
    let sql = format!(
        "SELECT \
            {date_text} AS group_key, \
            SUM(input_tokens){cast_bigint} AS input_tokens, \
            SUM(output_tokens){cast_bigint} AS output_tokens, \
            SUM(total_tokens){cast_bigint} AS total_tokens, \
            SUM(cost_estimate) AS cost_estimate, \
            COUNT(*){cast_bigint} AS request_count \
         FROM api_usage_log \
         WHERE workspace_id = $1 \
           AND timestamp >= $2 \
           AND timestamp <= $3 \
           AND ({param_cast} IS NULL OR component = $4) \
         GROUP BY DATE(timestamp) \
         ORDER BY DATE(timestamp)"
    );
    let start_str = start.to_rfc3339();
    let end_str = end.to_rfc3339();
    let rows = kyomi_core::db_fetch_all!(
        db, GroupedUsageRow, &sql,
        workspace_id, &start_str, &end_str, &component_filter
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("Failed to query usage: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|r| UsageRecord::ByDate {
            date: r.group_key,
            input_tokens: r.input_tokens.unwrap_or(0),
            output_tokens: r.output_tokens.unwrap_or(0),
            total_tokens: r.total_tokens.unwrap_or(0),
            cost_estimate: r.cost_estimate.unwrap_or(0.0),
            request_count: r.request_count,
        })
        .collect())
}

#[derive(sqlx::FromRow)]
struct UngroupedUsageRow {
    timestamp: chrono::DateTime<Utc>,
    component: Option<String>,
    model: String,
    input_tokens: i32,
    output_tokens: i32,
    total_tokens: i32,
    cost_estimate: Option<f64>,
    provider: String,
}

async fn query_ungrouped(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
    component_filter: Option<&str>,
) -> Result<Vec<UsageRecord>, kyomi_core::Error> {
    let is_pg = db.is_postgres();
    let param_cast = if is_pg { "$4::text" } else { "$4" };
    let sql = format!(
        "SELECT timestamp, component, model, input_tokens, output_tokens, \
         total_tokens, cost_estimate, provider \
         FROM api_usage_log \
         WHERE workspace_id = $1 \
           AND timestamp >= $2 \
           AND timestamp <= $3 \
           AND ({param_cast} IS NULL OR component = $4) \
         ORDER BY timestamp DESC \
         LIMIT 1000"
    );
    let start_str = start.to_rfc3339();
    let end_str = end.to_rfc3339();
    let rows = kyomi_core::db_fetch_all!(
        db, UngroupedUsageRow, &sql,
        workspace_id, &start_str, &end_str, &component_filter
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("Failed to query usage: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|r| UsageRecord::Ungrouped {
            timestamp: r.timestamp.to_rfc3339(),
            component: r.component.unwrap_or_else(|| "unknown".into()),
            model: r.model,
            input_tokens: r.input_tokens,
            output_tokens: r.output_tokens,
            total_tokens: r.total_tokens,
            cost_estimate: r.cost_estimate.unwrap_or(0.0),
            provider: r.provider,
        })
        .collect())
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // UsageQuery deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn usage_query_defaults() {
        let json = json!({});
        let q: UsageQuery = serde_json::from_value(json).unwrap();
        assert_eq!(q.days, 30);
        assert!(q.group_by.is_none());
        assert!(q.component.is_none());
    }

    #[test]
    fn usage_query_with_all_fields() {
        let json = json!({"days": 7, "group_by": "model", "component": "chat_agent"});
        let q: UsageQuery = serde_json::from_value(json).unwrap();
        assert_eq!(q.days, 7);
        assert_eq!(q.group_by.as_deref(), Some("model"));
        assert_eq!(q.component.as_deref(), Some("chat_agent"));
    }

    // -----------------------------------------------------------------------
    // UsageRecord serialization
    // -----------------------------------------------------------------------

    #[test]
    fn usage_record_by_component_serializes() {
        let record = UsageRecord::ByComponent {
            component: "chat_agent".into(),
            input_tokens: 1000,
            output_tokens: 500,
            total_tokens: 1500,
            cost_estimate: 0.05,
            request_count: 10,
        };
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["component"], "chat_agent");
        assert_eq!(json["input_tokens"], 1000);
        assert_eq!(json["request_count"], 10);
        // Should NOT have model/date/timestamp keys
        assert!(json.get("model").is_none());
        assert!(json.get("date").is_none());
    }

    #[test]
    fn usage_record_by_model_serializes() {
        let record = UsageRecord::ByModel {
            model: "claude-sonnet-4-5-20250929".into(),
            input_tokens: 2000,
            output_tokens: 800,
            total_tokens: 2800,
            cost_estimate: 0.12,
            request_count: 5,
        };
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["model"], "claude-sonnet-4-5-20250929");
        assert!(json.get("component").is_none());
    }

    #[test]
    fn usage_record_by_date_serializes() {
        let record = UsageRecord::ByDate {
            date: "2025-01-15".into(),
            input_tokens: 5000,
            output_tokens: 2000,
            total_tokens: 7000,
            cost_estimate: 0.30,
            request_count: 25,
        };
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["date"], "2025-01-15");
        assert!(json.get("model").is_none());
    }

    #[test]
    fn usage_record_ungrouped_serializes() {
        let record = UsageRecord::Ungrouped {
            timestamp: "2025-01-15T10:30:00+00:00".into(),
            component: "sql_autocomplete".into(),
            model: "claude-haiku-4-5-20251001".into(),
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            cost_estimate: 0.001,
            provider: "anthropic".into(),
        };
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["timestamp"], "2025-01-15T10:30:00+00:00");
        assert_eq!(json["provider"], "anthropic");
        // Should NOT have request_count (that's grouped-only)
        assert!(json.get("request_count").is_none());
    }

    #[test]
    fn usage_record_round_trip() {
        let record = UsageRecord::ByComponent {
            component: "test".into(),
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cost_estimate: 0.0,
            request_count: 0,
        };
        let json_str = serde_json::to_string(&record).unwrap();
        let deserialized: UsageRecord = serde_json::from_str(&json_str).unwrap();
        // Untagged enum deserializes based on fields present
        match deserialized {
            UsageRecord::ByComponent { component, .. } => assert_eq!(component, "test"),
            _ => panic!("Expected ByComponent variant"),
        }
    }
}
