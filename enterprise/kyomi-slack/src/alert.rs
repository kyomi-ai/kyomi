// SPDX-License-Identifier: LicenseRef-Alytic-Enterprise

//! Slack-specific alert delivery for watch alerts/reports.
//!
//! Extracted from `kyomi-agent/src/alert.rs` — contains the Slack channel
//! membership verification, bot token lookup, and Block Kit message posting.

use std::sync::Arc;

use kyomi_core::{Config, DbPool, WatchMode};
use tracing::{error, warn};

use crate::client::SlackClient;
use crate::message_processor;
use kyomi_agent::tools::QueryContext;

// (Row types removed — bot token and user lookups now use platform tables.)

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Send a watch alert to Slack if the watch has a Slack channel configured.
///
/// This is called from the core `deliver_watch_alert` orchestrator via the
/// `MessagingPlatform::send_alert` trait method. It handles:
/// 1. Looking up the bot token from the workspace
/// 2. Verifying channel membership
/// 3. Processing the message through the Slack message processor
/// 4. Posting the Block Kit message
#[allow(clippy::too_many_arguments)]
pub async fn send_slack_alert(
    slack_client: &SlackClient,
    db: &DbPool,
    encryption_key: &Arc<[u8; 32]>,
    config: &Arc<Config>,
    connect_registry: Option<kyomi_datasource_server::ConnectRegistry>,
    creator_user_id: &str,
    workspace_id: &str,
    channel_id: &str,
    watch_name: &str,
    alert_title: &str,
    message: &str,
    execution_id: i32,
    mode: WatchMode,
) -> bool {
    // Look up bot token
    let bot_token = match lookup_slack_bot_token(db, encryption_key, creator_user_id, workspace_id)
        .await
    {
        Ok(Some(token)) => token,
        Ok(None) => {
            warn!(
                workspace_id = %workspace_id,
                "Watch has Slack channel configured but workspace has no Slack bot token"
            );
            return false;
        }
        Err(e) => {
            error!(
                workspace_id = %workspace_id,
                error = %e,
                "Failed to look up Slack bot token"
            );
            return false;
        }
    };

    // Build query context for chart rendering
    let query_ctx = QueryContext {
        db: db.clone(),
        user_id: creator_user_id.to_string(),
        workspace_id: workspace_id.to_string(),
        encryption_key: encryption_key.clone(),
        config: config.clone(),
        connect_registry,
    };

    send_watch_alert_to_slack(
        slack_client,
        &bot_token,
        channel_id,
        watch_name,
        alert_title,
        message,
        execution_id,
        &config.frontend_url,
        mode,
        &query_ctx,
    )
    .await
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Look up the Slack bot token for the workspace.
///
/// Enforces, itself, every condition required before a token is handed back
/// — it does not trust a caller to have checked them first:
/// 1. The watch creator (`creator_user_id`) is still an *active* member of
///    `workspace_id`, verified directly against `workspace_users` via
///    [`kyomi_auth::user_service::get_workspace_user`]. A `platform_user_links`
///    row can outlive membership — `remove_workspace_member` deletes it on
///    removal, but a deactivation that flips `workspace_users.active` to
///    false without deleting the row would leave it behind — so this check
///    cannot be replaced by the link check below.
/// 2. The creator has a linked Slack account via `platform_user_links`.
/// 3. The workspace has a Slack installation in `workspace_integrations`.
///
/// Any failed condition returns `Ok(None)` (fail closed) rather than an
/// error — callers already treat `Ok(None)` as "don't deliver."
async fn lookup_slack_bot_token(
    db: &DbPool,
    encryption_key: &Arc<[u8; 32]>,
    creator_user_id: &str,
    workspace_id: &str,
) -> Result<Option<String>, String> {
    // Verify the watch creator is still an active workspace member.
    // `get_workspace_user` filters on `active = true`, so `None` here
    // covers both "membership removed" and "membership deactivated."
    let active_member =
        kyomi_auth::user_service::get_workspace_user(db, workspace_id, creator_user_id)
            .await
            .map_err(|e| format!("failed to check workspace membership: {e}"))?;

    if active_member.is_none() {
        warn!(
            workspace_id = %workspace_id,
            creator_user_id = %creator_user_id,
            "Watch creator is not an active workspace member; refusing to deliver Slack alert"
        );
        return Ok(None);
    }

    // Check creator has a Slack connection via platform_user_links.
    let has_link = crate::routes::lookup_platform_slack_user(db, workspace_id, creator_user_id)
        .await
        .map_err(|e| format!("failed to check platform user link: {e}"))?;

    if has_link.is_none() {
        warn!("Watch creator has no Slack connection");
        return Ok(None);
    }

    // Get bot token from workspace_integrations config (encrypted at rest).
    let bot_token = crate::routes::get_slack_bot_token(db, encryption_key, workspace_id)
        .await
        .map_err(|e| format!("failed to get Slack bot token: {e}"))?;

    match bot_token {
        None => {
            warn!("Workspace has no Slack installation");
            Ok(None)
        }
        Some(token) => Ok(Some(token)),
    }
}

/// Verify the bot is a member of the specified Slack channel.
///
/// Returns `true` if the bot is a member (or if verification fails gracefully).
/// Returns `false` with a warning if the bot is explicitly NOT a member.
async fn verify_channel_membership(
    slack_client: &SlackClient,
    bot_token: &str,
    channel_id: &str,
) -> bool {
    match slack_client.conversations_info(bot_token, channel_id).await {
        Ok(info) => {
            if !info.is_member {
                warn!(
                    channel = %info.name,
                    "Slack bot is not a member of the channel. Add the Kyomi bot first."
                );
                return false;
            }
            true
        }
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("channel_not_found") {
                warn!(channel_id = %channel_id, "Slack channel not found or bot lacks access");
                return false;
            }
            // Don't block on other API errors — proceed anyway
            warn!(
                channel_id = %channel_id,
                error = %e,
                "Failed to verify Slack channel membership, proceeding anyway"
            );
            true
        }
    }
}

/// Send a watch alert/report to a Slack channel via [`SlackClient`].
///
/// Uses the full Slack message processor pipeline: ChartML rendering,
/// markdown tables, text chunking, and Block Kit assembly.
///
/// Returns `true` on success, `false` on failure.
#[allow(clippy::too_many_arguments)]
async fn send_watch_alert_to_slack(
    slack_client: &SlackClient,
    bot_token: &str,
    channel_id: &str,
    watch_name: &str,
    alert_title: &str,
    message: &str,
    execution_id: i32,
    frontend_url: &str,
    mode: WatchMode,
    query_ctx: &QueryContext,
) -> bool {
    let is_report = mode == WatchMode::Report;
    let type_label = if is_report { "report" } else { "alert" };

    // Verify bot is a member of the channel
    if !verify_channel_membership(slack_client, bot_token, channel_id).await {
        warn!(
            channel_id = %channel_id,
            "Cannot send Slack {type_label}: bot is not a member of the channel"
        );
        return false;
    }

    let emoji = if is_report { ":bar_chart:" } else { ":bell:" };
    let header_text = if alert_title.is_empty() {
        watch_name
    } else {
        alert_title
    };
    let footer_url = format!("{frontend_url}/watches/alerts?alert={execution_id}");
    let type_label_upper = if is_report { "Report" } else { "Alert" };
    let footer_text = format!(
        "{type_label_upper} | Execution #{execution_id} | View in Kyomi"
    );

    // Split message at table boundaries for multiple Slack messages
    let chunks = message_processor::split_message_for_multiple_tables(message);
    let total_chunks = chunks.len();
    let mut any_success = false;

    for (idx, chunk) in chunks.iter().enumerate() {
        let is_first = idx == 0;
        let is_last = idx == total_chunks - 1;

        let (blocks, fallback) = message_processor::process_and_build_slack_blocks(
            chunk,
            bot_token,
            slack_client,
            query_ctx,
            if is_last { Some(&footer_url) } else { None },
            &footer_text,
            if is_first { Some(header_text) } else { None },
            if is_first { Some(emoji) } else { None },
        )
        .await;

        match slack_client
            .post_message(bot_token, channel_id, &fallback, Some(&blocks), None)
            .await
        {
            Ok(result) => {
                if result.ok {
                    any_success = true;
                } else {
                    let slack_error = result.error.as_deref().unwrap_or("unknown_error");
                    error!(
                        channel_id = %channel_id,
                        chunk = idx,
                        error = %slack_error,
                        "Slack chat.postMessage returned error"
                    );
                }
            }
            Err(e) => {
                error!(
                    channel_id = %channel_id,
                    chunk = idx,
                    error = %e,
                    "Failed to post Slack {type_label}"
                );
            }
        }

        // Small delay between chunks to respect Slack rate limits
        if !is_last {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    any_success
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    /// Build an in-memory SQLite pool with migrations applied.
    ///
    /// Mirrors the `test_pool()` helper in `enterprise/kyomi-slack/src/routes.rs`
    /// — the established in-memory-sqlite pattern used across the workspace's
    /// unit tests.
    async fn test_pool() -> DbPool {
        let _ = kyomi_core::constants::load_with_fallback();

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await
            .expect("enable foreign keys");

        sqlx::migrate!("../../apps/server/migrations-sqlite")
            .run(&pool)
            .await
            .expect("run sqlite migrations");

        DbPool::Sqlite(pool)
    }

    /// Insert a user row with the given id. Mirrors `routes.rs`'s helper.
    async fn insert_user(pool: &DbPool, user_id: &str) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        };
        sqlx::query("INSERT INTO users (user_id, email) VALUES ($1, $2)")
            .bind(user_id)
            .bind(format!("{user_id}@test.local"))
            .execute(sq)
            .await
            .expect("insert user");
    }

    /// Insert a workspace row owned by `owner_user_id`. Mirrors `routes.rs`'s helper.
    async fn insert_workspace(pool: &DbPool, workspace_id: &str, owner_user_id: &str) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        };
        sqlx::query(
            "INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $2, $3)",
        )
        .bind(workspace_id)
        .bind(format!("Workspace {workspace_id}"))
        .bind(owner_user_id)
        .execute(sq)
        .await
        .expect("insert workspace");
    }

    /// Insert a `workspace_users` row with an explicit `active` flag.
    /// Mirrors `routes.rs`'s helper.
    async fn insert_workspace_user(
        pool: &DbPool,
        workspace_id: &str,
        user_id: &str,
        role: &str,
        active: bool,
    ) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        };
        sqlx::query(
            "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(workspace_id)
        .bind(user_id)
        .bind(role)
        .bind(active)
        .execute(sq)
        .await
        .expect("insert workspace_users row");
    }

    /// Insert a `platform_user_links` row linking `user_id` to a Slack
    /// `platform_user_id` in `workspace_id`.
    async fn insert_platform_user_link(
        pool: &DbPool,
        workspace_id: &str,
        user_id: &str,
        slack_user_id: &str,
    ) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        };
        sqlx::query(
            "INSERT INTO platform_user_links \
             (id, workspace_id, user_id, platform_type, platform_user_id) \
             VALUES ($1, $2, $3, 'slack', $4)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(workspace_id)
        .bind(user_id)
        .bind(slack_user_id)
        .execute(sq)
        .await
        .expect("insert platform_user_links row");
    }

    /// Insert a `workspace_integrations` Slack row whose `config.bot_token`
    /// is `bot_token_plaintext` encrypted with `key` — the same shape
    /// `get_slack_bot_token` (`crate::routes`) reads and decrypts.
    async fn insert_slack_integration(
        pool: &DbPool,
        workspace_id: &str,
        bot_token_plaintext: &str,
        key: &[u8; 32],
    ) {
        let sq = match pool {
            DbPool::Sqlite(sq) => sq,
            _ => unreachable!(),
        };
        let encrypted = kyomi_auth::encryption::encrypt_slack_token(bot_token_plaintext, key)
            .expect("encrypt test bot token");
        let config = serde_json::json!({ "bot_token": encrypted }).to_string();
        sqlx::query(
            "INSERT INTO workspace_integrations (id, workspace_id, platform_type, config) \
             VALUES ($1, $2, 'slack', $3)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(workspace_id)
        .bind(config)
        .execute(sq)
        .await
        .expect("insert workspace_integrations row");
    }

    /// A fixed 32-byte test key, mirroring `kyomi_auth::encryption`'s own
    /// `test_key()` helper.
    fn test_key() -> Arc<[u8; 32]> {
        let mut key = [0u8; 32];
        key[..16].copy_from_slice(b"test-key-1234567");
        key[16..].copy_from_slice(b"8901234567890123");
        Arc::new(key)
    }

    /// The regression guard: an active member with a Slack link and a
    /// workspace Slack installation must still receive a token. Silent
    /// non-delivery from an over-eager membership gate would be worse than
    /// the vulnerability this ticket closes.
    #[tokio::test]
    async fn lookup_slack_bot_token_returns_token_for_active_member() {
        let pool = test_pool().await;
        let key = test_key();
        insert_user(&pool, "user-1").await;
        insert_workspace(&pool, "ws-1", "user-1").await;
        insert_workspace_user(&pool, "ws-1", "user-1", "workspace_user", true).await;
        insert_platform_user_link(&pool, "ws-1", "user-1", "U12345").await;
        insert_slack_integration(&pool, "ws-1", "xoxb-test-token", &key).await;

        let result = lookup_slack_bot_token(&pool, &key, "user-1", "ws-1")
            .await
            .expect("lookup must not error");

        assert_eq!(
            result,
            Some("xoxb-test-token".to_string()),
            "active member with link and installation must get the real bot token"
        );
    }

    /// The load-bearing case per KYO-248: a `workspace_users` row that is
    /// present but deactivated (`active = false`) — not deleted — must be
    /// rejected exactly like a removed member, even though its
    /// `platform_user_links` row is still present (KYO-247's cleanup only
    /// fires on `remove_workspace_member`, which this case does not go
    /// through).
    #[tokio::test]
    async fn lookup_slack_bot_token_returns_none_for_deactivated_member() {
        let pool = test_pool().await;
        let key = test_key();
        insert_user(&pool, "user-1").await;
        insert_workspace(&pool, "ws-1", "user-1").await;
        insert_workspace_user(&pool, "ws-1", "user-1", "workspace_user", false).await;
        insert_platform_user_link(&pool, "ws-1", "user-1", "U12345").await;
        insert_slack_integration(&pool, "ws-1", "xoxb-test-token", &key).await;

        let result = lookup_slack_bot_token(&pool, &key, "user-1", "ws-1")
            .await
            .expect("lookup must not error");

        assert_eq!(
            result, None,
            "deactivated membership must block delivery even though the link row survives"
        );
    }

    /// A creator with no `workspace_users` row at all (fully removed, or
    /// never a member) must be rejected.
    #[tokio::test]
    async fn lookup_slack_bot_token_returns_none_when_no_workspace_user_row() {
        let pool = test_pool().await;
        let key = test_key();
        insert_user(&pool, "user-1").await;
        insert_workspace(&pool, "ws-1", "user-1").await;
        // No workspace_users row at all.
        insert_platform_user_link(&pool, "ws-1", "user-1", "U12345").await;
        insert_slack_integration(&pool, "ws-1", "xoxb-test-token", &key).await;

        let result = lookup_slack_bot_token(&pool, &key, "user-1", "ws-1")
            .await
            .expect("lookup must not error");

        assert_eq!(result, None, "missing membership row must block delivery");
    }

    /// Pre-existing branch: active member, but no Slack link at all.
    #[tokio::test]
    async fn lookup_slack_bot_token_returns_none_when_no_platform_link() {
        let pool = test_pool().await;
        let key = test_key();
        insert_user(&pool, "user-1").await;
        insert_workspace(&pool, "ws-1", "user-1").await;
        insert_workspace_user(&pool, "ws-1", "user-1", "workspace_user", true).await;
        // No platform_user_links row.
        insert_slack_integration(&pool, "ws-1", "xoxb-test-token", &key).await;

        let result = lookup_slack_bot_token(&pool, &key, "user-1", "ws-1")
            .await
            .expect("lookup must not error");

        assert_eq!(result, None, "no Slack connection must block delivery");
    }

    /// Pre-existing branch: active member with a link, but the workspace has
    /// no Slack installation.
    #[tokio::test]
    async fn lookup_slack_bot_token_returns_none_when_no_workspace_installation() {
        let pool = test_pool().await;
        let key = test_key();
        insert_user(&pool, "user-1").await;
        insert_workspace(&pool, "ws-1", "user-1").await;
        insert_workspace_user(&pool, "ws-1", "user-1", "workspace_user", true).await;
        insert_platform_user_link(&pool, "ws-1", "user-1", "U12345").await;
        // No workspace_integrations row.

        let result = lookup_slack_bot_token(&pool, &key, "user-1", "ws-1")
            .await
            .expect("lookup must not error");

        assert_eq!(result, None, "no Slack installation must block delivery");
    }
}
