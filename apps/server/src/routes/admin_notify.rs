// SPDX-License-Identifier: AGPL-3.0-or-later

//! Admin notification helpers for the REST server.
//!
//! Delegates to `kyomi_auth::notifications` (shared service layer) so that
//! both REST routes and Leptos server functions use the same logic.

use crate::state::AppState;

/// Send Slack + email notifications for a new user signup.
///
/// Fire-and-forget — call via `tokio::spawn`.
pub async fn notify_signup(state: &AppState, email: &str, name: &str, user_id: &str) {
    kyomi_auth::notifications::notify_signup(
        state.config.slack_feedback_webhook_url.as_deref(),
        &state.config.support_email,
        email,
        name,
        user_id,
    )
    .await;
}
