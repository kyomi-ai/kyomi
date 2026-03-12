// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat agent adapter — wraps [`CustomAgent`] with persistence, context loading,
//! and knowledge graph context retrieval.
//!
//! [`ChatAgentAdapter`] is the bridge between the agent loop and the database.
//! It handles:
//! - Loading conversation history from the DB into the agent's state
//! - Injecting knowledge context (tables, columns, metrics, learnings)
//!   via kyomi-knowledge SQL retrieval
//! - Persisting intermediate messages and agent metadata after each chat
//! - Wiring the thinking tracker to agent callbacks

use std::sync::Arc;

use anyhow::Context;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use kyomi_auth::chat_service;
use kyomi_core::{DbPool, KVPool};
use kyomi_embed::LazyEmbedding;

use crate::agent::CustomAgent;
use crate::thinking::AgentThinkingTracker;
use crate::types::{Message, MessageRole, ToolCall};

// ---------------------------------------------------------------------------
// ChatAgentAdapter
// ---------------------------------------------------------------------------

/// Adapter wrapping a [`CustomAgent`] with database persistence and context management.
///
/// One adapter is created per user message exchange. It:
/// 1. Loads existing conversation history from the DB
/// 2. Injects knowledge graph context into each user message
/// 3. Delegates to `CustomAgent::chat()` for the LLM loop
/// 4. Persists new messages and agent metadata back to the DB
pub struct ChatAgentAdapter {
    agent: CustomAgent,
    pub user_id: String,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub component: String,
    context_loaded: bool,
    db: DbPool,
    kv: KVPool,
    encryption_key: Arc<[u8; 32]>,
    embedding: LazyEmbedding,
    /// Number of messages loaded from the DB during context loading.
    /// Used to determine which new messages need to be persisted.
    messages_loaded_count: usize,
}

impl ChatAgentAdapter {
    /// Create a new adapter.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent: CustomAgent,
        user_id: String,
        workspace_id: String,
        session_id: Option<String>,
        component: String,
        db: DbPool,
        kv: KVPool,
        encryption_key: Arc<[u8; 32]>,
        embedding: LazyEmbedding,
    ) -> Self {
        Self {
            agent,
            user_id,
            workspace_id,
            session_id,
            component,
            context_loaded: false,
            db,
            kv,
            encryption_key,
            embedding,
            messages_loaded_count: 0,
        }
    }

    /// Wire a thinking tracker to the agent's callbacks.
    ///
    /// The tracker methods are async but callbacks are synchronous closures.
    /// We use `tokio::task::spawn` to bridge the gap — each callback fires
    /// an async task that runs the tracker method. This keeps the agent loop
    /// non-blocking while still delivering events in near-real-time.
    pub fn set_thinking_tracker(&mut self, tracker: Arc<tokio::sync::Mutex<AgentThinkingTracker>>) {
        let callbacks = self.agent.callbacks_mut();

        // on_thinking -> tracker.agent_thought(thought)
        // Note: error handling for Redis publish is inside the tracker methods themselves.
        let tracker_thinking = tracker.clone();
        callbacks.on_thinking = Some(Box::new(move |thought: &str| {
            let tracker = tracker_thinking.clone();
            let thought = thought.to_string();
            tokio::task::spawn(async move {
                tracker.lock().await.agent_thought(&thought).await;
            });
        }));

        // on_token_usage -> accumulate + tracker.update_token_usage(...)
        let tracker_usage = tracker.clone();
        callbacks.on_token_usage = Some(Box::new(
            move |input_tokens: u32, output_tokens: u32, cost: Option<f64>| {
                let tracker = tracker_usage.clone();
                tokio::task::spawn(async move {
                    tracker
                        .lock()
                        .await
                        .update_token_usage(input_tokens, output_tokens, cost)
                        .await;
                });
            },
        ));

        // on_tool_start -> tracker.tool_execution_started(tool_name, tool_input)
        let tracker_tool_start = tracker.clone();
        callbacks.on_tool_start = Some(Box::new(
            move |tool_name: &str, tool_input: &serde_json::Value| {
                let tracker = tracker_tool_start.clone();
                let name = tool_name.to_string();
                let input = tool_input.clone();
                tokio::task::spawn(async move {
                    tracker
                        .lock()
                        .await
                        .tool_execution_started(&name, &input)
                        .await;
                });
            },
        ));

        // on_tool_end -> tracker.tool_execution_completed(tool_name, result, success)
        let tracker_tool_end = tracker.clone();
        callbacks.on_tool_end = Some(Box::new(
            move |tool_name: &str, result: &str, success: bool| {
                let tracker = tracker_tool_end.clone();
                let name = tool_name.to_string();
                let result_str = result.to_string();
                tokio::task::spawn(async move {
                    tracker
                        .lock()
                        .await
                        .tool_execution_completed(&name, &result_str, success)
                        .await;
                });
            },
        ));

        // on_preparing_response -> tracker.preparing_response()
        let tracker_preparing = tracker;
        callbacks.on_preparing_response = Some(Box::new(move || {
            let tracker = tracker_preparing.clone();
            tokio::task::spawn(async move {
                tracker.lock().await.preparing_response().await;
            });
        }));
    }

    /// Load existing conversation context from the database.
    ///
    /// Restores agent metadata (iteration counter, compaction state) and
    /// message history so the agent can continue where it left off.
    ///
    /// Returns `true` if context was loaded, `false` if no session or no
    /// messages exist.
    pub async fn load_context(&mut self) -> kyomi_core::Result<bool> {
        let Some(ref session_id) = self.session_id else {
            return Ok(false);
        };

        // Get session info for metadata restoration.
        let session = chat_service::get_session(&self.db, session_id).await?;
        let Some(session) = session else {
            return Ok(false);
        };

        // Restore agent state from session config if available.
        if let Some(ref config) = session.config
            && let Some(agent_state) = config.get("agent_state")
        {
            let state = self.agent.state_mut();

            if let Some(gi) = agent_state.get("global_iteration").and_then(|v| v.as_u64()) {
                state.global_iteration = gi as u32;
            }
            if let Some(summary) = agent_state
                .get("compacted_summary")
                .and_then(|v| v.as_str())
                && !summary.is_empty()
            {
                state.compacted_summary = Some(summary.to_string());
            }
            if let Some(idx) = agent_state
                .get("messages_since_compaction_index")
                .and_then(|v| v.as_u64())
            {
                state.messages_since_compaction_index = idx as usize;
            }
            if let Some(lit) = agent_state
                .get("last_input_tokens")
                .and_then(|v| v.as_u64())
            {
                state.last_input_tokens = lit as u32;
            }
        }

        // Load all messages from DB.
        let db_messages = chat_service::get_agent_messages(
            &self.db,
            &self.encryption_key,
            session_id,
            None, // load all messages
        )
        .await?;

        if db_messages.is_empty() {
            self.context_loaded = true;
            return Ok(false);
        }

        // Convert DB messages to agent Message structs.
        let state = self.agent.state_mut();
        for msg in &db_messages {
            let agent_msg = db_message_to_agent_message(msg);
            state.messages.push(agent_msg);
        }

        self.messages_loaded_count = db_messages.len();
        self.context_loaded = true;

        info!(
            session_id = %session_id,
            message_count = db_messages.len(),
            "Loaded agent context from database"
        );

        Ok(true)
    }

    /// Retrieve knowledge context (tables, columns, metrics, learnings) using
    /// pgvector-based semantic search in PostgreSQL.
    ///
    /// Loads conversation context from Redis, performs vector search + expansion,
    /// records new injections, saves context back, and returns a formatted
    /// `<knowledge_context>` block ready for prepending to the user message.
    ///
    /// Returns an empty string if no relevant context is found.
    async fn inject_knowledge_context(
        &self,
        session_id: &str,
        message: &str,
    ) -> anyhow::Result<String> {
        // 1. Get the embedding service
        let embed = self.embedding.wait_ready().await
            .context("Embedding service not ready")?;

        // 2. Retrieve and inject (loads context from Redis, searches pgvector,
        //    records injection, saves back to Redis)
        let (context_block, _context) = kyomi_knowledge::context::retrieve_and_inject(
            &self.kv,
            session_id,
            &self.db,
            embed,
            &self.workspace_id,
            message,
            kyomi_knowledge::retrieval::PER_TURN_TOKEN_BUDGET,
        )
        .await?;

        Ok(context_block)
    }

    /// Persist new messages and agent metadata to the database after a chat.
    ///
    /// Saves any intermediate messages (tool calls, tool results) that the
    /// agent generated during the loop, and updates the session config with
    /// the current agent state.
    pub async fn persist_after_chat(&mut self) -> kyomi_core::Result<()> {
        let Some(ref session_id) = self.session_id else {
            return Ok(());
        };

        let state = self.agent.state();
        let total_messages = state.messages.len();

        // Save any messages beyond what we loaded from the DB.
        // The first `messages_loaded_count` messages came from the DB;
        // everything after is new (user message, tool calls, tool results, etc.).
        if total_messages > self.messages_loaded_count {
            let new_messages = &state.messages[self.messages_loaded_count..];

            for msg in new_messages {
                // Skip system messages (they're part of the prompt, not stored).
                if msg.role == MessageRole::System {
                    continue;
                }

                // Skip user messages — they're already stored by the HTTP handler
                // before the agent runs. Persisting them again would create
                // duplicates (with the metadata prefix baked into content).
                if msg.role == MessageRole::User {
                    continue;
                }

                let role = match msg.role {
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::Tool => "tool",
                    MessageRole::System => continue,
                };

                let tool_calls_json = msg
                    .tool_calls
                    .as_ref()
                    .map(|tc| serde_json::to_value(tc).unwrap_or_default());

                chat_service::add_message(
                    &self.db,
                    &self.encryption_key,
                    session_id,
                    role,
                    &msg.content,
                    None, // metadata
                    None, // auto-generate message_id
                    None, // current_time_user_tz
                    if msg.role == MessageRole::User {
                        msg.user_id.as_deref()
                    } else {
                        None
                    },
                    msg.tool_call_id.as_deref(),
                    msg.name.as_deref(),
                    tool_calls_json.as_ref(),
                )
                .await?;
            }
        }

        // Save agent metadata to session config.
        let metadata = serde_json::json!({
            "agent_state": {
                "global_iteration": state.global_iteration,
                "compacted_summary": state.compacted_summary,
                "messages_since_compaction_index": state.messages_since_compaction_index,
                "last_input_tokens": state.last_input_tokens,
            }
        });

        chat_service::update_session(&self.db, session_id, None, None, Some(&metadata)).await?;

        info!(
            session_id = %session_id,
            new_messages = total_messages.saturating_sub(self.messages_loaded_count),
            "Persisted agent state after chat"
        );

        Ok(())
    }

    /// Run the agent loop for a user message.
    ///
    /// Handles context loading (lazy), learning injection, delegation to the
    /// agent, and post-chat persistence.
    pub async fn chat(
        &mut self,
        message: &str,
        cancel_token: CancellationToken,
        current_time_user_tz: Option<&str>,
        message_source: Option<&str>,
        user_id: Option<&str>,
    ) -> kyomi_core::Result<String> {
        // Lazy context loading on first call.
        if !self.context_loaded {
            self.load_context().await?;
        }

        let mut augmented_message = message.to_string();

        // Inject knowledge context (pgvector-based semantic search).
        if let Some(ref session_id) = self.session_id {
            match self
                .inject_knowledge_context(session_id, &augmented_message)
                .await
            {
                Ok(context_block) => {
                    if !context_block.is_empty() {
                        // Prepend knowledge context before the message.
                        augmented_message =
                            format!("{context_block}\n\n{augmented_message}");
                        info!("Injected knowledge context into user message");
                    }
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Knowledge context retrieval failed, continuing without"
                    );
                }
            }
        }

        // Run the agent loop.
        let result = self
            .agent
            .chat(
                &augmented_message,
                cancel_token,
                current_time_user_tz,
                message_source,
                user_id,
            )
            .await;

        // Persist intermediate messages and metadata regardless of success/failure.
        // We log at error level (not warn) because message persistence failures
        // mean data loss — the user's conversation may not be saved.
        if let Err(e) = self.persist_after_chat().await {
            error!(error = %e, "Failed to persist agent state after chat — messages may be lost");
        }

        result
    }

    /// Read-only access to the underlying agent state.
    pub fn agent_state(&self) -> &crate::agent::AgentState {
        self.agent.state()
    }

    /// Read-only access to the agent model name.
    pub fn model_name(&self) -> &str {
        // The model is stored on the AnthropicClient, but we expose it
        // through the adapter for convenience. For now, return the default.
        crate::anthropic::DEFAULT_MODEL
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a database `AgentMessage` to an agent `Message`.
fn db_message_to_agent_message(msg: &chat_service::AgentMessage) -> Message {
    match msg.role.as_str() {
        "user" => Message::user(&msg.content),
        "assistant" => {
            if let Some(ref tc_json) = msg.tool_calls {
                // Parse tool_calls JSON into Vec<ToolCall>.
                let tool_calls: Vec<ToolCall> = serde_json::from_value(tc_json.clone())
                    .unwrap_or_default();
                if tool_calls.is_empty() {
                    Message::assistant(&msg.content)
                } else {
                    Message::assistant_with_tool_calls(&msg.content, tool_calls)
                }
            } else {
                Message::assistant(&msg.content)
            }
        }
        "tool" => Message::tool_result(
            msg.tool_call_id.as_deref().unwrap_or(""),
            msg.tool_name.as_deref().unwrap_or(""),
            &msg.content,
        ),
        "system" => Message::system(&msg.content),
        _ => {
            warn!(role = %msg.role, "Unknown message role in DB, treating as user");
            Message::user(&msg.content)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_message_to_agent_message_user() {
        let msg = chat_service::AgentMessage {
            message_id: "m1".into(),
            role: "user".into(),
            content: "Hello".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::User);
        assert_eq!(result.content, "Hello");
    }

    #[test]
    fn db_message_to_agent_message_assistant() {
        let msg = chat_service::AgentMessage {
            message_id: "m2".into(),
            role: "assistant".into(),
            content: "Here are the results.".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Assistant);
        assert_eq!(result.content, "Here are the results.");
        assert!(result.tool_calls.is_none());
    }

    #[test]
    fn db_message_to_agent_message_assistant_with_tool_calls() {
        let tc = serde_json::json!([{
            "id": "tc_001",
            "name": "search_catalog",
            "arguments": {"query": "revenue"}
        }]);
        let msg = chat_service::AgentMessage {
            message_id: "m3".into(),
            role: "assistant".into(),
            content: "Let me search.".into(),
            tool_calls: Some(tc),
            tool_call_id: None,
            tool_name: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Assistant);
        let tc = result.tool_calls.as_ref().expect("should have tool calls");
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].name, "search_catalog");
    }

    #[test]
    fn db_message_to_agent_message_tool() {
        let msg = chat_service::AgentMessage {
            message_id: "m4".into(),
            role: "tool".into(),
            content: r#"{"tables": []}"#.into(),
            tool_calls: None,
            tool_call_id: Some("tc_001".into()),
            tool_name: Some("search_catalog".into()),
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Tool);
        assert_eq!(result.tool_call_id.as_deref(), Some("tc_001"));
        assert_eq!(result.name.as_deref(), Some("search_catalog"));
    }

    #[test]
    fn db_message_to_agent_message_system() {
        let msg = chat_service::AgentMessage {
            message_id: "m5".into(),
            role: "system".into(),
            content: "You are helpful.".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::System);
    }

    #[test]
    fn db_message_to_agent_message_unknown_role() {
        let msg = chat_service::AgentMessage {
            message_id: "m6".into(),
            role: "unknown".into(),
            content: "mystery message".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        };
        let result = db_message_to_agent_message(&msg);
        // Falls back to user.
        assert_eq!(result.role, MessageRole::User);
    }

    // -- Contract: db_message_to_agent_message edge cases -------------------

    #[test]
    fn db_message_to_agent_message_assistant_with_empty_tool_calls_array() {
        // An empty tool_calls array should be treated as no tool calls.
        let tc = serde_json::json!([]);
        let msg = chat_service::AgentMessage {
            message_id: "m7".into(),
            role: "assistant".into(),
            content: "No tools needed.".into(),
            tool_calls: Some(tc),
            tool_call_id: None,
            tool_name: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Assistant);
        // Empty tool_calls array -> treated as plain assistant message.
        assert!(result.tool_calls.is_none());
    }

    #[test]
    fn db_message_to_agent_message_assistant_with_multiple_tool_calls() {
        let tc = serde_json::json!([
            {"id": "tc_1", "name": "search_catalog", "arguments": {"query": "rev"}},
            {"id": "tc_2", "name": "get_table_info", "arguments": {"table_name": "orders"}},
            {"id": "tc_3", "name": "query_datasource", "arguments": {"sql_query": "SELECT 1", "datasource": "pg"}}
        ]);
        let msg = chat_service::AgentMessage {
            message_id: "m8".into(),
            role: "assistant".into(),
            content: "Let me investigate all these things.".into(),
            tool_calls: Some(tc),
            tool_call_id: None,
            tool_name: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Assistant);
        let tool_calls = result.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 3);
        assert_eq!(tool_calls[0].name, "search_catalog");
        assert_eq!(tool_calls[1].name, "get_table_info");
        assert_eq!(tool_calls[2].name, "query_datasource");
    }

    #[test]
    fn db_message_to_agent_message_tool_with_missing_call_id() {
        // tool_call_id is None — should default to empty string.
        let msg = chat_service::AgentMessage {
            message_id: "m9".into(),
            role: "tool".into(),
            content: "result data".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Tool);
        assert_eq!(result.tool_call_id.as_deref(), Some(""));
        assert_eq!(result.name.as_deref(), Some(""));
    }

    #[test]
    fn db_message_to_agent_message_user_preserves_content() {
        let msg = chat_service::AgentMessage {
            message_id: "m10".into(),
            role: "user".into(),
            content: "[source: web, user_local_time: 2025-01-15T10:00:00+11:00] Show me monthly revenue broken down by region and product category.".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::User);
        // Full content including metadata prefix is preserved.
        assert!(result.content.contains("[source: web"));
        assert!(result.content.contains("monthly revenue"));
    }

    #[test]
    fn db_message_to_agent_message_assistant_empty_content_with_tool_calls() {
        // Assistant message with empty content but valid tool calls.
        let tc = serde_json::json!([
            {"id": "tc_1", "name": "search_catalog", "arguments": {"query": "sales"}}
        ]);
        let msg = chat_service::AgentMessage {
            message_id: "m11".into(),
            role: "assistant".into(),
            content: "".into(),
            tool_calls: Some(tc),
            tool_call_id: None,
            tool_name: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Assistant);
        assert_eq!(result.content, "");
        assert!(result.tool_calls.is_some());
        assert_eq!(result.tool_calls.as_ref().unwrap().len(), 1);
    }

    // -- Contract: Round-trip: agent Message -> serialize -> deserialize -----

    #[test]
    fn agent_message_roundtrip_user() {
        let msg = Message::user("Show me data");
        let json = serde_json::to_string(&msg).unwrap();
        let restored: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.role, MessageRole::User);
        assert_eq!(restored.content, "Show me data");
    }

    #[test]
    fn agent_message_roundtrip_assistant_with_tool_calls() {
        let tool_calls = vec![
            ToolCall {
                id: "tc_1".into(),
                name: "search_catalog".into(),
                arguments: serde_json::json!({"query": "orders"}),
            },
        ];
        let msg = Message::assistant_with_tool_calls("Investigating.", tool_calls);
        let json = serde_json::to_string(&msg).unwrap();
        let restored: Message = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.role, MessageRole::Assistant);
        assert_eq!(restored.content, "Investigating.");
        let tc = restored.tool_calls.unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].id, "tc_1");
        assert_eq!(tc[0].name, "search_catalog");
        assert_eq!(tc[0].arguments["query"], "orders");
    }

    #[test]
    fn agent_message_roundtrip_tool_result() {
        let msg = Message::tool_result("tc_abc", "query_datasource", r#"{"rows": [{"id": 1}]}"#);
        let json = serde_json::to_string(&msg).unwrap();
        let restored: Message = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.role, MessageRole::Tool);
        assert_eq!(restored.tool_call_id.as_deref(), Some("tc_abc"));
        assert_eq!(restored.name.as_deref(), Some("query_datasource"));
        assert_eq!(restored.content, r#"{"rows": [{"id": 1}]}"#);
    }

    // -- Contract: Tool call deserialization from DB JSON format -------------

    #[test]
    fn tool_call_deserialization_from_db_format() {
        // DB stores tool_calls as a JSON array.
        let db_json = serde_json::json!([
            {
                "id": "toolu_abc123",
                "name": "search_catalog",
                "arguments": {"query": "revenue", "datasource": "prod-pg"}
            }
        ]);
        let tool_calls: Vec<ToolCall> =
            serde_json::from_value(db_json).expect("should deserialize tool_calls from DB format");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "toolu_abc123");
        assert_eq!(tool_calls[0].arguments["query"], "revenue");
    }

    #[test]
    fn tool_call_deserialization_invalid_json_defaults_to_empty() {
        // When tool_calls JSON is malformed, serde_json::from_value returns
        // an error, and the code falls back to unwrap_or_default().
        let bad_json = serde_json::json!("not an array");
        let tool_calls: Vec<ToolCall> = serde_json::from_value(bad_json).unwrap_or_default();
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn tool_call_deserialization_partial_fields() {
        // Missing optional-like fields should still deserialize (arguments can be null).
        let json = serde_json::json!([
            {"id": "tc_1", "name": "list_datasources", "arguments": null}
        ]);
        let tool_calls: Vec<ToolCall> = serde_json::from_value(json).unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "list_datasources");
        assert!(tool_calls[0].arguments.is_null());
    }

    // -- Contract: db_message_to_agent_message different role values ---------

    #[test]
    fn db_message_to_agent_message_capitalized_role_fallback() {
        // Role values that don't match lowercase fall back to user.
        let msg = chat_service::AgentMessage {
            message_id: "m12".into(),
            role: "Assistant".into(), // capital A
            content: "response".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        };
        let result = db_message_to_agent_message(&msg);
        // Falls back to user since "Assistant" != "assistant".
        assert_eq!(result.role, MessageRole::User);
    }

    #[test]
    fn db_message_to_agent_message_function_role_fallback() {
        // "function" is not a valid role, falls back to user.
        let msg = chat_service::AgentMessage {
            message_id: "m13".into(),
            role: "function".into(),
            content: "some content".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::User);
    }

    // -- Note: Adapter integration contracts --------------------------------
    //
    // The following contracts require a real DB/Redis connection and are
    // covered by integration tests rather than unit tests:
    //
    // - `ChatAgentAdapter::persist_after_chat` when `session_id` is None:
    //   should skip persistence entirely and log a warning.
    //
    // - `ChatAgentAdapter::inject_knowledge_context` deduplication: handled
    //   inside `kyomi_knowledge::context::retrieve_and_inject`, which tracks
    //   previously injected items per session via Redis.
    //
    // - `ChatAgentAdapter::load_context` restores full conversation history
    //   including tool_calls and tool_results from the DB.
}
