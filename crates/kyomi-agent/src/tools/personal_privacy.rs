// SPDX-License-Identifier: AGPL-3.0-or-later
//! Registry-level native write guard, shared by legacy and durable execution.
use super::{AgentTool, ToolContext};
use std::sync::Arc;

/// This allowlist describes trusted native implementations, not remote tool
/// annotations. New/unknown native tools default to potentially publishing.
fn reads_only(name: &str) -> bool {
    matches!(
        name,
        "list_datasources"
            | "validate_sql"
            | "get_table_info"
            | "browse_catalog"
            | "search_knowledge"
            | "read_knowledge_file"
            | "list_knowledge_files"
            | "get_workspace_info"
            | "get_chartml_spec"
            | "validate_chartml"
            | "search_dashboards"
            | "get_dashboard_info"
            | "search_watches"
            | "get_watch_info"
            | "forecast_data"
            | "browse_resources"
            | "read_resource"
            | "list_analytics_sites"
    )
}

pub(super) struct NativePrivacyTool(pub Arc<dyn AgentTool>);
#[async_trait::async_trait]
impl AgentTool for NativePrivacyTool {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn description(&self) -> &str {
        self.0.description()
    }
    fn parameters_schema(&self) -> serde_json::Value {
        self.0.parameters_schema()
    }
    fn is_copilot_only(&self) -> bool {
        self.0.is_copilot_only()
    }
    fn is_mcp_only(&self) -> bool {
        self.0.is_mcp_only()
    }
    fn annotations(&self) -> Option<crate::types::ToolAnnotations> {
        self.0.annotations()
    }
    fn result_domain_outcome(&self, text: &str) -> agent_runtime::DomainOutcome {
        self.0.result_domain_outcome(text)
    }
    fn failure_domain_outcome(&self) -> agent_runtime::DomainOutcome {
        self.0.failure_domain_outcome()
    }
    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let document_read = matches!(
            self.name(),
            "search_knowledge"
                | "read_knowledge_file"
                | "list_knowledge_files"
                | "search_dashboards"
                | "get_dashboard_info"
                | "edit_knowledge_file"
                | "modify_dashboard"
                | "write_knowledge_file"
        );
        if document_read {
            inherit(ctx).await?;
        }
        let result = self.execute_with_publication(args, ctx).await;
        if document_read {
            // Admission commits the document marker before external content can
            // be written. Recheck after reads, including read-modify-write error
            // paths, so a concurrent admission cannot expose content-derived
            // results through the earlier authorization snapshot. The completed
            // operation's own publication claim has already been released.
            inherit(ctx).await?;
        }
        result
    }
}

impl NativePrivacyTool {
    async fn execute_with_publication(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> kyomi_core::Result<String> {
        let Some(session) = ctx
            .session_id
            .as_deref()
            .filter(|_| !reads_only(self.name()))
        else {
            return self.0.execute(args, ctx).await;
        };
        // Publication ordering is durable and independent of this worker's kill
        // switch. Other replicas (or already-running turns) may enable MCP.
        let restricted =
            kyomi_auth::personal_mcp_privacy::restricted_owner(&ctx.db, session).await?;
        if restricted.is_some() {
            let private_edit = matches!(
                self.name(),
                "modify_dashboard" | "edit_knowledge_file" | "write_knowledge_file"
            ) && if let Some(document) = ctx.document_id.as_deref() {
                kyomi_auth::personal_mcp_privacy::is_private_copilot_destination(
                    &ctx.db,
                    &ctx.user_id,
                    session,
                    document,
                )
                .await?
            } else {
                false
            };
            if !private_edit {
                return Err(kyomi_core::Error::Forbidden(
                    "Personal external data cannot be published by native tools".into(),
                ));
            }
            return self.0.execute(args, ctx).await;
        }
        let token = kyomi_auth::personal_mcp_privacy::begin_native_publication(
            &ctx.db,
            &ctx.user_id,
            session,
        )
        .await?;
        // Keep owning the operation even if the caller cancels: releasing a lease
        // on Drop could authorize MCP while a buffered native side effect survives.
        let tool = self.0.clone();
        let context = ctx.clone();
        tokio::spawn(async move {
            let outcome = tool.execute(args, &context).await;
            kyomi_auth::personal_mcp_privacy::finish_native_publication(
                &context.db,
                &context.user_id,
                &token,
            )
            .await?;
            outcome
        })
        .await
        .map_err(|_| {
            kyomi_core::Error::ServiceUnavailable(
                "Native operation interrupted; privacy admission remains closed".into(),
            )
        })?
    }
}

async fn inherit(ctx: &ToolContext) -> kyomi_core::Result<()> {
    kyomi_auth::personal_mcp_privacy::inherit_private_document_context(
        &ctx.db,
        &ctx.user_id,
        &ctx.workspace_id,
        ctx.session_id.as_deref(),
        ctx.document_id.as_deref(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct PublishingTool {
        calls: Arc<AtomicUsize>,
        started: Arc<tokio::sync::Notify>,
        finish: Arc<tokio::sync::Notify>,
    }
    #[async_trait::async_trait]
    impl AgentTool for PublishingTool {
        fn name(&self) -> &str {
            "create_dashboard"
        }
        fn description(&self) -> &str {
            "Controlled native write"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        async fn execute(
            &self,
            _: serde_json::Value,
            _: &ToolContext,
        ) -> kyomi_core::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            self.finish.notified().await;
            Err(kyomi_core::Error::BadRequest("controlled failure".into()))
        }
    }
    #[tokio::test]
    async fn personal_mcp_off_config_native_claim_blocks_on_config_admission_and_survives_cancellation()
     {
        let db = crate::test_support::test_pool().await;
        kyomi_core::db_execute!(
            &db,
            "INSERT INTO users (user_id,email) VALUES ('user-a','a@example.com')"
        )
        .unwrap();
        kyomi_core::db_execute!(&db,"INSERT INTO workspaces (workspace_id,name,owner_user_id) VALUES ('ws-1','fixture','user-a')").unwrap();
        kyomi_core::db_execute!(&db,"INSERT INTO workspace_users (workspace_id,user_id,role,active) VALUES ('ws-1','user-a','workspace_user',TRUE)").unwrap();
        kyomi_core::db_execute!(&db,"INSERT INTO chat_sessions (session_id,user_id,workspace_id) VALUES ('native-session','user-a','ws-1')").unwrap();
        let mut ctx = crate::test_support::build_ctx(db.clone());
        ctx.session_id = Some("native-session".into());
        let mut config = kyomi_core::Config::test_config();
        config.personal_mcp_enabled = false;
        ctx.config = Arc::new(config.clone());
        let mut enabled_config = config;
        enabled_config.personal_mcp_enabled = true;
        let connection = kyomi_auth::personal_mcp_service::create_connection(
            &db,
            "user-a",
            kyomi_types::personal_mcp::SavePersonalMcpConnection {
                name: "off/on overlap fixture".into(),
                endpoint: "https://example.com/mcp".into(),
                auth_kind: kyomi_types::personal_mcp::McpAuthKind::None,
                bearer_secret: None,
            },
            &[0; 32],
        )
        .await
        .unwrap();
        kyomi_auth::personal_mcp_service::set_workspace_enabled(
            &db,
            "user-a",
            &connection.connection_id,
            1,
            "ws-1",
            true,
        )
        .await
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let finish = Arc::new(tokio::sync::Notify::new());
        let mut registry = super::super::ToolRegistry::new();
        registry.register(Arc::new(PublishingTool {
            calls: calls.clone(),
            started: started.clone(),
            finish: finish.clone(),
        }));
        let prepared = registry.get_tool("create_dashboard").unwrap().clone();
        let run = tokio::spawn({
            let ctx = ctx.clone();
            let prepared = prepared.clone();
            async move { prepared.execute(serde_json::json!({}), &ctx).await }
        });
        started.notified().await;
        let outstanding = kyomi_auth::personal_mcp_privacy::outstanding_native_publications(
            &db,
            "user-a",
            "native-session",
        )
        .await
        .unwrap();
        assert_eq!(outstanding.len(), 1);
        assert!(
            kyomi_auth::personal_mcp_privacy::admit_external_dispatch(
                &db,
                &enabled_config,
                kyomi_auth::personal_mcp_privacy::ExternalDispatch {
                    actor: "user-a",
                    workspace: "ws-1",
                    session: "native-session",
                    connection: &connection.connection_id,
                    generation: 1,
                    surface: kyomi_auth::personal_mcp_privacy::PersonalMcpSurface::InteractiveChat,
                    document: None,
                }
            )
            .await
            .is_err(),
            "an off-config native worker still blocks an on-config external dispatcher"
        );
        assert_eq!(
            kyomi_auth::personal_mcp_privacy::restricted_owner(&db, "native-session")
                .await
                .unwrap(),
            None
        );
        run.abort();
        let _ = run.await;
        assert_eq!(
            kyomi_auth::personal_mcp_privacy::outstanding_native_publications(
                &db,
                "user-a",
                "native-session"
            )
            .await
            .unwrap(),
            outstanding,
            "caller cancellation must not release a live side effect"
        );
        assert!(kyomi_core::db_execute!(&db,"UPDATE chat_sessions SET personal_data_owner_id='user-a' WHERE session_id='native-session'").is_err());
        finish.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if kyomi_auth::personal_mcp_privacy::outstanding_native_publications(
                    &db,
                    "user-a",
                    "native-session",
                )
                .await
                .unwrap()
                .is_empty()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        kyomi_auth::personal_mcp_privacy::admit_external_dispatch(
            &db,
            &enabled_config,
            kyomi_auth::personal_mcp_privacy::ExternalDispatch {
                actor: "user-a",
                workspace: "ws-1",
                session: "native-session",
                connection: &connection.connection_id,
                generation: 1,
                surface: kyomi_auth::personal_mcp_privacy::PersonalMcpSurface::InteractiveChat,
                document: None,
            },
        )
        .await
        .unwrap();
        assert!(prepared.execute(serde_json::json!({}), &ctx).await.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "prepared tool cannot publish after taint"
        );
        let builtins = super::super::create_default_registry();
        for name in [
            "create_dashboard",
            "write_knowledge_file",
            "create_watch",
            "update_watch",
            "trigger_watch",
            "update_watch_draft",
            "update_chart",
            "query_datasource",
        ] {
            assert!(
                builtins
                    .get_tool(name)
                    .unwrap()
                    .execute(serde_json::json!({}), &ctx)
                    .await
                    .is_err(),
                "actual native publishing path {name} must deny before validation/network"
            );
        }
    }

    struct DocumentFixture {
        ctx: ToolContext,
        connection: String,
        config: kyomi_core::Config,
    }
    impl DocumentFixture {
        async fn new() -> Self {
            let db = crate::test_support::test_pool().await;
            crate::test_support::seed_user_and_workspace(&db).await;
            kyomi_core::db_execute!(
                &db,
                "INSERT INTO users (user_id,email) VALUES ('user-b','b@example.com')"
            )
            .unwrap();
            kyomi_core::db_execute!(&db,"INSERT INTO workspace_users (workspace_id,user_id,role,active) VALUES ('ws-1','user-b','workspace_user',TRUE)").unwrap();
            let ctx = crate::test_support::build_ctx(db.clone());
            let connection = kyomi_auth::personal_mcp_service::create_connection(
                &db,
                "user-a",
                kyomi_types::personal_mcp::SavePersonalMcpConnection {
                    name: "private document fixture".into(),
                    endpoint: "https://example.com/mcp".into(),
                    auth_kind: kyomi_types::personal_mcp::McpAuthKind::None,
                    bearer_secret: None,
                },
                &[0; 32],
            )
            .await
            .unwrap()
            .connection_id;
            kyomi_auth::personal_mcp_service::set_workspace_enabled(
                &db,
                "user-a",
                &connection,
                1,
                "ws-1",
                true,
            )
            .await
            .unwrap();
            let mut config = kyomi_core::Config::test_config();
            config.personal_mcp_enabled = true;
            Self {
                ctx,
                connection,
                config,
            }
        }
        async fn session(&self, id: &str, kind: &str, shared: bool) {
            kyomi_auth::chat_service::create_session_with_id(
                &self.ctx.db,
                "user-a",
                "ws-1",
                id,
                None,
                kind,
                None,
            )
            .await
            .unwrap();
            if shared {
                assert!(
                    kyomi_auth::chat_service::set_session_shared(&self.ctx.db, id, true)
                        .await
                        .unwrap()
                );
            }
        }
        async fn document(&self, id: &str, content: &str) {
            kyomi_core::db_execute!(&self.ctx.db,"INSERT INTO dashboards (dashboard_id,user_id,workspace_id,title,content,doc_type) VALUES ($1,'user-a','ws-1',$1,$2,'knowledge')",id,content).unwrap();
        }
        async fn admit(&self, session: &str, document: &str) {
            kyomi_auth::personal_mcp_privacy::admit_external_dispatch(
                &self.ctx.db,&self.config,kyomi_auth::personal_mcp_privacy::ExternalDispatch {
                    actor:"user-a",workspace:"ws-1",session,connection:&self.connection,generation:1,
                    surface:kyomi_auth::personal_mcp_privacy::PersonalMcpSurface::PrivateDocumentCopilot,
                    document:Some(document),
                },
            ).await.unwrap();
        }
    }

    #[tokio::test]
    async fn personal_mcp_document_mutators_deny_shared_diagnostics_and_inherit_private_context() {
        let f = DocumentFixture::new().await;
        let secret = "external-sentinel";
        f.document("private-doc", &format!("{secret} {secret}"))
            .await;
        f.session("private-copilot", "knowledge_copilot", false)
            .await;
        f.admit("private-copilot", "private-doc").await;
        f.session("shared-chat", "chat", true).await;
        f.session("private-chat", "chat", false).await;
        let registry = super::super::create_default_registry();
        let mut ctx = f.ctx.clone();
        ctx.session_id = Some("shared-chat".into());
        let (_, mut other) = ctx.ws_manager.connect("user-b").unwrap();
        let cases = [
            (
                "edit_knowledge_file",
                serde_json::json!({"path":"private-doc","old_text":secret,"new_text":"replacement"}),
            ),
            (
                "edit_knowledge_file",
                serde_json::json!({"path":"private-doc","old_text":"absent","new_text":"replacement"}),
            ),
            (
                "modify_dashboard",
                serde_json::json!({"dashboard_id":"private-doc","title":"private-doc"}),
            ),
            (
                "modify_dashboard",
                serde_json::json!({"dashboard_id":"private-doc","title":"renamed"}),
            ),
            (
                "write_knowledge_file",
                serde_json::json!({"path":"private-doc","content":"replacement"}),
            ),
        ];
        let mut tracker = crate::thinking::AgentThinkingTracker::new(
            crate::thinking::AgentThinkingTrackerConfig {
                workspace_id: "ws-1".into(),
                session_id: "shared-chat".into(),
                user_id: "user-a".into(),
                message_id: "shared-message".into(),
                ws_manager: ctx.ws_manager.clone(),
                workspace_user_ids: Some(vec!["user-a".into(), "user-b".into()]),
                context_type: Some("chat".into()),
                context_window: 0,
                incremental_flush: None,
            },
        );
        for (name, args) in cases {
            let outcome = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                registry.get_tool(name).unwrap().execute(args, &ctx),
            )
            .await
            .unwrap();
            assert!(
                matches!(&outcome, Err(kyomi_core::Error::Forbidden(_))),
                "{name}: {outcome:?}"
            );
            let error = outcome.unwrap_err().to_string();
            assert!(
                !error.contains(secret)
                    && !error.contains("old_text")
                    && !error.contains("2 times")
            );
            tracker
                .tool_execution_started(name, &serde_json::json!({}))
                .await;
            tracker.tool_execution_completed(name, &error, false).await;
        }
        while let Ok(frame) = other.try_recv() {
            assert!(
                !frame.contains(secret)
                    && !frame.contains("old_text")
                    && !frame.contains("2 times"),
                "{frame}"
            );
        }
        let messages = kyomi_auth::chat_service::get_authorized_session_messages(
            &ctx.db,
            &[0; 32],
            "shared-chat",
            "user-b",
            "ws-1",
            100,
        )
        .await
        .unwrap();
        assert!(
            messages.is_empty(),
            "denied tools must not persist private output into shared history"
        );
        assert_eq!(
            kyomi_auth::personal_mcp_privacy::restricted_owner(&ctx.db, "shared-chat")
                .await
                .unwrap(),
            None
        );
        ctx.session_id = Some("private-chat".into());
        let outcome = registry.get_tool("edit_knowledge_file").unwrap().execute(serde_json::json!({"path":"private-doc","old_text":secret,"new_text":"replacement"}),&ctx).await;
        assert!(matches!(outcome, Err(kyomi_core::Error::Forbidden(_))));
        assert_eq!(
            kyomi_auth::personal_mcp_privacy::restricted_owner(&ctx.db, "private-chat")
                .await
                .unwrap()
                .as_deref(),
            Some("user-a")
        );
        assert!(
            !kyomi_auth::chat_service::set_session_shared(&ctx.db, "private-chat", true)
                .await
                .unwrap()
        );
        assert!(
            kyomi_auth::conversation_events::ConversationStore::new(
                &ctx.db, &[0; 32], "user-b", "ws-1",
            )
            .replay(
                &agent_runtime::ConversationId("private-chat".into()),
                0,
                100
            )
            .await
            .is_err(),
            "another member cannot replay the inherited private conversation",
        );
        ctx.session_id = Some("private-copilot".into());
        ctx.document_id = Some("private-doc".into());
        // Diagnostics remain intact for the admitted owner-only destination.
        let result = registry.get_tool("edit_knowledge_file").unwrap().execute(serde_json::json!({"path":"private-doc","old_text":secret,"new_text":"replacement"}),&ctx).await.unwrap();
        assert!(result.contains("old_text appears 2 times"), "{result}");
        ctx.embedding = crate::test_support::loaded_embedding();
        let result = registry.get_tool("edit_knowledge_file").unwrap().execute(serde_json::json!({"path":"private-doc","old_text":format!("{secret} {secret}"),"new_text":"owner edit"}),&ctx).await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result).unwrap()["success"],
            true
        );
        let saved =
            kyomi_auth::dashboard_service::get_dashboard(&ctx.db, "private-doc", "ws-1", "user-a")
                .await
                .unwrap()
                .unwrap();
        assert_eq!(saved.content, "owner edit");
        // Exercise the legacy dashboard mutation emitter too: its sessionless
        // dashboard_id is private metadata and must not reach another member.
        let result = registry
            .get_tool("modify_dashboard")
            .unwrap()
            .execute(
                serde_json::json!({"dashboard_id":"private-doc","title":"owner title"}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result).unwrap()["success"],
            true
        );
        assert!(
            other.try_recv().is_err(),
            "private copilot writes cannot broadcast protected document metadata"
        );
    }

    #[tokio::test]
    async fn personal_mcp_mutator_rechecks_inheritance_after_concurrent_document_admission() {
        let f = DocumentFixture::new().await;
        f.document("racing-doc", "old private content").await;
        f.session("admitting-copilot", "knowledge_copilot", false)
            .await;
        f.session("racing-chat", "chat", true).await;
        let registry = super::super::create_default_registry();
        let tool = registry.get_tool("write_knowledge_file").unwrap().clone();
        let mut ctx = f.ctx.clone();
        ctx.session_id = Some("racing-chat".into());
        let embedding = ctx.embedding.clone();
        // The real tool waits for embedding before looking up stored content.
        // Seeing its claim proves the wrapper's initial inheritance passed;
        // withholding embedding ensures document reads cannot precede admission.
        let run = tokio::spawn(async move {
            tool.execute(serde_json::json!({"path":"racing-doc","content":"replacement","content_hash":"stale-hash"}),&ctx).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if !kyomi_auth::personal_mcp_privacy::outstanding_native_publications(
                    &f.ctx.db,
                    "user-a",
                    "racing-chat",
                )
                .await
                .unwrap()
                .is_empty()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        f.admit("admitting-copilot", "racing-doc").await;
        let loaded = crate::test_support::loaded_embedding();
        embedding.set(loaded.wait_ready().await.unwrap().clone());
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), run)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(outcome, Err(kyomi_core::Error::Forbidden(_))),
            "the underlying CAS result cannot escape to the shared session"
        );
        assert!(
            kyomi_auth::personal_mcp_privacy::outstanding_native_publications(
                &f.ctx.db,
                "user-a",
                "racing-chat"
            )
            .await
            .unwrap()
            .is_empty()
        );
        assert_eq!(
            kyomi_auth::personal_mcp_privacy::restricted_owner(&f.ctx.db, "racing-chat")
                .await
                .unwrap(),
            None
        );
        let saved =
            kyomi_auth::dashboard_service::get_dashboard(&f.ctx.db, "racing-doc", "ws-1", "user-a")
                .await
                .unwrap()
                .unwrap();
        assert_eq!(saved.content, "old private content");
    }

    #[tokio::test]
    async fn personal_mcp_mutator_sql_validation_is_private_and_preserves_owner_diagnostics() {
        let f = DocumentFixture::new().await;
        let content = super::super::document::sql_validation_tests::chartml("SELECT FROM");
        f.document("invalid-doc", &content).await;
        f.session("invalid-copilot", "dashboard_copilot", false)
            .await;
        f.admit("invalid-copilot", "invalid-doc").await;
        f.session("invalid-shared", "chat", true).await;
        let mut ctx = f.ctx.clone();
        ctx.embedding = crate::test_support::loaded_embedding();
        let (calls, handler, connection) =
            super::super::document::sql_validation_tests::attach_sql_datasource(&mut ctx).await;
        let registry = super::super::create_default_registry();
        let tool = registry.get_tool("modify_dashboard").unwrap();
        ctx.session_id = Some("invalid-shared".into());
        for title in ["invalid-doc", "renamed"] {
            let outcome = tool
                .execute(
                    serde_json::json!({"dashboard_id":"invalid-doc","title":title}),
                    &ctx,
                )
                .await;
            assert!(matches!(outcome, Err(kyomi_core::Error::Forbidden(_))));
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "shared attempts must deny before reading/validating private SQL"
        );
        ctx.session_id = Some("invalid-copilot".into());
        ctx.document_id = Some("invalid-doc".into());
        for title in ["invalid-doc", "renamed"] {
            let result = tool
                .execute(
                    serde_json::json!({"dashboard_id":"invalid-doc","title":title}),
                    &ctx,
                )
                .await
                .unwrap();
            let value: serde_json::Value = serde_json::from_str(&result).unwrap();
            assert_eq!(value["success"], false);
            assert!(
                value["error"]
                    .as_str()
                    .unwrap()
                    .starts_with("Document contains invalid SQL: "),
                "{result}"
            );
            assert!(!value["validation_errors"].as_array().unwrap().is_empty());
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "both no-op and title-only private writes keep their real SQL validation"
        );
        super::super::document::sql_validation_tests::finish(&ctx, handler, connection).await;
    }
}
