// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for the onboarding flow.
//!
//! These are public server functions (no `extract_auth()` call) used by the
//! welcome and onboarding pages:
//! - `accept_terms` — validates temp token, creates user/session, sets cookies
//!
//! Mirrors `POST /auth/accept-terms` in
//! `apps/server/src/routes/auth_google_oauth.rs`.

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

/// Accept terms of service, completing the signup or re-acceptance flow.
///
/// Public endpoint — no authentication required. The user is identified
/// by a temporary token stored in Redis during the OAuth callback flow.
///
/// Flow:
/// 1. Try pending signup (new user via Google OAuth):
///    - Create user account (verified)
///    - Store OAuth data
///    - Update terms acceptance
///    - Register `google_oauth` auth method
///    - Create personal workspace
///    - Create authenticated session, set cookies
/// 2. Try pending terms (existing user needing re-acceptance):
///    - Update terms acceptance
///    - Create authenticated session, set cookies
/// 3. If neither found, return error (expired/invalid token)
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
        .clone()
        .ok_or_else(|| ServerFnError::new("Encryption key not available"))?;

    // ── Try pending signup first (new user via Google OAuth) ─────────────
    if let Some(signup_data) =
        kyomi_auth::redis_ops::get_pending_signup(&kv, &temp_token)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get pending signup");
                ServerFnError::new("Internal server error")
            })?
    {
        let email = signup_data["email"]
            .as_str()
            .ok_or_else(|| ServerFnError::new("Missing email in signup data"))?;
        let name = signup_data["name"].as_str().unwrap_or("");

        // Create user (verified = true — OAuth means email is verified by Google)
        let user = kyomi_auth::user_service::create_user(&ctx.db, email, Some(name), true)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to create user");
                ServerFnError::new("Internal server error")
            })?;

        // Admin notification (Slack + email) — fire-and-forget
        let notify_webhook = ctx.config.slack_feedback_webhook_url.clone();
        let notify_support = ctx.config.support_email.clone();
        let notify_email = email.to_string();
        let notify_name = name.to_string();
        let notify_user_id = user.user_id.clone();
        tokio::spawn(async move {
            kyomi_auth::notifications::notify_signup(
                notify_webhook.as_deref(),
                &notify_support,
                &notify_email,
                &notify_name,
                &notify_user_id,
            )
            .await;
        });

        // Store OAuth data
        if let Some(oauth_data_json) = signup_data.get("oauth_data") {
            let oauth = kyomi_auth::google_oauth::OAuthData {
                google_id: oauth_data_json
                    .get("google_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                oauth_provider: oauth_data_json
                    .get("oauth_provider")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                picture: oauth_data_json
                    .get("picture")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                last_oauth_login: Some(chrono::Utc::now().to_rfc3339()),
                ..Default::default()
            };
            let encrypted =
                kyomi_auth::google_oauth::build_oauth_data(&oauth, &encryption_key).map_err(
                    |e| {
                        tracing::error!(error = %e, "Failed to encrypt OAuth data");
                        ServerFnError::new("Internal server error")
                    },
                )?;
            kyomi_auth::user_service::update_user_oauth_data(
                &ctx.db,
                &user.user_id,
                Some(&encrypted),
            )
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to store OAuth data");
                ServerFnError::new("Internal server error")
            })?;
        }

        // Update terms acceptance
        kyomi_auth::user_service::update_terms_acceptance(
            &ctx.db,
            &user.user_id,
            kyomi_core::TERMS_VERSION,
            marketing_consent,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update terms acceptance");
            ServerFnError::new("Internal server error")
        })?;

        // Register google_oauth auth method
        let auth_data = serde_json::json!({
            "linked_at": chrono::Utc::now().to_rfc3339(),
        });
        kyomi_auth::user_service::upsert_auth_method(
            &ctx.db,
            &user.user_id,
            "google_oauth",
            &auth_data,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to register auth method");
            ServerFnError::new("Internal server error")
        })?;

        // Create personal workspace
        kyomi_auth::user_service::create_workspace_for_user(
            &ctx.db,
            &user.user_id,
            Some(name),
            email,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create workspace");
            ServerFnError::new("Internal server error")
        })?;

        // Create authenticated session
        let device = super::auth::extract_device_info(&headers);
        let sess = kyomi_auth::session::create_authenticated_session(
            &ctx.db,
            &kv,
            &ctx.config.jwt_secret,
            &user,
            &device,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create session");
            ServerFnError::new("Internal server error")
        })?;

        // Set HTTPOnly cookies via ResponseOptions
        let response_options = expect_context::<leptos_axum::ResponseOptions>();
        for (name, value) in sess.cookie_headers.iter() {
            if name == axum::http::header::SET_COOKIE {
                response_options.append_header(name.clone(), value.clone());
            }
        }

        return Ok(AcceptTermsResult::Success);
    }

    // ── Try pending terms (existing user) ────────────────────────────────
    if let Some(terms_data) =
        kyomi_auth::redis_ops::get_pending_terms(&kv, &temp_token)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get pending terms");
                ServerFnError::new("Internal server error")
            })?
    {
        let user_id = terms_data["user_id"]
            .as_str()
            .ok_or_else(|| ServerFnError::new("Missing user_id in terms data"))?;

        // Update terms acceptance
        kyomi_auth::user_service::update_terms_acceptance(
            &ctx.db,
            user_id,
            kyomi_core::TERMS_VERSION,
            marketing_consent,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to update terms acceptance");
            ServerFnError::new("Internal server error")
        })?;

        // Get fresh user for session creation
        let user = kyomi_auth::user_service::get_user_by_id(&ctx.db, user_id)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to get user");
                ServerFnError::new("Internal server error")
            })?
            .ok_or_else(|| ServerFnError::new("User not found"))?;

        // Create authenticated session
        let device = super::auth::extract_device_info(&headers);
        let sess = kyomi_auth::session::create_authenticated_session(
            &ctx.db,
            &kv,
            &ctx.config.jwt_secret,
            &user,
            &device,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create session");
            ServerFnError::new("Internal server error")
        })?;

        // Set HTTPOnly cookies via ResponseOptions
        let response_options = expect_context::<leptos_axum::ResponseOptions>();
        for (name, value) in sess.cookie_headers.iter() {
            if name == axum::http::header::SET_COOKIE {
                response_options.append_header(name.clone(), value.clone());
            }
        }

        return Ok(AcceptTermsResult::Success);
    }

    // ── Neither found — token expired or invalid ─────────────────────────
    Ok(AcceptTermsResult::Error {
        message: "Invalid or expired token. Please try signing up again.".to_string(),
    })
}

// ---------------------------------------------------------------------------
// Onboarding state — datasource setup flow
// ---------------------------------------------------------------------------

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

/// Get the combined onboarding state for the current user.
///
/// Combines: list datasources, check credential status, check sample availability,
/// and determine admin status — all in one server call.
///
/// Mirrors the logic in `DatasourceOnboarding.jsx`'s `checkWorkspaceState()`.
#[server(prefix = "/leptos-api")]
pub async fn get_onboarding_state() -> Result<OnboardingState, ServerFnError> {
    use super::{extract_auth, extract_context, workspace_id};

    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Check if user is admin or owner
    let is_admin = auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
        || auth.is_owner;

    // Fetch active datasources
    let datasources =
        kyomi_auth::datasource_service::list_datasources(&ctx.db, ws_id, false)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;

    let has_datasources = !datasources.is_empty();

    if !has_datasources {
        // No datasources — check sample availability for admins.
        // We already know `datasources` is empty, so no sample exists yet.
        let sample_available = if is_admin {
            std::env::var("SAMPLE_CLICKHOUSE_HOST").is_ok()
        } else {
            false
        };

        return Ok(OnboardingState {
            has_datasources: false,
            is_admin,
            sample_available,
            needs_credentials: false,
            total_datasources: 0,
            credential_status: Vec::new(),
        });
    }

    // Datasources exist — check credential status for the current user
    let encryption_key = ctx
        .encryption_key
        .as_deref()
        .ok_or_else(|| ServerFnError::new("Encryption key not configured"))?;

    // Fetch user credentials in bulk
    let user_credentials = kyomi_core::db_fetch_all!(
        &ctx.db,
        kyomi_core::models::datasource::UserDatasourceCredential,
        "SELECT id, user_id, datasource_config_id, workspace_id, credentials, \
         enabled, created_at, updated_at \
         FROM user_datasource_credentials \
         WHERE user_id = $1 AND workspace_id = $2",
        &auth.user_id,
        ws_id
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let creds_by_ds: std::collections::HashMap<&str, &kyomi_core::models::datasource::UserDatasourceCredential> =
        user_credentials
            .iter()
            .map(|c| (c.datasource_config_id.as_str(), c))
            .collect();

    // Fetch user preferences for shared-auth datasources
    let user_preferences = kyomi_core::db_fetch_all!(
        &ctx.db,
        kyomi_core::models::datasource::UserDatasourcePreference,
        "SELECT id, user_id, datasource_config_id, enabled, \
         created_at, updated_at \
         FROM user_datasource_preferences \
         WHERE user_id = $1",
        &auth.user_id
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let prefs_by_ds: std::collections::HashMap<&str, &kyomi_core::models::datasource::UserDatasourcePreference> =
        user_preferences
            .iter()
            .map(|p| (p.datasource_config_id.as_str(), p))
            .collect();

    let mut credential_items = Vec::new();
    let mut needs_credentials = false;

    for ds in &datasources {
        let connection_config = &ds.connection_config;
        let is_connect = ds.connection_type == "connect";

        let result = if is_connect {
            let pref = prefs_by_ds.get(ds.id.as_str()).copied();
            let _enabled = pref.is_none_or(|p| p.enabled);
            kyomi_auth::datasource_auth_service::CredentialStatusResult {
                credential_status: "shared".to_string(),
                auth_method: "connect".to_string(),
                oauth_provider: None,
            }
        } else {
            let user_cred = creds_by_ds.get(ds.id.as_str()).copied();
            kyomi_auth::datasource_auth_service::check_credential_status(
                ds.datasource_type.as_ref(),
                connection_config,
                user_cred,
                encryption_key,
            )
        };

        let needs_action =
            result.credential_status == "missing" || result.credential_status == "expired";
        if needs_action {
            needs_credentials = true;
        }

        // Extract auth_mode from connection_config (non-secret config field).
        // Needed by the frontend to route BigQuery to the correct OAuth endpoint.
        let auth_mode = connection_config
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        credential_items.push(CredentialStatusItem {
            datasource_id: ds.id.clone(),
            datasource_name: ds.name.clone(),
            datasource_type: ds.datasource_type.to_string(),
            slug: ds.slug.clone(),
            status: result.credential_status,
            auth_method: result.auth_method,
            oauth_provider: result.oauth_provider,
            auth_mode,
            needs_action,
        });
    }

    let total_datasources = datasources.len();

    Ok(OnboardingState {
        has_datasources: true,
        is_admin,
        sample_available: false,
        needs_credentials,
        total_datasources,
        credential_status: credential_items,
    })
}

/// Create the sample datasource for the workspace (admin only).
///
/// Mirrors `POST /api/v1/datasources/sample` in
/// `apps/server/src/routes/datasources.rs`.
#[server(prefix = "/leptos-api")]
pub async fn create_sample_datasource() -> Result<(), ServerFnError> {
    use super::{extract_auth, extract_context, workspace_id};

    let auth = extract_auth().await?;
    let ctx = extract_context()?;
    let ws_id = workspace_id(&auth)?;

    // Require admin
    if !auth
        .workspace
        .workspace_roles
        .contains(&kyomi_core::enums::WorkspaceRole::WorkspaceAdmin)
        && !auth.is_owner
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
            .map_err(|e| ServerFnError::new(e.to_string()))?;

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
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    tracing::info!(
        "Created sample datasource '{}' (id: {}) for workspace {} by user {}",
        ds.name,
        ds.id,
        ws_id,
        auth.user_id
    );

    // Trigger sample data indexing in background
    let db = ctx.db.clone();
    let embedding = ctx.embedding.clone();
    tokio::spawn(async move {
        use kyomi_auth::catalog::indexers::SampleDataIndexer;

        tracing::info!("Sample data indexing background task started");
        let count = SampleDataIndexer::get_sample_table_count(&db).await;
        if count == 0 {
            tracing::info!("Sample data index empty — triggering indexing");
            let emb = match embedding.wait_ready().await {
                Ok(e) => e,
                Err(e) => {
                    tracing::error!(error = %e, "Embedding not available for sample data indexing");
                    return;
                }
            };
            let result = SampleDataIndexer::index_sample_data(&db, emb).await;
            tracing::info!(
                status = ?result.status,
                tables = result.tables_indexed,
                "sample data indexing finished"
            );
        } else {
            tracing::info!(count, "sample data already indexed — skipping");
        }
    });

    Ok(())
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
