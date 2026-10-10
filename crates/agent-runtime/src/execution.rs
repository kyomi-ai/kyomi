//! Awaited complete-response and tool orchestration. Adapters supply provider policy,
//! exposed text, tool domain outcomes and atomic persistence. No token streaming.
use crate::{
    AppendCommand, CommitReceipt, ConversationId, DetailId, EventId, IdempotencyKey, ModelCallId,
    Payload, PublicPayload, RestrictedPayload, RunId, Text, ToolCallId, Usage, VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("provider failed: {0}")]
    Provider(String),
    #[error("durable prerequisite failed: {0}")]
    Persistence(String),
    #[error("tool outcome is unknown after execution: {0}")]
    Uncertain(String),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportOutcome {
    Completed,
    Failed,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainOutcome {
    Succeeded,
    Rejected,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub transport: TransportOutcome,
    pub domain: DomainOutcome,
    pub text: String,
}
impl ToolOutcome {
    pub fn succeeded(&self) -> bool {
        self.transport == TransportOutcome::Completed && self.domain == DomainOutcome::Succeeded
    }
}
#[derive(Clone, Debug)]
pub struct ResponseRecord {
    pub raw: serde_json::Value,
    pub continuation: serde_json::Value,
    pub candidate: String,
    /// Only adapter-approved exposed text; never private/opaque provider blocks.
    pub planning: Vec<String>,
    pub usage: Usage,
    pub cost: Option<f64>,
}
#[async_trait::async_trait]
pub trait CompleteProvider: Send + Sync {
    type Response: Send;
    async fn complete(&self) -> Result<(Self::Response, ResponseRecord), ExecutionError>;
}
#[async_trait::async_trait]
pub trait ToolExecution: Send + Sync {
    /// Exactly one invocation. Persistence retries must never repeat this operation.
    async fn execute(&self) -> ToolOutcome;
}
#[async_trait::async_trait]
pub trait EventSink: Send + Sync {
    /// Commit the entire command atomically; notify only after successful commit.
    /// Notification failures are committed-but-not-notified, not a failed write.
    async fn commit(&self, command: &AppendCommand) -> Result<SinkReceipt, ExecutionError>;
    /// Read an authorized committed result before considering execution. A start without
    /// this receipt remains uncertain; it is never a retryable tool invocation.
    async fn tool_outcome(
        &self,
        context: &ExecutionContext,
        id: &ToolCallId,
    ) -> Result<Option<ToolOutcome>, ExecutionError>;
}
#[derive(Clone, Debug)]
pub struct SinkReceipt {
    pub receipt: CommitReceipt,
    pub notification_error: Option<String>,
}
#[derive(Clone, Debug)]
pub struct ExecutionContext {
    pub conversation_id: ConversationId,
    pub run_id: RunId,
}
impl ExecutionContext {
    /// Fixed-size deterministic envelope identities, even with maximum-length application IDs.
    pub fn identity(&self, namespace: &str, key: &str) -> String {
        let bytes = serde_json::to_vec(&(
            self.conversation_id.as_str(),
            self.run_id.as_str(),
            namespace,
            key,
        ))
        .expect("serializing string tuple cannot fail");
        format!("{:x}", Sha256::digest(bytes))
    }
    pub fn command(&self, key: &str, payload: Payload, detail: Option<String>) -> AppendCommand {
        let id = self.identity("event", key);
        AppendCommand {
            version: VERSION,
            conversation_id: self.conversation_id.clone(),
            run_id: self.run_id.clone(),
            event_id: EventId(id.clone()),
            idempotency_key: IdempotencyKey(id),
            payload,
            detail,
        }
    }
    pub fn text(&self, key: &str, body: &str, limit: usize) -> (Text, Option<String>) {
        let long = body.chars().count() > limit;
        (
            Text {
                preview: body.chars().take(limit).collect(),
                detail: long.then(|| DetailId(self.identity("detail", key))),
            },
            long.then(|| body.to_string()),
        )
    }
    pub async fn complete<P: CompleteProvider + ?Sized, S: EventSink + ?Sized>(
        &self,
        provider: &P,
        sink: &S,
        model: &ModelCallId,
    ) -> Result<P::Response, ExecutionError> {
        let (response, record) = provider.complete().await?;
        self.ingest(sink, model, record).await?;
        Ok(response)
    }
    /// Complete raw response and candidate are restricted, immediately durable before tools.
    pub async fn ingest<S: EventSink + ?Sized>(
        &self,
        sink: &S,
        model: &ModelCallId,
        record: ResponseRecord,
    ) -> Result<(), ExecutionError> {
        let prefix = model.as_str();
        sink.commit(&self.command(
            &format!("{prefix}:response"),
            Payload::Restricted(RestrictedPayload::ProviderResponse {
                model_call_id: model.clone(),
                response: record.raw,
                continuation: record.continuation,
            }),
            None,
        ))
        .await?;
        sink.commit(&self.command(
            &format!("{prefix}:candidate"),
            Payload::Restricted(RestrictedPayload::Candidate {
                model_call_id: model.clone(),
                text: record.candidate,
            }),
            None,
        ))
        .await?;
        sink.commit(&self.command(
            &format!("{prefix}:usage"),
            Payload::Public(PublicPayload::Usage {
                model_call_id: model.clone(),
                usage: record.usage,
                cost: record.cost,
            }),
            None,
        ))
        .await?;
        for (index, body) in record.planning.iter().enumerate() {
            let key = format!("{prefix}:planning:{index}");
            let (text, detail) = self.text(&key, body, 200);
            sink.commit(&self.command(
                &key,
                Payload::Public(PublicPayload::Planning { text }),
                detail,
            ))
            .await?;
        }
        Ok(())
    }
    pub async fn tool<T: ToolExecution + ?Sized, S: EventSink + ?Sized>(
        &self,
        sink: &S,
        tool: &T,
        id: ToolCallId,
        name: String,
        arguments: serde_json::Value,
    ) -> Result<ToolOutcome, ExecutionError> {
        if let Some(outcome) = sink.tool_outcome(self, &id).await? {
            if outcome.transport == TransportOutcome::Unknown
                || outcome.domain == DomainOutcome::Unknown
            {
                return Err(ExecutionError::Uncertain(outcome.text));
            }
            return Ok(outcome);
        }
        let prefix = id.as_str();
        sink.commit(&self.command(
            &format!("{prefix}:intent"),
            Payload::Public(PublicPayload::ToolIntent {
                tool_call_id: id.clone(),
                name,
                arguments,
            }),
            None,
        ))
        .await?;
        let started = sink
            .commit(&self.command(
                &format!("{prefix}:start"),
                Payload::Public(PublicPayload::ToolStarted {
                    tool_call_id: id.clone(),
                }),
                None,
            ))
            .await?;
        // A persisted start is a durable execution fence, not permission to repeat an action.
        if started.receipt.duplicate {
            return Err(ExecutionError::Uncertain(
                "tool start already committed; reconcile its receipt instead of rerunning".into(),
            ));
        }
        let outcome = tool.execute().await;
        let key = format!("{prefix}:outcome");
        let (text, detail) = self.text(&key, &outcome.text, 8000);
        let command = self.command(
            &key,
            Payload::Public(PublicPayload::ToolOutcome {
                tool_call_id: id,
                text,
                transport: outcome.transport.clone(),
                domain: outcome.domain.clone(),
            }),
            detail,
        );
        // Retrying the same result command is safe; invoking the tool again is not.
        sink.commit(&command)
            .await
            .map_err(|error| ExecutionError::Uncertain(error.to_string()))?;
        if outcome.transport == TransportOutcome::Unknown
            || outcome.domain == DomainOutcome::Unknown
        {
            return Err(ExecutionError::Uncertain(outcome.text));
        }
        Ok(outcome)
    }
}
