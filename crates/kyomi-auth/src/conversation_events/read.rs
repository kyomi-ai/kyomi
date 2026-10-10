//! Authorized durable reads share the writer's lock, encryption and visibility gate.
use super::*;
use agent_runtime::{ConversationSnapshot, CursorPage, CursorReset, ReadRun, RunId};

#[derive(sqlx::FromRow)]
struct ReadRunRow {
    run_id: String,
    state: String,
    assistant_message_id: Option<String>,
    cancellation_requested: bool,
}
macro_rules! read_run {
    ($store:expr, $tx:ident, $conversation:expr, $run:expr) => {{
        let row = sqlx::query_as::<_, ReadRunRow>(
            "SELECT r.run_id,r.state,r.assistant_message_id,r.cancellation_requested FROM conversation_runs r WHERE r.session_id=$1 AND ($2 IS NULL OR r.run_id=$2) ORDER BY (SELECT MIN(e.sequence) FROM conversation_read_projection e WHERE e.run_id=r.run_id AND e.session_id=r.session_id) DESC, r.run_id DESC LIMIT 1")
            .bind($conversation.as_str()).bind($run.map(RunId::as_str)).fetch_optional(&mut *$tx).await?;
        match row {
            Some(row) => ReadRun { run_id: RunId(row.run_id), state: row.state.parse()?,
                assistant_message_id: row.assistant_message_id, cancellation_requested: row.cancellation_requested },
            None => {
                let exists: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM conversation_runs WHERE session_id=$1")
                    .bind($conversation.as_str()).fetch_one(&mut *$tx).await?;
                return Err(if exists.0 == 0 { EventStoreError::NoRun } else { EventStoreError::RunMismatch });
            }
        }
    }};
}
macro_rules! read_page {
    ($store:expr, $tx:ident, $conversation:expr, $run:expr, $after:expr, $watermark:expr, $limit:expr, $bytes:expr, $snapshot:expr) => {{
        // Restricted and exact-run-filtered ciphertext is never fetched/decrypted.
        // LIMIT bounds encrypted staging by MAX_REPLAY_EVENTS * MAX_EVENT_BYTES.
        let query = if $snapshot {
            "SELECT event_id,run_id,sequence,version,true AS public,CASE WHEN $4 IS NULL OR run_id=$4 THEN encrypted_payload ELSE '' END AS encrypted_payload,'' AS encrypted_command FROM conversation_read_projection WHERE session_id=$1 AND sequence>$2 AND sequence<=$3 ORDER BY sequence LIMIT $5"
        } else {
            "SELECT event_id,run_id,sequence,version,public,CASE WHEN public=true AND ($4 IS NULL OR run_id=$4) THEN encrypted_payload ELSE '' END AS encrypted_payload,'' AS encrypted_command FROM conversation_events WHERE session_id=$1 AND sequence>$2 AND sequence<=$3 ORDER BY sequence LIMIT $5"
        };
        let rows = sqlx::query_as::<_, EventRow>(query)
            .bind($conversation.as_str()).bind($after).bind($watermark).bind($run.map(RunId::as_str)).bind($limit as i64).fetch_all(&mut *$tx).await?;
        let row_count = rows.len();
        let mut byte_stopped = false;
        let mut page = CursorPage::new($after, $watermark);
        for row in rows {
            if row.version != i32::from(VERSION) { return Err(EventStoreError::UnsupportedVersion(row.version)); }
            let visible = if row.public && $run.is_none_or(|run| run.as_str() == row.run_id) {
                let payload: Payload = serde_json::from_str(&encryption::decrypt(&row.encrypted_payload, $store.key)?)?;
                let Payload::Public(payload) = payload else { return Err(EventStoreError::Unauthorized); };
                Some(PublicEvent { version: VERSION, conversation_id: $conversation.clone(), run_id: RunId(row.run_id),
                    event_id: EventId(row.event_id), sequence: row.sequence, payload })
            } else { None };
            let included = if $snapshot { page.scan_snapshot(row.sequence, visible, $bytes) } else { page.scan(row.sequence, visible, $bytes) }
                .map_err(EventStoreError::CursorReset)?;
            if !included { byte_stopped = true; break; }
        }
        if $snapshot && row_count < $limit && !byte_stopped {
            page.through_cursor = $watermark;
            page.has_more = false;
        }
        if !$snapshot && row_count == 0 && $after < $watermark {
            return Err(EventStoreError::CursorReset(CursorReset::Pruned));
        }
        if byte_stopped && page.through_cursor == $after { return Err(EventStoreError::InvalidReplay); }
        page
    }};
}
impl ConversationStore<'_> {
    /// Delivery boundary uses the exact same transaction gate as every read. No run
    /// lookup happens until current workspace and owner/shared visibility is proven.
    pub async fn authorize_read(&self, conversation: &ConversationId) -> Result<()> {
        transaction!(self, tx, {
            authorize!(self, tx, conversation.as_str(), false);
            Ok(())
        })
    }

    /// The transport send and revocation share database row locks. Revocation
    /// cannot report completion while a previously authorized send is pending.
    /// The caller supplies a bounded page and closes its sink on delivery failure.
    pub async fn deliver_authorized<F, T>(
        &self,
        conversation: &ConversationId,
        delivery: F,
    ) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        transaction!(self, tx, {
            authorize!(self, tx, conversation.as_str(), false);
            delivery.await
        })
    }

    pub async fn snapshot(
        &self,
        conversation: &ConversationId,
        run: Option<&RunId>,
        limit: usize,
        byte_limit: usize,
    ) -> Result<ConversationSnapshot> {
        read_limits(limit, byte_limit)?;
        transaction!(self, tx, {
            let access = authorize!(self, tx, conversation.as_str(), false);
            let selected = read_run!(self, tx, conversation, run);
            let page = read_page!(
                self,
                tx,
                conversation,
                run,
                0_i64,
                access.event_sequence,
                limit,
                byte_limit,
                true
            );
            Ok(ConversationSnapshot {
                version: VERSION,
                conversation_id: conversation.clone(),
                through_cursor: access.event_sequence,
                run: Some(selected),
                page,
            })
        })
    }

    /// A fixed through watermark pages a snapshot. Omit it for an ordinary catch-up.
    /// Examined private/other-run records advance through_cursor without disclosure.
    pub async fn read_replay(
        &self,
        conversation: &ConversationId,
        run: Option<&RunId>,
        after: i64,
        through: Option<i64>,
        limit: usize,
        byte_limit: usize,
    ) -> Result<CursorPage> {
        read_limits(limit, byte_limit)?;
        transaction!(self, tx, {
            let access = authorize!(self, tx, conversation.as_str(), false);
            if run.is_some() {
                read_run!(self, tx, conversation, run);
            }
            let watermark = through.unwrap_or(access.event_sequence);
            if watermark < 0 || watermark > access.event_sequence || after > watermark {
                return Err(EventStoreError::CursorReset(CursorReset::Invalid));
            }
            let first: (Option<i64>,) =
                sqlx::query_as("SELECT MIN(sequence) FROM conversation_events WHERE session_id=$1")
                    .bind(conversation.as_str())
                    .fetch_one(&mut *tx)
                    .await?;
            if through.is_none() {
                agent_runtime::validate_cursor(after, watermark, first.0)
                    .map_err(EventStoreError::CursorReset)?;
            } else if after < 0 {
                return Err(EventStoreError::CursorReset(CursorReset::Invalid));
            }
            let page = read_page!(
                self,
                tx,
                conversation,
                run,
                after,
                watermark,
                limit,
                byte_limit,
                through.is_some()
            );
            Ok(page)
        })
    }

    /// Exact-run detail expansion proves the public committed event's scope. Opaque
    /// provider/candidate blocks have no public detail path, before or after terminal.
    pub async fn read_detail(
        &self,
        conversation: &ConversationId,
        run: Option<&RunId>,
        detail: &DetailId,
    ) -> Result<Option<String>> {
        transaction!(self, tx, {
            authorize!(self, tx, conversation.as_str(), false);
            if run.is_some() {
                read_run!(self, tx, conversation, run);
            }
            let body: Option<(String,)> = sqlx::query_as("SELECT encrypted_detail FROM conversation_read_projection WHERE session_id=$1 AND detail_id=$2 AND ($3 IS NULL OR run_id=$3)")
                .bind(conversation.as_str()).bind(detail.as_str()).bind(run.map(RunId::as_str)).fetch_optional(&mut *tx).await?;
            body.map(|row| encryption::decrypt(&row.0, self.key).map_err(EventStoreError::from))
                .transpose()
        })
    }
}
fn read_limits(limit: usize, bytes: usize) -> Result<()> {
    if limit == 0
        || limit > agent_runtime::MAX_REPLAY_EVENTS
        || !(agent_runtime::MIN_REPLAY_BYTES..=agent_runtime::MAX_REPLAY_BYTES).contains(&bytes)
    {
        return Err(EventStoreError::InvalidReplay);
    }
    Ok(())
}
