// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google OAuth service — authorization URL construction, token exchange, user info.
//!
//! Wire-compatible with Python's `GoogleOAuthService`.
//! Uses direct HTTP calls (reqwest) instead of a Google SDK.

use serde::{Deserialize, Serialize};

use crate::datasource_oauth::TokenResponse;

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
// Datasource provider helpers
// ---------------------------------------------------------------------------

/// Build a `UserContext` for datasource provider creation (Google OAuth path).
///
/// Loads the user's Google OAuth tokens from the DB, refreshes if expired, and
/// returns a populated `UserContext`. If Google OAuth is not configured
/// (`client_id` / `client_secret` are `None`) or the user has no tokens, the
/// `oauth_data` field is `None` — providers that support multiple auth modes
/// (e.g. BigQuery with service-account auth) will fall back automatically.
///
/// # Parameters
///
/// - `db` — database pool
/// - `user_id` — user whose OAuth tokens to resolve
/// - `encryption_key` — key used to decrypt stored OAuth data; only consulted
///   when `google_client_id` and `google_client_secret` are both `Some`.
///   Pass `None` in environments where the key is not configured — the function
///   returns an error only if the key is needed but absent.
/// - `google_client_id` / `google_client_secret` — OAuth app credentials; both
///   must be `Some` for token resolution to proceed
/// - `user_email` — passed through verbatim into `UserContext`
/// - `workspace_id` — passed through verbatim into `UserContext`
pub async fn build_datasource_user_context(
    db: &kyomi_core::DbPool,
    user_id: &str,
    encryption_key: Option<&[u8; 32]>,
    google_client_id: Option<&str>,
    google_client_secret: Option<&str>,
    user_email: String,
    workspace_id: String,
) -> kyomi_core::Result<Option<kyomi_datasource_server::UserContext>> {
    let oauth_data = if let (Some(client_id), Some(client_secret)) =
        (google_client_id, google_client_secret)
    {
        let key = encryption_key.ok_or_else(|| {
            kyomi_core::Error::Internal("Encryption key not configured".into())
        })?;
        match ensure_valid_google_token(db, user_id, key, client_id, client_secret).await {
            Ok(tokens) => {
                let data = OAuthData {
                    google_oauth_tokens: Some(tokens),
                    ..Default::default()
                };
                serde_json::to_value(data).ok()
            }
            Err(_) => None,
        }
    } else {
        None
    };

    Ok(Some(kyomi_datasource_server::UserContext {
        oauth_data,
        user_email,
        workspace_id,
    }))
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

// ---------------------------------------------------------------------------
// Service functions — business logic extracted for reuse by server_fns
// ---------------------------------------------------------------------------

/// Result of `google_oauth_status_service`.
///
/// Defined in `kyomi_types` as `GoogleOAuthStatus` (the canonical wire name)
/// because it also crosses into the WASM client as a server_fn response —
/// see `kyomi_types::datasource_contracts`. Re-exported under this
/// service-return name so existing call sites keep compiling unchanged.
pub use kyomi_types::GoogleOAuthStatus as GoogleOAuthStatusResult;

/// Get the current Google OAuth connection status for a user.
///
/// Reads the user's encrypted `oauth_data` from the database and returns
/// the connection status without making any external API calls.
///
/// Mirrors the logic from `apps/server/src/routes/auth_google_oauth.rs::google_oauth_status`.
pub async fn google_oauth_status_service(
    db: &kyomi_core::DbPool,
    user_id: &str,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<GoogleOAuthStatusResult> {
    let db_user = crate::user_service::get_user_by_id(db, user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))?;

    let oauth_data = parse_oauth_data(db_user.oauth_data.as_deref(), encryption_key)?;
    let tokens = oauth_data
        .as_ref()
        .and_then(|o| o.google_oauth_tokens.as_ref());

    match tokens {
        None => Ok(GoogleOAuthStatusResult {
            connected: false,
            google_email: None,
            has_bigquery_scopes: false,
            needs_bigquery_connect: true,
            token_expired: false,
            has_refresh_token: false,
        }),
        Some(t) => {
            let has_bq_scopes = has_bigquery_scopes(&t.scope);
            let has_refresh = t.refresh_token.is_some();
            let token_expired = t
                .expires_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|exp| exp.with_timezone(&chrono::Utc) < chrono::Utc::now())
                .unwrap_or(false);
            // Only report expired if no refresh token to auto-refresh
            let effectively_expired = token_expired && !has_refresh;
            let needs_connect = !has_bq_scopes || effectively_expired;

            Ok(GoogleOAuthStatusResult {
                connected: true,
                google_email: t.email.clone(),
                has_bigquery_scopes: has_bq_scopes,
                needs_bigquery_connect: needs_connect,
                token_expired: effectively_expired,
                has_refresh_token: has_refresh,
            })
        }
    }
}

/// Result of `google_oauth_disconnect_service`.
///
/// Defined in `kyomi_types` because it also crosses into the WASM client as
/// a server_fn response — see `kyomi_types::datasource_contracts`.
pub use kyomi_types::GoogleOAuthDisconnectResult;

/// Disconnect Google OAuth from a user account.
///
/// Clears the stored tokens from the user's `oauth_data` and removes the
/// `google_oauth` auth method entry.
///
/// Mirrors the logic from `apps/server/src/routes/auth_google_oauth.rs::google_oauth_disconnect`.
pub async fn google_oauth_disconnect_service(
    db: &kyomi_core::DbPool,
    user_id: &str,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<GoogleOAuthDisconnectResult> {
    let db_user = crate::user_service::get_user_by_id(db, user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))?;

    let existing_oauth = parse_oauth_data(db_user.oauth_data.as_deref(), encryption_key)?;

    let has_tokens = existing_oauth
        .as_ref()
        .and_then(|o| o.google_oauth_tokens.as_ref())
        .is_some();

    if !has_tokens {
        return Ok(GoogleOAuthDisconnectResult {
            success: true,
            already_disconnected: true,
            disconnected_email: None,
        });
    }

    let disconnected_email = existing_oauth
        .as_ref()
        .and_then(|o| o.google_oauth_tokens.as_ref())
        .and_then(|t| t.email.clone());

    // Clear OAuth tokens but keep picture
    let cleared_oauth = OAuthData {
        picture: existing_oauth.and_then(|o| o.picture),
        ..Default::default()
    };

    let encrypted = build_oauth_data(&cleared_oauth, encryption_key)?;
    crate::user_service::update_user_oauth_data(db, user_id, Some(&encrypted)).await?;
    crate::user_service::remove_auth_method(db, user_id, "google_oauth").await?;

    Ok(GoogleOAuthDisconnectResult {
        success: true,
        already_disconnected: false,
        disconnected_email,
    })
}

/// A single Google Cloud project.
///
/// Defined in `kyomi_types` because it also crosses into the WASM client as
/// a server_fn response — see `kyomi_types::datasource_contracts`.
pub use kyomi_types::GoogleProject;

/// Result of `google_oauth_projects_service`.
///
/// Defined in `kyomi_types` because it also crosses into the WASM client as
/// a server_fn response — see `kyomi_types::datasource_contracts`.
pub use kyomi_types::GoogleOAuthProjectsResult;

/// List active GCP projects visible to a resolved Google OAuth access
/// token, via the Cloud Resource Manager API.
///
/// Split out of `google_oauth_projects_service` (KYO-444) so a caller that
/// has already resolved an access token through some *other* path can reuse
/// the Resource Manager call instead of re-implementing it. The BigQuery
/// catalog indexer is exactly that caller: it resolves its own access token
/// per `auth_mode` (`kyomi_oauth` / `enterprise_oauth` / `service_account`)
/// and only one of those three modes goes through
/// `ensure_valid_google_token` — the token resolution
/// `google_oauth_projects_service` performs below is `kyomi_oauth`-specific
/// and would be the wrong call for the other two.
///
/// A `resourcemanager.projects.list` permission denial (common for service
/// accounts scoped only to e.g. "BigQuery Job User") surfaces as an `Err`
/// here — callers decide how to degrade (see
/// `kyomi_agent::catalog::indexers::bigquery`, which turns it into a
/// recorded `"failed"` status rather than a silent skip).
pub async fn list_active_google_projects(
    access_token: &str,
) -> kyomi_core::Result<Vec<GoogleProject>> {
    let client = crate::http_client()?;
    let resp = client
        .get(GOOGLE_PROJECTS_URI)
        .bearer_auth(access_token)
        .query(&[("filter", "lifecycleState:ACTIVE")])
        .send()
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("Google projects request failed: {e}"))
        })?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(kyomi_core::Error::Unauthorized(
            "Google OAuth token expired or revoked".into(),
        ));
    }

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "Google projects request failed ({status}): {body}"
        )));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to parse projects response: {e}"))
    })?;

    let mut projects: Vec<GoogleProject> = body["projects"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let project_id = p["projectId"].as_str().unwrap_or("").to_string();
            let name = p["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(&project_id)
                .to_string();
            GoogleProject { project_id, name }
        })
        .collect();

    projects.sort_by_key(|a| a.name.to_lowercase());

    Ok(projects)
}

/// List Google Cloud projects accessible to the authenticated user.
///
/// Resolves the user's Google OAuth token (refreshing if needed), then
/// calls the Google Cloud Resource Manager API to list active projects.
///
/// Mirrors the logic from `apps/server/src/routes/auth_google_oauth.rs::google_oauth_projects`.
pub async fn google_oauth_projects_service(
    db: &kyomi_core::DbPool,
    user_id: &str,
    encryption_key: &[u8; 32],
    client_id: &str,
    client_secret: &str,
) -> kyomi_core::Result<GoogleOAuthProjectsResult> {
    let tokens = ensure_valid_google_token(db, user_id, encryption_key, client_id, client_secret)
        .await?;

    let projects = list_active_google_projects(&tokens.access_token).await?;

    Ok(GoogleOAuthProjectsResult {
        projects,
        message: None,
    })
}
