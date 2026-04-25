// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server function for submitting user feedback.
//!
//! Thin adapter over [`kyomi_auth::feedback_service::submit_feedback`] — the
//! same shared service called by the REST handler at
//! `apps/server/src/routes/feedback.rs`. Keeping the logic (validation,
//! persistence, Linear/Slack/email notifications) in one place guarantees the
//! two surfaces cannot silently diverge.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

/// Response from the feedback submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackResponse {
    pub status: String,
    pub feedback_id: String,
    pub message: String,
}

/// Submit user feedback via the shared feedback service.
///
/// Mirrors the REST handler at `POST /api/v1/feedback` by delegating to the
/// same `kyomi_auth::feedback_service::submit_feedback` entry point. Produces
/// the same persistence, rate limiting, and notification side effects.
#[server(prefix = "/leptos-api", endpoint = "submit_feedback")]
pub async fn submit_feedback(
    feedback_type: String,
    description: String,
    include_context: bool,
    context: String,
    screenshot: Option<String>,
) -> Result<FeedbackResponse, ServerFnError> {
    let auth = extract_auth().await?;
    let ctx = extract_context()?;

    let context_value: serde_json::Value = if include_context {
        serde_json::from_str(context.trim()).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let input = kyomi_auth::feedback_service::FeedbackInput {
        feedback_type,
        description,
        screenshot,
        include_context,
        context: context_value,
        // server_fn path takes workspace from AuthUser inside the service.
        workspace_id: None,
    };

    let result = kyomi_auth::feedback_service::submit_feedback(
        &ctx.db,
        &ctx.config,
        &auth,
        input,
    )
    .await
    .into_sfn()?;

    Ok(FeedbackResponse {
        status: result.status,
        feedback_id: result.feedback_id,
        message: result.message,
    })
}

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{extract_auth, extract_context, IntoServerFnError};
