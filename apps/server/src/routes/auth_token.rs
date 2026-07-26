// SPDX-License-Identifier: AGPL-3.0-or-later

//! Token-refresh endpoint — external caller, cannot be a server_fn.
//!
//! `POST /api/v1/auth/refresh` is called directly by client-side JavaScript
//! in two places:
//!
//! 1. `crates/kyomi-ui/index.html` — page-load token pre-warm before WASM boots.
//! 2. `crates/kyomi-ui/src/utils/auth_refresh.rs` — silent retry after server_fn
//!    auth failures.
//!
//! Token refresh must set `Set-Cookie` response headers with the rotated tokens,
//! which requires a real HTTP handler — Leptos server functions cannot set
//! arbitrary response headers. This file is therefore the single keeper from
//! the auth bundle deletion (KYO-73 Group 1).

use axum::{
    extract::State,
    http::HeaderMap,
    response::IntoResponse,
    routing::post,
    Json, Router,
};

use kyomi_auth::{cookies, rate_limiter, token_refresh};

use crate::state::AppState;

/// Build the token-refresh sub-router mounted at `/api/v1/auth`.
pub fn routes() -> Router<AppState> {
    Router::new().route("/refresh", post(refresh_token))
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
    let device = kyomi_auth::request_meta::extract_device_info(&headers);
    let ip = device.ip_address.as_deref().unwrap_or("0.0.0.0");
    let rate_result = rate_limiter::check_rate_limit(&state.kv, ip, "refresh", None).await?;
    if !rate_result.allowed {
        return Err(kyomi_core::Error::TooManyRequests(
            format!("Rate limited. Try again in {} seconds", rate_result.retry_after_secs),
            rate_result.retry_after_secs,
        ));
    }

    // Shared refresh-token flow (verify + mint + rotate).
    let refreshed = token_refresh::refresh_tokens(
        &state.db,
        &state.config.jwt_secret,
        &refresh_token_value,
        &device,
    )
    .await?;

    // Set cookies
    let mut response_headers = HeaderMap::new();
    cookies::set_token_cookies(
        &mut response_headers,
        Some(&refreshed.access_token),
        Some(&refreshed.raw_refresh_token),
    );

    let body = serde_json::json!({
        "access_token": refreshed.access_token,
        "token_type": "bearer",
        "expires_in": refreshed.access_expires_in_secs,
        "user": {
            "user_id": refreshed.user_id,
            "email": refreshed.email,
            "name": refreshed.name,
            "roles": refreshed.roles,
        }
    });

    Ok((response_headers, Json(body)))
}
