// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server function for submitting user feedback.
//!
//! Replicates the storage logic from `apps/server/src/routes/feedback.rs`
//! (the `submit_feedback` handler) using the same DB insert pattern.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// Response from the feedback submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackResponse {
    pub status: String,
    pub feedback_id: String,
    pub message: String,
}

/// Submit user feedback directly to the database.
///
/// Mirrors the `POST /api/v1/feedback` route handler — validates input,
/// generates a feedback ID, and inserts into the `feedback` table.
///
/// **Known gap**: does NOT trigger Slack/email notifications. The REST route's
/// background task fires those, but extracting notification logic into a shared
/// service is out of scope here. Tracked for follow-up.
#[server(prefix = "/leptos-api")]
pub async fn submit_feedback(
    feedback_type: String,
    description: String,
    include_context: bool,
    context: String,
    screenshot: Option<String>,
) -> Result<FeedbackResponse, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    // Validate feedback type — must match the REST route's allowed values
    if !["bug", "feature", "question"].contains(&feedback_type.as_str()) {
        return Err(ServerFnError::new(
            "Invalid feedback type. Must be 'bug', 'feature', or 'question'.",
        ));
    }

    // Validate description length — same 10-char minimum as the REST route
    let description = description.trim().to_string();
    if description.len() < 10 {
        return Err(ServerFnError::new(
            "Description must be at least 10 characters.",
        ));
    }

    // Generate feedback ID: fb-{uuid4_hex_first_12_chars} — same format as REST route
    let feedback_id = format!(
        "fb-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );

    // Resolve workspace_id from auth context
    let workspace_id = auth.workspace.workspace_id.clone();

    // Build the context JSON blob. If the user opted in and provided a
    // context string, parse it as JSON. If a screenshot was attached, merge
    // it into the context object. Uses serde_json for safe manipulation.
    let mut context_value: serde_json::Value = if include_context {
        serde_json::from_str(context.trim()).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if let Some(ref data) = screenshot
        && !data.is_empty()
    {
        // Validate size: 2MB limit (matching REST route MAX_SCREENSHOT_BYTES)
        let estimated_size = data.len() * 3 / 4;
        if estimated_size <= 2 * 1024 * 1024
            && let Some(obj) = context_value.as_object_mut()
        {
            obj.insert(
                "screenshot_base64".to_string(),
                serde_json::Value::String(data.clone()),
            );
        }
    }

    let context_str = context_value.to_string();

    // Insert feedback — same SQL as apps/server/src/routes/feedback.rs
    let is_pg = ctx.db.is_postgres();
    let sql = format!(
        "INSERT INTO feedback \
            (id, user_id, workspace_id, type, description, include_context, context, status, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, {}, 'new', {})",
        kyomi_core::sql_compat::cast_to_json(is_pg, "$7"),
        kyomi_core::sql_compat::now(is_pg)
    );
    kyomi_core::db_execute!(
        &ctx.db,
        &sql,
        &feedback_id,
        &auth.user_id,
        workspace_id.as_deref(),
        &feedback_type,
        &description,
        &include_context,
        &context_str
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    tracing::info!(
        feedback_id = %feedback_id,
        user = %auth.email,
        feedback_type = %feedback_type,
        include_context = %include_context,
        "Feedback submitted via server function"
    );

    Ok(FeedbackResponse {
        status: "received".into(),
        feedback_id,
        message: "Thank you! Feedback like yours helps shape Kyomi".into(),
    })
}

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context};
