// SPDX-License-Identifier: LicenseRef-Alytic-Enterprise

//! Reusable Slack HTTP client with retry, rate-limit handling, and typed responses.
//!
//! Extracted from `kyomi-agent/src/alert.rs` and extended with the full set of
//! Slack Web API methods needed for OAuth flows, channel management, file
//! uploads, and bot interactions.
//!
//! All methods include:
//! - 3-attempt retry with exponential backoff (1s, 2s, 4s)
//! - HTTP 429 rate-limit detection with `Retry-After` header
//! - Configurable timeouts: 30s for POST, 10s for GET
//! - Proper error types

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Slack Web API endpoints.
pub const POST_MESSAGE_URL: &str = "https://slack.com/api/chat.postMessage";
pub const UPDATE_MESSAGE_URL: &str = "https://slack.com/api/chat.update";
pub const POST_EPHEMERAL_URL: &str = "https://slack.com/api/chat.postEphemeral";
pub const CONVERSATIONS_INFO_URL: &str = "https://slack.com/api/conversations.info";
pub const CONVERSATIONS_LIST_URL: &str = "https://slack.com/api/conversations.list";
pub const USERS_INFO_URL: &str = "https://slack.com/api/users.info";
pub const OAUTH_ACCESS_URL: &str = "https://slack.com/api/oauth.v2.access";
pub const FILES_GET_UPLOAD_URL_URL: &str = "https://slack.com/api/files.getUploadURLExternal";
pub const FILES_COMPLETE_UPLOAD_URL: &str = "https://slack.com/api/files.completeUploadExternal";
pub const OAUTH_AUTHORIZE_URL: &str = "https://slack.com/oauth/v2/authorize";

/// Bot OAuth scopes for workspace installation.
pub const SLACK_BOT_SCOPES: &str =
    "chat:write,channels:read,groups:read,commands,app_mentions:read,files:write,im:history,im:read";

/// User OAuth scopes for individual account linking.
pub const SLACK_USER_SCOPES: &str = "chat:write,users:read";

/// Maximum text length for a Slack section block text field.
pub const SLACK_MAX_BLOCK_TEXT_LENGTH: usize = 2800;

/// Result from uploading a file for use in Block Kit image blocks.
///
/// Unlike [`SlackClient::upload_file`] which attaches directly to a channel,
/// this represents a file uploaded *without* channel attachment — the `id`
/// can be referenced in `{"type": "image", "slack_file": {"id": ...}}`.
#[derive(Debug, Clone)]
pub struct SlackFileUpload {
    /// Slack file ID (e.g. `F0123456789`).
    pub id: String,
    /// Private URL for the file (if returned by Slack).
    pub url_private: Option<String>,
}

/// Cached user timezone TTL in hours.
pub const SLACK_TIMEZONE_CACHE_HOURS: i64 = 24;

/// Maximum retry attempts for Slack API calls.
const MAX_RETRIES: u32 = 3;

/// Timeout for POST requests (30 seconds).
const POST_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for GET requests (10 seconds).
const GET_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Result from `chat.postMessage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackPostResult {
    /// Whether the API call succeeded.
    pub ok: bool,
    /// Message timestamp (returned on success).
    pub ts: Option<String>,
    /// Channel the message was posted to.
    pub channel: Option<String>,
    /// Slack error code (returned on failure).
    pub error: Option<String>,
}

/// Channel information from `conversations.info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackChannelInfo {
    pub id: String,
    pub name: String,
    pub is_member: bool,
    pub is_private: bool,
}

/// Channel entry from `conversations.list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackChannel {
    pub id: String,
    pub name: String,
    pub is_private: bool,
}

/// User info from `users.info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackUserInfo {
    pub id: String,
    pub real_name: Option<String>,
    pub name: Option<String>,
    pub tz: Option<String>,
    pub tz_label: Option<String>,
}

/// Response from `oauth.v2.access`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthAccessResponse {
    pub ok: bool,
    pub access_token: Option<String>,
    pub bot_user_id: Option<String>,
    pub team: Option<OAuthTeam>,
    pub authed_user: Option<OAuthAuthedUser>,
    pub error: Option<String>,
}

/// Team info in the OAuth response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTeam {
    pub id: String,
    pub name: Option<String>,
}

/// Authenticated user info in the OAuth response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthAuthedUser {
    pub id: String,
    pub access_token: Option<String>,
}

// ---------------------------------------------------------------------------
// SlackClient
// ---------------------------------------------------------------------------

/// Reusable Slack HTTP client.
///
/// Wraps a shared `reqwest::Client` and provides typed methods for all Slack
/// Web API calls with built-in retry and rate-limit handling.
///
/// This struct is cheaply cloneable and should be shared via `Arc` in `AppState`.
#[derive(Debug, Clone)]
pub struct SlackClient {
    http: reqwest::Client,
}

/// Extract detailed error messages from a Slack API response.
///
/// Slack returns `response_metadata.messages` with human-readable detail
/// about what went wrong (e.g. which block is invalid and why).
fn extract_slack_error_detail(data: &serde_json::Value) -> String {
    let messages = data
        .get("response_metadata")
        .and_then(|m| m.get("messages"))
        .and_then(|m| m.as_array());

    match messages {
        Some(msgs) if !msgs.is_empty() => msgs
            .iter()
            .filter_map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join("; "),
        _ => String::new(),
    }
}

impl SlackClient {
    /// Create a new `SlackClient` with a shared reqwest client.
    pub fn new() -> kyomi_core::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("kyomi-slack/1.0")
            .build()
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("Failed to build Slack HTTP client: {e}"))
            })?;
        Ok(Self { http })
    }

    // -----------------------------------------------------------------------
    // Chat methods
    // -----------------------------------------------------------------------

    /// Post a message to a Slack channel.
    ///
    /// Returns the posted message's timestamp on success.
    pub async fn post_message(
        &self,
        bot_token: &str,
        channel: &str,
        text: &str,
        blocks: Option<&[serde_json::Value]>,
        thread_ts: Option<&str>,
    ) -> kyomi_core::Result<SlackPostResult> {
        let mut payload = serde_json::json!({
            "channel": channel,
            "text": text,
            "unfurl_links": false,
            "unfurl_media": false,
        });

        if let Some(blocks) = blocks {
            payload["blocks"] = serde_json::Value::Array(blocks.to_vec());
        }
        if let Some(ts) = thread_ts {
            payload["thread_ts"] = serde_json::Value::String(ts.to_string());
        }

        let data = self
            .post_with_retry(POST_MESSAGE_URL, bot_token, &payload)
            .await?;

        let ok = data.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        let ts = data.get("ts").and_then(|v| v.as_str()).map(String::from);
        let channel_id = data
            .get("channel")
            .and_then(|v| v.as_str())
            .map(String::from);
        let error = data
            .get("error")
            .and_then(|v| v.as_str())
            .map(String::from);

        if !ok {
            let detail = extract_slack_error_detail(&data);
            let blocks_json = payload.get("blocks")
                .map(|b| serde_json::to_string_pretty(b).unwrap_or_default())
                .unwrap_or_default();
            tracing::error!(
                error = ?error,
                detail = %detail,
                blocks = %blocks_json,
                "chat.postMessage failed"
            );
        }

        Ok(SlackPostResult {
            ok,
            ts,
            channel: channel_id,
            error,
        })
    }

    /// Update an existing message in a Slack channel.
    pub async fn update_message(
        &self,
        bot_token: &str,
        channel: &str,
        ts: &str,
        text: &str,
        blocks: Option<&[serde_json::Value]>,
    ) -> kyomi_core::Result<()> {
        let mut payload = serde_json::json!({
            "channel": channel,
            "ts": ts,
            "text": text,
            "unfurl_links": false,
            "unfurl_media": false,
        });

        if let Some(blocks) = blocks {
            payload["blocks"] = serde_json::Value::Array(blocks.to_vec());
        }

        let data = self
            .post_with_retry(UPDATE_MESSAGE_URL, bot_token, &payload)
            .await?;

        if data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = data
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            let detail = extract_slack_error_detail(&data);
            let blocks_json = payload.get("blocks")
                .map(|b| serde_json::to_string_pretty(b).unwrap_or_default())
                .unwrap_or_default();
            tracing::error!(
                error = %error,
                detail = %detail,
                blocks = %blocks_json,
                "chat.update failed"
            );
            return Err(kyomi_core::Error::Internal(format!(
                "chat.update failed: {error} — {detail}"
            )));
        }

        Ok(())
    }

    /// Post an ephemeral message visible only to a specific user.
    pub async fn post_ephemeral(
        &self,
        bot_token: &str,
        channel: &str,
        user: &str,
        text: &str,
        blocks: Option<&[serde_json::Value]>,
    ) -> kyomi_core::Result<()> {
        let mut payload = serde_json::json!({
            "channel": channel,
            "user": user,
            "text": text,
        });

        if let Some(blocks) = blocks {
            payload["blocks"] = serde_json::Value::Array(blocks.to_vec());
        }

        let data = self
            .post_with_retry(POST_EPHEMERAL_URL, bot_token, &payload)
            .await?;

        if data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = data
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            return Err(kyomi_core::Error::Internal(format!(
                "chat.postEphemeral failed: {error}"
            )));
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Channel methods
    // -----------------------------------------------------------------------

    /// Get info about a Slack channel. Used to verify bot membership.
    pub async fn conversations_info(
        &self,
        bot_token: &str,
        channel: &str,
    ) -> kyomi_core::Result<SlackChannelInfo> {
        let data = self
            .get_with_retry(
                CONVERSATIONS_INFO_URL,
                bot_token,
                &[("channel", channel)],
            )
            .await?;

        if data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = data
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            return Err(kyomi_core::Error::Internal(format!(
                "conversations.info failed: {error}"
            )));
        }

        let ch = data
            .get("channel")
            .ok_or_else(|| kyomi_core::Error::Internal("missing channel in response".into()))?;

        Ok(SlackChannelInfo {
            id: ch
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            name: ch
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            is_member: ch
                .get("is_member")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            is_private: ch
                .get("is_private")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    }

    /// List channels where the bot is a member (paginated, returns all pages).
    pub async fn conversations_list(
        &self,
        bot_token: &str,
    ) -> kyomi_core::Result<Vec<SlackChannel>> {
        let mut channels = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut params = vec![
                ("types", "public_channel,private_channel".to_string()),
                ("exclude_archived", "true".to_string()),
                ("limit", "200".to_string()),
            ];
            if let Some(ref c) = cursor {
                params.push(("cursor", c.clone()));
            }

            let query_params: Vec<(&str, &str)> =
                params.iter().map(|(k, v)| (*k, v.as_str())).collect();

            let data = self
                .get_with_retry(CONVERSATIONS_LIST_URL, bot_token, &query_params)
                .await?;

            if data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                let error = data
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown_error");
                return Err(kyomi_core::Error::Internal(format!(
                    "conversations.list failed: {error}"
                )));
            }

            if let Some(channel_list) = data.get("channels").and_then(|v| v.as_array()) {
                for ch in channel_list {
                    // Only include channels where the bot is a member
                    let is_member = ch
                        .get("is_member")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if is_member {
                        channels.push(SlackChannel {
                            id: ch
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            name: ch
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            is_private: ch
                                .get("is_private")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false),
                        });
                    }
                }
            }

            // Check for next page
            cursor = data
                .get("response_metadata")
                .and_then(|m| m.get("next_cursor"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from);

            if cursor.is_none() {
                break;
            }
        }

        // Sort by name for consistent display
        channels.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(channels)
    }

    // -----------------------------------------------------------------------
    // User methods
    // -----------------------------------------------------------------------

    /// Get user profile info (name, timezone).
    pub async fn users_info(
        &self,
        bot_token: &str,
        user_id: &str,
    ) -> kyomi_core::Result<SlackUserInfo> {
        let data = self
            .get_with_retry(USERS_INFO_URL, bot_token, &[("user", user_id)])
            .await?;

        if data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = data
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            return Err(kyomi_core::Error::Internal(format!(
                "users.info failed: {error}"
            )));
        }

        let user = data
            .get("user")
            .ok_or_else(|| kyomi_core::Error::Internal("missing user in response".into()))?;

        Ok(SlackUserInfo {
            id: user
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            real_name: user.get("real_name").and_then(|v| v.as_str()).map(String::from),
            name: user.get("name").and_then(|v| v.as_str()).map(String::from),
            tz: user.get("tz").and_then(|v| v.as_str()).map(String::from),
            tz_label: user
                .get("tz_label")
                .and_then(|v| v.as_str())
                .map(String::from),
        })
    }

    // -----------------------------------------------------------------------
    // OAuth
    // -----------------------------------------------------------------------

    /// Exchange an OAuth authorization code for tokens.
    pub async fn exchange_code(
        &self,
        client_id: &str,
        client_secret: &str,
        code: &str,
        redirect_uri: &str,
    ) -> kyomi_core::Result<OAuthAccessResponse> {
        let payload = serde_json::json!({});

        let mut last_error = String::new();

        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let backoff = Duration::from_secs(1 << attempt);
                tokio::time::sleep(backoff).await;
            }

            let response = match self
                .http
                .post(OAUTH_ACCESS_URL)
                .form(&[
                    ("client_id", client_id),
                    ("client_secret", client_secret),
                    ("code", code),
                    ("redirect_uri", redirect_uri),
                ])
                .timeout(POST_TIMEOUT)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    last_error = format!("HTTP request failed: {e}");
                    warn!(
                        attempt = attempt + 1,
                        error = %e,
                        "Slack OAuth exchange failed, retrying"
                    );
                    continue;
                }
            };

            let status = response.status();
            if status.is_server_error() {
                last_error = format!("HTTP {status}");
                warn!(
                    attempt = attempt + 1,
                    status = %status,
                    "Slack OAuth server error, retrying"
                );
                continue;
            }

            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = Self::extract_retry_after(&payload);
                last_error = "rate limited".into();
                warn!(
                    attempt = attempt + 1,
                    retry_after_secs = retry_after,
                    "Slack OAuth rate limited"
                );
                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                continue;
            }

            let data: OAuthAccessResponse = match response.json().await {
                Ok(d) => d,
                Err(e) => {
                    last_error = format!("JSON parse failed: {e}");
                    warn!(
                        attempt = attempt + 1,
                        error = %e,
                        "Failed to parse Slack OAuth response, retrying"
                    );
                    continue;
                }
            };

            return Ok(data);
        }

        Err(kyomi_core::Error::Internal(format!(
            "Slack OAuth exchange failed after {MAX_RETRIES} attempts: {last_error}"
        )))
    }

    // -----------------------------------------------------------------------
    // File upload (two-step: getUploadURLExternal + completeUploadExternal)
    // -----------------------------------------------------------------------

    /// Upload a file to a Slack channel.
    ///
    /// Uses the two-step upload flow:
    /// 1. `files.getUploadURLExternal` to get an upload URL
    /// 2. HTTP PUT to upload the file data
    /// 3. `files.completeUploadExternal` to attach the file to a channel
    pub async fn upload_file(
        &self,
        bot_token: &str,
        channel: &str,
        filename: &str,
        data: Vec<u8>,
        thread_ts: Option<&str>,
    ) -> kyomi_core::Result<()> {
        // Step 1: Get upload URL
        let file_size = data.len();
        let step1_data = self
            .get_with_retry(
                FILES_GET_UPLOAD_URL_URL,
                bot_token,
                &[
                    ("filename", filename),
                    ("length", &file_size.to_string()),
                ],
            )
            .await?;

        if step1_data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = step1_data
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            return Err(kyomi_core::Error::Internal(format!(
                "files.getUploadURLExternal failed: {error}"
            )));
        }

        let upload_url = step1_data
            .get("upload_url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::Internal("missing upload_url in response".into())
            })?;

        let file_id = step1_data
            .get("file_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::Internal("missing file_id in response".into())
            })?
            .to_string();

        // Step 2: Upload file data via POST (matches Python's httpx client.post)
        let upload_response = self
            .http
            .post(upload_url)
            .header("Content-Type", "application/octet-stream")
            .body(data)
            .timeout(POST_TIMEOUT)
            .send()
            .await
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("file upload POST failed: {e}"))
            })?;

        if !upload_response.status().is_success() {
            return Err(kyomi_core::Error::Internal(format!(
                "file upload POST returned HTTP {}",
                upload_response.status()
            )));
        }

        // Step 3: Complete upload and attach to channel.
        // Use form-encoded data (not JSON) to match Python's `data=` parameter.
        let files_json = serde_json::to_string(&serde_json::json!(
            [{"id": file_id, "title": filename}]
        ))
        .map_err(|e| kyomi_core::Error::Internal(format!("files JSON serialization failed: {e}")))?;

        let mut form_params = vec![
            ("files", files_json.as_str()),
            ("channel_id", channel),
        ];
        // thread_ts must be a String we can borrow — keep it alive
        let ts_string;
        if let Some(ts) = thread_ts {
            ts_string = ts.to_string();
            form_params.push(("thread_ts", &ts_string));
        }

        let step3_response = self
            .http
            .post(FILES_COMPLETE_UPLOAD_URL)
            .header("Authorization", format!("Bearer {bot_token}"))
            .form(&form_params)
            .timeout(POST_TIMEOUT)
            .send()
            .await
            .map_err(|e| {
                kyomi_core::Error::Internal(format!(
                    "files.completeUploadExternal request failed: {e}"
                ))
            })?;

        let step3_data: serde_json::Value = step3_response.json().await.map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "files.completeUploadExternal response parse failed: {e}"
            ))
        })?;

        if step3_data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = step3_data
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            return Err(kyomi_core::Error::Internal(format!(
                "files.completeUploadExternal failed: {error}"
            )));
        }

        info!(channel = %channel, filename = %filename, "Uploaded file to Slack");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // File upload for Block Kit image blocks (no channel attachment)
    // -----------------------------------------------------------------------

    /// Upload a file to Slack *without* attaching it to a channel.
    ///
    /// Returns a [`SlackFileUpload`] whose `id` can be referenced in
    /// Block Kit `image` blocks:
    /// ```json
    /// {"type": "image", "slack_file": {"id": "<file_id>"}, "alt_text": "..."}
    /// ```
    ///
    /// Steps 1–2 are identical to [`upload_file`]. Step 3 calls
    /// `completeUploadExternal` with `files` but **no** `channel_id`,
    /// so the file is uploaded but not posted to any channel.
    pub async fn upload_file_for_blocks(
        &self,
        bot_token: &str,
        filename: &str,
        title: &str,
        data: Vec<u8>,
    ) -> kyomi_core::Result<SlackFileUpload> {
        // Step 1: Get upload URL
        let file_size = data.len();
        let step1_data = self
            .get_with_retry(
                FILES_GET_UPLOAD_URL_URL,
                bot_token,
                &[
                    ("filename", filename),
                    ("length", &file_size.to_string()),
                ],
            )
            .await?;

        if step1_data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = step1_data
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            return Err(kyomi_core::Error::Internal(format!(
                "files.getUploadURLExternal failed: {error}"
            )));
        }

        let upload_url = step1_data
            .get("upload_url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::Internal("missing upload_url in response".into())
            })?;

        let file_id = step1_data
            .get("file_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                kyomi_core::Error::Internal("missing file_id in response".into())
            })?
            .to_string();

        // Step 2: Upload file data via POST (matches Python's httpx client.post)
        let upload_response = self
            .http
            .post(upload_url)
            .header("Content-Type", "application/octet-stream")
            .body(data)
            .timeout(POST_TIMEOUT)
            .send()
            .await
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("file upload POST failed: {e}"))
            })?;

        if !upload_response.status().is_success() {
            return Err(kyomi_core::Error::Internal(format!(
                "file upload POST returned HTTP {}",
                upload_response.status()
            )));
        }

        // Step 3: Complete upload WITHOUT channel_id — file is uploaded
        // but not posted to any channel, ready for Block Kit image blocks.
        // Use form-encoded data (not JSON) to match Python's `data=` parameter.
        let files_json = serde_json::to_string(&serde_json::json!(
            [{"id": file_id, "title": title}]
        ))
        .map_err(|e| kyomi_core::Error::Internal(format!("files JSON serialization failed: {e}")))?;

        let step3_response = self
            .http
            .post(FILES_COMPLETE_UPLOAD_URL)
            .header("Authorization", format!("Bearer {bot_token}"))
            .form(&[("files", files_json.as_str())])
            .timeout(POST_TIMEOUT)
            .send()
            .await
            .map_err(|e| {
                kyomi_core::Error::Internal(format!(
                    "files.completeUploadExternal request failed: {e}"
                ))
            })?;

        let step3_data: serde_json::Value = step3_response.json().await.map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "files.completeUploadExternal response parse failed: {e}"
            ))
        })?;

        if step3_data.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let error = step3_data
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            return Err(kyomi_core::Error::Internal(format!(
                "files.completeUploadExternal failed: {error}"
            )));
        }

        // Extract file ID and URL from the completeUploadExternal response.
        // The response file_id may differ from the step-1 file_id — always
        // prefer the response value (matches Python behaviour).
        let response_file = step3_data
            .get("files")
            .and_then(|f| f.as_array())
            .and_then(|arr| arr.first());

        let final_file_id = response_file
            .and_then(|f| f.get("id"))
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or(file_id);

        let url_private = response_file
            .and_then(|f| f.get("url_private"))
            .and_then(|v| v.as_str())
            .map(String::from);

        info!(filename = %filename, file_id = %final_file_id, "Uploaded file for Block Kit image blocks");

        Ok(SlackFileUpload {
            id: final_file_id,
            url_private,
        })
    }

    // -----------------------------------------------------------------------
    // Retry primitives
    // -----------------------------------------------------------------------

    /// POST with retry logic (3 attempts, exponential backoff, 429 handling).
    async fn post_with_retry(
        &self,
        url: &str,
        bot_token: &str,
        payload: &serde_json::Value,
    ) -> kyomi_core::Result<serde_json::Value> {
        let mut last_error = String::new();

        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let backoff = Duration::from_secs(1 << attempt); // 2s, 4s
                tokio::time::sleep(backoff).await;
            }

            let response = match self
                .http
                .post(url)
                .header("Authorization", format!("Bearer {bot_token}"))
                .json(payload)
                .timeout(POST_TIMEOUT)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    last_error = format!("HTTP request failed: {e}");
                    warn!(
                        attempt = attempt + 1,
                        max_attempts = MAX_RETRIES,
                        error = %e,
                        url = %url,
                        "Slack API POST failed, retrying"
                    );
                    continue;
                }
            };

            let status = response.status();
            if status.is_server_error() {
                last_error = format!("HTTP {status}");
                warn!(
                    attempt = attempt + 1,
                    status = %status,
                    url = %url,
                    "Slack API server error, retrying"
                );
                continue;
            }

            // Rate limiting
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(5);
                last_error = "rate limited".into();
                warn!(
                    attempt = attempt + 1,
                    retry_after_secs = retry_after,
                    url = %url,
                    "Slack rate limited, backing off"
                );
                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                continue;
            }

            let data: serde_json::Value = match response.json().await {
                Ok(d) => d,
                Err(e) => {
                    last_error = format!("JSON parse failed: {e}");
                    warn!(
                        attempt = attempt + 1,
                        error = %e,
                        url = %url,
                        "Failed to parse Slack response, retrying"
                    );
                    continue;
                }
            };

            return Ok(data);
        }

        Err(kyomi_core::Error::Internal(format!(
            "Slack API POST {url} failed after {MAX_RETRIES} attempts: {last_error}"
        )))
    }

    /// GET with retry logic (3 attempts, exponential backoff, 429 handling).
    async fn get_with_retry(
        &self,
        url: &str,
        bot_token: &str,
        query_params: &[(&str, &str)],
    ) -> kyomi_core::Result<serde_json::Value> {
        let mut last_error = String::new();

        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let backoff = Duration::from_secs(1 << attempt); // 2s, 4s
                tokio::time::sleep(backoff).await;
            }

            let response = match self
                .http
                .get(url)
                .header("Authorization", format!("Bearer {bot_token}"))
                .query(query_params)
                .timeout(GET_TIMEOUT)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    last_error = format!("HTTP request failed: {e}");
                    warn!(
                        attempt = attempt + 1,
                        max_attempts = MAX_RETRIES,
                        error = %e,
                        url = %url,
                        "Slack API GET failed, retrying"
                    );
                    continue;
                }
            };

            let status = response.status();
            if status.is_server_error() {
                last_error = format!("HTTP {status}");
                warn!(
                    attempt = attempt + 1,
                    status = %status,
                    url = %url,
                    "Slack API server error, retrying"
                );
                continue;
            }

            // Rate limiting
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(5);
                last_error = "rate limited".into();
                warn!(
                    attempt = attempt + 1,
                    retry_after_secs = retry_after,
                    url = %url,
                    "Slack rate limited, backing off"
                );
                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                continue;
            }

            let data: serde_json::Value = match response.json().await {
                Ok(d) => d,
                Err(e) => {
                    last_error = format!("JSON parse failed: {e}");
                    warn!(
                        attempt = attempt + 1,
                        error = %e,
                        url = %url,
                        "Failed to parse Slack response, retrying"
                    );
                    continue;
                }
            };

            return Ok(data);
        }

        Err(kyomi_core::Error::Internal(format!(
            "Slack API GET {url} failed after {MAX_RETRIES} attempts: {last_error}"
        )))
    }

    /// Extract Retry-After from a JSON response body (fallback to 5s).
    fn extract_retry_after(data: &serde_json::Value) -> u64 {
        data.get("headers")
            .and_then(|h| h.get("Retry-After"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(5)
    }
}


// ---------------------------------------------------------------------------
// Signature verification
// ---------------------------------------------------------------------------

/// A Slack signing secret that is guaranteed non-empty.
///
/// `verify_slack_signature` takes this rather than `&str` so that a missing or
/// empty secret cannot reach the verification path at all. Construction is the
/// only place the empty case is handled, and it fails there rather than
/// silently accepting every request.
///
/// Deliberately has no `Debug` impl: the secret must never be printable via
/// `{:?}` (e.g. an accidental `debug!(?config, ...)` log line). If a caller
/// needs to debug-print something containing this type, that is itself a sign
/// the value shouldn't be logged.
pub struct SlackSigningSecret(String);

impl SlackSigningSecret {
    /// Returns `None` if the secret is absent, empty, or whitespace-only.
    ///
    /// Trims surrounding whitespace before the emptiness check and stores the
    /// trimmed value — a secret with stray whitespace from an env var would
    /// otherwise produce silently-wrong HMACs.
    pub fn new(raw: Option<&str>) -> Option<Self> {
        let trimmed = raw?.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(Self(trimmed.to_string()))
        }
    }

    /// The trimmed secret bytes, for HMAC computation.
    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// Verify a Slack request signature using HMAC-SHA256.
///
/// Slack signs each request with `v0=HMAC-SHA256(signing_secret, "v0:{timestamp}:{body}")`.
/// The signature is sent in the `X-Slack-Signature` header and the timestamp
/// in `X-Slack-Request-Timestamp`.
///
/// Returns `true` if the signature is valid, `false` otherwise.
pub fn verify_slack_signature(
    signing_secret: &SlackSigningSecret,
    timestamp: &str,
    body: &[u8],
    signature: &str,
) -> bool {
    use sha2::Sha256;
    use hmac::{Hmac, Mac};

    if timestamp.is_empty() || signature.is_empty() {
        return false;
    }

    // Check timestamp is within 5 minutes (300 seconds)
    let ts: i64 = match timestamp.parse() {
        Ok(t) => t,
        Err(_) => return false,
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    if (now - ts).unsigned_abs() > 300 {
        warn!(
            timestamp = %timestamp,
            drift_secs = (now - ts).unsigned_abs(),
            "Slack request timestamp too old"
        );
        return false;
    }

    // Compute expected signature: v0=HMAC-SHA256(signing_secret, "v0:{timestamp}:{body}")
    let body_str = match std::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let sig_basestring = format!("v0:{timestamp}:{body_str}");

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = match HmacSha256::new_from_slice(signing_secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(sig_basestring.as_bytes());
    let result = mac.finalize();
    let expected = format!("v0={}", hex::encode(result.into_bytes()));

    // Constant-time comparison
    use subtle::ConstantTimeEq;
    expected.as_bytes().ct_eq(signature.as_bytes()).into()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slack_client_creates() {
        let client = SlackClient::new().unwrap();
        // Just verify it constructs without error
        let _ = format!("{:?}", client);
    }

    #[test]
    fn constants_have_expected_values() {
        assert_eq!(POST_MESSAGE_URL, "https://slack.com/api/chat.postMessage");
        assert_eq!(UPDATE_MESSAGE_URL, "https://slack.com/api/chat.update");
        assert_eq!(CONVERSATIONS_INFO_URL, "https://slack.com/api/conversations.info");
        assert_eq!(CONVERSATIONS_LIST_URL, "https://slack.com/api/conversations.list");
        assert_eq!(USERS_INFO_URL, "https://slack.com/api/users.info");
        assert_eq!(OAUTH_ACCESS_URL, "https://slack.com/api/oauth.v2.access");
        assert_eq!(SLACK_MAX_BLOCK_TEXT_LENGTH, 2800);
        assert_eq!(SLACK_TIMEZONE_CACHE_HOURS, 24);
    }

    #[test]
    fn bot_scopes_contains_expected() {
        assert!(SLACK_BOT_SCOPES.contains("chat:write"));
        assert!(SLACK_BOT_SCOPES.contains("channels:read"));
        assert!(SLACK_BOT_SCOPES.contains("commands"));
        assert!(SLACK_BOT_SCOPES.contains("files:write"));
    }

    #[test]
    fn user_scopes_contains_expected() {
        assert!(SLACK_USER_SCOPES.contains("chat:write"));
        assert!(SLACK_USER_SCOPES.contains("users:read"));
    }

    #[test]
    fn signing_secret_rejects_absent_empty_and_blank() {
        assert!(SlackSigningSecret::new(None).is_none());
        assert!(SlackSigningSecret::new(Some("")).is_none());
        assert!(SlackSigningSecret::new(Some("   ")).is_none());
    }

    #[test]
    fn signing_secret_trims_valid_value() {
        let secret = SlackSigningSecret::new(Some(" secret ")).unwrap();
        assert_eq!(secret.as_bytes(), b"secret");
    }

    #[test]
    fn verify_signature_empty_timestamp_fails() {
        let secret = SlackSigningSecret::new(Some("secret")).unwrap();
        assert!(!verify_slack_signature(&secret, "", b"body", "v0=abc"));
    }

    #[test]
    fn verify_signature_empty_signature_fails() {
        let secret = SlackSigningSecret::new(Some("secret")).unwrap();
        assert!(!verify_slack_signature(&secret, "12345", b"body", ""));
    }

    #[test]
    fn verify_signature_invalid_timestamp_fails() {
        let secret = SlackSigningSecret::new(Some("secret")).unwrap();
        assert!(!verify_slack_signature(&secret, "not_a_number", b"body", "v0=abc"));
    }

    #[test]
    fn verify_signature_known_valid() {
        // Use a known timestamp, body, and signing secret to compute an expected signature.
        use sha2::Sha256;
        use hmac::{Hmac, Mac};

        let secret = "test-signing-secret";
        let body = b"test-body-content";
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let timestamp = now.to_string();

        let sig_basestring = format!("v0:{timestamp}:{}", std::str::from_utf8(body).unwrap());
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(sig_basestring.as_bytes());
        let result = mac.finalize();
        let signature = format!("v0={}", hex::encode(result.into_bytes()));

        let secret = SlackSigningSecret::new(Some(secret)).unwrap();
        assert!(verify_slack_signature(&secret, &timestamp, body, &signature));
    }

    #[test]
    fn verify_signature_wrong_secret_fails() {
        use sha2::Sha256;
        use hmac::{Hmac, Mac};

        let secret = "correct-secret";
        let wrong_secret = "wrong-secret";
        let body = b"test-body";
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let timestamp = now.to_string();

        let sig_basestring = format!("v0:{timestamp}:{}", std::str::from_utf8(body).unwrap());
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(wrong_secret.as_bytes()).unwrap();
        mac.update(sig_basestring.as_bytes());
        let result = mac.finalize();
        let signature = format!("v0={}", hex::encode(result.into_bytes()));

        let secret = SlackSigningSecret::new(Some(secret)).unwrap();
        assert!(!verify_slack_signature(&secret, &timestamp, body, &signature));
    }

    #[test]
    fn verify_signature_old_timestamp_fails() {
        use sha2::Sha256;
        use hmac::{Hmac, Mac};

        let secret = "test-secret";
        let body = b"test-body";
        // 10 minutes ago -- too old
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let old_timestamp = (now - 600).to_string();

        let sig_basestring = format!("v0:{old_timestamp}:{}", std::str::from_utf8(body).unwrap());
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(sig_basestring.as_bytes());
        let result = mac.finalize();
        let signature = format!("v0={}", hex::encode(result.into_bytes()));

        let secret = SlackSigningSecret::new(Some(secret)).unwrap();
        assert!(!verify_slack_signature(&secret, &old_timestamp, body, &signature));
    }
}
