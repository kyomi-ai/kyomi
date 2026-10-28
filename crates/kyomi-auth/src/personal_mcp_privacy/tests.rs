// SPDX-License-Identifier: AGPL-3.0-or-later
use super::*;
use crate::personal_mcp_service as connections;
use agent_runtime::{ConversationId, DetailId};
use kyomi_core::{db_execute, db_fetch_scalar};
use kyomi_types::personal_mcp::{McpAuthKind, SavePersonalMcpConnection};

const KEY: [u8; 32] = [37; 32];
struct Fixture {
    db: DbPool,
    a: String,
    b: String,
    ws: String,
    connection: String,
    config: Config,
}
impl Fixture {
    async fn new(db: DbPool) -> Self {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let a = format!("privacy-a-{id}");
        let b = format!("privacy-b-{id}");
        let ws = format!("privacy-w-{id}");
        for user in [&a, &b] {
            db_execute!(
                &db,
                "INSERT INTO users (user_id,email) VALUES ($1,$2)",
                user,
                format!("{user}@example.com")
            )
            .unwrap();
        }
        db_execute!(
            &db,
            "INSERT INTO workspaces (workspace_id,name,owner_user_id) VALUES ($1,$1,$2)",
            &ws,
            &a
        )
        .unwrap();
        for user in [&a, &b] {
            db_execute!(&db,"INSERT INTO workspace_users (workspace_id,user_id,role,active) VALUES ($1,$2,'workspace_user',TRUE)",&ws,user).unwrap();
        }
        let connection = connections::create_connection(
            &db,
            &a,
            SavePersonalMcpConnection {
                name: "Private fixture".into(),
                endpoint: "https://example.com/mcp".into(),
                auth_kind: McpAuthKind::None,
                bearer_secret: None,
            },
            &KEY,
        )
        .await
        .unwrap()
        .connection_id;
        connections::set_workspace_enabled(&db, &a, &connection, 1, &ws, true)
            .await
            .unwrap();
        let mut config = Config::test_config();
        config.personal_mcp_enabled = true;
        Self {
            db,
            a,
            b,
            ws,
            connection,
            config,
        }
    }
    async fn session(&self, kind: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        crate::chat_service::create_session_with_id(
            &self.db,
            &self.a,
            &self.ws,
            &id,
            Some("Private fixture"),
            kind,
            None,
        )
        .await
        .unwrap();
        id
    }
    fn scope<'a>(
        &'a self,
        session: &'a str,
        surface: PersonalMcpSurface,
        document: Option<&'a str>,
    ) -> ExternalDispatch<'a> {
        ExternalDispatch {
            actor: &self.a,
            workspace: &self.ws,
            session,
            connection: &self.connection,
            generation: 1,
            surface,
            document,
        }
    }
    async fn admit(&self, session: &str) -> Result<()> {
        admit_external_dispatch(
            &self.db,
            &self.config,
            self.scope(session, PersonalMcpSurface::InteractiveChat, None),
        )
        .await
    }
    async fn doc(&self) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        db_execute!(&self.db,"INSERT INTO dashboards (dashboard_id,user_id,workspace_id,title,content) VALUES ($1,$2,$3,'Private fixture','private output')",&id,&self.a,&self.ws).unwrap();
        id
    }
    async fn collection(&self, public: bool) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        db_execute!(&self.db,"INSERT INTO collections (id,workspace_id,created_by,name,is_public) VALUES ($1,$2,$3,$1,$4)",&id,&self.ws,&self.a,public).unwrap();
        id
    }
    async fn prepare_copilot(
        &self,
        actor: &str,
        session: &str,
        document: &str,
    ) -> Result<crate::copilot_service::ScopedCopilotMessagePrep> {
        let mut config = self.config.clone();
        config.self_hosted = true;
        config.llm_api_key = Some("privacy-fixture-not-a-real-key".into());
        crate::copilot_service::prepare_scoped_copilot_message(
            crate::copilot_service::ScopedCopilotMessageInputs {
                message: crate::copilot_service::CopilotMessageInputs {
                    db: &self.db,
                    encryption_key: &KEY,
                    config: &config,
                    workspace_id: &self.ws,
                    user_id: actor,
                    session_id: session,
                    message: "Explain this private document",
                    content: Some("untrusted client replacement"),
                    current_time_user_tz: None,
                    message_source: Some("web"),
                },
                context_type: "dashboard_copilot",
                document_id: Some(document),
                historical_preview: false,
                view_context: None,
            },
        )
        .await
    }
    async fn cleanup(self) {
        // UUID-scoped children first: baseline membership/account FKs do not cascade.
        db_execute!(&self.db,"DELETE FROM chat_messages WHERE session_id IN (SELECT session_id FROM chat_sessions WHERE workspace_id=$1)",&self.ws).unwrap();
        db_execute!(
            &self.db,
            "DELETE FROM sync_log WHERE workspace_id=$1",
            &self.ws
        )
        .unwrap();
        db_execute!(
            &self.db,
            "DELETE FROM chat_sessions WHERE workspace_id=$1",
            &self.ws
        )
        .unwrap();
        db_execute!(&self.db,"DELETE FROM collection_dashboards WHERE collection_id IN (SELECT id FROM collections WHERE workspace_id=$1)",&self.ws).unwrap();
        db_execute!(
            &self.db,
            "DELETE FROM collections WHERE workspace_id=$1",
            &self.ws
        )
        .unwrap();
        db_execute!(
            &self.db,
            "DELETE FROM dashboards WHERE workspace_id=$1",
            &self.ws
        )
        .unwrap();
        db_execute!(
            &self.db,
            "DELETE FROM workspace_users WHERE workspace_id=$1",
            &self.ws
        )
        .unwrap();
        db_execute!(
            &self.db,
            "DELETE FROM workspaces WHERE workspace_id=$1",
            &self.ws
        )
        .unwrap();
        for user in [&self.a, &self.b] {
            db_execute!(&self.db, "DELETE FROM users WHERE user_id=$1", user).unwrap();
        }
    }
}
async fn matrix(db: DbPool) {
    let f = Fixture::new(db).await;
    let private = f.session("chat").await;
    // Private native behavior is preserved before any external data is admitted.
    let token = begin_native_publication(&f.db, &f.a, &private)
        .await
        .unwrap();
    assert!(f.admit(&private).await.is_err());
    assert_eq!(
        restricted_owner(&f.db, &private).await.unwrap(),
        None,
        "denied dispatch must roll back taint"
    );
    finish_native_publication(&f.db, &f.b, &token)
        .await
        .unwrap();
    assert!(
        f.admit(&private).await.is_err(),
        "another user cannot release an outstanding write"
    );
    finish_native_publication(&f.db, &f.a, &token)
        .await
        .unwrap();
    finish_native_publication(&f.db, &f.a, &token)
        .await
        .unwrap();
    let count = db_fetch_scalar!(
        &f.db,
        i64,
        "SELECT native_publications FROM chat_sessions WHERE session_id=$1",
        &private
    )
    .unwrap();
    assert_eq!(count, 0);
    f.admit(&private).await.unwrap();
    assert_eq!(
        restricted_owner(&f.db, &private).await.unwrap(),
        Some(f.a.clone())
    );
    assert!(
        !crate::chat_service::set_session_shared(&f.db, &private, true)
            .await
            .unwrap()
    );
    assert!(
        begin_native_publication(&f.db, &f.a, &private)
            .await
            .is_err(),
        "prepared native writes cannot start after restriction"
    );
    assert!(
        db_execute!(
            &f.db,
            "UPDATE chat_sessions SET shared=TRUE WHERE session_id=$1",
            &private
        )
        .is_err(),
        "direct share writer also fails"
    );
    assert!(
        db_execute!(
            &f.db,
            "UPDATE chat_sessions SET personal_data_owner_id=NULL WHERE session_id=$1",
            &private
        )
        .is_err()
    );
    assert!(
        crate::chat_service::get_session_info(&f.db, &f.a, &private, Some(&f.ws))
            .await
            .unwrap()
            .is_some(),
        "owner private history/export stays usable"
    );
    assert!(
        crate::chat_service::get_session_info(&f.db, &f.b, &private, Some(&f.ws))
            .await
            .unwrap()
            .is_none()
    );
    let owner = crate::conversation_events::ConversationStore::new(&f.db, &KEY, &f.a, &f.ws);
    let other = crate::conversation_events::ConversationStore::new(&f.db, &KEY, &f.b, &f.ws);
    let conversation = ConversationId(private.clone());
    owner.authorize_read(&conversation).await.unwrap();
    assert!(other.authorize_read(&conversation).await.is_err());
    assert!(other.replay(&conversation, 0, 100).await.is_err());
    assert!(
        other
            .detail(&conversation, &DetailId("hidden-detail".into()))
            .await
            .is_err()
    );
    let encrypted = crate::encryption::encrypt("owner-only history output", &KEY).unwrap();
    db_execute!(&f.db,"INSERT INTO chat_messages (message_id,session_id,role,content) VALUES ($1,$2,'assistant',$3)",uuid::Uuid::new_v4().to_string(),&private,encrypted).unwrap();
    assert_eq!(
        crate::chat_service::get_authorized_session_messages(
            &f.db, &KEY, &private, &f.a, &f.ws, 200
        )
        .await
        .unwrap()
        .len(),
        1
    );
    assert!(
        crate::chat_service::get_authorized_session_messages(
            &f.db, &KEY, &private, &f.b, &f.ws, 200
        )
        .await
        .unwrap()
        .is_empty()
    );
    // Both local submission and delayed Redis receipt enforce the restriction,
    // regardless of a stale publisher's cached workspace recipient list.
    let manager = crate::websocket::WebSocketManager::new(None, f.db.clone());
    let (_, mut owner_frames) = manager.connect(&f.a).unwrap();
    let (_, mut other_frames) = manager.connect(&f.b).unwrap();
    while owner_frames.try_recv().is_ok() {}
    while other_frames.try_recv().is_ok() {}
    let message = kyomi_core::WebSocketMessage::new(kyomi_core::MessageType::AgentThinking)
        .with_workspace(&f.ws)
        .with_session(&private)
        .with_data(serde_json::json!({"result":"private external sentinel"}));
    manager
        .try_send_to_user(&f.b, message.clone())
        .await
        .unwrap();
    manager
        .deliver_redis_payload(&f.b, &serde_json::to_string(&message).unwrap())
        .await
        .unwrap();
    assert!(other_frames.try_recv().is_err());
    manager
        .try_send_to_user(&f.a, message.clone())
        .await
        .unwrap();
    assert!(
        owner_frames
            .try_recv()
            .unwrap()
            .contains("private external sentinel")
    );
    manager
        .deliver_redis_payload(&f.a, &serde_json::to_string(&message).unwrap())
        .await
        .unwrap();
    assert!(
        owner_frames
            .try_recv()
            .unwrap()
            .contains("private external sentinel")
    );
    // Existing shared native turns by another member retain their ordinary
    // publishing behavior, but must hold the same durable admission fence.
    let native_shared = f.session("chat").await;
    crate::chat_service::set_session_shared(&f.db, &native_shared, true)
        .await
        .unwrap();
    let other_token = begin_native_publication(&f.db, &f.b, &native_shared)
        .await
        .unwrap();
    crate::chat_service::set_session_shared(&f.db, &native_shared, false)
        .await
        .unwrap();
    assert!(f.admit(&native_shared).await.is_err());
    finish_native_publication(&f.db, &f.b, &other_token)
        .await
        .unwrap();
    f.admit(&native_shared).await.unwrap();
    // Concurrent share versus admission: exactly one accepts, never both.
    let race = f.session("chat").await;
    let (shared, admitted) = tokio::join!(
        crate::chat_service::set_session_shared(&f.db, &race, true),
        f.admit(&race)
    );
    assert_ne!(shared.unwrap_or(false), admitted.is_ok());
    let was_shared = db_fetch_scalar!(
        &f.db,
        bool,
        "SELECT COALESCE(shared,FALSE) FROM chat_sessions WHERE session_id=$1",
        &race
    )
    .unwrap();
    assert!(!(was_shared && restricted_owner(&f.db, &race).await.unwrap().is_some()));
    let abandoned = f.session("chat").await;
    let token = begin_native_publication(&f.db, &f.a, &abandoned)
        .await
        .unwrap();
    assert!(
        f.admit(&abandoned).await.is_err(),
        "process-loss claim fails closed"
    );
    assert!(
        !reconcile_stopped_native_publication(&f.db, &f.a, &private, &token, async { Ok(()) })
            .await
            .unwrap()
    );
    assert!(
        !reconcile_stopped_native_publication(&f.db, &f.b, &abandoned, &token, async { Ok(()) })
            .await
            .unwrap()
    );
    assert!(
        reconcile_stopped_native_publication(&f.db, &f.a, &abandoned, &token, async {
            Err(denied())
        })
        .await
        .is_err()
    );
    assert_eq!(
        outstanding_native_publications(&f.db, &f.a, &abandoned)
            .await
            .unwrap(),
        vec![token.clone()]
    );
    assert!(
        reconcile_stopped_native_publication(&f.db, &f.a, &abandoned, &token, async { Ok(()) })
            .await
            .unwrap()
    );
    f.admit(&abandoned).await.unwrap();
    // Share wins deterministically; admission does not alter the existing shared row.
    let shared = f.session("chat").await;
    assert!(
        crate::chat_service::set_session_shared(&f.db, &shared, true)
            .await
            .unwrap()
    );
    assert!(f.admit(&shared).await.is_err());
    assert_eq!(restricted_owner(&f.db, &shared).await.unwrap(), None);
    for kind in [
        "watch_execution",
        "watch",
        "slack",
        "chart_builder_copilot",
        "system",
    ] {
        let session = f.session(kind).await;
        assert!(
            f.admit(&session).await.is_err(),
            "scheduled/ambiguous {kind}"
        );
    }
    let slack = f.session("chat").await;
    db_execute!(
        &f.db,
        "UPDATE chat_sessions SET platform_type='slack' WHERE session_id=$1",
        &slack
    )
    .unwrap();
    assert!(f.admit(&slack).await.is_err());
    let mut off = f.config.clone();
    off.personal_mcp_enabled = false;
    let disabled = f.session("chat").await;
    assert!(
        admit_external_dispatch(
            &f.db,
            &off,
            f.scope(&disabled, PersonalMcpSurface::InteractiveChat, None)
        )
        .await
        .is_err()
    );
    assert_eq!(restricted_owner(&f.db, &disabled).await.unwrap(), None);
    // Copilot restriction persists on document and prevents both public-add and flip.
    let doc = f.doc().await;
    let collection = f.collection(false).await;
    db_execute!(
        &f.db,
        "INSERT INTO collection_dashboards (collection_id,dashboard_id,position) VALUES ($1,$2,0)",
        &collection,
        &doc
    )
    .unwrap();
    let copilot = f.session("dashboard_copilot").await;
    admit_external_dispatch(
        &f.db,
        &f.config,
        f.scope(
            &copilot,
            PersonalMcpSurface::PrivateDocumentCopilot,
            Some(&doc),
        ),
    )
    .await
    .unwrap();
    assert!(
        is_private_copilot_destination(&f.db, &f.a, &copilot, &doc)
            .await
            .unwrap()
    );
    assert!(
        !is_private_copilot_destination(&f.db, &f.b, &copilot, &doc)
            .await
            .unwrap()
    );
    assert!(
        crate::dashboard_service::update_dashboard(
            crate::dashboard_service::UpdateDashboardParams {
                db: &f.db,
                embed: None,
                dashboard_id: &doc,
                workspace_id: &f.ws,
                user_id: &f.a,
                title: None,
                content: Some("private copilot autosave"),
                change_summary: Some("private fixture"),
                expected_content_hash: None,
                copilot_receipt: None,
            }
        )
        .await
        .unwrap(),
        "private copilot autosave stays usable"
    );
    // Actual private native edits also emit a legacy sessionless dashboard
    // update. The document identity itself is protected, including delayed
    // Redis messages and stale workspace-visible sync broadcasts.
    let document_manager = crate::websocket::WebSocketManager::new(None, f.db.clone());
    let (_, mut document_owner_frames) = document_manager.connect(&f.a).unwrap();
    let (_, mut document_other_frames) = document_manager.connect(&f.b).unwrap();
    // connect() sends exactly one immediate transport heartbeat, independently
    // of document delivery. Verify that frame before testing publication output.
    for frames in [&mut document_owner_frames, &mut document_other_frames] {
        let initial = frames.try_recv().expect("new connection heartbeat");
        let heartbeat: kyomi_core::WebSocketMessage = serde_json::from_str(&initial).unwrap();
        assert_eq!(
            heartbeat.message_type,
            kyomi_core::MessageType::Heartbeat,
            "{initial}"
        );
    }
    crate::websocket::helpers::send_dashboard_update(
        &document_manager,
        &f.ws,
        &doc,
        "updated",
        &f.a,
        "Private owner",
        None,
    )
    .await;
    let unexpected = document_other_frames.try_recv();
    assert!(
        unexpected.is_err(),
        "nonowner received private document notification: {unexpected:?}"
    );
    let owner_frame = document_owner_frames.try_recv().unwrap();
    assert!(
        owner_frame.contains(&doc),
        "owner document notification missing: {owner_frame}"
    );
    for message in [
        kyomi_core::WebSocketMessage::new(kyomi_core::MessageType::DashboardUpdate)
            .with_workspace(&f.ws).with_data(serde_json::json!({"dashboard_id":doc,"action":"updated"})),
        kyomi_core::WebSocketMessage::new(kyomi_core::MessageType::SyncAction)
            .with_workspace(&f.ws).with_data(serde_json::json!({"entity_type":"dashboard","entity_id":doc,"data":{"title":"private external title"}})),
    ] {
        let json = serde_json::to_string(&message).unwrap();
        document_manager.deliver_redis_payload(&f.b, &json).await.unwrap();
        let unexpected = document_other_frames.try_recv();
        assert!(unexpected.is_err(), "nonowner received private Redis document notification: {unexpected:?}");
        document_manager.deliver_redis_payload(&f.a, &json).await.unwrap();
        let owner_frame = document_owner_frames.try_recv().unwrap();
        assert!(owner_frame.contains(&doc), "owner Redis document notification missing: {owner_frame}");
    }
    let second_doc = f.doc().await;
    assert!(
        !is_private_copilot_destination(&f.db, &f.a, &copilot, &second_doc)
            .await
            .unwrap()
    );
    assert!(
        admit_external_dispatch(
            &f.db,
            &f.config,
            f.scope(
                &copilot,
                PersonalMcpSurface::PrivateDocumentCopilot,
                Some(&second_doc)
            )
        )
        .await
        .is_err()
    );
    assert_eq!(
        db_fetch_scalar!(
            &f.db,
            i64,
            "SELECT COUNT(*) FROM personal_mcp_private_documents WHERE dashboard_id=$1",
            &second_doc
        )
        .unwrap(),
        0,
        "failed destination switch rolls back document taint"
    );
    // Drive the actual pre-spawn copilot authorization path. Subsequent turns
    // may use the same private destination, while a fresh copilot must inherit
    // the restriction before persisted document content enters its model input.
    let prepared = f.prepare_copilot(&f.a, &copilot, &doc).await.unwrap();
    assert!(
        prepared
            .message
            .user_message
            .contains("private copilot autosave")
    );
    assert!(
        !prepared
            .message
            .user_message
            .contains("untrusted client replacement")
    );
    assert!(!prepared.dashboard_read_only);
    assert!(
        f.prepare_copilot(&f.a, &copilot, &second_doc)
            .await
            .is_err()
    );
    assert!(f.prepare_copilot(&f.b, &copilot, &doc).await.is_err());
    let fresh_copilot = f.session("dashboard_copilot").await;
    let prepared = f.prepare_copilot(&f.a, &fresh_copilot, &doc).await.unwrap();
    assert!(
        prepared
            .message
            .user_message
            .contains("private copilot autosave")
    );
    assert_eq!(
        restricted_owner(&f.db, &fresh_copilot).await.unwrap(),
        Some(f.a.clone())
    );
    assert!(
        is_private_copilot_destination(&f.db, &f.a, &fresh_copilot, &doc)
            .await
            .unwrap()
    );
    let scheduled_copilot = f.session("watch_execution").await;
    assert!(
        f.prepare_copilot(&f.a, &scheduled_copilot, &doc)
            .await
            .is_err()
    );
    assert_eq!(
        restricted_owner(&f.db, &scheduled_copilot).await.unwrap(),
        None
    );
    assert!(
        db_execute!(
            &f.db,
            "UPDATE collections SET is_public=TRUE WHERE id=$1",
            &collection
        )
        .is_err()
    );
    let public = f.collection(true).await;
    assert!(
        crate::collection_service::add_dashboard(&f.db, &public, &doc, &f.ws, &f.a, None)
            .await
            .is_err()
    );
    assert!(
        db_execute!(
            &f.db,
            "UPDATE dashboards SET user_id=$1 WHERE dashboard_id=$2",
            &f.b,
            &doc
        )
        .is_err()
    );
    let racing_doc = f.doc().await;
    let racing_collection = f.collection(false).await;
    db_execute!(
        &f.db,
        "INSERT INTO collection_dashboards (collection_id,dashboard_id,position) VALUES ($1,$2,0)",
        &racing_collection,
        &racing_doc
    )
    .unwrap();
    let racing_copilot = f.session("knowledge_copilot").await;
    let (admitted, published) = tokio::join!(
        admit_external_dispatch(
            &f.db,
            &f.config,
            f.scope(
                &racing_copilot,
                PersonalMcpSurface::PrivateDocumentCopilot,
                Some(&racing_doc)
            )
        ),
        async {
            db_execute!(
                &f.db,
                "UPDATE collections SET is_public=TRUE WHERE id=$1",
                &racing_collection
            )
        }
    );
    assert_ne!(
        admitted.is_ok(),
        published.is_ok(),
        "document permission flip and external admission cannot both win"
    );
    // New private conversations inherit restricted document context; schedules deny.
    let inherited = f.session("chat").await;
    inherit_private_document_context(&f.db, &f.a, &f.ws, Some(&inherited), None)
        .await
        .unwrap();
    assert!(
        begin_native_publication(&f.db, &f.a, &inherited)
            .await
            .is_err()
    );
    let scheduled = f.session("watch_execution").await;
    assert!(
        inherit_private_document_context(&f.db, &f.a, &f.ws, Some(&scheduled), None)
            .await
            .is_err()
    );
    assert!(
        inherit_private_document_context(&f.db, &f.a, &f.ws, None, None)
            .await
            .is_err(),
        "inbound proxy has no private session"
    );
    // Current restriction also filters stale workspace-visible sync rows.
    crate::sync_log_service::write_sync_entry(
        &f.db,
        crate::sync_log_service::SyncEntryParams {
            workspace_id: &f.ws,
            entity_type: "chat_session",
            entity_id: &private,
            action: kyomi_types::sync::SyncActionType::Update,
            data: Some(serde_json::json!({"title":"private external output"})),
            owner_user_id: Some(&f.a),
            is_workspace_visible: true,
        },
    )
    .await
    .unwrap();
    let entries = crate::sync_log_service::get_entries_since(&f.db, &f.ws, 0, &f.b, 1000)
        .await
        .unwrap();
    assert!(entries.iter().all(|entry| entry.entity_id != private));
    assert!(
        crate::sync_log_service::get_entries_since(&f.db, &f.ws, 0, &f.a, 1000)
            .await
            .unwrap()
            .iter()
            .any(|entry| entry.entity_id == private)
    );
    connections::disable_connection(&f.db, &f.a, &f.connection, 1)
        .await
        .unwrap();
    let disconnected = f.session("chat").await;
    assert!(f.admit(&disconnected).await.is_err());
    assert_eq!(
        restricted_owner(&f.db, &private).await.unwrap(),
        Some(f.a.clone()),
        "disconnect never restores sharing"
    );
    f.cleanup().await;
}
#[tokio::test]
async fn personal_mcp_privacy_sqlite() {
    matrix(crate::test_support::test_pool().await).await;
}
#[tokio::test]
async fn personal_mcp_privacy_postgres() {
    let Some(db) =
        crate::test_pg::postgres_test_pool_or_skip("personal_mcp_privacy_postgres").await
    else {
        return;
    };
    matrix(db).await;
}
