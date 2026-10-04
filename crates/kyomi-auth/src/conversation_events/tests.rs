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
            .filter(|m| !m.description.contains("durable conversation journal"))
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
