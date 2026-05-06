// SPDX-License-Identifier: AGPL-3.0-or-later

//! Onboarding service — business logic for the user onboarding flow.
//!
//! Contains two orchestration functions extracted from the Leptos server_fns
//! layer so that both the server_fn path and future REST routes share the
//! same implementation without duplication.
//!
//! - [`accept_terms`] — validates a temp token (pending signup or pending
//!   terms), creates user accounts / workspace / sessions as needed, and
//!   returns the cookies that should be set on the HTTP response.
//! - [`get_onboarding_state`] — queries multiple tables to determine which
//!   onboarding step the user is at, returning a [`OnboardingState`] value.

use axum::http::HeaderMap;
use kyomi_core::models::datasource::{DatasourceConfig, UserDatasourceCredential, UserDatasourcePreference};
use kyomi_core::{Config, DbPool, KVPool};

use crate::{datasource_auth_service, datasource_service, google_oauth, notifications, redis_ops, session, user_service};

// ---------------------------------------------------------------------------
// Public data types
// ---------------------------------------------------------------------------
//
// `CredentialStatusItem` and `OnboardingState` are defined in `kyomi_types`
// so they can be shared between this service and the Leptos server_fn layer
// without a conversion step. Re-exported here for backwards compatibility.
pub use kyomi_types::{CredentialStatusItem, OnboardingState};

/// Outcome of the [`accept_terms`] flow.
///
/// On success the caller must set `cookie_headers` on the HTTP response.
pub enum AcceptTermsOutcome {
    /// Terms accepted and session created. The caller must forward the
    /// included `Set-Cookie` headers to the HTTP response.
    Success { cookie_headers: HeaderMap },
    /// The token was not found — expired or invalid.
    InvalidToken,
}

// ---------------------------------------------------------------------------
// accept_terms
// ---------------------------------------------------------------------------

/// Accept terms of service, completing the signup or re-acceptance flow.
///
/// Orchestrates the full accept-terms workflow:
///
/// 1. Try **pending signup** (new user via Google OAuth):
///    - Create user account (verified, email confirmed by Google)
///    - Store OAuth credential data (encrypted)
///    - Mark terms acceptance
///    - Register `google_oauth` auth method
///    - Create personal workspace
///    - Create authenticated session
///    - Fire-and-forget admin signup notification
/// 2. Try **pending terms** (existing user needing re-acceptance):
///    - Mark terms acceptance
///    - Fetch user record
///    - Create authenticated session
/// 3. If neither token exists → [`AcceptTermsOutcome::InvalidToken`]
///
/// The caller is responsible for forwarding the `Set-Cookie` headers
/// included in [`AcceptTermsOutcome::Success`] to the HTTP response.
pub async fn accept_terms(
    pool: &DbPool,
    kv: &KVPool,
    encryption_key: &[u8; 32],
    config: &Config,
    device_info: &crate::token_service::DeviceInfo,
    temp_token: &str,
    marketing_consent: bool,
) -> kyomi_core::Result<AcceptTermsOutcome> {
    // ── Try pending signup first (new user via Google OAuth) ─────────────
    if let Some(signup_data) = redis_ops::get_pending_signup(kv, temp_token).await? {
        let email = signup_data["email"]
            .as_str()
            .ok_or_else(|| kyomi_core::Error::Internal("Missing email in signup data".into()))?;
        let name = signup_data["name"].as_str().unwrap_or("");

        // Create user (verified = true — OAuth means email is verified by Google)
        let user = user_service::create_user(pool, email, Some(name), true).await?;

        // Admin notification (Slack + email) — fire-and-forget
        let notify_webhook = config.slack_feedback_webhook_url.clone();
        let notify_support = config.support_email.clone();
        let notify_email = email.to_string();
        let notify_name = name.to_string();
        let notify_user_id = user.user_id.clone();
        tokio::spawn(async move {
            notifications::notify_signup(
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
            let oauth = google_oauth::OAuthData {
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
            let encrypted = google_oauth::build_oauth_data(&oauth, encryption_key)?;
            user_service::update_user_oauth_data(pool, &user.user_id, Some(&encrypted)).await?;
        }

        // Update terms acceptance
        user_service::update_terms_acceptance(
            pool,
            &user.user_id,
            kyomi_core::TERMS_VERSION,
            marketing_consent,
        )
        .await?;

        // Register google_oauth auth method
        let auth_data = serde_json::json!({
            "linked_at": chrono::Utc::now().to_rfc3339(),
        });
        user_service::upsert_auth_method(pool, &user.user_id, "google_oauth", &auth_data).await?;

        // Create personal workspace
        user_service::create_workspace_for_user(pool, &user.user_id, Some(name), email, Some(config))
            .await?;

        // Create authenticated session
        let sess =
            session::create_authenticated_session(pool, kv, &config.jwt_secret, &user, device_info)
                .await?;

        return Ok(AcceptTermsOutcome::Success {
            cookie_headers: sess.cookie_headers,
        });
    }

    // ── Try pending terms (existing user) ────────────────────────────────
    if let Some(terms_data) = redis_ops::get_pending_terms(kv, temp_token).await? {
        let user_id = terms_data["user_id"]
            .as_str()
            .ok_or_else(|| kyomi_core::Error::Internal("Missing user_id in terms data".into()))?;

        // Update terms acceptance
        user_service::update_terms_acceptance(
            pool,
            user_id,
            kyomi_core::TERMS_VERSION,
            marketing_consent,
        )
        .await?;

        // Get fresh user for session creation
        let user = user_service::get_user_by_id(pool, user_id)
            .await?
            .ok_or_else(|| kyomi_core::Error::Internal("User not found".into()))?;

        // Create authenticated session
        let sess =
            session::create_authenticated_session(pool, kv, &config.jwt_secret, &user, device_info)
                .await?;

        return Ok(AcceptTermsOutcome::Success {
            cookie_headers: sess.cookie_headers,
        });
    }

    // ── Neither found — token expired or invalid ─────────────────────────
    Ok(AcceptTermsOutcome::InvalidToken)
}

// ---------------------------------------------------------------------------
// get_onboarding_state
// ---------------------------------------------------------------------------

/// Get the combined onboarding state for a workspace user.
///
/// Combines:
/// - List active datasources for the workspace
/// - For each datasource, check the user's credential status
/// - Determine if the sample ClickHouse is available (admins only)
///
/// All in one database round-trip per call (plus one credentials bulk-fetch).
pub async fn get_onboarding_state(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
    is_admin: bool,
    encryption_key: &[u8; 32],
) -> kyomi_core::Result<OnboardingState> {
    // Fetch active datasources
    let datasources = datasource_service::list_datasources(pool, workspace_id, false).await?;
    let has_datasources = !datasources.is_empty();

    if !has_datasources {
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
    let user_credentials = kyomi_core::db_fetch_all!(
        pool,
        UserDatasourceCredential,
        "SELECT id, user_id, datasource_config_id, workspace_id, credentials, \
         enabled, created_at, updated_at \
         FROM user_datasource_credentials \
         WHERE user_id = $1 AND workspace_id = $2",
        user_id,
        workspace_id
    )?;

    let creds_by_ds: std::collections::HashMap<&str, &UserDatasourceCredential> =
        user_credentials
            .iter()
            .map(|c| (c.datasource_config_id.as_str(), c))
            .collect();

    let user_preferences = kyomi_core::db_fetch_all!(
        pool,
        UserDatasourcePreference,
        "SELECT id, user_id, datasource_config_id, enabled, \
         created_at, updated_at \
         FROM user_datasource_preferences \
         WHERE user_id = $1",
        user_id
    )?;

    let prefs_by_ds: std::collections::HashMap<&str, &UserDatasourcePreference> =
        user_preferences
            .iter()
            .map(|p| (p.datasource_config_id.as_str(), p))
            .collect();

    let credential_status =
        build_credential_status(&datasources, &creds_by_ds, &prefs_by_ds, encryption_key);

    let needs_credentials = credential_status.iter().any(|item| item.needs_action);
    let total_datasources = datasources.len();

    Ok(OnboardingState {
        has_datasources: true,
        is_admin,
        sample_available: false,
        needs_credentials,
        total_datasources,
        credential_status,
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn build_credential_status(
    datasources: &[DatasourceConfig],
    creds_by_ds: &std::collections::HashMap<&str, &UserDatasourceCredential>,
    prefs_by_ds: &std::collections::HashMap<&str, &UserDatasourcePreference>,
    encryption_key: &[u8; 32],
) -> Vec<CredentialStatusItem> {
    datasources
        .iter()
        .map(|ds| {
            let connection_config = &ds.connection_config;
            let is_connect = ds.connection_type == "connect";

            let result = if is_connect {
                let _pref = prefs_by_ds.get(ds.id.as_str()).copied();
                datasource_auth_service::CredentialStatusResult {
                    credential_status: "shared".to_string(),
                    auth_method: "connect".to_string(),
                    oauth_provider: None,
                }
            } else {
                let user_cred = creds_by_ds.get(ds.id.as_str()).copied();
                datasource_auth_service::check_credential_status(
                    ds.datasource_type.as_ref(),
                    connection_config,
                    user_cred,
                    encryption_key,
                )
            };

            let needs_action =
                result.credential_status == "missing" || result.credential_status == "expired";

            let auth_mode = connection_config
                .get("auth_mode")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            CredentialStatusItem {
                datasource_id: ds.id.clone(),
                datasource_name: ds.name.clone(),
                datasource_type: ds.datasource_type.to_string(),
                slug: ds.slug.clone(),
                status: result.credential_status,
                auth_method: result.auth_method,
                oauth_provider: result.oauth_provider,
                auth_mode,
                needs_action,
            }
        })
        .collect()
}
