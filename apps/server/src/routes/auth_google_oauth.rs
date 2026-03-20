// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google OAuth endpoints.
//!
//! Wire-compatible with Python's `routers/auth_google_oauth.py`.
//!
//! Phase 3B: Login flow (3 endpoints)
//!   - GET  /auth/google/login     — Generate authorization URL
//!   - POST /auth/google/callback  — Exchange code, detect new/existing user
//!   - POST /auth/accept-terms     — Accept terms, complete signup
//!
//! Phase 3C: BigQuery flow (5 endpoints) — added later

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use kyomi_auth::{
    google_oauth::{self, OAuthData, GoogleOAuthTokens},
    middleware::AuthUser,
    rate_limiter,
    redis_ops,
    session,
    token_service::DeviceInfo,
    user_service,
    websocket::helpers as ws_helpers,
};

use crate::state::AppState;

/// Build the Google OAuth router.
///
/// Mounted at `/auth` — these paths are relative:
///   - `/google/login`
///   - `/google/callback`
///   - `/accept-terms`
///   - `/google-oauth/connect`     (Phase 3C)
///   - `/google/link-callback`     (Phase 3C)
///   - `/google-oauth/disconnect`  (Phase 3C)
///   - `/google-oauth/status`      (Phase 3C)
///   - `/google-oauth/projects`    (Phase 3C)
pub fn routes() -> Router<AppState> {
    Router::new()
        // Phase 3B: Login flow
        .route("/google/login", get(google_login))
        .route("/google/callback", post(google_callback))
        .route("/accept-terms", post(accept_terms))
        // Phase 3C: BigQuery connect + management
        .route("/google-oauth/connect", get(google_oauth_connect))
        .route("/google/link-callback", post(google_link_callback))
        .route("/google-oauth/disconnect", post(google_oauth_disconnect))
        .route("/google-oauth/status", get(google_oauth_status))
        .route("/google-oauth/projects", get(google_oauth_projects))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_device_info(headers: &HeaderMap) -> DeviceInfo {
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let ip_address = extract_client_ip(headers);

    let country_code = headers
        .get("cf-ipcountry")
        .and_then(|v| v.to_str().ok())
        .filter(|s| *s != "XX")
        .map(|s| s.to_uppercase());

    DeviceInfo {
        user_agent,
        ip_address: Some(ip_address),
        country_code,
        oauth_client_id: None,
    }
}

fn extract_client_ip(headers: &HeaderMap) -> String {
    crate::helpers::extract_client_ip(headers, None)
}

use kyomi_core::TERMS_VERSION;

// ---------------------------------------------------------------------------
// Request/Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GoogleLoginQuery {
    oauth_continue: Option<String>,
}

#[derive(Deserialize)]
struct GoogleCallbackRequest {
    code: String,
    state: Option<String>,
}

#[derive(Deserialize)]
struct AcceptTermsRequest {
    temp_token: String,
    accepted: bool,
    #[serde(default)]
    marketing_consent: bool,
}

#[derive(Deserialize)]
struct LinkCallbackRequest {
    code: String,
    state: String,
}

// ---------------------------------------------------------------------------
// GET /auth/google/login
// ---------------------------------------------------------------------------

async fn google_login(
    State(state): State<AppState>,
    Query(query): Query<GoogleLoginQuery>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let (client_id, client_secret) = get_oauth_credentials(&state)?;
    let _ = client_secret; // Not needed for auth URL

    // Generate CSRF state token
    let csrf_state = redis_ops::generate_token();

    // Build redirect URI
    let redirect_uri = format!(
        "{}/auth/google/callback",
        state.config.frontend_url.trim_end_matches('/')
    );

    // Build authorization URL (login scopes, no offline access)
    let authorization_url = google_oauth::build_authorization_url(
        &client_id,
        &redirect_uri,
        &csrf_state,
        google_oauth::LOGIN_SCOPES,
        false, // don't force consent for login
        false, // no offline access
    );

    // Store state in Redis
    let mut state_data = serde_json::json!({
        "created_at": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(ref oauth_continue) = query.oauth_continue {
        state_data["oauth_continue"] = serde_json::json!(oauth_continue);
    }
    redis_ops::store_oauth_state(&state.kv, "google", &csrf_state, &state_data).await?;

    Ok(Json(serde_json::json!({
        "authorization_url": authorization_url,
        "state": csrf_state,
    })))
}

// ---------------------------------------------------------------------------
// POST /auth/google/callback
// ---------------------------------------------------------------------------

async fn google_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<GoogleCallbackRequest>,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    let (client_id, client_secret) = get_oauth_credentials(&state)?;

    // Rate limit
    let ip = extract_client_ip(&headers);
    let rate_result = rate_limiter::check_rate_limit(&state.kv, &ip, "login", None).await?;
    if !rate_result.allowed {
        tracing::warn!(ip = %ip, "Google OAuth callback rate limited");
        return Err(kyomi_core::Error::TooManyRequests(
            format!("Rate limited. Try again in {} seconds", rate_result.retry_after_secs),
            rate_result.retry_after_secs,
        ));
    }

    // Verify CSRF state (optional — frontend may not send it)
    let mut oauth_continue = None;
    if let Some(ref csrf_state) = data.state {
        let state_data = redis_ops::verify_oauth_state(&state.kv, "google", csrf_state).await?;
        if let Some(state_data) = state_data {
            oauth_continue = state_data
                .get("oauth_continue")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }

    // Exchange code for tokens
    let redirect_uri = format!(
        "{}/auth/google/callback",
        state.config.frontend_url.trim_end_matches('/')
    );
    let token_data =
        google_oauth::exchange_code_for_tokens(&client_id, &client_secret, &data.code, &redirect_uri)
            .await?;

    // Get user info from Google
    let user_info = google_oauth::get_user_info(&token_data.access_token).await?;
    let email = user_info.email.to_lowercase();

    // Look up existing user
    let existing_user = user_service::get_user_by_email(&state.db, &email).await?;

    match existing_user {
        None => {
            // NEW USER — store pending signup, return temp token
            let temp_token = redis_ops::generate_token();
            let signup_data = serde_json::json!({
                "email": email,
                "name": user_info.name.unwrap_or_default(),
                "oauth_data": {
                    "google_id": user_info.id,
                    "oauth_provider": "google",
                    "picture": user_info.picture,
                }
            });
            redis_ops::store_pending_signup(&state.kv, &temp_token, &signup_data).await?;

            let redirect_url = format!(
                "{}/welcome?temp_token={temp_token}",
                state.config.frontend_url.trim_end_matches('/')
            );

            Ok((
                HeaderMap::new(),
                Json(serde_json::json!({
                    "status": "pending_terms",
                    "temp_token": temp_token,
                    "redirect_url": redirect_url,
                    "message": "Please accept terms to complete signup",
                })),
            ))
        }
        Some(user) if user.terms_accepted_at.is_none() => {
            // EXISTING USER — needs terms acceptance
            let temp_token = redis_ops::generate_token();
            let terms_data = serde_json::json!({
                "user_id": user.user_id,
                "email": email,
            });
            redis_ops::store_pending_terms(&state.kv, &temp_token, &terms_data).await?;

            let redirect_url = format!(
                "{}/welcome?temp_token={temp_token}&existing_user=true",
                state.config.frontend_url.trim_end_matches('/')
            );

            Ok((
                HeaderMap::new(),
                Json(serde_json::json!({
                    "status": "pending_terms",
                    "temp_token": temp_token,
                    "redirect_url": redirect_url,
                    "message": "Please accept updated terms",
                })),
            ))
        }
        Some(user) => {
            // EXISTING USER — terms accepted, normal login

            // Ensure google_oauth auth method exists
            let auth_method = user_service::get_auth_method(&state.db, &user.user_id, "google_oauth").await?;
            if auth_method.is_none() {
                let auth_data = serde_json::json!({
                    "linked_at": chrono::Utc::now().to_rfc3339(),
                });
                user_service::upsert_auth_method(&state.db, &user.user_id, "google_oauth", &auth_data)
                    .await?;
            }

            // Ensure user has a workspace
            let ws_ctx = user_service::get_user_workspace_context(&state.db, &user.user_id).await?;
            if ws_ctx.is_none() {
                user_service::create_workspace_for_user(
                    &state.db,
                    &user.user_id,
                    user.name.as_deref(),
                    &email,
                )
                .await?;
            }

            // Update profile in oauth_data (NOT tokens — login doesn't store tokens)
            let existing_oauth = google_oauth::parse_oauth_data(
                user.oauth_data.as_deref(),
                &state.encryption_key,
            )?;

            let updated_oauth = OAuthData {
                google_id: Some(user_info.id),
                oauth_provider: Some("google".to_string()),
                picture: user_info.picture,
                last_oauth_login: Some(chrono::Utc::now().to_rfc3339()),
                // Preserve existing BigQuery tokens
                google_oauth_tokens: existing_oauth.and_then(|o| o.google_oauth_tokens),
                ..Default::default()
            };

            let encrypted =
                google_oauth::build_oauth_data(&updated_oauth, &state.encryption_key)?;
            user_service::update_user_oauth_data(&state.db, &user.user_id, Some(&encrypted))
                .await?;

            // Create authenticated session
            let device = extract_device_info(&headers);
            let sess = session::create_authenticated_session(
                &state.db,
                &state.kv,
                &state.config.jwt_secret,
                &user,
                &device,
            )
            .await?;

            let mut body = serde_json::json!({
                "message": "Successfully logged in with Google",
                "user": {
                    "user_id": sess.user.user_id,
                    "email": sess.user.email,
                    "name": sess.user.name,
                    "roles": sess.user.roles(),
                },
                "access_token": sess.access_token,
                "refresh_token": sess.refresh_token,
            });

            if let Some(ref oc) = oauth_continue {
                body["oauth_continue"] = serde_json::json!(oc);
            }

            Ok((sess.cookie_headers, Json(body)))
        }
    }
}

// ---------------------------------------------------------------------------
// POST /auth/accept-terms
// ---------------------------------------------------------------------------

async fn accept_terms(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<AcceptTermsRequest>,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    if !data.accepted {
        return Err(kyomi_core::Error::BadRequest(
            "You must accept the terms of service to continue".into(),
        ));
    }

    // Try pending signup first (new user)
    if let Some(signup_data) = redis_ops::get_pending_signup(&state.kv, &data.temp_token).await?
    {
        let email = signup_data["email"]
            .as_str()
            .ok_or_else(|| kyomi_core::Error::Internal("Missing email in signup data".into()))?;
        let name = signup_data["name"].as_str().unwrap_or("");

        // Create user (verified = true — OAuth means email is verified by Google)
        let user = user_service::create_user(&state.db, email, Some(name), true).await?;

        // Notify admin (Slack + email) — fire-and-forget
        let notify_state = state.clone();
        let notify_email = email.to_string();
        let notify_name = name.to_string();
        let notify_user_id = user.user_id.clone();
        tokio::spawn(async move {
            crate::routes::admin_notify::notify_signup(
                &notify_state,
                &notify_email,
                &notify_name,
                &notify_user_id,
            )
            .await;
        });

        // Store OAuth data
        if let Some(oauth_data_json) = signup_data.get("oauth_data") {
            let oauth = OAuthData {
                google_id: oauth_data_json.get("google_id").and_then(|v| v.as_str()).map(|s| s.to_string()),
                oauth_provider: oauth_data_json.get("oauth_provider").and_then(|v| v.as_str()).map(|s| s.to_string()),
                picture: oauth_data_json.get("picture").and_then(|v| v.as_str()).map(|s| s.to_string()),
                last_oauth_login: Some(chrono::Utc::now().to_rfc3339()),
                ..Default::default()
            };
            let encrypted = google_oauth::build_oauth_data(&oauth, &state.encryption_key)?;
            user_service::update_user_oauth_data(&state.db, &user.user_id, Some(&encrypted))
                .await?;
        }

        // Update terms
        user_service::update_terms_acceptance(
            &state.db,
            &user.user_id,
            TERMS_VERSION,
            data.marketing_consent,
        )
        .await?;

        // Register google_oauth auth method
        let auth_data = serde_json::json!({
            "linked_at": chrono::Utc::now().to_rfc3339(),
        });
        user_service::upsert_auth_method(&state.db, &user.user_id, "google_oauth", &auth_data)
            .await?;

        // Create personal workspace
        user_service::create_workspace_for_user(
            &state.db,
            &user.user_id,
            Some(name),
            email,
        )
        .await?;

        // Create authenticated session
        let device = extract_device_info(&headers);
        let sess = session::create_authenticated_session(
            &state.db,
            &state.kv,
            &state.config.jwt_secret,
            &user,
            &device,
        )
        .await?;

        let body = serde_json::json!({
            "message": "Account created successfully",
            "user": {
                "user_id": sess.user.user_id,
                "email": sess.user.email,
                "name": sess.user.name,
                "roles": sess.user.roles(),
            },
            "access_token": sess.access_token,
            "refresh_token": sess.refresh_token,
        });

        return Ok((sess.cookie_headers, Json(body)));
    }

    // Try pending terms (existing user)
    if let Some(terms_data) = redis_ops::get_pending_terms(&state.kv, &data.temp_token).await? {
        let user_id = terms_data["user_id"]
            .as_str()
            .ok_or_else(|| kyomi_core::Error::Internal("Missing user_id in terms data".into()))?;

        // Update terms
        user_service::update_terms_acceptance(
            &state.db,
            user_id,
            TERMS_VERSION,
            data.marketing_consent,
        )
        .await?;

        // Get fresh user
        let user = user_service::get_user_by_id(&state.db, user_id)
            .await?
            .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))?;

        // Create authenticated session
        let device = extract_device_info(&headers);
        let sess = session::create_authenticated_session(
            &state.db,
            &state.kv,
            &state.config.jwt_secret,
            &user,
            &device,
        )
        .await?;

        let body = serde_json::json!({
            "message": "Terms accepted successfully",
            "user": {
                "user_id": sess.user.user_id,
                "email": sess.user.email,
                "name": sess.user.name,
                "roles": sess.user.roles(),
            },
            "access_token": sess.access_token,
            "refresh_token": sess.refresh_token,
        });

        return Ok((sess.cookie_headers, Json(body)));
    }

    // Neither found
    Err(kyomi_core::Error::BadRequest(
        "Invalid or expired terms acceptance token".into(),
    ))
}

// ---------------------------------------------------------------------------
// GET /auth/google-oauth/connect (authenticated — BigQuery linking)
// ---------------------------------------------------------------------------

async fn google_oauth_connect(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    let (client_id, _) = get_oauth_credentials(&state)?;

    let csrf_state = redis_ops::generate_token();

    let state_data = serde_json::json!({
        "user_id": user.user_id,
        "action": "link_account",
        "workspace_id": user.workspace.workspace_id,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    redis_ops::store_oauth_state(&state.kv, "google_link", &csrf_state, &state_data).await?;

    let redirect_uri = format!(
        "{}/auth/google/link-callback",
        state.config.frontend_url.trim_end_matches('/')
    );

    let authorization_url = google_oauth::build_authorization_url(
        &client_id,
        &redirect_uri,
        &csrf_state,
        google_oauth::BIGQUERY_SCOPES,
        true,  // force consent to get refresh token
        true,  // offline access for refresh token
    );

    tracing::info!(
        user_email = %user.email,
        authorization_url = %authorization_url,
        "Starting Google account linking (BigQuery OAuth) — redirecting to Google"
    );

    // 302 Found — matches Python's RedirectResponse(status_code=302)
    Ok((axum::http::StatusCode::FOUND, [(axum::http::header::LOCATION, authorization_url)]))
}

// ---------------------------------------------------------------------------
// POST /auth/google/link-callback
// ---------------------------------------------------------------------------

async fn google_link_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<LinkCallbackRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let (client_id, client_secret) = get_oauth_credentials(&state)?;

    // Rate limit
    let ip = extract_client_ip(&headers);
    let rate_result = rate_limiter::check_rate_limit(&state.kv, &ip, "login", None).await?;
    if !rate_result.allowed {
        tracing::warn!(ip = %ip, "Google link-callback rate limited");
        return Err(kyomi_core::Error::TooManyRequests(
            format!("Rate limited. Try again in {} seconds", rate_result.retry_after_secs),
            rate_result.retry_after_secs,
        ));
    }

    // Verify CSRF state
    let state_data = redis_ops::verify_oauth_state(&state.kv, "google_link", &data.state)
        .await?
        .ok_or_else(|| {
            tracing::warn!(ip = %ip, "Google link-callback: invalid or expired CSRF state");
            kyomi_core::Error::BadRequest("Invalid or expired state".into())
        })?;

    let action = state_data["action"].as_str().unwrap_or("");
    if action != "link_account" {
        return Err(kyomi_core::Error::BadRequest("Invalid state action".into()));
    }

    let link_user_id = state_data["user_id"]
        .as_str()
        .ok_or_else(|| kyomi_core::Error::BadRequest("Missing user_id in state".into()))?;

    // Exchange code for tokens
    let redirect_uri = format!(
        "{}/auth/google/link-callback",
        state.config.frontend_url.trim_end_matches('/')
    );
    let token_data =
        google_oauth::exchange_code_for_tokens(&client_id, &client_secret, &data.code, &redirect_uri)
            .await?;

    tracing::info!(
        scope = ?token_data.scope,
        has_refresh_token = token_data.refresh_token.is_some(),
        expires_in = ?token_data.expires_in,
        "Google link-callback: token exchange response — ACTUAL scopes Google granted"
    );

    // Get user info from Google
    let user_info = google_oauth::get_user_info(&token_data.access_token).await?;

    // Find the user
    let user = user_service::get_user_by_id(&state.db, link_user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))?;

    // Check if Google account is already linked to another user
    let existing_oauth = google_oauth::parse_oauth_data(
        user.oauth_data.as_deref(),
        &state.encryption_key,
    )?;

    // Preserve existing refresh token if Google doesn't return a new one
    let existing_refresh = existing_oauth
        .as_ref()
        .and_then(|o| o.google_oauth_tokens.as_ref())
        .and_then(|t| t.refresh_token.clone());

    let new_refresh_token = token_data.refresh_token.or(existing_refresh);

    let expires_in = token_data.expires_in.unwrap_or(3600);
    let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(expires_in)).to_rfc3339();

    let google_email = user_info.email.clone();

    // Build updated oauth data WITH tokens (this is the BigQuery connect flow)
    let updated_oauth = OAuthData {
        google_id: Some(user_info.id),
        oauth_provider: Some("google".to_string()),
        picture: user_info.picture,
        last_oauth_login: Some(chrono::Utc::now().to_rfc3339()),
        google_oauth_tokens: Some(GoogleOAuthTokens {
            access_token: token_data.access_token,
            refresh_token: new_refresh_token,
            token_type: "Bearer".to_string(),
            scope: token_data.scope.unwrap_or_default(),
            expires_in: Some(expires_in),
            expires_at: Some(expires_at),
            email: Some(google_email.clone()),
            name: user_info.name,
        }),
        ..Default::default()
    };

    let encrypted = google_oauth::build_oauth_data(&updated_oauth, &state.encryption_key)?;
    user_service::update_user_oauth_data(&state.db, &user.user_id, Some(&encrypted)).await?;

    // Send credential_status_changed WebSocket notification (for BigQuery kyomi_oauth mode)
    if let Some(workspace_id) = state_data.get("workspace_id").and_then(|v| v.as_str()) {
        let ws_manager = state.ws_manager.clone();
        let ws_user_id = link_user_id.to_string();
        let ws_workspace_id = workspace_id.to_string();
        tokio::spawn(async move {
            ws_helpers::send_credential_status_changed(
                &ws_manager,
                &ws_user_id,
                &ws_workspace_id,
                // Global OAuth — no specific datasource slug; affects all BigQuery datasources
                "",
                "connected",
                "bigquery",
            )
            .await;
        });
    }

    // Determine BigQuery access level
    let bq_access = updated_oauth
        .google_oauth_tokens
        .as_ref()
        .map(|t| google_oauth::bigquery_access_level(&t.scope))
        .unwrap_or("none");

    Ok(Json(serde_json::json!({
        "message": "Google account successfully linked",
        "google_email": google_email,
        "bigquery_access": bq_access,
        "linked_at": chrono::Utc::now().to_rfc3339(),
    })))
}

// ---------------------------------------------------------------------------
// POST /auth/google-oauth/disconnect
// ---------------------------------------------------------------------------

async fn google_oauth_disconnect(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let db_user = user_service::get_user_by_id(&state.db, &user.user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))?;

    let existing_oauth = google_oauth::parse_oauth_data(
        db_user.oauth_data.as_deref(),
        &state.encryption_key,
    )?;

    // Check if connected
    let has_tokens = existing_oauth
        .as_ref()
        .and_then(|o| o.google_oauth_tokens.as_ref())
        .is_some();

    if !has_tokens {
        return Ok(Json(serde_json::json!({
            "already_disconnected": true,
        })));
    }

    let disconnected_email = existing_oauth
        .as_ref()
        .and_then(|o| o.google_oauth_tokens.as_ref())
        .and_then(|t| t.email.clone())
        .unwrap_or_default();

    // Clear oauth data (keep picture if available)
    let cleared_oauth = OAuthData {
        picture: existing_oauth.and_then(|o| o.picture),
        ..Default::default()
    };

    let encrypted = google_oauth::build_oauth_data(&cleared_oauth, &state.encryption_key)?;
    user_service::update_user_oauth_data(&state.db, &user.user_id, Some(&encrypted)).await?;

    // Remove auth method
    user_service::remove_auth_method(&state.db, &user.user_id, "google_oauth").await?;

    Ok(Json(serde_json::json!({
        "message": "Google account disconnected successfully",
        "disconnected_account": disconnected_email,
        "bigquery_access": "disabled",
        "disconnected_at": chrono::Utc::now().to_rfc3339(),
    })))
}

// ---------------------------------------------------------------------------
// GET /auth/google-oauth/status
// ---------------------------------------------------------------------------

async fn google_oauth_status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let db_user = user_service::get_user_by_id(&state.db, &user.user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))?;

    let oauth_data = google_oauth::parse_oauth_data(
        db_user.oauth_data.as_deref(),
        &state.encryption_key,
    )?;

    let tokens = oauth_data
        .as_ref()
        .and_then(|o| o.google_oauth_tokens.as_ref());

    match tokens {
        None => {
            Ok(Json(serde_json::json!({
                "connected": false,
                "needs_bigquery_connect": true,
                "connect_url": "/api/v1/auth/google-oauth/connect",
                "disconnect_url": "/api/v1/auth/google-oauth/disconnect",
            })))
        }
        Some(t) => {
            let scope_str = &t.scope;
            let has_bq_scopes = google_oauth::has_bigquery_scopes(scope_str);
            let bq_access = google_oauth::bigquery_access_level(scope_str);

            // Check token expiry
            let expires_at = t.expires_at.as_deref();
            let has_refresh = t.refresh_token.is_some();

            let parsed_expiry = expires_at
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|exp| exp.with_timezone(&chrono::Utc));

            let token_expired = parsed_expiry
                .map(|exp| exp < chrono::Utc::now())
                .unwrap_or(false);

            // Only report expired if no refresh token
            let effectively_expired = token_expired && !has_refresh;

            let needs_connect = !has_bq_scopes || effectively_expired;

            let google_email = t.email.clone();
            let google_name = t.name.clone();

            // Parse scopes into a list
            let scopes: Vec<&str> = scope_str.split_whitespace().collect();

            // Calculate time to expiry
            let (expires_in_seconds, expires_in_minutes) = parsed_expiry
                .map(|exp| {
                    let secs = (exp - chrono::Utc::now()).num_seconds().max(0);
                    (secs, secs / 60)
                })
                .unwrap_or((0, 0));

            Ok(Json(serde_json::json!({
                "connected": true,
                "google_email": google_email,
                "google_name": google_name,
                "has_bigquery_scopes": has_bq_scopes,
                "bigquery_access": bq_access,
                "needs_bigquery_connect": needs_connect,
                "token_expired": effectively_expired,
                "has_refresh_token": has_refresh,
                "expires_at": expires_at,
                "expires_in_seconds": expires_in_seconds,
                "expires_in_minutes": expires_in_minutes,
                "scopes": scopes,
                "last_login": oauth_data.as_ref().and_then(|o| o.last_oauth_login.as_deref()),
                "can_disconnect": true,
                "connect_url": "/api/v1/auth/google-oauth/connect",
                "disconnect_url": "/api/v1/auth/google-oauth/disconnect",
            })))
        }
    }
}

// ---------------------------------------------------------------------------
// GET /auth/google-oauth/projects
// ---------------------------------------------------------------------------

async fn google_oauth_projects(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let client_id = state
        .config
        .google_oauth_client_id
        .as_deref()
        .ok_or_else(|| {
            kyomi_core::Error::Internal("GOOGLE_OAUTH_CLIENT_ID not configured".into())
        })?;
    let client_secret = state
        .config
        .google_oauth_client_secret
        .as_deref()
        .ok_or_else(|| {
            kyomi_core::Error::Internal("GOOGLE_OAUTH_CLIENT_SECRET not configured".into())
        })?;

    // Centralized token resolution: reads DB, checks expiry, refreshes, persists
    let tokens = google_oauth::ensure_valid_google_token(
        &state.db,
        &user.user_id,
        &state.encryption_key,
        client_id,
        client_secret,
    )
    .await?;

    let google_email = tokens.email.clone().unwrap_or_default();

    // Call Google Cloud Resource Manager API
    let client = kyomi_datasource_server::http_client()?;
    let resp = client
        .get(google_oauth::GOOGLE_PROJECTS_URI)
        .bearer_auth(&tokens.access_token)
        .query(&[("filter", "lifecycleState:ACTIVE")])
        .send()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Google projects request failed: {e}")))?;

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

    // Extract projects and sort by name.
    // Shape must match Python's GCPService.list_projects_with_permission():
    //   { project_id, name, project_number, display_name, can_be_billing_project }
    let mut projects: Vec<serde_json::Value> = body["projects"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let project_id = p["projectId"].as_str().unwrap_or("");
            let name = p["name"].as_str().unwrap_or(project_id);
            let project_number = p["projectNumber"].as_str().unwrap_or("");
            serde_json::json!({
                "project_id": project_id,
                "name": name,
                "project_number": project_number,
                "display_name": format!("{name} ({project_id})"),
                "can_be_billing_project": true,
            })
        })
        .collect();

    projects.sort_by(|a, b| {
        let name_a = a["name"].as_str().unwrap_or("");
        let name_b = b["name"].as_str().unwrap_or("");
        name_a.to_lowercase().cmp(&name_b.to_lowercase())
    });

    let total = projects.len();

    Ok(Json(serde_json::json!({
        "projects": projects,
        "total_count": total,
        "google_email": google_email,
    })))
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Get OAuth client credentials from config.
fn get_oauth_credentials(state: &AppState) -> kyomi_core::Result<(String, String)> {
    let client_id = state
        .config
        .google_oauth_client_id
        .as_ref()
        .ok_or_else(|| {
            kyomi_core::Error::Internal("GOOGLE_OAUTH_CLIENT_ID not configured".into())
        })?
        .clone();

    let client_secret = state
        .config
        .google_oauth_client_secret
        .as_ref()
        .ok_or_else(|| {
            kyomi_core::Error::Internal("GOOGLE_OAUTH_CLIENT_SECRET not configured".into())
        })?
        .clone();

    Ok((client_id, client_secret))
}
