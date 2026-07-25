// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-datasource OAuth service — authorization URL construction, code exchange, user info.
//!
//! Supports Snowflake, Databricks, BigQuery Enterprise (Google), and Microsoft Enterprise.
//! Each provider extracts its config from the datasource's `connection_config` JSON.
//!
//! Wire-compatible with Python's `auth/oauth_providers/` implementations.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

// ---------------------------------------------------------------------------
// Provider enum
// ---------------------------------------------------------------------------

/// Supported per-datasource OAuth providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthProvider {
    Snowflake,
    Databricks,
    BigqueryEnterprise,
    MicrosoftEnterprise,
}

impl OAuthProvider {
    /// Parse a provider name from a URL path segment.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "snowflake" => Some(Self::Snowflake),
            "databricks" => Some(Self::Databricks),
            "bigquery-enterprise" => Some(Self::BigqueryEnterprise),
            "microsoft-enterprise" => Some(Self::MicrosoftEnterprise),
            _ => None,
        }
    }

    /// Canonical string name (used in Redis keys, logs, responses).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Snowflake => "snowflake",
            Self::Databricks => "databricks",
            Self::BigqueryEnterprise => "bigquery-enterprise",
            Self::MicrosoftEnterprise => "microsoft-enterprise",
        }
    }

    /// All registered providers.
    pub fn all() -> &'static [OAuthProvider] {
        &[
            Self::Snowflake,
            Self::Databricks,
            Self::BigqueryEnterprise,
            Self::MicrosoftEnterprise,
        ]
    }

    /// Whether this provider uses PKCE (S256).
    pub fn uses_pkce(&self) -> bool {
        matches!(
            self,
            Self::Snowflake | Self::Databricks | Self::MicrosoftEnterprise
        )
    }
}

// ---------------------------------------------------------------------------
// Provider config extracted from connection_config
// ---------------------------------------------------------------------------

/// OAuth credentials and provider-specific identifiers extracted from a
/// datasource's `connection_config` JSON.
#[derive(Debug)]
pub struct ProviderConfig {
    pub provider: OAuthProvider,
    pub client_id: String,
    pub client_secret: String,
    /// Provider-specific host/account identifier:
    /// - Snowflake: `account` (e.g., "xy12345.us-east-1")
    /// - Databricks: `server_hostname` (e.g., "dbc-abc123.cloud.databricks.com")
    /// - BigQuery Enterprise: unused (Google endpoints are static)
    /// - Microsoft Enterprise: `tenant_id` (e.g., "common" or a GUID)
    pub account_or_host: String,
}

impl ProviderConfig {
    /// Extract provider config from a datasource's `connection_config` JSON.
    ///
    /// For Microsoft Enterprise, falls back to `MICROSOFT_OAUTH_CLIENT_ID` and
    /// `MICROSOFT_OAUTH_CLIENT_SECRET` env vars when per-datasource credentials
    /// are not configured. This matches the Python implementation where Kyomi
    /// has one Azure AD multi-tenant app registration that can authenticate
    /// users from any tenant.
    pub fn from_connection_config(
        provider: OAuthProvider,
        config: &serde_json::Value,
    ) -> kyomi_core::Result<Self> {
        // Read per-datasource client credentials from connection_config
        let config_client_id = config
            .get("oauth_client_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        let config_client_secret = config
            .get("oauth_client_secret")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        // For Microsoft Enterprise, fall back to env vars if per-datasource
        // credentials are not set (Kyomi's multi-tenant Azure AD app)
        let (client_id, client_secret) = if provider == OAuthProvider::MicrosoftEnterprise
            && config_client_id.is_none()
        {
            let env_client_id = std::env::var("MICROSOFT_OAUTH_CLIENT_ID").ok();
            let env_client_secret = std::env::var("MICROSOFT_OAUTH_CLIENT_SECRET").ok();

            let cid = env_client_id.filter(|s| !s.is_empty()).ok_or_else(|| {
                kyomi_core::Error::BadRequest(
                    "Microsoft Enterprise OAuth requires oauth_client_id in connection config \
                     or MICROSOFT_OAUTH_CLIENT_ID env var"
                        .into(),
                )
            })?;

            let csec = env_client_secret
                .filter(|s| !s.is_empty())
                .unwrap_or_default();

            (cid, csec)
        } else {
            let cid = config_client_id
                .ok_or_else(|| {
                    kyomi_core::Error::BadRequest(format!(
                        "{} OAuth requires oauth_client_id in connection config",
                        provider.as_str()
                    ))
                })?
                .to_string();

            let csec = config_client_secret.unwrap_or("").to_string();

            (cid, csec)
        };

        let account_or_host = match provider {
            OAuthProvider::Snowflake => config
                .get("account")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    kyomi_core::Error::BadRequest(
                        "Snowflake OAuth requires account in connection config".into(),
                    )
                })?
                .to_string(),

            OAuthProvider::Databricks => config
                .get("server_hostname")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    kyomi_core::Error::BadRequest(
                        "Databricks OAuth requires server_hostname in connection config".into(),
                    )
                })?
                .to_string(),

            OAuthProvider::BigqueryEnterprise => {
                // Google endpoints are static; no host needed.
                String::new()
            }

            OAuthProvider::MicrosoftEnterprise => config
                .get("tenant_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("common")
                .to_string(),
        };

        Ok(Self {
            provider,
            client_id,
            client_secret,
            account_or_host,
        })
    }
}

// ---------------------------------------------------------------------------
// PKCE
// ---------------------------------------------------------------------------

/// PKCE code verifier + challenge pair (S256).
pub struct PkceChallenge {
    pub code_verifier: String,
    pub code_challenge: String,
}

/// Generate a PKCE code verifier (43-char URL-safe random) and its S256 challenge.
pub fn generate_pkce() -> PkceChallenge {
    use rand::Rng;

    // 32 random bytes → 43-char base64url (no padding)
    let random_bytes: [u8; 32] = rand::rng().random();
    let code_verifier = URL_SAFE_NO_PAD.encode(random_bytes);

    // S256: SHA-256 hash of the verifier, then base64url encode
    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let hash = hasher.finalize();
    let code_challenge = URL_SAFE_NO_PAD.encode(hash);

    PkceChallenge {
        code_verifier,
        code_challenge,
    }
}

// ---------------------------------------------------------------------------
// Authorization URL
// ---------------------------------------------------------------------------

/// Result of building an authorization URL.
pub struct AuthorizationResult {
    pub url: String,
    /// Only set for PKCE providers (Snowflake, Databricks).
    pub code_verifier: Option<String>,
}

/// Build the OAuth authorization URL for the given provider.
///
/// `redirect_uri` is the full callback URL (e.g., `https://dev.kyomi.ai/auth/oauth/snowflake/callback`).
pub fn build_authorization_url(
    config: &ProviderConfig,
    redirect_uri: &str,
    state: &str,
) -> AuthorizationResult {
    match config.provider {
        OAuthProvider::Snowflake => build_snowflake_auth_url(config, redirect_uri, state),
        OAuthProvider::Databricks => build_databricks_auth_url(config, redirect_uri, state),
        OAuthProvider::BigqueryEnterprise => {
            build_bigquery_enterprise_auth_url(config, redirect_uri, state)
        }
        OAuthProvider::MicrosoftEnterprise => {
            build_microsoft_enterprise_auth_url(config, redirect_uri, state)
        }
    }
}

fn build_snowflake_auth_url(
    config: &ProviderConfig,
    redirect_uri: &str,
    state: &str,
) -> AuthorizationResult {
    let pkce = generate_pkce();
    let account = &config.account_or_host;

    let mut url = Url::parse(&format!(
        "https://{account}.snowflakecomputing.com/oauth/authorize"
    ))
    .expect("valid base URL");

    // Snowflake scopes are controlled by the security integration — send empty
    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("state", state)
        .append_pair("code_challenge", &pkce.code_challenge)
        .append_pair("code_challenge_method", "S256");

    AuthorizationResult {
        url: url.to_string(),
        code_verifier: Some(pkce.code_verifier),
    }
}

fn build_databricks_auth_url(
    config: &ProviderConfig,
    redirect_uri: &str,
    state: &str,
) -> AuthorizationResult {
    let pkce = generate_pkce();
    let host = &config.account_or_host;

    let mut url =
        Url::parse(&format!("https://{host}/oidc/v1/authorize")).expect("valid base URL");

    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("state", state)
        .append_pair("scope", "all-apis sql offline_access")
        .append_pair("code_challenge", &pkce.code_challenge)
        .append_pair("code_challenge_method", "S256");

    AuthorizationResult {
        url: url.to_string(),
        code_verifier: Some(pkce.code_verifier),
    }
}

fn build_bigquery_enterprise_auth_url(
    config: &ProviderConfig,
    redirect_uri: &str,
    state: &str,
) -> AuthorizationResult {
    // Reuse the same Google authorization endpoint as the global Google OAuth,
    // but with per-datasource client credentials and BigQuery scopes.
    let scopes = [
        "https://www.googleapis.com/auth/bigquery.readonly",
        "https://www.googleapis.com/auth/cloudplatformprojects.readonly",
        "https://www.googleapis.com/auth/userinfo.email",
        "https://www.googleapis.com/auth/userinfo.profile",
    ]
    .join(" ");

    let mut url =
        Url::parse("https://accounts.google.com/o/oauth2/auth").expect("valid base URL");

    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", &scopes)
        .append_pair("state", state)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent");

    AuthorizationResult {
        url: url.to_string(),
        code_verifier: None,
    }
}

fn build_microsoft_enterprise_auth_url(
    config: &ProviderConfig,
    redirect_uri: &str,
    state: &str,
) -> AuthorizationResult {
    let pkce = generate_pkce();
    let tenant = &config.account_or_host;

    let mut url = Url::parse(&format!(
        "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"
    ))
    .expect("valid base URL");

    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("response_mode", "query")
        .append_pair("scope", "https://database.windows.net/.default offline_access")
        .append_pair("state", state)
        .append_pair("code_challenge", &pkce.code_challenge)
        .append_pair("code_challenge_method", "S256");

    AuthorizationResult {
        url: url.to_string(),
        code_verifier: Some(pkce.code_verifier),
    }
}

// ---------------------------------------------------------------------------
// Token exchange
// ---------------------------------------------------------------------------

/// Standard OAuth token response (normalized across providers).
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
    pub scope: Option<String>,
    pub token_type: Option<String>,
}

/// Exchange an authorization code for tokens.
pub async fn exchange_code_for_tokens(
    config: &ProviderConfig,
    code: &str,
    redirect_uri: &str,
    code_verifier: Option<&str>,
) -> kyomi_core::Result<TokenResponse> {
    let client = crate::http_client()?;

    match config.provider {
        OAuthProvider::Snowflake => {
            let url = format!(
                "https://{}.snowflakecomputing.com/oauth/token-request",
                config.account_or_host
            );
            let mut params = vec![
                ("client_id", config.client_id.as_str()),
                ("client_secret", config.client_secret.as_str()),
                ("code", code),
                ("grant_type", "authorization_code"),
                ("redirect_uri", redirect_uri),
            ];
            if let Some(verifier) = code_verifier {
                params.push(("code_verifier", verifier));
            }
            post_token_exchange(&client, &url, &params, "Snowflake").await
        }

        OAuthProvider::Databricks => {
            let url = format!(
                "https://{}/oidc/v1/token",
                config.account_or_host
            );
            let mut params = vec![
                ("client_id", config.client_id.as_str()),
                ("client_secret", config.client_secret.as_str()),
                ("code", code),
                ("grant_type", "authorization_code"),
                ("redirect_uri", redirect_uri),
            ];
            if let Some(verifier) = code_verifier {
                params.push(("code_verifier", verifier));
            }
            post_token_exchange(&client, &url, &params, "Databricks").await
        }

        OAuthProvider::BigqueryEnterprise => {
            let params = vec![
                ("client_id", config.client_id.as_str()),
                ("client_secret", config.client_secret.as_str()),
                ("code", code),
                ("grant_type", "authorization_code"),
                ("redirect_uri", redirect_uri),
            ];
            post_token_exchange(
                &client,
                "https://oauth2.googleapis.com/token",
                &params,
                "BigQuery Enterprise",
            )
            .await
        }

        OAuthProvider::MicrosoftEnterprise => {
            let url = format!(
                "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
                config.account_or_host
            );
            let mut params = vec![
                ("client_id", config.client_id.as_str()),
                ("client_secret", config.client_secret.as_str()),
                ("code", code),
                ("grant_type", "authorization_code"),
                ("redirect_uri", redirect_uri),
            ];
            if let Some(verifier) = code_verifier {
                params.push(("code_verifier", verifier));
            }
            post_token_exchange(&client, &url, &params, "Microsoft Enterprise").await
        }
    }
}

/// POST a form-encoded token exchange and parse the response.
async fn post_token_exchange(
    client: &reqwest::Client,
    url: &str,
    params: &[(&str, &str)],
    provider_name: &str,
) -> kyomi_core::Result<TokenResponse> {
    let resp = client
        .post(url)
        .form(params)
        .send()
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("{provider_name} token exchange request failed: {e}"))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::BadRequest(format!(
            "{provider_name} token exchange failed ({status}): {body}"
        )));
    }

    resp.json::<TokenResponse>().await.map_err(|e| {
        kyomi_core::Error::Internal(format!(
            "Failed to parse {provider_name} token response: {e}"
        ))
    })
}

// ---------------------------------------------------------------------------
// User info
// ---------------------------------------------------------------------------

/// Normalized user info from a provider.
#[derive(Debug, Default)]
pub struct ProviderUserInfo {
    pub username: Option<String>,
    pub email: Option<String>,
}

/// Fetch user info from the provider using an access token.
///
/// Some providers (Snowflake) don't have a userinfo endpoint — returns a
/// placeholder in that case.
pub async fn get_user_info(
    provider: OAuthProvider,
    access_token: &str,
    account_or_host: &str,
) -> kyomi_core::Result<ProviderUserInfo> {
    match provider {
        OAuthProvider::Snowflake => {
            // Snowflake has no standard userinfo endpoint
            Ok(ProviderUserInfo {
                username: Some("snowflake_user".to_string()),
                email: None,
            })
        }

        OAuthProvider::Databricks => {
            get_databricks_user_info(access_token, account_or_host).await
        }

        OAuthProvider::BigqueryEnterprise => {
            get_google_user_info(access_token).await
        }

        OAuthProvider::MicrosoftEnterprise => {
            get_microsoft_user_info(access_token).await
        }
    }
}

async fn get_databricks_user_info(
    access_token: &str,
    server_hostname: &str,
) -> kyomi_core::Result<ProviderUserInfo> {
    let url = format!("https://{server_hostname}/api/2.0/preview/scim/v2/Me");
    let client = crate::http_client()?;

    let resp = client
        .get(&url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("Databricks user info request failed: {e}"))
        })?;

    if !resp.status().is_success() {
        tracing::warn!(
            status = %resp.status(),
            "Databricks user info request failed, returning placeholder"
        );
        return Ok(ProviderUserInfo {
            username: Some("databricks_user".to_string()),
            email: None,
        });
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to parse Databricks user info: {e}"))
    })?;

    // Extract email from SCIM response: emails[].value where primary=true
    let email = body
        .get("emails")
        .and_then(|e| e.as_array())
        .and_then(|emails| {
            emails.iter().find_map(|entry| {
                if entry.get("primary").and_then(|p| p.as_bool()).unwrap_or(false) {
                    entry.get("value").and_then(|v| v.as_str()).map(|s| s.to_string())
                } else {
                    None
                }
            })
        })
        .or_else(|| {
            // Fallback: first email entry
            body.get("emails")
                .and_then(|e| e.as_array())
                .and_then(|emails| emails.first())
                .and_then(|entry| entry.get("value").and_then(|v| v.as_str()))
                .map(|s| s.to_string())
        });

    let username = body
        .get("userName")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("displayName").and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    Ok(ProviderUserInfo { username, email })
}

async fn get_google_user_info(access_token: &str) -> kyomi_core::Result<ProviderUserInfo> {
    let client = crate::http_client()?;

    let resp = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("Google user info request failed: {e}"))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::BadRequest(format!(
            "Google user info request failed ({status}): {body}"
        )));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to parse Google user info: {e}"))
    })?;

    Ok(ProviderUserInfo {
        username: body.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()),
        email: body.get("email").and_then(|v| v.as_str()).map(|s| s.to_string()),
    })
}

async fn get_microsoft_user_info(access_token: &str) -> kyomi_core::Result<ProviderUserInfo> {
    let client = crate::http_client()?;

    let resp = client
        .get("https://graph.microsoft.com/v1.0/me")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!("Microsoft user info request failed: {e}"))
        })?;

    if !resp.status().is_success() {
        // Microsoft database-only tokens (Azure SQL) lack Graph API scopes.
        // This is expected — return placeholder gracefully.
        tracing::debug!(
            status = %resp.status(),
            "Microsoft Graph user info failed (expected for database-only tokens), returning placeholder"
        );
        return Ok(ProviderUserInfo {
            username: Some("microsoft_user".to_string()),
            email: None,
        });
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to parse Microsoft user info: {e}"))
    })?;

    let email = body
        .get("mail")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("userPrincipalName").and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    let username = body
        .get("displayName")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(ProviderUserInfo { username, email })
}

// ---------------------------------------------------------------------------
// Service functions — business logic extracted for reuse by server_fns
// ---------------------------------------------------------------------------

/// Resolve a datasource slug to its primary-key `id`.
///
/// Returns `NotFound` if no active datasource with the given slug exists in
/// the workspace.
async fn resolve_datasource_id(
    db: &kyomi_core::DbPool,
    slug: &str,
    workspace_id: &str,
) -> kyomi_core::Result<String> {
    #[derive(sqlx::FromRow)]
    struct IdRow {
        id: String,
    }

    kyomi_core::db_fetch_optional!(
        db,
        IdRow,
        "SELECT id FROM datasource_configs \
         WHERE slug = $1 AND workspace_id = $2 AND active = true",
        slug,
        workspace_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("DB error: {e}")))?
    .map(|r| r.id)
    .ok_or_else(|| kyomi_core::Error::NotFound(format!("Datasource not found: {slug}")))
}

/// Result of `datasource_oauth_status_service`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DatasourceOAuthStatusResult {
    pub connected: bool,
    pub provider_email: Option<String>,
    pub token_expired: bool,
    pub needs_reconnect: bool,
    pub connect_url: String,
    pub disconnect_url: String,
}

/// Get the OAuth connection status for a specific datasource and user.
///
/// Looks up the user's credentials for the given datasource, decrypts them,
/// and returns the connection status without making any external API calls.
///
/// Mirrors the logic from `apps/server/src/routes/auth_datasource_oauth.rs::status`.
pub async fn datasource_oauth_status_service(
    db: &kyomi_core::DbPool,
    user_id: &str,
    workspace_id: &str,
    provider: OAuthProvider,
    datasource_slug: &str,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<DatasourceOAuthStatusResult> {
    use kyomi_core::models::datasource::UserDatasourceCredential;

    let provider_str = provider.as_str();
    let connect_url = format!("/api/v1/auth/oauth/{provider_str}/connect");
    let disconnect_url = format!("/api/v1/auth/oauth/{provider_str}/disconnect");

    let ds_id = resolve_datasource_id(db, datasource_slug, workspace_id).await?;

    // Look up user credentials
    let cred = kyomi_core::db_fetch_optional!(
        db,
        UserDatasourceCredential,
        "SELECT id, user_id, datasource_config_id, workspace_id, credentials, \
         enabled, created_at, updated_at \
         FROM user_datasource_credentials \
         WHERE user_id = $1 AND datasource_config_id = $2",
        user_id,
        &ds_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("DB error: {e}")))?;

    let Some(cred) = cred else {
        return Ok(DatasourceOAuthStatusResult {
            connected: false,
            provider_email: None,
            token_expired: false,
            needs_reconnect: true,
            connect_url,
            disconnect_url,
        });
    };

    let credentials = crate::encryption::decrypt_json(&cred.credentials, encryption_key)?;

    let access_token = credentials
        .get("oauth_access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    let refresh_token = credentials
        .get("oauth_refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    let has_refresh = refresh_token.is_some();

    let token_expired = credentials
        .get("oauth_token_expiry")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|exp| exp.with_timezone(&chrono::Utc) < chrono::Utc::now())
        .unwrap_or(false);

    let needs_reconnect = token_expired && !has_refresh;

    let provider_email = credentials
        .get("oauth_email")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(DatasourceOAuthStatusResult {
        connected: access_token.is_some(),
        provider_email,
        token_expired,
        needs_reconnect,
        connect_url,
        disconnect_url,
    })
}

/// Result of `datasource_oauth_disconnect_service`.
///
/// Defined in `kyomi_types` because it also crosses into the WASM client as
/// a server_fn response — see `kyomi_types::datasource_contracts`.
pub use kyomi_types::DatasourceOAuthDisconnectResult;

/// Disconnect OAuth credentials for a specific datasource and user.
///
/// Deletes the user's credential row from `user_datasource_credentials`.
///
/// Mirrors the logic from `apps/server/src/routes/auth_datasource_oauth.rs::disconnect`.
pub async fn datasource_oauth_disconnect_service(
    db: &kyomi_core::DbPool,
    user_id: &str,
    workspace_id: &str,
    provider: OAuthProvider,
    datasource_slug: &str,
) -> kyomi_core::Result<DatasourceOAuthDisconnectResult> {
    let ds_id = resolve_datasource_id(db, datasource_slug, workspace_id).await?;

    let result = kyomi_core::db_execute!(
        db,
        "DELETE FROM user_datasource_credentials \
         WHERE user_id = $1 AND datasource_config_id = $2",
        user_id,
        &ds_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("DB error: {e}")))?;

    if result.rows_affected() == 0 {
        return Ok(DatasourceOAuthDisconnectResult {
            success: true,
            already_disconnected: true,
        });
    }

    tracing::info!(
        provider = provider.as_str(),
        datasource_slug = datasource_slug,
        user_id = %user_id,
        "Disconnected OAuth credentials via server_fn"
    );

    Ok(DatasourceOAuthDisconnectResult {
        success: true,
        already_disconnected: false,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Mutex to serialize tests that mutate `MICROSOFT_OAUTH_*` env vars.
    /// `std::env::set_var`/`remove_var` is process-global and not thread-safe,
    /// so these tests must not run concurrently.
    static MS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // -- PKCE generation --

    #[test]
    fn pkce_verifier_length() {
        let pkce = generate_pkce();
        // 32 bytes → 43 base64url chars (no padding)
        assert_eq!(pkce.code_verifier.len(), 43);
    }

    #[test]
    fn pkce_challenge_is_sha256_of_verifier() {
        let pkce = generate_pkce();

        // Recompute the challenge from the verifier
        let mut hasher = Sha256::new();
        hasher.update(pkce.code_verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(hasher.finalize());

        assert_eq!(pkce.code_challenge, expected);
    }

    #[test]
    fn pkce_generates_unique_values() {
        let a = generate_pkce();
        let b = generate_pkce();
        assert_ne!(a.code_verifier, b.code_verifier);
        assert_ne!(a.code_challenge, b.code_challenge);
    }

    // -- Provider parsing --

    #[test]
    fn parse_provider_names() {
        assert_eq!(OAuthProvider::parse("snowflake"), Some(OAuthProvider::Snowflake));
        assert_eq!(OAuthProvider::parse("databricks"), Some(OAuthProvider::Databricks));
        assert_eq!(
            OAuthProvider::parse("bigquery-enterprise"),
            Some(OAuthProvider::BigqueryEnterprise)
        );
        assert_eq!(
            OAuthProvider::parse("microsoft-enterprise"),
            Some(OAuthProvider::MicrosoftEnterprise)
        );
        assert_eq!(OAuthProvider::parse("unknown"), None);
        assert_eq!(OAuthProvider::parse("google"), None);
    }

    #[test]
    fn provider_roundtrip() {
        for provider in OAuthProvider::all() {
            assert_eq!(OAuthProvider::parse(provider.as_str()), Some(*provider));
        }
    }

    #[test]
    fn pkce_flags() {
        assert!(OAuthProvider::Snowflake.uses_pkce());
        assert!(OAuthProvider::Databricks.uses_pkce());
        assert!(OAuthProvider::MicrosoftEnterprise.uses_pkce());
        assert!(!OAuthProvider::BigqueryEnterprise.uses_pkce());
    }

    // -- ProviderConfig extraction --

    #[test]
    fn extract_snowflake_config() {
        let config = json!({
            "account": "xy12345.us-east-1",
            "oauth_client_id": "my-client",
            "oauth_client_secret": "my-secret",
        });
        let pc = ProviderConfig::from_connection_config(OAuthProvider::Snowflake, &config).unwrap();
        assert_eq!(pc.client_id, "my-client");
        assert_eq!(pc.client_secret, "my-secret");
        assert_eq!(pc.account_or_host, "xy12345.us-east-1");
    }

    #[test]
    fn extract_snowflake_config_missing_account() {
        let config = json!({
            "oauth_client_id": "my-client",
        });
        let result = ProviderConfig::from_connection_config(OAuthProvider::Snowflake, &config);
        assert!(result.is_err());
    }

    #[test]
    fn extract_databricks_config() {
        let config = json!({
            "server_hostname": "dbc-abc123.cloud.databricks.com",
            "oauth_client_id": "db-client",
            "oauth_client_secret": "db-secret",
        });
        let pc =
            ProviderConfig::from_connection_config(OAuthProvider::Databricks, &config).unwrap();
        assert_eq!(pc.client_id, "db-client");
        assert_eq!(pc.account_or_host, "dbc-abc123.cloud.databricks.com");
    }

    #[test]
    fn extract_bigquery_enterprise_config() {
        let config = json!({
            "oauth_client_id": "google-client",
            "oauth_client_secret": "google-secret",
            "auth_mode": "enterprise_oauth",
        });
        let pc = ProviderConfig::from_connection_config(
            OAuthProvider::BigqueryEnterprise,
            &config,
        )
        .unwrap();
        assert_eq!(pc.client_id, "google-client");
        assert_eq!(pc.account_or_host, ""); // Google endpoints are static
    }

    #[test]
    fn extract_microsoft_config_with_tenant() {
        let config = json!({
            "tenant_id": "my-tenant-guid",
            "oauth_client_id": "ms-client",
            "oauth_client_secret": "ms-secret",
        });
        let pc = ProviderConfig::from_connection_config(
            OAuthProvider::MicrosoftEnterprise,
            &config,
        )
        .unwrap();
        assert_eq!(pc.account_or_host, "my-tenant-guid");
    }

    #[test]
    fn extract_microsoft_config_defaults_to_common() {
        let config = json!({
            "oauth_client_id": "ms-client",
        });
        let pc = ProviderConfig::from_connection_config(
            OAuthProvider::MicrosoftEnterprise,
            &config,
        )
        .unwrap();
        assert_eq!(pc.account_or_host, "common");
    }

    #[test]
    fn extract_microsoft_config_falls_back_to_env_vars() {
        let _lock = MS_ENV_LOCK.lock().unwrap();
        // SAFETY: mutex serializes all env-var-mutating tests in this module
        unsafe {
            std::env::set_var("MICROSOFT_OAUTH_CLIENT_ID", "env-ms-client");
            std::env::set_var("MICROSOFT_OAUTH_CLIENT_SECRET", "env-ms-secret");
        }

        // No oauth_client_id in connection config — should fall back to env
        let config = json!({
            "tenant_id": "test-tenant",
        });
        let pc = ProviderConfig::from_connection_config(
            OAuthProvider::MicrosoftEnterprise,
            &config,
        )
        .unwrap();
        assert_eq!(pc.client_id, "env-ms-client");
        assert_eq!(pc.client_secret, "env-ms-secret");
        assert_eq!(pc.account_or_host, "test-tenant");

        // Clean up
        unsafe {
            std::env::remove_var("MICROSOFT_OAUTH_CLIENT_ID");
            std::env::remove_var("MICROSOFT_OAUTH_CLIENT_SECRET");
        }
    }

    #[test]
    fn extract_microsoft_config_prefers_connection_config_over_env() {
        let _lock = MS_ENV_LOCK.lock().unwrap();
        // SAFETY: mutex serializes all env-var-mutating tests in this module
        unsafe {
            std::env::set_var("MICROSOFT_OAUTH_CLIENT_ID", "env-should-not-use");
            std::env::set_var("MICROSOFT_OAUTH_CLIENT_SECRET", "env-should-not-use");
        }

        let config = json!({
            "tenant_id": "test-tenant",
            "oauth_client_id": "ds-client",
            "oauth_client_secret": "ds-secret",
        });
        let pc = ProviderConfig::from_connection_config(
            OAuthProvider::MicrosoftEnterprise,
            &config,
        )
        .unwrap();
        assert_eq!(pc.client_id, "ds-client");
        assert_eq!(pc.client_secret, "ds-secret");

        // Clean up
        unsafe {
            std::env::remove_var("MICROSOFT_OAUTH_CLIENT_ID");
            std::env::remove_var("MICROSOFT_OAUTH_CLIENT_SECRET");
        }
    }

    #[test]
    fn extract_microsoft_config_errors_when_no_client_id_anywhere() {
        let _lock = MS_ENV_LOCK.lock().unwrap();
        // SAFETY: mutex serializes all env-var-mutating tests in this module
        unsafe {
            std::env::remove_var("MICROSOFT_OAUTH_CLIENT_ID");
            std::env::remove_var("MICROSOFT_OAUTH_CLIENT_SECRET");
        }

        let config = json!({
            "tenant_id": "test-tenant",
        });
        let result = ProviderConfig::from_connection_config(
            OAuthProvider::MicrosoftEnterprise,
            &config,
        );
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("MICROSOFT_OAUTH_CLIENT_ID"),
            "Error should mention env var fallback, got: {err_msg}"
        );
    }

    #[test]
    fn missing_client_id_errors() {
        let config = json!({"account": "test"});
        let result = ProviderConfig::from_connection_config(OAuthProvider::Snowflake, &config);
        assert!(result.is_err());
    }

    // -- Authorization URL construction --

    #[test]
    fn snowflake_auth_url_contains_pkce() {
        let config = ProviderConfig {
            provider: OAuthProvider::Snowflake,
            client_id: "cid".to_string(),
            client_secret: "csec".to_string(),
            account_or_host: "xy12345".to_string(),
        };
        let result = build_authorization_url(&config, "https://example.com/callback", "test-state");
        assert!(result.url.contains("xy12345.snowflakecomputing.com/oauth/authorize"));
        assert!(result.url.contains("code_challenge="));
        assert!(result.url.contains("code_challenge_method=S256"));
        assert!(result.code_verifier.is_some());
    }

    #[test]
    fn databricks_auth_url_contains_scopes_and_pkce() {
        let config = ProviderConfig {
            provider: OAuthProvider::Databricks,
            client_id: "cid".to_string(),
            client_secret: "csec".to_string(),
            account_or_host: "myhost.databricks.com".to_string(),
        };
        let result = build_authorization_url(&config, "https://example.com/callback", "test-state");
        assert!(result.url.contains("myhost.databricks.com/oidc/v1/authorize"));
        assert!(result.url.contains("all-apis"));
        assert!(result.url.contains("offline_access"));
        assert!(result.code_verifier.is_some());
    }

    #[test]
    fn bigquery_enterprise_auth_url_uses_google() {
        let config = ProviderConfig {
            provider: OAuthProvider::BigqueryEnterprise,
            client_id: "cid".to_string(),
            client_secret: "csec".to_string(),
            account_or_host: String::new(),
        };
        let result = build_authorization_url(&config, "https://example.com/callback", "test-state");
        assert!(result.url.contains("accounts.google.com"));
        assert!(result.url.contains("bigquery.readonly"));
        assert!(result.url.contains("access_type=offline"));
        assert!(result.url.contains("prompt=consent"));
        assert!(result.code_verifier.is_none());
    }

    #[test]
    fn microsoft_enterprise_auth_url_uses_tenant_and_pkce() {
        let config = ProviderConfig {
            provider: OAuthProvider::MicrosoftEnterprise,
            client_id: "cid".to_string(),
            client_secret: "csec".to_string(),
            account_or_host: "my-tenant".to_string(),
        };
        let result = build_authorization_url(&config, "https://example.com/callback", "test-state");
        assert!(result.url.contains("login.microsoftonline.com/my-tenant"));
        assert!(result.url.contains("database.windows.net"));
        assert!(result.url.contains("offline_access"));
        assert!(result.url.contains("code_challenge="));
        assert!(result.url.contains("code_challenge_method=S256"));
        assert!(result.code_verifier.is_some());
    }
}
