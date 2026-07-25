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

/// Build a dedicated HTTP client for Web Push delivery.
///
/// KYO-219: this must NOT be `kyomi_datasource_server::http_client()` (the
/// client shared with datasource drivers). That client leaves `reqwest`'s
/// default redirect policy in place — up to 10 redirects followed
/// automatically — which other callers may legitimately need. For push
/// sends that default is a live risk: an allowlisted vendor endpoint
/// (`endpoint_host_is_allowed` only checks the request host, not where a 3xx
/// response might point) that responded with a redirect to an internal
/// address would otherwise be followed silently, carrying the push payload
/// wherever the `Location` header names.
///
/// Per RFC 8030 §5, a push service's response to a push message is never a
/// redirect (only 2xx/4xx/5xx are defined), so disabling redirects here
/// costs nothing for legitimate delivery — any 3xx we actually receive is
/// already anomalous and is handled by `send_to_subscription`'s catch-all
/// `PushError::Other` branch.
pub fn push_http_client() -> kyomi_core::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("Kyomi/1.0")
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to build push HTTP client: {e}")))
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
#[derive(Debug)]
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

    // Re-validate against the same predicate enforced at subscribe time
    // (KYO-219). This row may predate the fix, or may have been written by
    // something that bypassed the route — either way, a stored endpoint that
    // now fails validation must never be sent to (it would carry the VAPID
    // `Authorization` JWT to whatever host it names), and the subscription is
    // removed rather than retried, reusing the existing `Gone` cleanup path.
    if let Err(reason) = kyomi_auth::push_service::validate_push_endpoint(&sub.endpoint) {
        warn!(
            subscription_id = sub.id,
            reason = %reason,
            "Push subscription endpoint failed validation at send time; removing"
        );
        return Err(PushError::Gone);
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// A syntactically valid P-256 uncompressed public key / 16-byte auth
    /// secret pair (not tied to any real subscriber), so
    /// `send_to_subscription` gets past the p256dh/auth parsing steps and
    /// reaches the endpoint validation check under test.
    const TEST_P256DH: &str =
        "BNvsCh-iTzovU3ujQD_THGIPeFKTMjZV0V4GN3IeN5otYKGgFtPEQ7IC0D0kGE8VPil54L_IcWUsIMIjpwa2bww";
    const TEST_AUTH: &str = "BdoZ3UeQMCUdmCzzw6OuDg";

    fn test_subscription(endpoint: &str) -> PushSubscription {
        PushSubscription {
            id: 1,
            user_id: "test-user".to_string(),
            endpoint: endpoint.to_string(),
            p256dh: TEST_P256DH.to_string(),
            auth: TEST_AUTH.to_string(),
            user_agent: None,
            device_label: None,
            created_at: Utc::now(),
            last_used_at: None,
            failure_count: 0,
        }
    }

    fn test_vapid_config() -> VapidConfig {
        VapidConfig {
            key_pair: ES256KeyPair::generate(),
            contact: "mailto:test@example.com".to_string(),
        }
    }

    /// KYO-219 — egress re-validation. A subscription row whose endpoint
    /// fails `validate_push_endpoint` (e.g. it predates the fix, or was
    /// written directly to the DB) must never reach `http_client.post`: if
    /// it did, the VAPID `Authorization` JWT would be sent straight to
    /// whatever host the row names. It must instead surface as
    /// `PushError::Gone` so the caller (`send_push_notifications`) removes
    /// the subscription via the existing `record_failure(.., true)` path.
    #[tokio::test]
    async fn send_to_subscription_rejects_ssrf_endpoint_without_sending() {
        let sub = test_subscription("https://169.254.169.254/latest/meta-data/");
        let vapid_config = test_vapid_config();
        let http_client = reqwest::Client::new();

        let result = send_to_subscription(&http_client, &vapid_config, &sub, "{}").await;

        assert!(
            matches!(result, Err(PushError::Gone)),
            "expected PushError::Gone so the subscription gets removed, got {result:?}"
        );
    }

    #[tokio::test]
    async fn send_to_subscription_rejects_unrecognized_host_without_sending() {
        let sub = test_subscription("https://attacker.example/collect");
        let vapid_config = test_vapid_config();
        let http_client = reqwest::Client::new();

        let result = send_to_subscription(&http_client, &vapid_config, &sub, "{}").await;

        assert!(
            matches!(result, Err(PushError::Gone)),
            "expected PushError::Gone so the subscription gets removed, got {result:?}"
        );
    }

    /// KYO-219 (review follow-up) — `push_http_client()` must not follow
    /// redirects. `send_to_subscription` itself can't be used for this test:
    /// its endpoint would have to be a loopback address to reach a local
    /// mock server, and `validate_push_endpoint` correctly refuses to send
    /// to loopback addresses regardless of redirect behavior. So this tests
    /// the client factory directly — the same object `send_to_subscription`
    /// uses to issue the real request — against a hand-rolled local TCP
    /// server that returns a 302 pointing at an address that must never be
    /// followed.
    #[tokio::test]
    async fn push_http_client_does_not_follow_redirects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local mock server");
        let addr = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept connection");
            let mut buf = [0u8; 1024];
            // Drain (some of) the request; we don't need to parse it.
            let _ = socket.read(&mut buf).await;
            let body = "redirected";
            let response = format!(
                "HTTP/1.1 302 Found\r\n\
                 Location: http://169.254.169.254/should-not-be-followed\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write mock response");
            let _ = socket.shutdown().await;
        });

        let client = push_http_client().expect("build push http client");
        let url = format!("http://{addr}/push-endpoint");
        let response = client
            .post(&url)
            .body("payload")
            .send()
            .await
            .expect("request to local mock server should succeed");

        assert_eq!(
            response.status(),
            reqwest::StatusCode::FOUND,
            "push_http_client must return the raw 302, not follow it"
        );
        assert_eq!(
            response.url().as_str(),
            url,
            "response URL must be the original request URL — a followed \
             redirect would show the Location target instead, proving the \
             client silently sent a second request"
        );

        server.await.expect("mock server task should not panic");
    }
}
