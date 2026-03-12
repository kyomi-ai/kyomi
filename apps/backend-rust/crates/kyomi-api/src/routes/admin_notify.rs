// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared admin notification helpers (Slack + email) for signups and other events.

use crate::state::AppState;

/// Send Slack + email notifications for a new user signup.
///
/// Fire-and-forget — call via `tokio::spawn`.
pub async fn notify_signup(state: &AppState, email: &str, name: &str, user_id: &str) {
    // Slack notification
    if let Err(e) = send_signup_slack(state, email, name, user_id).await {
        tracing::error!(user_id = %user_id, error = %e, "Failed to send signup Slack notification");
    }

    // Email notification to support
    send_signup_email(state, email, name, user_id).await;
}

/// Send a Slack webhook notification for a new signup.
async fn send_signup_slack(
    state: &AppState,
    email: &str,
    name: &str,
    user_id: &str,
) -> kyomi_core::Result<()> {
    let webhook_url = match state.config.slack_feedback_webhook_url.as_deref() {
        Some(url) if !url.is_empty() => url,
        _ => {
            tracing::debug!("SLACK_FEEDBACK_WEBHOOK_URL not configured, skipping signup notification");
            return Ok(());
        }
    };

    let name_display = if name.is_empty() { "Not provided" } else { name };

    let blocks = serde_json::json!([
        {
            "type": "header",
            "text": {
                "type": "plain_text",
                "text": "🎉 New User Signup!",
                "emoji": true,
            }
        },
        {
            "type": "section",
            "fields": [
                {"type": "mrkdwn", "text": format!("*Email:*\n{email}")},
                {"type": "mrkdwn", "text": format!("*Name:*\n{name_display}")},
            ]
        },
        {
            "type": "context",
            "elements": [
                {
                    "type": "mrkdwn",
                    "text": format!("User ID: `{user_id}` | {}", chrono::Utc::now().format("%Y-%m-%d %H:%M UTC")),
                }
            ]
        }
    ]);

    let payload = serde_json::json!({
        "text": format!("🎉 New user signup: {email}"),
        "blocks": blocks,
    });

    let http = kyomi_datasource_server::http_client()?;
    let response = http
        .post(webhook_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Slack webhook POST failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "Slack webhook returned {status}: {body}"
        )));
    }

    tracing::info!(user_id = %user_id, email = %email, "Slack signup notification sent");
    Ok(())
}

/// Send an email notification to the support address for a new signup.
async fn send_signup_email(state: &AppState, email: &str, name: &str, user_id: &str) {
    let email_svc = kyomi_auth::email_service::EmailService::from_env();
    if !email_svc.is_configured() {
        return;
    }

    let support_email = &state.config.support_email;
    let name_display = if name.is_empty() {
        "Not provided"
    } else {
        name
    };
    let subject = format!("New user signup: {email}");

    let sections: Vec<(&str, &str)> = vec![
        ("Email", email),
        ("Name", name_display),
        ("User ID", user_id),
    ];

    let sent = email_svc
        .send_admin_notification(support_email, &subject, &sections, Some(email))
        .await;

    if sent {
        tracing::info!(user_id = %user_id, "Signup email notification sent to {support_email}");
    } else {
        tracing::warn!(user_id = %user_id, "Failed to send signup email notification");
    }
}
