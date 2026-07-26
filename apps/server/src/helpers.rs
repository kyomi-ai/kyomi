// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared route helpers used across multiple route modules.
//!
//! Client-IP and device-info extraction moved to
//! `kyomi_auth::request_meta` in KYO-194 — `kyomi-ui` (a library, unlike
//! this binary crate) needed to call them too, and copying them across
//! crates had already caused real divergence. Use
//! `kyomi_auth::request_meta::extract_client_ip` /
//! `kyomi_auth::request_meta::extract_device_info` directly.

/// Default value for `#[serde(default)]` on boolean fields that should default to `true`.
pub fn default_true() -> bool {
    true
}

/// Send a verification email in a background task (fire-and-forget).
pub fn spawn_verification_email(email: String, name: String, url: String) {
    tokio::spawn(async move {
        let email_svc = kyomi_auth::email_service::EmailService::from_env();
        let sent = email_svc
            .send_verification_email(&email, &name, &url)
            .await;
        if sent {
            tracing::info!("Verification email sent to {email}");
        } else {
            tracing::warn!("Failed to send verification email to {email}");
        }
    });
}
