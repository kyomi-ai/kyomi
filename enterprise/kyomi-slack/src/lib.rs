// SPDX-License-Identifier: LicenseRef-Alytic-Enterprise

//! Kyomi Slack integration — enterprise crate.
//!
//! Contains all Slack-specific business logic:
//! - HTTP client for Slack Web API ([`client`])
//! - Markdown-to-Slack formatting helpers ([`helpers`])
//! - Block Kit message processor ([`message_processor`])
//! - Watch alert delivery via Slack ([`alert`])
//! - Axum routes for OAuth, events, commands, interactions ([`routes`])
//!
//! Implements the [`kyomi_core::platform::MessagingPlatform`] trait via [`SlackPlatform`].

pub mod alert;
pub mod client;
pub mod helpers;
pub mod message_processor;
pub mod routes;

use std::sync::Arc;

use axum::extract::FromRef;
use kyomi_auth::middleware::AuthState;
use kyomi_auth::websocket::WebSocketManager;
use kyomi_core::platform::{AlertPayload, ChannelInfo, MessagingPlatform, PlatformThread, AgentResponse};
use kyomi_core::{Config, DbPool, KVPool};
use kyomi_embed::LazyEmbedding;

use client::SlackClient;

// ---------------------------------------------------------------------------
// SlackState — axum shared state for Slack routes
// ---------------------------------------------------------------------------

/// Application state scoped to the Slack integration routes.
///
/// Created from the main `AppState` in `main.rs` (Task 5). Contains only the
/// fields that Slack handlers need, avoiding a circular dependency on `kyomi-api`.
#[derive(Clone)]
pub struct SlackState {
    pub db: DbPool,
    pub kv: KVPool,
    pub redis: Option<kyomi_core::RedisPool>,
    pub config: Arc<Config>,
    pub encryption_key: Arc<[u8; 32]>,
    pub slack_client: SlackClient,
    pub ws_manager: WebSocketManager,
    pub embedding: LazyEmbedding,
    pub connect_registry: kyomi_datasource_server::ConnectRegistry,
    pub platforms: Arc<kyomi_core::platform::PlatformRegistry>,
}

// Allow extracting AuthState from SlackState for the auth middleware.
impl FromRef<SlackState> for AuthState {
    fn from_ref(state: &SlackState) -> Self {
        AuthState {
            jwt_secret: state.config.jwt_secret.clone(),
            db: state.db.clone(),
            is_personal: false, // Slack is SaaS-only; personal mode never uses Slack routes
        }
    }
}

// ---------------------------------------------------------------------------
// SlackPlatform — MessagingPlatform implementation
// ---------------------------------------------------------------------------

/// The Slack messaging platform implementation.
///
/// Wraps a [`SlackClient`] and the configuration/state needed to send messages,
/// alerts, and list channels via the Slack Web API.
pub struct SlackPlatform {
    slack_client: SlackClient,
    db: DbPool,
    config: Arc<Config>,
    encryption_key: Arc<[u8; 32]>,
    connect_registry: Option<kyomi_datasource_server::ConnectRegistry>,
}

impl SlackPlatform {
    /// Create a new `SlackPlatform` from the shared application state.
    pub fn new(
        slack_client: SlackClient,
        db: DbPool,
        config: Arc<Config>,
        encryption_key: Arc<[u8; 32]>,
        connect_registry: Option<kyomi_datasource_server::ConnectRegistry>,
    ) -> Self {
        Self {
            slack_client,
            db,
            config,
            encryption_key,
            connect_registry,
        }
    }
}

#[async_trait::async_trait]
impl MessagingPlatform for SlackPlatform {
    fn platform_type(&self) -> &str {
        "slack"
    }

    fn display_name(&self) -> &str {
        "Slack"
    }

    async fn send_response(
        &self,
        _thread: &PlatformThread,
        _response: &AgentResponse,
    ) -> kyomi_core::Result<()> {
        // Slack message posting is handled directly by the route-level event
        // handlers (handle_app_mention / handle_direct_message) which manage
        // the full lifecycle: placeholder → agent execution → Block Kit
        // rendering → chat.update. This trait method is not used for Slack's
        // inbound message flow. Future platforms that use a simpler post-based
        // flow can implement this method directly.
        Ok(())
    }

    async fn send_alert(
        &self,
        channel_id: &str,
        alert: &AlertPayload,
    ) -> kyomi_core::Result<()> {
        let success = alert::send_slack_alert(
            &self.slack_client,
            &self.db,
            &self.encryption_key,
            &self.config,
            self.connect_registry.clone(),
            &alert.creator_user_id,
            &alert.workspace_id,
            channel_id,
            &alert.watch_name,
            &alert.alert_title,
            &alert.markdown,
            alert.execution_id,
            alert.mode,
        )
        .await;

        if success {
            Ok(())
        } else {
            Err(kyomi_core::Error::Internal(
                "Slack alert delivery failed".into(),
            ))
        }
    }

    async fn list_channels(
        &self,
        workspace_id: &str,
    ) -> kyomi_core::Result<Vec<ChannelInfo>> {
        // Get bot token from workspace_integrations via platform tables.
        let bot_token =
            routes::get_slack_bot_token(&self.db, &self.encryption_key, workspace_id).await?;

        let bot_token = match bot_token {
            Some(t) => t,
            None => return Ok(vec![]),
        };

        // Fetch channels from Slack.
        let slack_channels = self.slack_client.conversations_list(&bot_token).await?;

        Ok(slack_channels
            .into_iter()
            .map(|ch| ChannelInfo {
                id: ch.id,
                name: ch.name,
                is_private: ch.is_private,
            })
            .collect())
    }
}
