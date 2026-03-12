// SPDX-License-Identifier: AGPL-3.0-or-later

//! Push subscription service — CRUD operations for Web Push subscriptions.
//!
//! Manages browser/device push subscriptions stored in the `push_subscriptions` table.

use chrono::{DateTime, Utc};
use kyomi_core::models::PushSubscription;
use kyomi_core::DbPool;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Input for creating/updating a push subscription.
#[derive(Debug, Deserialize)]
pub struct SaveSubscriptionInput {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub user_agent: Option<String>,
    pub device_label: Option<String>,
}

/// Lightweight subscription info for the settings UI.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SubscriptionInfo {
    pub id: i32,
    pub device_label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// Save (upsert) a push subscription for a user.
///
/// Uses `ON CONFLICT (user_id, endpoint)` to update keys if the same device re-subscribes.
pub async fn save_subscription(
    db: &DbPool,
    user_id: &str,
    input: &SaveSubscriptionInput,
) -> kyomi_core::Result<PushSubscription> {
    let row = kyomi_core::db_fetch_one!(
        db,
        PushSubscription,
        r#"
        INSERT INTO push_subscriptions (user_id, endpoint, p256dh, auth, user_agent, device_label)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (user_id, endpoint)
        DO UPDATE SET
            p256dh = EXCLUDED.p256dh,
            auth = EXCLUDED.auth,
            user_agent = EXCLUDED.user_agent,
            device_label = EXCLUDED.device_label
        RETURNING id, user_id, endpoint, p256dh, auth, user_agent, device_label,
                  created_at, last_used_at, failure_count
        "#,
        user_id,
        &input.endpoint,
        &input.p256dh,
        &input.auth,
        &input.user_agent,
        &input.device_label
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to save push subscription: {e}")))?;

    info!(user_id = %user_id, endpoint_prefix = %truncate_endpoint(&input.endpoint), "Saved push subscription");
    Ok(row)
}

/// Remove a push subscription by endpoint.
pub async fn remove_subscription(
    db: &DbPool,
    user_id: &str,
    endpoint: &str,
) -> kyomi_core::Result<()> {
    kyomi_core::db_execute!(
        db,
        "DELETE FROM push_subscriptions WHERE user_id = $1 AND endpoint = $2",
        user_id,
        endpoint
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to remove push subscription: {e}")))?;

    info!(user_id = %user_id, "Removed push subscription");
    Ok(())
}

/// Remove a push subscription by ID (for settings UI delete button).
pub async fn remove_subscription_by_id(
    db: &DbPool,
    user_id: &str,
    id: i32,
) -> kyomi_core::Result<()> {
    let result = kyomi_core::db_execute!(
        db,
        "DELETE FROM push_subscriptions WHERE id = $1 AND user_id = $2",
        id,
        user_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to remove push subscription: {e}")))?;

    if result.rows_affected() == 0 {
        return Err(kyomi_core::Error::NotFound(
            "Push subscription not found".into(),
        ));
    }

    info!(user_id = %user_id, subscription_id = id, "Removed push subscription by ID");
    Ok(())
}

/// Get all push subscriptions for a user (full data, for push delivery).
pub async fn get_user_subscriptions(
    db: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<Vec<PushSubscription>> {
    let rows = kyomi_core::db_fetch_all!(
        db,
        PushSubscription,
        r#"
        SELECT id, user_id, endpoint, p256dh, auth, user_agent, device_label,
               created_at, last_used_at, failure_count
        FROM push_subscriptions
        WHERE user_id = $1
        ORDER BY created_at DESC
        "#,
        user_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get push subscriptions: {e}")))?;

    Ok(rows)
}

/// List push subscriptions for the settings UI (lightweight).
pub async fn list_user_subscriptions(
    db: &DbPool,
    user_id: &str,
) -> kyomi_core::Result<Vec<SubscriptionInfo>> {
    let rows = kyomi_core::db_fetch_all!(
        db,
        SubscriptionInfo,
        r#"
        SELECT id, device_label, created_at, last_used_at
        FROM push_subscriptions
        WHERE user_id = $1
        ORDER BY created_at DESC
        "#,
        user_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to list push subscriptions: {e}")))?;

    Ok(rows)
}

/// Record a successful push delivery — reset failure count and update last_used_at.
pub async fn record_success(db: &DbPool, id: i32) {
    let is_pg = db.is_postgres();
    let now_expr = kyomi_core::sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE push_subscriptions SET last_used_at = {now_expr}, failure_count = 0 WHERE id = $1"
    );
    if let Err(e) = kyomi_core::db_execute!(db, &sql, id) {
        warn!(subscription_id = id, error = %e, "Failed to record push success");
    }
}

/// Record a push delivery failure.
///
/// If `is_gone` is true (HTTP 410/404), the subscription is deleted immediately
/// because the browser has unsubscribed. Otherwise, the failure count is incremented.
pub async fn record_failure(db: &DbPool, id: i32, is_gone: bool) {
    if is_gone {
        // Subscription expired/unsubscribed — delete immediately
        if let Err(e) = kyomi_core::db_execute!(
            db,
            "DELETE FROM push_subscriptions WHERE id = $1",
            id
        ) {
            warn!(subscription_id = id, error = %e, "Failed to delete gone push subscription");
        } else {
            info!(subscription_id = id, "Deleted expired push subscription (410 Gone)");
        }
    } else {
        // Transient failure — increment counter
        if let Err(e) = kyomi_core::db_execute!(
            db,
            "UPDATE push_subscriptions SET failure_count = failure_count + 1 WHERE id = $1",
            id
        ) {
            warn!(subscription_id = id, error = %e, "Failed to record push failure");
        }
    }
}

/// Clean up stale push subscriptions.
///
/// Deletes subscriptions that:
/// - Have more than 5 consecutive failures, OR
/// - Have not been used in over 90 days
pub async fn cleanup_stale(db: &DbPool) {
    let cutoff = Utc::now() - chrono::Duration::days(90);

    match kyomi_core::db_execute!(
        db,
        r#"
        DELETE FROM push_subscriptions
        WHERE failure_count > 5
           OR (last_used_at IS NOT NULL AND last_used_at < $1)
           OR (last_used_at IS NULL AND created_at < $1)
        "#,
        cutoff
    ) {
        Ok(result) => {
            let count = result.rows_affected();
            if count > 0 {
                info!(count = count, "Cleaned up stale push subscriptions");
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to cleanup stale push subscriptions");
        }
    }
}

/// Truncate an endpoint URL for logging (show first 60 chars).
fn truncate_endpoint(endpoint: &str) -> String {
    if endpoint.len() <= 60 {
        endpoint.to_string()
    } else {
        format!("{}...", &endpoint[..60])
    }
}
