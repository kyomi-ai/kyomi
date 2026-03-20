// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chart context retrieval endpoint for "Continue in Kyomi" deep-links from MCP.
//!
//! When a chart is rendered via the MCP server, its spec is stored in Redis with a
//! 30-day TTL. This endpoint lets the frontend fetch that context to bootstrap a
//! conversation.
//!
//! Wire-compatible with Python's `routers/chart_context.py`.

use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde::Serialize;

use kyomi_auth::middleware::AuthUser;

use crate::state::AppState;

/// Build the chart-context router.
///
/// Mounted under `/api/v1/chart-context` so the full path is:
/// - `GET /api/v1/chart-context/{context_id}` — retrieve stored chart context
pub fn routes() -> Router<AppState> {
    Router::new().route("/{context_id}", get(get_chart_context))
}

// ---------------------------------------------------------------------------
// Response type
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(serde::Deserialize))]
struct ChartContextResponse {
    spec: Option<serde_json::Value>,
    title: Option<serde_json::Value>,
    #[serde(rename = "chartMarkdown")]
    chart_markdown: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// GET /chart-context/{context_id}
// ---------------------------------------------------------------------------

/// Retrieve a stored chart context by ID.
///
/// Chart contexts are created when `render_chart` produces an interactive chart
/// via MCP. The context includes the resolved spec and ChartML markdown for
/// bootstrapping a conversation in the Kyomi web app.
///
/// Returns 404 if the context has expired (30-day TTL) or doesn't exist.
async fn get_chart_context(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(context_id): Path<String>,
) -> Result<Json<ChartContextResponse>, kyomi_core::Error> {
    let key = format!("chart:context:{context_id}");

    let value: Option<String> = state.kv.get(&key).await?;

    let raw = value.ok_or_else(|| {
        kyomi_core::Error::NotFound("Chart context not found or expired.".into())
    })?;

    let data: serde_json::Value = serde_json::from_str(&raw)?;

    Ok(Json(ChartContextResponse {
        spec: data.get("spec").cloned(),
        title: data.get("title").cloned(),
        chart_markdown: data.get("chartMarkdown").cloned(),
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chart_context_response_serializes_with_camel_case() {
        let resp = ChartContextResponse {
            spec: Some(json!({"type": "bar"})),
            title: Some(json!("My Chart")),
            chart_markdown: Some(json!("```chartml\ntype: bar\n```")),
        };
        let json_val = serde_json::to_value(&resp).unwrap();
        // Should use camelCase "chartMarkdown", not snake_case
        assert!(json_val.get("chartMarkdown").is_some());
        assert!(json_val.get("chart_markdown").is_none());
        assert_eq!(json_val["spec"]["type"], "bar");
        assert_eq!(json_val["title"], "My Chart");
    }

    #[test]
    fn chart_context_response_handles_null_fields() {
        let resp = ChartContextResponse {
            spec: None,
            title: None,
            chart_markdown: None,
        };
        let json_val = serde_json::to_value(&resp).unwrap();
        assert!(json_val["spec"].is_null());
        assert!(json_val["title"].is_null());
        assert!(json_val["chartMarkdown"].is_null());
    }

    #[test]
    fn chart_context_response_round_trip() {
        let resp = ChartContextResponse {
            spec: Some(json!({"visualize": [{"mark": "bar"}]})),
            title: Some(json!("Revenue Trend")),
            chart_markdown: Some(json!("```chartml\nvisualze:\n  - mark: bar\n```")),
        };
        let json_str = serde_json::to_string(&resp).unwrap();
        let deserialized: ChartContextResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.spec, resp.spec);
        assert_eq!(deserialized.title, resp.title);
        assert_eq!(deserialized.chart_markdown, resp.chart_markdown);
    }
}
