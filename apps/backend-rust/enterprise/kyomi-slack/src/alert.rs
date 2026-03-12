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
        &config.chart_renderer_url,
    )
    .await
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Look up the Slack bot token for the workspace.
///
/// Requires the watch creator to have an active Slack connection via `platform_user_links`,
/// and the workspace to have a Slack installation in `workspace_integrations`.
async fn lookup_slack_bot_token(
    db: &DbPool,
    encryption_key: &Arc<[u8; 32]>,
    creator_user_id: &str,
    workspace_id: &str,
) -> Result<Option<String>, String> {
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
    chart_renderer_url: &str,
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
            chart_renderer_url,
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
