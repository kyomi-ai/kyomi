//! Bounded durable read protocol. A cursor counts examined records, not public events.
use crate::{
    ConversationId, DetailId, MAX_EVENT_BYTES, MAX_IDENTITY_BYTES, MAX_REPLAY_BYTES,
    MAX_REPLAY_EVENTS, PublicEvent, RunId, RunState, VERSION,
};
use serde::{Deserialize, Serialize};

/// One writer-valid public payload plus the complete event wrapper and JSON
/// array brackets. Each byte of the three bounded identities can expand to six
/// bytes (\u0001); fixed fields allow the largest u16 version and signed i64 cursor.
/// Payload's visibility envelope is included in MAX_EVENT_BYTES, so this remains
/// conservative for historical events without narrowing the common writer policy.
pub const MIN_REPLAY_BYTES: usize = MAX_EVENT_BYTES
    + 3 * MAX_IDENTITY_BYTES * 6
    + r#"{"version":65535,"conversation_id":"","run_id":"","event_id":"","sequence":-9223372036854775808,"payload":}"#.len()
    + 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadIdentity {
    pub connection_generation: String,
    pub request_generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorReset {
    Invalid,
    Pruned,
}

/// Check the whole scan range (including restricted records). A missing record is a
/// reset, never a silent advance. Zero is a fresh snapshot, not a retained cursor.
pub fn validate_cursor(
    after: i64,
    watermark: i64,
    first_retained: Option<i64>,
) -> Result<(), CursorReset> {
    if after < 0 || after > watermark {
        return Err(CursorReset::Invalid);
    }
    if after < watermark && first_retained.is_none_or(|first| after + 1 < first) {
        return Err(CursorReset::Pruned);
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CursorPage {
    pub version: u16,
    pub from_cursor: i64,
    pub through_cursor: i64,
    pub high_watermark: i64,
    pub has_more: bool,
    pub events: Vec<PublicEvent>,
}
impl CursorPage {
    pub fn new(from_cursor: i64, high_watermark: i64) -> Self {
        Self {
            version: VERSION,
            from_cursor,
            through_cursor: from_cursor,
            high_watermark,
            has_more: from_cursor < high_watermark,
            events: Vec::new(),
        }
    }
    /// Visibility is resolved by the adapter. None advances an examined private or
    /// run-filtered record without disclosing its identity or payload.
    pub fn scan(
        &mut self,
        sequence: i64,
        visible: Option<PublicEvent>,
        byte_limit: usize,
    ) -> Result<bool, CursorReset> {
        if sequence
            != self
                .through_cursor
                .checked_add(1)
                .ok_or(CursorReset::Invalid)?
        {
            return Err(CursorReset::Pruned);
        }
        self.scan_snapshot(sequence, visible, byte_limit)
    }
    /// A retained public projection has sparse sequences after private records are
    /// removed. Its adapter proves the projection range rather than journal adjacency.
    pub fn scan_snapshot(
        &mut self,
        sequence: i64,
        visible: Option<PublicEvent>,
        byte_limit: usize,
    ) -> Result<bool, CursorReset> {
        if sequence <= self.through_cursor || sequence > self.high_watermark {
            return Err(CursorReset::Pruned);
        }
        if let Some(event) = visible {
            if event.sequence != sequence || event.version != VERSION {
                return Err(CursorReset::Invalid);
            }
            let bytes = serde_json::to_vec(&event)
                .map_err(|_| CursorReset::Invalid)?
                .len();
            let used = serde_json::to_vec(&self.events)
                .map_err(|_| CursorReset::Invalid)?
                .len();
            let separator = usize::from(!self.events.is_empty());
            if used + bytes + separator > byte_limit.min(MAX_REPLAY_BYTES) {
                return Ok(false);
            }
            self.events.push(event);
        }
        self.through_cursor = sequence;
        self.has_more = sequence < self.high_watermark;
        Ok(true)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadRun {
    pub run_id: RunId,
    pub state: RunState,
    pub assistant_message_id: Option<String>,
    pub cancellation_requested: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConversationSnapshot {
    pub version: u16,
    pub conversation_id: ConversationId,
    /// Run metadata and all event pages are read at this exact watermark.
    pub through_cursor: i64,
    pub run: Option<ReadRun>,
    pub page: CursorPage,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationReadRequest {
    pub session_id: String,
    #[serde(default)]
    pub run_id: Option<RunId>,
    pub request_generation: u64,
    #[serde(default)]
    pub after: Option<i64>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default = "default_bytes")]
    pub byte_limit: usize,
}
fn default_limit() -> usize {
    MAX_REPLAY_EVENTS
}
fn default_bytes() -> usize {
    MAX_REPLAY_BYTES
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationReadResponse {
    Snapshot {
        identity: ReadIdentity,
        snapshot: ConversationSnapshot,
    },
    Replay {
        identity: ReadIdentity,
        conversation_id: ConversationId,
        page: CursorPage,
    },
    Reset {
        identity: ReadIdentity,
        reason: CursorReset,
    },
    Error {
        identity: ReadIdentity,
        code: ReadErrorCode,
    },
    Detail {
        identity: ReadIdentity,
        detail_id: DetailId,
        text: Option<String>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadErrorCode {
    Unauthorized,
    NoRun,
    RunMismatch,
    InvalidRequest,
    Unavailable,
}

/// Client acknowledgement is deliberately separate from receipt. Call applied only
/// after the corresponding projection transaction succeeds; a failed application
/// leaves the durable resume cursor intact. One staged page bounds transient memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadStageToken {
    identity: ReadIdentity,
    attempt: u64,
    from_cursor: i64,
    through_cursor: i64,
}
#[derive(Debug)]
pub struct ReadProjectionCursor {
    pub identity: ReadIdentity,
    pub cursor: i64,
    pending: Option<ReadStageToken>,
    attempt: u64,
}
impl ReadProjectionCursor {
    pub fn new(identity: ReadIdentity, cursor: i64) -> Self {
        Self {
            identity,
            cursor,
            pending: None,
            attempt: 0,
        }
    }
    /// Retain this opaque token with the asynchronous application. Every attempt
    /// has a distinct token, including a retry within the same read generation.
    pub fn stage(&mut self, identity: &ReadIdentity, page: &CursorPage) -> Option<ReadStageToken> {
        if identity != &self.identity
            || page.version != VERSION
            || page.has_more != (page.through_cursor < page.high_watermark)
            || page.events.iter().any(|event| {
                event.version != VERSION
                    || event.sequence <= page.from_cursor
                    || event.sequence > page.through_cursor
            })
            || page
                .events
                .windows(2)
                .any(|pair| pair[0].sequence >= pair[1].sequence)
            || self.pending.is_some()
            || page.from_cursor != self.cursor
            || page.through_cursor < self.cursor
            || page.through_cursor > page.high_watermark
            || page.events.len() > MAX_REPLAY_EVENTS
            || serde_json::to_vec(page).map_or(true, |bytes| bytes.len() > MAX_REPLAY_BYTES + 65536)
        {
            return None;
        }
        self.attempt = self.attempt.checked_add(1)?;
        let token = ReadStageToken {
            identity: identity.clone(),
            attempt: self.attempt,
            from_cursor: page.from_cursor,
            through_cursor: page.through_cursor,
        };
        self.pending = Some(token.clone());
        Some(token)
    }
    pub fn applied(&mut self, token: &ReadStageToken) -> bool {
        if self.pending.as_ref() != Some(token)
            || token.identity != self.identity
            || token.from_cursor != self.cursor
        {
            return false;
        }
        self.cursor = token.through_cursor;
        self.pending = None;
        true
    }
    pub fn application_failed(&mut self, token: &ReadStageToken) -> bool {
        if self.pending.as_ref() != Some(token) || token.identity != self.identity {
            return false;
        }
        self.pending = None;
        true
    }
    /// A new subscription replaces both generation and staging. Delayed responses
    /// belonging to the previous subscription cannot regress the new projection.
    pub fn replace(&mut self, identity: ReadIdentity, cursor: i64) {
        self.identity = identity;
        self.cursor = cursor;
        self.pending = None;
    }
}

/// Watermark-tagged run projection handles terminal snapshot/live races. A run
/// identity owns one monotonic terminal state; a new run has a separate projection.
#[derive(Clone, Debug, PartialEq)]
pub struct RunProjection {
    pub state: RunState,
    pub through_cursor: i64,
}
impl RunProjection {
    pub fn apply(&mut self, state: RunState, through_cursor: i64) -> bool {
        if through_cursor <= self.through_cursor
            || (self.state.is_terminal() && state != self.state)
            || (self.state == RunState::Running && state == RunState::Queued)
        {
            return false;
        }
        self.state = state;
        self.through_cursor = through_cursor;
        true
    }
}
