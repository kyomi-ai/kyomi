//! A second application uses durable read contracts with no Kyomi dependency.
use agent_runtime::*;
fn event(sequence: i64, state: RunState) -> PublicEvent {
    PublicEvent {
        version: VERSION,
        conversation_id: ConversationId("ledger".into()),
        run_id: RunId("job".into()),
        event_id: EventId(format!("event-{sequence}")),
        sequence,
        payload: PublicPayload::RunState { state },
    }
}
#[test]
fn sparse_visibility_failed_application_duplicates_and_generation_replacement() {
    let identity = ReadIdentity {
        connection_generation: "socket-a".into(),
        request_generation: 1,
    };
    let mut page = CursorPage::new(0, 3);
    assert!(
        page.scan(1, Some(event(1, RunState::Running)), MAX_REPLAY_BYTES)
            .unwrap()
    );
    assert!(page.scan(2, None, MAX_REPLAY_BYTES).unwrap());
    assert!(
        page.scan(3, Some(event(3, RunState::Cancelled)), MAX_REPLAY_BYTES)
            .unwrap()
    );
    assert_eq!(
        page.events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![1, 3]
    );
    let mut client = ReadProjectionCursor::new(identity.clone(), 0);
    let attempt = client.stage(&identity, &page).unwrap();
    assert_eq!(
        client.cursor, 0,
        "receipt must not acknowledge failed projection"
    );
    assert!(client.application_failed(&attempt));
    let retry = client.stage(&identity, &page).unwrap();
    assert!(client.applied(&retry));
    assert!(
        client.stage(&identity, &page).is_none(),
        "duplicate response is inert"
    );
    let newer = ReadIdentity {
        connection_generation: "socket-b".into(),
        request_generation: 2,
    };
    client.replace(newer, 3);
    assert!(
        client.stage(&identity, &CursorPage::new(3, 3)).is_none(),
        "stale connection cannot replace current state"
    );
    let mut run = RunProjection {
        state: RunState::Cancelled,
        through_cursor: 3,
    };
    assert!(!run.apply(RunState::Running, 1));
    assert!(
        !run.apply(RunState::Completed, 4),
        "terminal races cannot replace acknowledged terminal state"
    );
}
#[test]
fn bounded_scan_and_pruning_contract() {
    assert_eq!(validate_cursor(9, 8, Some(1)), Err(CursorReset::Invalid));
    assert_eq!(validate_cursor(0, 8, Some(3)), Err(CursorReset::Pruned));
    assert_eq!(validate_cursor(8, 8, None), Ok(()));
    let mut page = CursorPage::new(0, 5);
    assert_eq!(
        page.scan(2, None, MAX_REPLAY_BYTES),
        Err(CursorReset::Pruned)
    );
    assert!(
        page.scan_snapshot(2, Some(event(2, RunState::Running)), MAX_REPLAY_BYTES)
            .unwrap()
    );
    assert!(
        !page
            .scan_snapshot(4, Some(event(4, RunState::Completed)), 1)
            .unwrap()
    );
    assert_eq!(
        page.through_cursor, 2,
        "byte stop must not skip unapplied record"
    );
}

#[test]
fn maximum_cursor_and_monotonic_running_state_do_not_overflow_or_regress() {
    let mut maximum = CursorPage::new(i64::MAX, i64::MAX);
    assert_eq!(
        maximum.scan(0, None, MAX_REPLAY_BYTES),
        Err(CursorReset::Invalid)
    );
    let mut running = RunProjection {
        state: RunState::Running,
        through_cursor: 4,
    };
    assert!(!running.apply(RunState::Queued, 5));
    assert_eq!(running.state, RunState::Running);
    assert!(running.apply(RunState::Completed, 6));
}

#[test]
fn byte_bound_includes_array_separators() {
    let first = event(1, RunState::Running);
    let second = event(2, RunState::Completed);
    let exact = serde_json::to_vec(&vec![first.clone(), second.clone()])
        .unwrap()
        .len();
    let mut page = CursorPage::new(0, 2);
    assert!(page.scan(1, Some(first), exact - 1).unwrap());
    assert!(!page.scan(2, Some(second.clone()), exact - 1).unwrap());
    assert_eq!(page.through_cursor, 1);
    assert!(page.scan(2, Some(second), exact).unwrap());
    assert_eq!(serde_json::to_vec(&page.events).unwrap().len(), exact);
}

#[test]
fn stale_application_callbacks_cannot_complete_or_clear_a_replacement_attempt() {
    let old = ReadIdentity {
        connection_generation: "socket-a".into(),
        request_generation: 1,
    };
    let new = ReadIdentity {
        connection_generation: "socket-b".into(),
        request_generation: 2,
    };
    let mut page = CursorPage::new(0, 3);
    page.scan_snapshot(3, None, MAX_REPLAY_BYTES).unwrap();
    let mut cursor = ReadProjectionCursor::new(old.clone(), 0);
    let old_attempt = cursor.stage(&old, &page).unwrap();
    cursor.replace(new.clone(), 0);
    let replacement = cursor.stage(&new, &page).unwrap();
    assert!(!cursor.applied(&old_attempt));
    assert!(!cursor.application_failed(&old_attempt));
    assert_eq!(cursor.cursor, 0);
    assert!(
        cursor.stage(&new, &page).is_none(),
        "replacement remains pending"
    );
    assert!(cursor.application_failed(&replacement));
    let retry = cursor.stage(&new, &page).unwrap();
    assert_ne!(replacement, retry);
    assert!(!cursor.applied(&replacement));
    assert!(!cursor.application_failed(&replacement));
    assert_eq!(cursor.cursor, 0);
    assert!(cursor.applied(&retry));
    assert_eq!(cursor.cursor, 3);
    assert!(!cursor.applied(&retry));
}

#[test]
fn minimum_replay_budget_contains_maximum_payload_and_escaped_identity_wrapper() {
    let identity = "\u{1}".repeat(MAX_IDENTITY_BYTES);
    let mut command = AppendCommand {
        version: VERSION,
        conversation_id: ConversationId(identity.clone()),
        run_id: RunId(identity.clone()),
        event_id: EventId(identity.clone()),
        idempotency_key: IdempotencyKey(identity),
        payload: Payload::Public(PublicPayload::Planning {
            text: Text {
                preview: String::new(),
                detail: None,
            },
        }),
        detail: None,
    };
    let overhead = serde_json::to_vec(&command.payload).unwrap().len();
    let Payload::Public(PublicPayload::Planning { text }) = &mut command.payload else {
        unreachable!()
    };
    text.preview = "x".repeat(MAX_EVENT_BYTES - overhead);
    assert_eq!(
        serde_json::to_vec(&command.payload).unwrap().len(),
        MAX_EVENT_BYTES
    );
    validate(&command).unwrap();
    let Payload::Public(payload) = command.payload else {
        unreachable!()
    };
    let event = PublicEvent {
        version: VERSION,
        conversation_id: command.conversation_id,
        run_id: command.run_id,
        event_id: command.event_id,
        sequence: i64::MAX,
        payload,
    };
    let mut page = CursorPage::new(i64::MAX - 1, i64::MAX);
    assert!(page.scan(i64::MAX, Some(event), MIN_REPLAY_BYTES).unwrap());
    assert!(serde_json::to_vec(&page.events).unwrap().len() <= MIN_REPLAY_BYTES);
    assert_eq!(page.through_cursor, i64::MAX);
}
