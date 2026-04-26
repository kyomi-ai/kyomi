// SPDX-License-Identifier: AGPL-3.0-or-later

//! Linear API integration for creating feedback issues.
//!
//! Creates Linear issues via the GraphQL API with full context from
//! in-app feedback submissions. Supports screenshot upload via Linear's
//! two-step file upload flow.

use serde::Deserialize;

// ── Linear GraphQL endpoint ─────────────────────────────────────────────
const LINEAR_API_URL: &str = "https://api.linear.app/graphql";

// ── Hardcoded label IDs from the Kyomi team ─────────────────────────────
const LABEL_NEEDS_TRIAGE: &str = "14f8d0a1-dacd-4830-bdb6-78c738ca4343";
const LABEL_BUG: &str = "c46f17d5-81a5-4d18-ae76-2daecfc86aec";
const LABEL_FEATURE: &str = "f7aae904-efdb-4a35-a934-e1e6cce5247e";
const LABEL_IMPROVEMENT: &str = "4657ad67-cc68-4781-b2c6-f5ff21cbcb38";

// ── Public types ────────────────────────────────────────────────────────

/// Result of creating a Linear issue.
pub struct LinearIssueResult {
    /// Internal Linear issue UUID.
    pub _id: String,
    /// Human-readable issue identifier (e.g. "KYO-47").
    pub identifier: String,
    /// Web URL for the issue.
    pub url: String,
}

/// Input data for creating a feedback issue on Linear.
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

// ── GraphQL response types ──────────────────────────────────────────────

#[derive(Deserialize)]
struct GraphQLResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Deserialize)]
struct GraphQLError {
    message: String,
}

#[derive(Deserialize)]
struct IssueCreateData {
    #[serde(rename = "issueCreate")]
    issue_create: IssueCreateResult,
}

#[derive(Deserialize)]
struct IssueCreateResult {
    success: bool,
    issue: Option<IssueFields>,
}

#[derive(Deserialize)]
struct IssueFields {
    id: String,
    identifier: String,
    url: String,
}

#[derive(Deserialize)]
struct FileUploadData {
    #[serde(rename = "fileUpload")]
    file_upload: FileUploadResult,
}

#[derive(Deserialize)]
struct FileUploadResult {
    #[serde(rename = "uploadFile")]
    upload_file: Option<UploadFile>,
}

#[derive(Deserialize)]
struct UploadFile {
    #[serde(rename = "assetUrl")]
    asset_url: String,
    headers: Vec<UploadHeader>,
}

#[derive(Deserialize)]
struct UploadHeader {
    key: String,
    value: String,
}

// ── Public API ──────────────────────────────────────────────────────────

/// Create a Linear issue from feedback data.
///
/// Uploads screenshot first (if provided), then creates the issue with
/// the screenshot embedded in the description markdown.
///
/// `screenshot_bytes` carries both the raw bytes and the MIME type so Linear
/// receives the correct content type and file extension.
pub async fn create_feedback_issue(
    api_key: &str,
    team_id: &str,
    feedback: &FeedbackIssueInput,
    screenshot_bytes: Option<(&[u8], &str)>,
) -> Result<LinearIssueResult, String> {
    // Upload screenshot if available
    let screenshot_url = if let Some((bytes, content_type)) = screenshot_bytes {
        let ext = match content_type {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            _ => "png",
        };
        let filename = format!("feedback_{}.{ext}", feedback.feedback_id);
        match upload_screenshot(api_key, bytes, &filename, content_type)
            .await
        {
            Ok(url) => Some(url),
            Err(e) => {
                tracing::warn!(
                    feedback_id = %feedback.feedback_id,
                    error = %e,
                    "Failed to upload screenshot to Linear, creating issue without it"
                );
                None
            }
        }
    } else {
        None
    };

    let title = build_issue_title(&feedback.feedback_type, &feedback.description);
    let description = build_issue_description(feedback, screenshot_url.as_deref());
    let label_ids = label_ids_for_type(&feedback.feedback_type);

    let label_ids_json: Vec<serde_json::Value> = label_ids
        .iter()
        .map(|id| serde_json::Value::String(id.clone()))
        .collect();

    let query = r#"
        mutation IssueCreate($input: IssueCreateInput!) {
            issueCreate(input: $input) {
                success
                issue { id identifier url }
            }
        }
    "#;

    let variables = serde_json::json!({
        "input": {
            "title": title,
            "description": description,
            "teamId": team_id,
            "labelIds": label_ids_json,
        }
    });

    let payload = serde_json::json!({
        "query": query,
        "variables": variables,
    });

    let http = crate::http_client()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let response = http
        .post(LINEAR_API_URL)
        .header("Authorization", api_key)
        .header("Content-Type", "application/json")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Linear API request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Linear API returned {status}: {body}"));
    }

    let result: GraphQLResponse<IssueCreateData> = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Linear response: {e}"))?;

    if let Some(errors) = result.errors {
        let msgs: Vec<&str> = errors.iter().map(|e| e.message.as_str()).collect();
        return Err(format!("Linear GraphQL errors: {}", msgs.join(", ")));
    }

    let data = result
        .data
        .ok_or_else(|| "Linear response missing data field".to_string())?;

    if !data.issue_create.success {
        return Err("Linear issueCreate returned success=false".to_string());
    }

    let issue = data
        .issue_create
        .issue
        .ok_or_else(|| "Linear issueCreate returned no issue".to_string())?;

    Ok(LinearIssueResult {
        _id: issue.id,
        identifier: issue.identifier,
        url: issue.url,
    })
}

/// Upload a screenshot to Linear via the two-step file upload flow.
///
/// 1. Call `fileUpload` mutation to get an upload URL and asset URL
/// 2. PUT the binary bytes to the upload URL with returned headers
///
/// Returns the permanent asset URL for embedding in issue descriptions.
async fn upload_screenshot(
    api_key: &str,
    image_bytes: &[u8],
    filename: &str,
    content_type: &str,
) -> Result<String, String> {
    let http = crate::http_client()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    // Step 1: Get upload URL from Linear
    let query = r#"
        mutation FileUpload($contentType: String!, $filename: String!, $size: Int!) {
            fileUpload(contentType: $contentType, filename: $filename, size: $size) {
                uploadFile {
                    assetUrl
                    headers { key value }
                }
            }
        }
    "#;

    let variables = serde_json::json!({
        "contentType": content_type,
        "filename": filename,
        "size": image_bytes.len(),
    });

    let payload = serde_json::json!({
        "query": query,
        "variables": variables,
    });

    let response = http
        .post(LINEAR_API_URL)
        .header("Authorization", api_key)
        .header("Content-Type", "application/json")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Linear fileUpload request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Linear fileUpload API returned {status}: {body}"));
    }

    let result: GraphQLResponse<FileUploadData> = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Linear fileUpload response: {e}"))?;

    if let Some(errors) = result.errors {
        let msgs: Vec<&str> = errors.iter().map(|e| e.message.as_str()).collect();
        return Err(format!("Linear fileUpload GraphQL errors: {}", msgs.join(", ")));
    }

    let upload_file = result
        .data
        .and_then(|d| d.file_upload.upload_file)
        .ok_or_else(|| "Linear fileUpload returned no uploadFile".to_string())?;

    let asset_url = upload_file.asset_url.clone();

    // Step 2: PUT the image bytes to the upload URL
    // The asset_url doubles as the upload target — Linear returns headers
    // that must be included in the PUT request.
    let mut put_request = http
        .put(&asset_url)
        .body(image_bytes.to_vec())
        .timeout(std::time::Duration::from_secs(30));

    for header in &upload_file.headers {
        put_request = put_request.header(&header.key, &header.value);
    }

    let put_response = put_request
        .send()
        .await
        .map_err(|e| format!("Linear file PUT failed: {e}"))?;

    if !put_response.status().is_success() {
        let status = put_response.status();
        return Err(format!("Linear file PUT returned {status}"));
    }

    Ok(asset_url)
}

/// Build the issue title from feedback type and description.
///
/// Format: `[feedback] {emoji} {first 80 chars of description}`
pub fn build_issue_title(feedback_type: &str, description: &str) -> String {
    let emoji = match feedback_type {
        "bug" => "\u{1f41b}",       // 🐛
        "feature" => "\u{1f4a1}",   // 💡
        "question" => "\u{2753}",   // ❓
        _ => "\u{1f4dd}",           // 📝
    };

    let truncated: String = description.chars().take(80).collect();
    let suffix = if description.chars().count() > 80 { "..." } else { "" };

    format!("[feedback] {emoji} {truncated}{suffix}")
}

/// Build the issue description markdown from feedback data.
pub fn build_issue_description(
    feedback: &FeedbackIssueInput,
    screenshot_url: Option<&str>,
) -> String {
    let workspace_display = feedback
        .workspace_name
        .as_deref()
        .unwrap_or("(no workspace)");

    let page_url_display = feedback.page_url.as_deref().unwrap_or("N/A");

    let mut md = String::new();

    // Header info
    md.push_str(&format!("**Type:** {}\n", feedback.feedback_type));
    md.push_str(&format!("**From:** {}\n", feedback.user_email));
    md.push_str(&format!("**Workspace:** {}\n", workspace_display));
    md.push_str(&format!("**Page:** {}\n", page_url_display));
    md.push_str(&format!("**Timestamp:** {}\n", feedback.timestamp));
    md.push_str(&format!("**Feedback ID:** `{}`\n", feedback.feedback_id));

    // Description
    md.push_str("\n---\n\n");
    md.push_str("## Description\n\n");
    md.push_str(&feedback.description);
    md.push('\n');

    // Screenshot
    if let Some(url) = screenshot_url {
        md.push_str("\n## Screenshot\n\n");
        md.push_str(&format!("![Screenshot]({url})\n"));
    }

    // Technical context
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

    // Footer
    md.push_str("\n---\n");
    md.push_str("*Auto-created from in-app feedback*\n");

    md
}

/// Get Linear label IDs for a given feedback type.
///
/// Always includes "needs-triage". Adds the type-specific label.
pub fn label_ids_for_type(feedback_type: &str) -> Vec<String> {
    let type_label = match feedback_type {
        "bug" => LABEL_BUG,
        "feature" => LABEL_FEATURE,
        "question" => LABEL_IMPROVEMENT,
        _ => LABEL_IMPROVEMENT,
    };

    vec![
        LABEL_NEEDS_TRIAGE.to_string(),
        type_label.to_string(),
    ]
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
        // [feedback] + emoji + space + 80 chars + "..."
        assert!(title.ends_with("..."));
        assert!(title.contains(&"a".repeat(80)));
    }

    #[test]
    fn build_title_no_truncation_for_short() {
        let title = build_issue_title("question", "Short desc");
        assert!(!title.ends_with("..."));
        assert!(title.contains("Short desc"));
    }

    #[test]
    fn label_ids_bug() {
        let labels = label_ids_for_type("bug");
        assert_eq!(labels.len(), 2);
        assert!(labels.contains(&LABEL_NEEDS_TRIAGE.to_string()));
        assert!(labels.contains(&LABEL_BUG.to_string()));
    }

    #[test]
    fn label_ids_feature() {
        let labels = label_ids_for_type("feature");
        assert!(labels.contains(&LABEL_FEATURE.to_string()));
    }

    #[test]
    fn label_ids_question_maps_to_improvement() {
        let labels = label_ids_for_type("question");
        assert!(labels.contains(&LABEL_IMPROVEMENT.to_string()));
    }

    #[test]
    fn build_description_includes_all_sections() {
        let input = FeedbackIssueInput {
            feedback_id: "fb-test123".into(),
            feedback_type: "bug".into(),
            description: "The chart is not rendering correctly".into(),
            user_email: "user@example.com".into(),
            workspace_name: Some("Test Workspace".into()),
            page_url: Some("https://app.kyomi.ai/dashboards".into()),
            browser: Some("Chrome 120".into()),
            os: Some("macOS 14".into()),
            console_errors: vec![serde_json::json!({"message": "TypeError: cannot read property"})],
            failed_requests: vec![serde_json::json!({"method": "GET", "url": "/api/v1/data", "status": "500"})],
            timestamp: "2026-04-16 10:00 UTC".into(),
        };

        let desc = build_issue_description(&input, Some("https://linear.app/screenshot.png"));

        assert!(desc.contains("**Type:** bug"));
        assert!(desc.contains("**From:** user@example.com"));
        assert!(desc.contains("**Workspace:** Test Workspace"));
        assert!(desc.contains("## Description"));
        assert!(desc.contains("The chart is not rendering correctly"));
        assert!(desc.contains("## Screenshot"));
        assert!(desc.contains("![Screenshot](https://linear.app/screenshot.png)"));
        assert!(desc.contains("## Technical Context"));
        assert!(desc.contains("Chrome 120"));
        assert!(desc.contains("macOS 14"));
        assert!(desc.contains("Console Errors (1)"));
        assert!(desc.contains("TypeError: cannot read property"));
        assert!(desc.contains("Failed Requests (1)"));
        assert!(desc.contains("Auto-created from in-app feedback"));
    }

    #[test]
    fn build_description_without_optional_fields() {
        let input = FeedbackIssueInput {
            feedback_id: "fb-min123".into(),
            feedback_type: "feature".into(),
            description: "Add dark mode".into(),
            user_email: "user@example.com".into(),
            workspace_name: None,
            page_url: None,
            browser: None,
            os: None,
            console_errors: vec![],
            failed_requests: vec![],
            timestamp: "2026-04-16 10:00 UTC".into(),
        };

        let desc = build_issue_description(&input, None);

        assert!(desc.contains("(no workspace)"));
        assert!(desc.contains("N/A")); // page URL
        assert!(!desc.contains("## Screenshot"));
        assert!(!desc.contains("## Technical Context"));
    }
}
