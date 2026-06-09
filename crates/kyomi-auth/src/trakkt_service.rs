// SPDX-License-Identifier: AGPL-3.0-or-later

//! Trakkt API integration for creating feedback issues.
//!
//! Creates Trakkt issues via the REST API with full context from
//! in-app feedback submissions. Supports screenshot upload via Trakkt's
//! JSON attachment endpoint (`content_base64`).

use base64::Engine as _;
use serde::Deserialize;

// ── Hardcoded label IDs from the Kyomi workspace on Trakkt ────────────
const LABEL_FEEDBACK: &str = "69a2be3c-4d6d-40a4-91ee-f50308b14cc8";
const LABEL_BUG: &str = "99a19b06-38c9-48c4-90d5-5b8697792566";
const LABEL_FEATURE: &str = "9a552138-1d44-467d-9331-a4042603893b";
const LABEL_IMPROVEMENT: &str = "08ba364f-b5d5-436c-ac96-0b4ac90efc9a";

// ── Public types ────────────────────────────────────────────────────────

pub struct TrakktIssueResult {
    pub identifier: String,
    pub url: String,
}

pub struct FeedbackIssueInput {
    pub feedback_id: String,
    pub feedback_type: String,
    pub description: String,
    pub user_email: String,
    pub workspace_name: Option<String>,
    pub page_url: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub console_errors: Vec<serde_json::Value>,
    pub failed_requests: Vec<serde_json::Value>,
    pub timestamp: String,
}

// ── API response types ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateIssueResponse {
    issue_id: String,
    number: i64,
}


// ── Public API ──────────────────────────────────────────────────────────

pub async fn create_feedback_issue(
    api_url: &str,
    api_token: &str,
    team_key: &str,
    feedback: &FeedbackIssueInput,
    screenshot_bytes: Option<(&[u8], &str)>,
) -> Result<TrakktIssueResult, String> {
    let title = build_issue_title(&feedback.feedback_type, &feedback.description);
    let description = build_issue_description(feedback);
    let label_ids = label_ids_for_type(&feedback.feedback_type);

    let payload = serde_json::json!({
        "title": title,
        "description": description,
        "team_key": team_key,
        "labels": label_ids,
        "priority": priority_for_type(&feedback.feedback_type),
    });

    let http = crate::http_client()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let response = http
        .post(format!("{api_url}/api/v1/issues"))
        .bearer_auth(api_token)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Trakkt API request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Trakkt API returned {status}: {body}"));
    }

    let result: CreateIssueResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Trakkt response: {e}"))?;

    let identifier = format!("{team_key}-{}", result.number);
    let issue_url = format!("{api_url}/issue/{identifier}");

    if let Some((bytes, content_type)) = screenshot_bytes {
        let ext = match content_type {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            _ => "png",
        };
        let filename = format!("feedback_{}.{ext}", feedback.feedback_id);
        match upload_screenshot(api_url, api_token, bytes, &filename, content_type, &result.issue_id).await {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!(
                    feedback_id = %feedback.feedback_id,
                    error = %e,
                    "Failed to upload screenshot to Trakkt, issue created without it"
                );
            }
        }
    }

    Ok(TrakktIssueResult {
        identifier,
        url: issue_url,
    })
}

async fn upload_screenshot(
    api_url: &str,
    api_token: &str,
    image_bytes: &[u8],
    filename: &str,
    content_type: &str,
    issue_id: &str,
) -> Result<(), String> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(image_bytes);

    let payload = serde_json::json!({
        "content_base64": b64,
        "filename": filename,
        "content_type": content_type,
        "issue_id": issue_id,
    });

    let http = crate::http_client()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let url = format!("{api_url}/api/v1/attachments");
    let response = http
        .post(&url)
        .bearer_auth(api_token)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Trakkt attachment upload failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Trakkt attachment upload returned {status}: {body}"));
    }

    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn build_issue_title(feedback_type: &str, description: &str) -> String {
    let emoji = match feedback_type {
        "bug" => "\u{1f41b}",
        "feature" => "\u{1f4a1}",
        "question" => "\u{2753}",
        _ => "\u{1f4dd}",
    };

    let truncated: String = description.chars().take(80).collect();
    let suffix = if description.chars().count() > 80 { "..." } else { "" };

    format!("[feedback] {emoji} {truncated}{suffix}")
}

fn build_issue_description(feedback: &FeedbackIssueInput) -> String {
    let workspace_display = feedback
        .workspace_name
        .as_deref()
        .unwrap_or("(no workspace)");
    let page_url_display = feedback.page_url.as_deref().unwrap_or("N/A");

    let mut md = String::new();

    md.push_str(&format!("**Type:** {}\n", feedback.feedback_type));
    md.push_str(&format!("**From:** {}\n", feedback.user_email));
    md.push_str(&format!("**Workspace:** {}\n", workspace_display));
    md.push_str(&format!("**Page:** {}\n", page_url_display));
    md.push_str(&format!("**Timestamp:** {}\n", feedback.timestamp));
    md.push_str(&format!("**Feedback ID:** `{}`\n", feedback.feedback_id));

    md.push_str("\n---\n\n");
    md.push_str("## Description\n\n");
    md.push_str(&feedback.description);
    md.push('\n');

    let has_tech_context = feedback.browser.is_some()
        || feedback.os.is_some()
        || !feedback.console_errors.is_empty()
        || !feedback.failed_requests.is_empty();

    if has_tech_context {
        md.push_str("\n## Technical Context\n\n");

        if feedback.browser.is_some() || feedback.os.is_some() {
            md.push_str("| Field | Value |\n|-------|-------|\n");
            if let Some(ref browser) = feedback.browser {
                md.push_str(&format!("| Browser | {browser} |\n"));
            }
            if let Some(ref os) = feedback.os {
                md.push_str(&format!("| OS | {os} |\n"));
            }
            md.push('\n');
        }

        if !feedback.console_errors.is_empty() {
            let total = feedback.console_errors.len();
            let show = total.min(10);
            md.push_str(&format!("### Console Errors ({total})\n\n"));
            for error in feedback.console_errors.iter().take(10) {
                let msg = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("N/A");
                let truncated: String = msg.chars().take(200).collect();
                md.push_str(&format!("- `{truncated}`\n"));
            }
            if total > show {
                md.push_str(&format!("- *...and {} more*\n", total - show));
            }
            md.push('\n');
        }

        if !feedback.failed_requests.is_empty() {
            let total = feedback.failed_requests.len();
            let show = total.min(10);
            md.push_str(&format!("### Failed Requests ({total})\n\n"));
            for req in feedback.failed_requests.iter().take(10) {
                let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
                let url = req.get("url").and_then(|v| v.as_str()).unwrap_or("");
                let url_truncated: String = url.chars().take(100).collect();
                let status = req
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("N/A");
                md.push_str(&format!("- `{method} {url_truncated}` -> {status}\n"));
            }
            if total > show {
                md.push_str(&format!("- *...and {} more*\n", total - show));
            }
            md.push('\n');
        }
    }

    md.push_str("\n---\n");
    md.push_str("*Auto-created from in-app feedback*\n");

    md
}

fn label_ids_for_type(feedback_type: &str) -> Vec<String> {
    let type_label = match feedback_type {
        "bug" => LABEL_BUG,
        "feature" => LABEL_FEATURE,
        "question" => LABEL_IMPROVEMENT,
        _ => LABEL_IMPROVEMENT,
    };

    vec![
        LABEL_FEEDBACK.to_string(),
        type_label.to_string(),
    ]
}

fn priority_for_type(feedback_type: &str) -> i32 {
    match feedback_type {
        "bug" => 2,     // high
        "feature" => 3, // medium
        _ => 3,         // medium
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_title_bug() {
        let title = build_issue_title("bug", "Something is broken in the dashboard view");
        assert!(title.starts_with("[feedback] \u{1f41b}"));
        assert!(title.contains("Something is broken"));
    }

    #[test]
    fn build_title_truncates_long_description() {
        let long_desc = "a".repeat(200);
        let title = build_issue_title("feature", &long_desc);
        assert!(title.ends_with("..."));
        assert!(title.contains(&"a".repeat(80)));
    }

    #[test]
    fn label_ids_bug() {
        let labels = label_ids_for_type("bug");
        assert_eq!(labels.len(), 2);
        assert!(labels.contains(&LABEL_FEEDBACK.to_string()));
        assert!(labels.contains(&LABEL_BUG.to_string()));
    }

    #[test]
    fn label_ids_feature() {
        let labels = label_ids_for_type("feature");
        assert!(labels.contains(&LABEL_FEATURE.to_string()));
    }

    #[test]
    fn priority_bug_is_high() {
        assert_eq!(priority_for_type("bug"), 2);
    }

    #[test]
    fn priority_feature_is_medium() {
        assert_eq!(priority_for_type("feature"), 3);
    }
}
