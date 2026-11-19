// SPDX-License-Identifier: AGPL-3.0-or-later
//! Registry-level private-document context inheritance, shared by legacy and
//! durable execution. It does not gate tool use.
use super::{AgentTool, ToolContext};
use std::sync::Arc;

/// Inherit the workspace's persisted private-document context around document
/// reads, before their content can enter the model context and again after
/// they complete. This is conversation confidentiality, not a tool restriction.
pub(super) struct PrivateDocumentContextTool(pub Arc<dyn AgentTool>);
#[async_trait::async_trait]
impl AgentTool for PrivateDocumentContextTool {
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
    fn is_internal_only(&self) -> bool {
        self.0.is_internal_only()
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
    fn bad_request_is_pre_effect(&self) -> bool {
        self.0.bad_request_is_pre_effect()
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
        let result = self.0.execute(args, ctx).await;
        if document_read {
            // Admission commits the document marker before external content can
            // be written. Recheck after reads, including read-modify-write error
            // paths, so a concurrent admission cannot expose content-derived
            // results through the earlier authorization snapshot.
            inherit(ctx).await?;
        }
        result
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
    use std::sync::atomic::{AtomicBool, Ordering};

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
        let outcome = registry.get_tool("edit_knowledge_file").unwrap().execute(serde_json::json!({"path":"private-doc","old_text":secret,"new_text":"replacement"}),&ctx).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&outcome).unwrap();
        assert_eq!(value["success"], false);
        assert!(
            value["error"].as_str().unwrap().contains("old_text appears 2 times"),
            "the inherited private session runs the tool instead of being refused: {outcome}"
        );
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

    /// A document read that has passed the wrapper's first inheritance check
    /// and waits, so a concurrent admission lands before the second check.
    struct ReadThenWaitTool {
        read: Arc<AtomicBool>,
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }
    #[async_trait::async_trait]
    impl AgentTool for ReadThenWaitTool {
        fn name(&self) -> &str {
            "write_knowledge_file"
        }
        fn description(&self) -> &str {
            "Controlled document read"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        async fn execute(
            &self,
            _: serde_json::Value,
            _: &ToolContext,
        ) -> kyomi_core::Result<String> {
            self.read.store(true, Ordering::SeqCst);
            self.started.notify_one();
            self.release.notified().await;
            Ok("private content read".into())
        }
    }

    #[tokio::test]
    async fn personal_mcp_mutator_rechecks_inheritance_after_concurrent_document_admission() {
        let f = DocumentFixture::new().await;
        f.document("racing-doc", "old private content").await;
        f.session("admitting-copilot", "knowledge_copilot", false)
            .await;
        f.session("racing-chat", "chat", true).await;
        let read = Arc::new(AtomicBool::new(false));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let mut registry = super::super::ToolRegistry::new();
        registry.register(Arc::new(ReadThenWaitTool {
            read: read.clone(),
            started: started.clone(),
            release: release.clone(),
        }));
        let tool = registry.get_tool("write_knowledge_file").unwrap().clone();
        let mut ctx = f.ctx.clone();
        ctx.session_id = Some("racing-chat".into());
        let run = tokio::spawn(async move {
            tool.execute(
                serde_json::json!({"path":"racing-doc","content":"replacement"}),
                &ctx,
            )
            .await
        });
        started.notified().await;
        assert!(
            read.load(Ordering::SeqCst),
            "the tool read before the concurrent admission"
        );
        f.admit("admitting-copilot", "racing-doc").await;
        release.notify_one();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), run)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(outcome, Err(kyomi_core::Error::Forbidden(_))),
            "the tool result cannot escape to the shared session"
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
