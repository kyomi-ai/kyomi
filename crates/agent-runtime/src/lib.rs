//! Durable conversation foundation. Adapters authorize and lock state before planning,
//! then commit the event, detail, run and compatibility projection atomically.
//! Complete responses only; this protocol has no token delta events.
mod lifecycle;
mod execution;
pub use execution::*;
pub use lifecycle::*;

use serde::{Deserialize, Serialize};

pub const VERSION: u16 = 1;
pub const MAX_EVENT_BYTES: usize = 64 * 1024;
pub const MAX_DETAIL_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_REPLAY_EVENTS: usize = 100;
pub const MAX_REPLAY_BYTES: usize = 1024 * 1024;

macro_rules! identity {
    ($($name:ident),+) => {$(
        #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl $name { pub fn as_str(&self) -> &str { &self.0 } }
    )+};
}
identity!(
    ConversationId,
    RunId,
    MessageId,
    ModelCallId,
    ToolCallId,
    EventId,
    IdempotencyKey,
    DetailId
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}
impl RunState {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}
impl std::str::FromStr for RunState {
    type Err = PolicyError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(PolicyError::InvalidState),
        }
    }
}

/// A preview and its lazy body are persisted together. Detail references are generated
/// by the writer, never supplied by an untrusted public reader.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Text {
    pub preview: String,
    pub detail: Option<DetailId>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordedRole {
    Assistant,
    Tool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PublicPayload {
    Submitted {
        message_id: MessageId,
        text: Text,
    },
    RunState {
        state: RunState,
    },
    ModelResponse {
        model_call_id: ModelCallId,
        text: Text,
    },
    /// A complete message recorded by an owner, without inferring tool success.
    MessageRecorded {
        message_id: MessageId,
        role: RecordedRole,
        text: Text,
        tool_call_id: Option<ToolCallId>,
        name: Option<String>,
        tool_calls: Option<serde_json::Value>,
    },
    Planning {
        text: Text,
    },
    ToolIntent {
        tool_call_id: ToolCallId,
        name: String,
        arguments: serde_json::Value,
    },
    ToolStarted {
        tool_call_id: ToolCallId,
    },
    ToolResult {
        tool_call_id: ToolCallId,
        text: Text,
        succeeded: bool,
    },
    ToolOutcome {
        tool_call_id: ToolCallId,
        text: Text,
        transport: TransportOutcome,
        domain: DomainOutcome,
    },
    Usage {
        model_call_id: ModelCallId,
        usage: Usage,
        #[serde(default)]
        cost: Option<f64>,
    },
    Validation {
        passed: bool,
        text: Text,
    },
    ApprovedAnswer {
        message_id: MessageId,
        text: Text,
    },
    CancellationRequested,
    Cancelled,
    Failed {
        text: Text,
    },
    Interrupted {
        text: Text,
    },
}
impl PublicPayload {
    pub fn text(&self) -> Option<&Text> {
        match self {
            Self::Submitted { text, .. }
            | Self::ModelResponse { text, .. }
            | Self::MessageRecorded { text, .. }
            | Self::Planning { text }
            | Self::ToolResult { text, .. }
            | Self::ToolOutcome { text, .. }
            | Self::Validation { text, .. }
            | Self::ApprovedAnswer { text, .. }
            | Self::Interrupted { text }
            | Self::Failed { text } => Some(text),
            _ => None,
        }
    }
}
/// Restricted payloads deliberately cannot be serialized by a public replay batch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RestrictedPayload {
    ProviderResponse {
        model_call_id: ModelCallId,
        response: serde_json::Value,
        continuation: serde_json::Value,
    },
    Candidate {
        model_call_id: ModelCallId,
        text: String,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "visibility", content = "payload", rename_all = "snake_case")]
pub enum Payload {
    Public(PublicPayload),
    Restricted(RestrictedPayload),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppendCommand {
    pub version: u16,
    pub conversation_id: ConversationId,
    pub run_id: RunId,
    pub event_id: EventId,
    pub idempotency_key: IdempotencyKey,
    pub payload: Payload,
    /// Full exposed text, encrypted by the adapter. Must correspond to the payload's detail ID.
    pub detail: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PublicEvent {
    pub version: u16,
    pub conversation_id: ConversationId,
    pub run_id: RunId,
    pub event_id: EventId,
    pub sequence: i64,
    pub payload: PublicPayload,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayBatch {
    pub version: u16,
    pub events: Vec<PublicEvent>,
    /// Last examined committed sequence, including restricted events. Resume after this value.
    pub scanned_through: i64,
    pub high_watermark: i64,
    pub has_more: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommitReceipt {
    pub event_id: EventId,
    pub sequence: i64,
    pub duplicate: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Projection {
    pub message_id: MessageId,
    pub role: &'static str,
    pub content: String,
    pub status: &'static str,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub next_state: RunState,
    pub projection: Option<Projection>,
    pub tool_receipt: Option<(ToolCallId, bool)>,
    pub usage_key: Option<ModelCallId>,
}
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PolicyError {
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("idempotency identity was reused for a different command")]
    IdempotencyConflict,
    #[error("invalid or missing identity")]
    InvalidIdentity,
    #[error("run does not exist or submission already occurred")]
    InvalidSubmission,
    #[error("terminal run cannot accept new events")]
    Terminal,
    #[error("invalid run transition")]
    InvalidState,
    #[error("payload exceeds the protocol limit")]
    TooLarge,
    #[error("detail body and reference must be supplied together")]
    InvalidDetail,
    #[error("serialization failed: {0}")]
    Serialization(String),
}
/// Validate the version, size limits and identity/detail envelope before deduplication.
pub fn validate(command: &AppendCommand) -> Result<(), PolicyError> {
    if command.version != VERSION {
        return Err(PolicyError::UnsupportedVersion(command.version));
    }
    for id in [
        &command.conversation_id.0,
        &command.run_id.0,
        &command.event_id.0,
        &command.idempotency_key.0,
    ] {
        if id.is_empty() || id.len() > 128 {
            return Err(PolicyError::InvalidIdentity);
        }
    }
    if serde_json::to_vec(command)
        .map_err(|e| PolicyError::Serialization(e.to_string()))?
        .len()
        > MAX_DETAIL_BYTES + MAX_EVENT_BYTES
    {
        return Err(PolicyError::TooLarge);
    }
    if serde_json::to_vec(&command.payload)
        .map_err(|e| PolicyError::Serialization(e.to_string()))?
        .len()
        > if matches!(command.payload, Payload::Restricted(_)) { MAX_DETAIL_BYTES } else { MAX_EVENT_BYTES }
    {
        return Err(PolicyError::TooLarge);
    }
    let text = match &command.payload {
        Payload::Public(p) => p.text(),
        Payload::Restricted(_) => None,
    };
    if text.and_then(|t| t.detail.as_ref()).is_some() != command.detail.is_some() {
        return Err(PolicyError::InvalidDetail);
    }
    if command
        .detail
        .as_ref()
        .is_some_and(|s| s.len() > MAX_DETAIL_BYTES)
    {
        return Err(PolicyError::TooLarge);
    }
    let check_id = |id: &str| {
        if id.is_empty() || id.len() > 128 {
            Err(PolicyError::InvalidIdentity)
        } else {
            Ok(())
        }
    };
    if let Payload::Public(p) = &command.payload {
        match p {
            PublicPayload::Submitted { message_id, .. }
            | PublicPayload::ApprovedAnswer { message_id, .. } => check_id(message_id.as_str())?,
            PublicPayload::MessageRecorded {
                message_id,
                tool_call_id,
                ..
            } => {
                check_id(message_id.as_str())?;
                if let Some(id) = tool_call_id {
                    check_id(id.as_str())?;
                }
            }
            PublicPayload::ModelResponse { model_call_id, .. }
            | PublicPayload::Usage { model_call_id, .. } => check_id(model_call_id.as_str())?,
            PublicPayload::ToolIntent { tool_call_id, .. }
            | PublicPayload::ToolStarted { tool_call_id }
            | PublicPayload::ToolResult { tool_call_id, .. }
            | PublicPayload::ToolOutcome { tool_call_id, .. } => check_id(tool_call_id.as_str())?,
            _ => {}
        }
    } else if let Payload::Restricted(p) = &command.payload {
        match p {
            RestrictedPayload::ProviderResponse { model_call_id, .. }
            | RestrictedPayload::Candidate { model_call_id, .. } => {
                check_id(model_call_id.as_str())?
            }
        }
    }
    if let Some(id) = text.and_then(|t| t.detail.as_ref()) {
        check_id(id.as_str())?;
    }
    Ok(())
}
/// Apply only against run state read under the adapter's transaction lock. Deduplicate
/// before calling: a retry of a terminal event must return the committed receipt.
pub fn plan(command: &AppendCommand, current: Option<RunState>) -> Result<Plan, PolicyError> {
    validate(command)?;
    let mut result = Plan {
        next_state: current.unwrap_or(RunState::Queued),
        projection: None,
        tool_receipt: None,
        usage_key: None,
    };
    let Payload::Public(payload) = &command.payload else {
        if current.is_none() {
            return Err(PolicyError::InvalidSubmission);
        }
        if current.is_some_and(RunState::is_terminal) {
            return Err(PolicyError::Terminal);
        }
        return Ok(result);
    };
    if let PublicPayload::Submitted { message_id, text } = payload {
        if current.is_some() {
            return Err(PolicyError::InvalidSubmission);
        }
        result.projection = Some(Projection {
            message_id: message_id.clone(),
            role: "user",
            content: command
                .detail
                .clone()
                .unwrap_or_else(|| text.preview.clone()),
            status: "complete",
        });
        return Ok(result);
    }
    let state = current.ok_or(PolicyError::InvalidSubmission)?;
    if state.is_terminal() {
        return Err(PolicyError::Terminal);
    }
    match payload {
        PublicPayload::RunState { state: next } => {
            if (*next == RunState::Queued && state != RunState::Queued)
                || *next == RunState::Completed
            {
                return Err(PolicyError::InvalidState);
            }
            result.next_state = *next;
        }
        PublicPayload::ApprovedAnswer { message_id, text } => {
            result.next_state = RunState::Completed;
            result.projection = Some(Projection {
                message_id: message_id.clone(),
                role: "assistant",
                content: command
                    .detail
                    .clone()
                    .unwrap_or_else(|| text.preview.clone()),
                status: "complete",
            });
        }
        PublicPayload::Cancelled => result.next_state = RunState::Cancelled,
        PublicPayload::Failed { .. } => result.next_state = RunState::Failed,
        PublicPayload::Interrupted { .. } => result.next_state = RunState::Interrupted,
        PublicPayload::ToolResult {
            tool_call_id,
            succeeded,
            ..
        } => result.tool_receipt = Some((tool_call_id.clone(), *succeeded)),
        PublicPayload::ToolOutcome { tool_call_id, transport, domain, .. } => {
            result.tool_receipt = Some((tool_call_id.clone(),
                *transport == TransportOutcome::Completed && *domain == DomainOutcome::Succeeded));
        }
        PublicPayload::Usage { model_call_id, .. } => {
            result.usage_key = Some(model_call_id.clone())
        }
        _ => {}
    }
    Ok(result)
}
#[async_trait::async_trait]
pub trait AtomicPersistence: Send + Sync {
    type Error: Send;
    /// Authorize, lock, deduplicate, plan and commit all affected records in one transaction.
    async fn commit(&self, command: &AppendCommand) -> Result<CommitReceipt, Self::Error>;
}
#[async_trait::async_trait]
pub trait Notification: Send + Sync {
    type Error: Send;
    async fn committed(
        &self,
        conversation: &ConversationId,
        receipt: &CommitReceipt,
    ) -> Result<(), Self::Error>;
}
#[derive(Debug)]
pub enum AppendOutcome<E> {
    Committed(CommitReceipt),
    CommittedButNotNotified { receipt: CommitReceipt, error: E },
}
/// The only notification path: failed persistence never publishes. A duplicate retry
/// does not notify again. Notification failure preserves the successful commit receipt.
pub async fn append<S: AtomicPersistence, N: Notification>(
    store: &S,
    notify: &N,
    command: &AppendCommand,
) -> Result<AppendOutcome<N::Error>, S::Error> {
    let receipt = store.commit(command).await?;
    if !receipt.duplicate
        && let Err(error) = notify.committed(&command.conversation_id, &receipt).await
    {
        return Ok(AppendOutcome::CommittedButNotNotified { receipt, error });
    }
    Ok(AppendOutcome::Committed(receipt))
}
