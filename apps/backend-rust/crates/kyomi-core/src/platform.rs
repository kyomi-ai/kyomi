// SPDX-License-Identifier: AGPL-3.0-or-later

//! Messaging platform abstraction types.
//!
//! Defines the `MessagingPlatform` trait and supporting types that allow
//! the Kyomi agent to communicate through pluggable messaging backends
//! (Slack, Teams, Telegram, etc.) without depending on any specific one.

use std::sync::Arc;

use crate::WatchMode;

// ── Data types ──────────────────────────────────────────────────────

/// Opaque reference to a conversation thread on an external platform.
pub struct PlatformThread {
    /// Platform identifier, e.g. "slack", "teams", "telegram".
    pub platform: String,
    /// Kyomi workspace UUID.
    pub workspace_id: String,
    /// Platform-specific opaque key (e.g. Slack `channel:ts`).
    pub thread_key: String,
}

/// A rendered chart image ready for upload to a messaging platform.
pub struct RenderedChart {
    /// PNG (or other format) image bytes.
    pub image_bytes: bytes::Bytes,
    /// Optional human-readable title for the chart.
    pub title: Option<String>,
}

/// The response produced by the Kyomi agent for a messaging thread.
pub struct AgentResponse {
    /// Markdown-formatted text body.
    pub markdown: String,
    /// Zero or more chart images to attach.
    pub charts: Vec<RenderedChart>,
    /// URL to the conversation in the Kyomi web UI.
    pub web_url: String,
}

/// Payload for a watch alert notification.
pub struct AlertPayload {
    /// Human-readable watch name.
    pub watch_name: String,
    /// Short title for the alert.
    pub alert_title: String,
    /// Markdown-formatted alert body.
    pub markdown: String,
    /// Chart images to attach.
    pub charts: Vec<RenderedChart>,
    /// Database ID of the watch execution that triggered this alert.
    pub execution_id: i32,
    /// URL to the watch execution in the Kyomi web UI.
    pub web_url: String,
    /// The watch mode (scheduled, anomaly, etc.).
    pub mode: WatchMode,
    /// User ID of the watch creator (needed for bot token lookup).
    pub creator_user_id: String,
    /// Workspace ID that owns the watch (needed for bot token lookup).
    pub workspace_id: String,
}

/// Summary information about a channel on the external platform.
pub struct ChannelInfo {
    /// Platform-specific channel identifier.
    pub id: String,
    /// Human-readable channel name.
    pub name: String,
    /// Whether this is a private channel/group.
    pub is_private: bool,
}

// ── Trait ────────────────────────────────────────────────────────────

/// A pluggable messaging platform backend.
///
/// Implementations handle the platform-specific details of sending
/// messages, uploading charts, and listing channels. Route mounting
/// is handled separately to avoid circular dependencies with `AppState`.
#[async_trait::async_trait]
pub trait MessagingPlatform: Send + Sync {
    /// Short identifier for this platform, e.g. `"slack"`.
    fn platform_type(&self) -> &str;

    /// Human-readable display name, e.g. `"Slack"`.
    fn display_name(&self) -> &str;

    /// Send an agent response (text + charts) to a thread.
    async fn send_response(
        &self,
        thread: &PlatformThread,
        response: &AgentResponse,
    ) -> crate::Result<()>;

    /// Send a typing / "thinking" indicator.
    ///
    /// Returns an optional identifier that the platform can use to
    /// clear the indicator later. The default implementation is a no-op.
    async fn send_typing_indicator(
        &self,
        thread: &PlatformThread,
    ) -> crate::Result<Option<String>> {
        let _ = thread; // suppress unused warning in default impl
        Ok(None)
    }

    /// Send a watch alert to a specific channel.
    async fn send_alert(
        &self,
        channel_id: &str,
        alert: &AlertPayload,
    ) -> crate::Result<()>;

    /// List channels accessible in the given workspace.
    async fn list_channels(
        &self,
        workspace_id: &str,
    ) -> crate::Result<Vec<ChannelInfo>>;
}

// ── Registry ────────────────────────────────────────────────────────

/// Stores registered messaging platform implementations.
pub struct PlatformRegistry {
    platforms: Vec<Arc<dyn MessagingPlatform>>,
}

impl PlatformRegistry {
    pub fn new() -> Self {
        Self {
            platforms: Vec::new(),
        }
    }

    /// Register a new messaging platform.
    pub fn register(&mut self, platform: Arc<dyn MessagingPlatform>) {
        self.platforms.push(platform);
    }

    /// Look up a platform by its type identifier.
    pub fn get(&self, platform_type: &str) -> Option<&Arc<dyn MessagingPlatform>> {
        self.platforms
            .iter()
            .find(|p| p.platform_type() == platform_type)
    }

    /// Return all registered platforms.
    pub fn all(&self) -> &[Arc<dyn MessagingPlatform>] {
        &self.platforms
    }
}

impl Default for PlatformRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Database helper structs ────────────────────────────────────────

/// A row from the `watch_alert_channels` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WatchAlertChannel {
    pub id: String,
    pub watch_id: String,
    pub channel_type: String,
    pub channel_id: String,
    pub channel_name: Option<String>,
}

// ── Database helper functions ──────────────────────────────────────

/// Look up a Kyomi user_id by platform identity.
///
/// Returns `None` if no link exists for this platform user in the given workspace.
pub async fn resolve_platform_user(
    db: &crate::db::DbPool,
    platform_type: &str,
    platform_user_id: &str,
    workspace_id: &str,
) -> crate::Result<Option<String>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        user_id: String,
    }
    let row = crate::db_fetch_optional!(
        db,
        Row,
        "SELECT user_id FROM platform_user_links \
         WHERE workspace_id = $1 AND platform_type = $2 AND platform_user_id = $3",
        workspace_id,
        platform_type,
        platform_user_id
    )?;
    Ok(row.map(|r| r.user_id))
}

/// Find an existing chat session for a platform thread, or create a new one.
///
/// Returns `(session_id, is_new)` — `is_new` is true when a session was created.
pub async fn find_or_create_platform_session(
    db: &crate::db::DbPool,
    thread: &PlatformThread,
    user_id: &str,
    workspace_id: &str,
    is_shared: bool,
) -> crate::Result<(String, bool)> {
    // Check for existing session by platform thread key.
    #[derive(sqlx::FromRow)]
    struct SessionIdRow {
        session_id: String,
    }
    let existing = crate::db_fetch_optional!(
        db,
        SessionIdRow,
        "SELECT session_id FROM chat_sessions \
         WHERE platform_type = $1 AND platform_thread_key = $2",
        &thread.platform,
        &thread.thread_key
    )?;

    if let Some(row) = existing {
        return Ok((row.session_id, false));
    }

    // Create new session — match column list from existing session creation code.
    let session_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let now_str = now.to_rfc3339();
    let title: Option<&str> = None;

    crate::db_execute!(
        db,
        "INSERT INTO chat_sessions \
         (session_id, user_id, workspace_id, title, session_type, shared, \
          platform_type, platform_thread_key, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, 'chat', $5, $6, $7, $8, $9)",
        &session_id,
        user_id,
        workspace_id,
        title,
        is_shared,
        &thread.platform,
        &thread.thread_key,
        &now_str,
        &now_str
    )?;

    tracing::info!(
        session_id = %session_id,
        platform = %thread.platform,
        thread_key = %thread.thread_key,
        "Created new platform chat session"
    );

    Ok((session_id, true))
}

/// Get the workspace integration config for a given platform.
///
/// Returns `None` if no integration is installed for this platform in the workspace.
pub async fn get_workspace_integration(
    db: &crate::db::DbPool,
    workspace_id: &str,
    platform_type: &str,
) -> crate::Result<Option<serde_json::Value>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        config: String,
    }
    let config_expr = crate::sql_compat::cast_to_text(db.is_postgres(), "config");
    let sql = format!(
        "SELECT {config_expr} AS config FROM workspace_integrations \
         WHERE workspace_id = $1 AND platform_type = $2"
    );
    let row = crate::db_fetch_optional!(
        db,
        Row,
        &sql,
        workspace_id,
        platform_type
    )?;
    match row {
        Some(r) => {
            let val: serde_json::Value = serde_json::from_str(&r.config)
                .map_err(|e| crate::Error::Internal(format!("invalid JSON in workspace_integrations config: {e}")))?;
            Ok(Some(val))
        }
        None => Ok(None),
    }
}

/// Insert or update a workspace integration.
pub async fn upsert_workspace_integration(
    db: &crate::db::DbPool,
    workspace_id: &str,
    platform_type: &str,
    config: &serde_json::Value,
    installed_by: &str,
) -> crate::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let config_str = serde_json::to_string(config)
        .map_err(|e| crate::Error::Internal(format!("JSON serialization failed: {e}")))?;
    let now = chrono::Utc::now().to_rfc3339();

    crate::db_execute!(
        db,
        "INSERT INTO workspace_integrations (id, workspace_id, platform_type, config, installed_by, installed_at) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (workspace_id, platform_type) \
         DO UPDATE SET config = $4, installed_by = $5",
        &id,
        workspace_id,
        platform_type,
        &config_str,
        installed_by,
        &now
    )?;
    Ok(())
}

/// Delete a workspace integration.
pub async fn delete_workspace_integration(
    db: &crate::db::DbPool,
    workspace_id: &str,
    platform_type: &str,
) -> crate::Result<()> {
    crate::db_execute!(
        db,
        "DELETE FROM workspace_integrations WHERE workspace_id = $1 AND platform_type = $2",
        workspace_id,
        platform_type
    )?;
    Ok(())
}

/// Get the per-user integration config for a given platform.
///
/// Returns `None` if no user integration exists.
pub async fn get_user_integration(
    db: &crate::db::DbPool,
    workspace_id: &str,
    user_id: &str,
    platform_type: &str,
) -> crate::Result<Option<serde_json::Value>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        config: String,
    }
    let config_expr = crate::sql_compat::cast_to_text(db.is_postgres(), "config");
    let sql = format!(
        "SELECT {config_expr} AS config FROM workspace_user_integrations \
         WHERE workspace_id = $1 AND user_id = $2 AND platform_type = $3"
    );
    let row = crate::db_fetch_optional!(
        db,
        Row,
        &sql,
        workspace_id,
        user_id,
        platform_type
    )?;
    match row {
        Some(r) => {
            let val: serde_json::Value = serde_json::from_str(&r.config)
                .map_err(|e| crate::Error::Internal(format!("invalid JSON in workspace_user_integrations config: {e}")))?;
            Ok(Some(val))
        }
        None => Ok(None),
    }
}

/// Insert or update a per-user integration.
pub async fn upsert_user_integration(
    db: &crate::db::DbPool,
    workspace_id: &str,
    user_id: &str,
    platform_type: &str,
    config: &serde_json::Value,
) -> crate::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let config_str = serde_json::to_string(config)
        .map_err(|e| crate::Error::Internal(format!("JSON serialization failed: {e}")))?;

    crate::db_execute!(
        db,
        "INSERT INTO workspace_user_integrations (id, workspace_id, user_id, platform_type, config) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (workspace_id, user_id, platform_type) \
         DO UPDATE SET config = $5",
        &id,
        workspace_id,
        user_id,
        platform_type,
        &config_str
    )?;
    Ok(())
}

/// Delete a per-user integration.
pub async fn delete_user_integration(
    db: &crate::db::DbPool,
    workspace_id: &str,
    user_id: &str,
    platform_type: &str,
) -> crate::Result<()> {
    crate::db_execute!(
        db,
        "DELETE FROM workspace_user_integrations \
         WHERE workspace_id = $1 AND user_id = $2 AND platform_type = $3",
        workspace_id,
        user_id,
        platform_type
    )?;
    Ok(())
}

/// Get all alert channels configured for a watch.
pub async fn get_watch_alert_channels(
    db: &crate::db::DbPool,
    watch_id: &str,
) -> crate::Result<Vec<WatchAlertChannel>> {
    let rows = crate::db_fetch_all!(
        db,
        WatchAlertChannel,
        "SELECT id, watch_id, channel_type, channel_id, channel_name \
         FROM watch_alert_channels WHERE watch_id = $1",
        watch_id
    )?;
    Ok(rows)
}

/// Set (upsert) an alert channel for a watch.
///
/// Uses `ON CONFLICT (watch_id, channel_type)` — each watch can have
/// at most one channel per platform type.
pub async fn set_watch_alert_channel(
    db: &crate::db::DbPool,
    watch_id: &str,
    channel_type: &str,
    channel_id: &str,
    channel_name: Option<&str>,
) -> crate::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();

    crate::db_execute!(
        db,
        "INSERT INTO watch_alert_channels (id, watch_id, channel_type, channel_id, channel_name) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (watch_id, channel_type) \
         DO UPDATE SET channel_id = $4, channel_name = $5",
        &id,
        watch_id,
        channel_type,
        channel_id,
        channel_name
    )?;
    Ok(())
}

/// Remove an alert channel from a watch by platform type.
pub async fn remove_watch_alert_channel(
    db: &crate::db::DbPool,
    watch_id: &str,
    channel_type: &str,
) -> crate::Result<()> {
    crate::db_execute!(
        db,
        "DELETE FROM watch_alert_channels WHERE watch_id = $1 AND channel_type = $2",
        watch_id,
        channel_type
    )?;
    Ok(())
}
