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

    /// How — if at all — Kyomi can revoke this provider's OAuth grant at the
    /// provider itself when a user disconnects (KYO-714).
    ///
    /// Written as an exhaustive `match` with **no wildcard arm on purpose**.
    /// A fifth provider added to [`OAuthProvider`] will fail to compile here
    /// until someone establishes, and records below, whether that provider
    /// can be revoked. Before this method existed the answer was implicit and
    /// uniformly wrong: `datasource_oauth_disconnect_service` deleted the
    /// local credential for every provider and revoked nothing, while telling
    /// the user their account was disconnected.
    ///
    /// Each arm's evidence was checked against primary vendor documentation
    /// on 2026-09-15.
    pub fn revocation_capability(&self) -> RevocationCapability {
        match self {
            // Snowflake's OAuth surface is `/oauth/authorize`,
            // `/oauth/token-request` and `/oauth/token` — there is no HTTP
            // revocation endpoint. Revocation is SQL DDL
            // (`ALTER USER … REMOVE DELEGATED AUTHORIZATION`), which needs a
            // privileged session Kyomi does not hold on the OAuth path.
            // Checked 2026-09-15 against docs.snowflake.com
            // /en/user-guide/oauth-custom and /sql-reference/functions-system.
            //
            // The `SYSTEM$REVOKE_OAUTH_REFRESH_TOKEN` function some earlier
            // notes referred to does not exist — it is absent from
            // Snowflake's system-function index. Do not build on it.
            Self::Snowflake => RevocationCapability::Unsupported,

            // Databricks' live account-level discovery document
            // (`accounts.cloud.databricks.com/oidc/.well-known
            // /oauth-authorization-server`) advertises no
            // `revocation_endpoint`. A consent-removal API exists, but
            // Databricks states "Revoking consent doesn't invalidate existing
            // tokens", so calling it would not revoke the grant either.
            // Checked 2026-09-15 against
            // docs.databricks.com/aws/en/dev-tools/auth/oauth-u2m.
            Self::Databricks => RevocationCapability::Unsupported,

            // Google documents `POST https://oauth2.googleapis.com/revoke`
            // with a form-encoded `token` parameter, accepting either an
            // access or a refresh token, and revoking either one revokes the
            // whole grant. Checked 2026-09-15 against
            // developers.google.com/identity/protocols/oauth2/web-server.
            Self::BigqueryEnterprise => {
                RevocationCapability::GoogleRevokeEndpoint(crate::google_oauth::GOOGLE_REVOKE_URI)
            }

            // Entra ID's live v2.0 discovery document advertises no
            // `revocation_endpoint`. Microsoft Graph's `revokeSignInSessions`
            // is not a substitute: it requires `User.RevokeSessions.All` and
            // signs the user out of *every* application in the tenant — the
            // wrong blast radius for disconnecting one datasource. Checked
            // 2026-09-15 against
            // learn.microsoft.com/en-us/graph/api/user-revokesigninsessions.
            Self::MicrosoftEnterprise => RevocationCapability::Unsupported,
        }
    }
}

/// What revocation mechanism, if any, Kyomi can drive for a given
/// [`OAuthProvider`] — the return of
/// [`OAuthProvider::revocation_capability`].
///
/// Deliberately describes a *concrete* mechanism rather than a boolean: the
/// disconnect path has to know which request to make, and a bare
/// `supports_revocation -> bool` would push that decision back out to the
/// caller where it could disagree with this mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationCapability {
    /// The grant is a Google OAuth grant, revocable by POSTing the stored
    /// token to Google's revocation endpoint at this URI (form field
    /// `token`; HTTP 400 means the token was already invalid). Handled by
    /// `crate::google_oauth::revoke_google_token_string_at`.
    GoogleRevokeEndpoint(&'static str),

    /// Kyomi has no revocation mechanism it can call for this provider from
    /// an OAuth-authenticated session. Disconnecting deletes the locally
    /// stored credential only — the grant stays live at the provider, and
    /// the user has to remove Kyomi's access there themselves. Callers must
    /// report this honestly rather than claiming the account was
    /// disconnected; see `kyomi_types::DatasourceOAuthRevocationOutcome`.
    Unsupported,
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
    /// `config` is the **raw, encrypted-at-rest** `connection_config` as read
    /// from the database — `oauth_client_secret` is a `COMMON_SENSITIVE`
    /// field (KYO-786) and is decrypted internally via
    /// [`crate::credential_service::decrypt_connection_config_secrets`]
    /// before it's read. Taking `key` here rather than requiring callers to
    /// decrypt first closes the class of bug KYO-786 fixed: there were two
    /// production call sites reading `connection_config` straight off a
    /// `SELECT` and handing it to this function, both silently treating a
    /// ciphertext client secret as if it were plaintext.
    ///
    /// For Microsoft Enterprise, falls back to `MICROSOFT_OAUTH_CLIENT_ID` and
    /// `MICROSOFT_OAUTH_CLIENT_SECRET` env vars when per-datasource credentials
    /// are not configured. This matches the Python implementation where Kyomi
    /// has one Azure AD multi-tenant app registration that can authenticate
    /// users from any tenant.
    pub fn from_connection_config(
        provider: OAuthProvider,
        config: &serde_json::Value,
        key: &[u8; 32],
    ) -> kyomi_core::Result<Self> {
        let config = crate::credential_service::decrypt_connection_config_secrets(config, key)?;
        let config = &config;

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
///
/// Defined in `kyomi_types` as `DatasourceOAuthStatus` (the canonical wire
/// name) because it also crosses into the WASM client as a server_fn
/// response — see `kyomi_types::datasource_contracts`. Re-exported under
/// this service-return name so existing call sites keep compiling
/// unchanged.
pub use kyomi_types::DatasourceOAuthStatus as DatasourceOAuthStatusResult;

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

/// What happened to the grant at the provider — see the type's own docs.
pub use kyomi_types::DatasourceOAuthRevocationOutcome;

/// Disconnect OAuth credentials for a specific datasource and user.
///
/// Revokes the grant at the provider where that is possible, then deletes the
/// user's credential row from `user_datasource_credentials`. The returned
/// [`DatasourceOAuthDisconnectResult::revocation`] says which of those two
/// things actually happened, because for three of the four providers only the
/// second one can (see [`OAuthProvider::revocation_capability`]).
///
/// # Failure policy: fail closed, but only where there is something to fail
///
/// If revocation is attempted and does not succeed, this function **returns
/// the error and leaves the credential row intact**. Deleting the row would
/// discard the only copy of the token Kyomi holds, so a grant left live at
/// Google could never afterwards be revoked — not by Kyomi, and not by a
/// retry. Keeping the row costs the user a retry; deleting it costs them a
/// permanently unrevokable grant. This mirrors KYO-700's
/// `crate::google_oauth::google_oauth_disconnect_service`.
///
/// A Google HTTP 400 is *not* a failure: it means the token was already
/// invalid, which is the end state the disconnect was trying to reach. It is
/// reported as [`DatasourceOAuthRevocationOutcome::AlreadyInvalid`] and the
/// row is deleted.
///
/// The policy is deliberately asymmetric. For a provider whose capability is
/// [`RevocationCapability::Unsupported`] there is no request to make and so
/// nothing that can fail, and the disconnect always proceeds. Blocking it
/// would strand the user with a credential they cannot remove from Kyomi and
/// would buy no revocation in exchange, since none was ever available. The
/// asymmetry extends to *decrypting* the stored credential, which happens
/// only inside the revocable branch: a key that can no longer read the row
/// must not make a non-revocable provider's credential undeletable, because
/// the reconnect path decrypts the same row and would fail the same way.
///
/// Both call sites — the `disconnect_datasource_oauth` server_fn and
/// `POST /api/v1/auth/oauth/{provider}/disconnect` — go through this
/// function; neither reimplements it.
pub async fn datasource_oauth_disconnect_service(
    db: &kyomi_core::DbPool,
    user_id: &str,
    workspace_id: &str,
    provider: OAuthProvider,
    datasource_slug: &str,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<DatasourceOAuthDisconnectResult> {
    datasource_oauth_disconnect_service_at(
        None,
        db,
        user_id,
        workspace_id,
        provider,
        datasource_slug,
        encryption_key,
    )
    .await
}

/// Test seam for [`datasource_oauth_disconnect_service`]: identical
/// behaviour, except that `revoke_uri_override` — when `Some` — replaces the
/// revocation URI carried by
/// [`RevocationCapability::GoogleRevokeEndpoint`], so `mod tests` can point
/// it at a local `wiremock::MockServer`.
///
/// The override is applied *after* the capability match, never instead of it:
/// a provider whose capability is [`RevocationCapability::Unsupported`] still
/// makes no HTTP request even when an override is supplied, which is what
/// lets a test prove no call was attempted.
async fn datasource_oauth_disconnect_service_at(
    revoke_uri_override: Option<&str>,
    db: &kyomi_core::DbPool,
    user_id: &str,
    workspace_id: &str,
    provider: OAuthProvider,
    datasource_slug: &str,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<DatasourceOAuthDisconnectResult> {
    use kyomi_core::models::datasource::UserDatasourceCredential;

    let ds_id = resolve_datasource_id(db, datasource_slug, workspace_id).await?;

    // Read the row *before* deleting anything: the token it holds is the only
    // thing that can revoke the grant, and the pre-KYO-714 code deleted blind.
    // Modelled on `datasource_oauth_status_service` above.
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
        return Ok(DatasourceOAuthDisconnectResult {
            success: true,
            already_disconnected: true,
            revocation: DatasourceOAuthRevocationOutcome::NoStoredToken,
        });
    };

    // Decrypting the stored credential happens *inside* the revocable arm,
    // not above this match, and the placement is load-bearing. A provider
    // whose capability is `Unsupported` never reads the decrypted value, so
    // hoisting the `?` would let a rotated `ENCRYPTION_KEY` — or a database
    // restored against a different one — abort a disconnect that had no
    // revocation to lose. It would also strand the row: reconnecting goes
    // through `datasource_service::merge_credentials`, which decrypts the
    // same row and fails identically, leaving the credential neither
    // removable nor replaceable. See the failure-policy section on
    // `datasource_oauth_disconnect_service`.
    let revocation = match provider.revocation_capability() {
        RevocationCapability::Unsupported => DatasourceOAuthRevocationOutcome::NotSupported,
        RevocationCapability::GoogleRevokeEndpoint(uri) => {
            // Load-bearing in this arm, unlike the one above: the revocation
            // request is built from what this decrypts, so failing closed
            // here is the correct outcome.
            let credentials = crate::encryption::decrypt_json(&cred.credentials, encryption_key)?;

            // Prefer the refresh token for the same reason
            // `google_oauth::select_revocation_token` documents: it is the
            // longer-lived credential, and Google revokes the whole grant
            // given either kind.
            let token = ["oauth_refresh_token", "oauth_access_token"]
                .into_iter()
                .find_map(|field| {
                    credentials
                        .get(field)
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                });

            match token {
                None => DatasourceOAuthRevocationOutcome::NoStoredToken,
                Some(token) => {
                    let uri = revoke_uri_override.unwrap_or(uri);
                    match crate::google_oauth::revoke_google_token_string_at(
                        uri,
                        token,
                        crate::google_oauth::GoogleGrantSubject::Datasource,
                    )
                    .await
                    {
                        Ok(crate::google_oauth::GoogleRevokeOutcome::Revoked) => {
                            DatasourceOAuthRevocationOutcome::Revoked
                        }
                        Ok(crate::google_oauth::GoogleRevokeOutcome::AlreadyInvalid) => {
                            DatasourceOAuthRevocationOutcome::AlreadyInvalid
                        }
                        Err(e) => {
                            tracing::warn!(
                                provider = provider.as_str(),
                                datasource_slug = datasource_slug,
                                user_id = %user_id,
                                error = %e,
                                "OAuth revocation failed; disconnect aborted, credential preserved"
                            );
                            return Err(e);
                        }
                    }
                }
            }
        }
    };

    // `rows_affected` is not re-checked against zero here: the row was just
    // read, and a concurrent delete racing this one reaches the same end
    // state the caller asked for. `already_disconnected` reports what was
    // observed at read time.
    kyomi_core::db_execute!(
        db,
        "DELETE FROM user_datasource_credentials \
         WHERE user_id = $1 AND datasource_config_id = $2",
        user_id,
        &ds_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("DB error: {e}")))?;

    tracing::info!(
        provider = provider.as_str(),
        datasource_slug = datasource_slug,
        user_id = %user_id,
        revocation = ?revocation,
        "Disconnected OAuth credentials"
    );

    Ok(DatasourceOAuthDisconnectResult {
        success: true,
        already_disconnected: false,
        revocation,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::EnvVarGuard;
    use crate::test_support::{seed_user, seed_workspace, sqlite_pool, test_key, test_pool};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
        let pc = ProviderConfig::from_connection_config(OAuthProvider::Snowflake, &config, &test_key()).unwrap();
        assert_eq!(pc.client_id, "my-client");
        assert_eq!(pc.client_secret, "my-secret");
        assert_eq!(pc.account_or_host, "xy12345.us-east-1");
    }

    #[test]
    fn extract_snowflake_config_decrypts_an_encrypted_client_secret() {
        // KYO-786 regression guard: `connection_config` as read from the
        // database has oauth_client_secret encrypted at rest
        // (COMMON_SENSITIVE). Without `from_connection_config` decrypting
        // internally, `client_secret` here would be the raw ciphertext blob
        // — which Snowflake's token endpoint would reject as `invalid_client`
        // rather than authenticating with the real secret.
        let key = test_key();
        let ciphertext = crate::encryption::encrypt("my-real-secret", &key).unwrap();
        let config = json!({
            "account": "xy12345.us-east-1",
            "oauth_client_id": "my-client",
            "oauth_client_secret": ciphertext,
        });

        let pc = ProviderConfig::from_connection_config(OAuthProvider::Snowflake, &config, &key).unwrap();

        assert_eq!(
            pc.client_secret, "my-real-secret",
            "from_connection_config must decrypt oauth_client_secret before returning it"
        );
    }

    #[test]
    fn extract_snowflake_config_missing_account() {
        let config = json!({
            "oauth_client_id": "my-client",
        });
        let result = ProviderConfig::from_connection_config(OAuthProvider::Snowflake, &config, &test_key());
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
            ProviderConfig::from_connection_config(OAuthProvider::Databricks, &config, &test_key()).unwrap();
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
            &test_key(),
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
            &test_key(),
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
            &test_key(),
        )
        .unwrap();
        assert_eq!(pc.account_or_host, "common");
    }

    #[test]
    fn extract_microsoft_config_falls_back_to_env_vars() {
        let _guard = EnvVarGuard::acquire()
            .set("MICROSOFT_OAUTH_CLIENT_ID", "env-ms-client")
            .set("MICROSOFT_OAUTH_CLIENT_SECRET", "env-ms-secret");

        // No oauth_client_id in connection config — should fall back to env
        let config = json!({
            "tenant_id": "test-tenant",
        });
        let pc = ProviderConfig::from_connection_config(
            OAuthProvider::MicrosoftEnterprise,
            &config,
            &test_key(),
        )
        .unwrap();
        assert_eq!(pc.client_id, "env-ms-client");
        assert_eq!(pc.client_secret, "env-ms-secret");
        assert_eq!(pc.account_or_host, "test-tenant");
    }

    #[test]
    fn extract_microsoft_config_prefers_connection_config_over_env() {
        let _guard = EnvVarGuard::acquire()
            .set("MICROSOFT_OAUTH_CLIENT_ID", "env-should-not-use")
            .set("MICROSOFT_OAUTH_CLIENT_SECRET", "env-should-not-use");

        let config = json!({
            "tenant_id": "test-tenant",
            "oauth_client_id": "ds-client",
            "oauth_client_secret": "ds-secret",
        });
        let pc = ProviderConfig::from_connection_config(
            OAuthProvider::MicrosoftEnterprise,
            &config,
            &test_key(),
        )
        .unwrap();
        assert_eq!(pc.client_id, "ds-client");
        assert_eq!(pc.client_secret, "ds-secret");
    }

    #[test]
    fn extract_microsoft_config_errors_when_no_client_id_anywhere() {
        let _guard = EnvVarGuard::acquire()
            .remove("MICROSOFT_OAUTH_CLIENT_ID")
            .remove("MICROSOFT_OAUTH_CLIENT_SECRET");

        let config = json!({
            "tenant_id": "test-tenant",
        });
        let result = ProviderConfig::from_connection_config(
            OAuthProvider::MicrosoftEnterprise,
            &config,
            &test_key(),
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
        let result = ProviderConfig::from_connection_config(OAuthProvider::Snowflake, &config, &test_key());
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
    // ── KYO-714: revocation capability mapping ──────────────────────────

    /// AC1 lives in `revocation_capability`'s `match`, so this test is what
    /// pins the four answers that match encodes. The evidence for each is
    /// recorded in the arm's own comment; asserting them here means a later
    /// edit that flips one has to flip a test too.
    #[test]
    fn only_bigquery_enterprise_has_a_revocation_endpoint() {
        assert_eq!(
            OAuthProvider::BigqueryEnterprise.revocation_capability(),
            RevocationCapability::GoogleRevokeEndpoint(crate::google_oauth::GOOGLE_REVOKE_URI),
            "BigQuery Enterprise grants are Google grants and must revoke at \
             Google's documented endpoint"
        );

        for provider in [
            OAuthProvider::Snowflake,
            OAuthProvider::Databricks,
            OAuthProvider::MicrosoftEnterprise,
        ] {
            assert_eq!(
                provider.revocation_capability(),
                RevocationCapability::Unsupported,
                "{} exposes no revocation endpoint Kyomi can call — claiming \
                 otherwise is the KYO-714 defect",
                provider.as_str()
            );
        }
    }

    /// `OAuthProvider::all()` is what the REST layer enumerates, so a
    /// provider missing from it would never be exercised by the loop above.
    /// Asserting `all()` covers every variant keeps that loop honest as
    /// providers are added.
    #[test]
    fn every_registered_provider_has_a_recorded_revocation_capability() {
        assert_eq!(
            OAuthProvider::all().len(),
            4,
            "a provider was added or removed — `revocation_capability` must \
             record its evidence and this test must cover it"
        );

        for provider in OAuthProvider::all() {
            // Exercises the match arm; a provider added without an arm would
            // not compile at all, which is the point of the wildcard-free
            // match.
            let _ = provider.revocation_capability();
        }
    }

    // ── KYO-714: disconnect — revocation then delete ────────────────────

    /// Seed a workspace, a datasource and one encrypted OAuth credential row
    /// for `user-a`, returning the pool. Domain-specific to this module, so
    /// it lives here rather than in `test_support` (see that module's docs).
    async fn seed_oauth_datasource(
        credentials: serde_json::Value,
    ) -> (kyomi_core::DbPool, [u8; 32]) {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "user-a", "a@example.com").await;
        seed_workspace(sq, "ws-1", "user-a").await;

        sqlx::query(
            "INSERT INTO datasource_configs \
             (id, workspace_id, name, datasource_type, slug, active) \
             VALUES ('ds-1', 'ws-1', 'Warehouse', 'bigquery', 'warehouse', 1)",
        )
        .execute(sq)
        .await
        .expect("insert datasource_config");

        let key = test_key();
        let encrypted = crate::encryption::encrypt_json(&credentials, &key).expect("encrypt");

        sqlx::query(
            "INSERT INTO user_datasource_credentials \
             (user_id, datasource_config_id, workspace_id, credentials, created_at, updated_at) \
             VALUES ('user-a', 'ds-1', 'ws-1', $1, $2, $3)",
        )
        .bind(&encrypted)
        .bind(chrono::Utc::now())
        .bind(chrono::Utc::now())
        .execute(sq)
        .await
        .expect("insert credential");

        (db, key)
    }

    /// A key that cannot decrypt anything `seed_oauth_datasource` wrote —
    /// the rotated-`ENCRYPTION_KEY` / restored-from-another-deployment case.
    fn rotated_key(key: [u8; 32]) -> [u8; 32] {
        let mut rotated = key;
        rotated[0] ^= 0xff;
        rotated
    }

    /// Guard for the two tests below: assert the seeded row really is
    /// undecryptable under `key`. Without this, a fixture that happened to
    /// stay readable would make both of them pass for the wrong reason.
    async fn assert_credential_is_unreadable(db: &kyomi_core::DbPool, key: &[u8; 32]) {
        let stored = sqlx::query_scalar::<_, String>(
            "SELECT credentials FROM user_datasource_credentials \
             WHERE user_id = 'user-a' AND datasource_config_id = 'ds-1'",
        )
        .fetch_one(sqlite_pool(db))
        .await
        .expect("read stored credential");

        assert!(
            crate::encryption::decrypt_json(&stored, key).is_err(),
            "the fixture must be undecryptable under this key, or the test \
             proves nothing"
        );
    }

    async fn credential_rows(db: &kyomi_core::DbPool) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM user_datasource_credentials \
             WHERE user_id = 'user-a' AND datasource_config_id = 'ds-1'",
        )
        .fetch_one(sqlite_pool(db))
        .await
        .expect("count credentials")
    }

    #[tokio::test]
    async fn disconnect_revokes_at_google_then_deletes_the_row() {
        let (db, key) = seed_oauth_datasource(json!({
            "oauth_access_token": "access-xyz",
            "oauth_refresh_token": "refresh-abc",
        }))
        .await;

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;
        let revoke_uri = format!("{}/revoke", mock_server.uri());

        let result = datasource_oauth_disconnect_service_at(
            Some(&revoke_uri),
            &db,
            "user-a",
            "ws-1",
            OAuthProvider::BigqueryEnterprise,
            "warehouse",
            &key,
        )
        .await
        .expect("disconnect must succeed when Google confirms revocation");

        assert!(result.success);
        assert!(!result.already_disconnected);
        assert_eq!(
            result.revocation,
            DatasourceOAuthRevocationOutcome::Revoked
        );
        assert_eq!(credential_rows(&db).await, 0, "the row must be deleted");

        let requests = mock_server
            .received_requests()
            .await
            .expect("request recording enabled");
        assert_eq!(requests.len(), 1, "exactly one revocation request");
        let body = String::from_utf8(requests[0].body.clone()).expect("utf8 body");
        assert!(
            body.contains("refresh-abc"),
            "the refresh token is the more durable credential and must be the \
             one sent: {body}"
        );
        assert!(!body.contains("access-xyz"));
    }

    /// Google answers 400 when the token is already invalid. The grant is
    /// already gone, which is the end state the user asked for, so the
    /// disconnect completes rather than failing.
    #[tokio::test]
    async fn disconnect_treats_google_400_as_already_invalid_and_deletes_the_row() {
        let (db, key) = seed_oauth_datasource(json!({
            "oauth_access_token": "access-xyz",
        }))
        .await;

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&mock_server)
            .await;
        let revoke_uri = format!("{}/revoke", mock_server.uri());

        let result = datasource_oauth_disconnect_service_at(
            Some(&revoke_uri),
            &db,
            "user-a",
            "ws-1",
            OAuthProvider::BigqueryEnterprise,
            "warehouse",
            &key,
        )
        .await
        .expect("a 400 means the grant was already gone — not a failure");

        assert_eq!(
            result.revocation,
            DatasourceOAuthRevocationOutcome::AlreadyInvalid
        );
        assert_eq!(credential_rows(&db).await, 0, "the row must be deleted");
    }

    /// The load-bearing fail-closed assertion: an unconfirmed revocation must
    /// leave the credential row in place. Deleting it would discard Kyomi's
    /// only copy of the token, leaving a live grant at Google that nobody —
    /// not even a retry — could ever revoke.
    #[tokio::test]
    async fn disconnect_keeps_the_row_when_google_returns_500() {
        let (db, key) = seed_oauth_datasource(json!({
            "oauth_refresh_token": "refresh-abc",
        }))
        .await;

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;
        let revoke_uri = format!("{}/revoke", mock_server.uri());

        let result = datasource_oauth_disconnect_service_at(
            Some(&revoke_uri),
            &db,
            "user-a",
            "ws-1",
            OAuthProvider::BigqueryEnterprise,
            "warehouse",
            &key,
        )
        .await;

        assert!(result.is_err(), "a 500 must fail the disconnect");
        assert_eq!(
            credential_rows(&db).await,
            1,
            "the credential must survive so the user can retry the revocation"
        );
    }

    /// Same fail-closed policy, reached through a genuine connection failure
    /// rather than a mocked status — nothing is listening on this port.
    #[tokio::test]
    async fn disconnect_keeps_the_row_when_the_revocation_request_cannot_be_sent() {
        let (db, key) = seed_oauth_datasource(json!({
            "oauth_refresh_token": "refresh-abc",
        }))
        .await;

        let result = datasource_oauth_disconnect_service_at(
            Some("http://127.0.0.1:1/revoke"),
            &db,
            "user-a",
            "ws-1",
            OAuthProvider::BigqueryEnterprise,
            "warehouse",
            &key,
        )
        .await;

        assert!(result.is_err(), "a transport failure must fail the disconnect");
        assert_eq!(
            credential_rows(&db).await,
            1,
            "the credential must survive so the user can retry the revocation"
        );
    }

    /// A provider with no revocation endpoint must not have one invented for
    /// it. The override URI is supplied deliberately: if the capability match
    /// were bypassed, the mock would record a request and this test would
    /// fail.
    #[tokio::test]
    async fn disconnect_of_a_non_revocable_provider_makes_no_http_call() {
        let (db, key) = seed_oauth_datasource(json!({
            "oauth_access_token": "access-xyz",
            "oauth_refresh_token": "refresh-abc",
        }))
        .await;

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;
        let revoke_uri = format!("{}/revoke", mock_server.uri());

        let result = datasource_oauth_disconnect_service_at(
            Some(&revoke_uri),
            &db,
            "user-a",
            "ws-1",
            OAuthProvider::Snowflake,
            "warehouse",
            &key,
        )
        .await
        .expect("a provider with nothing to revoke must still disconnect");

        assert_eq!(
            result.revocation,
            DatasourceOAuthRevocationOutcome::NotSupported,
            "the caller must be told the grant is still live at the provider"
        );
        assert_eq!(credential_rows(&db).await, 0, "the row must be deleted");

        let requests = mock_server
            .received_requests()
            .await
            .expect("request recording enabled");
        assert!(
            requests.is_empty(),
            "no revocation request may be sent for a provider that has no \
             revocation endpoint, got {} request(s)",
            requests.len()
        );
    }

    /// A revocable provider whose stored credential holds no token: there is
    /// nothing to send, so no call is made and the row is still deleted.
    #[tokio::test]
    async fn disconnect_with_no_stored_token_deletes_without_calling_the_provider() {
        let (db, key) = seed_oauth_datasource(json!({
            "oauth_access_token": "",
            "oauth_email": "user@example.com",
        }))
        .await;

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;
        let revoke_uri = format!("{}/revoke", mock_server.uri());

        let result = datasource_oauth_disconnect_service_at(
            Some(&revoke_uri),
            &db,
            "user-a",
            "ws-1",
            OAuthProvider::BigqueryEnterprise,
            "warehouse",
            &key,
        )
        .await
        .expect("an empty token is nothing to revoke, not a failure");

        assert_eq!(
            result.revocation,
            DatasourceOAuthRevocationOutcome::NoStoredToken
        );
        assert!(!result.already_disconnected, "a row did exist");
        assert_eq!(credential_rows(&db).await, 0);
        assert!(
            mock_server
                .received_requests()
                .await
                .expect("request recording enabled")
                .is_empty(),
            "an empty token must not be POSTed to Google"
        );
    }

    /// The asymmetry the disconnect policy claims in its docs, pinned: a
    /// provider with nothing to revoke never reads the decrypted credential,
    /// so a key that cannot read it must not block the disconnect.
    ///
    /// This is the rotated-`ENCRYPTION_KEY` / database-restored-against-a-
    /// different-key case, which is realistic on self-hosted. Failing here
    /// would strand the user with no way out: the row could not be deleted,
    /// and reconnecting goes through `save_user_credential` →
    /// `datasource_service::merge_credentials`, which decrypts the same row
    /// and fails identically — leaving the credential both unremovable and
    /// unreplaceable through the UI. It would also buy nothing, since no
    /// revocation was ever available for this provider.
    #[tokio::test]
    async fn disconnect_of_a_non_revocable_provider_survives_an_unreadable_credential() {
        let (db, key) = seed_oauth_datasource(json!({
            "oauth_access_token": "access-xyz",
            "oauth_refresh_token": "refresh-abc",
        }))
        .await;

        let rotated = rotated_key(key);
        assert_credential_is_unreadable(&db, &rotated).await;

        let result = datasource_oauth_disconnect_service(
            &db,
            "user-a",
            "ws-1",
            OAuthProvider::Snowflake,
            "warehouse",
            &rotated,
        )
        .await
        .expect(
            "a provider with nothing to revoke must disconnect even when the stored \
             credential cannot be decrypted",
        );

        assert!(result.success);
        assert!(!result.already_disconnected, "a row did exist");
        assert_eq!(
            result.revocation,
            DatasourceOAuthRevocationOutcome::NotSupported
        );
        assert_eq!(
            credential_rows(&db).await,
            0,
            "the row must be deleted — keeping it makes the credential both \
             unremovable and unreplaceable through the UI"
        );
    }

    /// The other half of the same asymmetry: for the one revocable provider
    /// the decrypted credential *is* what the revocation request is built
    /// from, so a credential that cannot be read means the grant cannot be
    /// revoked — and the fail-closed policy applies exactly as it does to a
    /// 500 from Google. The row must survive.
    #[tokio::test]
    async fn disconnect_of_a_revocable_provider_fails_closed_on_an_unreadable_credential() {
        let (db, key) = seed_oauth_datasource(json!({
            "oauth_refresh_token": "refresh-abc",
        }))
        .await;

        let rotated = rotated_key(key);
        assert_credential_is_unreadable(&db, &rotated).await;

        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/revoke"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;
        let revoke_uri = format!("{}/revoke", mock_server.uri());

        let result = datasource_oauth_disconnect_service_at(
            Some(&revoke_uri),
            &db,
            "user-a",
            "ws-1",
            OAuthProvider::BigqueryEnterprise,
            "warehouse",
            &rotated,
        )
        .await;

        assert!(
            result.is_err(),
            "an unreadable credential means the grant cannot be revoked, which \
             must not be reported as a disconnect"
        );
        assert_eq!(
            credential_rows(&db).await,
            1,
            "the credential must survive — deleting it would discard the only \
             token that could ever revoke the grant"
        );
        assert!(
            mock_server
                .received_requests()
                .await
                .expect("request recording enabled")
                .is_empty(),
            "nothing readable to send, so no revocation request may be made"
        );
    }

    /// Pre-existing behaviour that must not change: no credential row at all
    /// still returns `already_disconnected: true` with `success: true`.
    #[tokio::test]
    async fn disconnect_with_no_credential_row_reports_already_disconnected() {
        let db = test_pool().await;
        let sq = sqlite_pool(&db);
        seed_user(sq, "user-a", "a@example.com").await;
        seed_workspace(sq, "ws-1", "user-a").await;
        sqlx::query(
            "INSERT INTO datasource_configs \
             (id, workspace_id, name, datasource_type, slug, active) \
             VALUES ('ds-1', 'ws-1', 'Warehouse', 'bigquery', 'warehouse', 1)",
        )
        .execute(sq)
        .await
        .expect("insert datasource_config");

        let result = datasource_oauth_disconnect_service(
            &db,
            "user-a",
            "ws-1",
            OAuthProvider::BigqueryEnterprise,
            "warehouse",
            &test_key(),
        )
        .await
        .expect("disconnecting nothing is not an error");

        assert!(result.success);
        assert!(result.already_disconnected);
        assert_eq!(
            result.revocation,
            DatasourceOAuthRevocationOutcome::NoStoredToken
        );
    }
}
