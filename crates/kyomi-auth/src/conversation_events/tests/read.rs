use super::*;
use agent_runtime::{CursorReset, MAX_EVENT_BYTES, MAX_REPLAY_BYTES};

async fn durable_read_contract(db: &DbPool) {
    let (owner, member, wid, sid) = seed(db).await;
    let conversation = ConversationId(sid.clone());
    let store = ConversationStore::new(db, &KEY, &owner, &wid);
    let viewer = ConversationStore::new(db, &KEY, &member, &wid);
    assert!(matches!(
        store
            .snapshot(&conversation, None, 100, MAX_REPLAY_BYTES)
            .await,
        Err(EventStoreError::NoRun)
    ));
    let old = RunId(uuid::Uuid::new_v4().to_string());
    let submitted = command(
        &sid,
        old.as_str(),
        Payload::Public(P::Submitted {
            message_id: MessageId(uuid::Uuid::new_v4().to_string()),
            text: text("durable question"),
        }),
    );
    store.append(&submitted).await.unwrap();
    store
        .append(&command(
            &sid,
            old.as_str(),
            Payload::Restricted(RestrictedPayload::Candidate {
                model_call_id: ModelCallId("private-model".into()),
                text: "private candidate must never be delivered".into(),
            }),
        ))
        .await
        .unwrap();
    let internal = viewer
        .read_replay(&conversation, Some(&old), 1, None, 1, MAX_REPLAY_BYTES)
        .await
        .unwrap();
    assert!(internal.events.is_empty());
    assert_eq!(
        (
            internal.from_cursor,
            internal.through_cursor,
            internal.high_watermark
        ),
        (1, 2, 2)
    );
    let detail = DetailId(uuid::Uuid::new_v4().to_string());
    let mut thinking = command(
        &sid,
        old.as_str(),
        Payload::Public(P::Planning {
            text: Text {
                preview: "committed thinking".into(),
                detail: Some(detail.clone()),
            },
        }),
    );
    thinking.detail = Some("complete independently finished planning".repeat(5000));
    store.append(&thinking).await.unwrap();
    // Full detail is available while the worker is still running, on another adapter.
    assert_eq!(
        viewer
            .read_detail(&conversation, Some(&old), &detail)
            .await
            .unwrap(),
        thinking.detail
    );
    let before = viewer
        .snapshot(&conversation, Some(&old), 1, MAX_REPLAY_BYTES)
        .await
        .unwrap();
    assert_eq!(before.through_cursor, 3);
    assert_eq!(before.page.through_cursor, 1);
    assert_eq!(before.run.as_ref().unwrap().state, RunState::Queued);
    // This terminal write lands during bounded snapshot paging. The captured state
    // and remaining snapshot pages stay at 3; catch-up returns exactly the new write.
    let interrupted = command(
        &sid,
        old.as_str(),
        Payload::Public(P::Interrupted {
            text: text("worker lost"),
        }),
    );
    store.append(&interrupted).await.unwrap();
    let rest = viewer
        .read_replay(
            &conversation,
            Some(&old),
            before.page.through_cursor,
            Some(before.through_cursor),
            100,
            MAX_REPLAY_BYTES,
        )
        .await
        .unwrap();
    assert_eq!(rest.through_cursor, 3);
    assert_eq!(
        rest.events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![3]
    );
    let after = viewer
        .read_replay(&conversation, Some(&old), 3, None, 100, MAX_REPLAY_BYTES)
        .await
        .unwrap();
    assert_eq!(after.events.len(), 1);
    assert!(matches!(after.events[0].payload, P::Interrupted { .. }));
    let restarted = ConversationStore::new(db, &KEY, &member, &wid);
    let recovered = restarted
        .snapshot(&conversation, Some(&old), 100, MAX_REPLAY_BYTES)
        .await
        .unwrap();
    assert_eq!(recovered.run.unwrap().state, RunState::Interrupted);
    assert!(
        recovered
            .page
            .events
            .iter()
            .any(|event| matches!(event.payload, P::Planning { .. }))
    );
    assert_eq!(
        restarted
            .read_detail(&conversation, Some(&old), &detail)
            .await
            .unwrap(),
        thinking.detail
    );
    let new = RunId(uuid::Uuid::new_v4().to_string());
    store
        .append(&command(
            &sid,
            new.as_str(),
            Payload::Public(P::Submitted {
                message_id: MessageId(uuid::Uuid::new_v4().to_string()),
                text: text("new question"),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        viewer
            .snapshot(&conversation, None, 100, MAX_REPLAY_BYTES)
            .await
            .unwrap()
            .run
            .unwrap()
            .run_id,
        new
    );
    let exact = viewer
        .snapshot(&conversation, Some(&old), 100, MAX_REPLAY_BYTES)
        .await
        .unwrap();
    assert_eq!(exact.run.unwrap().run_id, old);
    assert!(exact.page.events.iter().all(|event| event.run_id == old));
    assert_eq!(
        exact.page.through_cursor, 5,
        "exact-run filters must advance over other runs"
    );
    assert!(matches!(
        viewer
            .snapshot(
                &conversation,
                Some(&RunId("missing".into())),
                100,
                MAX_REPLAY_BYTES
            )
            .await,
        Err(EventStoreError::RunMismatch)
    ));
    assert_eq!(
        viewer
            .read_detail(&conversation, Some(&new), &detail)
            .await
            .unwrap(),
        None
    );
    // The byte bound stops before an unapplied event; retry resumes that event.
    for _ in 0..4 {
        store
            .append(&command(
                &sid,
                new.as_str(),
                Payload::Public(P::Planning {
                    text: text(&"x".repeat(32000)),
                }),
            ))
            .await
            .unwrap();
    }
    let bytes = viewer
        .read_replay(
            &conversation,
            Some(&new),
            5,
            None,
            100,
            agent_runtime::MIN_REPLAY_BYTES,
        )
        .await
        .unwrap();
    assert!(bytes.has_more);
    assert!(bytes.events.len() < 4);
    assert!(serde_json::to_vec(&bytes.events).unwrap().len() <= agent_runtime::MIN_REPLAY_BYTES);
    let next = viewer
        .read_replay(
            &conversation,
            Some(&new),
            bytes.through_cursor,
            None,
            100,
            MAX_REPLAY_BYTES,
        )
        .await
        .unwrap();
    assert_eq!(bytes.events.len() + next.events.len(), 4);
    // Projection ciphertext stays encrypted and excludes restricted payloads.
    let public_count = count(db, "conversation_read_projection", &sid).await;
    assert_eq!(public_count, 8);
    let encrypted: (String, Option<String>) = kyomi_core::db_fetch_one!(db, (String, Option<String>), "SELECT encrypted_payload,encrypted_detail FROM conversation_read_projection WHERE detail_id=$1", detail.as_str()).unwrap();
    assert!(!encrypted.0.contains("committed thinking"));
    assert!(!encrypted.1.unwrap().contains("complete independently"));
    // Re-run the actual migration against pre-projection journal rows on both
    // baselines, then repeat its insert to prove idempotence and encryption parity.
    match db {
        DbPool::Sqlite(pool) => {
            sqlx::query("DROP TABLE conversation_read_projection")
                .execute(pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM _sqlx_migrations WHERE version=48")
                .execute(pool)
                .await
                .unwrap();
            sqlx::migrate!("../../apps/server/migrations-sqlite")
                .run(pool)
                .await
                .unwrap();
        }
        DbPool::Postgres(pool) => {
            sqlx::query("DROP TABLE conversation_read_projection")
                .execute(pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM _sqlx_migrations WHERE version=20261011000000")
                .execute(pool)
                .await
                .unwrap();
            sqlx::migrate!("../../apps/server/migrations")
                .run(pool)
                .await
                .unwrap();
        }
    }
    assert_eq!(
        count(db, "conversation_read_projection", &sid).await,
        public_count
    );
    assert_eq!(
        viewer
            .read_detail(&conversation, Some(&old), &detail)
            .await
            .unwrap(),
        thinking.detail
    );
    let sql = include_str!(
        "../../../../../apps/server/migrations/20261011000000_durable_read_projection.sql"
    );
    let insert = &sql[sql.find("INSERT INTO").unwrap()..];
    kyomi_core::db_execute!(db, insert).unwrap();
    kyomi_core::db_execute!(db, insert).unwrap();
    assert_eq!(
        count(db, "conversation_read_projection", &sid).await,
        public_count
    );
    // Pruned journal forces a reset, but an uninterrupted reader/new replica can
    // recover complete public message/progress snapshots and full detail from DB.
    kyomi_core::db_execute!(
        db,
        "DELETE FROM conversation_events WHERE session_id=$1",
        &sid
    )
    .unwrap();
    assert!(matches!(
        viewer
            .read_replay(&conversation, None, 0, None, 100, MAX_REPLAY_BYTES)
            .await,
        Err(EventStoreError::CursorReset(CursorReset::Pruned))
    ));
    assert!(matches!(
        viewer
            .read_replay(&conversation, None, 999, None, 100, MAX_REPLAY_BYTES)
            .await,
        Err(EventStoreError::CursorReset(CursorReset::Invalid))
    ));
    let retained = viewer
        .snapshot(&conversation, Some(&old), 100, MAX_REPLAY_BYTES)
        .await
        .unwrap();
    assert_eq!(retained.run.unwrap().state, RunState::Interrupted);
    assert_eq!(retained.page.events.len(), 3);
    assert!(
        retained
            .page
            .events
            .iter()
            .any(|event| matches!(event.payload, P::Submitted { .. }))
    );
    assert!(
        retained
            .page
            .events
            .iter()
            .any(|event| matches!(event.payload, P::Planning { .. }))
    );
    assert_eq!(
        viewer
            .read_detail(&conversation, Some(&old), &detail)
            .await
            .unwrap(),
        thinking.detail
    );
    assert_eq!(
        viewer
            .snapshot(&conversation, None, 100, MAX_REPLAY_BYTES)
            .await
            .unwrap()
            .run
            .unwrap()
            .run_id,
        new
    );
    // Revocation denies every entry point before run-existence disclosure.
    kyomi_core::db_execute!(
        db,
        "UPDATE chat_sessions SET shared=false WHERE session_id=$1",
        &sid
    )
    .unwrap();
    for run in [old.clone(), RunId("does-not-exist".into())] {
        assert!(matches!(
            viewer
                .snapshot(&conversation, Some(&run), 100, MAX_REPLAY_BYTES)
                .await,
            Err(EventStoreError::Unauthorized)
        ));
    }
    assert!(matches!(
        viewer.read_detail(&conversation, Some(&old), &detail).await,
        Err(EventStoreError::Unauthorized)
    ));
    assert!(matches!(
        viewer
            .read_replay(&conversation, Some(&old), 9, None, 100, MAX_REPLAY_BYTES)
            .await,
        Err(EventStoreError::Unauthorized)
    ));
    kyomi_core::db_execute!(
        db,
        "UPDATE workspace_users SET active=false WHERE workspace_id=$1 AND user_id=$2",
        &wid,
        &owner
    )
    .unwrap();
    assert!(matches!(
        store
            .snapshot(&conversation, None, 100, MAX_REPLAY_BYTES)
            .await,
        Err(EventStoreError::Unauthorized)
    ));
    let unrelated = ConversationStore::new(db, &KEY, &member, "unrelated-workspace");
    assert!(matches!(
        unrelated
            .snapshot(&conversation, Some(&old), 100, MAX_REPLAY_BYTES)
            .await,
        Err(EventStoreError::Unauthorized)
    ));
    kyomi_core::db_execute!(
        db,
        "UPDATE workspace_users SET active=true WHERE workspace_id=$1 AND user_id=$2",
        &wid,
        &owner
    )
    .unwrap();
    assert!(
        crate::chat_service::delete_session(db, &owner, &sid, Some(&wid))
            .await
            .unwrap()
    );
    assert_eq!(count(db, "conversation_read_projection", &sid).await, 0);
    latest_run_uses_submission_identity(db).await;
    minimum_budget_reads_maximum_escaped_event(db).await;
}
#[tokio::test]
async fn sqlite_durable_read_contract() {
    durable_read_contract(&crate::test_support::test_pool().await).await;
}
#[tokio::test]
async fn postgres_durable_read_contract() {
    let Some(scratch) = IsolatedPostgres::connect("postgres_durable_read_contract").await else {
        return;
    };
    scratch.run(durable_read_contract(&scratch.db)).await;
    scratch.close().await;
}

async fn durable_delivery_ordering(db: &DbPool) {
    let (owner, member, wid, sid) = seed(db).await;
    let conversation = ConversationId(sid.clone());
    // Authorized flush holds the exact session/membership locks used by reads.
    let db_for_send = db.clone();
    let member_for_send = member.clone();
    let wid_for_send = wid.clone();
    let conversation_for_send = conversation.clone();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let delivery = tokio::spawn(async move {
        ConversationStore::new(&db_for_send, &KEY, &member_for_send, &wid_for_send)
            .deliver_authorized(&conversation_for_send, async {
                entered_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok(())
            })
            .await
    });
    entered_rx.await.unwrap();
    let db_for_revoke = db.clone();
    let sid_for_revoke = sid.clone();
    let (revoke_started_tx, revoke_started_rx) = tokio::sync::oneshot::channel();
    let mut revoke = tokio::spawn(async move {
        revoke_started_tx.send(()).unwrap();
        kyomi_core::db_execute!(
            &db_for_revoke,
            "UPDATE chat_sessions SET shared=false WHERE session_id=$1",
            &sid_for_revoke
        )
        .unwrap();
    });
    revoke_started_rx.await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut revoke)
            .await
            .is_err(),
        "completed revocation must not overtake authorized flush"
    );
    release_tx.send(()).unwrap();
    delivery.await.unwrap().unwrap();
    revoke.await.unwrap();
    let reader = ConversationStore::new(db, &KEY, &member, &wid);
    let polled = AtomicUsize::new(0);
    assert!(matches!(
        reader
            .deliver_authorized(&conversation, async {
                polled.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await,
        Err(EventStoreError::Unauthorized)
    ));
    assert_eq!(
        polled.load(Ordering::SeqCst),
        0,
        "revocation must prevent polling protected send"
    );
    // A failed flush rolls back and releases the lock, admitting the owner.
    let owner_store = ConversationStore::new(db, &KEY, &owner, &wid);
    assert!(matches!(
        owner_store
            .deliver_authorized(&conversation, async {
                Err::<(), _>(EventStoreError::DeliveryTimedOut)
            })
            .await,
        Err(EventStoreError::DeliveryTimedOut)
    ));
    owner_store.authorize_read(&conversation).await.unwrap();
}
#[tokio::test]
async fn sqlite_durable_delivery_ordering() {
    durable_delivery_ordering(&crate::test_support::test_pool().await).await;
}
#[tokio::test]
async fn postgres_durable_delivery_ordering() {
    let Some(scratch) = IsolatedPostgres::connect("postgres_durable_delivery_ordering").await
    else {
        return;
    };
    scratch.run(durable_delivery_ordering(&scratch.db)).await;
    scratch.close().await;
}

async fn latest_run_uses_submission_identity(db: &DbPool) {
    let (owner, _, wid, sid) = seed(db).await;
    let store = ConversationStore::new(db, &KEY, &owner, &wid);
    let older = RunId(uuid::Uuid::new_v4().to_string());
    let newer = RunId(uuid::Uuid::new_v4().to_string());
    for run in [&older, &newer] {
        store
            .append(&command(
                &sid,
                run.as_str(),
                Payload::Public(P::Submitted {
                    message_id: MessageId(uuid::Uuid::new_v4().to_string()),
                    text: text("accepted turn"),
                }),
            ))
            .await
            .unwrap();
    }
    // A delayed terminal write for an older accepted turn must never make it
    // the current run. Exact old identity remains independently readable.
    store
        .append(&command(
            &sid,
            older.as_str(),
            Payload::Public(P::Interrupted {
                text: text("late old-worker termination"),
            }),
        ))
        .await
        .unwrap();
    let conversation = ConversationId(sid);
    assert_eq!(
        store
            .snapshot(&conversation, None, 100, MAX_REPLAY_BYTES)
            .await
            .unwrap()
            .run
            .unwrap()
            .run_id,
        newer
    );
    assert_eq!(
        store
            .snapshot(&conversation, Some(&older), 100, MAX_REPLAY_BYTES)
            .await
            .unwrap()
            .run
            .unwrap()
            .state,
        RunState::Interrupted
    );
}

#[tokio::test]
async fn postgres_commit_during_snapshot_capture_cannot_create_a_gap() {
    let Some(scratch) =
        IsolatedPostgres::connect("postgres_commit_during_snapshot_capture_cannot_create_a_gap")
            .await
    else {
        return;
    };
    scratch.run(async {
        let db = &scratch.db;
        let (owner, _, wid, sid) = seed(db).await;
        let run = RunId(uuid::Uuid::new_v4().to_string());
        let store = ConversationStore::new(db, &KEY, &owner, &wid);
        store.append(&command(&sid, run.as_str(), Payload::Public(P::Submitted { message_id:MessageId(uuid::Uuid::new_v4().to_string()), text:text("before snapshot") }))).await.unwrap();
        let pool = crate::test_pg::postgres_pool(db);
        let mut guard = pool.acquire().await.unwrap();
        sqlx::query("SELECT pg_advisory_lock(884)").execute(&mut *guard).await.unwrap();
        sqlx::query("CREATE FUNCTION pause_snapshot() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(884); RETURN NEW; END $$").execute(pool).await.unwrap();
        sqlx::query("CREATE TRIGGER pause_snapshot BEFORE UPDATE ON chat_sessions FOR EACH ROW EXECUTE FUNCTION pause_snapshot()").execute(pool).await.unwrap();
        let db_for_snapshot = db.clone();
        let owner_for_snapshot = owner.clone();
        let wid_for_snapshot = wid.clone();
        let sid_for_snapshot = sid.clone();
        let snapshot = tokio::spawn(async move {
            ConversationStore::new(&db_for_snapshot, &KEY, &owner_for_snapshot, &wid_for_snapshot)
                .snapshot(&ConversationId(sid_for_snapshot), None, 100, MAX_REPLAY_BYTES).await
        });
        // Observe the actual production snapshot waiting inside its UPDATE trigger:
        // its session row lock has already been acquired, not just its future started.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let waiting: (bool,) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE a.datname=current_database() AND l.locktype='advisory' AND NOT l.granted)").fetch_one(pool).await.unwrap();
                if waiting.0 { break; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        let terminal = command(&sid, run.as_str(), Payload::Public(P::Interrupted { text:text("committed during capture") }));
        let db_for_writer = db.clone();
        let owner_for_writer = owner.clone();
        let wid_for_writer = wid.clone();
        let writer = tokio::spawn(async move {
            ConversationStore::new(&db_for_writer, &KEY, &owner_for_writer, &wid_for_writer).append(&terminal).await
        });
        sqlx::query("SELECT pg_advisory_unlock(884)").execute(&mut *guard).await.unwrap();
        let captured = snapshot.await.unwrap().unwrap();
        writer.await.unwrap().unwrap();
        assert_eq!(captured.through_cursor, 1);
        assert_eq!(captured.run.unwrap().state, RunState::Queued);
        assert_eq!(captured.page.events.len(), 1);
        let after = store.read_replay(&ConversationId(sid), None, captured.through_cursor, None, 100, MAX_REPLAY_BYTES).await.unwrap();
        assert_eq!((after.from_cursor, after.through_cursor), (1, 2));
        assert!(matches!(after.events[0].payload, P::Interrupted { .. }));
    }).await;
    scratch.close().await;
}

async fn minimum_budget_reads_maximum_escaped_event(db: &DbPool) {
    let (owner, member, wid, old_sid) = seed(db).await;
    let identity = "\u{1}".repeat(agent_runtime::MAX_IDENTITY_BYTES);
    // PostgreSQL's existing chat_sessions key is VARCHAR(50); run/event
    // identities retain the runtime's full128-byte limit on both backends.
    let session = "\u{1}".repeat(if db.is_postgres() {
        50
    } else {
        agent_runtime::MAX_IDENTITY_BYTES
    });
    kyomi_core::db_execute!(
        db,
        "UPDATE chat_sessions SET session_id=$1 WHERE session_id=$2",
        &session,
        &old_sid
    )
    .unwrap();
    let store = ConversationStore::new(db, &KEY, &owner, &wid);
    let viewer = ConversationStore::new(db, &KEY, &member, &wid);
    let run = RunId(identity.clone());
    let conversation = ConversationId(session.clone());
    store
        .append(&command(
            &session,
            &identity,
            Payload::Public(P::Submitted {
                message_id: MessageId(uuid::Uuid::new_v4().to_string()),
                text: text("maximum event fixture"),
            }),
        ))
        .await
        .unwrap();
    let mut maximum = command(
        &session,
        &identity,
        Payload::Public(P::Planning { text: text("") }),
    );
    maximum.event_id = EventId(identity);
    let overhead = serde_json::to_vec(&maximum.payload).unwrap().len();
    let Payload::Public(P::Planning { text }) = &mut maximum.payload else {
        unreachable!()
    };
    text.preview = "x".repeat(MAX_EVENT_BYTES - overhead);
    assert_eq!(
        serde_json::to_vec(&maximum.payload).unwrap().len(),
        MAX_EVENT_BYTES
    );
    agent_runtime::validate(&maximum).unwrap();
    store.append(&maximum).await.unwrap();
    let budget = agent_runtime::MIN_REPLAY_BYTES;
    let snapshot = viewer
        .snapshot(&conversation, Some(&run), 1, budget)
        .await
        .unwrap();
    assert_eq!(snapshot.page.through_cursor, 1);
    let continuation = viewer
        .read_replay(
            &conversation,
            Some(&run),
            1,
            Some(snapshot.through_cursor),
            1,
            budget,
        )
        .await
        .unwrap();
    let replay = viewer
        .read_replay(&conversation, Some(&run), 1, None, 1, budget)
        .await
        .unwrap();
    assert_eq!(continuation.events, replay.events);
    assert_eq!(replay.through_cursor, 2);
    assert_eq!(replay.events.len(), 1);
    assert!(serde_json::to_vec(&replay.events).unwrap().len() <= budget);
    assert!(matches!(
        viewer
            .snapshot(&conversation, Some(&run), 1, budget - 1)
            .await,
        Err(EventStoreError::InvalidReplay)
    ));
}
