// SPDX-License-Identifier: AGPL-3.0-or-later

//! Web Push delivery — sends push notifications to subscribed browser/device endpoints.
//!
//! Uses the `web-push-native` crate for RFC 8291 (ECE encryption) and RFC 8292 (VAPID signing).
//! The HTTP transport uses the workspace-shared `reqwest` client (rustls, no OpenSSL).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Serialize;
use tracing::{error, info, warn};
use web_push_native::jwt_simple::prelude::ES256KeyPair;
use web_push_native::p256::PublicKey;
use web_push_native::{Auth, WebPushBuilder};

use kyomi_auth::push_service;
use kyomi_core::models::PushSubscription;
use kyomi_core::DbPool;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Parsed VAPID configuration, created once at startup from env vars.
pub struct VapidConfig {
    /// The ES256 key pair parsed from the base64url-encoded private key.
    pub key_pair: ES256KeyPair,
    /// Contact URI (e.g., "mailto:support@kyomi.ai").
    pub contact: String,
}

impl VapidConfig {
    /// Parse VAPID config from base64url-encoded private key and contact URI.
    pub fn from_config(private_key_b64: &str, contact: &str) -> Result<Self, String> {
        let private_bytes = URL_SAFE_NO_PAD
            .decode(private_key_b64)
            .map_err(|e| format!("Invalid VAPID private key base64: {e}"))?;

        let key_pair = ES256KeyPair::from_bytes(&private_bytes)
            .map_err(|e| format!("Invalid VAPID private key: {e}"))?;

        Ok(Self {
            key_pair,
            contact: contact.to_string(),
        })
    }
}

/// Payload sent to the service worker via push notification.
#[derive(Debug, Serialize)]
pub struct PushPayload {
    /// Notification type (e.g., "watch_alert", "watch_report").
    #[serde(rename = "type")]
    pub notification_type: String,
    /// Watch ID.
    pub watch_id: String,
    /// Watch name.
    pub watch_name: String,
    /// Execution ID.
    pub execution_id: i32,
    /// Notification title.
    pub title: String,
    /// Notification body text (truncated).
    pub body: String,
    /// URL to navigate to when clicking the notification.
    pub url: String,
    /// Notification icon path.
    pub icon: String,
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

/// Send push notifications to all of a user's subscribed devices.
///
/// Returns the number of successfully delivered notifications.
/// Failures are handled per-subscription (410 Gone → delete, 5xx → increment failure count).
pub async fn send_push_notifications(
    db: &DbPool,
    http_client: &reqwest::Client,
    vapid_config: &VapidConfig,
    user_id: &str,
    payload: &PushPayload,
) -> usize {
    let subscriptions = match push_service::get_user_subscriptions(db, user_id).await {
        Ok(subs) => subs,
        Err(e) => {
            error!(user_id = %user_id, error = %e, "Failed to get push subscriptions");
            return 0;
        }
    };

    if subscriptions.is_empty() {
        return 0;
    }

    let payload_json = match serde_json::to_string(payload) {
        Ok(j) => j,
        Err(e) => {
            error!(error = %e, "Failed to serialize push payload");
            return 0;
        }
    };

    let mut success_count = 0;

    for sub in &subscriptions {
        match send_to_subscription(http_client, vapid_config, sub, &payload_json).await {
            Ok(()) => {
                push_service::record_success(db, sub.id).await;
                success_count += 1;
            }
            Err(PushError::Gone) => {
                push_service::record_failure(db, sub.id, true).await;
            }
            Err(PushError::RateLimited) => {
                warn!(subscription_id = sub.id, "Push rate limited, skipping");
                // Don't count as failure — transient
            }
            Err(PushError::ServerError(status)) => {
                warn!(subscription_id = sub.id, status = status, "Push server error");
                push_service::record_failure(db, sub.id, false).await;
            }
            Err(PushError::Other(msg)) => {
                warn!(subscription_id = sub.id, error = %msg, "Push delivery failed");
                push_service::record_failure(db, sub.id, false).await;
            }
        }
    }

    if success_count > 0 {
        info!(
            user_id = %user_id,
            total = subscriptions.len(),
            success = success_count,
            "Sent push notifications"
        );
    }

    success_count
}

// ---------------------------------------------------------------------------
// Per-subscription delivery
// ---------------------------------------------------------------------------

/// Error types for a single push delivery attempt.
enum PushError {
    /// 410 Gone or 404 — subscription expired/unsubscribed.
    Gone,
    /// 429 Too Many Requests — rate limited by push service.
    RateLimited,
    /// 5xx server error from push service.
    ServerError(u16),
    /// Any other failure (network, encryption, etc.).
    Other(String),
}

/// Send an encrypted push notification to a single subscription endpoint.
async fn send_to_subscription(
    http_client: &reqwest::Client,
    vapid_config: &VapidConfig,
    sub: &PushSubscription,
    payload_json: &str,
) -> Result<(), PushError> {
    // Parse the subscription's client public key (P-256 uncompressed point).
    let ua_public_bytes = URL_SAFE_NO_PAD
        .decode(&sub.p256dh)
        .map_err(|e| PushError::Other(format!("Invalid p256dh base64: {e}")))?;

    let ua_public = PublicKey::from_sec1_bytes(&ua_public_bytes)
        .map_err(|e| PushError::Other(format!("Invalid p256dh key: {e}")))?;

    // Parse the subscription's auth secret (16 bytes).
    let auth_bytes = URL_SAFE_NO_PAD
        .decode(&sub.auth)
        .map_err(|e| PushError::Other(format!("Invalid auth base64: {e}")))?;

    let auth_array: [u8; 16] = auth_bytes
        .as_slice()
        .try_into()
        .map_err(|_| PushError::Other(format!("Auth must be 16 bytes, got {}", auth_bytes.len())))?;

    let ua_auth: Auth = Auth::from(auth_array);

    // Parse the push service endpoint URL.
    let endpoint: http::Uri = sub
        .endpoint
        .parse()
        .map_err(|e| PushError::Other(format!("Invalid endpoint URL: {e}")))?;

    // Build the encrypted push request with VAPID authorization.
    let push_request = WebPushBuilder::new(endpoint, ua_public, ua_auth)
        .with_vapid(&vapid_config.key_pair, &vapid_config.contact)
        .build(payload_json.as_bytes())
        .map_err(|e| PushError::Other(format!("Failed to build push request: {e}")))?;

    // Convert http::Request to reqwest and send.
    let (parts, body) = push_request.into_parts();
    let url = parts.uri.to_string();

    let mut req = http_client.post(&url).body(body);

    // Copy headers from the web-push-native request (TTL, Content-Encoding, Authorization, etc.)
    for (name, value) in &parts.headers {
        if let Ok(v) = value.to_str() {
            req = req.header(name.as_str(), v);
        }
    }

    let response = req
        .send()
        .await
        .map_err(|e| PushError::Other(format!("HTTP request failed: {e}")))?;

    let status = response.status().as_u16();

    match status {
        201 => Ok(()),
        410 | 404 => Err(PushError::Gone),
        429 => Err(PushError::RateLimited),
        s if s >= 500 => Err(PushError::ServerError(s)),
        _ => {
            let body = response.text().await.unwrap_or_default();
            Err(PushError::Other(format!(
                "Unexpected status {status}: {body}"
            )))
        }
    }
}
