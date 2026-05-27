// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat service — CRUD operations for chat sessions, messages, query cache, and charts.
//!
//! Wire-compatible with Python's `PostgreSQLChatHistoryStore` and the chat router.
//! All functions are stateless and take a DB pool reference (and encryption key
//! where encrypted fields are involved).
//!
//! Encrypted fields: `chat_messages.content` and `chat_messages.extra_metadata`
//! are stored as AES-256-GCM ciphertext in the DB. This service encrypts on
//! write and decrypts on read using the functions from `crate::encryption`.

use chrono::Utc;
use kyomi_core::DbPool;
use kyomi_core::db::in_clause_placeholders;
use kyomi_core::models::Chart;
use serde::{Deserialize, Serialize};

use crate::encryption;
use crate::sync_log_service;
use kyomi_types::CreatedBy;
use kyomi_types::sync::{SyncActionType, entity_types};

// ---------------------------------------------------------------------------
// Response structs
// ---------------------------------------------------------------------------

/// Summary of a session for list endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListItem {
    pub session_id: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub session_type: String,
    pub shared: bool,
    pub shared_at: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub message_count: i64,
    pub pinned_count: i64,
    pub unread_count: i64,
    pub created_by: Option<CreatedBy>,
    pub platform_type: Option<String>,
}

/// Full session detail (for get_session / get_session_info).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub user_id: String,
    pub workspace_id: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub shared: bool,
    pub shared_at: Option<String>,
    pub created_by: Option<CreatedBy>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub config: Option<serde_json::Value>,
    pub platform_type: Option<String>,
    pub platform_thread_key: Option<String>,
}

/// A UI-visible message (user or final assistant).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageItem {
    pub message_id: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub content: String,
    pub timestamp: Option<String>,
    pub model: Option<String>,
    pub pinned: bool,
    pub metadata: serde_json::Value,
    pub status: String,
    pub thinking_events: Vec<serde_json::Value>,
    pub token_usage: Option<serde_json::Value>,
    pub current_time_user_tz: Option<String>,
    pub sent_by_user_id: Option<String>,
    pub sent_by: Option<CreatedBy>,
}

// ---------------------------------------------------------------------------
// Internal row types for joined queries
// ---------------------------------------------------------------------------

/// Session row joined with user info (for created_by).
#[derive(Debug, sqlx::FromRow)]
struct SessionWithUserRow {
    // session fields
    session_id: String,
    user_id: String,
    workspace_id: String,
    title: Option<String>,
    model: Option<String>,
    session_type: String,
    #[sqlx(default)]
    shared: bool,
    shared_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    config: Option<serde_json::Value>,
    platform_type: Option<String>,
    platform_thread_key: Option<String>,
    // joined user fields
    owner_user_id: String,
    owner_display_name: String,
}

/// Aggregate counts row for batch message/pinned counts.
#[derive(Debug, sqlx::FromRow)]
struct SessionCountRow {
    session_id: String,
    message_count: i64,
    pinned_count: i64,
}

/// Message row joined with sender user info.
#[derive(Debug, sqlx::FromRow)]
struct MessageWithSenderRow {
    message_id: String,
    role: String,
    content: String,
    #[sqlx(default)]
    pinned: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    current_time_user_tz: Option<String>,
    extra_metadata: Option<String>,
    sent_by_user_id: Option<String>,
    // Joined sender fields (nullable because assistant msgs have no sender).
    sender_name: Option<String>,
    sender_email: Option<String>,
}

/// Row for unread count calculation.
#[derive(Debug, sqlx::FromRow)]
struct ReadStatusRow {
    session_id: String,
    last_read_message_id: Option<String>,
}

/// Row for last-read timestamp lookup.
#[derive(Debug, sqlx::FromRow)]
struct MessageTimestampRow {
    message_id: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// Row for unread count result.
#[derive(Debug, sqlx::FromRow)]
struct UnreadCountRow {
    session_id: Option<String>,
    unread_count: i64,
}

/// Row for scalar session_id lookups.
#[derive(Debug, sqlx::FromRow)]
struct SessionIdRow {
    session_id: String,
}

/// Row for query cache SQL value.
#[derive(Debug, sqlx::FromRow)]
struct QueryCacheRow {
    sql: String,
}

/// Row for scalar created_at timestamp.
#[derive(Debug, sqlx::FromRow)]
struct CreatedAtRow {
    created_at: chrono::DateTime<chrono::Utc>,
}

// ─── Sync snapshot helpers ────────────────────────────────────────────────────

/// Row for building a session metadata snapshot.
#[derive(Debug, sqlx::FromRow)]
struct SessionSnapshotRow {
    session_id: String,
    user_id: String,
    workspace_id: String,
    title: Option<String>,
    session_type: String,
    updated_at: String,
    created_at: String,
}

/// Build a JSON snapshot for the sync log from a live session row.
///
/// Returns `(workspace_id, snapshot_json)`. Returns `None` if the session is
/// not found or if the query fails.
pub(crate) async fn fetch_session_snapshot(
    db: &DbPool,
    session_id: &str,
) -> Option<(String, serde_json::Value)> {
    let row = kyomi_core::db_fetch_optional!(
        db,
        SessionSnapshotRow,
        r#"SELECT session_id, user_id, workspace_id, title, session_type,
                  CAST(updated_at AS TEXT) AS updated_at,
                  CAST(created_at AS TEXT) AS created_at
           FROM chat_sessions
           WHERE session_id = $1 AND session_type = 'chat'"#,
        session_id
    )
    .ok()?;

    let row = row?;
    let workspace_id = row.workspace_id.clone();
    let json = serde_json::json!({
        "session_id": row.session_id,
        "user_id": row.user_id,
        "workspace_id": row.workspace_id,
        "title": row.title,
        "session_type": row.session_type,
        "updated_at": row.updated_at,
        "created_at": row.created_at,
    });
    Some((workspace_id, json))
}

/// Fetch session counts for a list of session IDs.
///
/// Uses `= ANY($1)` on Postgres and individual placeholders on SQLite.
async fn fetch_session_counts(
    db: &DbPool,
    session_ids: &[String],
) -> Result<Vec<SessionCountRow>, sqlx::Error> {
    if session_ids.is_empty() {
        return Ok(Vec::new());
    }

    match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            // Postgres: use = ANY($1) with array bind, and FILTER for pinned count
            sqlx::query_as::<_, SessionCountRow>(
                "SELECT \
                   cm.session_id, \
                   COUNT(cm.message_id) AS message_count, \
                   COUNT(cm.message_id) FILTER (WHERE cm.pinned = true) AS pinned_count \
                 FROM chat_messages cm \
                 WHERE cm.session_id = ANY($1) \
                 GROUP BY cm.session_id",
            )
            .bind(session_ids)
            .fetch_all(pg)
            .await
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            // SQLite: use IN clause with individual binds, SUM(CASE) for pinned count
            let (in_clause, _) = in_clause_placeholders(session_ids.len(), 1);
            let sql = format!(
                "SELECT \
                   cm.session_id, \
                   COUNT(cm.message_id) AS message_count, \
                   SUM(CASE WHEN cm.pinned = 1 THEN 1 ELSE 0 END) AS pinned_count \
                 FROM chat_messages cm \
                 WHERE cm.session_id IN {in_clause} \
                 GROUP BY cm.session_id"
            );
            let mut query = sqlx::query_as::<_, SessionCountRow>(&sql);
            for sid in session_ids {
                query = query.bind(sid);
            }
            query.fetch_all(sq).await
        }
    }
}

/// Fetch message timestamps for a list of message IDs.
async fn fetch_message_timestamps(
    db: &DbPool,
    message_ids: &[String],
) -> Result<Vec<MessageTimestampRow>, sqlx::Error> {
    if message_ids.is_empty() {
        return Ok(Vec::new());
    }

    match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            sqlx::query_as::<_, MessageTimestampRow>(
                "SELECT message_id, created_at FROM chat_messages WHERE message_id = ANY($1)",
            )
            .bind(message_ids)
            .fetch_all(pg)
            .await
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let (in_clause, _) = in_clause_placeholders(message_ids.len(), 1);
            let sql = format!(
                "SELECT message_id, created_at FROM chat_messages WHERE message_id IN {in_clause}"
            );
            let mut query = sqlx::query_as::<_, MessageTimestampRow>(&sql);
            for mid in message_ids {
                query = query.bind(mid);
            }
            query.fetch_all(sq).await
        }
    }
}

// ---------------------------------------------------------------------------
// Session management
// ---------------------------------------------------------------------------

/// Resolve the LLM model to use for a new session.
///
/// Fallback chain: explicit caller value → `LLM_MODEL` env var → `"unknown"`.
fn resolve_model(model: Option<&str>) -> String {
    model
        .map(|s| s.to_string())
        .or_else(|| std::env::var("LLM_MODEL").ok())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Create a new chat session. Returns the generated session_id.
///
/// Pass `None` for `model` to use the `LLM_MODEL` env var (or the built-in default).
pub async fn create_session(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
    model: Option<&str>,
) -> kyomi_core::Result<String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now();
    let empty_config = serde_json::json!({});
    let model = resolve_model(model);

    kyomi_core::db_execute!(
        db,
        "INSERT INTO chat_sessions \
         (session_id, user_id, workspace_id, title, model, session_type, \
          shared, created_at, updated_at, config) \
         VALUES ($1, $2, $3, NULL, $4, 'chat', \
                 false, $5, $6, $7)",
        &session_id,
        user_id,
        workspace_id,
        &model,
        now,
        now,
        empty_config
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to create session: {e}")))?;

    // Sync log — best-effort: log a warning and continue on failure.
    {
        let snapshot = serde_json::json!({
            "session_id": session_id,
            "user_id": user_id,
            "workspace_id": workspace_id,
            "title": serde_json::Value::Null,
            "updated_at": now.to_rfc3339(),
            "created_at": now.to_rfc3339(),
        });
        if let Err(e) = sync_log_service::write_sync_entry(
            db,
            entity_types::CHAT_SESSION,
            &session_id,
            workspace_id,
            SyncActionType::Insert,
            Some(snapshot),
        )
        .await
        {
            tracing::warn!(error = %e, session_id = %session_id, "Failed to write sync log entry");
        }
    }

    Ok(session_id)
}

/// Create a session with caller-provided ID, title, and session_type.
///
/// Pass `None` for `model` to use the `LLM_MODEL` env var (or the built-in default).
pub async fn create_session_with_id(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
    session_id: &str,
    title: Option<&str>,
    session_type: &str,
    model: Option<&str>,
) -> kyomi_core::Result<()> {
    let now = Utc::now();
    let empty_config = serde_json::json!({});
    let model = resolve_model(model);

    kyomi_core::db_execute!(
        db,
        "INSERT INTO chat_sessions \
         (session_id, user_id, workspace_id, title, model, session_type, \
          shared, created_at, updated_at, config) \
         VALUES ($1, $2, $3, $4, $5, $6, \
                 false, $7, $8, $9)",
        session_id,
        user_id,
        workspace_id,
        title,
        &model,
        session_type,
        now,
        now,
        empty_config
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to create session: {e}")))?;

    Ok(())
}

/// List sessions for a user, including shared sessions from workspace members.
///
/// Uses batch queries for message/pinned counts to avoid N+1.
/// For shared sessions owned by others, calculates `unread_count` from
/// `conversation_read_status`.
pub async fn get_user_sessions(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
    limit: i64,
    offset: i64,
    pinned_only: bool,
    session_type: &str,
) -> kyomi_core::Result<Vec<SessionListItem>> {
    let is_pg = db.is_postgres();
    let bool_true_val = kyomi_core::sql_compat::bool_true(is_pg);

    // Step 1: Fetch sessions (own + shared in workspace), joined with user for created_by.
    let sql = format!(
        r#"SELECT
           cs.session_id, cs.user_id, cs.workspace_id, cs.title, cs.model,
           cs.session_type,
           cs.shared,
           cs.shared_at, cs.created_at, cs.updated_at,
           cs.config, cs.platform_type, cs.platform_thread_key,
           u.user_id AS owner_user_id,
           COALESCE(u.name, u.email) AS owner_display_name
         FROM chat_sessions cs
         JOIN users u ON cs.user_id = u.user_id
         WHERE cs.session_type = $1
           AND (cs.user_id = $2 OR (cs.shared = {bool_true_val} AND cs.workspace_id = $3))
         ORDER BY cs.updated_at DESC
         LIMIT $4 OFFSET $5"#
    );

    let sessions = kyomi_core::db_fetch_all!(
        db,
        SessionWithUserRow,
        &sql,
        session_type,
        user_id,
        workspace_id,
        limit,
        offset
    )?;

    if sessions.is_empty() {
        return Ok(Vec::new());
    }

    let session_ids: Vec<String> = sessions.iter().map(|s| s.session_id.clone()).collect();

    // Step 2: Batch query message + pinned counts.
    let counts = fetch_session_counts(db, &session_ids).await?;

    let count_map: std::collections::HashMap<&str, (i64, i64)> = counts
        .iter()
        .map(|c| (c.session_id.as_str(), (c.message_count, c.pinned_count)))
        .collect();

    // Step 3: Load conversation read status for this user (for unread counts).
    let read_statuses = kyomi_core::db_fetch_all!(
        db,
        ReadStatusRow,
        "SELECT session_id, last_read_message_id \
         FROM conversation_read_status \
         WHERE user_id = $1",
        user_id
    )?;

    let read_status_map: std::collections::HashMap<&str, Option<&str>> = read_statuses
        .iter()
        .map(|r| (r.session_id.as_str(), r.last_read_message_id.as_deref()))
        .collect();

    // Step 4: Pre-compute last-read timestamps for unread count calculation.
    let last_read_msg_ids: Vec<String> = read_statuses
        .iter()
        .filter_map(|r| r.last_read_message_id.clone())
        .collect();

    let last_read_timestamps = fetch_message_timestamps(db, &last_read_msg_ids).await?;

    let timestamp_map: std::collections::HashMap<&str, chrono::DateTime<chrono::Utc>> =
        last_read_timestamps
            .iter()
            .map(|t| (t.message_id.as_str(), t.created_at))
            .collect();

    // Step 5: For shared sessions with a last_read timestamp, calculate unread counts.
    let mut unread_queries: Vec<(&str, chrono::DateTime<chrono::Utc>)> = Vec::new();
    let mut never_read_sessions: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for s in &sessions {
        if s.shared && s.user_id != user_id {
            if let Some(last_read_msg_id) = read_status_map
                .get(s.session_id.as_str())
                .copied()
                .flatten()
            {
                if let Some(&ts) = timestamp_map.get(last_read_msg_id) {
                    unread_queries.push((s.session_id.as_str(), ts));
                }
            } else {
                never_read_sessions.insert(s.session_id.as_str());
            }
        }
    }

    // Compute unread counts.
    let mut unread_map: std::collections::HashMap<String, i64> = std::collections::HashMap::new();

    if !unread_queries.is_empty() {
        // For each (session_id, cutoff_ts), count messages created after the cutoff.
        // Postgres can use UNNEST for a single query; SQLite does individual queries.
        match db {
            kyomi_core::db::DbPool::Postgres(pg) => {
                let sids: Vec<String> = unread_queries
                    .iter()
                    .map(|(sid, _)| sid.to_string())
                    .collect();
                let cutoffs: Vec<chrono::DateTime<chrono::Utc>> =
                    unread_queries.iter().map(|(_, ts)| *ts).collect();

                let rows = sqlx::query_as::<_, UnreadCountRow>(
                    "SELECT u.session_id, COUNT(cm.message_id) AS unread_count \
                     FROM UNNEST($1::text[], $2::timestamptz[]) AS u(session_id, cutoff) \
                     JOIN chat_messages cm ON cm.session_id = u.session_id AND cm.created_at > u.cutoff \
                     GROUP BY u.session_id",
                )
                .bind(&sids)
                .bind(&cutoffs)
                .fetch_all(pg)
                .await?;

                for row in rows {
                    if let Some(sid) = row.session_id {
                        unread_map.insert(sid, row.unread_count);
                    }
                }
            }
            kyomi_core::db::DbPool::Sqlite(sq) => {
                // SQLite: individual queries per session (N queries, but N is small)
                for (sid, cutoff) in &unread_queries {
                    let count: i64 = sqlx::query_scalar::<_, i64>(
                        "SELECT COUNT(*) FROM chat_messages \
                         WHERE session_id = $1 AND created_at > $2",
                    )
                    .bind(sid)
                    .bind(cutoff)
                    .fetch_one(sq)
                    .await
                    .unwrap_or(0);
                    if count > 0 {
                        unread_map.insert(sid.to_string(), count);
                    }
                }
            }
        }
    }

    // Step 6: Build result.
    let mut result = Vec::with_capacity(sessions.len());
    for s in &sessions {
        let (message_count, pinned_count) = count_map
            .get(s.session_id.as_str())
            .copied()
            .unwrap_or((0, 0));

        // Skip if pinned_only and no pinned messages.
        if pinned_only && pinned_count == 0 {
            continue;
        }

        let unread_count = if never_read_sessions.contains(s.session_id.as_str()) {
            message_count
        } else {
            unread_map.get(s.session_id.as_str()).copied().unwrap_or(0)
        };

        result.push(SessionListItem {
            session_id: s.session_id.clone(),
            title: s.title.clone(),
            model: s.model.clone(),
            session_type: s.session_type.clone(),
            shared: s.shared,
            shared_at: s.shared_at.map(|dt| dt.to_rfc3339()),
            created_at: Some(s.created_at.to_rfc3339()),
            updated_at: Some(s.updated_at.to_rfc3339()),
            message_count,
            pinned_count,
            unread_count,
            created_by: Some(CreatedBy {
                user_id: s.owner_user_id.clone(),
                display_name: Some(s.owner_display_name.clone()),
                ..Default::default()
            }),
            platform_type: s.platform_type.clone(),
        });
    }

    Ok(result)
}

/// Get a single session with user info (for `created_by`).
pub async fn get_session(
    db: &DbPool,
    session_id: &str,
) -> kyomi_core::Result<Option<SessionMetadata>> {
    let row = kyomi_core::db_fetch_optional!(
        db,
        SessionWithUserRow,
        r#"SELECT
           cs.session_id, cs.user_id, cs.workspace_id, cs.title, cs.model,
           cs.session_type,
           cs.shared,
           cs.shared_at, cs.created_at, cs.updated_at,
           cs.config, cs.platform_type, cs.platform_thread_key,
           u.user_id AS owner_user_id,
           COALESCE(u.name, u.email) AS owner_display_name
         FROM chat_sessions cs
         JOIN users u ON cs.user_id = u.user_id
         WHERE cs.session_id = $1"#,
        session_id
    )?;

    Ok(row.map(session_row_to_detail))
}

/// Get session with permission check: owner OR shared-in-workspace.
pub async fn get_session_info(
    db: &DbPool,
    user_id: &str,
    session_id: &str,
    workspace_id: Option<&str>,
) -> kyomi_core::Result<Option<SessionMetadata>> {
    let detail = match get_session(db, session_id).await? {
        Some(d) => d,
        None => return Ok(None),
    };

    let is_owner = detail.user_id == user_id;
    let is_shared_in_workspace = workspace_id
        .map(|wid| detail.workspace_id == wid && detail.shared)
        .unwrap_or(false);

    if is_owner || is_shared_in_workspace {
        Ok(Some(detail))
    } else {
        Ok(None)
    }
}

/// Get UI-visible messages: user messages + final assistant messages (no tool_calls).
///
/// Decrypts `content` and `extra_metadata`.
pub async fn get_session_messages(
    db: &DbPool,
    encryption_key: &[u8; 32],
    session_id: &str,
    limit: i64,
) -> kyomi_core::Result<Vec<MessageItem>> {
    // Fetch user messages and assistant messages WITHOUT tool_calls (final responses).
    // tool_calls IS NULL filters out intermediate assistant messages that have tool calls.
    let rows = kyomi_core::db_fetch_all!(
        db,
        MessageWithSenderRow,
        r#"SELECT
           cm.message_id, cm.role, cm.content,
           cm.pinned,
           cm.created_at,
           cm.current_time_user_tz, cm.extra_metadata, cm.sent_by_user_id,
           u.name AS sender_name, u.email AS sender_email
         FROM chat_messages cm
         LEFT JOIN users u ON cm.sent_by_user_id = u.user_id
         WHERE cm.session_id = $1
           AND (cm.role = 'user' OR (cm.role = 'assistant' AND cm.tool_calls IS NULL))
         ORDER BY cm.created_at ASC
         LIMIT $2"#,
        session_id,
        limit
    )?;

    let mut result = Vec::with_capacity(rows.len());

    for row in rows {
        // Decrypt content — skip messages with corrupt ciphertext rather than
        // failing the entire session load.
        let content = match encryption::decrypt(&row.content, encryption_key) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    message_id = %row.message_id,
                    "Skipping message with corrupt content: {e}"
                );
                continue;
            }
        };

        // Decrypt extra_metadata to JSON (if present).
        let metadata: serde_json::Value = match &row.extra_metadata {
            Some(enc_meta) => match encryption::decrypt_json(enc_meta, encryption_key) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(
                        message_id = %row.message_id,
                        "Skipping message with corrupt metadata: {e}"
                    );
                    continue;
                }
            },
            None => serde_json::Value::Object(serde_json::Map::new()),
        };

        // Extract fields from metadata.
        let model = metadata
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let thinking_events = metadata
            .get("thinking_events")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let token_usage = metadata.get("token_usage").cloned();

        let sent_by = if let Some(ref sender_user_id) = row.sent_by_user_id {
            let display_name = row
                .sender_name
                .clone()
                .unwrap_or_else(|| row.sender_email.clone().unwrap_or_default());
            Some(CreatedBy {
                user_id: sender_user_id.clone(),
                display_name: Some(display_name),
                ..Default::default()
            })
        } else {
            None
        };

        // Strip the metadata prefix from user messages for UI display.
        // The DB stores the decorated content (e.g. "[source: web, user_local_time: ...] actual message")
        // to preserve LLM prompt cache hits. The UI should see only the raw message.
        let display_content = if row.role == "user" && content.starts_with('[') {
            if let Some(pos) = content.find("] ") {
                // Verify it looks like our metadata prefix (starts with "[source:" or "[user_local_time:")
                let prefix = &content[1..pos];
                if prefix.contains("source:") || prefix.contains("user_local_time:") {
                    content[pos + 2..].to_string()
                } else {
                    content
                }
            } else {
                content
            }
        } else {
            content
        };

        result.push(MessageItem {
            message_id: row.message_id,
            message_type: row.role,
            content: display_content,
            timestamp: Some(row.created_at.to_rfc3339()),
            model,
            pinned: row.pinned,
            metadata,
            status: "completed".to_string(),
            thinking_events,
            token_usage,
            current_time_user_tz: row.current_time_user_tz,
            sent_by_user_id: row.sent_by_user_id,
            sent_by,
        });
    }

    Ok(result)
}

/// Add a message to a session. Encrypts content and metadata before storage.
///
/// Returns the message_id.
#[allow(clippy::too_many_arguments)]
pub async fn add_message(
    db: &DbPool,
    encryption_key: &[u8; 32],
    session_id: &str,
    role: &str,
    content: &str,
    metadata: Option<&serde_json::Value>,
    message_id: Option<&str>,
    current_time_user_tz: Option<&str>,
    sent_by_user_id: Option<&str>,
    tool_call_id: Option<&str>,
    tool_name: Option<&str>,
    tool_calls: Option<&serde_json::Value>,
) -> kyomi_core::Result<String> {
    let msg_id = message_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let now = Utc::now();

    // Encrypt content.
    let encrypted_content = encryption::encrypt(content, encryption_key)?;

    // Encrypt metadata (if present).
    let encrypted_metadata: Option<String> = match metadata {
        Some(m) => Some(encryption::encrypt_json(m, encryption_key)?),
        None => None,
    };

    // Insert message.
    kyomi_core::db_execute!(
        db,
        "INSERT INTO chat_messages \
         (message_id, session_id, role, content, sent_by_user_id, pinned, \
          created_at, current_time_user_tz, extra_metadata, \
          tool_call_id, tool_name, tool_calls) \
         VALUES ($1, $2, $3, $4, $5, false, $6, $7, $8, $9, $10, $11)",
        &msg_id,
        session_id,
        role,
        &encrypted_content,
        sent_by_user_id,
        now,
        current_time_user_tz,
        encrypted_metadata,
        tool_call_id,
        tool_name,
        tool_calls as Option<&serde_json::Value>
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to add message: {e}")))?;

    // Update session timestamp.
    kyomi_core::db_execute!(
        db,
        "UPDATE chat_sessions SET updated_at = $1 WHERE session_id = $2",
        now,
        session_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to update session timestamp: {e}")))?;

    // Sync log for the session metadata update — best-effort.
    if let Some((workspace_id, snapshot)) = fetch_session_snapshot(db, session_id).await
        && let Err(e) = sync_log_service::write_sync_entry(
            db,
            entity_types::CHAT_SESSION,
            session_id,
            &workspace_id,
            SyncActionType::Update,
            Some(snapshot),
        )
        .await
    {
        tracing::warn!(error = %e, session_id = %session_id, "Failed to write sync log entry");
    }

    Ok(msg_id)
}

/// Update message content and/or metadata (re-encrypts).
pub async fn update_message(
    db: &DbPool,
    encryption_key: &[u8; 32],
    message_id: &str,
    content: Option<&str>,
    metadata: Option<&serde_json::Value>,
) -> kyomi_core::Result<bool> {
    // Dynamic SQL — cannot use dispatch macros
    let mut set_parts: Vec<String> = Vec::new();
    let mut param_idx = 2u32; // $1 = message_id

    if content.is_some() {
        set_parts.push(format!("content = ${param_idx}"));
        param_idx += 1;
    }
    if metadata.is_some() {
        set_parts.push(format!("extra_metadata = ${param_idx}"));
        // param_idx incremented but not used further
    }

    if set_parts.is_empty() {
        return Ok(false);
    }

    let sql = format!(
        "UPDATE chat_messages SET {} WHERE message_id = $1",
        set_parts.join(", ")
    );

    // Dynamic SQL — encrypt fields before binding so both arms are identical.
    let encrypted_content: Option<String> = match content {
        Some(c) => Some(encryption::encrypt(c, encryption_key)?),
        None => None,
    };
    let encrypted_metadata_dyn: Option<String> = match metadata {
        Some(m) => Some(encryption::encrypt_json(m, encryption_key)?),
        None => None,
    };
    let rows_affected = kyomi_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query(&sql).bind(message_id);
        if let Some(ref enc) = encrypted_content {
            query = query.bind(enc);
        }
        if let Some(ref enc) = encrypted_metadata_dyn {
            query = query.bind(enc);
        }
        query.execute(p).await.map(|r| r.rows_affected())
    })
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to update message: {e}")))?;
    Ok(rows_affected > 0)
}

/// Update session title, model, and/or config.
pub async fn update_session(
    db: &DbPool,
    session_id: &str,
    title: Option<&str>,
    model: Option<&str>,
    config: Option<&serde_json::Value>,
) -> kyomi_core::Result<bool> {
    // Dynamic SQL — cannot use dispatch macros
    let now = Utc::now();
    let mut set_parts: Vec<String> = Vec::new();
    let mut param_idx = 2u32; // $1 = session_id

    if title.is_some() {
        set_parts.push(format!("title = ${param_idx}"));
        param_idx += 1;
    }
    if model.is_some() {
        set_parts.push(format!("model = ${param_idx}"));
        param_idx += 1;
    }
    if config.is_some() {
        set_parts.push(format!("config = ${param_idx}"));
        param_idx += 1;
    }

    set_parts.push(format!("updated_at = ${param_idx}"));

    let sql = format!(
        "UPDATE chat_sessions SET {} WHERE session_id = $1",
        set_parts.join(", ")
    );

    let rows_affected = kyomi_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query(&sql).bind(session_id);
        if let Some(t) = title {
            query = query.bind(t);
        }
        if let Some(m) = model {
            query = query.bind(m);
        }
        if let Some(c) = config {
            query = query.bind(c);
        }
        query = query.bind(now);
        query.execute(p).await.map(|r| r.rows_affected())
    })
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to update session: {e}")))?;

    // Sync log — best-effort: log a warning and continue on failure.
    if rows_affected > 0
        && let Some((workspace_id, snapshot)) = fetch_session_snapshot(db, session_id).await
        && let Err(e) = sync_log_service::write_sync_entry(
            db,
            entity_types::CHAT_SESSION,
            session_id,
            &workspace_id,
            SyncActionType::Update,
            Some(snapshot),
        )
        .await
    {
        tracing::warn!(error = %e, session_id = %session_id, "Failed to write sync log entry");
    }

    Ok(rows_affected > 0)
}

/// Convenience: update session title only.
pub async fn update_session_title(
    db: &DbPool,
    session_id: &str,
    title: &str,
) -> kyomi_core::Result<bool> {
    update_session(db, session_id, Some(title), None, None).await
}

/// Delete a session (owner + optional workspace check, cascades messages).
pub async fn delete_session(
    db: &DbPool,
    user_id: &str,
    session_id: &str,
    workspace_id: Option<&str>,
) -> kyomi_core::Result<bool> {
    // Verify ownership before deleting.
    let session_exists = if let Some(wid) = workspace_id {
        kyomi_core::db_fetch_optional!(
            db,
            SessionIdRow,
            "SELECT session_id FROM chat_sessions \
             WHERE session_id = $1 AND user_id = $2 AND workspace_id = $3",
            session_id,
            user_id,
            wid
        )?
    } else {
        kyomi_core::db_fetch_optional!(
            db,
            SessionIdRow,
            "SELECT session_id FROM chat_sessions \
             WHERE session_id = $1 AND user_id = $2",
            session_id,
            user_id
        )?
    };

    if session_exists.is_none() {
        return Ok(false);
    }

    // Resolve workspace_id BEFORE deletion so the sync log entry has it even
    // when the caller passes workspace_id=None.
    let resolved_wid: Option<String> = match workspace_id {
        Some(w) => Some(w.to_string()),
        None => {
            let row = kyomi_core::db_fetch_optional!(
                db,
                SessionSnapshotRow,
                r#"SELECT session_id, user_id, workspace_id, title, session_type,
                          CAST(updated_at AS TEXT) AS updated_at,
                          CAST(created_at AS TEXT) AS created_at
                   FROM chat_sessions WHERE session_id = $1"#,
                session_id
            )
            .ok()
            .flatten();
            row.map(|r| r.workspace_id)
        }
    };

    // Delete messages first (explicit cascade for safety, matching Python).
    kyomi_core::db_execute!(
        db,
        "DELETE FROM chat_messages WHERE session_id = $1",
        session_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete messages: {e}")))?;

    // Delete the session.
    let result = if let Some(wid) = workspace_id {
        kyomi_core::db_execute!(
            db,
            "DELETE FROM chat_sessions \
             WHERE session_id = $1 AND user_id = $2 AND workspace_id = $3",
            session_id,
            user_id,
            wid
        )
    } else {
        kyomi_core::db_execute!(
            db,
            "DELETE FROM chat_sessions \
             WHERE session_id = $1 AND user_id = $2",
            session_id,
            user_id
        )
    }
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to delete session: {e}")))?;

    // Sync log — best-effort: log a warning and continue on failure.
    if result.rows_affected() > 0 {
        if let Some(wid) = &resolved_wid {
            if let Err(e) = sync_log_service::write_sync_entry(
                db,
                entity_types::CHAT_SESSION,
                session_id,
                wid,
                SyncActionType::Delete,
                None,
            )
            .await
            {
                tracing::warn!(error = %e, session_id = %session_id, "Failed to write sync log entry");
            }
        } else {
            tracing::warn!(session_id = %session_id, "Skipped sync log entry: workspace_id unknown");
        }
    }

    Ok(result.rows_affected() > 0)
}

/// Bulk delete sessions (owner + workspace check).
///
/// Returns count of sessions deleted.
pub async fn bulk_delete_sessions(
    db: &DbPool,
    user_id: &str,
    session_ids: &[String],
    workspace_id: &str,
) -> kyomi_core::Result<i64> {
    if session_ids.is_empty() {
        return Ok(0);
    }

    // Find sessions owned by this user in this workspace.
    let owned: Vec<SessionIdRow> = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            sqlx::query_as::<_, SessionIdRow>(
                "SELECT session_id FROM chat_sessions \
                 WHERE session_id = ANY($1) AND user_id = $2 AND workspace_id = $3",
            )
            .bind(session_ids)
            .bind(user_id)
            .bind(workspace_id)
            .fetch_all(pg)
            .await?
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let (in_clause, _) = in_clause_placeholders(session_ids.len(), 3);
            let sql = format!(
                "SELECT session_id FROM chat_sessions \
                 WHERE user_id = $1 AND workspace_id = $2 AND session_id IN {in_clause}"
            );
            let mut query = sqlx::query_as::<_, SessionIdRow>(&sql)
                .bind(user_id)
                .bind(workspace_id);
            for sid in session_ids {
                query = query.bind(sid);
            }
            query.fetch_all(sq).await?
        }
    };

    if owned.is_empty() {
        return Ok(0);
    }

    let owned_ids: Vec<String> = owned.into_iter().map(|r| r.session_id).collect();

    // Delete messages first, then sessions using match blocks for array binds.
    let deleted_count = match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            sqlx::query("DELETE FROM chat_messages WHERE session_id = ANY($1)")
                .bind(&owned_ids)
                .execute(pg)
                .await?;

            let result = sqlx::query("DELETE FROM chat_sessions WHERE session_id = ANY($1)")
                .bind(&owned_ids)
                .execute(pg)
                .await?;

            result.rows_affected() as i64
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            let (in_clause, _) = in_clause_placeholders(owned_ids.len(), 1);

            let del_msgs_sql = format!("DELETE FROM chat_messages WHERE session_id IN {in_clause}");
            let mut query = sqlx::query(&del_msgs_sql);
            for sid in &owned_ids {
                query = query.bind(sid);
            }
            query.execute(sq).await?;

            let del_sessions_sql =
                format!("DELETE FROM chat_sessions WHERE session_id IN {in_clause}");
            let mut query = sqlx::query(&del_sessions_sql);
            for sid in &owned_ids {
                query = query.bind(sid);
            }
            let result = query.execute(sq).await?;

            result.rows_affected() as i64
        }
    };

    // Sync log — one Delete entry per removed session, best-effort.
    for sid in &owned_ids {
        if let Err(e) = sync_log_service::write_sync_entry(
            db,
            entity_types::CHAT_SESSION,
            sid,
            workspace_id,
            SyncActionType::Delete,
            None,
        )
        .await
        {
            tracing::warn!(error = %e, session_id = %sid, "Failed to write sync log entry");
        }
    }

    Ok(deleted_count)
}

/// Toggle the pinned flag on a message (with session ownership check).
pub async fn toggle_message_pin(
    db: &DbPool,
    session_id: &str,
    message_id: &str,
    user_id: &str,
    workspace_id: Option<&str>,
) -> kyomi_core::Result<bool> {
    let is_pg = db.is_postgres();
    let bool_true_val = kyomi_core::sql_compat::bool_true(is_pg);

    // Verify session access (owner or shared in workspace).
    let session_access = if let Some(wid) = workspace_id {
        let sql = format!(
            "SELECT session_id FROM chat_sessions \
             WHERE session_id = $1 \
               AND (user_id = $2 OR (shared = {bool_true_val} AND workspace_id = $3))"
        );
        kyomi_core::db_fetch_optional!(db, SessionIdRow, &sql, session_id, user_id, wid)?
    } else {
        kyomi_core::db_fetch_optional!(
            db,
            SessionIdRow,
            "SELECT session_id FROM chat_sessions \
             WHERE session_id = $1 AND user_id = $2",
            session_id,
            user_id
        )?
    };

    if session_access.is_none() {
        return Ok(false);
    }

    // Toggle pinned status.
    let result = kyomi_core::db_execute!(
        db,
        "UPDATE chat_messages \
         SET pinned = NOT pinned \
         WHERE message_id = $1 AND session_id = $2",
        message_id,
        session_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to toggle pin: {e}")))?;

    Ok(result.rows_affected() > 0)
}

/// Search sessions by title (ILIKE). Returns owned + shared sessions.
pub async fn search_sessions(
    db: &DbPool,
    user_id: &str,
    workspace_id: &str,
    query: &str,
    limit: i64,
) -> kyomi_core::Result<Vec<SessionListItem>> {
    let pattern = format!("%{query}%");
    let is_pg = db.is_postgres();
    let bool_true_val = kyomi_core::sql_compat::bool_true(is_pg);
    let ilike_expr = kyomi_core::sql_compat::ilike(is_pg, "cs.title", "$3");

    let sql = format!(
        r#"SELECT
           cs.session_id, cs.user_id, cs.workspace_id, cs.title, cs.model,
           cs.session_type,
           cs.shared,
           cs.shared_at, cs.created_at, cs.updated_at,
           cs.config, cs.platform_type, cs.platform_thread_key,
           u.user_id AS owner_user_id,
           COALESCE(u.name, u.email) AS owner_display_name
         FROM chat_sessions cs
         JOIN users u ON cs.user_id = u.user_id
         WHERE (cs.user_id = $1 OR (cs.shared = {bool_true_val} AND cs.workspace_id = $2))
           AND cs.session_type = 'chat'
           AND {ilike_expr}
         ORDER BY cs.updated_at DESC
         LIMIT $4"#,
    );

    let sessions = kyomi_core::db_fetch_all!(
        db,
        SessionWithUserRow,
        &sql,
        user_id,
        workspace_id,
        &pattern,
        limit
    )?;

    if sessions.is_empty() {
        return Ok(Vec::new());
    }

    let session_ids: Vec<String> = sessions.iter().map(|s| s.session_id.clone()).collect();

    // Batch counts.
    let counts = fetch_session_counts(db, &session_ids).await?;

    let count_map: std::collections::HashMap<&str, (i64, i64)> = counts
        .iter()
        .map(|c| (c.session_id.as_str(), (c.message_count, c.pinned_count)))
        .collect();

    let result = sessions
        .iter()
        .map(|s| {
            let (message_count, pinned_count) = count_map
                .get(s.session_id.as_str())
                .copied()
                .unwrap_or((0, 0));

            SessionListItem {
                session_id: s.session_id.clone(),
                title: s.title.clone(),
                model: s.model.clone(),
                session_type: s.session_type.clone(),
                shared: s.shared,
                shared_at: s.shared_at.map(|dt| dt.to_rfc3339()),
                created_at: Some(s.created_at.to_rfc3339()),
                updated_at: Some(s.updated_at.to_rfc3339()),
                message_count,
                pinned_count,
                unread_count: 0, // Search results don't compute unread for simplicity
                created_by: Some(CreatedBy {
                    user_id: s.owner_user_id.clone(),
                    display_name: Some(s.owner_display_name.clone()),
                    ..Default::default()
                }),
                platform_type: s.platform_type.clone(),
            }
        })
        .collect();

    Ok(result)
}

// ---------------------------------------------------------------------------
// Query cache
// ---------------------------------------------------------------------------

/// Store/upsert a query in the cache.
pub async fn store_query(db: &DbPool, query_id: &str, sql: &str) -> kyomi_core::Result<()> {
    let now = Utc::now();

    kyomi_core::db_execute!(
        db,
        "INSERT INTO query_cache (query_id, sql, last_accessed_at, created_at) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (query_id) DO UPDATE SET last_accessed_at = $3",
        query_id,
        sql,
        now,
        now
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to store query: {e}")))?;

    Ok(())
}

/// Get a cached query by ID. Updates `last_accessed_at`.
pub async fn get_query(db: &DbPool, query_id: &str) -> kyomi_core::Result<Option<String>> {
    let now = Utc::now();

    let row = kyomi_core::db_fetch_optional!(
        db,
        QueryCacheRow,
        "UPDATE query_cache SET last_accessed_at = $2 \
         WHERE query_id = $1 \
         RETURNING sql",
        query_id,
        now
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to get query: {e}")))?;

    Ok(row.map(|r| r.sql))
}

// ---------------------------------------------------------------------------
// Charts
// ---------------------------------------------------------------------------

/// Get a chart by ID.
pub async fn get_chart(db: &DbPool, chart_id: &str) -> kyomi_core::Result<Option<Chart>> {
    let chart = kyomi_core::db_fetch_optional!(
        db,
        Chart,
        "SELECT chart_id, message_id, chart_data, created_at, updated_at \
         FROM charts \
         WHERE chart_id = $1",
        chart_id
    )?;

    Ok(chart)
}

/// Update chart data. Returns true if a row was updated.
pub async fn update_chart(
    db: &DbPool,
    chart_id: &str,
    chart_data: &serde_json::Value,
) -> kyomi_core::Result<bool> {
    let now = Utc::now();

    let result = kyomi_core::db_execute!(
        db,
        "UPDATE charts SET chart_data = $1, updated_at = $2 \
         WHERE chart_id = $3",
        chart_data,
        now,
        chart_id
    )
    .map_err(|e| kyomi_core::Error::Internal(format!("failed to update chart: {e}")))?;

    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Agent message loading
// ---------------------------------------------------------------------------

/// Agent message for context restoration.
///
/// Contains all message data needed by the agent, including tool call
/// metadata that is excluded from UI-facing `get_session_messages`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub message_id: String,
    pub role: String,
    pub content: String,
    pub tool_calls: Option<serde_json::Value>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    /// User ID of the sender (for user messages in shared conversations).
    pub sent_by_user_id: Option<String>,
}

/// Row type for agent message queries.
#[derive(Debug, sqlx::FromRow)]
struct AgentMessageRow {
    message_id: String,
    role: String,
    content: String,
    tool_calls: Option<serde_json::Value>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    sent_by_user_id: Option<String>,
}

/// Get ALL messages for agent context restoration (including tool calls and tool results).
///
/// Unlike `get_session_messages` (which filters to user + final assistant only),
/// this returns every message in chronological order so the agent can rebuild
/// its full conversation state.
///
/// Decrypts `content`. Skips empty assistant placeholders (no content AND no tool_calls).
///
/// If `after_message_id` is provided, only returns messages created after that
/// message's timestamp.
pub async fn get_agent_messages(
    db: &DbPool,
    encryption_key: &[u8; 32],
    session_id: &str,
    after_message_id: Option<&str>,
) -> kyomi_core::Result<Vec<AgentMessage>> {
    let rows = if let Some(after_id) = after_message_id {
        // Get the timestamp of the reference message.
        let cutoff_row = kyomi_core::db_fetch_optional!(
            db,
            CreatedAtRow,
            "SELECT created_at FROM chat_messages WHERE message_id = $1",
            after_id
        )?;

        let Some(cutoff_row) = cutoff_row else {
            // Reference message not found, return empty.
            return Ok(Vec::new());
        };

        kyomi_core::db_fetch_all!(
            db,
            AgentMessageRow,
            "SELECT message_id, role, content, tool_calls, tool_call_id, tool_name, sent_by_user_id \
             FROM chat_messages \
             WHERE session_id = $1 AND created_at > $2 \
             ORDER BY created_at ASC",
            session_id,
            cutoff_row.created_at
        )?
    } else {
        kyomi_core::db_fetch_all!(
            db,
            AgentMessageRow,
            "SELECT message_id, role, content, tool_calls, tool_call_id, tool_name, sent_by_user_id \
             FROM chat_messages \
             WHERE session_id = $1 \
             ORDER BY created_at ASC",
            session_id
        )?
    };

    let mut result = Vec::with_capacity(rows.len());

    for row in rows {
        // Decrypt content.
        let content = match encryption::decrypt(&row.content, encryption_key) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    message_id = %row.message_id,
                    "Skipping agent message with corrupt content: {e}"
                );
                continue;
            }
        };

        // Skip empty assistant placeholders (no content AND no tool_calls).
        if row.role == "assistant" && content.trim().is_empty() && row.tool_calls.is_none() {
            continue;
        }

        result.push(AgentMessage {
            message_id: row.message_id,
            role: row.role,
            content,
            tool_calls: row.tool_calls,
            tool_call_id: row.tool_call_id,
            tool_name: row.tool_name,
            sent_by_user_id: row.sent_by_user_id,
        });
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Orchestration — send_chat_message dispatch
// ---------------------------------------------------------------------------

/// Outcome returned by [`prepare_chat_dispatch`].
///
/// `SkippedAi` means the user message was stored and no AI agent should be
/// spawned. `Ready` means the caller should build an [`AgentExecutionConfig`]
/// and spawn the agent.
pub enum ChatDispatchOutcome {
    /// AI was skipped (`skip_ai = true`). The user message has been stored.
    SkippedAi {
        session_id: String,
        user_message_id: String,
    },
    /// AI agent spawn is required. User message has NOT been stored yet (the
    /// agent executor handles storage). Shared-session user message broadcast
    /// has already been sent.
    Ready {
        session_id: String,
        is_new_session: bool,
        user_message_id: String,
        assistant_message_id: String,
        is_shared: bool,
    },
}

/// Parameters for [`prepare_chat_dispatch`].
pub struct ChatDispatchParams<'a> {
    pub db: &'a DbPool,
    pub encryption_key: &'a [u8; 32],
    /// `None` when WebSocket is not configured (standalone mode without WS).
    pub ws_manager: Option<&'a crate::websocket::WebSocketManager>,
    pub user_id: &'a str,
    pub workspace_id: &'a str,
    /// Display name used for shared-session user broadcast.
    pub user_display_name: &'a str,
    pub session_id: Option<&'a str>,
    pub message: &'a str,
    pub current_time_user_tz: Option<&'a str>,
    pub skip_ai: bool,
    /// Optimistic client message ID for deduplication.
    pub client_msg_id: Option<&'a str>,
}

/// Find-or-create session, optionally store user message (skip_ai path), and
/// optionally broadcast the user message to shared-session observers.
///
/// On success returns [`ChatDispatchOutcome`] describing what the caller should
/// do next.
///
/// This consolidates the pre-spawn orchestration from `send_chat_message` so
/// the server_fn remains a thin wrapper.
pub async fn prepare_chat_dispatch(
    p: ChatDispatchParams<'_>,
) -> kyomi_core::Result<ChatDispatchOutcome> {
    // ── Find or create session ─────────────────────────────────────────────
    let is_new_session = p.session_id.is_none();
    let (session_id, is_shared) = if let Some(sid) = p.session_id {
        let session = get_session_info(p.db, p.user_id, sid, Some(p.workspace_id)).await?;
        match session {
            Some(s) => (sid.to_string(), s.shared),
            None => {
                return Err(kyomi_core::Error::Internal(
                    "Session not found or access denied".to_string(),
                ));
            }
        }
    } else {
        let new_sid = create_session(p.db, p.user_id, p.workspace_id, None).await?;

        // Notify the frontend so the sidebar updates immediately.
        if let Some(ws_manager) = p.ws_manager
            && let Ok(Some(session_info)) =
                get_session_info(p.db, p.user_id, &new_sid, Some(p.workspace_id)).await
            && let Ok(data) = serde_json::to_value(&session_info)
        {
            crate::websocket::helpers::send_session_created(ws_manager, p.user_id, &new_sid, data)
                .await;
        }

        (new_sid, false) // New sessions are always private
    };

    // ── Generate message IDs ───────────────────────────────────────────────
    let user_message_id = uuid::Uuid::new_v4().to_string();

    // ── skip_ai fast path ──────────────────────────────────────────────────
    if p.skip_ai {
        let saved_id = add_message(
            p.db,
            p.encryption_key,
            &session_id,
            "user",
            p.message,
            None,
            Some(&user_message_id),
            p.current_time_user_tz,
            Some(p.user_id),
            None,
            None,
            None,
        )
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("Failed to store message: {e}")))?;

        tracing::info!(
            session_id = %session_id,
            message_id = %saved_id,
            "Stored user message (skip_ai=true)"
        );

        return Ok(ChatDispatchOutcome::SkippedAi {
            session_id,
            user_message_id: saved_id,
        });
    }

    // ── Generate assistant placeholder ID ─────────────────────────────────
    let assistant_message_id = uuid::Uuid::new_v4().to_string();

    // ── Broadcast user message to shared-session observers ────────────────
    if is_shared && let Some(ws_manager) = p.ws_manager {
        crate::websocket::helpers::send_shared_chat_message(
            ws_manager,
            p.workspace_id,
            &session_id,
            &user_message_id,
            "user",
            p.message,
            &chrono::Utc::now().to_rfc3339(),
            Some(p.user_display_name),
            Some(p.user_id),
            p.client_msg_id,
        )
        .await;
    }

    Ok(ChatDispatchOutcome::Ready {
        session_id,
        is_new_session,
        user_message_id,
        assistant_message_id,
        is_shared,
    })
}

// ---------------------------------------------------------------------------
// Orchestration — agent error persistence
// ---------------------------------------------------------------------------

/// Parameters for `save_agent_error`.
pub struct SaveAgentErrorParams<'a> {
    pub db: &'a DbPool,
    pub encryption_key: &'a [u8; 32],
    pub ws_manager: &'a crate::websocket::WebSocketManager,
    pub session_id: &'a str,
    pub user_id: &'a str,
    pub assistant_message_id: &'a str,
    pub context_type: &'a str,
    pub error: &'a str,
}

/// Store an agent execution error as an assistant message and send a WebSocket
/// error notification.
///
/// Tries to update the existing placeholder message first. If no placeholder
/// exists (e.g. agent crashed before persisting), inserts a new one. Always
/// sends a `send_error` WebSocket event to the user.
///
/// This consolidates the error-handling block inside the `tokio::spawn` in
/// `send_chat_message` so the spawn closure remains a thin wrapper.
pub async fn save_agent_error(params: SaveAgentErrorParams<'_>) {
    let SaveAgentErrorParams {
        db,
        encryption_key,
        ws_manager,
        session_id,
        user_id,
        assistant_message_id,
        context_type,
        error,
    } = params;
    let error_text = format!("I encountered an error while processing your request: {error}");
    let error_metadata = serde_json::json!({
        "status": "error",
        "error": error,
    });

    // Try update first (persist may have already saved the placeholder).
    let updated = update_message(
        db,
        encryption_key,
        assistant_message_id,
        Some(&error_text),
        Some(&error_metadata),
    )
    .await
    .unwrap_or(false);

    // If no placeholder existed, insert a new message so the user sees the
    // error in the conversation.
    if !updated {
        let _ = add_message(
            db,
            encryption_key,
            session_id,
            "assistant",
            &error_text,
            Some(&error_metadata),
            Some(assistant_message_id),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
    }

    crate::websocket::helpers::send_error(
        ws_manager,
        user_id,
        Some(session_id),
        &format!("AI processing failed: {error}"),
        Some("agent_error"),
        Some(context_type),
    )
    .await;
}

// ---------------------------------------------------------------------------
// Orchestration — update_message_content
// ---------------------------------------------------------------------------

/// Verify session ownership, verify message membership, re-encrypt, and
/// persist updated content.
///
/// Returns `Err` if the session is not found, the caller is not the owner,
/// or the message does not belong to the session.
///
/// This consolidates the orchestration from `update_message_content` server_fn.
pub async fn update_message_content_owned(
    db: &DbPool,
    encryption_key: &[u8; 32],
    user_id: &str,
    workspace_id: &str,
    session_id: &str,
    message_id: &str,
    content: &str,
) -> kyomi_core::Result<()> {
    // Verify session ownership (only owner can edit messages).
    let session = get_session_info(db, user_id, session_id, Some(workspace_id)).await?;
    match session {
        Some(ref s) if s.user_id == user_id => {}
        Some(_) => {
            return Err(kyomi_core::Error::Internal(
                "Only the session owner can edit messages".to_string(),
            ));
        }
        None => {
            return Err(kyomi_core::Error::Internal(
                "Session not found or access denied".to_string(),
            ));
        }
    }

    // Verify the message belongs to this session.
    let msg_exists = kyomi_core::db_fetch_optional!(
        db,
        ExistsRow,
        "SELECT 1 as _n FROM chat_messages \
         WHERE message_id = $1 AND session_id = $2",
        message_id,
        session_id
    )?;

    if msg_exists.is_none() {
        return Err(kyomi_core::Error::Internal("Message not found".to_string()));
    }

    // Update and re-encrypt.
    let updated = update_message(db, encryption_key, message_id, Some(content), None).await?;
    if !updated {
        return Err(kyomi_core::Error::Internal("Message not found".to_string()));
    }

    Ok(())
}

/// Minimal row for existence checks.
#[derive(Debug, sqlx::FromRow)]
struct ExistsRow {
    _n: i32,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// ─── Sync helpers ─────────────────────────────────────────────────────────────

/// List all chat sessions for a workspace, returning list-level metadata
/// as JSON values for the sync bootstrap protocol.
pub async fn list_sessions_for_sync(
    db: &kyomi_core::DbPool,
    workspace_id: &str,
) -> kyomi_core::Result<Vec<serde_json::Value>> {
    #[derive(sqlx::FromRow)]
    struct SessionSyncRow {
        session_id: String,
        user_id: String,
        title: Option<String>,
        model: Option<String>,
        session_type: String,
        #[sqlx(default)]
        shared: bool,
        shared_at: Option<String>,
        updated_at: String,
        created_at: String,
        display_name: String,
    }

    let is_pg = db.is_postgres();
    let bf = kyomi_core::sql_compat::bool_false(is_pg);
    let sql = format!(
        r#"SELECT cs.session_id, cs.user_id, cs.title,
                  cs.model, cs.session_type,
                  COALESCE(cs.shared, {bf}) AS shared,
                  CAST(cs.shared_at AS TEXT) AS shared_at,
                  CAST(cs.updated_at AS TEXT) AS updated_at,
                  CAST(cs.created_at AS TEXT) AS created_at,
                  COALESCE(u.name, u.email, 'Unknown') AS display_name
           FROM chat_sessions cs
           LEFT JOIN users u ON cs.user_id = u.user_id
           WHERE cs.workspace_id = $1 AND cs.session_type = 'chat'
           ORDER BY cs.updated_at DESC"#
    );

    let rows: Vec<SessionSyncRow> =
        kyomi_core::db_fetch_all!(db, SessionSyncRow, &sql, workspace_id).map_err(|e| {
            kyomi_core::Error::Internal(format!("failed to list sessions for sync: {e}"))
        })?;

    let values = rows
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "session_id": row.session_id,
                "title": row.title,
                "model": row.model,
                "session_type": row.session_type,
                "shared": row.shared,
                "shared_at": row.shared_at,
                "created_at": row.created_at,
                "updated_at": row.updated_at,
                "message_count": 0,
                "pinned_count": 0,
                "unread_count": 0,
                "created_by": {
                    "user_id": row.user_id,
                    "display_name": row.display_name,
                },
                "slack_channel_id": null,
            })
        })
        .collect();

    Ok(values)
}

/// Convert a `SessionWithUserRow` into a `SessionMetadata`.
fn session_row_to_detail(row: SessionWithUserRow) -> SessionMetadata {
    SessionMetadata {
        session_id: row.session_id,
        user_id: row.user_id,
        workspace_id: row.workspace_id,
        title: row.title,
        model: row.model,
        shared: row.shared,
        shared_at: row.shared_at.map(|dt| dt.to_rfc3339()),
        created_by: Some(CreatedBy {
            user_id: row.owner_user_id,
            display_name: Some(row.owner_display_name),
            ..Default::default()
        }),
        created_at: Some(row.created_at.to_rfc3339()),
        updated_at: Some(row.updated_at.to_rfc3339()),
        config: row.config,
        platform_type: row.platform_type,
        platform_thread_key: row.platform_thread_key,
    }
}
