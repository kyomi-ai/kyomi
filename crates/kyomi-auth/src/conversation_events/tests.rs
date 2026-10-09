use super::*;
use agent_runtime::{
    IdempotencyKey, MessageId, ModelCallId, PublicPayload as P, RestrictedPayload, RunId, Text,
    ToolCallId, Usage,
};
use std::sync::atomic::{AtomicUsize, Ordering};
const KEY: [u8; 32] = [42; 32];
struct Notify {
    calls: AtomicUsize,
    fail: bool,
}
#[async_trait::async_trait]
impl agent_runtime::Notification for Notify {
    type Error = &'static str;
    async fn committed(
        &self,
        _: &ConversationId,
        _: &CommitReceipt,
    ) -> std::result::Result<(), Self::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err("notification offline")
        } else {
            Ok(())
        }
    }
}
fn text(value: &str) -> Text {
    Text {
        preview: value.into(),
        detail: None,
    }
}
fn command(sid: &str, run: &str, payload: Payload) -> AppendCommand {
    let id = uuid::Uuid::new_v4().to_string();
    AppendCommand {
        version: VERSION,
        conversation_id: ConversationId(sid.into()),
        run_id: RunId(run.into()),
        event_id: EventId(id.clone()),
        idempotency_key: IdempotencyKey(id),
        payload,
        detail: None,
    }
}
async fn seed(db: &DbPool) -> (String, String, String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    let owner = format!("owner-{id}");
    let member = format!("member-{id}");
    let wid = format!("ws-{id}");
    let sid = format!("session-{id}");
    for user in [&owner, &member] {
        kyomi_core::db_execute!(
            db,
            "INSERT INTO users(user_id,email,active) VALUES ($1,$2,true)",
            user,
            format!("{user}@example.test")
        )
        .unwrap();
    }
    kyomi_core::db_execute!(
        db,
        "INSERT INTO workspaces(workspace_id,name,owner_user_id) VALUES ($1,'test',$2)",
        &wid,
        &owner
    )
    .unwrap();
    for user in [&owner, &member] {
        kyomi_core::db_execute!(db,"INSERT INTO workspace_users(workspace_id,user_id,role,active) VALUES ($1,$2,'member',true)",&wid,user).unwrap();
    }
    kyomi_core::db_execute!(
        db,
        "INSERT INTO chat_sessions(session_id,user_id,workspace_id,shared) VALUES ($1,$2,$3,true)",
        &sid,
        &owner,
        &wid
    )
    .unwrap();
    (owner, member, wid, sid)
}
async fn count(db: &DbPool, table: &str, sid: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE session_id = $1");
    kyomi_core::db_fetch_one!(db, (i64,), &sql, sid).unwrap().0
}
async fn conformance(db: &DbPool) {
    let (owner, member, wid, sid) = seed(db).await;
    let store = ConversationStore::new(db, &KEY, &owner, &wid);
    let viewer = ConversationStore::new(db, &KEY, &member, &wid);
    let run = uuid::Uuid::new_v4().to_string();
    let message = MessageId(uuid::Uuid::new_v4().to_string());
    let submitted = command(
        &sid,
        &run,
        Payload::Public(P::Submitted {
            message_id: message.clone(),
            text: text("private user content"),
        }),
    );
    let notify = Notify {
        calls: AtomicUsize::new(0),
        fail: true,
    };
    assert!(matches!(
        agent_runtime::append(&store, &notify, &submitted)
            .await
            .unwrap(),
        agent_runtime::AppendOutcome::CommittedButNotNotified { .. }
    ));
    let retry = store.append(&submitted).await.unwrap();
    assert!(retry.duplicate);
    assert_eq!(retry.sequence, 1);
    let mut conflict = submitted.clone();
    conflict.payload = Payload::Public(P::Planning {
        text: text("different"),
    });
    assert!(matches!(
        store.append(&conflict).await,
        Err(EventStoreError::IdempotencyConflict)
    ));
    assert!(matches!(
        viewer.append(&submitted).await,
        Err(EventStoreError::Unauthorized)
    ));
    let restricted = command(
        &sid,
        &run,
        Payload::Restricted(RestrictedPayload::ProviderResponse {
            model_call_id: ModelCallId("model-1".into()),
            response: serde_json::json!({"raw":"secret provider block"}),
            continuation: serde_json::json!({"opaque":"continuation-token"}),
        }),
    );
    assert_eq!(store.append(&restricted).await.unwrap().sequence, 2);
    let detail = DetailId(uuid::Uuid::new_v4().to_string());
    let mut planning = command(
        &sid,
        &run,
        Payload::Public(P::Planning {
            text: Text {
                preview: "exposed preview".into(),
                detail: Some(detail.clone()),
            },
        }),
    );
    planning.detail = Some("full exposed planning content".repeat(5000));
    assert_eq!(store.append(&planning).await.unwrap().sequence, 3);
    assert_eq!(
        viewer
            .detail(&ConversationId(sid.clone()), &detail)
            .await
            .unwrap(),
        planning.detail
    );
    let first = viewer
        .replay(&ConversationId(sid.clone()), 0, 2)
        .await
        .unwrap();
    assert_eq!(first.events.len(), 1);
    assert_eq!(first.scanned_through, 2);
    assert!(first.has_more);
    assert!(
        !serde_json::to_string(&first)
            .unwrap()
            .contains("continuation")
    );
    let recovered = store
        .restricted(&ConversationId(sid.clone()), 0, 10)
        .await
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].payload,
        match &restricted.payload {
            Payload::Restricted(p) => p.clone(),
            _ => panic!("restricted fixture"),
        }
    );
    let raw_detail = kyomi_core::db_fetch_one!(
        db,
        (String,),
        "SELECT encrypted_content FROM conversation_event_details WHERE detail_id = $1",
        detail.as_str()
    )
    .unwrap()
    .0;
    assert!(!raw_detail.contains("full exposed planning content"));
    assert_eq!(
        encryption::decrypt(&raw_detail, &KEY).unwrap(),
        planning.detail.clone().unwrap()
    );
    assert!(matches!(
        viewer.restricted(&ConversationId(sid.clone()), 0, 10).await,
        Err(EventStoreError::Unauthorized)
    ));
    // A real projection constraint failure occurs after event and run state writes.
    let failed = command(
        &sid,
        &run,
        Payload::Public(P::ApprovedAnswer {
            message_id: message,
            text: text("failed answer"),
        }),
    );
    assert!(
        agent_runtime::append(&store, &notify, &failed)
            .await
            .is_err()
    );
    assert_eq!(notify.calls.load(Ordering::SeqCst), 1);
    assert_eq!(count(db, "conversation_events", &sid).await, 3);
    let state = kyomi_core::db_fetch_one!(
        db,
        (String,),
        "SELECT state FROM conversation_runs WHERE session_id = $1",
        &sid
    )
    .unwrap()
    .0;
    assert_eq!(state, "queued");
    let usage = command(
        &sid,
        &run,
        Payload::Public(P::Usage {
            cost: None,
            model_call_id: ModelCallId("model-1".into()),
            usage: Usage {
                input_tokens: 12,
                output_tokens: 7,
            },
        }),
    );
    assert_eq!(store.append(&usage).await.unwrap().sequence, 4);
    let repeated_usage = command(&sid, &run, usage.payload.clone());
    assert_eq!(store.append(&repeated_usage).await.unwrap().sequence, 4);
    assert_eq!(count(db, "conversation_usage_receipts", &sid).await, 1);
    let mut alias_conflict = repeated_usage.clone();
    alias_conflict.payload = Payload::Public(P::Planning {
        text: text("cannot reuse an acknowledged usage alias"),
    });
    assert!(matches!(
        store.append(&alias_conflict).await,
        Err(EventStoreError::IdempotencyConflict)
    ));
    assert!(store.append(&repeated_usage).await.unwrap().duplicate);

    let receipt = command(
        &sid,
        &run,
        Payload::Public(P::ToolResult {
            tool_call_id: ToolCallId("call-1".into()),
            text: text("tool finished"),
            succeeded: true,
        }),
    );
    assert_eq!(store.append(&receipt).await.unwrap().sequence, 5);
    let repeated_tool = command(&sid, &run, receipt.payload.clone());
    assert!(store.append(&repeated_tool).await.unwrap().duplicate);
    let mut tool_alias_conflict = repeated_tool.clone();
    tool_alias_conflict.payload = Payload::Public(P::ToolResult {
        tool_call_id: ToolCallId("new-call-under-reused-alias".into()),
        text: text("changed retry"),
        succeeded: true,
    });
    assert!(matches!(
        store.append(&tool_alias_conflict).await,
        Err(EventStoreError::IdempotencyConflict)
    ));
    assert!(store.append(&repeated_tool).await.unwrap().duplicate);

    // Inject a database error at the final counter update, after every event/state/
    // projection write has succeeded. Both native backends must roll all of it back.
    let trigger = format!("journal_fail_{}", uuid::Uuid::new_v4().simple());
    if db.is_postgres() {
        let function = format!(
            "CREATE FUNCTION {trigger}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected journal failure'; END $$"
        );
        kyomi_core::db_execute!(db, &function).unwrap();
        let sql = format!(
            "CREATE TRIGGER {trigger} BEFORE UPDATE OF event_sequence ON chat_sessions FOR EACH ROW WHEN (NEW.session_id = '{sid}' AND NEW.event_sequence > OLD.event_sequence) EXECUTE FUNCTION {trigger}()"
        );
        kyomi_core::db_execute!(db, &sql).unwrap();
    } else {
        let sql = format!(
            "CREATE TRIGGER {trigger} BEFORE UPDATE OF event_sequence ON chat_sessions WHEN NEW.session_id = '{sid}' AND NEW.event_sequence > OLD.event_sequence BEGIN SELECT RAISE(ABORT, 'injected journal failure'); END"
        );
        kyomi_core::db_execute!(db, &sql).unwrap();
    }
    let injected = command(
        &sid,
        &run,
        Payload::Public(P::ApprovedAnswer {
            message_id: MessageId(uuid::Uuid::new_v4().to_string()),
            text: text("must roll back"),
        }),
    );
    assert!(
        agent_runtime::append(&store, &notify, &injected)
            .await
            .is_err()
    );
    assert_eq!(notify.calls.load(Ordering::SeqCst), 1);
    assert_eq!(count(db, "conversation_events", &sid).await, 5);
    assert_eq!(count(db, "chat_messages", &sid).await, 1);
    assert_eq!(
        store
            .replay(&ConversationId(sid.clone()), 0, 100)
            .await
            .unwrap()
            .high_watermark,
        5
    );
    let drop_trigger = if db.is_postgres() {
        format!("DROP TRIGGER {trigger} ON chat_sessions")
    } else {
        format!("DROP TRIGGER {trigger}")
    };
    kyomi_core::db_execute!(db, &drop_trigger).unwrap();
    if db.is_postgres() {
        kyomi_core::db_execute!(db, &format!("DROP FUNCTION {trigger}()")).unwrap();
    }
    let answer = command(
        &sid,
        &run,
        Payload::Public(P::ApprovedAnswer {
            message_id: MessageId(uuid::Uuid::new_v4().to_string()),
            text: text("approved answer"),
        }),
    );
    assert_eq!(store.append(&answer).await.unwrap().sequence, 6);
    assert!(store.append(&answer).await.unwrap().duplicate);
    assert!(matches!(
        store
            .append(&command(
                &sid,
                &run,
                Payload::Public(P::RunState {
                    state: RunState::Running
                })
            ))
            .await,
        Err(EventStoreError::Policy(
            agent_runtime::PolicyError::Terminal
        ))
    ));
    assert_eq!(count(db, "chat_messages", &sid).await, 2);
    let rows = kyomi_core::db_fetch_all!(
        db,
        (String, String),
        "SELECT encrypted_payload,encrypted_command FROM conversation_events WHERE session_id = $1",
        &sid
    )
    .unwrap();
    for (payload, command) in rows {
        assert!(!payload.contains("private user content"));
        assert!(!command.contains("continuation-token"));
        encryption::decrypt(&payload, &KEY).unwrap();
    }
    // Revocation applies immediately even to previously authorized detail reads.
    kyomi_core::db_execute!(
        db,
        "UPDATE workspace_users SET active = false WHERE workspace_id = $1 AND user_id = $2",
        &wid,
        &member
    )
    .unwrap();
    assert!(matches!(
        viewer.detail(&ConversationId(sid.clone()), &detail).await,
        Err(EventStoreError::Unauthorized)
    ));
    assert!(matches!(
        viewer.replay(&ConversationId(sid.clone()), 0, 10).await,
        Err(EventStoreError::Unauthorized)
    ));
    kyomi_core::db_execute!(
        db,
        "UPDATE conversation_events SET version = 99 WHERE event_id = $1",
        restricted.event_id.as_str()
    )
    .unwrap();
    assert!(matches!(
        store.replay(&ConversationId(sid.clone()), 1, 10).await,
        Err(EventStoreError::UnsupportedVersion(99))
    ));
    kyomi_core::db_execute!(
        db,
        "UPDATE workspace_users SET active = false WHERE workspace_id = $1 AND user_id = $2",
        &wid,
        &owner
    )
    .unwrap();
    assert!(matches!(
        store.replay(&ConversationId(sid.clone()), 0, 10).await,
        Err(EventStoreError::Unauthorized)
    ));
    assert!(matches!(
        store.restricted(&ConversationId(sid.clone()), 0, 10).await,
        Err(EventStoreError::Unauthorized)
    ));
    kyomi_core::db_execute!(
        db,
        "UPDATE workspace_users SET active = true WHERE workspace_id = $1 AND user_id = $2",
        &wid,
        &owner
    )
    .unwrap();
    kyomi_core::db_execute!(
        db,
        "UPDATE users SET active = false WHERE user_id = $1",
        &owner
    )
    .unwrap();
    assert!(matches!(
        store.detail(&ConversationId(sid.clone()), &detail).await,
        Err(EventStoreError::Unauthorized)
    ));
    kyomi_core::db_execute!(
        db,
        "UPDATE users SET active = true WHERE user_id = $1",
        &owner
    )
    .unwrap();
    // Existing deletion retains its sync-log behavior; journal data follows FK cascades.
    assert!(
        crate::chat_service::delete_session(db, &owner, &sid, Some(&wid))
            .await
            .unwrap()
    );
    for table in [
        "conversation_events",
        "conversation_event_aliases",
        "conversation_runs",
        "conversation_event_details",
        "conversation_usage_receipts",
        "conversation_tool_receipts",
        "chat_messages",
    ] {
        assert_eq!(count(db, table, &sid).await, 0, "{table}");
    }
}
#[tokio::test]
async fn sqlite_journal_conformance() {
    conformance(&crate::test_support::test_pool().await).await;
}
#[tokio::test]
async fn postgres_journal_conformance() {
    let Some(db) = crate::test_pg::postgres_test_pool_or_skip("postgres_journal_conformance").await
    else {
        return;
    };
    conformance(&db).await;
}

async fn concurrent_writers(db: &DbPool, other: &DbPool) {
    let (owner, _, wid, sid) = seed(db).await;
    let a = ConversationStore::new(db, &KEY, &owner, &wid);
    let b = ConversationStore::new(other, &KEY, &owner, &wid);
    let mut commands = Vec::new();
    for _ in 0..12 {
        commands.push(command(
            &sid,
            &uuid::Uuid::new_v4().to_string(),
            Payload::Public(P::Submitted {
                message_id: MessageId(uuid::Uuid::new_v4().to_string()),
                text: text("concurrent submission"),
            }),
        ));
    }
    let results = futures_util::future::join_all(commands.iter().enumerate().map(|(i, c)| {
        let store = if i % 2 == 0 { &a } else { &b };
        async move { store.append(c).await.unwrap() }
    }))
    .await;
    let mut sequences: Vec<_> = results.iter().map(|r| r.sequence).collect();
    sequences.sort();
    assert_eq!(sequences, (1..=12).collect::<Vec<_>>());
    let c = command(
        &sid,
        &uuid::Uuid::new_v4().to_string(),
        Payload::Public(P::Submitted {
            message_id: MessageId(uuid::Uuid::new_v4().to_string()),
            text: text("simultaneous first retry"),
        }),
    );
    let (one, two) = tokio::join!(a.append(&c), b.append(&c));
    let (one, two) = (one.unwrap(), two.unwrap());
    assert_eq!(one.sequence, 13);
    assert_eq!(one.sequence, two.sequence);
    assert_ne!(one.duplicate, two.duplicate);
    assert_eq!(count(db, "conversation_events", &sid).await, 13);
    let replay = a
        .replay(&ConversationId(sid.clone()), 0, 100)
        .await
        .unwrap();
    assert_eq!(
        replay.events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        (1..=13).collect::<Vec<_>>()
    );
    held_commit_order(db, other).await;
    crate::chat_service::delete_session(db, &owner, &sid, Some(&wid))
        .await
        .unwrap();
}
#[tokio::test]
async fn sqlite_independent_connections_serialize_concurrent_writers() {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    let path = std::env::temp_dir().join(format!("kyo881-{}.sqlite", uuid::Uuid::new_v4()));
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(30));
    let first = SqlitePoolOptions::new()
        .max_connections(6)
        .connect_with(options.clone())
        .await
        .unwrap();
    sqlx::migrate!("../../apps/server/migrations-sqlite")
        .run(&first)
        .await
        .unwrap();
    let second = SqlitePoolOptions::new()
        .max_connections(6)
        .connect_with(options)
        .await
        .unwrap();
    concurrent_writers(
        &DbPool::Sqlite(first.clone()),
        &DbPool::Sqlite(second.clone()),
    )
    .await;
    first.close().await;
    second.close().await;
    std::fs::remove_file(path).unwrap();
}
#[tokio::test]
async fn postgres_connections_serialize_concurrent_writers() {
    let Some(db) = crate::test_pg::postgres_test_pool_or_skip(
        "postgres_connections_serialize_concurrent_writers",
    )
    .await
    else {
        return;
    };
    concurrent_writers(&db, &db).await;
}

fn before_journal(mut full: sqlx::migrate::Migrator) -> sqlx::migrate::Migrator {
    full.migrations = std::borrow::Cow::Owned(
        full.iter()
            .filter(|m| {
                !m.description.contains("durable conversation journal")
                    && !m.description.contains("durable run lifecycle")
            })
            .cloned()
            .collect(),
    );
    full
}
#[tokio::test]
async fn sqlite_upgrade_preserves_existing_chat_history() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&pool)
        .await
        .unwrap();
    before_journal(sqlx::migrate!("../../apps/server/migrations-sqlite"))
        .run(&pool)
        .await
        .unwrap();
    let absent: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM sqlite_master WHERE name = 'conversation_events'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(absent.0, 0);
    let db = DbPool::Sqlite(pool.clone());
    let (owner, _, wid, sid) = seed(&db).await;
    let message = uuid::Uuid::new_v4().to_string();
    kyomi_core::db_execute!(&db,"INSERT INTO chat_messages(message_id,session_id,role,content) VALUES ($1,$2,'user','legacy ciphertext')",&message,&sid).unwrap();
    sqlx::migrate!("../../apps/server/migrations-sqlite")
        .run(&pool)
        .await
        .unwrap();
    assert_eq!(count(&db, "chat_messages", &sid).await, 1);
    assert_eq!(
        kyomi_core::db_fetch_one!(
            &db,
            (String,),
            "SELECT content FROM chat_messages WHERE session_id = $1",
            &sid
        )
        .unwrap()
        .0,
        "legacy ciphertext"
    );
    assert_eq!(
        ConversationStore::new(&db, &KEY, &owner, &wid)
            .replay(&ConversationId(sid.clone()), 0, 10)
            .await
            .unwrap()
            .high_watermark,
        0
    );
}
#[tokio::test]
async fn postgres_fresh_and_upgrade_preserve_existing_history() {
    let Some(db) = crate::test_pg::postgres_test_pool_or_skip(
        "postgres_fresh_and_upgrade_preserve_existing_history",
    )
    .await
    else {
        return;
    };
    drop(db);
    let base_url = kyomi_core::test_db::test_database_url();
    let (server_url, _) = kyomi_core::test_db::split_database_url(&base_url);
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&format!("{server_url}/postgres"))
        .await
        .unwrap();
    // Older migrations explicitly target public. Fresh databases, rather than search_path
    // overrides, preserve the actual native migration semantics without touching another suite.
    for upgrade in [false, true] {
        let name = format!("journal_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(&admin)
            .await
            .unwrap();
        let scratch = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&format!("{server_url}/{name}"))
            .await
            .unwrap();
        if upgrade {
            before_journal(sqlx::migrate!("../../apps/server/migrations"))
                .run(&scratch)
                .await
                .unwrap();
            let absent:(i64,)=sqlx::query_as("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'public' AND table_name = 'conversation_events'").fetch_one(&scratch).await.unwrap();
            assert_eq!(absent.0, 0);
        } else {
            sqlx::migrate!("../../apps/server/migrations")
                .run(&scratch)
                .await
                .unwrap();
        }
        let db = DbPool::Postgres(scratch.clone());
        let (owner, _, wid, sid) = seed(&db).await;
        kyomi_core::db_execute!(&db,"INSERT INTO chat_messages(message_id,session_id,role,content) VALUES ($1,$2,'user','legacy ciphertext')",uuid::Uuid::new_v4().to_string(),&sid).unwrap();
        if upgrade {
            sqlx::migrate!("../../apps/server/migrations")
                .run(&scratch)
                .await
                .unwrap();
        }
        assert_eq!(count(&db, "chat_messages", &sid).await, 1);
        assert_eq!(
            kyomi_core::db_fetch_one!(
                &db,
                (String,),
                "SELECT content FROM chat_messages WHERE session_id = $1",
                &sid
            )
            .unwrap()
            .0,
            "legacy ciphertext"
        );
        assert_eq!(
            ConversationStore::new(&db, &KEY, &owner, &wid)
                .replay(&ConversationId(sid), 0, 10)
                .await
                .unwrap()
                .high_watermark,
            0
        );
        drop(db);
        scratch.close().await;
        sqlx::query(&format!("DROP DATABASE {name}"))
            .execute(&admin)
            .await
            .unwrap();
    }
}

async fn held_commit_order(db: &DbPool, other: &DbPool) {
    let (owner, _, wid, sid) = seed(db).await;
    let first = command(
        &sid,
        &uuid::Uuid::new_v4().to_string(),
        Payload::Public(P::Submitted {
            message_id: MessageId(uuid::Uuid::new_v4().to_string()),
            text: text("held first commit"),
        }),
    );
    let second = command(
        &sid,
        &uuid::Uuid::new_v4().to_string(),
        Payload::Public(P::Submitted {
            message_id: MessageId(uuid::Uuid::new_v4().to_string()),
            text: text("must commit second"),
        }),
    );
    // A deliberately held native transaction installs one valid journal record under
    // the same parent-row lock as the adapter. The second real append must wait for its
    // commit, and independent readers must see neither its cursor nor its event early.
    macro_rules! held {
        ($tx:ident) => {{
            sqlx::query("UPDATE chat_sessions SET event_sequence = 1 WHERE session_id = $1").bind(&sid).execute(&mut *$tx).await.unwrap();
            let plan=agent_runtime::plan(&first,None).unwrap();
            sqlx::query("INSERT INTO conversation_runs(run_id,session_id,state) VALUES ($1,$2,'queued')").bind(first.run_id.as_str()).bind(&sid).execute(&mut *$tx).await.unwrap();
            let encrypted=encryption::encrypt(&serde_json::to_string(&first.payload).unwrap(),&KEY).unwrap();
            let hash=encryption::encrypt(&hex::encode(Sha256::digest(serde_json::to_vec(&first).unwrap())),&KEY).unwrap();
            sqlx::query("INSERT INTO conversation_events(event_id,session_id,run_id,sequence,version,idempotency_key,public,encrypted_payload,encrypted_command) VALUES ($1,$2,$3,1,1,$4,true,$5,$6)").bind(first.event_id.as_str()).bind(&sid).bind(first.run_id.as_str()).bind(first.idempotency_key.as_str()).bind(encrypted).bind(&hash).execute(&mut *$tx).await.unwrap();
            sqlx::query("INSERT INTO conversation_event_aliases(session_id,idempotency_key,alias_event_id,event_id,encrypted_command) VALUES ($1,$2,$3,$3,$4)").bind(&sid).bind(first.idempotency_key.as_str()).bind(first.event_id.as_str()).bind(hash).execute(&mut *$tx).await.unwrap();
            let projection=plan.projection.unwrap();
            sqlx::query("INSERT INTO chat_messages(message_id,session_id,role,content,status,pinned,created_at) VALUES ($1,$2,'user',$3,'complete',false,$4)").bind(projection.message_id.as_str()).bind(&sid).bind(encryption::encrypt(&projection.content,&KEY).unwrap()).bind(chrono::Utc::now()).execute(&mut *$tx).await.unwrap();
            let writer_db=other.clone();let writer_owner=owner.clone();let writer_wid=wid.clone();let writer_command=second.clone();
            let mut pending=tokio::spawn(async move {ConversationStore::new(&writer_db,&KEY,&writer_owner,&writer_wid).append(&writer_command).await});
            assert!(tokio::time::timeout(std::time::Duration::from_millis(100),&mut pending).await.is_err(),"append escaped the held session lock");
            assert_eq!(count(other,"conversation_events",&sid).await,0,"uncommitted event became observable");
            assert_eq!(kyomi_core::db_fetch_one!(other,(i64,),"SELECT event_sequence FROM chat_sessions WHERE session_id = $1",&sid).unwrap().0,0,"uncommitted cursor became observable");
            $tx.commit().await.unwrap();
            assert_eq!(pending.await.unwrap().unwrap().sequence,2);
        }};
    }
    match db {
        DbPool::Postgres(pool) => {
            let mut tx = pool.begin().await.unwrap();
            held!(tx);
        }
        DbPool::Sqlite(pool) => {
            let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
            held!(tx);
        }
    }
    let replay = ConversationStore::new(db, &KEY, &owner, &wid)
        .replay(&ConversationId(sid.clone()), 0, 10)
        .await
        .unwrap();
    assert_eq!(
        replay
            .events
            .iter()
            .map(|e| e.event_id.clone())
            .collect::<Vec<_>>(),
        vec![first.event_id, second.event_id]
    );
    crate::chat_service::delete_session(db, &owner, &sid, Some(&wid))
        .await
        .unwrap();
}

fn lifecycle_submission(
    owner: &str,
    sid: &str,
    request: &str,
    body: &str,
    at: i64,
) -> agent_runtime::SubmitCommand {
    agent_runtime::SubmitCommand {
        submitted: command(
            sid,
            &uuid::Uuid::new_v4().to_string(),
            Payload::Public(P::Submitted {
                message_id: MessageId(uuid::Uuid::new_v4().to_string()),
                text: text(body),
            }),
        ),
        assistant_message_id: MessageId(uuid::Uuid::new_v4().to_string()),
        request_id: IdempotencyKey(request.into()),
        context: serde_json::json!({"current_time_user_tz":"2026-10-04T12:00:00Z","model_name":"fixture"}),
        actor: agent_runtime::ActorIdentity {
            actor_id: owner.into(),
            source: "test".into(),
        },
        submitted_at: at,
        new_conversation: false,
    }
}
async fn lifecycle_conformance(db: &DbPool, other: &DbPool) {
    let (owner, member, wid, sid) = seed(db).await;
    let store = ConversationStore::new(db, &KEY, &owner, &wid).with_deterministic_time();
    let replica = ConversationStore::new(other, &KEY, &owner, &wid).with_deterministic_time();
    let viewer = ConversationStore::new(db, &KEY, &member, &wid).with_deterministic_time();
    let conversation = ConversationId(sid.clone());
    let first = lifecycle_submission(&owner, &sid, "first-request", "first prompt", 1);
    let receipt = store.submit(&first).await.unwrap();
    let regenerated = lifecycle_submission(&owner, &sid, "first-request", "first prompt", 999);
    let retry = replica.submit(&regenerated).await.unwrap();
    assert!(retry.duplicate);
    assert_eq!(retry.run_id, receipt.run_id);
    assert_eq!(retry.user_message_id, receipt.user_message_id);
    assert_eq!(retry.assistant_message_id, receipt.assistant_message_id);
    assert_eq!(count(db, "conversation_runs", &sid).await, 1);
    assert_eq!(count(db, "chat_messages", &sid).await, 2);
    assert_eq!(count(db, "conversation_events", &sid).await, 2);
    let mut conflict = regenerated.clone();
    conflict.context["model_name"] = serde_json::json!("changed");
    assert!(store.submit(&conflict).await.is_err());
    assert!(
        viewer
            .cancel(&conversation, &receipt.run_id, 10)
            .await
            .is_err()
    );
    assert!(
        viewer
            .claim(&conversation, &receipt.run_id, "worker-b", 10, 100)
            .await
            .is_err()
    );
    assert!(
        discover_queue(db, 100)
            .await
            .unwrap()
            .iter()
            .any(|r| r.run_id == receipt.run_id.0)
    );
    // A duplicate assistant PK fails after Submitted projection/event/run were inserted.
    let mut broken = lifecycle_submission(&owner, &sid, "rollback-request", "must roll back", 2);
    broken.assistant_message_id = receipt.assistant_message_id.clone();
    assert!(store.submit(&broken).await.is_err());
    assert_eq!(count(db, "conversation_runs", &sid).await, 1);
    assert_eq!(count(db, "chat_messages", &sid).await, 2);
    assert_eq!(count(db, "conversation_events", &sid).await, 2);
    let second = lifecycle_submission(&owner, &sid, "second-request", "later queued prompt", 2);
    let second_receipt = store.submit(&second).await.unwrap();
    assert!(
        store
            .claim(&conversation, &second_receipt.run_id, "worker-b", 1000, 100)
            .await
            .is_err(),
        "queue cannot bypass earlier accepted turn"
    );
    let (claim_a, claim_b) = tokio::join!(
        store.claim(&conversation, &receipt.run_id, "worker-a", 1000, 100),
        replica.claim(&conversation, &receipt.run_id, "worker-b", 1000, 100)
    );
    assert_eq!(
        usize::from(claim_a.is_ok()) + usize::from(claim_b.is_ok()),
        1
    );
    let claimed = claim_a.or(claim_b).unwrap();
    assert!(
        claimed.context.messages.is_empty(),
        "claim snapshot excludes own accepted prompt and future queued prompts"
    );
    let lease = claimed.snapshot.lease.unwrap();
    let history = crate::chat_service::get_agent_messages(db, &KEY, &sid, None)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].content, "first prompt",
        "later queued user message must not enter committed claim context"
    );
    assert!(
        store
            .claim(&conversation, &second_receipt.run_id, "worker-b", 1010, 100)
            .await
            .is_err()
    );
    let renewed = store
        .heartbeat(&conversation, &receipt.run_id, &lease, 1050, 100)
        .await
        .unwrap();
    assert_eq!(renewed.lease.as_ref().unwrap().expires_at, 1150);
    assert_eq!(
        replica
            .expire(&conversation, &receipt.run_id, 1100)
            .await
            .unwrap()
            .state,
        RunState::Running,
        "another replica cannot interrupt a live lease"
    );
    let compaction = serde_json::json!({"agent_state":{"global_iteration":4,"compacted_summary":"committed prior summary"}});
    store
        .fenced_session_metadata(&conversation, &receipt.run_id, &lease, 1090, &compaction)
        .await
        .unwrap();
    let message = MessageWrite {
        message_id: uuid::Uuid::new_v4().to_string(),
        role: "assistant".into(),
        content: "completed intermediate response".into(),
        tool_call_id: None,
        name: None,
        tool_calls: Some(
            serde_json::json!([{"id":"fixture-tool-call","type":"function","function":{"name":"fixture_lookup","arguments":"{}"}}]),
        ),
    };
    assert!(
        !store
            .fenced_message(&conversation, &receipt.run_id, &lease, 1100, &message)
            .await
            .unwrap()
            .duplicate
    );
    assert!(
        store
            .fenced_message(&conversation, &receipt.run_id, &lease, 1101, &message)
            .await
            .unwrap()
            .duplicate
    );
    assert_eq!(
        replica
            .expire(&conversation, &receipt.run_id, 1150)
            .await
            .unwrap()
            .state,
        RunState::Interrupted
    );
    assert!(
        store
            .fenced_message(&conversation, &receipt.run_id, &lease, 1151, &message)
            .await
            .is_err()
    );
    assert!(
        store
            .finish(
                &conversation,
                &receipt.run_id,
                &lease,
                1151,
                RunState::Completed,
                "late winner"
            )
            .await
            .is_err()
    );
    let terminal = crate::chat_service::get_session_messages(db, &KEY, &sid, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.message_id == receipt.assistant_message_id.0)
        .unwrap();
    assert_eq!(terminal.status, "interrupted");
    assert_eq!(terminal.content, "Interrupted");
    let next = replica
        .claim(&conversation, &second_receipt.run_id, "worker-b", 1200, 100)
        .await
        .unwrap();
    assert_eq!(next.context.config, Some(compaction));
    assert!(
        next.context
            .messages
            .iter()
            .any(|m| m.content == "Interrupted")
    );
    assert!(
        !next
            .context
            .messages
            .iter()
            .any(|m| m.message_id == second_receipt.user_message_id.0)
    );
    assert!(
        next.context.messages.iter().any(|m| m.tool_calls.is_some()),
        "native JSON tool history survives snapshot"
    );
    let history = crate::chat_service::get_agent_messages(db, &KEY, &sid, None)
        .await
        .unwrap();
    assert!(
        history
            .iter()
            .any(|m| m.content == "completed intermediate response")
    );
    assert!(history.iter().any(|m| m.content == "Interrupted"));
    let cancellation = store
        .cancel(&conversation, &second_receipt.run_id, 1210)
        .await
        .unwrap();
    assert!(cancellation.cancellation_requested);
    let terminal = replica
        .finish(
            &conversation,
            &second_receipt.run_id,
            next.snapshot.lease.as_ref().unwrap(),
            1220,
            RunState::Completed,
            "completion lost to persisted cancellation",
        )
        .await
        .unwrap();
    assert_eq!(terminal.state, RunState::Cancelled);
    let events_before = count(db, "conversation_events", &sid).await;
    assert_eq!(
        store
            .cancel(&conversation, &second_receipt.run_id, 1221)
            .await
            .unwrap()
            .state,
        RunState::Cancelled
    );
    assert_eq!(count(db, "conversation_events", &sid).await, events_before);
    let queued = lifecycle_submission(&owner, &sid, "queued-cancel", "never execute", 3);
    let queued_receipt = store.submit(&queued).await.unwrap();
    assert_eq!(
        replica
            .cancel(&conversation, &queued_receipt.run_id, 1230)
            .await
            .unwrap()
            .state,
        RunState::Cancelled
    );
    assert!(
        store
            .claim(&conversation, &queued_receipt.run_id, "worker-a", 1231, 100)
            .await
            .is_err()
    );
    // First provider failure needs no prior assistant response to finalize one truthful row.
    let failed = lifecycle_submission(&owner, &sid, "first-provider-error", "error prompt", 4);
    let failed_receipt = store.submit(&failed).await.unwrap();
    let failed_claim = store
        .claim(&conversation, &failed_receipt.run_id, "worker-a", 1300, 100)
        .await
        .unwrap();
    assert_eq!(
        store
            .finish(
                &conversation,
                &failed_receipt.run_id,
                failed_claim.snapshot.lease.as_ref().unwrap(),
                1310,
                RunState::Failed,
                "Provider unavailable"
            )
            .await
            .unwrap()
            .state,
        RunState::Failed
    );
    let failed_row = crate::chat_service::get_session_messages(db, &KEY, &sid, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.message_id == failed_receipt.assistant_message_id.0)
        .unwrap();
    assert_eq!(failed_row.status, "error");
    assert_eq!(failed_row.content, "Provider unavailable");
    // A missing/ineligible projection causes the complete terminal transaction to roll back.
    let missing = lifecycle_submission(&owner, &sid, "zero-row-finalize", "zero row prompt", 5);
    let missing_receipt = store.submit(&missing).await.unwrap();
    let missing_claim = store
        .claim(
            &conversation,
            &missing_receipt.run_id,
            "worker-a",
            1400,
            100,
        )
        .await
        .unwrap();
    kyomi_core::db_execute!(
        db,
        "UPDATE chat_messages SET status='complete' WHERE message_id=$1",
        missing_receipt.assistant_message_id.as_str()
    )
    .unwrap();
    let before = count(db, "conversation_events", &sid).await;
    assert!(matches!(
        store
            .finish(
                &conversation,
                &missing_receipt.run_id,
                missing_claim.snapshot.lease.as_ref().unwrap(),
                1410,
                RunState::Completed,
                "should not persist"
            )
            .await,
        Err(EventStoreError::MissingProjection)
    ));
    assert_eq!(count(db, "conversation_events", &sid).await, before);
    assert_eq!(
        kyomi_core::db_fetch_one!(
            db,
            (String,),
            "SELECT state FROM conversation_runs WHERE run_id=$1",
            missing_receipt.run_id.as_str()
        )
        .unwrap()
        .0,
        "running"
    );
    kyomi_core::db_execute!(
        db,
        "UPDATE chat_messages SET status='in_progress' WHERE message_id=$1",
        missing_receipt.assistant_message_id.as_str()
    )
    .unwrap();
    store
        .finish(
            &conversation,
            &missing_receipt.run_id,
            missing_claim.snapshot.lease.as_ref().unwrap(),
            1420,
            RunState::Completed,
            "committed answer",
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .cancel(&conversation, &missing_receipt.run_id, 1421)
            .await
            .unwrap()
            .state,
        RunState::Completed,
        "committed completion wins later cancellation"
    );
    // Lifecycle collaboration preserves existing shared-member submission permission,
    // without widening raw event writes or allowing one actor to own another actor's run.
    let member_submission =
        lifecycle_submission(&member, &sid, "member-request", "member shared prompt", 10);
    let member_receipt = viewer.submit(&member_submission).await.unwrap();
    assert!(
        viewer.append(&member_submission.submitted).await.is_err(),
        "raw foundation append remains owner-only"
    );
    let member_claim = viewer
        .claim(
            &conversation,
            &member_receipt.run_id,
            "member-worker",
            2000,
            100,
        )
        .await
        .unwrap();
    let member_lease = member_claim.snapshot.lease.as_ref().unwrap();
    assert!(
        store
            .finish(
                &conversation,
                &member_receipt.run_id,
                member_lease,
                2010,
                RunState::Completed,
                "impersonated actor"
            )
            .await
            .is_err()
    );
    assert!(
        ConversationStore::new(db, &KEY, &member, "wrong-workspace")
            .with_deterministic_time()
            .cancel(&conversation, &member_receipt.run_id, 2010)
            .await
            .is_err()
    );
    let member_message = MessageWrite {
        message_id: uuid::Uuid::new_v4().to_string(),
        role: "assistant".into(),
        content: "member completed response".into(),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    };
    viewer
        .fenced_message(
            &conversation,
            &member_receipt.run_id,
            member_lease,
            2010,
            &member_message,
        )
        .await
        .unwrap();
    let member_replica =
        ConversationStore::new(other, &KEY, &member, &wid).with_deterministic_time();
    member_replica
        .cancel(&conversation, &member_receipt.run_id, 2020)
        .await
        .unwrap();
    assert_eq!(
        viewer
            .finish(
                &conversation,
                &member_receipt.run_id,
                member_lease,
                2030,
                RunState::Completed,
                "member completion loses race"
            )
            .await
            .unwrap()
            .state,
        RunState::Cancelled
    );
    let revoked_running = lifecycle_submission(
        &member,
        &sid,
        "revoked-running",
        "revoked running prompt",
        11,
    );
    let revoked_running_receipt = viewer.submit(&revoked_running).await.unwrap();
    let revoked_claim = viewer
        .claim(
            &conversation,
            &revoked_running_receipt.run_id,
            "member-worker",
            2100,
            100,
        )
        .await
        .unwrap();
    let revoked_queued =
        lifecycle_submission(&member, &sid, "revoked-queued", "revoked queue prompt", 12);
    let revoked_queued_receipt = viewer.submit(&revoked_queued).await.unwrap();
    let allowed_queue = lifecycle_submission(
        &owner,
        &sid,
        "following-authorized",
        "following authorized prompt",
        13,
    );
    let allowed_receipt = store.submit(&allowed_queue).await.unwrap();
    kyomi_core::db_execute!(
        db,
        "UPDATE workspace_users SET active=false WHERE workspace_id=$1 AND user_id=$2",
        &wid,
        &member
    )
    .unwrap();
    assert!(
        viewer
            .heartbeat(
                &conversation,
                &revoked_running_receipt.run_id,
                revoked_claim.snapshot.lease.as_ref().unwrap(),
                2110,
                100
            )
            .await
            .is_err()
    );
    assert!(viewer.replay(&conversation, 0, 10).await.is_err());
    assert_eq!(
        viewer
            .expire(&conversation, &revoked_running_receipt.run_id, 2199)
            .await
            .unwrap()
            .state,
        RunState::Running,
        "revocation recovery cannot interrupt a live lease"
    );
    assert_eq!(
        viewer
            .expire(&conversation, &revoked_running_receipt.run_id, 2200)
            .await
            .unwrap()
            .state,
        RunState::Interrupted
    );
    let discovered = discover_queue(db, 100).await.unwrap();
    assert!(
        discovered
            .iter()
            .any(|r| r.run_id == revoked_queued_receipt.run_id.0 && r.expired)
    );
    assert_eq!(
        viewer
            .expire(&conversation, &revoked_queued_receipt.run_id, 2201)
            .await
            .unwrap()
            .state,
        RunState::Interrupted,
        "revoked queued head must release following authorized work"
    );
    let following = store
        .claim(
            &conversation,
            &allowed_receipt.run_id,
            "owner-worker",
            2210,
            100,
        )
        .await
        .unwrap();
    store
        .finish(
            &conversation,
            &allowed_receipt.run_id,
            following.snapshot.lease.as_ref().unwrap(),
            2220,
            RunState::Completed,
            "authorized following answer",
        )
        .await
        .unwrap();
    kyomi_core::db_execute!(
        db,
        "UPDATE workspace_users SET active=true WHERE workspace_id=$1 AND user_id=$2",
        &wid,
        &member
    )
    .unwrap();
    kyomi_core::db_execute!(
        db,
        "UPDATE chat_sessions SET shared=false WHERE session_id=$1",
        &sid
    )
    .unwrap();
    let private =
        lifecycle_submission(&member, &sid, "private-denial", "private denied prompt", 14);
    assert!(viewer.submit(&private).await.is_err());
    // New session participates in rollback and an accepted retry returns original IDs.
    let new_sid = uuid::Uuid::new_v4().to_string();
    let mut new = lifecycle_submission(
        &owner,
        &new_sid,
        "new-session-request",
        "new session prompt",
        6,
    );
    new.new_conversation = true;
    let mut new_retry = lifecycle_submission(
        &owner,
        &new_sid,
        "new-session-request",
        "new session prompt",
        7,
    );
    new_retry.new_conversation = true;
    let (accepted_a, accepted_b) = tokio::join!(store.submit(&new), replica.submit(&new_retry));
    let (created, duplicate) = (accepted_a.unwrap(), accepted_b.unwrap());
    assert_eq!(duplicate.run_id, created.run_id);
    assert_ne!(created.duplicate, duplicate.duplicate);
    assert_eq!(count(db, "chat_messages", &new_sid).await, 2);
    let rollback_sid = uuid::Uuid::new_v4().to_string();
    let mut rollback = lifecycle_submission(
        &owner,
        &rollback_sid,
        "new-session-rollback",
        "rollback new session",
        8,
    );
    rollback.new_conversation = true;
    rollback.assistant_message_id = created.assistant_message_id;
    assert!(store.submit(&rollback).await.is_err());
    assert!(
        kyomi_core::db_fetch_optional!(
            db,
            (String,),
            "SELECT session_id FROM chat_sessions WHERE session_id=$1",
            &rollback_sid
        )
        .unwrap()
        .is_none()
    );
    crate::chat_service::delete_session(db, &owner, &sid, Some(&wid))
        .await
        .unwrap();
    assert_eq!(count(db, "conversation_runs", &sid).await, 0);
    crate::chat_service::delete_session(db, &owner, &new_sid, Some(&wid))
        .await
        .unwrap();
}
async fn assert_lifecycle_sync(db: &DbPool, sid: &str, owner: &str, shared: bool) {
    let (_, snapshot) = crate::chat_service::fetch_session_snapshot(db, sid)
        .await
        .unwrap()
        .unwrap();
    let (data, stored_owner, visible): (serde_json::Value, Option<String>, bool) = kyomi_core::db_fetch_one!(db, (serde_json::Value,Option<String>,bool), "SELECT data,owner_user_id,is_workspace_visible FROM sync_log WHERE entity_type='chat_session' AND entity_id=$1 ORDER BY sync_id DESC LIMIT 1", sid).unwrap();
    assert_eq!(
        data, snapshot,
        "durable sync delta differs from canonical committed snapshot"
    );
    assert_eq!(stored_owner.as_deref(), Some(owner));
    assert_eq!(visible, shared);
}
async fn lifecycle_sync_count(db: &DbPool, sid: &str) -> i64 {
    kyomi_core::db_fetch_one!(
        db,
        (i64,),
        "SELECT COUNT(*) FROM sync_log WHERE entity_type='chat_session' AND entity_id=$1",
        sid
    )
    .unwrap()
    .0
}
async fn lifecycle_review_regressions(db: &DbPool) {
    use agent_runtime::RunState;
    let (owner, _, wid, sid) = seed(db).await;
    let conversation = ConversationId(sid.clone());
    let store = ConversationStore::new(db, &KEY, &owner, &wid).with_deterministic_time();
    let submitted = lifecycle_submission(&owner, &sid, "encrypted-turn", "tool turn", 1);
    let receipt = store.submit(&submitted).await.unwrap();
    assert_lifecycle_sync(db, &sid, &owner, true).await;
    let accepted_sync = lifecycle_sync_count(db, &sid).await;
    assert!(store.submit(&submitted).await.unwrap().duplicate);
    assert_eq!(lifecycle_sync_count(db, &sid).await, accepted_sync);
    let claim = store
        .claim(&conversation, &receipt.run_id, "owner", 1000, 1000)
        .await
        .unwrap();
    let lease = claim.snapshot.lease.unwrap();
    let config = serde_json::json!({"agent_state":{"global_iteration":2,"compacted_summary":"SENSITIVE-COMPACTION-MARKER","messages_since_compaction_index":1}});
    store
        .fenced_session_metadata(&conversation, &receipt.run_id, &lease, 1100, &config)
        .await
        .unwrap();
    let raw_config = kyomi_core::db_fetch_one!(
        db,
        (serde_json::Value,),
        "SELECT config FROM chat_sessions WHERE session_id=$1",
        &sid
    )
    .unwrap()
    .0;
    assert!(
        !raw_config
            .to_string()
            .contains("SENSITIVE-COMPACTION-MARKER")
    );
    assert_eq!(
        encryption::restore_chat_config(&raw_config, &KEY).unwrap(),
        config
    );
    assert!(encryption::restore_chat_config(&raw_config, &[19u8; 32]).is_err());
    assert_lifecycle_sync(db, &sid, &owner, true).await;
    let sync_before_duplicate = lifecycle_sync_count(db, &sid).await;
    store
        .fenced_session_metadata(&conversation, &receipt.run_id, &lease, 1101, &config)
        .await
        .unwrap();
    assert_eq!(lifecycle_sync_count(db, &sid).await, sync_before_duplicate);
    let calls = serde_json::json!([{"id":"secret-call","type":"function","function":{"name":"sql","arguments":"SENSITIVE-TOOL-ARGUMENTS-MARKER"}}]);
    let message = MessageWrite {
        message_id: uuid::Uuid::new_v4().to_string(),
        role: "assistant".into(),
        content: "tool request".into(),
        tool_call_id: None,
        name: None,
        tool_calls: Some(calls.clone()),
    };
    store
        .fenced_message(&conversation, &receipt.run_id, &lease, 1102, &message)
        .await
        .unwrap();
    assert_eq!(
        encryption::restore_json_field(&calls, &KEY).unwrap(),
        calls,
        "legacy plaintext tool metadata stays readable"
    );
    assert!(
        encryption::restore_json_field(&serde_json::json!({"__kyomi_encrypted_json_v1":3}), &KEY)
            .is_err()
    );
    assert_eq!(
        encryption::restore_chat_config(&config, &KEY).unwrap(),
        config,
        "legacy plaintext compaction stays readable"
    );
    let raw_calls = kyomi_core::db_fetch_one!(
        db,
        (Option<serde_json::Value>,),
        "SELECT tool_calls FROM chat_messages WHERE message_id=$1",
        &message.message_id
    )
    .unwrap()
    .0
    .unwrap();
    assert!(
        !raw_calls
            .to_string()
            .contains("SENSITIVE-TOOL-ARGUMENTS-MARKER")
    );
    assert_eq!(
        encryption::restore_json_field(&raw_calls, &KEY).unwrap(),
        calls
    );
    let history = crate::chat_service::get_agent_messages(db, &KEY, &sid, None)
        .await
        .unwrap();
    assert_eq!(
        history
            .iter()
            .find(|item| item.message_id == message.message_id)
            .unwrap()
            .tool_calls,
        Some(calls.clone())
    );
    assert!(
        !crate::chat_service::get_session_messages(db, &KEY, &sid, 100)
            .await
            .unwrap()
            .iter()
            .any(|item| item.message_id == message.message_id),
        "encrypted tool calls must retain SQL non-NULL UI filtering"
    );
    assert_lifecycle_sync(db, &sid, &owner, true).await;
    let sync_before_duplicate = lifecycle_sync_count(db, &sid).await;
    assert!(
        store
            .fenced_message(&conversation, &receipt.run_id, &lease, 1103, &message)
            .await
            .unwrap()
            .duplicate
    );
    assert_eq!(lifecycle_sync_count(db, &sid).await, sync_before_duplicate);
    let mut invalid = message.clone();
    invalid.role = "invalid".into();
    assert!(
        store
            .fenced_message(&conversation, &receipt.run_id, &lease, 1104, &invalid)
            .await
            .is_err()
    );
    assert_eq!(lifecycle_sync_count(db, &sid).await, sync_before_duplicate);
    let preserved = serde_json::json!({"model":"preview-model","component":"chat","token_usage":{"input_tokens":5}});
    kyomi_core::db_execute!(
        db,
        "UPDATE chat_messages SET extra_metadata=$1 WHERE message_id=$2",
        encryption::encrypt_json(&preserved, &KEY).unwrap(),
        receipt.assistant_message_id.as_str()
    )
    .unwrap();
    let preview = vec![
        serde_json::json!({"event_id":"preview-1","event_type":"llm_thinking","title":"Thinking","description":"SENSITIVE-THINKING-PREVIEW-MARKER","data":{"arguments":"SENSITIVE-PREVIEW-ARGS","result":"SENSITIVE-PREVIEW-RESULT"}}),
    ];
    let preview_receipt = store
        .fenced_thinking_events(&conversation, &receipt.run_id, &lease, 1110, &preview)
        .await
        .unwrap();
    assert!(!preview_receipt.duplicate);
    let preview_body = serde_json::to_string(&preview).unwrap();
    let stored_key = kyomi_core::db_fetch_one!(
        db,
        (String,),
        "SELECT idempotency_key FROM conversation_events WHERE event_id=$1",
        preview_receipt.event_id.as_str()
    )
    .unwrap()
    .0;
    assert_ne!(
        stored_key,
        hex::encode(Sha256::digest(format!(
            "{}:thinking-preview:{preview_body}",
            receipt.run_id.as_str()
        ))),
        "stored reasoning checkpoint identity must not expose a guessable content hash"
    );
    let stored = crate::chat_service::get_session_messages(db, &KEY, &sid, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.message_id == receipt.assistant_message_id.0)
        .unwrap();
    assert_eq!(stored.status, "in_progress");
    assert_eq!(stored.thinking_events, preview);
    assert_eq!(stored.metadata["model"], preserved["model"]);
    assert_eq!(stored.metadata["token_usage"], preserved["token_usage"]);
    let raw = kyomi_core::db_fetch_one!(
        db,
        (String,),
        "SELECT extra_metadata FROM chat_messages WHERE message_id=$1",
        receipt.assistant_message_id.as_str()
    )
    .unwrap()
    .0;
    for marker in [
        "SENSITIVE-THINKING-PREVIEW-MARKER",
        "SENSITIVE-PREVIEW-ARGS",
        "SENSITIVE-PREVIEW-RESULT",
    ] {
        assert!(!raw.contains(marker));
    }
    assert_lifecycle_sync(db, &sid, &owner, true).await;
    let mut extended = preview.clone();
    extended.push(serde_json::json!({"event_id":"preview-2","event_type":"tool_execution_end","description":"Second checkpoint"}));
    assert!(
        !store
            .fenced_thinking_events(&conversation, &receipt.run_id, &lease, 1111, &extended)
            .await
            .unwrap()
            .duplicate
    );
    let count_before = count(db, "conversation_events", &sid).await;
    let detail_before = count(db, "conversation_event_details", &sid).await;
    let cursor_before = kyomi_core::db_fetch_one!(
        db,
        (i64,),
        "SELECT event_sequence FROM chat_sessions WHERE session_id=$1",
        &sid
    )
    .unwrap()
    .0;
    let sync_before = lifecycle_sync_count(db, &sid).await;
    let current_raw = kyomi_core::db_fetch_one!(
        db,
        (String,),
        "SELECT extra_metadata FROM chat_messages WHERE message_id=$1",
        receipt.assistant_message_id.as_str()
    )
    .unwrap()
    .0;
    assert!(
        store
            .fenced_thinking_events(&conversation, &receipt.run_id, &lease, 1112, &preview)
            .await
            .unwrap()
            .duplicate,
        "retrying an earlier checkpoint must not regress its preview"
    );
    assert!(
        store
            .fenced_thinking_events(&conversation, &receipt.run_id, &lease, 1113, &extended)
            .await
            .unwrap()
            .duplicate
    );
    let mut newer = extended.clone();
    newer.push(serde_json::json!({"event_id":"preview-3","description":"Must not write"}));
    let mut stale_preview_lease = lease.clone();
    stale_preview_lease.fence += 1;
    assert!(
        store
            .fenced_thinking_events(
                &conversation,
                &receipt.run_id,
                &stale_preview_lease,
                1114,
                &newer
            )
            .await
            .is_err()
    );
    assert!(
        store
            .fenced_thinking_events(&conversation, &receipt.run_id, &lease, 2000, &newer)
            .await
            .is_err()
    );
    assert_eq!(count(db, "conversation_events", &sid).await, count_before);
    assert_eq!(lifecycle_sync_count(db, &sid).await, sync_before);
    assert_eq!(
        kyomi_core::db_fetch_one!(
            db,
            (String,),
            "SELECT extra_metadata FROM chat_messages WHERE message_id=$1",
            receipt.assistant_message_id.as_str()
        )
        .unwrap()
        .0,
        current_raw
    );
    // A native trigger forces the compatibility projection to affect zero rows
    // after the journal append. Both native transactions must roll back the event,
    // cursor, lazy detail and sync delta together.
    let trigger = format!("preview_ignore_{}", uuid::Uuid::new_v4().simple());
    let assistant_id = receipt.assistant_message_id.as_str();
    if db.is_postgres() {
        kyomi_core::db_execute!(db,&format!("CREATE FUNCTION {trigger}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.message_id='{assistant_id}' THEN RETURN NULL; END IF; RETURN NEW; END $$")).unwrap();
        kyomi_core::db_execute!(db,&format!("CREATE TRIGGER {trigger} BEFORE UPDATE OF extra_metadata ON chat_messages FOR EACH ROW EXECUTE FUNCTION {trigger}()")).unwrap();
    } else {
        kyomi_core::db_execute!(db,&format!("CREATE TRIGGER {trigger} BEFORE UPDATE OF extra_metadata ON chat_messages WHEN NEW.message_id='{assistant_id}' BEGIN SELECT RAISE(IGNORE); END")).unwrap();
    }
    assert!(matches!(
        store
            .fenced_thinking_events(&conversation, &receipt.run_id, &lease, 1115, &newer)
            .await,
        Err(EventStoreError::MissingProjection)
    ));
    if db.is_postgres() {
        kyomi_core::db_execute!(db, &format!("DROP TRIGGER {trigger} ON chat_messages")).unwrap();
        kyomi_core::db_execute!(db, &format!("DROP FUNCTION {trigger}()")).unwrap();
    } else {
        kyomi_core::db_execute!(db, &format!("DROP TRIGGER {trigger}")).unwrap();
    }
    assert_eq!(count(db, "conversation_events", &sid).await, count_before);
    assert_eq!(
        count(db, "conversation_event_details", &sid).await,
        detail_before
    );
    assert_eq!(
        kyomi_core::db_fetch_one!(
            db,
            (i64,),
            "SELECT event_sequence FROM chat_sessions WHERE session_id=$1",
            &sid
        )
        .unwrap()
        .0,
        cursor_before
    );
    assert_eq!(lifecycle_sync_count(db, &sid).await, sync_before);
    assert_eq!(
        kyomi_core::db_fetch_one!(
            db,
            (String,),
            "SELECT extra_metadata FROM chat_messages WHERE message_id=$1",
            receipt.assistant_message_id.as_str()
        )
        .unwrap()
        .0,
        current_raw
    );
    let current = crate::chat_service::get_session_messages(db, &KEY, &sid, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.message_id == receipt.assistant_message_id.0)
        .unwrap();
    assert_eq!(current.thinking_events, extended);
    let usage = ApiUsageWrite {
        provider: "anthropic".into(),
        model: "fixture".into(),
        input_tokens: 11,
        output_tokens: 7,
        total_tokens: 18,
        cost_estimate: 0.5,
        component: "chat".into(),
        provider_cost_usd: None,
    };
    store
        .fenced_api_usage(&conversation, &receipt.run_id, &lease, 1105, &usage)
        .await
        .unwrap();
    let mut stale = lease.clone();
    stale.fence += 1;
    assert!(
        store
            .fenced_api_usage(&conversation, &receipt.run_id, &stale, 1106, &usage)
            .await
            .is_err()
    );
    assert!(
        store
            .fenced_api_usage(&conversation, &receipt.run_id, &lease, 2000, &usage)
            .await
            .is_err()
    );
    assert_eq!(
        kyomi_core::db_fetch_one!(
            db,
            (i64,),
            "SELECT COUNT(*) FROM api_usage_log WHERE session_id=$1",
            &sid
        )
        .unwrap()
        .0,
        1,
        "stale/expired owners must write zero usage rows"
    );
    store
        .finish(
            &conversation,
            &receipt.run_id,
            &lease,
            1200,
            RunState::Completed,
            "complete tool turn",
        )
        .await
        .unwrap();
    assert_lifecycle_sync(db, &sid, &owner, true).await;
    let terminal_sync = lifecycle_sync_count(db, &sid).await;
    assert!(
        store
            .fenced_thinking_events(&conversation, &receipt.run_id, &lease, 1201, &preview)
            .await
            .is_err(),
        "late iteration preview must not overwrite terminal metadata"
    );
    assert_eq!(lifecycle_sync_count(db, &sid).await, terminal_sync);
    // A queued successor restores encrypted compaction and tool metadata at claim.
    let successor = lifecycle_submission(&owner, &sid, "restore-context", "next turn", 2);
    let successor_receipt = store.submit(&successor).await.unwrap();
    let restored = store
        .claim(&conversation, &successor_receipt.run_id, "next", 1300, 1000)
        .await
        .unwrap();
    assert_eq!(restored.context.config, Some(config));
    assert_eq!(
        restored
            .context
            .messages
            .iter()
            .find(|item| item.message_id == message.message_id)
            .unwrap()
            .tool_calls,
        Some(calls)
    );
    store
        .cancel(&conversation, &successor_receipt.run_id, 1400)
        .await
        .unwrap();
    store
        .finish(
            &conversation,
            &successor_receipt.run_id,
            restored.snapshot.lease.as_ref().unwrap(),
            1401,
            RunState::Completed,
            "cancel must win",
        )
        .await
        .unwrap();
    assert_lifecycle_sync(db, &sid, &owner, true).await;
    let terminal_sync = lifecycle_sync_count(db, &sid).await;
    store
        .cancel(&conversation, &successor_receipt.run_id, 1402)
        .await
        .unwrap();
    assert_eq!(lifecycle_sync_count(db, &sid).await, terminal_sync);
    let interrupted =
        lifecycle_submission(&owner, &sid, "sync-interruption", "interrupted turn", 3);
    let interrupted_receipt = store.submit(&interrupted).await.unwrap();
    store
        .claim(
            &conversation,
            &interrupted_receipt.run_id,
            "expired",
            1500,
            100,
        )
        .await
        .unwrap();
    store
        .expire(&conversation, &interrupted_receipt.run_id, 1600)
        .await
        .unwrap();
    assert_lifecycle_sync(db, &sid, &owner, true).await;
    let missing = lifecycle_submission(&owner, &sid, "sync-rollback", "rollback turn", 4);
    let missing_receipt = store.submit(&missing).await.unwrap();
    let missing_claim = store
        .claim(
            &conversation,
            &missing_receipt.run_id,
            "missing",
            1700,
            1000,
        )
        .await
        .unwrap();
    kyomi_core::db_execute!(
        db,
        "UPDATE chat_messages SET status='complete' WHERE message_id=$1",
        missing_receipt.assistant_message_id.as_str()
    )
    .unwrap();
    let before_rollback = lifecycle_sync_count(db, &sid).await;
    assert!(
        store
            .finish(
                &conversation,
                &missing_receipt.run_id,
                missing_claim.snapshot.lease.as_ref().unwrap(),
                1800,
                RunState::Completed,
                "rolled back success"
            )
            .await
            .is_err()
    );
    assert_eq!(lifecycle_sync_count(db, &sid).await, before_rollback);
    kyomi_core::db_execute!(
        db,
        "UPDATE chat_messages SET status='in_progress' WHERE message_id=$1",
        missing_receipt.assistant_message_id.as_str()
    )
    .unwrap();
    store
        .finish(
            &conversation,
            &missing_receipt.run_id,
            missing_claim.snapshot.lease.as_ref().unwrap(),
            1801,
            RunState::Completed,
            "real success",
        )
        .await
        .unwrap();
    kyomi_core::db_execute!(
        db,
        "UPDATE chat_sessions SET shared=false WHERE session_id=$1",
        &sid
    )
    .unwrap();
    let private = lifecycle_submission(&owner, &sid, "sync-private", "private turn", 5);
    let private_receipt = store.submit(&private).await.unwrap();
    store
        .cancel(&conversation, &private_receipt.run_id, 1900)
        .await
        .unwrap();
    assert_lifecycle_sync(db, &sid, &owner, false).await;
    crate::chat_service::delete_session(db, &owner, &sid, Some(&wid))
        .await
        .unwrap();
}

async fn lease_wait_cannot_resurrect(db: &DbPool, other: &DbPool) {
    let (owner, _, wid, sid) = seed(db).await;
    let conversation = ConversationId(sid.clone());
    let store = ConversationStore::new(db, &KEY, &owner, &wid);
    // A later cancelled queued turn remains a future prompt, even though its state
    // is already terminal. Claim context is ordered by acceptance, not status.
    let earlier = lifecycle_submission(&owner, &sid, "context-earlier", "earlier prompt", 1);
    let future = lifecycle_submission(&owner, &sid, "context-future", "cancelled future prompt", 2);
    let earlier_receipt = store.submit(&earlier).await.unwrap();
    let future_receipt = store.submit(&future).await.unwrap();
    store
        .cancel(
            &conversation,
            &future_receipt.run_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .unwrap();
    let claimed = store
        .claim(
            &conversation,
            &earlier_receipt.run_id,
            "context-worker",
            chrono::Utc::now().timestamp_millis(),
            5000,
        )
        .await
        .unwrap();
    assert!(
        claimed.context.messages.is_empty(),
        "cancelled future prompt leaked into earlier claim context"
    );
    store
        .finish(
            &conversation,
            &earlier_receipt.run_id,
            claimed.snapshot.lease.as_ref().unwrap(),
            chrono::Utc::now().timestamp_millis(),
            agent_runtime::RunState::Completed,
            "earlier answer",
        )
        .await
        .unwrap();
    for operation in ["heartbeat", "finish", "usage"] {
        let now = chrono::Utc::now().timestamp_millis();
        let submitted = lifecycle_submission(
            &owner,
            &sid,
            &uuid::Uuid::new_v4().to_string(),
            "lease wait",
            now,
        );
        let accepted = store.submit(&submitted).await.unwrap();
        let claimed = store
            .claim(&conversation, &accepted.run_id, "blocked-worker", now, 250)
            .await
            .unwrap();
        let lease = claimed.snapshot.lease.unwrap();
        let before = count(db, "conversation_events", &sid).await;
        macro_rules! held {
            ($tx:ident) => {{
                sqlx::query(
                    "UPDATE chat_sessions SET event_sequence=event_sequence WHERE session_id=$1",
                )
                .bind(&sid)
                .execute(&mut *$tx)
                .await
                .unwrap();
                let writer_db = other.clone();
                let writer_owner = owner.clone();
                let writer_wid = wid.clone();
                let writer_conversation = conversation.clone();
                let writer_run = accepted.run_id.clone();
                let mut pending = tokio::spawn(async move {
                    let writer =
                        ConversationStore::new(&writer_db, &KEY, &writer_owner, &writer_wid);
                    let reference = chrono::Utc::now().timestamp_millis();
                    match operation {
                        "finish" => writer
                            .finish(
                                &writer_conversation,
                                &writer_run,
                                &lease,
                                reference,
                                agent_runtime::RunState::Completed,
                                "stale success",
                            )
                            .await
                            .map(|_| ()),
                        "heartbeat" => writer
                            .heartbeat(&writer_conversation, &writer_run, &lease, reference, 1000)
                            .await
                            .map(|_| ()),
                        _ => {
                            writer
                                .fenced_api_usage(
                                    &writer_conversation,
                                    &writer_run,
                                    &lease,
                                    reference,
                                    &ApiUsageWrite {
                                        provider: "anthropic".into(),
                                        model: "fixture".into(),
                                        input_tokens: 1,
                                        output_tokens: 1,
                                        total_tokens: 2,
                                        cost_estimate: 0.1,
                                        component: "chat".into(),
                                        provider_cost_usd: None,
                                    },
                                )
                                .await
                        }
                    }
                });
                assert!(
                    tokio::time::timeout(std::time::Duration::from_millis(400), &mut pending)
                        .await
                        .is_err(),
                    "operation escaped the held session lock"
                );
                $tx.commit().await.unwrap();
                assert!(
                    matches!(
                        pending.await.unwrap(),
                        Err(EventStoreError::Lifecycle(
                            agent_runtime::LifecycleError::StaleLease
                        ))
                    ),
                    "expired owner wrote after waiting for lock"
                );
            }};
        }
        match db {
            DbPool::Postgres(pool) => {
                let mut tx = pool.begin().await.unwrap();
                held!(tx);
            }
            DbPool::Sqlite(pool) => {
                let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
                held!(tx);
            }
        }
        assert_eq!(count(db, "conversation_events", &sid).await, before);
        assert_eq!(
            kyomi_core::db_fetch_one!(
                db,
                (i64,),
                "SELECT COUNT(*) FROM api_usage_log WHERE session_id=$1",
                &sid
            )
            .unwrap()
            .0,
            0,
            "expired usage insert escaped the held lease lock"
        );
        assert_eq!(
            store
                .expire(
                    &conversation,
                    &accepted.run_id,
                    chrono::Utc::now().timestamp_millis()
                )
                .await
                .unwrap()
                .state,
            agent_runtime::RunState::Interrupted
        );
    }
    crate::chat_service::delete_session(db, &owner, &sid, Some(&wid))
        .await
        .unwrap();
}
#[tokio::test]
async fn sqlite_lifecycle_conformance() {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    let path = std::env::temp_dir().join(format!("kyo882-{}.sqlite", uuid::Uuid::new_v4()));
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(30));
    let first = SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(options.clone())
        .await
        .unwrap();
    sqlx::migrate!("../../apps/server/migrations-sqlite")
        .run(&first)
        .await
        .unwrap();
    let second = SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await
        .unwrap();
    lifecycle_conformance(
        &DbPool::Sqlite(first.clone()),
        &DbPool::Sqlite(second.clone()),
    )
    .await;
    lifecycle_review_regressions(&DbPool::Sqlite(first.clone())).await;
    lease_wait_cannot_resurrect(
        &DbPool::Sqlite(first.clone()),
        &DbPool::Sqlite(second.clone()),
    )
    .await;
    first.close().await;
    second.close().await;
    std::fs::remove_file(path).unwrap();
}
#[tokio::test]
async fn postgres_lifecycle_conformance() {
    let Some(db) =
        crate::test_pg::postgres_test_pool_or_skip("postgres_lifecycle_conformance").await
    else {
        return;
    };
    lifecycle_conformance(&db, &db).await;
    lifecycle_review_regressions(&db).await;
    lease_wait_cannot_resurrect(&db, &db).await;
}

#[tokio::test]
async fn durable_dispatch_acceptance_is_atomic_retryable_and_preserves_long_input() {
    let db = crate::test_support::test_pool().await;
    let (owner, _, wid, _) = seed(&db).await;
    let sid = uuid::Uuid::new_v4().to_string();
    let body = "wide complete request ".repeat(4000);
    let context = serde_json::json!({"model_name":"fixture-model","user_display_name":"Fixture"});
    let dispatch = || crate::chat_service::ChatDispatchParams {
        db: &db,
        encryption_key: &KEY,
        ws_manager: None,
        user_id: &owner,
        workspace_id: &wid,
        user_display_name: "Fixture",
        session_id: &sid,
        is_new_session: true,
        message: &body,
        current_time_user_tz: Some("2026-10-04T12:00:00Z"),
        message_source: Some("web"),
        skip_ai: false,
        client_msg_id: Some("stable-dispatch-request"),
        execution_context: Some(&context),
        owner_instance: "accepting-api-replica",
    };
    let first = crate::chat_service::prepare_chat_dispatch(dispatch())
        .await
        .unwrap();
    let retry = crate::chat_service::prepare_chat_dispatch(dispatch())
        .await
        .unwrap();
    let crate::chat_service::ChatDispatchOutcome::Ready {
        run_id: first_run,
        user_message_id: first_user,
        assistant_message_id: first_assistant,
        duplicate: false,
        ..
    } = first
    else {
        panic!("first submit must accept queued work");
    };
    let crate::chat_service::ChatDispatchOutcome::Ready {
        run_id: retry_run,
        user_message_id: retry_user,
        assistant_message_id: retry_assistant,
        duplicate: true,
        ..
    } = retry
    else {
        panic!("same client request must return original acceptance");
    };
    assert_eq!(
        (retry_run, retry_user, retry_assistant),
        (first_run.clone(), first_user, first_assistant)
    );
    assert_eq!(count(&db, "chat_messages", &sid).await, 2);
    assert_eq!(count(&db, "conversation_runs", &sid).await, 1);
    let stored = crate::chat_service::get_session_messages(&db, &KEY, &sid, 100)
        .await
        .unwrap();
    assert!(
        stored
            .iter()
            .any(|m| m.message_type == "user" && m.content == body)
    );
    // No handler spawn happened: a replacement worker still discovers and claims it.
    assert!(
        discover_queue(&db, 100)
            .await
            .unwrap()
            .iter()
            .any(|r| r.run_id == first_run)
    );
    let claim = ConversationStore::new(&db, &KEY, &owner, &wid)
        .claim(
            &ConversationId(sid.clone()),
            &RunId(first_run),
            "replacement-worker",
            1000,
            100,
        )
        .await
        .unwrap();
    assert_eq!(
        claim.submission.submitted.detail.as_deref(),
        Some(body.as_str())
    );
    assert!(claim.context.messages.is_empty());
}

async fn execution_sink_conformance(db: &DbPool) {
    let (owner, _, wid, sid) = seed(db).await;
    let now = chrono::Utc::now().timestamp_millis();
    let store = ConversationStore::new(db, &KEY, &owner, &wid).with_deterministic_time();
    let submission = lifecycle_submission(&owner, &sid, "awaited-sink", "inspect", now);
    store.submit(&submission).await.unwrap();
    let run = &submission.submitted.run_id;
    let conversation = &submission.submitted.conversation_id;
    let claimed = store.claim(conversation, run, "sink-owner", now, 30_000).await.unwrap();
    let lease = claimed.snapshot.lease.unwrap();
    let context = agent_runtime::ExecutionContext { conversation_id: conversation.clone(), run_id: run.clone() };
    let body = "Inspecting the complete dataset before invoking the delayed tool. ".repeat(12);
    let (planning, detail) = context.text("long-planning", &body, 200);
    let long = context.command("long-planning", Payload::Public(P::Planning { text: planning }), detail);
    let progress = serde_json::json!({"event_id":long.event_id,"event_type":"agent_thought","title":"Planning",
        "description":body.chars().take(200).collect::<String>(),"has_full_text":true});
    let before = count(db, "conversation_events", &sid).await;
    // A failure in the compatibility detail insert rolls back the event and metadata together.
    let trigger_name = format!("reject_detail_{}", uuid::Uuid::new_v4().simple());
    if db.is_postgres() {
        let function = format!("CREATE FUNCTION {trigger_name}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected detail failure'; END $$");
        kyomi_core::db_execute!(db, &function).unwrap();
        let trigger = format!("CREATE TRIGGER {trigger_name} BEFORE INSERT ON thinking_event_details FOR EACH ROW WHEN (NEW.message_id='{}') EXECUTE FUNCTION {trigger_name}()", submission.assistant_message_id.as_str());
        kyomi_core::db_execute!(db, &trigger).unwrap();
    } else {
        let trigger = format!("CREATE TRIGGER {trigger_name} BEFORE INSERT ON thinking_event_details WHEN NEW.message_id='{}' BEGIN SELECT RAISE(ABORT,'injected detail failure'); END", submission.assistant_message_id.as_str());
        kyomi_core::db_execute!(db, &trigger).unwrap();
    }
    assert!(store.fenced_execution_event(&long, &lease, Some(&progress)).await.is_err());
    assert_eq!(count(db, "conversation_events", &sid).await, before);
    let dropped = if db.is_postgres() { format!("DROP TRIGGER {trigger_name} ON thinking_event_details") } else { format!("DROP TRIGGER {trigger_name}") };
    kyomi_core::db_execute!(db, &dropped).unwrap();
    if db.is_postgres() { kyomi_core::db_execute!(db, &format!("DROP FUNCTION {trigger_name}()")).unwrap(); }
    let saved = store.fenced_execution_event(&long, &lease, Some(&progress)).await.unwrap();
    assert!(!saved.duplicate);
    assert!(store.fenced_execution_event(&long, &lease, Some(&progress)).await.unwrap().duplicate);
    let full = crate::chat_service::get_thinking_event_detail(db, &KEY, submission.assistant_message_id.as_str(), long.event_id.as_str(), &owner, &wid).await.unwrap();
    assert_eq!(full, Some(body.clone()));
    let encrypted: (String,) = kyomi_core::db_fetch_one!(db, (String,), "SELECT full_text FROM thinking_event_details WHERE message_id=$1 AND event_id=$2", submission.assistant_message_id.as_str(), long.event_id.as_str()).unwrap();
    assert!(!encrypted.0.contains("complete dataset"));
    let (short_text, detail) = context.text("short-planning", "Checking totals next", 200);
    assert!(detail.is_none());
    let short = context.command("short-planning", Payload::Public(P::Planning { text: short_text }), detail);
    store.fenced_execution_event(&short, &lease, Some(&serde_json::json!({"event_id":short.event_id,"event_type":"agent_thought","description":"Checking totals next"}))).await.unwrap();
    let raw = context.command("model:response", Payload::Restricted(RestrictedPayload::ProviderResponse {
        model_call_id: ModelCallId("model-1".into()), response: serde_json::json!({"content":"invalid chartml candidate", "signature":"private-sig"}), continuation: serde_json::json!({"opaque":"private-block"}),
    }), None);
    store.fenced_execution_event(&raw, &lease, None).await.unwrap();
    let candidate = context.command("model:candidate", Payload::Restricted(RestrictedPayload::Candidate {
        model_call_id: ModelCallId("model-1".into()), text: "```chartml\ninvalid chartml candidate\n```".into(),
    }), None);
    store.fenced_execution_event(&candidate, &lease, None).await.unwrap();
    let usage = context.command("model:usage", Payload::Public(P::Usage { cost: None, model_call_id: ModelCallId("model-1".into()), usage: Usage { input_tokens:42, output_tokens:17 } }), None);
    store.fenced_execution_event(&usage, &lease, None).await.unwrap();
    assert!(store.fenced_execution_event(&usage, &lease, None).await.unwrap().duplicate);
    assert_eq!(count(db, "conversation_usage_receipts", &sid).await, 1);
    for id in ["same-tool-first", "same-tool-second"] {
        let intent = context.command(&format!("{id}:intent"), Payload::Public(P::ToolIntent { tool_call_id:ToolCallId(id.into()), name:"validate_chartml".into(), arguments:serde_json::json!({}) }), None);
        store.fenced_execution_event(&intent, &lease, None).await.unwrap();
        let start = context.command(&format!("{id}:start"), Payload::Public(P::ToolStarted { tool_call_id:ToolCallId(id.into()) }), None);
        store.fenced_execution_event(&start, &lease, None).await.unwrap();
        if id == "same-tool-first" {
            let result = context.command(&format!("{id}:outcome"), Payload::Public(P::ToolOutcome { tool_call_id:ToolCallId(id.into()), text:text("{\"valid\":false}"), transport:agent_runtime::TransportOutcome::Completed, domain:agent_runtime::DomainOutcome::Rejected }), None);
            store.fenced_execution_event(&result, &lease, None).await.unwrap();
        }
    }
    // Simulate a committed side effect and lost result. Expiry must record unknown, never claim it again.
    kyomi_core::db_execute!(db, "UPDATE workspaces SET name='side effect happened' WHERE workspace_id=$1", &wid).unwrap();
    store.expire(conversation, run, now + 30_001).await.unwrap();
    assert!(store.claim(conversation, run, "replacement", now + 30_002, 30_000).await.is_err());
    assert!(store.fenced_execution_event(&short, &lease, None).await.is_err());
    let replay = store.replay(conversation, 0, 100).await.unwrap();
    let serialized = serde_json::to_string(&replay).unwrap();
    assert!(!serialized.contains("invalid chartml candidate"));
    assert!(!serialized.contains("private-sig"));
    assert!(replay.events.iter().any(|event| matches!(&event.payload, P::ToolOutcome { tool_call_id, transport:agent_runtime::TransportOutcome::Unknown, domain:agent_runtime::DomainOutcome::Unknown, .. } if tool_call_id.as_str()=="same-tool-second")));
    assert_eq!(replay.events.iter().filter(|event| matches!(event.payload, P::Interrupted { .. })).count(), 1);
    assert_eq!(count(db, "conversation_tool_receipts", &sid).await, 2);
    let persisted: (String,) = kyomi_core::db_fetch_one!(db, (String,), "SELECT name FROM workspaces WHERE workspace_id=$1", &wid).unwrap();
    assert_eq!(persisted.0, "side effect happened");
}
#[tokio::test]
async fn sqlite_awaited_execution_sink_conformance() {
    execution_sink_conformance(&crate::test_support::test_pool().await).await;
}
#[tokio::test]
async fn postgres_awaited_execution_sink_conformance() {
    let Some(db) = crate::test_pg::postgres_test_pool_or_skip("postgres_awaited_execution_sink_conformance").await else { return; };
    execution_sink_conformance(&db).await;
}
