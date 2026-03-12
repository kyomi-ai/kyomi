// SPDX-License-Identifier: AGPL-3.0-or-later

//! Authentication endpoints.
//!
//! Wire-compatible with Python's `routers/auth.py`.
//! All responses use `{"detail": "message"}` format for errors.

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::IntoResponse,
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use serde::Deserialize;

use kyomi_auth::{
    cookies,
    jwt,
    middleware::AuthUser,
    rate_limiter,
    token_service::{self, DeviceInfo, RefreshTokenVerifyResult},
    user_service,
};

use crate::state::AppState;

/// Build the `/auth` router with all authentication endpoints.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/refresh", post(refresh_token))
        .route("/logout", post(logout))
        .route("/logout-all", post(logout_all))
        .route("/me", get(get_me))
        .route("/profile", get(get_profile))
        .route("/profile", put(update_profile))
        .route("/me/bigquery-preferences", patch(update_bigquery_preferences))
        .route("/sessions", get(get_sessions))
        .route("/sessions/{token_id}", delete(revoke_session))
        .route("/verify", get(verify_email))
        .route("/resend-verification", post(resend_verification))
        .route("/check-email", post(check_email))
        .route("/check-token", get(check_token))
        .route("/switch-workspace/{workspace_id}", post(switch_workspace))
        .route("/websocket-token", get(websocket_token))
        .route("/config", get(get_auth_config))
}

// ---------------------------------------------------------------------------
// Helper: extract device info from headers
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

/// Load a user from the database or return 404.
async fn fetch_user_or_404(
    db: &kyomi_core::DbPool,
    user_id: &str,
) -> Result<kyomi_core::models::User, kyomi_core::Error> {
    user_service::get_user_by_id(db, user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("User not found".into()))
}

/// Set the X-Token-Refresh-Required header if token is near expiry.
fn maybe_set_refresh_header(headers: &mut HeaderMap, user: &AuthUser) {
    if user.token_needs_refresh() {
        headers.insert("x-token-refresh-required", axum::http::HeaderValue::from_static("true"));
    }
}

// ---------------------------------------------------------------------------
// Request/Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct VerifyQuery {
    token: String,
}

#[derive(Deserialize)]
struct ResendVerificationRequest {
    email: String,
}

#[derive(Deserialize)]
struct CheckEmailRequest {
    email: String,
}

#[derive(Deserialize)]
struct UpdateProfileRequest {
    name: Option<String>,
}

#[derive(Deserialize)]
struct BigQueryPreferencesRequest {
    billing_project: Option<String>,
    default_project: Option<String>,
    query_size_limit_gb: Option<i32>,
}

// ---------------------------------------------------------------------------
// Endpoint: POST /auth/refresh
// ---------------------------------------------------------------------------

async fn refresh_token(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    // Get refresh token from cookie
    let refresh_token_value = cookies::get_cookie_value(
        &headers,
        &kyomi_core::constants::get().cookies.refresh_token_name,
    )
    .ok_or_else(|| kyomi_core::Error::Unauthorized("No refresh token provided".into()))?
    .to_string();

    // Rate limit check
    let device = extract_device_info(&headers);
    let ip = device.ip_address.as_deref().unwrap_or("0.0.0.0");
    let rate_result = rate_limiter::check_rate_limit(&state.kv, ip, "refresh", None).await?;
    if !rate_result.allowed {
        return Err(kyomi_core::Error::TooManyRequests(
            format!("Rate limited. Try again in {} seconds", rate_result.retry_after_secs),
            rate_result.retry_after_secs,
        ));
    }

    // Verify refresh token (handles grace period + theft detection)
    let verify_result = token_service::verify_refresh_token(&state.db, &refresh_token_value).await?;

    let user_data = match verify_result {
        RefreshTokenVerifyResult::Valid(data)
        | RefreshTokenVerifyResult::GracePeriod(data) => data,
        RefreshTokenVerifyResult::TheftDetected { .. } => {
            return Err(kyomi_core::Error::Unauthorized(
                "Refresh token has been revoked (possible token theft detected)".into(),
            ));
        }
        RefreshTokenVerifyResult::Invalid => {
            return Err(kyomi_core::Error::Unauthorized(
                "Invalid or expired refresh token".into(),
            ));
        }
    };

    // Get workspace context for the user
    let mut extra = std::collections::HashMap::new();
    extra.insert("user_id".into(), serde_json::json!(user_data.user_id));
    extra.insert("email".into(), serde_json::json!(user_data.email));
    extra.insert("name".into(), serde_json::json!(user_data.name));
    extra.insert("roles".into(), serde_json::json!(user_data.roles));

    // Load workspace context
    if let Ok(Some((ws, wu))) = user_service::get_user_workspace_context(&state.db, &user_data.user_id).await {
        extra.insert("workspace_id".into(), serde_json::json!(ws.workspace_id));
        extra.insert("workspace_roles".into(), serde_json::json!(vec![wu.role]));
    }

    // Create new access token
    let jwt_config = &kyomi_core::constants::get().jwt;
    let new_access_token = jwt::create_access_token_str(
        &user_data.user_id,
        &state.config.jwt_secret,
        jwt_config.access_token_expire_minutes,
        extra,
    )?;

    // Always rotate: every tab gets a fresh token (prevents multi-tab sign-out bug)
    let new_raw_refresh = jwt::create_refresh_token();
    let new_token_hash = token_service::hash_refresh_token(&new_raw_refresh);
    let expires_at = chrono::Utc::now() + chrono::Duration::days(jwt_config.refresh_token_expire_days);

    token_service::rotate_refresh_token(
        &state.db,
        &user_data.token_id,
        &user_data.user_id,
        &user_data.family_id,
        &new_token_hash,
        expires_at,
        &device,
    ).await?;

    // Set cookies
    let mut response_headers = HeaderMap::new();
    cookies::set_token_cookies(
        &mut response_headers,
        Some(&new_access_token),
        Some(&new_raw_refresh),
    );

    let body = serde_json::json!({
        "access_token": new_access_token,
        "token_type": "bearer",
        "expires_in": jwt_config.access_token_expire_minutes * 60,
        "user": {
            "user_id": user_data.user_id,
            "email": user_data.email,
            "name": user_data.name,
            "roles": user_data.roles,
        }
    });

    Ok((response_headers, Json(body)))
}

// ---------------------------------------------------------------------------
// Endpoint: POST /auth/logout
// ---------------------------------------------------------------------------

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Get refresh token from cookie
    let refresh_token_name = &kyomi_core::constants::get().cookies.refresh_token_name;
    if let Some(refresh_token_value) = cookies::get_cookie_value(&headers, refresh_token_name) {
        // Verify and revoke the entire family
        match token_service::verify_refresh_token(&state.db, refresh_token_value).await {
            Ok(RefreshTokenVerifyResult::Valid(data) | RefreshTokenVerifyResult::GracePeriod(data)) => {
                let _ = token_service::revoke_token_family(&state.db, &data.family_id).await;
            }
            _ => {
                // Token invalid/theft-detected — already revoked or nothing to do
            }
        }
    }

    let mut response_headers = HeaderMap::new();
    cookies::clear_token_cookies(&mut response_headers);

    (response_headers, Json(serde_json::json!({
        "success": true,
        "message": "Logged out successfully"
    })))
}

// ---------------------------------------------------------------------------
// Endpoint: POST /auth/logout-all
// ---------------------------------------------------------------------------

async fn logout_all(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    let revoked_count = token_service::revoke_all_user_refresh_tokens(&state.db, &user.user_id).await?;

    let mut response_headers = HeaderMap::new();
    cookies::clear_token_cookies(&mut response_headers);

    Ok((response_headers, Json(serde_json::json!({
        "success": true,
        "message": format!("Logged out from all devices successfully ({revoked_count} sessions revoked)")
    }))))
}

// ---------------------------------------------------------------------------
// Endpoint: GET /auth/me
// ---------------------------------------------------------------------------

async fn get_me(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    // Get fresh user data from database for BigQuery preferences
    let db_user = fetch_user_or_404(&state.db, &user.user_id).await?;

    let mut response_headers = HeaderMap::new();
    maybe_set_refresh_header(&mut response_headers, &user);

    let body = serde_json::json!({
        "user_id": db_user.user_id,
        "email": db_user.email,
        "name": db_user.name,
        "roles": db_user.roles(),
        "active": db_user.active,
        "created_at": db_user.created_at,
        "last_login": db_user.last_login,
        "workspace_id": user.workspace.workspace_id,
        "workspace_name": user.workspace.workspace_name,
        "workspace_roles": user.workspace.workspace_roles,
        "is_owner": user.workspace.is_owner,
        "billing_project": db_user.billing_project,
        "default_project": db_user.default_project,
        "query_size_limit_gb": db_user.query_size_limit_gb,
    });

    Ok((response_headers, Json(body)))
}

// ---------------------------------------------------------------------------
// Endpoint: GET /auth/profile
// ---------------------------------------------------------------------------

async fn get_profile(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    let db_user = fetch_user_or_404(&state.db, &user.user_id).await?;

    let has_password = user_service::has_password(&state.db, &user.user_id).await?;

    let mut response_headers = HeaderMap::new();
    maybe_set_refresh_header(&mut response_headers, &user);

    let body = serde_json::json!({
        "user_id": db_user.user_id,
        "email": db_user.email,
        "name": db_user.name,
        "roles": db_user.roles(),
        "active": db_user.active,
        "created_at": db_user.created_at,
        "last_login": db_user.last_login,
        "has_password": has_password,
        "extra_metadata": db_user.extra_metadata.unwrap_or(serde_json::json!({})),
        "workspace_id": user.workspace.workspace_id,
        "workspace_name": user.workspace.workspace_name,
        "workspace_roles": user.workspace.workspace_roles,
        "workspace_status": user.workspace.workspace_status,
        "subscription_tier": user.workspace.subscription_tier,
        "is_owner": user.workspace.is_owner,
    });

    Ok((response_headers, Json(body)))
}

// ---------------------------------------------------------------------------
// Endpoint: PUT /auth/profile
// ---------------------------------------------------------------------------

async fn update_profile(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<UpdateProfileRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    if let Some(ref name) = data.name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(kyomi_core::Error::BadRequest("Name cannot be empty".into()));
        }
        user_service::update_user_name(&state.db, &user.user_id, trimmed).await?;
    }

    // Return updated user
    let updated = fetch_user_or_404(&state.db, &user.user_id).await?;

    Ok(Json(serde_json::json!({
        "user_id": updated.user_id,
        "email": updated.email,
        "name": updated.name,
        "roles": updated.roles(),
        "message": "Profile updated successfully",
    })))
}

// ---------------------------------------------------------------------------
// Endpoint: PATCH /auth/me/bigquery-preferences
// ---------------------------------------------------------------------------

async fn update_bigquery_preferences(
    State(state): State<AppState>,
    user: AuthUser,
    Json(data): Json<BigQueryPreferencesRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    if data.billing_project.is_none() && data.default_project.is_none() && data.query_size_limit_gb.is_none() {
        return Err(kyomi_core::Error::BadRequest("No BigQuery preferences provided".into()));
    }

    let success = user_service::update_bigquery_preferences(
        &state.db,
        &user.user_id,
        data.billing_project.as_deref(),
        data.default_project.as_deref(),
        data.query_size_limit_gb,
    ).await?;

    if !success {
        return Err(kyomi_core::Error::NotFound("User not found".into()));
    }

    let mut preferences = serde_json::Map::new();
    if let Some(bp) = &data.billing_project {
        preferences.insert("billing_project".into(), serde_json::json!(bp));
    }
    if let Some(dp) = &data.default_project {
        preferences.insert("default_project".into(), serde_json::json!(dp));
    }
    if let Some(qsl) = data.query_size_limit_gb {
        preferences.insert("query_size_limit_gb".into(), serde_json::json!(qsl));
    }

    Ok(Json(serde_json::json!({
        "message": "BigQuery preferences updated successfully",
        "preferences": preferences,
    })))
}

// ---------------------------------------------------------------------------
// Endpoint: GET /auth/sessions
// ---------------------------------------------------------------------------

async fn get_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let sessions = token_service::get_user_sessions(&state.db, &user.user_id).await?;

    // Determine current session by looking up the family_id of the cookie's refresh token.
    // With rotation the token_hash changes on each refresh, so we compare by family instead.
    let refresh_token_name = &kyomi_core::constants::get().cookies.refresh_token_name;
    #[derive(sqlx::FromRow)]
    struct FamilyIdRow { family_id: String }

    let current_family_id: Option<String> = if let Some(raw_token) = cookies::get_cookie_value(&headers, refresh_token_name) {
        let hash = token_service::hash_refresh_token(raw_token);
        kyomi_core::db_fetch_optional!(
            &state.db, FamilyIdRow,
            "SELECT family_id FROM refresh_tokens WHERE token_hash = $1 AND is_active = true",
            &hash
        )
        .ok()
        .flatten()
        .map(|r| r.family_id)
    } else {
        None
    };

    let sessions_json: Vec<serde_json::Value> = sessions
        .iter()
        .map(|s| {
            let is_current = current_family_id.as_deref() == Some(&s.family_id);
            serde_json::json!({
                "token_id": s.token_id,
                "created_at": s.created_at,
                "last_used": s.last_used,
                "expires_at": s.expires_at,
                "user_agent": s.user_agent,
                "ip_address": s.ip_address,
                "country_code": s.country_code,
                "oauth_client_name": s.oauth_client_name,
                "is_current": is_current,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "sessions": sessions_json,
        "total": sessions_json.len(),
    })))
}

// ---------------------------------------------------------------------------
// Endpoint: DELETE /auth/sessions/{token_id}
// ---------------------------------------------------------------------------

async fn revoke_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(token_id): Path<String>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Prevent revoking current session — compare by family_id (stable across rotation)
    let refresh_token_name = &kyomi_core::constants::get().cookies.refresh_token_name;
    if let Some(current_token) = cookies::get_cookie_value(&headers, refresh_token_name) {
        let current_hash = token_service::hash_refresh_token(current_token);

        #[derive(sqlx::FromRow)]
        struct FamilyRow { family_id: String }

        // Look up family_id of the current session's refresh token
        let current_family = kyomi_core::db_fetch_optional!(
            &state.db, FamilyRow,
            "SELECT family_id FROM refresh_tokens WHERE token_hash = $1 AND is_active = true",
            &current_hash
        )?
        .map(|r| r.family_id);

        // Look up family_id of the target session
        let target_family = kyomi_core::db_fetch_optional!(
            &state.db, FamilyRow,
            "SELECT family_id FROM refresh_tokens \
             WHERE token_id = $1 AND user_id = $2 AND is_active = true",
            &token_id,
            &user.user_id
        )?
        .map(|r| r.family_id);

        if let (Some(current_fam), Some(target_fam)) = (&current_family, &target_family) {
            if current_fam == target_fam {
                return Err(kyomi_core::Error::BadRequest(
                    "Cannot revoke your current session. Use logout instead.".into()
                ));
            }
        }
    }

    let revoked = token_service::revoke_user_refresh_token(&state.db, &user.user_id, &token_id).await?;
    if !revoked {
        return Err(kyomi_core::Error::NotFound("Session not found".into()));
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Session revoked successfully",
    })))
}

// ---------------------------------------------------------------------------
// Endpoint: GET /auth/verify?token=...
// ---------------------------------------------------------------------------

async fn verify_email(
    State(state): State<AppState>,
    Query(query): Query<VerifyQuery>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Verify the token and get email
    let email = token_service::verify_verification_token(
        &state.db,
        &query.token,
        "email_verification",
    ).await?
    .ok_or_else(|| kyomi_core::Error::BadRequest("Invalid or expired verification token".into()))?;

    // Mark user as verified
    user_service::mark_user_verified(&state.db, &email).await?;

    Ok(Json(serde_json::json!({
        "message": "Email verified successfully",
    })))
}

// ---------------------------------------------------------------------------
// Endpoint: POST /auth/resend-verification
// ---------------------------------------------------------------------------

async fn resend_verification(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<ResendVerificationRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    // Rate limit — uses "register" bucket (same abuse pattern as registration)
    let ip = extract_client_ip(&headers);
    let rate_result = rate_limiter::check_rate_limit(&state.kv, &ip, "register", None).await?;
    if !rate_result.allowed {
        return Err(kyomi_core::Error::TooManyRequests(
            format!("Rate limited. Try again in {} seconds", rate_result.retry_after_secs),
            rate_result.retry_after_secs,
        ));
    }

    let success_message = "If an account exists with this email and is not yet verified, a verification link has been sent.";

    // Self-hosted without SMTP: all users are pre-verified, nothing to send
    if state.config.self_hosted && !state.config.smtp_configured() {
        tracing::info!(
            email = %data.email,
            "Self-hosted SMTP-less: resend-verification is a no-op (users are pre-verified)"
        );
        return Ok(Json(serde_json::json!({ "message": success_message })));
    }

    // Look up user
    let user = user_service::get_user_by_email(&state.db, &data.email).await?;
    let Some(user) = user else {
        return Ok(Json(serde_json::json!({ "message": success_message })));
    };

    if user.verified {
        return Ok(Json(serde_json::json!({ "message": success_message })));
    }

    // Create verification token
    let raw_token = token_service::create_verification_token(
        &state.db,
        &data.email,
        "email_verification",
    ).await?;

    let verification_url = format!(
        "{}/verify-email?token={raw_token}",
        state.config.frontend_url.trim_end_matches('/')
    );
    tracing::info!("verification token for {}: {raw_token}", data.email);

    // Send verification email (async, non-blocking)
    let email_clone = data.email.clone();
    let name_clone = user.name.clone().unwrap_or_default();
    let url_clone = verification_url;
    tokio::spawn(async move {
        let email_svc = kyomi_auth::email_service::EmailService::from_env();
        let sent = email_svc
            .send_verification_email(&email_clone, &name_clone, &url_clone)
            .await;
        if sent {
            tracing::info!("📧 Verification email sent to {email_clone}");
        } else {
            tracing::warn!("⚠️ Failed to send verification email to {email_clone}");
        }
    });

    Ok(Json(serde_json::json!({ "message": success_message })))
}

// ---------------------------------------------------------------------------
// Endpoint: POST /auth/check-email
// ---------------------------------------------------------------------------

async fn check_email(
    State(state): State<AppState>,
    Json(data): Json<CheckEmailRequest>,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    if data.email.is_empty() {
        return Err(kyomi_core::Error::BadRequest("Email is required".into()));
    }

    let user = user_service::get_user_by_email(&state.db, &data.email).await?;

    if let Some(user) = user {
        Ok(Json(serde_json::json!({
            "exists": true,
            "email": data.email,
            "verified": user.verified,
            "message": "Email address is already registered",
        })))
    } else {
        Ok(Json(serde_json::json!({
            "exists": false,
            "email": data.email,
            "message": "Email address is available",
        })))
    }
}

// ---------------------------------------------------------------------------
// Endpoint: GET /auth/check-token
// ---------------------------------------------------------------------------

/// Validates the current access token. Returns 200 if valid, 401 if not.
/// (The AuthUser extractor handles the 401 case automatically.)
async fn check_token(
    user: AuthUser,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "valid": true,
        "user_id": user.user_id,
    }))
}

// ---------------------------------------------------------------------------
// Endpoint: POST /auth/switch-workspace/{workspace_id}
// ---------------------------------------------------------------------------

async fn switch_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(workspace_id): Path<String>,
) -> Result<impl IntoResponse, kyomi_core::Error> {
    // Verify user has access to target workspace
    let wu = user_service::get_workspace_user(&state.db, &workspace_id, &user.user_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::Forbidden("You do not have access to this workspace".into()))?;

    let ws = user_service::get_workspace(&state.db, &workspace_id)
        .await?
        .ok_or_else(|| kyomi_core::Error::NotFound("Workspace not found".into()))?;

    // Build new JWT with workspace context
    let jwt_config = &kyomi_core::constants::get().jwt;
    let mut extra = std::collections::HashMap::new();
    extra.insert("user_id".into(), serde_json::json!(user.user_id));
    extra.insert("email".into(), serde_json::json!(user.email));
    extra.insert("name".into(), serde_json::json!(user.name));
    extra.insert("roles".into(), serde_json::json!(user.roles));
    extra.insert("workspace_id".into(), serde_json::json!(workspace_id));
    extra.insert("workspace_name".into(), serde_json::json!(ws.name));
    extra.insert("workspace_status".into(), serde_json::json!(ws.status));
    extra.insert("workspace_roles".into(), serde_json::json!(vec![wu.role]));
    extra.insert("subscription_tier".into(), serde_json::json!(ws.subscription_tier));

    let new_access_token = jwt::create_access_token_str(
        &user.user_id,
        &state.config.jwt_secret,
        jwt_config.access_token_expire_minutes,
        extra,
    )?;

    // Create new refresh token with a new family (workspace switch = new session)
    let raw_refresh = jwt::create_refresh_token();
    let token_hash = token_service::hash_refresh_token(&raw_refresh);
    let expires_at = chrono::Utc::now() + chrono::Duration::days(jwt_config.refresh_token_expire_days);
    let new_family_id = token_service::generate_family_id();

    let device = extract_device_info(&headers);
    token_service::store_refresh_token(&state.db, &user.user_id, &token_hash, expires_at, &device, &new_family_id).await?;

    // Revoke the old session's family
    let refresh_token_name = &kyomi_core::constants::get().cookies.refresh_token_name;
    if let Some(old_token) = cookies::get_cookie_value(&headers, refresh_token_name) {
        let old_hash = token_service::hash_refresh_token(old_token);
        #[derive(sqlx::FromRow)]
        struct FamRow { family_id: String }
        if let Ok(Some(row)) = kyomi_core::db_fetch_optional!(
            &state.db, FamRow,
            "SELECT family_id FROM refresh_tokens WHERE token_hash = $1 AND is_active = true",
            &old_hash
        ) {
            let _ = token_service::revoke_token_family(&state.db, &row.family_id).await;
        }
    }

    // Set cookies
    let mut response_headers = HeaderMap::new();
    cookies::set_token_cookies(
        &mut response_headers,
        Some(&new_access_token),
        Some(&raw_refresh),
    );

    // Update last_workspace_id
    let _ = user_service::update_last_workspace(&state.db, &user.user_id, &workspace_id).await;

    let body = serde_json::json!({
        "success": true,
        "workspace_id": workspace_id,
        "workspace_name": ws.name,
        "message": "Workspace switched successfully",
    });

    Ok((response_headers, Json(body)))
}

// ---------------------------------------------------------------------------
// Endpoint: GET /auth/websocket-token
// ---------------------------------------------------------------------------

async fn websocket_token(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<serde_json::Value>, kyomi_core::Error> {
    let mut extra = std::collections::HashMap::new();
    extra.insert("user_id".into(), serde_json::json!(user.user_id));
    extra.insert("email".into(), serde_json::json!(user.email));
    extra.insert("name".into(), serde_json::json!(user.name));
    extra.insert("roles".into(), serde_json::json!(user.roles));

    // Short-lived token (15 minutes, matching Python)
    let token = jwt::create_access_token_str(
        &user.user_id,
        &state.config.jwt_secret,
        15,
        extra,
    )?;

    Ok(Json(serde_json::json!({
        "token": token,
        "expires_in": 15 * 60,
        "user_id": user.user_id,
    })))
}

// ---------------------------------------------------------------------------
// Endpoint: GET /auth/config
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct AuthConfigResponse {
    google_oauth: bool,
    passkeys: bool,
    password: bool,
    self_hosted: bool,
    smtp_configured: bool,
}

/// Public endpoint — returns which auth methods are available.
/// No `AuthUser` extractor = no authentication required.
async fn get_auth_config(
    State(state): State<AppState>,
) -> impl IntoResponse {
    Json(AuthConfigResponse {
        google_oauth: state.config.google_oauth_client_id.is_some()
            && state.config.google_oauth_client_secret.is_some(),
        passkeys: state.config.passkeys_enabled,
        password: state.config.password_auth_enabled,
        self_hosted: state.config.self_hosted,
        smtp_configured: state.config.smtp_configured(),
    })
}
