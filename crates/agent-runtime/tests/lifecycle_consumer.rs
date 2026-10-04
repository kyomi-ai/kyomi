//! Independent notebook application uses the same locked planners without Kyomi types.
use agent_runtime::*;
use std::sync::Mutex;

#[derive(Default, Clone)]
struct Rows {
    submissions: Vec<SubmitCommand>,
    runs: Vec<(RunId, ConversationId, RunSnapshot)>,
    projections: Vec<Projection>,
    journal: Vec<(RunId, RunState)>,
}
#[derive(Default)]
struct NotebookQueue {
    rows: Mutex<Rows>,
}
impl NotebookQueue {
    fn transaction<T>(
        &self,
        operation: impl FnOnce(&mut Rows) -> Result<T, LifecycleError>,
    ) -> Result<T, LifecycleError> {
        let mut locked = self.rows.lock().expect("fixture operation succeeds");
        let mut pending = locked.clone();
        let receipt = operation(&mut pending)?;
        *locked = pending;
        Ok(receipt)
    }
    fn apply(rows: &mut Rows, id: &RunId, plan: &LifecyclePlan) {
        if plan.changed {
            rows.runs
                .iter_mut()
                .find(|r| &r.0 == id)
                .expect("fixture operation succeeds")
                .2 = plan.snapshot.clone();
            rows.journal.push((id.clone(), plan.snapshot.state));
            if let Some(projection) = &plan.terminal_projection {
                *rows
                    .projections
                    .iter_mut()
                    .find(|p| p.message_id == projection.message_id)
                    .expect("fixture operation succeeds") = projection.clone();
            }
        }
    }
    fn snapshot(&self, id: &RunId) -> RunSnapshot {
        self.rows
            .lock()
            .expect("fixture operation succeeds")
            .runs
            .iter()
            .find(|r| &r.0 == id)
            .expect("fixture operation succeeds")
            .2
            .clone()
    }
}
#[async_trait::async_trait]
impl AtomicLifecyclePersistence for NotebookQueue {
    type Error = LifecycleError;
    async fn submit(&self, command: &SubmitCommand) -> Result<SubmissionPlan, Self::Error> {
        self.transaction(|rows| {
            let existing = rows.submissions.iter().find(|s| {
                s.request_id == command.request_id && s.actor.actor_id == command.actor.actor_id
            });
            let receipt = plan_submission(command, existing)?;
            if !receipt.duplicate {
                let user = plan(&command.submitted, None)?
                    .projection
                    .expect("fixture operation succeeds");
                rows.projections.push(user);
                rows.projections.push(Projection {
                    message_id: command.assistant_message_id.clone(),
                    role: "assistant",
                    content: String::new(),
                    status: "in_progress",
                });
                rows.runs.push((
                    receipt.run_id.clone(),
                    command.submitted.conversation_id.clone(),
                    RunSnapshot {
                        state: RunState::Queued,
                        lease: None,
                        cancellation_requested: false,
                        assistant_message_id: command.assistant_message_id.clone(),
                        fence: 0,
                        queued_at: command.submitted_at,
                        started_at: None,
                        terminal_at: None,
                    },
                ));
                rows.submissions.push(command.clone());
                rows.journal
                    .push((receipt.run_id.clone(), RunState::Queued));
            }
            Ok(receipt)
        })
    }
    async fn claim(&self, command: &ClaimCommand) -> Result<RunSnapshot, Self::Error> {
        self.transaction(|rows| {
            let row = rows
                .runs
                .iter()
                .find(|r| r.0 == command.run_id)
                .expect("fixture operation succeeds");
            let active = rows
                .runs
                .iter()
                .any(|r| r.1 == row.1 && r.2.state == RunState::Running);
            let snapshot = plan_claim(&row.2, &command.owner, command.now, command.ttl, active)?;
            rows.runs
                .iter_mut()
                .find(|r| r.0 == command.run_id)
                .expect("fixture operation succeeds")
                .2 = snapshot.clone();
            rows.journal
                .push((command.run_id.clone(), RunState::Running));
            Ok(snapshot)
        })
    }
    async fn heartbeat(&self, command: &HeartbeatCommand) -> Result<RunSnapshot, Self::Error> {
        self.transaction(|rows| {
            let row = rows
                .runs
                .iter_mut()
                .find(|r| r.0 == command.run_id)
                .expect("fixture operation succeeds");
            row.2 = plan_heartbeat(&row.2, &command.lease, command.now, command.ttl)?;
            Ok(row.2.clone())
        })
    }
    async fn cancel(&self, command: &CancelCommand) -> Result<LifecyclePlan, Self::Error> {
        self.transaction(|rows| {
            let row = rows
                .runs
                .iter()
                .find(|r| r.0 == command.run_id)
                .expect("fixture operation succeeds");
            let plan = plan_cancel(&row.2, command.now)?;
            Self::apply(rows, &command.run_id, &plan);
            Ok(plan)
        })
    }
    async fn finish(&self, command: &FinishCommand) -> Result<LifecyclePlan, Self::Error> {
        self.transaction(|rows| {
            let row = rows
                .runs
                .iter()
                .find(|r| r.0 == command.run_id)
                .expect("fixture operation succeeds");
            let plan = plan_finalize(
                &row.2,
                &command.lease,
                command.now,
                command.state,
                &command.content,
            )?;
            Self::apply(rows, &command.run_id, &plan);
            Ok(plan)
        })
    }
}
fn submit(id: &str) -> SubmitCommand {
    SubmitCommand {
        submitted: AppendCommand {
            version: VERSION,
            conversation_id: ConversationId("notebook".into()),
            run_id: RunId(id.into()),
            event_id: EventId(format!("event-{id}")),
            idempotency_key: IdempotencyKey(format!("event-{id}")),
            payload: Payload::Public(PublicPayload::Submitted {
                message_id: MessageId(format!("user-{id}")),
                text: Text {
                    preview: "hello".into(),
                    detail: None,
                },
            }),
            detail: None,
        },
        assistant_message_id: MessageId(format!("assistant-{id}")),
        request_id: IdempotencyKey(id.into()),
        context: serde_json::json!({"collection":"notes"}),
        actor: ActorIdentity {
            actor_id: "writer".into(),
            source: "notebook".into(),
        },
        submitted_at: 10,
        new_conversation: false,
    }
}
fn claim(id: &RunId, now: i64) -> ClaimCommand {
    ClaimCommand {
        run_id: id.clone(),
        owner: "worker-a".into(),
        now,
        ttl: 100,
    }
}
fn finish(id: &RunId, lease: Lease, now: i64, state: RunState) -> FinishCommand {
    FinishCommand {
        run_id: id.clone(),
        lease,
        now,
        state,
        content: "result".into(),
    }
}

#[tokio::test]
async fn duplicate_submission_preserves_one_atomic_turn_and_rejects_payload_conflicts() {
    let store = NotebookQueue::default();
    let original = submit("first");
    store
        .submit(&original)
        .await
        .expect("fixture operation succeeds");
    let mut retry = original.clone();
    retry.submitted.run_id = RunId("new-generated-id".into());
    retry.submitted.event_id = EventId("new-event".into());
    retry.assistant_message_id = MessageId("new-assistant".into());
    retry.submitted_at = 99;
    assert_eq!(
        store
            .submit(&retry)
            .await
            .expect("fixture operation succeeds"),
        SubmissionPlan {
            run_id: original.submitted.run_id.clone(),
            duplicate: true
        }
    );
    for field in 0..4 {
        let mut conflict = retry.clone();
        match field {
            0 => conflict.context = serde_json::json!({"collection":"other"}),
            1 => conflict.actor.source = "other".into(),
            2 => conflict.submitted.conversation_id = ConversationId("other".into()),
            _ => {
                conflict.submitted.payload = Payload::Public(PublicPayload::Submitted {
                    message_id: MessageId("user".into()),
                    text: Text {
                        preview: "changed".into(),
                        detail: None,
                    },
                })
            }
        }
        assert_eq!(
            store.submit(&conflict).await,
            Err(LifecycleError::Policy(PolicyError::IdempotencyConflict))
        );
    }
    let rows = store.rows.lock().expect("fixture operation succeeds");
    assert_eq!(rows.runs.len(), 1);
    assert_eq!(rows.projections.len(), 2);
    assert_eq!(rows.journal.len(), 1);
}
#[tokio::test]
async fn durable_queue_serializes_conversation_and_cancellation_wins_completion_race() {
    let store = NotebookQueue::default();
    let first = store
        .submit(&submit("first"))
        .await
        .expect("fixture operation succeeds")
        .run_id;
    let second = store
        .submit(&submit("second"))
        .await
        .expect("fixture operation succeeds")
        .run_id;
    let running = store
        .claim(&claim(&first, 20))
        .await
        .expect("fixture operation succeeds");
    let lease = running.lease.expect("fixture operation succeeds");
    assert_eq!(
        store.claim(&claim(&first, 21)).await,
        Err(LifecycleError::NotClaimable)
    );
    assert_eq!(
        store.claim(&claim(&second, 21)).await,
        Err(LifecycleError::ConversationBusy)
    );
    store
        .heartbeat(&HeartbeatCommand {
            run_id: first.clone(),
            lease: lease.clone(),
            now: 90,
            ttl: 100,
        })
        .await
        .expect("fixture operation succeeds");
    assert!(validate_ownership(&store.snapshot(&first), &lease, 150).is_ok());
    store
        .cancel(&CancelCommand {
            run_id: first.clone(),
            now: 151,
        })
        .await
        .expect("fixture operation succeeds");
    let final_plan = store
        .finish(&finish(&first, lease.clone(), 152, RunState::Completed))
        .await
        .expect("fixture operation succeeds");
    assert_eq!(final_plan.snapshot.state, RunState::Cancelled);
    assert_eq!(
        final_plan
            .terminal_projection
            .expect("fixture operation succeeds")
            .status,
        "cancelled"
    );
    assert_eq!(
        store
            .finish(&finish(&first, lease, 153, RunState::Failed))
            .await,
        Err(LifecycleError::StaleLease)
    );
    assert!(
        !store
            .cancel(&CancelCommand {
                run_id: first.clone(),
                now: 154
            })
            .await
            .expect("fixture operation succeeds")
            .changed
    );
    assert_eq!(
        store
            .claim(&claim(&second, 154))
            .await
            .expect("fixture operation succeeds")
            .state,
        RunState::Running
    );
    assert_eq!(
        store
            .rows
            .lock()
            .expect("fixture operation succeeds")
            .projections
            .iter()
            .filter(|p| p.status == "cancelled")
            .count(),
        1
    );
}
#[tokio::test]
async fn queued_cancel_and_first_provider_failure_have_one_truthful_terminal_projection() {
    let store = NotebookQueue::default();
    let queued = store
        .submit(&submit("queued"))
        .await
        .expect("fixture operation succeeds")
        .run_id;
    let plan = store
        .cancel(&CancelCommand {
            run_id: queued.clone(),
            now: 20,
        })
        .await
        .expect("fixture operation succeeds");
    assert_eq!(plan.snapshot.terminal_at, Some(20));
    assert_eq!(
        plan.terminal_projection
            .expect("fixture operation succeeds")
            .message_id,
        MessageId("assistant-queued".into())
    );
    assert_eq!(
        store.claim(&claim(&queued, 21)).await,
        Err(LifecycleError::NotClaimable)
    );
    let failed = store
        .submit(&submit("failed"))
        .await
        .expect("fixture operation succeeds")
        .run_id;
    let lease = store
        .claim(&claim(&failed, 22))
        .await
        .expect("fixture operation succeeds")
        .lease
        .expect("fixture operation succeeds");
    assert_eq!(
        store
            .finish(&finish(&failed, lease, 23, RunState::Failed))
            .await
            .expect("fixture operation succeeds")
            .terminal_projection
            .expect("fixture operation succeeds")
            .status,
        "error"
    );
    assert_eq!(
        store
            .rows
            .lock()
            .expect("fixture operation succeeds")
            .projections
            .iter()
            .filter(|p| ["cancelled", "error"].contains(&p.status))
            .count(),
        2
    );
}
#[tokio::test]
async fn expired_fences_cannot_append_heartbeat_or_finish_and_live_replicas_are_protected() {
    let store = NotebookQueue::default();
    let run = store
        .submit(&submit("first"))
        .await
        .expect("fixture operation succeeds")
        .run_id;
    let running = store
        .claim(&claim(&run, 20))
        .await
        .expect("fixture operation succeeds");
    let lease = running.lease.clone().expect("fixture operation succeeds");
    assert!(
        !plan_expire(&running, 119)
            .expect("fixture operation succeeds")
            .changed
    );
    let interrupted = plan_expire(&running, 120).expect("fixture operation succeeds");
    assert_eq!(interrupted.snapshot.state, RunState::Interrupted);
    assert_eq!(
        interrupted
            .terminal_projection
            .expect("fixture operation succeeds")
            .status,
        "interrupted"
    );
    assert_eq!(
        validate_ownership(&running, &lease, 120),
        Err(LifecycleError::StaleLease)
    );
    assert_eq!(
        plan_heartbeat(&running, &lease, 120, 100),
        Err(LifecycleError::StaleLease)
    );
    assert_eq!(
        plan_finalize(&running, &lease, 120, RunState::Completed, "late"),
        Err(LifecycleError::StaleLease)
    );
    let mut stale = lease;
    stale.fence += 1;
    assert_eq!(
        validate_ownership(&running, &stale, 21),
        Err(LifecycleError::StaleLease)
    );
    let mut queued = running;
    queued.state = RunState::Queued;
    queued.lease = None;
    assert!(
        !plan_expire(&queued, 999)
            .expect("fixture operation succeeds")
            .changed
    );
    assert_eq!(
        plan_claim(&queued, "replacement", 999, 100, false)
            .expect("fixture operation succeeds")
            .fence,
        2
    );
    assert_eq!(
        plan_claim(&queued, "replacement", i64::MAX, 1, false),
        Err(LifecycleError::InvalidLease)
    );
}
#[test]
fn atomic_fixture_rolls_back_every_write_after_injected_failure() {
    let store = NotebookQueue::default();
    let result = store.transaction(|rows| {
        rows.submissions.push(submit("partial"));
        rows.journal
            .push((RunId("partial".into()), RunState::Queued));
        Err::<(), _>(LifecycleError::Policy(PolicyError::InvalidSubmission))
    });
    assert!(result.is_err());
    let rows = store.rows.lock().expect("fixture operation succeeds");
    assert_eq!(rows.submissions.len(), 0);
    assert_eq!(rows.journal.len(), 0);
    assert_eq!(rows.runs.len(), 0);
    assert_eq!(rows.projections.len(), 0);
}

#[tokio::test]
async fn revoked_queued_identity_is_interrupted_without_touching_live_ownership() {
    let store = NotebookQueue::default();
    let run = store
        .submit(&submit("revoked"))
        .await
        .expect("accepted queued turn")
        .run_id;
    let queued = store.snapshot(&run);
    let interrupted =
        plan_interrupt_queued(&queued, 20, "Authorization revoked").expect("queued interruption");
    assert_eq!(interrupted.snapshot.state, RunState::Interrupted);
    assert!(!interrupted.snapshot.cancellation_requested);
    assert_eq!(interrupted.snapshot.terminal_at, Some(20));
    let projection = interrupted
        .terminal_projection
        .expect("truthful terminal assistant");
    assert_eq!(projection.message_id, queued.assistant_message_id);
    assert_eq!(projection.content, "Authorization revoked");
    assert_eq!(projection.status, "interrupted");
    assert!(
        !plan_interrupt_queued(&interrupted.snapshot, 21, "again")
            .expect("terminal no-op")
            .changed
    );
    let running = plan_claim(&queued, "live-owner", 20, 100, false).expect("live claim");
    let live = plan_interrupt_queued(&running, 21, "Authorization revoked").expect("live no-op");
    assert!(!live.changed);
    assert_eq!(live.snapshot, running);
}
