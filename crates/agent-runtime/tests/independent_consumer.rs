//! A second application's persistence adapter: no Kyomi, SQLx or server dependencies.
use agent_runtime::*;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
#[derive(Default)]
struct Notebook {
    journal: Mutex<Vec<AppendCommand>>,
}
#[async_trait::async_trait]
impl AtomicPersistence for Notebook {
    type Error = PolicyError;
    async fn commit(&self, command: &AppendCommand) -> Result<CommitReceipt, Self::Error> {
        validate(command)?;
        let mut rows = self.journal.lock().expect("not poisoned");
        if let Some((i, saved)) = rows.iter().enumerate().find(|(_, r)| {
            r.conversation_id == command.conversation_id
                && (r.idempotency_key == command.idempotency_key || r.event_id == command.event_id)
        }) {
            if saved != command {
                return Err(PolicyError::IdempotencyConflict);
            }
            return Ok(CommitReceipt {
                event_id: saved.event_id.clone(),
                sequence: rows[..=i]
                    .iter()
                    .filter(|r| r.conversation_id == command.conversation_id)
                    .count() as i64,
                duplicate: true,
            });
        }
        let state = rows
            .iter()
            .filter(|r| r.conversation_id == command.conversation_id && r.run_id == command.run_id)
            .try_fold(None, |state, r| plan(r, state).map(|p| Some(p.next_state)))?;
        plan(command, state)?;
        rows.push(command.clone());
        Ok(CommitReceipt {
            event_id: command.event_id.clone(),
            sequence: rows
                .iter()
                .filter(|r| r.conversation_id == command.conversation_id)
                .count() as i64,
            duplicate: false,
        })
    }
}
struct Bell {
    calls: AtomicUsize,
    fail: bool,
}
#[async_trait::async_trait]
impl Notification for Bell {
    type Error = &'static str;
    async fn committed(&self, _: &ConversationId, _: &CommitReceipt) -> Result<(), Self::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail { Err("offline") } else { Ok(()) }
    }
}
fn command(payload: PublicPayload, key: &str) -> AppendCommand {
    AppendCommand {
        version: VERSION,
        conversation_id: ConversationId("notebook".into()),
        run_id: RunId("entry".into()),
        event_id: EventId(key.into()),
        idempotency_key: IdempotencyKey(key.into()),
        payload: Payload::Public(payload),
        detail: None,
    }
}
#[tokio::test]
async fn independent_application_commits_before_notification_and_preserves_terminal_retries() {
    let store = Notebook::default();
    let bell = Bell {
        calls: AtomicUsize::new(0),
        fail: true,
    };
    let submitted = command(
        PublicPayload::Submitted {
            message_id: MessageId("user".into()),
            text: Text {
                preview: "hello".into(),
                detail: None,
            },
        },
        "submit",
    );
    assert!(matches!(
        append(&store, &bell, &submitted).await.unwrap(),
        AppendOutcome::CommittedButNotNotified {
            receipt: CommitReceipt { sequence: 1, .. },
            ..
        }
    ));
    let answer = command(
        PublicPayload::ApprovedAnswer {
            message_id: MessageId("reply".into()),
            text: Text {
                preview: "done".into(),
                detail: None,
            },
        },
        "answer",
    );
    append(&store, &bell, &answer).await.unwrap();
    assert!(matches!(
        append(&store, &bell, &answer).await.unwrap(),
        AppendOutcome::Committed(CommitReceipt {
            sequence: 2,
            duplicate: true,
            ..
        })
    ));
    assert!(matches!(
        append(
            &store,
            &bell,
            &command(
                PublicPayload::RunState {
                    state: RunState::Running
                },
                "late"
            )
        )
        .await,
        Err(PolicyError::Terminal)
    ));
    assert_eq!(bell.calls.load(Ordering::SeqCst), 2);
}
#[test]
fn explicit_versions_detail_contract_and_payload_bounds() {
    let mut c = command(
        PublicPayload::Submitted {
            message_id: MessageId("user".into()),
            text: Text {
                preview: "a".into(),
                detail: Some(DetailId("body".into())),
            },
        },
        "submit",
    );
    assert_eq!(plan(&c, None), Err(PolicyError::InvalidDetail));
    c.detail = Some("large exposed planning".into());
    assert!(plan(&c, None).is_ok());
    c.version = 9;
    assert_eq!(plan(&c, None), Err(PolicyError::UnsupportedVersion(9)));
    c.version = VERSION;
    c.detail = Some("x".repeat(MAX_DETAIL_BYTES + 1));
    assert_eq!(plan(&c, None), Err(PolicyError::TooLarge));
}

#[tokio::test]
async fn independent_adapter_rejects_conflicts_and_scopes_keys() {
    let store = Notebook::default();
    let submitted = command(
        PublicPayload::Submitted {
            message_id: MessageId("user".into()),
            text: Text {
                preview: "hello".into(),
                detail: None,
            },
        },
        "submit",
    );
    store.commit(&submitted).await.unwrap();
    let mut conflicting = submitted.clone();
    conflicting.payload = Payload::Public(PublicPayload::Planning {
        text: Text {
            preview: "different".into(),
            detail: None,
        },
    });
    assert_eq!(
        store.commit(&conflicting).await,
        Err(PolicyError::IdempotencyConflict)
    );
    let mut invalid = submitted.clone();
    invalid.version = 2;
    assert_eq!(
        store.commit(&invalid).await,
        Err(PolicyError::UnsupportedVersion(2))
    );
    let mut other = submitted.clone();
    other.conversation_id = ConversationId("second-notebook".into());
    assert_eq!(
        store.commit(&other).await.unwrap(),
        CommitReceipt {
            event_id: other.event_id,
            sequence: 1,
            duplicate: false
        }
    );
}

#[test]
fn initial_queued_event_preserves_submission_and_cannot_requeue_owned_work() {
    let queued = command(
        PublicPayload::RunState {
            state: RunState::Queued,
        },
        "queued",
    );
    assert_eq!(
        plan(&queued, Some(RunState::Queued))
            .expect("initial queued event")
            .next_state,
        RunState::Queued
    );
    assert_eq!(
        plan(&queued, Some(RunState::Running)),
        Err(PolicyError::InvalidState)
    );
    assert_eq!(
        plan(&queued, Some(RunState::Completed)),
        Err(PolicyError::Terminal)
    );
}

#[test]
fn provider_failure_has_truthful_replay_text_and_monotonic_terminal_state() {
    let failed = command(
        PublicPayload::Failed {
            text: Text {
                preview: "Provider unavailable".into(),
                detail: None,
            },
        },
        "failed",
    );
    assert_eq!(plan(&failed, None), Err(PolicyError::InvalidSubmission));
    assert_eq!(
        plan(&failed, Some(RunState::Running))
            .expect("terminal failure event")
            .next_state,
        RunState::Failed
    );
    assert_eq!(
        plan(&failed, Some(RunState::Failed)),
        Err(PolicyError::Terminal)
    );
    let Payload::Public(payload) = &failed.payload else {
        panic!("public failure event")
    };
    assert_eq!(
        payload.text().expect("exposed failure reason").preview,
        "Provider unavailable"
    );
    let serialized = serde_json::to_string(payload).expect("serializable replay failure");
    assert!(serialized.contains("Provider unavailable"));
    assert!(serialized.contains("failed"));
}

#[test]
fn complete_tool_message_envelope_does_not_infer_success_or_terminal_state() {
    let mut recorded = command(
        PublicPayload::MessageRecorded {
            message_id: MessageId("tool-message".into()),
            role: RecordedRole::Tool,
            text: Text {
                preview: "tool output".into(),
                detail: None,
            },
            tool_call_id: Some(ToolCallId("tool-call".into())),
            name: Some("notebook-search".into()),
            tool_calls: None,
        },
        "recorded",
    );
    let planned = plan(&recorded, Some(RunState::Running)).expect("owned complete message");
    assert_eq!(planned.next_state, RunState::Running);
    assert_eq!(planned.projection, None);
    assert_eq!(planned.tool_receipt, None);
    let json = serde_json::to_value(&recorded.payload).expect("complete message envelope");
    assert_eq!(json["payload"]["role"], "tool");
    assert!(json["payload"].get("succeeded").is_none());
    assert_eq!(plan(&recorded, None), Err(PolicyError::InvalidSubmission));
    assert_eq!(
        plan(&recorded, Some(RunState::Completed)),
        Err(PolicyError::Terminal)
    );
    if let Payload::Public(PublicPayload::MessageRecorded { tool_call_id, .. }) =
        &mut recorded.payload
    {
        *tool_call_id = Some(ToolCallId(String::new()));
    }
    assert_eq!(validate(&recorded), Err(PolicyError::InvalidIdentity));
}

// Complete-turn consumer uses only agent-runtime's public ports.
struct TurnSink<'a> {
    notebook: &'a Notebook,
    notifications: AtomicUsize,
    fail_kind: Option<&'static str>,
}
#[async_trait::async_trait]
impl EventSink for TurnSink<'_> {
    async fn tool_outcome(
        &self,
        _: &ExecutionContext,
        id: &ToolCallId,
    ) -> Result<Option<ToolOutcome>, ExecutionError> {
        let rows = self.notebook.journal.lock().unwrap();
        Ok(rows.iter().find_map(|row| match &row.payload {
            Payload::Public(PublicPayload::ToolOutcome {
                tool_call_id,
                text,
                transport,
                domain,
            }) if tool_call_id == id => Some(ToolOutcome {
                transport: transport.clone(),
                domain: domain.clone(),
                text: row.detail.clone().unwrap_or_else(|| text.preview.clone()),
            }),
            _ => None,
        }))
    }
    async fn commit(&self, command: &AppendCommand) -> Result<SinkReceipt, ExecutionError> {
        if self.fail_kind.is_some_and(|kind| {
            matches!(
                (kind, &command.payload),
                (
                    "response",
                    Payload::Restricted(RestrictedPayload::ProviderResponse { .. })
                ) | ("result", Payload::Public(PublicPayload::ToolOutcome { .. }))
            )
        }) {
            return Err(ExecutionError::Persistence(
                "injected writer failure".into(),
            ));
        }
        let receipt = AtomicPersistence::commit(self.notebook, command)
            .await
            .map_err(|error| ExecutionError::Persistence(error.to_string()))?;
        if !receipt.duplicate && matches!(command.payload, Payload::Public(_)) {
            self.notifications.fetch_add(1, Ordering::SeqCst);
        }
        Ok(SinkReceipt {
            receipt,
            notification_error: None,
        })
    }
}
struct NotebookProvider;
#[async_trait::async_trait]
impl CompleteProvider for NotebookProvider {
    type Response = String;
    async fn complete(&self) -> Result<(String, ResponseRecord), ExecutionError> {
        Ok((
            "complete response".into(),
            ResponseRecord {
                cost: Some(0.001),
                raw: serde_json::json!({"content":"complete response", "private_signature":"secret"}),
                continuation: serde_json::json!({"opaque":"continue-this"}),
                candidate: "complete response".into(),
                planning: vec![
                    "Reviewing all notebook records. ".repeat(20),
                    "Checking totals next".into(),
                ],
                usage: Usage {
                    input_tokens: 42,
                    output_tokens: 17,
                },
            },
        ))
    }
}
struct NotebookTool<'a> {
    notebook: &'a Notebook,
    calls: &'a AtomicUsize,
}
#[async_trait::async_trait]
impl ToolExecution for NotebookTool<'_> {
    async fn execute(&self) -> ToolOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let rows = self.notebook.journal.lock().unwrap();
        assert!(rows.iter().any(|row| matches!(&row.payload, Payload::Restricted(RestrictedPayload::ProviderResponse { continuation, .. }) if continuation["opaque"] == "continue-this")));
        assert!(rows.iter().any(|row| matches!(&row.payload, Payload::Public(PublicPayload::Planning { text }) if text.detail.is_some()) && row.detail.as_ref().is_some_and(|body| body.len()>200)));
        assert!(rows.iter().any(|row| matches!(
            row.payload,
            Payload::Public(PublicPayload::ToolStarted { .. })
        )));
        assert!(!rows.iter().any(|row| matches!(
            row.payload,
            Payload::Public(PublicPayload::ApprovedAnswer { .. })
        )));
        ToolOutcome {
            transport: TransportOutcome::Completed,
            domain: DomainOutcome::Rejected,
            text: "{\"valid\":false,\"errors\":[\"missing column\"]}".into(),
        }
    }
}
fn turn() -> ExecutionContext {
    ExecutionContext {
        conversation_id: ConversationId("notebook".into()),
        run_id: RunId("entry".into()),
    }
}
async fn accepted(notebook: &Notebook) {
    AtomicPersistence::commit(
        notebook,
        &command(
            PublicPayload::Submitted {
                message_id: MessageId("user".into()),
                text: Text {
                    preview: "review notes".into(),
                    detail: None,
                },
            },
            "submit",
        ),
    )
    .await
    .unwrap();
}
#[tokio::test]
async fn independent_complete_turn_persists_prerequisites_and_distinct_tool_domain_outcomes() {
    let notebook = Notebook::default();
    accepted(&notebook).await;
    let sink = TurnSink {
        notebook: &notebook,
        notifications: AtomicUsize::new(0),
        fail_kind: None,
    };
    let context = turn();
    let model = ModelCallId("call-1".into());
    context
        .complete(&NotebookProvider, &sink, &model)
        .await
        .unwrap();
    // Same model-call ingestion is idempotent, including detail references and usage.
    context
        .complete(&NotebookProvider, &sink, &model)
        .await
        .unwrap();
    let calls = AtomicUsize::new(0);
    let tool = NotebookTool {
        notebook: &notebook,
        calls: &calls,
    };
    for id in ["lookup-1", "lookup-2"] {
        let outcome = context
            .tool(
                &sink,
                &tool,
                ToolCallId(id.into()),
                "lookup".into(),
                serde_json::json!({}),
            )
            .await
            .unwrap();
        assert!(!outcome.succeeded());
        assert_eq!(outcome.transport, TransportOutcome::Completed);
        assert_eq!(outcome.domain, DomainOutcome::Rejected);
    }
    let replayed = context
        .tool(
            &sink,
            &tool,
            ToolCallId("lookup-1".into()),
            "lookup".into(),
            serde_json::json!({}),
        )
        .await
        .unwrap();
    assert_eq!(replayed.domain, DomainOutcome::Rejected);
    let (text, detail) = context.text("answer", "Notebook checked", 200);
    let answer = context.command(
        "answer",
        Payload::Public(PublicPayload::ApprovedAnswer {
            message_id: MessageId("assistant".into()),
            text,
        }),
        detail,
    );
    sink.commit(&answer).await.unwrap();
    sink.commit(&answer).await.unwrap();
    let rows = notebook.journal.lock().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row.payload, Payload::Public(PublicPayload::Usage { .. })))
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(
                row.payload,
                Payload::Public(PublicPayload::ToolOutcome { .. })
            ))
            .count(),
        2
    );
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(
                row.payload,
                Payload::Public(PublicPayload::ApprovedAnswer { .. })
            ))
            .count(),
        1
    );
    assert_eq!(rows.iter().filter(|row| matches!(&row.payload, Payload::Public(PublicPayload::Planning { text }) if text.detail.is_none())).count(), 1);
    assert_eq!(sink.notifications.load(Ordering::SeqCst), 10);
}
#[tokio::test]
async fn complete_response_failure_prevents_dependent_tools_and_public_notifications() {
    let notebook = Notebook::default();
    accepted(&notebook).await;
    let sink = TurnSink {
        notebook: &notebook,
        notifications: AtomicUsize::new(0),
        fail_kind: Some("response"),
    };
    let calls = AtomicUsize::new(0);
    let context = turn();
    let result = context
        .complete(&NotebookProvider, &sink, &ModelCallId("model".into()))
        .await;
    if result.is_ok() {
        context
            .tool(
                &sink,
                &NotebookTool {
                    notebook: &notebook,
                    calls: &calls,
                },
                ToolCallId("write".into()),
                "write".into(),
                serde_json::json!({}),
            )
            .await
            .unwrap();
    }
    assert!(matches!(result, Err(ExecutionError::Persistence(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(sink.notifications.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn side_effect_without_receipt_is_uncertain_and_duplicate_start_never_reexecutes() {
    let notebook = Notebook::default();
    accepted(&notebook).await;
    let context = turn();
    let sink = TurnSink {
        notebook: &notebook,
        notifications: AtomicUsize::new(0),
        fail_kind: Some("result"),
    };
    context
        .complete(&NotebookProvider, &sink, &ModelCallId("model".into()))
        .await
        .unwrap();
    let calls = AtomicUsize::new(0);
    let tool = NotebookTool {
        notebook: &notebook,
        calls: &calls,
    };
    let id = ToolCallId("write-1".into());
    assert!(matches!(
        context
            .tool(
                &sink,
                &tool,
                id.clone(),
                "write".into(),
                serde_json::json!({})
            )
            .await,
        Err(ExecutionError::Uncertain(_))
    ));
    assert!(matches!(
        context
            .tool(&sink, &tool, id, "write".into(), serde_json::json!({}))
            .await,
        Err(ExecutionError::Uncertain(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn execution_envelopes_remain_bounded_with_maximum_application_identities() {
    let context = ExecutionContext {
        conversation_id: ConversationId("c".repeat(128)),
        run_id: RunId("r".repeat(128)),
    };
    let (text, detail) = context.text(&"k".repeat(512), &"long plan ".repeat(60), 200);
    let command = context.command(
        &"k".repeat(512),
        Payload::Public(PublicPayload::Planning { text }),
        detail,
    );
    validate(&command).unwrap();
    assert_eq!(command.event_id.as_str().len(), 64);
    assert_eq!(command.idempotency_key.as_str().len(), 64);
}
