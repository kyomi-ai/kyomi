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

    // Build empty context JSON (no screenshot/browser context from the modal)
    let context_str = "{}";

    // Insert feedback — same SQL as apps/server/src/routes/feedback.rs
    let is_pg = ctx.db.is_postgres();
    let sql = format!(
        "INSERT INTO feedback \
            (id, user_id, workspace_id, type, description, include_context, context, status, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'new', {})",
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
        &false, // include_context — no browser context from modal
        &context_str
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    tracing::info!(
        feedback_id = %feedback_id,
        user = %auth.email,
        feedback_type = %feedback_type,
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
