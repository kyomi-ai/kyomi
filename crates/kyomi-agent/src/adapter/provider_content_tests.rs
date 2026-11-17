// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-969: storing the provider's verbatim server-tool blocks with a message and
//! replaying them on a later user turn.

use super::tests::{agent_message, claim_durable_turn, dispatch_chat_turn};
use super::*;

/// Replies to the first call with a captured provider response and to later
/// calls with fixed text, recording every request's messages.
struct CapturedThenText {
    first: std::sync::Mutex<Option<crate::types::LLMResponse>>,
    seen: Arc<std::sync::Mutex<Vec<Vec<Message>>>>,
}

#[async_trait::async_trait]
impl crate::provider::LLMProvider for CapturedThenText {
    fn model(&self) -> &str {
        "claude-test"
    }

    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[crate::types::Tool],
        _options: &crate::provider::RequestOptions,
        _user_names: &std::collections::HashMap<String, String>,
    ) -> kyomi_core::Result<crate::types::LLMResponse> {
        self.seen.lock().unwrap().push(messages.to_vec());
        if let Some(response) = self.first.lock().unwrap().take() {
            return Ok(response);
        }
        Ok(crate::types::LLMResponse {
            raw_response: serde_json::Value::Null,
            content: "second answer".into(),
            finish_reason: "end_turn".into(),
            usage: crate::types::AgentTokenUsage::default(),
            tool_calls: None,
            cost: None,
            thinking_content: None,
        })
    }
}

fn anthropic_server_tool_config() -> crate::agent::AgentConfig {
    crate::agent::AgentConfig {
        server_tools: vec![crate::web::server_tools::ServerTool {
            kind: kyomi_core::web_tools::WebToolKind::Search,
            backend: kyomi_core::web_tools::WebBackendKind::Anthropic,
            entry: serde_json::json!({"type": "web_search_20250305", "name": "web_search", "max_uses": 5}),
        }],
        ..Default::default()
    }
}

fn adapter_with_server_tools(
    db: &kyomi_core::DbPool,
    session_id: &str,
    key: &Arc<[u8; 32]>,
    provider: CapturedThenText,
) -> ChatAgentAdapter {
    let mut ctx = crate::test_support::build_ctx(db.clone());
    ctx.session_id = Some(session_id.to_string());
    let agent = CustomAgent::new(
        Box::new(provider),
        anthropic_server_tool_config(),
        Arc::new(crate::tools::ToolRegistry::new()),
        ctx,
        std::collections::HashMap::new(),
    );
    ChatAgentAdapter::new(agent, "user-a".to_string(), "ws-1".to_string(), Some(session_id.to_string()),
        "custom_agent".to_string(), db.clone(), key.clone())
}

/// The reply the provider actually returned for a web-search turn (live capture).
fn captured_search_message() -> serde_json::Value {
    serde_json::from_str(include_str!("../web/fixtures/anthropic_search_message.json")).expect("fixture")
}

/// KYO-969: a later user turn sends the provider's stored server_tool_use /
/// web_search_tool_result blocks back with their encrypted fields unchanged,
/// through the production durable path (claim, fenced write, claim again).
struct TwoTurns {
    seen: Arc<std::sync::Mutex<Vec<Vec<Message>>>>,
    captured: serde_json::Value,
    second: Result<String, kyomi_core::Error>,
}

/// Turn one searches (live capture) and is stored; turn two is a new durable run
/// that reads history only from the database.
async fn run_two_turns() -> TwoTurns {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    let key = Arc::new([7u8; 32]);
    let session_id = uuid::Uuid::new_v4().to_string();
    let captured = captured_search_message();
    let live = crate::anthropic::AnthropicClient::parse_response("claude", &captured).expect("parse capture");
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));

    let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
    let mut first = adapter_with_server_tools(&db, &session_id, &key,
        CapturedThenText { first: std::sync::Mutex::new(Some(live)), seen: seen.clone() });
    let answer = first.chat(ChatParams {
        message: "what is new?", cancel_token: CancellationToken::new(), current_time_user_tz: None,
        message_source: Some("web"), user_id: Some("user-a"), user_message_persistence: &user,
        assistant_message_persistence: &assistant,
    }).await.expect("turn one");
    let run = assistant.durable_run().expect("durable");
    let store = kyomi_auth::conversation_events::ConversationStore::new(&db, &key, "user-a", "ws-1");
    store.finish(&run.conversation_id, &run.run_id, &run.lease, chrono::Utc::now().timestamp_millis(),
        agent_runtime::RunState::Completed, &answer).await.expect("finish turn one");

    let accepted = chat_service::prepare_chat_dispatch(chat_service::ChatDispatchParams {
        db: &db, encryption_key: &key, ws_manager: None, user_id: "user-a", workspace_id: "ws-1", user_display_name: "Test",
        session_id: &session_id, is_new_session: false, message: "and who wrote it?", current_time_user_tz: None,
        message_source: Some("web"), skip_ai: false, client_msg_id: Some("turn-two"), owner_instance: "test", execution_context: None,
    }).await.expect("accept turn two");
    let chat_service::ChatDispatchOutcome::Ready { run_id, user_message_id, assistant_message_id, .. } = accepted else { panic!("ready") };
    let run_id = agent_runtime::RunId(run_id);
    let claim = store.claim(&run.conversation_id, &run_id, "second-worker", chrono::Utc::now().timestamp_millis(), 30_000)
        .await.expect("claim turn two");
    let user2 = UserMessagePersistence::CallerPersisted(user_message_id);
    let assistant2 = AssistantMessagePersistence::Durable {
        message_id: assistant_message_id,
        run: DurableRun { conversation_id: run.conversation_id.clone(), run_id, lease: claim.snapshot.lease.expect("lease"), context: Arc::new(claim.context) },
    };
    let mut second = adapter_with_server_tools(&db, &session_id, &key,
        CapturedThenText { first: std::sync::Mutex::new(None), seen: seen.clone() });
    let second = second.chat(ChatParams {
        message: "and who wrote it?", cancel_token: CancellationToken::new(), current_time_user_tz: None,
        message_source: Some("web"), user_id: Some("user-a"), user_message_persistence: &user2,
        assistant_message_persistence: &assistant2,
    }).await;
    TwoTurns { seen, captured, second }
}

/// KYO-969: a later user turn sends the provider's stored server_tool_use /
/// web_search_tool_result blocks back with their encrypted fields unchanged,
/// through the production durable path (claim, fenced write, claim again).
#[tokio::test]
async fn a_later_user_turn_resends_the_stored_server_blocks_verbatim() {
    let turns = run_two_turns().await;
    turns.second.as_ref().expect("turn two");
    let blocks = turns.captured["content"].as_array().unwrap().clone();
    let request = turns.seen.lock().unwrap().last().cloned().expect("turn two request");
    let replayed = request.iter()
        .find(|m| m.role == MessageRole::Assistant && m.provider_content.is_some())
        .expect("the earlier server-tool turn keeps its provider blocks");
    assert_eq!(replayed.provider_content.as_ref().unwrap(), &blocks, "equal in value, encrypted fields included");
    let (_, wire) = crate::anthropic::AnthropicClient::convert_messages_to_anthropic(&request, &std::collections::HashMap::new());
    let sent = wire.iter().find(|m| m["role"] == "assistant").expect("assistant on the wire");
    assert_eq!(sent["content"], turns.captured["content"], "what Anthropic receives is what it returned");
    let result_block = blocks.iter().find(|b| b["type"] == "web_search_tool_result").expect("search result block");
    assert_eq!(sent["content"][1]["content"][0]["encrypted_content"], result_block["content"][0]["encrypted_content"]);
}

/// KYO-969: a provider-executed search is recorded as a tool event carrying the
/// normalised sources, in the shape the UI's existing source card reads, so the
/// answer needs no sources footer in a run that has events.
#[tokio::test]
async fn a_provider_executed_search_is_recorded_as_a_source_card_event_and_replaces_the_footer() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    let key = Arc::new([7u8; 32]);
    let session_id = uuid::Uuid::new_v4().to_string();
    let captured = captured_search_message();
    let live = crate::anthropic::AnthropicClient::parse_response("claude", &captured).expect("parse capture");
    let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
    let mut adapter = adapter_with_server_tools(&db, &session_id, &key,
        CapturedThenText { first: std::sync::Mutex::new(Some(live)), seen: Arc::new(std::sync::Mutex::new(Vec::new())) });
    let answer = adapter.chat(ChatParams {
        message: "what is new?", cancel_token: CancellationToken::new(), current_time_user_tz: None,
        message_source: Some("web"), user_id: Some("user-a"), user_message_persistence: &user,
        assistant_message_persistence: &assistant,
    }).await.expect("turn");
    assert!(!answer.contains("**Sources**"), "the card replaces the footer when events are recorded: {answer}");

    let run = assistant.durable_run().expect("durable");
    let store = kyomi_auth::conversation_events::ConversationStore::new(&db, &key, "user-a", "ws-1");
    let batch = store.replay(&run.conversation_id, 0, agent_runtime::MAX_REPLAY_EVENTS).await.expect("replay events");
    let outcome = batch.events.iter().find_map(|event| match &event.payload {
        agent_runtime::PublicPayload::ToolOutcome { tool_call_id, text, .. } => Some((tool_call_id.as_str().to_string(), text.preview.clone())),
        _ => None,
    }).expect("a recorded web call");
    let provider_id = captured["content"][0]["id"].as_str().unwrap();
    assert_eq!(outcome.0, provider_id, "the provider's own call id");
    let shown: serde_json::Value = serde_json::from_str(&outcome.1).expect("the card payload is JSON");
    assert_eq!(shown["tool"], "web_search");
    assert_eq!(shown["query"], captured["content"][0]["input"]["query"]);
    let first = &shown["results"][0];
    let expected = &captured["content"][1]["content"][0];
    assert_eq!(first["url"], expected["url"]);
    assert!(first["title"].as_str().unwrap().contains(expected["title"].as_str().unwrap()), "title fenced like a Kyomi-executed result");
}

#[tokio::test]
async fn blocks_stored_through_the_legacy_insert_and_placeholder_paths_reload_too() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    let key = Arc::new([7u8; 32]);
    let session_id = uuid::Uuid::new_v4().to_string();
    let (user, assistant) = dispatch_chat_turn(&db, &key, &session_id, "what is new?").await;
    let blocks = captured_search_message()["content"].as_array().unwrap().clone();
    let intermediate = Message::assistant_with_tool_calls("looking", vec![ToolCall {
        id: "call-1".into(), name: "noop".into(), arguments: serde_json::json!({}), arguments_error: None,
    }]).with_provider_content(Some(blocks.clone()));
    let mut finalmsg = Message::assistant("done").with_provider_content(Some(blocks.clone()));
    finalmsg.message_id = assistant.tag_id().map(str::to_string);
    for (index, msg) in [intermediate, finalmsg].iter().enumerate() {
        persist_one_new_message(&db, &key, &session_id, msg, user.caller_persisted_id(), assistant.tag_id(),
            MessagePersistenceScope { durable_run: None, user_id: "user-a", workspace_id: "ws-1", message_index: index })
            .await.expect("persist");
    }
    let history = chat_service::get_agent_messages(&db, &key, &session_id, None).await.expect("history");
    let with_blocks = history.iter().filter(|m| m.provider_content.is_some()).count();
    assert_eq!(with_blocks, 2, "INSERT and placeholder-finalize paths both store the blocks");
    for m in history.iter().filter(|m| m.provider_content.is_some()) {
        assert_eq!(db_message_to_agent_message(m).provider_content.as_ref(), Some(&blocks));
    }
}

#[test]
fn stored_blocks_are_replayed_only_for_the_anthropic_format_and_when_present() {
    let blocks = captured_search_message()["content"].clone();
    let row = |backend: Option<&str>, content: Option<serde_json::Value>| chat_service::AgentMessage {
        provider_backend: backend.map(str::to_string),
        provider_content: content,
        ..agent_message("m", "assistant", "text")
    };
    assert!(db_message_to_agent_message(&row(Some("anthropic"), Some(blocks.clone()))).provider_content.is_some());
    assert!(db_message_to_agent_message(&row(Some("gemini"), Some(blocks.clone()))).provider_content.is_none(), "another format replays text");
    assert!(db_message_to_agent_message(&row(None, None)).provider_content.is_none(), "old sessions replay text");
    assert!(db_message_to_agent_message(&row(Some("anthropic"), Some(serde_json::json!({"not": "an array"})))).provider_content.is_none());
}

#[tokio::test]
async fn deleting_a_message_deletes_its_stored_blocks() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    let key = Arc::new([7u8; 32]);
    let session_id = uuid::Uuid::new_v4().to_string();
    let (user, assistant) = dispatch_chat_turn(&db, &key, &session_id, "q").await;
    let mut msg = Message::assistant("a").with_provider_content(Some(captured_search_message()["content"].as_array().unwrap().clone()));
    msg.message_id = assistant.tag_id().map(str::to_string);
    persist_one_new_message(&db, &key, &session_id, &msg, user.caller_persisted_id(), assistant.tag_id(),
        MessagePersistenceScope { durable_run: None, user_id: "user-a", workspace_id: "ws-1", message_index: 1 })
        .await.expect("persist");
    let id = assistant.tag_id().unwrap().to_string();
    let count = |db: &kyomi_core::DbPool| {
        let id = id.clone();
        let db = db.clone();
        async move { kyomi_core::db_fetch_scalar!(&db, i64, "SELECT COUNT(*) FROM chat_message_provider_content WHERE message_id=$1", &id).unwrap() }
    };
    assert_eq!(count(&db).await, 1);
    kyomi_core::db_execute!(&db, "DELETE FROM chat_messages WHERE message_id=$1", &id).expect("delete message");
    assert_eq!(count(&db).await, 0, "cascade keeps blocks aligned with their message");
}


async fn stored_rows(db: &kyomi_core::DbPool, message_id: &str) -> i64 {
    kyomi_core::db_fetch_scalar!(db, i64, "SELECT COUNT(*) FROM chat_message_provider_content WHERE message_id=$1", message_id).unwrap()
}

/// A user edit rewrites the text; the old blocks must not override it on replay.
#[tokio::test]
async fn editing_a_message_drops_its_stored_blocks() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    let key = Arc::new([7u8; 32]);
    let session_id = uuid::Uuid::new_v4().to_string();
    let (user, assistant) = dispatch_chat_turn(&db, &key, &session_id, "q").await;
    let mut msg = Message::assistant("original").with_provider_content(Some(captured_search_message()["content"].as_array().unwrap().clone()));
    msg.message_id = assistant.tag_id().map(str::to_string);
    persist_one_new_message(&db, &key, &session_id, &msg, user.caller_persisted_id(), assistant.tag_id(),
        MessagePersistenceScope { durable_run: None, user_id: "user-a", workspace_id: "ws-1", message_index: 1 })
        .await.expect("persist");
    let id = assistant.tag_id().unwrap().to_string();
    assert_eq!(stored_rows(&db, &id).await, 1);
    chat_service::update_message_content_owned(&db, &key, "user-a", "ws-1", &session_id, &id, "edited").await.expect("edit");
    assert_eq!(stored_rows(&db, &id).await, 0);
    let history = chat_service::get_agent_messages(&db, &key, &session_id, None).await.unwrap();
    let edited = history.iter().find(|m| m.message_id == id).unwrap();
    assert_eq!(edited.content, "edited");
    assert!(edited.provider_content.is_none(), "the edit, not the old blocks, is what replays");
}

/// If the run fails after the blocks were stored with the placeholder, the
/// failure text becomes the message and the blocks go with the old text.
#[tokio::test]
async fn a_failed_run_does_not_keep_blocks_for_the_replaced_text() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    let key = Arc::new([7u8; 32]);
    let session_id = uuid::Uuid::new_v4().to_string();
    let live = crate::anthropic::AnthropicClient::parse_response("claude", &captured_search_message()).expect("parse");
    let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut adapter = adapter_with_server_tools(&db, &session_id, &key, CapturedThenText { first: std::sync::Mutex::new(Some(live)), seen });
    adapter.chat(ChatParams {
        message: "what is new?", cancel_token: CancellationToken::new(), current_time_user_tz: None,
        message_source: Some("web"), user_id: Some("user-a"), user_message_persistence: &user,
        assistant_message_persistence: &assistant,
    }).await.expect("turn");
    let id = assistant.tag_id().unwrap().to_string();
    assert_eq!(stored_rows(&db, &id).await, 1, "stored with the placeholder");
    let run = assistant.durable_run().unwrap();
    kyomi_auth::conversation_events::ConversationStore::new(&db, &key, "user-a", "ws-1")
        .finish(&run.conversation_id, &run.run_id, &run.lease, chrono::Utc::now().timestamp_millis(), agent_runtime::RunState::Failed, "it failed")
        .await.expect("fail the run");
    assert_eq!(stored_rows(&db, &id).await, 0);
}

/// The placeholder is gone or was not written: no blocks, no error.
#[tokio::test]
async fn blocks_are_not_stored_when_the_placeholder_text_was_not_written() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    let key = Arc::new([7u8; 32]);
    let session_id = uuid::Uuid::new_v4().to_string();
    let (user, assistant) = dispatch_chat_turn(&db, &key, &session_id, "q").await;
    let id = assistant.tag_id().unwrap().to_string();
    kyomi_core::db_execute!(&db, "DELETE FROM chat_messages WHERE message_id=$1", &id).unwrap();
    let mut msg = Message::assistant("a").with_provider_content(Some(captured_search_message()["content"].as_array().unwrap().clone()));
    msg.message_id = Some(id.clone());
    persist_one_new_message(&db, &key, &session_id, &msg, user.caller_persisted_id(), assistant.tag_id(),
        MessagePersistenceScope { durable_run: None, user_id: "user-a", workspace_id: "ws-1", message_index: 1 })
        .await.expect("a missing placeholder is not an error here");
    assert_eq!(stored_rows(&db, &id).await, 0);
}

/// Legacy (non-durable) failure path: the error text written to the placeholder
/// replaces the answer the blocks belonged to; a successful completion keeps them.
#[tokio::test]
async fn the_legacy_error_text_write_drops_blocks_but_completion_keeps_them() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    let key = Arc::new([7u8; 32]);
    let session_id = uuid::Uuid::new_v4().to_string();
    let (user, assistant) = dispatch_chat_turn(&db, &key, &session_id, "q").await;
    let mut msg = Message::assistant("answer").with_provider_content(Some(captured_search_message()["content"].as_array().unwrap().clone()));
    msg.message_id = assistant.tag_id().map(str::to_string);
    persist_one_new_message(&db, &key, &session_id, &msg, user.caller_persisted_id(), assistant.tag_id(),
        MessagePersistenceScope { durable_run: None, user_id: "user-a", workspace_id: "ws-1", message_index: 1 })
        .await.expect("persist");
    let id = assistant.tag_id().unwrap().to_string();
    chat_service::update_message(&db, &key, &id, Some("answer"), None, Some(chat_service::MessageStatus::Complete)).await.unwrap();
    assert_eq!(stored_rows(&db, &id).await, 1, "completion with the same text keeps the blocks");
    chat_service::update_message(&db, &key, &id, Some("it failed"), None, Some(chat_service::MessageStatus::Error)).await.unwrap();
    assert_eq!(stored_rows(&db, &id).await, 0, "an error notice replaces the text, so the blocks go");
}
