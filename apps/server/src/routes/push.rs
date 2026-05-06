// SPDX-License-Identifier: AGPL-3.0-or-later

//! Push notification REST endpoints — Web Push (VAPID) subscription management.
//!
//! ## Endpoints
//!
//! - `GET  /vapid-key`          — Return the VAPID public key (for browser `pushManager.subscribe()`)
//! - `POST /subscribe`          — Save a push subscription from the browser
//! - `POST /unsubscribe`        — Remove a push subscription by endpoint
//! - `GET  /subscriptions`      — List user's registered devices (settings UI)
//! - `DELETE /subscriptions/:id` — Remove a specific device subscription

use axum::{
    extract::{Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use kyomi_auth::{middleware::AuthUser, push_service};

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/vapid-key", get(get_vapid_key))
        .route("/subscribe", post(subscribe))
        .route("/unsubscribe", post(unsubscribe))
        .route("/subscriptions", get(list_subscriptions))
        .route("/subscriptions/{id}", delete(delete_subscription))
}

// ===========================================================================
// Types
// ===========================================================================

#[derive(Deserialize)]
struct SubscribeRequest {
    endpoint: String,
    p256dh: String,
    auth: String,
    user_agent: Option<String>,
    device_label: Option<String>,
}

#[derive(Deserialize)]
struct UnsubscribeRequest {
    endpoint: String,
}

// ===========================================================================
// Handlers
// ===========================================================================

/// `GET /vapid-key` — Return the VAPID public key for `pushManager.subscribe()`.
///
/// The browser needs the server's VAPID public key (base64url-encoded, uncompressed P-256)
/// to create a push subscription. This endpoint derives it from the configured private key.
async fn get_vapid_key(
    State(state): State<AppState>,
    _user: AuthUser,
) -> Result<Json<Value>, kyomi_core::Error> {
    let Some(ref private_key_b64) = state.config.vapid_private_key else {
        return Err(kyomi_core::Error::Internal(
            "Push notifications are not configured".into(),
        ));
    };

    let public_key = derive_vapid_public_key(private_key_b64)?;

    Ok(Json(json!({ "public_key": public_key })))
}

/// `POST /subscribe` — Save a push subscription from the browser.
async fn subscribe(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<SubscribeRequest>,
) -> Result<Json<Value>, kyomi_core::Error> {
    let input = push_service::SaveSubscriptionInput {
        endpoint: body.endpoint,
        p256dh: body.p256dh,
        auth: body.auth,
        user_agent: body.user_agent,
        device_label: body.device_label,
    };

    let sub = push_service::save_subscription(&state.db, &user.user_id, &input).await?;

    Ok(Json(json!({
        "id": sub.id,
        "device_label": sub.device_label,
    })))
}

/// `POST /unsubscribe` — Remove a push subscription by endpoint.
async fn unsubscribe(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<UnsubscribeRequest>,
) -> Result<Json<Value>, kyomi_core::Error> {
    push_service::remove_subscription(&state.db, &user.user_id, &body.endpoint).await?;
    Ok(Json(json!({ "ok": true })))
}

/// `GET /subscriptions` — List user's registered push devices (settings UI).
async fn list_subscriptions(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Value>, kyomi_core::Error> {
    let subs = push_service::list_user_subscriptions(&state.db, &user.user_id).await?;
    Ok(Json(json!({ "subscriptions": subs })))
}

/// `DELETE /subscriptions/:id` — Remove a specific device subscription.
async fn delete_subscription(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i32>,
) -> Result<Json<Value>, kyomi_core::Error> {
    push_service::remove_subscription_by_id(&state.db, &user.user_id, id).await?;
    Ok(Json(json!({ "ok": true })))
}

// ===========================================================================
// VAPID key derivation
// ===========================================================================

/// Derive the VAPID public key from the base64url-encoded private key.
///
/// The private key is a raw 32-byte P-256 scalar. We use `p256` crate (already
/// a transitive dependency via `web-push-native`) to derive the public key,
/// then return it as a base64url-encoded uncompressed point (65 bytes).
///
/// The browser's `pushManager.subscribe({ applicationServerKey })` expects
/// this format (base64url of the raw uncompressed public key bytes).
fn derive_vapid_public_key(private_key_b64: &str) -> Result<String, kyomi_core::Error> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use p256::elliptic_curve::sec1::ToEncodedPoint;

    let private_bytes = URL_SAFE_NO_PAD
        .decode(private_key_b64)
        .map_err(|e| kyomi_core::Error::Internal(format!("Invalid VAPID private key base64: {e}")))?;

    let secret_key = p256::SecretKey::from_slice(&private_bytes)
        .map_err(|e| kyomi_core::Error::Internal(format!("Invalid VAPID private key: {e}")))?;

    let public_key = secret_key.public_key();
    let public_bytes = public_key.to_encoded_point(false); // uncompressed (65 bytes)

    Ok(URL_SAFE_NO_PAD.encode(public_bytes.as_bytes()))
}
