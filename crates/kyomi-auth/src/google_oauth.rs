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
pub const GOOGLE_REVOKE_URI: &str = "https://oauth2.googleapis.com/revoke";

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
///
/// `include_granted_scopes` must be set per call site, not assumed `true`:
/// Google's `include_granted_scopes=true` hands back **every** scope ever
/// granted to this OAuth client for the user, not just the ones requested in
/// this authorization. That's exactly what the `bigquery` connect flow wants
/// (a user can connect BigQuery, then later re-consent to add another
/// datasource without losing the first grant) — but it's the opposite of
/// what the plain `login` flow wants: after a user disconnects BigQuery
/// (KYO-700, `google_oauth_disconnect_service`, which revokes the grant at
/// Google), the very next sign-in must not silently hand the BigQuery scopes
/// back by requesting the union of everything the client was ever granted.
pub fn build_authorization_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    scopes: &[&str],
    force_consent: bool,
    offline_access: bool,
    include_granted_scopes: bool,
) -> String {
    let scope = scopes.join(" ");

    let mut params = url::form_urlencoded::Serializer::new(String::new());
    params
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", &scope)
        .append_pair("state", state);

    if include_granted_scopes {
        params.append_pair("include_granted_scopes", "true");
    }

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
// Token revocation (KYO-700)
// ---------------------------------------------------------------------------

/// Outcome of asking Google to revoke a token via `GOOGLE_REVOKE_URI`.
///
/// Both variants mean the grant is gone at Google and it is safe for the
/// caller to clear its own local copy of the tokens. They're kept distinct
/// (rather than collapsed to `()`) so callers can log which case happened,
/// and so tests can assert on the exact classification instead of just
/// "did not error".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoogleRevokeOutcome {
    /// Google confirmed the token was live and has now revoked it (and its
    /// paired token — revoking either an access or refresh token revokes
    /// the whole grant).
    Revoked,
    /// Google returned HTTP 400, meaning the token was already
    /// invalid/expired. The grant is already gone, so this counts as
    /// success rather than a failure to disconnect.
    AlreadyInvalid,
}

/// Choose which stored token to send to `/revoke`.
///
/// Prefers the refresh token: it's the longer-lived credential, and the
/// BigQuery connect flow always requests `access_type=offline`, so a
/// refresh token is expected to be present. Falls back to the access token
/// for any tokens obtained before that was true. Google's revoke endpoint
/// accepts either kind and revoking either one revokes the whole grant, so
/// this preference is about picking the more durable credential to send —
/// not about one kind being able to revoke more than the other.
fn select_revocation_token(tokens: &GoogleOAuthTokens) -> &str {
    tokens
        .refresh_token
        .as_deref()
        .unwrap_or(&tokens.access_token)
}

/// Map a `/revoke` response status to an outcome, or an error for anything
/// that isn't a definite "the grant is gone" answer.
///
/// Split out as a pure function (no I/O) so the success/failure decision can
/// be unit tested without a live call to Google — see `mod tests` below.
fn classify_revoke_status(
    status: reqwest::StatusCode,
) -> kyomi_core::Result<GoogleRevokeOutcome> {
    if status.is_success() {
        Ok(GoogleRevokeOutcome::Revoked)
    } else if status == reqwest::StatusCode::BAD_REQUEST {
        // Google's documented shape for "this token is already
        // invalid/expired" — the grant is already gone, so this is the
        // desired end state, not a failure.
        Ok(GoogleRevokeOutcome::AlreadyInvalid)
    } else {
        Err(kyomi_core::Error::Internal(format!(
            "Google did not confirm the account was disconnected (revocation failed with \
             status {status}). Your Google account is still connected — please try again."
        )))
    }
}

/// Revoke a Google OAuth grant at the given `/revoke` endpoint.
///
/// Internal seam so `mod tests` can point this at a local
/// `wiremock::MockServer` and exercise the real HTTP path (status handling,
/// transport failures) without ever reaching Google. Production code should
/// call [`revoke_google_token`], not this directly.
async fn revoke_google_token_at(
    revoke_uri: &str,
    tokens: &GoogleOAuthTokens,
) -> kyomi_core::Result<GoogleRevokeOutcome> {
    let token = select_revocation_token(tokens);
    let client = crate::http_client()?;

    let resp = client
        .post(revoke_uri)
        .form(&[("token", token)])
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "Google did not confirm the account was disconnected (revocation request \
                 failed: {e}). Your Google account is still connected — please try again."
            ))
        })?;

    classify_revoke_status(resp.status())
}

/// Revoke a Google OAuth grant.
///
/// Calls `POST {GOOGLE_REVOKE_URI}` with the token chosen by
/// [`select_revocation_token`]. A 400 response means the token was already
/// invalid — that's treated as success, since the grant is already gone.
///
/// Any other failure (network error, timeout, non-400 status) is returned as
/// an `Err`, and **the caller must not clear local state in that case**. If
/// local state were cleared while revocation is unconfirmed, Kyomi would
/// discard the only copy of the token it holds — the grant would stay live
/// at Google, and Kyomi would have no way to ever revoke it again. Failing
/// the disconnect instead keeps the token so the user can retry. See
/// `google_oauth_disconnect_service`, the only caller.
pub async fn revoke_google_token(
    tokens: &GoogleOAuthTokens,
) -> kyomi_core::Result<GoogleRevokeOutcome> {
    revoke_google_token_at(GOOGLE_REVOKE_URI, tokens).await
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
/// Revokes the grant with Google *before* touching local state, then clears the
/// stored tokens from the user's `oauth_data` and removes the `google_oauth`
/// auth method entry. If revocation is not confirmed, the disconnect fails and
/// local state is left intact — see `google_oauth_disconnect_service_at` below
/// for why that direction is the safe one.
///
/// Both entrypoints delegate here: the Leptos `disconnect_google_oauth` server
/// fn, and the REST route
/// `apps/server/src/routes/auth_google_oauth.rs::google_oauth_disconnect`.
pub async fn google_oauth_disconnect_service(
    db: &kyomi_core::DbPool,
    user_id: &str,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<GoogleOAuthDisconnectResult> {
    google_oauth_disconnect_service_at(GOOGLE_REVOKE_URI, db, user_id, encryption_key).await
}

/// Test seam for [`google_oauth_disconnect_service`]: identical behavior
/// with an injectable revoke endpoint, so `mod tests` below can point it at
/// a local `wiremock::MockServer` and prove the fail-closed revocation
/// policy end-to-end (DB state included) without ever reaching Google.
async fn google_oauth_disconnect_service_at(
    revoke_uri: &str,
    db: &kyomi_core::DbPool,
    user_id: &str,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<GoogleOAuthDisconnectResult> {
    let db_user = crate::user_service::get_user_by_id(db, user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))?;

    let existing_oauth = parse_oauth_data(db_user.oauth_data.as_deref(), encryption_key)?;

    let tokens = existing_oauth
        .as_ref()
        .and_then(|o| o.google_oauth_tokens.as_ref());

    let Some(tokens) = tokens else {
        return Ok(GoogleOAuthDisconnectResult {
            success: true,
            already_disconnected: true,
            disconnected_email: None,
        });
    };

    // Revoke the grant at Google *before* clearing our only copy of the
    // token. A failure here (network error, timeout, non-400 status) must
    // abort the disconnect and leave local state untouched — see
    // `revoke_google_token`'s doc comment for why clearing on an unconfirmed
    // revocation would permanently strand the grant at Google.
    match revoke_google_token_at(revoke_uri, tokens).await {
        Ok(outcome) => {
            tracing::info!(user_id = %user_id, outcome = ?outcome, "Google OAuth grant revoked");
        }
        Err(e) => {
            tracing::warn!(
                user_id = %user_id,
                error = %e,
                "Google OAuth revocation failed; disconnect aborted, local tokens preserved"
            );
            return Err(e);
        }
    }

    let disconnected_email = tokens.email.clone();

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{seed_user, sqlite_pool, test_key, test_pool};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn tokens_with(refresh: Option<&str>, access: &str) -> GoogleOAuthTokens {
        GoogleOAuthTokens {
            access_token: access.to_string(),
            refresh_token: refresh.map(str::to_string),
            token_type: "Bearer".to_string(),
            scope: "https://www.googleapis.com/auth/bigquery.readonly".to_string(),
            expires_in: Some(3600),
            expires_at: None,
            email: Some("user@example.com".to_string()),
            name: None,
        }
    }

    // ── select_revocation_token ─────────────────────────────────────────

    #[test]
    fn select_revocation_token_prefers_refresh_token() {
        let tokens = tokens_with(Some("refresh-abc"), "access-xyz");
        assert_eq!(select_revocation_token(&tokens), "refresh-abc");
    }

    #[test]
    fn select_revocation_token_falls_back_to_access_token() {
        let tokens = tokens_with(None, "access-xyz");
        assert_eq!(select_revocation_token(&tokens), "access-xyz");
    }

    // ── classify_revoke_status ───────────────────────────────────────────

    #[test]
    fn classify_2xx_is_revoked() {
        let outcome = classify_revoke_status(reqwest::StatusCode::OK).unwrap();
        assert_eq!(outcome, GoogleRevokeOutcome::Revoked);
    }

    #[test]
    fn classify_400_is_already_invalid_not_an_error() {
        let outcome = classify_revoke_status(reqwest::StatusCode::BAD_REQUEST).unwrap();
        assert_eq!(outcome, GoogleRevokeOutcome::AlreadyInvalid);
    }

    #[test]
    fn classify_500_is_err() {
        let result = classify_revoke_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            result.is_err(),
            "a 500 must not be treated as a successful revocation"
        );
    }

    #[test]
    fn classify_401_is_err_not_already_invalid() {
        // A 401 means the caller isn't authorized to revoke — it does NOT
        // mean the token itself is already invalid. Only 400 gets the
        // "already gone" treatment; every other non-2xx status is a real
        // failure that must block clearing local state.
        let result = classify_revoke_status(reqwest::StatusCode::UNAUTHORIZED);
        assert!(result.is_err());
    }

    // ── revoke_google_token_at — real HTTP boundary via wiremock ─────────

    #[tokio::test]
    async fn revoke_at_200_sends_the_preferred_token_and_returns_revoked() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        let tokens = tokens_with(Some("refresh-abc"), "access-xyz");
        let revoke_uri = format!("{}/revoke", mock_server.uri());
        let outcome = revoke_google_token_at(&revoke_uri, &tokens).await.unwrap();

        assert_eq!(outcome, GoogleRevokeOutcome::Revoked);

        let requests = mock_server
            .received_requests()
            .await
            .expect("request recording enabled");
        assert_eq!(requests.len(), 1);
        let body = String::from_utf8(requests[0].body.clone()).unwrap();
        assert!(
            body.contains("refresh-abc"),
            "must send the refresh token, not the access token, when both are present: {body}"
        );
        assert!(!body.contains("access-xyz"));
    }

    #[tokio::test]
    async fn revoke_at_400_is_ok_already_invalid() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&mock_server)
            .await;

        let tokens = tokens_with(None, "access-xyz");
        let revoke_uri = format!("{}/revoke", mock_server.uri());
        let outcome = revoke_google_token_at(&revoke_uri, &tokens).await.unwrap();

        assert_eq!(outcome, GoogleRevokeOutcome::AlreadyInvalid);
    }

    #[tokio::test]
    async fn revoke_at_500_is_err() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let tokens = tokens_with(None, "access-xyz");
        let revoke_uri = format!("{}/revoke", mock_server.uri());
        let result = revoke_google_token_at(&revoke_uri, &tokens).await;

        assert!(
            result.is_err(),
            "a 500 must fail closed, not be treated as revoked"
        );
    }

    #[tokio::test]
    async fn revoke_at_transport_failure_is_err() {
        // Nothing is listening on this address — a genuine connection
        // failure, not a mocked status code. Proves the fail-closed policy
        // covers transport errors, not just bad HTTP statuses.
        let tokens = tokens_with(None, "access-xyz");
        let result = revoke_google_token_at("http://127.0.0.1:1/revoke", &tokens).await;

        assert!(
            result.is_err(),
            "a transport failure must fail closed exactly like a bad HTTP status"
        );
    }

    // ── google_oauth_disconnect_service_at — DB state under each outcome ─

    #[tokio::test]
    async fn disconnect_clears_local_state_when_revocation_succeeds() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "u1", "u1@example.com").await;
        let key = test_key();

        let data = OAuthData {
            google_oauth_tokens: Some(tokens_with(Some("refresh-abc"), "access-xyz")),
            ..Default::default()
        };
        let encrypted = build_oauth_data(&data, &key).unwrap();
        crate::user_service::update_user_oauth_data(&db, "u1", Some(&encrypted))
            .await
            .unwrap();
        crate::user_service::upsert_auth_method(&db, "u1", "google_oauth", &serde_json::json!({}))
            .await
            .unwrap();

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;
        let revoke_uri = format!("{}/revoke", mock_server.uri());

        let result = google_oauth_disconnect_service_at(&revoke_uri, &db, "u1", &key)
            .await
            .unwrap();

        assert!(result.success);
        assert!(!result.already_disconnected);

        let db_user = crate::user_service::get_user_by_id(&db, "u1")
            .await
            .unwrap()
            .unwrap();
        let stored = parse_oauth_data(db_user.oauth_data.as_deref(), &key).unwrap();
        assert!(
            stored.and_then(|o| o.google_oauth_tokens).is_none(),
            "tokens must be cleared once revocation is confirmed"
        );

        let auth_method = crate::user_service::get_auth_method(&db, "u1", "google_oauth")
            .await
            .unwrap();
        assert!(
            auth_method.is_none(),
            "the google_oauth auth method must be deactivated"
        );
    }

    #[tokio::test]
    async fn disconnect_preserves_local_state_when_revocation_fails() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "u1", "u1@example.com").await;
        let key = test_key();

        let data = OAuthData {
            google_oauth_tokens: Some(tokens_with(Some("refresh-abc"), "access-xyz")),
            ..Default::default()
        };
        let encrypted = build_oauth_data(&data, &key).unwrap();
        crate::user_service::update_user_oauth_data(&db, "u1", Some(&encrypted))
            .await
            .unwrap();
        crate::user_service::upsert_auth_method(&db, "u1", "google_oauth", &serde_json::json!({}))
            .await
            .unwrap();

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;
        let revoke_uri = format!("{}/revoke", mock_server.uri());

        let result = google_oauth_disconnect_service_at(&revoke_uri, &db, "u1", &key).await;

        assert!(
            result.is_err(),
            "disconnect must fail when Google does not confirm revocation"
        );

        let db_user = crate::user_service::get_user_by_id(&db, "u1")
            .await
            .unwrap()
            .unwrap();
        let stored = parse_oauth_data(db_user.oauth_data.as_deref(), &key).unwrap();
        assert!(
            stored.and_then(|o| o.google_oauth_tokens).is_some(),
            "tokens must be preserved when revocation could not be confirmed — otherwise \
             Kyomi loses its only copy of the token and can never revoke the grant"
        );

        let auth_method = crate::user_service::get_auth_method(&db, "u1", "google_oauth")
            .await
            .unwrap();
        assert!(
            auth_method.is_some(),
            "the google_oauth auth method must remain active when disconnect failed"
        );
    }

    // ── build_authorization_url — include_granted_scopes is per-flow ────

    #[test]
    fn authorization_url_omits_include_granted_scopes_when_false() {
        let url = build_authorization_url(
            "client-id",
            "https://example.com/cb",
            "state",
            LOGIN_SCOPES,
            false,
            false,
            false,
        );
        assert!(!url.contains("include_granted_scopes"));
    }

    #[test]
    fn authorization_url_includes_granted_scopes_when_true() {
        let url = build_authorization_url(
            "client-id",
            "https://example.com/cb",
            "state",
            BIGQUERY_SCOPES,
            true,
            true,
            true,
        );
        assert!(url.contains("include_granted_scopes=true"));
    }
}
