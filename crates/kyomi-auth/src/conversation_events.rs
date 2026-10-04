// SPDX-License-Identifier: AGPL-3.0-or-later
//! Encrypted, authorized atomic adapter for the independent agent-runtime protocol.
//! This is the foundation API; execution/provider/interface cutover belongs to later stages.
use crate::encryption;
use agent_runtime::{
    AppendCommand, AtomicPersistence, CommitReceipt, ConversationId, DetailId, EventId, Payload,
    PublicEvent, ReplayBatch, RunState, VERSION,
};
use kyomi_core::DbPool;
use serde::Deserialize;
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, EventStoreError>;
#[derive(Debug, thiserror::Error)]
pub enum EventStoreError {
    #[error("conversation access denied")]
    Unauthorized,
    #[error("idempotency identity was reused for a different command")]
    IdempotencyConflict,
    #[error("unsupported event version {0}")]
    UnsupportedVersion(i32),
    #[error("invalid replay cursor or limit")]
    InvalidReplay,
    #[error(transparent)]
    Policy(#[from] agent_runtime::PolicyError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Application(#[from] kyomi_core::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
#[derive(sqlx::FromRow)]
struct AccessRow {
    user_id: String,
    shared: bool,
    event_sequence: i64,
}
#[derive(sqlx::FromRow)]
struct EventRow {
    event_id: String,
    run_id: String,
    sequence: i64,
    version: i32,
    public: bool,
    encrypted_payload: String,
    encrypted_command: String,
}

// One transaction body is compiled for each native backend. SQLite acquires its write
// reservation before reading state; Postgres serializes on the parent session row.
macro_rules! transaction {
    ($store:expr, $tx:ident, $body:block) => {{
        match $store.db {
            DbPool::Postgres(pool) => {
                let mut $tx = pool.begin().await?;
                let result: Result<_> = async { $body }.await;
                match result {
                    Ok(value) => {
                        $tx.commit().await?;
                        Ok(value)
                    }
                    Err(error) => {
                        $tx.rollback().await?;
                        Err(error)
                    }
                }
            }
            DbPool::Sqlite(pool) => {
                let mut $tx = pool.begin_with("BEGIN IMMEDIATE").await?;
                let result: Result<_> = async { $body }.await;
                match result {
                    Ok(value) => {
                        $tx.commit().await?;
                        Ok(value)
                    }
                    Err(error) => {
                        $tx.rollback().await?;
                        Err(error)
                    }
                }
            }
        }
    }};
}
macro_rules! authorize {
    ($store:expr, $tx:ident, $session:expr, $write:expr) => {{
        // Updating an unchanged value locks the session on both databases. Reads lock too:
        // sharing revocation and deletion cannot race an authorized journal snapshot.
        let session = sqlx::query_as::<_, AccessRow>("UPDATE chat_sessions SET event_sequence = event_sequence WHERE session_id = $1 AND workspace_id = $2 RETURNING user_id, COALESCE(shared, false) AS shared, event_sequence")
            .bind($session).bind($store.workspace_id).fetch_optional(&mut *$tx).await?.ok_or(EventStoreError::Unauthorized)?;
        let membership_sql = if $store.db.is_postgres() {
            "SELECT wu.role FROM workspace_users wu JOIN users u ON u.user_id = wu.user_id WHERE wu.workspace_id = $1 AND wu.user_id = $2 AND wu.active = true AND u.active = true FOR SHARE OF wu, u"
        } else {
            "SELECT wu.role FROM workspace_users wu JOIN users u ON u.user_id = wu.user_id WHERE wu.workspace_id = $1 AND wu.user_id = $2 AND wu.active = true AND u.active = true"
        };
        let membership: Option<(String,)> = sqlx::query_as(membership_sql).bind($store.workspace_id).bind($store.actor_id).fetch_optional(&mut *$tx).await?;
        if membership.is_none() || (session.user_id != $store.actor_id && (!session.shared || $write)) {
            return Err(EventStoreError::Unauthorized);
        }
        session
    }};
}
/// Construct with an authenticated actor and resolved workspace, never client claims.
/// Every operation rechecks active membership and session visibility in its transaction.
pub struct ConversationStore<'a> {
    db: &'a DbPool,
    key: &'a [u8; 32],
    actor_id: &'a str,
    workspace_id: &'a str,
}
impl<'a> ConversationStore<'a> {
    pub fn new(
        db: &'a DbPool,
        key: &'a [u8; 32],
        actor_id: &'a str,
        workspace_id: &'a str,
    ) -> Self {
        Self {
            db,
            key,
            actor_id,
            workspace_id,
        }
    }

    pub async fn append(&self, command: &AppendCommand) -> Result<CommitReceipt> {
        agent_runtime::validate(command)?;
        let fingerprint = hex::encode(Sha256::digest(serde_json::to_vec(command)?));
        transaction!(self, tx, {
            let session_id = command.conversation_id.as_str();
            let access = authorize!(self, tx, session_id, true);
            let existing = sqlx::query_as::<_, EventRow>("SELECT e.event_id, e.run_id, e.sequence, e.version, e.public, e.encrypted_payload, a.encrypted_command FROM conversation_event_aliases a JOIN conversation_events e ON e.event_id = a.event_id WHERE a.session_id = $1 AND (a.idempotency_key = $2 OR a.alias_event_id = $3)")
                .bind(session_id).bind(command.idempotency_key.as_str()).bind(command.event_id.as_str()).fetch_optional(&mut *tx).await?;
            if let Some(row) = existing {
                let saved = encryption::decrypt(&row.encrypted_command, self.key)?;
                if saved != fingerprint {
                    return Err(EventStoreError::IdempotencyConflict);
                }
                return Ok(CommitReceipt {
                    event_id: EventId(row.event_id),
                    sequence: row.sequence,
                    duplicate: true,
                });
            }
            // Usage and tool receipt keys additionally deduplicate independently retried producers.
            let receipt_key = match &command.payload {
                Payload::Public(agent_runtime::PublicPayload::Usage { model_call_id, .. }) => {
                    Some((
                        "conversation_usage_receipts",
                        "model_call_id",
                        model_call_id.as_str(),
                    ))
                }
                Payload::Public(agent_runtime::PublicPayload::ToolResult {
                    tool_call_id, ..
                }) => Some((
                    "conversation_tool_receipts",
                    "tool_call_id",
                    tool_call_id.as_str(),
                )),
                _ => None,
            };
            if let Some((table, column, key)) = receipt_key {
                let sql = format!(
                    "SELECT e.event_id, e.run_id, e.sequence, e.version, e.public, e.encrypted_payload, e.encrypted_command FROM {table} r JOIN conversation_events e ON e.event_id = r.event_id WHERE r.session_id = $1 AND r.run_id = $2 AND r.{column} = $3"
                );
                if let Some(row) = sqlx::query_as::<_, EventRow>(&sql)
                    .bind(session_id)
                    .bind(command.run_id.as_str())
                    .bind(key)
                    .fetch_optional(&mut *tx)
                    .await?
                {
                    let saved: Payload = serde_json::from_str(&encryption::decrypt(
                        &row.encrypted_payload,
                        self.key,
                    )?)?;
                    let saved_detail = if let Payload::Public(ref payload) = saved {
                        if let Some(id) = payload.text().and_then(|t| t.detail.as_ref()) {
                            let body: (String,) = sqlx::query_as("SELECT encrypted_content FROM conversation_event_details WHERE session_id = $1 AND detail_id = $2").bind(session_id).bind(id.as_str()).fetch_one(&mut *tx).await?;
                            Some(encryption::decrypt(&body.0, self.key)?)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if saved != command.payload || saved_detail != command.detail {
                        return Err(EventStoreError::IdempotencyConflict);
                    }
                    let encrypted = encryption::encrypt(&fingerprint, self.key)?;
                    sqlx::query("INSERT INTO conversation_event_aliases (session_id,idempotency_key,alias_event_id,event_id,encrypted_command) VALUES ($1,$2,$3,$4,$5)")
                        .bind(session_id).bind(command.idempotency_key.as_str()).bind(command.event_id.as_str()).bind(&row.event_id).bind(encrypted).execute(&mut *tx).await?;
                    return Ok(CommitReceipt {
                        event_id: EventId(row.event_id),
                        sequence: row.sequence,
                        duplicate: true,
                    });
                }
            }
            let state: Option<(String,)> = sqlx::query_as(
                "SELECT state FROM conversation_runs WHERE session_id = $1 AND run_id = $2",
            )
            .bind(session_id)
            .bind(command.run_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
            let current = state.map(|s| s.0.parse::<RunState>()).transpose()?;
            let plan = agent_runtime::plan(command, current)?;
            let sequence = access
                .event_sequence
                .checked_add(1)
                .ok_or(agent_runtime::PolicyError::TooLarge)?;
            if current.is_none() {
                sqlx::query(
                    "INSERT INTO conversation_runs (run_id, session_id, state) VALUES ($1,$2,$3)",
                )
                .bind(command.run_id.as_str())
                .bind(session_id)
                .bind(plan.next_state.as_str())
                .execute(&mut *tx)
                .await?;
            } else {
                sqlx::query(
                    "UPDATE conversation_runs SET state = $1 WHERE session_id = $2 AND run_id = $3",
                )
                .bind(plan.next_state.as_str())
                .bind(session_id)
                .bind(command.run_id.as_str())
                .execute(&mut *tx)
                .await?;
            }
            let encrypted_payload =
                encryption::encrypt(&serde_json::to_string(&command.payload)?, self.key)?;
            let encrypted_command = encryption::encrypt(&fingerprint, self.key)?;
            sqlx::query("INSERT INTO conversation_events (event_id,session_id,run_id,sequence,version,idempotency_key,public,encrypted_payload,encrypted_command) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)")
                .bind(command.event_id.as_str()).bind(session_id).bind(command.run_id.as_str()).bind(sequence).bind(i32::from(command.version)).bind(command.idempotency_key.as_str()).bind(matches!(command.payload, Payload::Public(_))).bind(encrypted_payload).bind(encrypted_command).execute(&mut *tx).await?;
            let encrypted = encryption::encrypt(&fingerprint, self.key)?;
            sqlx::query("INSERT INTO conversation_event_aliases (session_id,idempotency_key,alias_event_id,event_id,encrypted_command) VALUES ($1,$2,$3,$4,$5)")
                .bind(session_id).bind(command.idempotency_key.as_str()).bind(command.event_id.as_str()).bind(command.event_id.as_str()).bind(encrypted).execute(&mut *tx).await?;
            if let (Payload::Public(payload), Some(body)) = (&command.payload, &command.detail) {
                let detail_id = payload
                    .text()
                    .and_then(|t| t.detail.as_ref())
                    .ok_or(agent_runtime::PolicyError::InvalidDetail)?;
                let encrypted = encryption::encrypt(body, self.key)?;
                sqlx::query("INSERT INTO conversation_event_details (detail_id,session_id,event_id,public,encrypted_content) VALUES ($1,$2,$3,true,$4)").bind(detail_id.as_str()).bind(session_id).bind(command.event_id.as_str()).bind(encrypted).execute(&mut *tx).await?;
            }
            if let Some(projection) = plan.projection {
                // Never overwrite a pre-existing message, including another conversation's row.
                // Later lifecycle tickets may add explicit, fenced projection update commands.
                let encrypted = encryption::encrypt(&projection.content, self.key)?;
                sqlx::query("INSERT INTO chat_messages (message_id,session_id,role,content,status,pinned,created_at) VALUES ($1,$2,$3,$4,$5,false,$6)")
                    .bind(projection.message_id.as_str()).bind(session_id).bind(projection.role).bind(encrypted).bind(projection.status).bind(chrono::Utc::now()).execute(&mut *tx).await?;
            }
            if let Some((tool_call_id, succeeded)) = plan.tool_receipt {
                sqlx::query("INSERT INTO conversation_tool_receipts (session_id,run_id,tool_call_id,event_id,succeeded) VALUES ($1,$2,$3,$4,$5)").bind(session_id).bind(command.run_id.as_str()).bind(tool_call_id.as_str()).bind(command.event_id.as_str()).bind(succeeded).execute(&mut *tx).await?;
            }
            if let Some(model_call_id) = plan.usage_key {
                sqlx::query("INSERT INTO conversation_usage_receipts (session_id,run_id,model_call_id,event_id) VALUES ($1,$2,$3,$4)").bind(session_id).bind(command.run_id.as_str()).bind(model_call_id.as_str()).bind(command.event_id.as_str()).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE chat_sessions SET event_sequence = $1, updated_at = $2 WHERE session_id = $3").bind(sequence).bind(chrono::Utc::now()).bind(session_id).execute(&mut *tx).await?;
            Ok(CommitReceipt {
                event_id: command.event_id.clone(),
                sequence,
                duplicate: false,
            })
        })
    }

    /// Public replay is structurally incapable of returning restricted payloads. Unknown
    /// versions are rejected even on filtered records, instead of silently advancing past them.
    pub async fn replay(
        &self,
        conversation: &ConversationId,
        after: i64,
        limit: usize,
    ) -> Result<ReplayBatch> {
        if after < 0 || limit == 0 || limit > agent_runtime::MAX_REPLAY_EVENTS {
            return Err(EventStoreError::InvalidReplay);
        }
        transaction!(self, tx, {
            let access = authorize!(self, tx, conversation.as_str(), false);
            if after > access.event_sequence {
                return Err(EventStoreError::InvalidReplay);
            }
            let rows = sqlx::query_as::<_, EventRow>("SELECT event_id,run_id,sequence,version,public,encrypted_payload,'' AS encrypted_command FROM conversation_events WHERE session_id = $1 AND sequence > $2 ORDER BY sequence LIMIT $3")
                .bind(conversation.as_str()).bind(after).bind(limit as i64).fetch_all(&mut *tx).await?;
            let mut batch = ReplayBatch {
                version: VERSION,
                events: Vec::new(),
                scanned_through: after,
                high_watermark: access.event_sequence,
                has_more: false,
            };
            let mut bytes = 0;
            for row in rows {
                if row.version != i32::from(VERSION) {
                    return Err(EventStoreError::UnsupportedVersion(row.version));
                }
                if row.public {
                    let payload: Payload = serde_json::from_str(&encryption::decrypt(
                        &row.encrypted_payload,
                        self.key,
                    )?)?;
                    let Payload::Public(payload) = payload else {
                        return Err(EventStoreError::Unauthorized);
                    };
                    let event = PublicEvent {
                        version: VERSION,
                        conversation_id: conversation.clone(),
                        run_id: agent_runtime::RunId(row.run_id),
                        event_id: EventId(row.event_id),
                        sequence: row.sequence,
                        payload,
                    };
                    let size = serde_json::to_vec(&event)?.len();
                    if bytes + size > agent_runtime::MAX_REPLAY_BYTES {
                        break;
                    }
                    bytes += size;
                    batch.events.push(event);
                }
                batch.scanned_through = row.sequence;
            }
            batch.has_more = batch.scanned_through < batch.high_watermark;
            Ok(batch)
        })
    }

    pub async fn detail(
        &self,
        conversation: &ConversationId,
        detail: &DetailId,
    ) -> Result<Option<String>> {
        transaction!(self, tx, {
            authorize!(self, tx, conversation.as_str(), false);
            let row: Option<(String,)> = sqlx::query_as("SELECT encrypted_content FROM conversation_event_details WHERE session_id = $1 AND detail_id = $2 AND public = true").bind(conversation.as_str()).bind(detail.as_str()).fetch_optional(&mut *tx).await?;
            row.map(|r| encryption::decrypt(&r.0, self.key).map_err(EventStoreError::from))
                .transpose()
        })
    }

    /// Worker-only owner-authorized recovery; never feed this type to public transport.
    pub async fn restricted(
        &self,
        conversation: &ConversationId,
        after: i64,
        limit: usize,
    ) -> Result<Vec<RestrictedRecord>> {
        if after < 0 || limit == 0 || limit > agent_runtime::MAX_REPLAY_EVENTS {
            return Err(EventStoreError::InvalidReplay);
        }
        transaction!(self, tx, {
            authorize!(self, tx, conversation.as_str(), true);
            let rows = sqlx::query_as::<_, EventRow>("SELECT event_id,run_id,sequence,version,public,encrypted_payload,'' AS encrypted_command FROM conversation_events WHERE session_id = $1 AND sequence > $2 AND public = false ORDER BY sequence LIMIT $3").bind(conversation.as_str()).bind(after).bind(limit as i64).fetch_all(&mut *tx).await?;
            let mut result = Vec::new();
            let mut bytes = 0;
            for row in rows {
                if row.version != i32::from(VERSION) {
                    return Err(EventStoreError::UnsupportedVersion(row.version));
                }
                let payload: Payload =
                    serde_json::from_str(&encryption::decrypt(&row.encrypted_payload, self.key)?)?;
                let Payload::Restricted(payload) = payload else {
                    return Err(EventStoreError::Unauthorized);
                };
                bytes += serde_json::to_vec(&payload)?.len();
                if bytes > agent_runtime::MAX_REPLAY_BYTES {
                    break;
                }
                result.push(RestrictedRecord {
                    event_id: EventId(row.event_id),
                    sequence: row.sequence,
                    payload,
                });
            }
            Ok(result)
        })
    }
}
// No Serialize implementation: restricted records are adapter-internal recovery values.
#[derive(Debug, Clone, Deserialize)]
pub struct RestrictedRecord {
    pub event_id: EventId,
    pub sequence: i64,
    pub payload: agent_runtime::RestrictedPayload,
}
#[async_trait::async_trait]
impl AtomicPersistence for ConversationStore<'_> {
    type Error = EventStoreError;
    async fn commit(&self, command: &AppendCommand) -> Result<CommitReceipt> {
        self.append(command).await
    }
}
#[cfg(test)]
mod tests;
