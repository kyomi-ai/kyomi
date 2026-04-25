// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the onboarding flow.
//!
//! These are thin wrappers around [`kyomi_auth::onboarding_service`] — they
//! extract HTTP context (cookies, headers) and delegate all business logic to
//! the shared service layer so the same orchestration can be used by both the
//! Leptos server_fn path and the REST route handlers.
//!
//! - `accept_terms` — validates temp token, creates user/session, sets cookies
//! - `get_onboarding_state` — queries state needed to render the onboarding page

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use super::extract_context;

/// Result of the terms acceptance flow.
///
/// Uses typed variants so the Leptos UI can pattern-match on outcomes.
/// Cookies are set via `ResponseOptions` for the `Success` variant.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AcceptTermsResult {
    /// Terms accepted, session created, cookies set.
    Success,
    /// Error — display message to the user and allow retry.
    Error { message: String },
}

/// Status of a single datasource's credentials for the current user.
///
/// Used by the onboarding page to show which datasources need credential
/// setup and what action the user should take (OAuth connect, password entry).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CredentialStatusItem {
    pub datasource_id: String,
    pub datasource_name: String,
    pub datasource_type: String,
    pub slug: String,
    /// "valid" | "expired" | "missing" | "shared"
    pub status: String,
    /// "password" | "oauth" | "connect" — determines the UI action button
    pub auth_method: String,
    /// For OAuth providers: "google" | "snowflake" | "microsoft" | "databricks"
    pub oauth_provider: Option<String>,
    /// The `auth_mode` from the datasource connection_config.
    /// For BigQuery: "kyomi_oauth" | "enterprise_oauth" | "service_account"
    /// Needed by the frontend to construct the correct OAuth URL.
    pub auth_mode: Option<String>,
    /// True if the user needs to take action (missing or expired)
    pub needs_action: bool,
}

/// Combined onboarding state fetched in a single server call.
///
/// The onboarding page uses this to decide which of the 5 states to show
/// without making multiple sequential API calls.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OnboardingState {
    pub has_datasources: bool,
    pub is_admin: bool,
    pub sample_available: bool,
    pub needs_credentials: bool,
    pub total_datasources: usize,
    pub credential_status: Vec<CredentialStatusItem>,
}

// ---------------------------------------------------------------------------
// From conversions — service types → server_fn types (ssr only)
// ---------------------------------------------------------------------------

#[cfg(feature = "ssr")]
impl From<kyomi_auth::onboarding_service::CredentialStatusItem> for CredentialStatusItem {
    fn from(s: kyomi_auth::onboarding_service::CredentialStatusItem) -> Self {
        Self {
            datasource_id: s.datasource_id,
            datasource_name: s.datasource_name,
            datasource_type: s.datasource_type,
            slug: s.slug,
            status: s.status,
            auth_method: s.auth_method,
            oauth_provider: s.oauth_provider,
            auth_mode: s.auth_mode,
            needs_action: s.needs_action,
        }
    }
}

#[cfg(feature = "ssr")]
impl From<kyomi_auth::onboarding_service::OnboardingState> for OnboardingState {
    fn from(s: kyomi_auth::onboarding_service::OnboardingState) -> Self {
        Self {
            has_datasources: s.has_datasources,
            is_admin: s.is_admin,
            sample_available: s.sample_available,
            needs_credentials: s.needs_credentials,
            total_datasources: s.total_datasources,
            credential_status: s.credential_status.into_iter().map(Into::into).collect(),
        }
    }
}

/// Accept terms of service, completing the signup or re-acceptance flow.
///
/// Public endpoint — no authentication required. The user is identified
/// by a temporary token stored in Redis during the OAuth callback flow.
/// All business logic is delegated to
/// [`kyomi_auth::onboarding_service::accept_terms`].
///
/// Mirrors `POST /auth/accept-terms` in
/// `apps/server/src/routes/auth_google_oauth.rs`.
#[server(prefix = "/leptos-api")]
pub async fn accept_terms(
    temp_token: String,
    marketing_consent: bool,
) -> Result<AcceptTermsResult, ServerFnError> {
    let ctx = extract_context()?;

    let headers: axum::http::HeaderMap = leptos_axum::extract()
        .await
        .map_err(|e| ServerFnError::new(format!("Failed to extract headers: {e}")))?;

    let kv = ctx
        .kv
        .clone()
        .ok_or_else(|| ServerFnError::new("KV store not available"))?;

    let encryption_key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Encryption key not available"))?;

    let device = super::auth::extract_device_info(&headers);

    let outcome = kyomi_auth::onboarding_service::accept_terms(
        &ctx.db,
        &kv,
        encryption_key,
        &ctx.config,
        &device,
        &temp_token,
        marketing_consent,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "accept_terms orchestration failed");
        ServerFnError::new("Internal server error")
    })?;

    match outcome {
        kyomi_auth::onboarding_service::AcceptTermsOutcome::Success { cookie_headers } => {
            let response_options = expect_context::<leptos_axum::ResponseOptions>();
            for (name, value) in cookie_headers.iter() {
                if name == axum::http::header::SET_COOKIE {
                    response_options.append_header(name.clone(), value.clone());
                }
            }
            Ok(AcceptTermsResult::Success)
        }
        kyomi_auth::onboarding_service::AcceptTermsOutcome::InvalidToken => {
            Ok(AcceptTermsResult::Error {
                message: "Invalid or expired token. Please try signing up again.".to_string(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Onboarding state — datasource setup flow
// ---------------------------------------------------------------------------

/// Get the combined onboarding state for the current user.
///
/// Delegates all business logic to
/// [`kyomi_auth::onboarding_service::get_onboarding_state`].
/// Mirrors the logic in `DatasourceOnboarding.jsx`'s `checkWorkspaceState()`.
#[server(prefix = "/leptos-api")]
pub async fn get_onboarding_state() -> Result<OnboardingState, ServerFnError> {
    use super::{extract_auth, extract_context, workspace_id, IntoServerFnError};

    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let is_admin = auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
        || auth.workspace.is_owner;

    let encryption_key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    kyomi_auth::onboarding_service::get_onboarding_state(&ctx.db, ws_id, &auth.user_id, is_admin, encryption_key)
        .await
        .map(Into::into)
        .into_sfn()
}

/// Create the sample datasource for the workspace (admin only).
///
/// Mirrors `POST /api/v1/datasources/sample` in
/// `apps/server/src/routes/datasources.rs`.
#[server(prefix = "/leptos-api")]
pub async fn create_sample_datasource() -> Result<(), ServerFnError> {
    use super::{extract_auth, extract_context, workspace_id, IntoServerFnError};

    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Require admin
    if !auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
        && !auth.workspace.is_owner
    {
        return Err(ServerFnError::new("Workspace admin access required"));
    }

    // Check if sample ClickHouse is configured
    let ch_config =
        kyomi_auth::catalog::indexers::sample_data::SampleClickHouseConfig::from_env()
            .ok_or_else(|| {
                ServerFnError::new("Sample database is not configured on this server")
            })?;

    // Check if workspace already has a sample datasource
    let datasources =
        kyomi_auth::datasource_service::list_datasources(&ctx.db, ws_id, true)
            .await
            .into_sfn()?;

    let has_sample = datasources.iter().any(|ds| {
        ds.connection_config
            .get("is_sample")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    });

    if has_sample {
        return Err(ServerFnError::new(
            "This workspace already has a sample datasource",
        ));
    }

    let connection_config = ch_config.sample_datasource_config_json();

    let ds = kyomi_auth::datasource_service::create_datasource(
        &ctx.db,
        ws_id,
        "Acme Analytics (Sample)",
        Some("acme-analytics-sample"),
        "clickhouse",
        connection_config,
        None, // direct connection
    )
    .await
    .into_sfn()?;

    tracing::info!(
        "Created sample datasource '{}' (id: {}) for workspace {} by user {}",
        ds.name,
        ds.id,
        ws_id,
        auth.user_id
    );

    // Trigger the generic catalog indexer in the background so the sample
    // tables show up in the user's workspace cache immediately. Previously
    // this used a sample-specific indexer that wrote into a shared sentinel
    // workspace — the generic per-workspace path is simpler, consistent with
    // other datasource types, and unblocks the `list_datasources` tool which
    // reads from the workspace cache.
    if let Some(encryption_key) = ctx.encryption_key.clone() {
        kyomi_agent::catalog::indexing_service::CatalogIndexingService::spawn_post_create(
            ctx.db.clone(),
            encryption_key,
            ctx.embedding.clone(),
            ws_id.to_string(),
            ds.id.clone(),
        );
    } else {
        tracing::warn!(
            workspace_id = %ws_id,
            datasource_id = %ds.id,
            "Encryption key not configured — skipping initial catalog index for sample datasource"
        );
    }

    Ok(())
}

/// Availability state for the sample datasource in the current workspace.
///
/// Mirrors `GET /api/v1/datasources/sample/available` in
/// `apps/server/src/routes/datasources.rs`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SampleDatasourceAvailability {
    /// True if the server has a sample ClickHouse configured (via
    /// `SAMPLE_CLICKHOUSE_HOST` / `SampleClickHouseConfig::from_env`).
    pub configured: bool,
    /// True if the current workspace already has a sample datasource.
    pub already_added: bool,
    /// True if the current user is a workspace admin (or owner).
    pub is_admin: bool,
}

/// Check whether the sample datasource can be added to the current workspace.
///
/// Used by the "Add Datasource" modal to show the sample quick-add tile.
/// Mirrors the React `GET /api/v1/datasources/sample/available` call.
#[server(prefix = "/leptos-api")]
pub async fn check_sample_datasource_available()
-> Result<SampleDatasourceAvailability, ServerFnError> {
    use super::{extract_auth, extract_context, workspace_id, IntoServerFnError};

    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    let is_admin = auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
        || auth.workspace.is_owner;

    let configured =
        kyomi_auth::catalog::indexers::sample_data::SampleClickHouseConfig::from_env()
            .is_some();

    let datasources =
        kyomi_auth::datasource_service::list_datasources(&ctx.db, ws_id, true)
            .await
            .into_sfn()?;

    let already_added = datasources.iter().any(|ds| {
        ds.connection_config
            .get("is_sample")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    });

    Ok(SampleDatasourceAvailability {
        configured,
        already_added,
        is_admin,
    })
}

/// Return the OAuth connect URL for a given datasource type and auth mode.
///
/// The URL is constructed server-side so that the frontend does not need to
/// know about per-provider path conventions. The frontend opens this URL
/// in a centered popup window.
#[server(prefix = "/leptos-api")]
pub async fn get_oauth_connect_url(
    datasource_type: String,
    auth_mode: String,
    datasource_slug: Option<String>,
) -> Result<String, ServerFnError> {
    // No auth extraction needed — the OAuth endpoint itself validates the session
    // via cookies. This function merely returns the URL to open in a popup.

    // Return relative URLs — the popup opens on the same origin, so absolute
    // URLs are unnecessary and would break if frontend_url differs from the
    // actual browsing origin (e.g. localhost vs dev.kyomi.ai).
    //
    // Datasource slugs are URL-safe by construction (alphanumeric + hyphens),
    // so no percent-encoding is needed for the query parameter.
    let url = match (datasource_type.as_str(), auth_mode.as_str()) {
        ("bigquery", "kyomi_oauth") | ("bigquery", "") => {
            "/api/v1/auth/google-oauth/connect".to_string()
        }
        ("bigquery", "enterprise_oauth") => {
            let slug = datasource_slug
                .ok_or_else(|| ServerFnError::new("datasource_slug required for enterprise OAuth"))?;
            format!(
                "/api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug={slug}"
            )
        }
        ("snowflake", _) => {
            let slug = datasource_slug
                .ok_or_else(|| ServerFnError::new("datasource_slug required for Snowflake OAuth"))?;
            format!(
                "/api/v1/auth/oauth/snowflake/connect?datasource_slug={slug}"
            )
        }
        ("synapse", _) => {
            let slug = datasource_slug
                .ok_or_else(|| ServerFnError::new("datasource_slug required for Synapse OAuth"))?;
            format!(
                "/api/v1/auth/oauth/microsoft-enterprise/connect?datasource_slug={slug}"
            )
        }
        ("databricks", _) => {
            let slug = datasource_slug
                .ok_or_else(|| ServerFnError::new("datasource_slug required for Databricks OAuth"))?;
            format!(
                "/api/v1/auth/oauth/databricks/connect?datasource_slug={slug}"
            )
        }
        _ => {
            return Err(ServerFnError::new(format!(
                "OAuth not supported for datasource type '{}' with auth mode '{}'",
                datasource_type, auth_mode
            )));
        }
    };

    Ok(url)
}
