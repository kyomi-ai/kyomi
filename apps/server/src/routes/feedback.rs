// SPDX-License-Identifier: AGPL-3.0-or-later

//! Feedback submission and retrieval endpoints.
//!
//! Wire-compatible with Python's `routers/feedback.py`.
//! All endpoints require authentication (AuthUser extractor).

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
// Constants
// ---------------------------------------------------------------------------

/// Maximum screenshot size in bytes (2 MB).
const MAX_SCREENSHOT_BYTES: usize = 2 * 1024 * 1024;

// ---------------------------------------------------------------------------
// POST /feedback
// ---------------------------------------------------------------------------

async fn submit_feedback(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<FeedbackRequest>,
) -> Result<Json<FeedbackResponse>, kyomi_core::Error> {
    // Feedback routes to our Linear — disable in self-hosted and personal mode
    if state.config.self_hosted || state.config.is_personal() {
        return Err(kyomi_core::Error::NotFound(
            "Feedback is not available in this deployment mode.".into(),
        ));
    }

    // Validate feedback type
    if !["bug", "feature", "question"].contains(&data.feedback_type.as_str()) {
        return Err(kyomi_core::Error::BadRequest(
            "Invalid feedback type. Must be 'bug', 'feature', or 'question'.".into(),
        ));
    }

    // Validate description length
    let description = data.description.trim().to_string();
    if description.len() < 10 {
        return Err(kyomi_core::Error::BadRequest(
            "Description must be at least 10 characters.".into(),
        ));
    }

    // Rate limit: max 5 feedback submissions per user per hour
    let is_pg = state.db.is_postgres();
    let rate_limit_sql = if is_pg {
        "SELECT COUNT(*) FROM feedback WHERE user_id = $1 AND created_at > NOW() - INTERVAL '1 hour'"
            .to_string()
    } else {
        "SELECT COUNT(*) FROM feedback WHERE user_id = $1 AND created_at > datetime('now', '-1 hour')"
            .to_string()
    };
    let recent_count: i64 =
        kyomi_core::db_fetch_scalar!(&state.db, i64, &rate_limit_sql, &user.user_id)?;
    if recent_count >= 5 {
        return Err(kyomi_core::Error::TooManyRequests(
            "You've submitted too many feedback items recently. Please try again in an hour.".into(),
            3600,
        ));
    }

    // Generate feedback ID: fb-{uuid4_hex_first_12_chars}
    let feedback_id = format!("fb-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);

    // Build context JSON
    let mut context = if data.include_context {
        data.context.unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Handle screenshot in context
    if let Some(ref screenshot_b64) = data.screenshot
        && data.include_context {
            // Estimate decoded size: base64 is ~4/3 of original
            let estimated_size = screenshot_b64.len() * 3 / 4;
            if estimated_size <= MAX_SCREENSHOT_BYTES {
                if let Some(obj) = context.as_object_mut() {
                    obj.insert(
                        "screenshot_base64".to_string(),
                        serde_json::Value::String(screenshot_b64.clone()),
                    );
                }
            } else if let Some(obj) = context.as_object_mut() {
                obj.insert(
                    "screenshot_too_large".to_string(),
                    serde_json::Value::Bool(true),
                );
            }
        }

    // Resolve workspace_id: from request body or from auth context
    let workspace_id = data
        .workspace_id
        .or_else(|| user.workspace.workspace_id.clone());

    // Insert feedback
    let context_str = context.to_string();
    let sql = format!(
        "INSERT INTO feedback \
            (id, user_id, workspace_id, type, description, include_context, context, status, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'new', {})",
        kyomi_core::sql_compat::now(is_pg)
    );
    kyomi_core::db_execute!(
        &state.db, &sql,
        &feedback_id,
        &user.user_id,
        workspace_id.as_deref(),
        &data.feedback_type,
        &description,
        &data.include_context,
        &context_str
    )?;

    tracing::info!(
        feedback_id = %feedback_id,
        user = %user.email,
        feedback_type = %data.feedback_type,
        "Feedback submitted"
    );

    // Spawn background notifications (fire-and-forget)
    let notify_state = state.clone();
    let fb_id = feedback_id.clone();
    let fb_type = data.feedback_type.clone();
    let fb_description = description.clone();
    let fb_email = user.email.clone();
    let fb_workspace_name = user.workspace.workspace_name.clone();
    let fb_context = context.clone();
    tokio::spawn(async move {
        // 1. Create Linear issue (primary feedback destination)
        let linear_result = create_linear_issue_from_feedback(
            &notify_state,
            &fb_id,
            &fb_type,
            &fb_description,
            &fb_email,
            fb_workspace_name.as_deref(),
            &fb_context,
        )
        .await;

        // 2. Slack notification — slim one-liner if Linear succeeded, full fallback otherwise
        match &linear_result {
            Some(result) => {
                // Slim Slack notification with Linear link
                if let Err(e) = send_slim_slack_notification(
                    &notify_state,
                    &fb_type,
                    &fb_email,
                    &result.identifier,
                    &result.url,
                )
                .await
                {
                    tracing::error!(
                        feedback_id = %fb_id,
                        error = %e,
                        "Failed to send slim Slack notification"
                    );
                }
            }
            None => {
                // Full Slack notification as fallback (Linear not configured or failed)
                if let Err(e) = send_feedback_slack_notification(
                    &notify_state,
                    &fb_id,
                    &fb_type,
                    &fb_description,
                    &fb_email,
                    fb_workspace_name.as_deref(),
                    &fb_context,
                )
                .await
                {
                    tracing::error!(
                        feedback_id = %fb_id,
                        error = %e,
                        "Failed to send Slack notification for feedback"
                    );
                }

                // Upload screenshot to Slack if available (only in fallback mode)
                if let Some(screenshot_b64) = fb_context
                    .as_object()
                    .and_then(|obj| obj.get("screenshot_base64"))
                    .and_then(|v| v.as_str())
                {
                    upload_screenshot_to_slack(&notify_state, &fb_id, screenshot_b64).await;
                }
            }
        }

        // 3. Email notification to support (always)
        send_feedback_email_notification(
            &notify_state,
            &fb_id,
            &fb_type,
            &fb_description,
            &fb_email,
            fb_workspace_name.as_deref(),
            &fb_context,
        )
        .await;
    });

    Ok(Json(FeedbackResponse {
        status: "received".into(),
        feedback_id,
        message: "Thank you! Feedback like yours helps shape Kyomi".into(),
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
// Background Linear issue creation
// ---------------------------------------------------------------------------

/// Create a Linear issue from feedback, returning the result if successful.
///
/// Returns `None` if Linear is not configured or if the API call fails.
/// Failures are logged but never propagate — feedback submission is not
/// blocked by Linear outages.
async fn create_linear_issue_from_feedback(
    state: &AppState,
    feedback_id: &str,
    feedback_type: &str,
    description: &str,
    user_email: &str,
    workspace_name: Option<&str>,
    context: &serde_json::Value,
) -> Option<crate::services::linear::LinearIssueResult> {
    let (api_key, team_id) = match (
        state.config.linear_api_key.as_deref(),
        state.config.linear_feedback_team_id.as_deref(),
    ) {
        (Some(key), Some(team)) => (key, team),
        _ => {
            tracing::debug!("Linear not configured, skipping issue creation");
            return None;
        }
    };

    // Decode screenshot bytes if available
    let screenshot_bytes = context
        .as_object()
        .and_then(|obj| obj.get("screenshot_base64"))
        .and_then(|v| v.as_str())
        .and_then(|b64| {
            // Strip data URL prefix if present
            let data = if let Some((_prefix, d)) = b64.split_once(',') { d } else { b64 };
            base64_decode(data).ok()
        });

    let input = crate::services::linear::FeedbackIssueInput {
        feedback_id: feedback_id.to_string(),
        feedback_type: feedback_type.to_string(),
        description: description.to_string(),
        user_email: user_email.to_string(),
        workspace_name: workspace_name.map(String::from),
        page_url: context
            .as_object()
            .and_then(|o| o.get("url"))
            .and_then(|v| v.as_str())
            .map(String::from),
        browser: context
            .as_object()
            .and_then(|o| o.get("browser"))
            .and_then(|v| v.as_str())
            .map(String::from),
        os: context
            .as_object()
            .and_then(|o| o.get("os"))
            .and_then(|v| v.as_str())
            .map(String::from),
        console_errors: context
            .as_object()
            .and_then(|o| o.get("console_errors"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
        failed_requests: context
            .as_object()
            .and_then(|o| o.get("failed_requests"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
        timestamp: chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string(),
    };

    match crate::services::linear::create_feedback_issue(
        api_key,
        team_id,
        &input,
        screenshot_bytes.as_deref(),
    )
    .await
    {
        Ok(result) => {
            tracing::info!(
                feedback_id = %feedback_id,
                linear_issue = %result.identifier,
                "Linear issue created for feedback"
            );
            Some(result)
        }
        Err(e) => {
            tracing::error!(
                feedback_id = %feedback_id,
                error = %e,
                "Failed to create Linear issue for feedback"
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Slim Slack notification (with Linear link)
// ---------------------------------------------------------------------------

/// Send a one-liner Slack notification with a link to the Linear issue.
///
/// Used when Linear issue creation succeeded — the full context is on Linear,
/// so Slack just gets a pointer.
async fn send_slim_slack_notification(
    state: &AppState,
    feedback_type: &str,
    user_email: &str,
    linear_identifier: &str,
    linear_url: &str,
) -> kyomi_core::Result<()> {
    let webhook_url = match state.config.slack_feedback_webhook_url.as_deref() {
        Some(url) if !url.is_empty() => url,
        _ => {
            tracing::debug!("SLACK_FEEDBACK_WEBHOOK_URL not configured, skipping notification");
            return Ok(());
        }
    };

    let emoji = match feedback_type {
        "bug" => ":bug:",
        "feature" => ":bulb:",
        "question" => ":question:",
        _ => ":memo:",
    };

    let text = format!(
        "{emoji} New {feedback_type} feedback from {user_email} \u{2014} <{linear_url}|{linear_identifier}>"
    );
    let payload = serde_json::json!({ "text": text });

    let http = kyomi_datasource_server::http_client()?;
    let response = http
        .post(webhook_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Slack webhook POST failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "Slack webhook returned {status}: {body}"
        )));
    }

    tracing::info!("Slim Slack notification sent with Linear link {linear_identifier}");
    Ok(())
}

// ---------------------------------------------------------------------------
// Background Slack notification (full fallback)
// ---------------------------------------------------------------------------

/// Send a Slack webhook notification for a new feedback submission.
///
/// Uses the incoming webhook URL (not the bot token API) so this is a simple
/// POST with a JSON body. The webhook URL is configured separately from the
/// main Slack integration.
async fn send_feedback_slack_notification(
    state: &AppState,
    feedback_id: &str,
    feedback_type: &str,
    description: &str,
    user_email: &str,
    workspace_name: Option<&str>,
    context: &serde_json::Value,
) -> kyomi_core::Result<()> {
    let webhook_url = match state.config.slack_feedback_webhook_url.as_deref() {
        Some(url) if !url.is_empty() => url,
        _ => {
            tracing::debug!("SLACK_FEEDBACK_WEBHOOK_URL not configured, skipping notification");
            return Ok(());
        }
    };

    // Choose emoji based on feedback type
    let emoji = match feedback_type {
        "bug" => ":bug:",
        "feature" => ":bulb:",
        "question" => ":question:",
        _ => ":memo:",
    };

    // Truncate description for Slack (max 1500 chars)
    let truncated_desc = if description.len() > 1500 {
        format!("{}...", &description[..1497])
    } else {
        description.to_string()
    };

    let workspace_display = workspace_name.unwrap_or("(no workspace)");

    // Build Block Kit blocks
    let mut blocks = vec![
        serde_json::json!({
            "type": "header",
            "text": {
                "type": "plain_text",
                "text": format!("{emoji} New {feedback_type} feedback"),
                "emoji": true,
            }
        }),
        serde_json::json!({
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": format!("*From:* {user_email}\n*Workspace:* {workspace_display}"),
            }
        }),
        serde_json::json!({
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": format!("*Description:*\n{truncated_desc}"),
            }
        }),
    ];

    // Add context details if available (matching Python's notify_feedback)
    if let Some(obj) = context.as_object() {
        let mut context_parts: Vec<String> = Vec::new();

        if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
            context_parts.push(format!("*Page:* {url}"));
        }

        if let Some(browser) = obj.get("browser").and_then(|v| v.as_str()) {
            context_parts.push(format!("*Browser:* {browser}"));
        }

        if let Some(os) = obj.get("os").and_then(|v| v.as_str()) {
            context_parts.push(format!("*OS:* {os}"));
        }

        if let Some(errors) = obj.get("console_errors").and_then(|v| v.as_array()) {
            let total = errors.len();
            let errors_to_show: Vec<_> = if total > 3 {
                errors[total - 3..].to_vec()
            } else {
                errors.to_vec()
            };
            let error_lines: Vec<String> = errors_to_show
                .iter()
                .map(|e| {
                    let msg = e.get("message").and_then(|v| v.as_str()).unwrap_or("N/A");
                    let truncated: String = msg.chars().take(150).collect();
                    format!("• {truncated}")
                })
                .collect();
            if !error_lines.is_empty() {
                let header = if total > 3 {
                    format!("*Recent Errors ({total} total):*")
                } else {
                    format!("*Recent Error{}:*", if total > 1 { "s" } else { "" })
                };
                context_parts.push(format!("{header}\n```{}```", error_lines.join("\n")));
            }
        }

        if let Some(requests) = obj.get("failed_requests").and_then(|v| v.as_array()) {
            let total = requests.len();
            let requests_to_show: Vec<_> = if total > 3 {
                requests[total - 3..].to_vec()
            } else {
                requests.to_vec()
            };
            let request_lines: Vec<String> = requests_to_show
                .iter()
                .map(|r| {
                    let method = r.get("method").and_then(|v| v.as_str()).unwrap_or("");
                    let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let url_truncated: String = url.chars().take(80).collect();
                    let status = r.get("status").and_then(|v| v.as_str()).unwrap_or("N/A");
                    format!("• `{method} {url_truncated}` → {status}")
                })
                .collect();
            if !request_lines.is_empty() {
                let header = if total > 3 {
                    format!("*Failed Requests ({total} total):*")
                } else {
                    format!("*Failed Request{}:*", if total > 1 { "s" } else { "" })
                };
                context_parts.push(format!("{header}\n{}", request_lines.join("\n")));
            }
        }

        if obj.contains_key("screenshot_base64") {
            let screenshot_upload_enabled = state.config.slack_bot_token.is_some()
                && state.config.slack_feedback_channel_id.is_some();
            if screenshot_upload_enabled {
                context_parts.push("*Screenshot:* ✅ See attached image below".to_string());
            } else {
                context_parts.push("*Screenshot:* ✅ Attached (stored in database)".to_string());
            }
        } else if obj.get("screenshot_too_large").and_then(|v| v.as_bool()).unwrap_or(false) {
            context_parts.push("*Screenshot:* ⚠️ Too large (max 2MB)".to_string());
        }

        if !context_parts.is_empty() {
            blocks.push(serde_json::json!({
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": context_parts.join("\n"),
                }
            }));
        }
    }

    // Metadata footer
    blocks.push(serde_json::json!({
        "type": "context",
        "elements": [
            {
                "type": "mrkdwn",
                "text": format!("Feedback ID: `{feedback_id}` | {}", chrono::Utc::now().format("%Y-%m-%d %H:%M UTC")),
            }
        ]
    }));

    let payload = serde_json::json!({
        "text": format!("{emoji} New {feedback_type} feedback from {user_email}"),
        "blocks": blocks,
    });

    // POST to webhook URL (no auth header needed — the URL itself is the secret)
    let http = kyomi_datasource_server::http_client()?;
    let response = http
        .post(webhook_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Slack webhook POST failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "Slack webhook returned {status}: {body}"
        )));
    }

    tracing::info!(
        feedback_id = %feedback_id,
        "Slack feedback notification sent"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Screenshot upload to Slack
// ---------------------------------------------------------------------------

/// Upload a feedback screenshot to Slack using the files.getUploadURLExternal API.
///
/// Matches Python's `SlackService._upload_screenshot`:
///   1. files.getUploadURLExternal → get upload URL + file_id
///   2. POST binary to the upload URL
///   3. files.completeUploadExternal → share to channel
///
/// Requires SLACK_BOT_TOKEN and SLACK_FEEDBACK_CHANNEL_ID.
async fn upload_screenshot_to_slack(state: &AppState, feedback_id: &str, screenshot_b64: &str) {
    let (Some(bot_token), Some(channel_id)) = (
        state.config.slack_bot_token.as_deref(),
        state.config.slack_feedback_channel_id.as_deref(),
    ) else {
        tracing::debug!("Slack screenshot upload disabled — SLACK_BOT_TOKEN or SLACK_FEEDBACK_CHANNEL_ID not set");
        return;
    };

    // Strip data URL prefix if present (e.g., "data:image/png;base64,...")
    let b64_data = if let Some((_prefix, data)) = screenshot_b64.split_once(',') {
        data
    } else {
        screenshot_b64
    };

    let image_bytes = match base64_decode(b64_data) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(feedback_id = %feedback_id, "Failed to decode screenshot base64: {e}");
            return;
        }
    };

    let filename = format!("feedback_{feedback_id}.png");

    let http = match kyomi_datasource_server::http_client() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(feedback_id = %feedback_id, "Failed to create HTTP client: {e}");
            return;
        }
    };

    // Step 1: Get upload URL
    let get_url_resp = match http
        .post("https://slack.com/api/files.getUploadURLExternal")
        .header("Authorization", format!("Bearer {bot_token}"))
        .form(&[
            ("filename", filename.as_str()),
            ("length", &image_bytes.len().to_string()),
        ])
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(feedback_id = %feedback_id, "Slack getUploadURLExternal failed: {e}");
            return;
        }
    };

    let get_url_json: serde_json::Value = match get_url_resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(feedback_id = %feedback_id, "Failed to parse getUploadURLExternal response: {e}");
            return;
        }
    };

    if !get_url_json["ok"].as_bool().unwrap_or(false) {
        let error = get_url_json["error"].as_str().unwrap_or("unknown");
        tracing::warn!(feedback_id = %feedback_id, "Slack getUploadURLExternal error: {error}");
        return;
    }

    let upload_url = match get_url_json["upload_url"].as_str() {
        Some(u) => u,
        None => {
            tracing::warn!(feedback_id = %feedback_id, "Missing upload_url in Slack response");
            return;
        }
    };
    let file_id = match get_url_json["file_id"].as_str() {
        Some(f) => f.to_string(),
        None => {
            tracing::warn!(feedback_id = %feedback_id, "Missing file_id in Slack response");
            return;
        }
    };

    // Step 2: Upload file bytes to the URL
    let upload_resp = match http
        .post(upload_url)
        .header("Content-Type", "image/png")
        .body(image_bytes)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(feedback_id = %feedback_id, "Slack file upload failed: {e}");
            return;
        }
    };

    if !upload_resp.status().is_success() {
        let status = upload_resp.status();
        tracing::warn!(feedback_id = %feedback_id, "Slack file upload returned {status}");
        return;
    }

    // Step 3: Complete upload and share to channel
    let complete_payload = serde_json::json!({
        "files": [{"id": file_id, "title": format!("Screenshot for {feedback_id}")}],
        "channel_id": channel_id,
        "initial_comment": "📸 Screenshot attached",
    });

    let complete_resp = match http
        .post("https://slack.com/api/files.completeUploadExternal")
        .header("Authorization", format!("Bearer {bot_token}"))
        .header("Content-Type", "application/json")
        .json(&complete_payload)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(feedback_id = %feedback_id, "Slack completeUploadExternal failed: {e}");
            return;
        }
    };

    let complete_json: serde_json::Value = match complete_resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(feedback_id = %feedback_id, "Failed to parse completeUploadExternal response: {e}");
            return;
        }
    };

    if complete_json["ok"].as_bool().unwrap_or(false) {
        tracing::info!(feedback_id = %feedback_id, "Screenshot uploaded to Slack");
    } else {
        let error = complete_json["error"].as_str().unwrap_or("unknown");
        tracing::warn!(feedback_id = %feedback_id, "Slack completeUploadExternal error: {error}");
    }
}

/// Decode base64 string to bytes (standard or URL-safe).
fn base64_decode(data: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(data))
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Email notification to support
// ---------------------------------------------------------------------------

/// Send an email notification to the support address for a new feedback submission.
///
/// Includes full technical context (URL, browser, OS, console errors, failed requests)
/// and embeds the screenshot inline if available.
/// Sets Reply-To to the submitting user's email so support can reply directly.
async fn send_feedback_email_notification(
    state: &AppState,
    feedback_id: &str,
    feedback_type: &str,
    description: &str,
    user_email: &str,
    workspace_name: Option<&str>,
    context: &serde_json::Value,
) {
    let email_svc = kyomi_auth::email_service::EmailService::from_env();
    if !email_svc.is_configured() {
        return;
    }

    let support_email = &state.config.support_email;
    let workspace_display = workspace_name.unwrap_or("(no workspace)");
    let subject = format!("New {} feedback from {}", feedback_type, user_email);

    // Truncate description for email
    let truncated_desc = if description.len() > 2000 {
        format!("{}...", &description[..1997])
    } else {
        description.to_string()
    };

    // Build context HTML rows
    let mut context_rows = String::new();
    if let Some(obj) = context.as_object() {
        if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
            context_rows.push_str(&format!(
                r#"<tr><td style="padding:4px 12px 4px 0;font-weight:600;color:#374151;vertical-align:top;">Page</td><td style="padding:4px 0;color:#4b5563;">{}</td></tr>"#,
                html_escape(url)
            ));
        }
        if let Some(browser) = obj.get("browser").and_then(|v| v.as_str()) {
            context_rows.push_str(&format!(
                r#"<tr><td style="padding:4px 12px 4px 0;font-weight:600;color:#374151;vertical-align:top;">Browser</td><td style="padding:4px 0;color:#4b5563;">{}</td></tr>"#,
                html_escape(browser)
            ));
        }
        if let Some(os) = obj.get("os").and_then(|v| v.as_str()) {
            context_rows.push_str(&format!(
                r#"<tr><td style="padding:4px 12px 4px 0;font-weight:600;color:#374151;vertical-align:top;">OS</td><td style="padding:4px 0;color:#4b5563;">{}</td></tr>"#,
                html_escape(os)
            ));
        }

        // Console errors — show last 3
        if let Some(errors) = obj.get("console_errors").and_then(|v| v.as_array())
            && !errors.is_empty() {
                let total = errors.len();
                let errors_to_show = if total > 3 { &errors[total - 3..] } else { errors.as_slice() };
                let error_html: Vec<String> = errors_to_show
                    .iter()
                    .map(|e| {
                        let msg = e.get("message").and_then(|v| v.as_str()).unwrap_or("N/A");
                        let truncated: String = msg.chars().take(200).collect();
                        format!("<li><code>{}</code></li>", html_escape(&truncated))
                    })
                    .collect();
                let label = if total > 3 {
                    format!("Console Errors ({total} total)")
                } else {
                    format!("Console Error{}", if total > 1 { "s" } else { "" })
                };
                context_rows.push_str(&format!(
                    r#"<tr><td style="padding:4px 12px 4px 0;font-weight:600;color:#374151;vertical-align:top;">{label}</td><td style="padding:4px 0;color:#4b5563;"><ul style="margin:0;padding-left:16px;">{}</ul></td></tr>"#,
                    error_html.join("")
                ));
            }

        // Failed requests — show last 3
        if let Some(requests) = obj.get("failed_requests").and_then(|v| v.as_array())
            && !requests.is_empty() {
                let total = requests.len();
                let requests_to_show = if total > 3 { &requests[total - 3..] } else { requests.as_slice() };
                let request_html: Vec<String> = requests_to_show
                    .iter()
                    .map(|r| {
                        let method = r.get("method").and_then(|v| v.as_str()).unwrap_or("");
                        let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("");
                        let url_truncated: String = url.chars().take(100).collect();
                        let status = r.get("status").and_then(|v| v.as_str()).unwrap_or("N/A");
                        format!(
                            "<li><code>{} {}</code> &rarr; {}</li>",
                            html_escape(method),
                            html_escape(&url_truncated),
                            html_escape(status)
                        )
                    })
                    .collect();
                let label = if total > 3 {
                    format!("Failed Requests ({total} total)")
                } else {
                    format!("Failed Request{}", if total > 1 { "s" } else { "" })
                };
                context_rows.push_str(&format!(
                    r#"<tr><td style="padding:4px 12px 4px 0;font-weight:600;color:#374151;vertical-align:top;">{label}</td><td style="padding:4px 0;color:#4b5563;"><ul style="margin:0;padding-left:16px;">{}</ul></td></tr>"#,
                    request_html.join("")
                ));
            }
    }

    // Screenshot as inline base64 img tag
    let screenshot_html = if let Some(screenshot_b64) = context.as_object()
        .and_then(|obj| obj.get("screenshot_base64"))
        .and_then(|v| v.as_str())
    {
        // Ensure data URL prefix
        let src = if screenshot_b64.starts_with("data:") {
            screenshot_b64.to_string()
        } else {
            format!("data:image/png;base64,{screenshot_b64}")
        };
        format!(
            r#"<h3 style="color:#111827;margin:20px 0 8px 0;font-size:16px;">Screenshot</h3><img src="{src}" alt="Feedback screenshot" style="max-width:100%;border:1px solid #e5e7eb;border-radius:8px;" />"#
        )
    } else if context.as_object()
        .and_then(|obj| obj.get("screenshot_too_large"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        r#"<p style="color:#9ca3af;font-style:italic;">Screenshot was too large (max 2MB)</p>"#.to_string()
    } else {
        String::new()
    };

    let html_body = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <style>
        body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; line-height: 1.6; color: #374151; max-width: 600px; margin: 0 auto; padding: 20px; background-color: #ffffff; }}
    </style>
</head>
<body>
    <h2 style="color:#111827;margin:0 0 16px 0;">{subject}</h2>
    <table style="border-collapse:collapse;width:100%;font-size:14px;">
        <tr><td style="padding:4px 12px 4px 0;font-weight:600;color:#374151;vertical-align:top;">Type</td><td style="padding:4px 0;color:#4b5563;">{feedback_type}</td></tr>
        <tr><td style="padding:4px 12px 4px 0;font-weight:600;color:#374151;vertical-align:top;">From</td><td style="padding:4px 0;color:#4b5563;">{user_email}</td></tr>
        <tr><td style="padding:4px 12px 4px 0;font-weight:600;color:#374151;vertical-align:top;">Workspace</td><td style="padding:4px 0;color:#4b5563;">{workspace_display}</td></tr>
        <tr><td style="padding:4px 12px 4px 0;font-weight:600;color:#374151;vertical-align:top;">Description</td><td style="padding:4px 0;color:#4b5563;">{description_escaped}</td></tr>
        {context_rows}
        <tr><td style="padding:4px 12px 4px 0;font-weight:600;color:#374151;vertical-align:top;">Feedback ID</td><td style="padding:4px 0;color:#4b5563;"><code>{feedback_id}</code></td></tr>
    </table>
    {screenshot_html}
    <hr style="border:none;border-top:1px solid #e5e7eb;margin:20px 0;" />
    <p style="color:#9ca3af;font-size:12px;margin:0;">kyomi.ai</p>
</body>
</html>"#,
        subject = html_escape(&subject),
        feedback_type = html_escape(feedback_type),
        user_email = html_escape(user_email),
        workspace_display = html_escape(workspace_display),
        description_escaped = html_escape(&truncated_desc),
        feedback_id = html_escape(feedback_id),
    );

    let sent = email_svc
        .send_email(
            support_email,
            &subject,
            &html_body,
            None,
            Some(user_email),
        )
        .await;

    if sent {
        tracing::info!(feedback_id = %feedback_id, "Feedback email notification sent to {support_email}");
    } else {
        tracing::warn!(feedback_id = %feedback_id, "Failed to send feedback email notification");
    }
}

/// HTML escaping for user-provided strings in email templates.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
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
