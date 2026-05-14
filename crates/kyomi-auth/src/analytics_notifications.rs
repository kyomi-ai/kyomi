// SPDX-License-Identifier: AGPL-3.0-or-later

//! Analytics quota notification dispatcher.
//!
//! Polls Redis for notification flags set by the collector and sends
//! email alerts to workspace admins.

use sqlx::PgPool;
use tracing::{info, warn};

use crate::email_service::EmailService;

/// Check for pending notification flags and dispatch emails.
///
/// Called periodically from a background task (every 30 seconds).
pub async fn dispatch_notifications(
    redis: &mut redis::aio::ConnectionManager,
    db: &PgPool,
    email: &EmailService,
    frontend_url: &str,
) {
    if let Err(e) = dispatch_inner(redis, db, email, frontend_url).await {
        warn!(error = %e, "Analytics notification dispatch failed");
    }
}

async fn dispatch_inner(
    redis: &mut redis::aio::ConnectionManager,
    db: &PgPool,
    email: &EmailService,
    frontend_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // SCAN for notification flags
    let mut cursor = 0u64;
    let mut keys_to_process: Vec<String> = Vec::new();

    loop {
        let (new_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg("analytics:notify:*")
            .arg("COUNT")
            .arg(100)
            .query_async(redis)
            .await?;

        keys_to_process.extend(keys);
        cursor = new_cursor;
        if cursor == 0 {
            break;
        }
    }

    for key in keys_to_process {
        // Parse key: analytics:notify:{workspace_id}:{YYYY-MM}:{threshold}
        let parts: Vec<&str> = key.split(':').collect();
        if parts.len() != 5 {
            continue;
        }
        let workspace_id = parts[2];
        let threshold = parts[4]; // "80", "100", or "grace"

        // Atomically rename to "notified" (marks as processed, prevents re-sending)
        let processed_key = key.replace("analytics:notify:", "analytics:notified:");
        let renamed: Result<(), _> = redis::cmd("RENAME")
            .arg(&key)
            .arg(&processed_key)
            .query_async(redis)
            .await;
        if renamed.is_err() {
            // Key was already consumed by another replica — skip
            continue;
        }

        // Look up workspace admins
        let admins: Vec<(String, String)> = sqlx::query_as(
            "SELECT u.email, u.name FROM users u \
             JOIN workspace_members wm ON wm.user_id = u.id \
             WHERE wm.workspace_id = $1 AND wm.role = 'workspace_admin' \
             AND u.email IS NOT NULL AND u.email != ''"
        )
        .bind(workspace_id)
        .fetch_all(db)
        .await
        .unwrap_or_default();

        if admins.is_empty() {
            continue;
        }

        // Look up workspace name
        let workspace_name: String = sqlx::query_scalar(
            "SELECT COALESCE(name, 'Your workspace') FROM workspaces WHERE id = $1"
        )
        .bind(workspace_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "Your workspace".into());

        let settings_url = format!("{}/settings/analytics", frontend_url);

        let (subject, status_message) = match threshold {
            "80" => (
                format!("Analytics usage at 80% — {}", workspace_name),
                format!("Your analytics usage has reached 80% of your monthly limit. Manage your analytics settings at {settings_url}"),
            ),
            "100" => (
                format!("Analytics quota reached — {}", workspace_name),
                format!("Your analytics usage has reached 100% of your monthly limit. Events are no longer being tracked. Manage your analytics settings at {settings_url}"),
            ),
            "grace" => (
                format!("Analytics events paused — {}", workspace_name),
                format!("Your analytics grace period has ended. Events are no longer being tracked. Manage your analytics settings at {settings_url}"),
            ),
            _ => continue,
        };

        let sections: Vec<(&str, &str)> = vec![
            ("Workspace", workspace_name.as_str()),
            ("Status", status_message.as_str()),
        ];

        for (admin_email, _admin_name) in &admins {
            if !email.send_admin_notification(admin_email, &subject, &sections, None).await {
                warn!(email = %admin_email, "Failed to send analytics notification email");
            }
        }

        info!(
            workspace_id = %workspace_id,
            threshold = %threshold,
            recipients = admins.len(),
            "Sent analytics quota notification"
        );
    }

    Ok(())
}
