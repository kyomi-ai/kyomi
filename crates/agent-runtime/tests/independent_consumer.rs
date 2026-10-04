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
