// SPDX-License-Identifier: AGPL-3.0-or-later

//! Feedback submission and retrieval endpoints.
//!
//! Wire-compatible with Python's `routers/feedback.py`.
//! All endpoints require authentication (AuthUser extractor).
//!
//! `submit_feedback` is a thin adapter over
//! [`kyomi_auth::feedback_service::submit_feedback`] — business logic
//! (validation, persistence, Linear/Slack/email notifications) lives in
//! the shared service so the Leptos server function can reuse it.

use axum::{
    extract::State,
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use kyomi_auth::middleware::AuthUser;

use crate::state::AppState;

/// Build the feedback router.
///
/// Mounted under `/api/v1/feedback` so the full paths are:
/// - `POST /api/v1/feedback` — submit feedback
/// - `GET  /api/v1/feedback` — list user's feedback
pub fn routes() -> Router<AppState> {
    Router::new().route("/", post(submit_feedback).get(list_feedback))
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct FeedbackRequest {
    /// Feedback type: "bug", "feature", or "question".
    #[serde(rename = "type")]
    feedback_type: String,

    /// User's description of the issue (min 10 chars after trim).
    description: String,

    /// Optional base64-encoded screenshot.
    screenshot: Option<String>,

    /// Whether to include technical context.
    #[serde(default = "default_true")]
    include_context: bool,

    /// Optional JSON context blob.
    context: Option<serde_json::Value>,

    /// Optional workspace scope.
    workspace_id: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct FeedbackResponse {
    status: String,
    feedback_id: String,
    message: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(Deserialize))]
struct FeedbackListItem {
    id: String,
    #[serde(rename = "type")]
    feedback_type: String,
    description: String,
    status: String,
    created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// POST /feedback
// ---------------------------------------------------------------------------

async fn submit_feedback(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<FeedbackRequest>,
) -> Result<Json<FeedbackResponse>, kyomi_core::Error> {
    let input = kyomi_auth::feedback_service::FeedbackInput {
        feedback_type: data.feedback_type,
        description: data.description,
        screenshot: data.screenshot,
        include_context: data.include_context,
        context: data.context.unwrap_or(serde_json::json!({})),
        workspace_id: data.workspace_id,
    };

    let result = kyomi_auth::feedback_service::submit_feedback(
        &state.db,
        &state.config,
        &user,
        input,
    )
    .await?;

    Ok(Json(FeedbackResponse {
        status: result.status,
        feedback_id: result.feedback_id,
        message: result.message,
    }))
}

// ---------------------------------------------------------------------------
// GET /feedback
// ---------------------------------------------------------------------------

async fn list_feedback(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<FeedbackListItem>>, kyomi_core::Error> {
    if state.config.self_hosted || state.config.is_personal() {
        return Err(kyomi_core::Error::NotFound(
            "Feedback is not available in this deployment mode.".into(),
        ));
    }

    #[derive(sqlx::FromRow)]
    struct FeedbackRow {
        id: String,
        feedback_type: String,
        description: String,
        status: String,
        created_at: chrono::DateTime<Utc>,
    }

    let is_pg = state.db.is_postgres();
    let left_fn = if is_pg { "LEFT(description, 100)" } else { "SUBSTR(description, 1, 100)" };
    let sql = format!(
        "SELECT id, type AS feedback_type, {left_fn} AS description, status, created_at \
         FROM feedback \
         WHERE user_id = $1 \
         ORDER BY created_at DESC \
         LIMIT 20"
    );
    let rows = kyomi_core::db_fetch_all!(&state.db, FeedbackRow, &sql, &user.user_id)?;

    let items: Vec<FeedbackListItem> = rows
        .into_iter()
        .map(|row| FeedbackListItem {
            id: row.id,
            feedback_type: row.feedback_type,
            description: row.description,
            status: row.status,
            created_at: row.created_at,
        })
        .collect();

    Ok(Json(items))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn feedback_request_deserializes() {
        let json = json!({
            "type": "bug",
            "description": "Something is broken in the dashboard",
        });
        let req: FeedbackRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.feedback_type, "bug");
        assert!(req.include_context); // default true
        assert!(req.screenshot.is_none());
        assert!(req.context.is_none());
    }

    #[test]
    fn feedback_request_with_all_fields() {
        let json = json!({
            "type": "feature",
            "description": "Please add dark mode support",
            "screenshot": "base64data==",
            "include_context": false,
            "context": {"browser": "Chrome", "os": "macOS"}
        });
        let req: FeedbackRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.feedback_type, "feature");
        assert!(!req.include_context);
        assert_eq!(req.screenshot.as_deref(), Some("base64data=="));
        assert!(req.context.is_some());
    }

    #[test]
    fn feedback_request_fails_without_required_fields() {
        let json = json!({"type": "bug"});
        assert!(serde_json::from_value::<FeedbackRequest>(json).is_err());

        let json = json!({"description": "test"});
        assert!(serde_json::from_value::<FeedbackRequest>(json).is_err());
    }

    #[test]
    fn feedback_response_serializes() {
        let resp = FeedbackResponse {
            status: "received".into(),
            feedback_id: "fb-abc12345".into(),
            message: "Thank you!".into(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "received");
        assert_eq!(json["feedback_id"], "fb-abc12345");
    }

    #[test]
    fn feedback_response_round_trip() {
        let resp = FeedbackResponse {
            status: "received".into(),
            feedback_id: "fb-12345678".into(),
            message: "Thanks".into(),
        };
        let json_str = serde_json::to_string(&resp).unwrap();
        let deserialized: FeedbackResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.feedback_id, "fb-12345678");
    }

    #[test]
    fn feedback_list_item_serializes_type_field() {
        let item = FeedbackListItem {
            id: "fb-test1234".into(),
            feedback_type: "bug".into(),
            description: "Test bug report".into(),
            status: "new".into(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_value(&item).unwrap();
        // Should serialize as "type", not "feedback_type"
        assert_eq!(json["type"], "bug");
        assert!(json.get("feedback_type").is_none());
        assert_eq!(json["status"], "new");
    }

    #[test]
    fn feedback_list_item_round_trip() {
        let item = FeedbackListItem {
            id: "fb-round123".into(),
            feedback_type: "feature".into(),
            description: "A feature request description".into(),
            status: "reviewed".into(),
            created_at: Utc::now(),
        };
        let json_str = serde_json::to_string(&item).unwrap();
        let deserialized: FeedbackListItem = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.id, "fb-round123");
        assert_eq!(deserialized.feedback_type, "feature");
    }
}
