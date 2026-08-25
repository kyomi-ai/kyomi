// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for OAuth credential management — Google OAuth and
//! per-datasource OAuth (Snowflake, Databricks, BigQuery Enterprise, etc.).
//!
//! All functions require an authenticated session.  They call service-layer
//! functions directly — no HTTP loopback.
//!
//! ## Corresponding REST routes
//!
//! - `GET  /auth/google-oauth/status`        → `get_google_oauth_status()`
//! - `GET  /auth/google-oauth/projects`      → `get_google_oauth_projects()`
//! - `POST /auth/google-oauth/disconnect`    → `disconnect_google_oauth()`
//! - `GET  /auth/oauth/{provider}/status`    → `get_datasource_oauth_status()`
//! - `POST /auth/oauth/{provider}/disconnect`→ `disconnect_datasource_oauth()`

use leptos::prelude::*;

#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

// ---------------------------------------------------------------------------
// Return types
// ---------------------------------------------------------------------------

/// Connection status for a user's Google OAuth account.
pub use kyomi_types::GoogleOAuthStatus;

/// A single Google Cloud project returned by `get_google_oauth_projects`.
pub use kyomi_types::GoogleProject;

/// Project list returned by `get_google_oauth_projects`.
pub use kyomi_types::GoogleOAuthProjectsResult;

/// Result of disconnecting a Google OAuth account.
pub use kyomi_types::GoogleOAuthDisconnectResult;

/// Connection status for a user's per-datasource OAuth account.
pub use kyomi_types::DatasourceOAuthStatus;

/// Result of disconnecting per-datasource OAuth credentials.
pub use kyomi_types::DatasourceOAuthDisconnectResult;

// ---------------------------------------------------------------------------
// Server functions
// ---------------------------------------------------------------------------

/// Get the current Google OAuth connection status for the authenticated user.
///
/// Authenticated endpoint — requires a valid session cookie.
/// Mirrors `GET /auth/google-oauth/status` in
/// `apps/server/src/routes/auth_google_oauth.rs`.
///
/// Delegates to `kyomi_auth::google_oauth::google_oauth_status_service`.
#[server(prefix = "/leptos-api")]
pub async fn get_google_oauth_status() -> Result<GoogleOAuthStatus, ServerFnError> {
    use kyomi_auth::google_oauth::google_oauth_status_service;

    let ac = AuthenticatedContext::extract().await?;
    let encryption_key = ac.encryption_key()?;

    google_oauth_status_service(ac.db(), &ac.auth.user_id, &encryption_key)
        .await
        .into_sfn()
}

/// List Google Cloud projects accessible to the authenticated user.
///
/// Authenticated endpoint — requires a valid session cookie.
/// Mirrors `GET /auth/google-oauth/projects` in
/// `apps/server/src/routes/auth_google_oauth.rs`.
///
/// Resolves the user's Google OAuth token (refreshing if needed), then calls
/// the Google Cloud Resource Manager API.  Requires `GOOGLE_OAUTH_CLIENT_ID`
/// and `GOOGLE_OAUTH_CLIENT_SECRET` to be configured.
///
/// Delegates to `kyomi_auth::google_oauth::google_oauth_projects_service`.
#[server(prefix = "/leptos-api")]
pub async fn get_google_oauth_projects() -> Result<GoogleOAuthProjectsResult, ServerFnError> {
    use kyomi_auth::google_oauth::google_oauth_projects_service;

    let ac = AuthenticatedContext::extract().await?;
    let encryption_key = ac.encryption_key()?;
    let client_id = ac
        .ctx
        .config
        .google_oauth_client_id
        .as_ref()
        .ok_or_else(|| ServerFnError::new("GOOGLE_OAUTH_CLIENT_ID not configured"))?
        .clone();
    let client_secret = ac
        .ctx
        .config
        .google_oauth_client_secret
        .as_ref()
        .ok_or_else(|| ServerFnError::new("GOOGLE_OAUTH_CLIENT_SECRET not configured"))?
        .clone();

    google_oauth_projects_service(
        ac.db(),
        &ac.auth.user_id,
        &encryption_key,
        &client_id,
        &client_secret,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "google_oauth_projects_service error");
        // user_message() (KYO-448) — Display would leak the variant tag.
        ServerFnError::new(e.user_message())
    })
}

/// Disconnect the Google OAuth account from the authenticated user.
///
/// Authenticated endpoint — requires a valid session cookie.
/// Mirrors `POST /auth/google-oauth/disconnect` in
/// `apps/server/src/routes/auth_google_oauth.rs`.
///
/// Clears stored tokens and removes the `google_oauth` auth method entry.
/// Returns `already_disconnected: true` if no account was linked.
///
/// Delegates to `kyomi_auth::google_oauth::google_oauth_disconnect_service`.
#[server(prefix = "/leptos-api")]
pub async fn disconnect_google_oauth() -> Result<GoogleOAuthDisconnectResult, ServerFnError> {
    use kyomi_auth::google_oauth::google_oauth_disconnect_service;

    let ac = AuthenticatedContext::extract().await?;
    let encryption_key = ac.encryption_key()?;

    google_oauth_disconnect_service(ac.db(), &ac.auth.user_id, &encryption_key)
        .await
        .into_sfn()
}

/// Get the OAuth connection status for a per-datasource provider.
///
/// Authenticated endpoint — requires a valid session cookie.
/// Mirrors `GET /auth/oauth/{provider}/status` in
/// `apps/server/src/routes/auth_datasource_oauth.rs`.
///
/// - `provider`: provider name, e.g. `"snowflake"`, `"databricks"`,
///   `"bigquery-enterprise"`, `"microsoft-enterprise"`.
/// - `datasource_slug`: slug of the datasource whose credential to check.
///
/// Delegates to `kyomi_auth::datasource_oauth::datasource_oauth_status_service`.
#[server(prefix = "/leptos-api")]
pub async fn get_datasource_oauth_status(
    provider: String,
    datasource_slug: String,
) -> Result<DatasourceOAuthStatus, ServerFnError> {
    use kyomi_auth::datasource_oauth::{datasource_oauth_status_service, OAuthProvider};

    let ac = AuthenticatedContext::extract().await?;
    let encryption_key = ac.encryption_key()?;

    let parsed_provider = OAuthProvider::parse(&provider)
        .ok_or_else(|| ServerFnError::new(format!("Unknown OAuth provider: {provider}")))?;

    datasource_oauth_status_service(
        ac.db(),
        &ac.auth.user_id,
        &ac.ws_id,
        parsed_provider,
        &datasource_slug,
        &encryption_key,
    )
    .await
    .into_sfn()
}

/// Disconnect per-datasource OAuth credentials for the authenticated user.
///
/// Authenticated endpoint — requires a valid session cookie.
/// Mirrors `POST /auth/oauth/{provider}/disconnect` in
/// `apps/server/src/routes/auth_datasource_oauth.rs`.
///
/// - `provider`: provider name, e.g. `"snowflake"`, `"databricks"`,
///   `"bigquery-enterprise"`, `"microsoft-enterprise"`.
/// - `datasource_slug`: slug of the datasource whose credential to remove.
///
/// Returns `already_disconnected: true` if no credential row existed.
///
/// Delegates to `kyomi_auth::datasource_oauth::datasource_oauth_disconnect_service`.
#[server(prefix = "/leptos-api")]
pub async fn disconnect_datasource_oauth(
    provider: String,
    datasource_slug: String,
) -> Result<DatasourceOAuthDisconnectResult, ServerFnError> {
    use kyomi_auth::datasource_oauth::{datasource_oauth_disconnect_service, OAuthProvider};

    let ac = AuthenticatedContext::extract().await?;

    let parsed_provider = OAuthProvider::parse(&provider)
        .ok_or_else(|| ServerFnError::new(format!("Unknown OAuth provider: {provider}")))?;

    datasource_oauth_disconnect_service(
        ac.db(),
        &ac.auth.user_id,
        &ac.ws_id,
        parsed_provider,
        &datasource_slug,
    )
    .await
    .into_sfn()
}
