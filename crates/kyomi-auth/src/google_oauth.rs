// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google OAuth service — authorization URL construction, token exchange, user info.
//!
//! Wire-compatible with Python's `GoogleOAuthService`.
//! Uses direct HTTP calls (reqwest) instead of a Google SDK.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Google API endpoints
// ---------------------------------------------------------------------------

pub const GOOGLE_AUTH_URI: &str = "https://accounts.google.com/o/oauth2/auth";
pub const GOOGLE_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
pub const GOOGLE_USER_INFO_URI: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
pub const GOOGLE_PROJECTS_URI: &str =
    "https://cloudresourcemanager.googleapis.com/v1/projects";

// ---------------------------------------------------------------------------
// Scopes
// ---------------------------------------------------------------------------

/// Minimal scopes for login (identify the user).
pub const LOGIN_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
];

/// Full scopes for BigQuery access (connect flow).
pub const BIGQUERY_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/bigquery.readonly",
    "https://www.googleapis.com/auth/cloudplatformprojects.readonly",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
];

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Google token exchange response.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: Option<i64>,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
}

/// Google userinfo response.
#[derive(Debug, Deserialize)]
pub struct GoogleUserInfo {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    pub picture: Option<String>,
    pub verified_email: Option<bool>,
}

/// Structured OAuth data stored encrypted in `users.oauth_data`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OAuthData {
    pub google_id: Option<String>,
    pub oauth_provider: Option<String>,
    pub picture: Option<String>,
    pub last_oauth_login: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub google_oauth_tokens: Option<GoogleOAuthTokens>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_reconnect_cancelled: Option<bool>,
}

/// Google OAuth tokens stored for BigQuery access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleOAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: String,
    pub scope: String,
    pub expires_in: Option<i64>,
    pub expires_at: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
}

// ---------------------------------------------------------------------------
// Authorization URL
// ---------------------------------------------------------------------------

/// Build a Google OAuth authorization URL.
///
/// - `login` flow: minimal scopes, no offline access, optional consent prompt
/// - `bigquery` flow: full scopes, offline access, forced consent
pub fn build_authorization_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    scopes: &[&str],
    force_consent: bool,
    offline_access: bool,
) -> String {
    let scope = scopes.join(" ");

    let mut params = url::form_urlencoded::Serializer::new(String::new());
    params
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", &scope)
        .append_pair("state", state)
        .append_pair("include_granted_scopes", "true");

    if offline_access {
        params.append_pair("access_type", "offline");
    }

    if force_consent {
        params.append_pair("prompt", "consent");
    }

    format!("{GOOGLE_AUTH_URI}?{}", params.finish())
}

// ---------------------------------------------------------------------------
// Token exchange
// ---------------------------------------------------------------------------

/// Exchange an authorization code for tokens.
pub async fn exchange_code_for_tokens(
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> kyomi_core::Result<TokenResponse> {
    let client = crate::http_client()?;

    let resp = client
        .post(GOOGLE_TOKEN_URI)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Google token exchange failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::BadRequest(format!(
            "Google token exchange failed ({status}): {body}"
        )));
    }

    resp.json::<TokenResponse>()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to parse token response: {e}")))
}

// ---------------------------------------------------------------------------
// Token refresh
// ---------------------------------------------------------------------------

/// Refresh a Google OAuth access token using the application's credentials.
///
/// This is for the `kyomi_oauth` auth mode where the user connected via the
/// app's own Google OAuth client. Uses `GOOGLE_OAUTH_CLIENT_ID` /
/// `GOOGLE_OAUTH_CLIENT_SECRET` (not per-datasource credentials).
///
/// Matches Python's `credentials.refresh(request)` in `get_oauth_credentials()`.
pub async fn refresh_access_token(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> kyomi_core::Result<TokenResponse> {
    let client = crate::http_client()?;

    let resp = client
        .post(GOOGLE_TOKEN_URI)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Google token refresh failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "Google token refresh failed ({status}): {body}"
        )));
    }

    resp.json::<TokenResponse>()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to parse refresh response: {e}")))
}

/// Check if a `GoogleOAuthTokens` access token is expired or about to expire.
///
/// Uses a 300-second (5-minute) buffer matching the Python implementation.
/// Returns `true` if expired, about to expire, or if no expiry info is available.
pub fn is_token_expired(tokens: &GoogleOAuthTokens) -> bool {
    const BUFFER_SECS: i64 = 300;

    if let Some(ref expires_at_str) = tokens.expires_at {
        let s = expires_at_str.trim();
        if s.is_empty() {
            return true;
        }

        // Try RFC 3339 (e.g., "2025-06-15T12:00:00+00:00" or "2025-06-15T12:00:00Z")
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
            let now = chrono::Utc::now();
            let buffer = chrono::Duration::seconds(BUFFER_SECS);
            return now >= dt.with_timezone(&chrono::Utc) - buffer;
        }

        // Try ISO 8601 without timezone (assume UTC)
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
            let now = chrono::Utc::now();
            let buffer = chrono::Duration::seconds(BUFFER_SECS);
            return now >= naive.and_utc() - buffer;
        }

        // Try with fractional seconds
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
            let now = chrono::Utc::now();
            let buffer = chrono::Duration::seconds(BUFFER_SECS);
            return now >= naive.and_utc() - buffer;
        }
    }

    // No expiry info or unparseable — assume expired (safe default)
    true
}

// ---------------------------------------------------------------------------
// Centralized token resolution — THE single entry point
// ---------------------------------------------------------------------------

/// Get a valid Google OAuth access token for the given user.
///
/// This is the **single centralized method** for obtaining Google OAuth tokens.
/// It mirrors Python's `GoogleOAuthService.get_oauth_credentials()`:
///
/// 1. Reads the user's encrypted `oauth_data` from the database
/// 2. Checks if the access token is expired (300s buffer)
/// 3. If expired, refreshes using the app's Google OAuth client credentials
/// 4. Persists the refreshed tokens back to the database
/// 5. Returns the valid `GoogleOAuthTokens`
///
/// **All code paths that need a Google access token MUST use this function.**
/// Do NOT read `oauth_data` and extract `access_token` directly — that bypasses
/// refresh and will break when tokens expire.
pub async fn ensure_valid_google_token(
    db: &kyomi_core::DbPool,
    user_id: &str,
    encryption_key: &[u8; 32],
    client_id: &str,
    client_secret: &str,
) -> kyomi_core::Result<GoogleOAuthTokens> {
    // 1. Read user from DB
    let db_user = crate::user_service::get_user_by_id(db, user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))?;

    // 2. Decrypt and parse oauth_data
    let mut oauth_data = parse_oauth_data(db_user.oauth_data.as_deref(), encryption_key)?
        .ok_or_else(|| {
            kyomi_core::Error::BadRequest(
                "No Google OAuth data found. Please connect your Google account first.".into(),
            )
        })?;

    let mut tokens = oauth_data.google_oauth_tokens.take().ok_or_else(|| {
        kyomi_core::Error::BadRequest(
            "No BigQuery tokens found. Please connect with BigQuery scopes.".into(),
        )
    })?;

    // 3. Check expiry and refresh if needed
    if is_token_expired(&tokens) {
        if let Some(ref refresh_token) = tokens.refresh_token {
            tracing::info!(user_id = %user_id, "Google OAuth token expired, refreshing");

            let refreshed = refresh_access_token(client_id, client_secret, refresh_token).await?;

            // Update tokens with refreshed values
            tokens.access_token = refreshed.access_token;
            if let Some(expires_in) = refreshed.expires_in {
                let expires_at = chrono::Utc::now() + chrono::Duration::seconds(expires_in);
                tokens.expires_at = Some(expires_at.to_rfc3339());
                tokens.expires_in = Some(expires_in);
            }
            if let Some(new_refresh) = refreshed.refresh_token {
                tokens.refresh_token = Some(new_refresh);
            }

            // 4. Persist refreshed tokens back to DB
            oauth_data.google_oauth_tokens = Some(tokens.clone());
            let encrypted = build_oauth_data(&oauth_data, encryption_key)?;
            crate::user_service::update_user_oauth_data(db, user_id, Some(&encrypted)).await?;

            tracing::info!(user_id = %user_id, "Google OAuth token refreshed and persisted");
        } else {
            return Err(kyomi_core::Error::BadRequest(
                "Google OAuth token expired and no refresh token available. \
                 Please reconnect your Google account."
                    .into(),
            ));
        }
    }

    // 5. Return valid tokens
    Ok(tokens)
}

// ---------------------------------------------------------------------------
// User info
// ---------------------------------------------------------------------------

/// Fetch user info from Google using an access token.
pub async fn get_user_info(access_token: &str) -> kyomi_core::Result<GoogleUserInfo> {
    let client = crate::http_client()?;

    let resp = client
        .get(GOOGLE_USER_INFO_URI)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Google userinfo request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::BadRequest(format!(
            "Google userinfo request failed ({status}): {body}"
        )));
    }

    resp.json::<GoogleUserInfo>()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to parse userinfo: {e}")))
}

// ---------------------------------------------------------------------------
// OAuth data helpers
// ---------------------------------------------------------------------------

/// Decrypt and parse `users.oauth_data` from the database.
pub fn parse_oauth_data(
    encrypted: Option<&str>,
    key: &[u8; 32],
) -> kyomi_core::Result<Option<OAuthData>> {
    let Some(encrypted) = encrypted else {
        return Ok(None);
    };

    if encrypted.is_empty() {
        return Ok(None);
    }

    let json_str = crate::encryption::decrypt(encrypted, key)?;
    let data: OAuthData = serde_json::from_str(&json_str)?;
    Ok(Some(data))
}

/// Serialize and encrypt `OAuthData` for storage in `users.oauth_data`.
pub fn build_oauth_data(
    data: &OAuthData,
    key: &[u8; 32],
) -> kyomi_core::Result<String> {
    let json_str = serde_json::to_string(data)?;
    crate::encryption::encrypt(&json_str, key)
}

// ---------------------------------------------------------------------------
// Scope checking
// ---------------------------------------------------------------------------

/// Check if the stored scopes include BigQuery access.
pub fn has_bigquery_scopes(scopes_str: &str) -> bool {
    scopes_str.contains("bigquery") || scopes_str.contains("cloud-platform")
}

/// Determine BigQuery access level from scopes.
pub fn bigquery_access_level(scopes_str: &str) -> &'static str {
    if scopes_str.contains("cloud-platform") {
        "full"
    } else if scopes_str.contains("bigquery") {
        "readonly"
    } else {
        "none"
    }
}
