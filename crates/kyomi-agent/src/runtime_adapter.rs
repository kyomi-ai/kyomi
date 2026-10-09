// SPDX-License-Identifier: AGPL-3.0-or-later
//! Kyomi's authorized encrypted persistence and interface projections for agent-runtime.
use crate::adapter::DurableRun;
use crate::thinking::{AgentThinkingEvent, AgentThinkingTracker, ThinkingEventType};
use agent_runtime::{
    AppendCommand, DomainOutcome, EventSink, ExecutionError, Payload, PublicPayload, SinkReceipt,
    TransportOutcome,
};
use kyomi_auth::conversation_events::{ConversationStore, EventStoreError};
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) struct RuntimeSink {
    pub db: kyomi_core::DbPool,
    pub key: Arc<[u8; 32]>,
    pub user: String,
    pub workspace: String,
    pub run: DurableRun,
    pub tracker: Option<Arc<tokio::sync::Mutex<AgentThinkingTracker>>>,
    pub tools: tokio::sync::Mutex<HashMap<String, (String, serde_json::Value)>>,
}
impl RuntimeSink {
    async fn progress(&self, command: &AppendCommand) -> Option<AgentThinkingEvent> {
        let Payload::Public(payload) = &command.payload else {
            return None;
        };
        let (event_type, title, description, data, full) = match payload {
            PublicPayload::Planning { text } => (
                ThinkingEventType::AgentThought,
                "Planning".to_string(),
                Some(text.preview.clone()),
                None,
                text.detail.is_some(),
            ),
            PublicPayload::ToolIntent {
                tool_call_id,
                name,
                arguments,
            } => {
                self.tools
                    .lock()
                    .await
                    .insert(tool_call_id.0.clone(), (name.clone(), arguments.clone()));
                return None;
            }
            PublicPayload::ToolStarted { tool_call_id } => {
                let (name, input) = self
                    .tools
                    .lock()
                    .await
                    .get(tool_call_id.as_str())
                    .cloned()
                    .unwrap_or_else(|| (String::new(), serde_json::json!({})));
                (
                    ThinkingEventType::ToolExecutionStart,
                    crate::thinking::get_friendly_name(&name).to_string(),
                    Some("Working on it...".into()),
                    Some(
                        serde_json::json!({"tool_name":name,"tool_call_id":tool_call_id,"status":"processing", "schema":crate::thinking::format_tool_schema(&name, &input, true)}),
                    ),
                    false,
                )
            }
            PublicPayload::ToolOutcome {
                tool_call_id,
                text,
                transport,
                domain,
            } => {
                let name = self
                    .tools
                    .lock()
                    .await
                    .get(tool_call_id.as_str())
                    .map(|(name, _)| name.clone())
                    .unwrap_or_default();
                let success = *transport == TransportOutcome::Completed
                    && *domain == DomainOutcome::Succeeded;
                (
                    ThinkingEventType::ToolExecutionEnd,
                    format!(
                        "{} {}",
                        if success { "\u{2705}" } else { "\u{274c}" },
                        crate::thinking::get_friendly_name(&name)
                    ),
                    Some(text.preview.clone()),
                    Some(
                        serde_json::json!({"tool_name":name,"tool_call_id":tool_call_id,"status":"completed","success":success,
                        "transport":transport,"domain":domain,"result":text.preview,
                        "schema":crate::thinking::format_tool_schema(&name, &serde_json::from_str::<serde_json::Value>(command.detail.as_deref().unwrap_or(&text.preview)).unwrap_or_else(|_| serde_json::json!({})), false)}),
                    ),
                    text.detail.is_some(),
                )
            }
            _ => return None,
        };
        Some(AgentThinkingEvent {
            event_type,
            timestamp: chrono::Utc::now().to_rfc3339(),
            title,
            event_id: Some(command.event_id.0.clone()),
            description,
            data,
            duration_ms: None,
            has_full_text: full,
        })
    }
}
#[async_trait::async_trait]
impl EventSink for RuntimeSink {
    async fn tool_outcome(
        &self,
        context: &agent_runtime::ExecutionContext,
        id: &agent_runtime::ToolCallId,
    ) -> Result<Option<agent_runtime::ToolOutcome>, ExecutionError> {
        if context.conversation_id != self.run.conversation_id || context.run_id != self.run.run_id
        {
            return Err(ExecutionError::Persistence("runtime scope mismatch".into()));
        }
        ConversationStore::new(&self.db, &self.key, &self.user, &self.workspace)
            .tool_outcome(&context.conversation_id, &context.run_id, id)
            .await
            .map_err(|error| ExecutionError::Persistence(error.to_string()))
    }
    async fn commit(&self, command: &AppendCommand) -> Result<SinkReceipt, ExecutionError> {
        if command.conversation_id != self.run.conversation_id || command.run_id != self.run.run_id
        {
            return Err(ExecutionError::Persistence("runtime scope mismatch".into()));
        }
        let progress = self.progress(command).await;
        let projection = progress
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| ExecutionError::Persistence(error.to_string()))?;
        let store = ConversationStore::new(&self.db, &self.key, &self.user, &self.workspace);
        let mut attempt = 0;
        let receipt = loop {
            match store
                .fenced_execution_event(command, &self.run.lease, projection.as_ref())
                .await
            {
                Ok(receipt) => break receipt,
                Err(error) => {
                    // Bounded backpressure only for unavailable storage. Authorization, fencing,
                    // policy and conflicting retries never become success or invoke a dependent tool.
                    let retryable = matches!(
                        &error,
                        EventStoreError::Database(
                            sqlx::Error::Io(_)
                                | sqlx::Error::PoolTimedOut
                                | sqlx::Error::PoolClosed
                        )
                    );
                    if !retryable || attempt >= 2 {
                        return Err(ExecutionError::Persistence(error.to_string()));
                    }
                    attempt += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(50 * attempt)).await;
                }
            }
        };
        let notification_error = if !receipt.duplicate
            && let Some(tracker) = &self.tracker
        {
            let mut tracker = tracker.lock().await;
            if let Payload::Public(PublicPayload::Usage { usage, cost, .. }) = &command.payload {
                match (
                    u32::try_from(usage.input_tokens),
                    u32::try_from(usage.output_tokens),
                ) {
                    (Ok(input), Ok(output)) => tracker
                        .try_update_token_usage(input, output, *cost)
                        .await
                        .err(),
                    _ => Some("Committed token usage exceeds the interface range".into()),
                }
            } else if let Some(event) = progress {
                tracker
                    .committed_execution_event(event, command.detail.as_deref())
                    .await
                    .err()
            } else {
                None
            }
        } else {
            None
        };
        if let Some(error) = &notification_error {
            tracing::warn!(event_id = %receipt.event_id.as_str(), %error, "Event committed but not notified; database history retained");
        }
        Ok(SinkReceipt {
            receipt,
            notification_error,
        })
    }
}
