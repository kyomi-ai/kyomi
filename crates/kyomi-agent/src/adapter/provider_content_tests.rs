// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-969: storing the provider's verbatim server-tool blocks with a message and
//! replaying them on a later user turn.

use super::tests::{agent_message, claim_durable_turn, dispatch_chat_turn};
use super::*;

/// Replies with captured provider responses in order, then with fixed text,
/// recording every request's messages.
struct CapturedThenText {
    script: std::sync::Mutex<std::collections::VecDeque<crate::types::LLMResponse>>,
    /// Fail instead of answering with text once the script is used up.
    fail_when_exhausted: bool,
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
        if let Some(response) = self.script.lock().unwrap().pop_front() {
            return Ok(response);
        }
        if self.fail_when_exhausted {
            return Err(kyomi_core::Error::ServiceUnavailable("provider down".into()));
        }
        Ok(crate::types::LLMResponse {
            raw_response: serde_json::Value::Null,
            content: "second answer".into(),
            finish_reason: "end_turn".into(),
            usage: crate::types::AgentTokenUsage::default(),
            tool_calls: None,
            cost: None,
            thinking_content: None,
            call_detail: None,
        })
    }
}

fn anthropic_server_tool_config() -> crate::agent::AgentConfig {
    crate::agent::AgentConfig {
        server_tools: vec![
            crate::web::server_tools::ServerTool {
                kind: kyomi_core::web_tools::WebToolKind::Search,
                backend: kyomi_core::web_tools::WebBackendKind::Anthropic,
                entry: serde_json::json!({"type": "web_search_20250305", "name": "web_search", "max_uses": 5}),
            },
            crate::web::server_tools::ServerTool {
                kind: kyomi_core::web_tools::WebToolKind::Fetch,
                backend: kyomi_core::web_tools::WebBackendKind::Anthropic,
                entry: serde_json::json!({"type": "web_fetch_20250910", "name": "web_fetch", "max_uses": 5}),
            },
        ],
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

/// The reply the provider actually returned for a web-fetch turn (live capture).
fn captured_fetch_message() -> serde_json::Value {
    serde_json::from_str(include_str!("../web/fixtures/anthropic_fetch_message.json")).expect("fixture")
}

/// The captured fetch reply with a client `tool_use` block appended, the shape
/// production gets when the model fetches a page and asks Kyomi to run tools in
/// the same response (KYO-979). The provider blocks are untouched.
fn fetch_alongside_client_tool_calls() -> serde_json::Value {
    let mut raw = captured_fetch_message();
    raw["stop_reason"] = serde_json::json!("tool_use");
    raw["content"].as_array_mut().expect("content").push(serde_json::json!({
        "type": "tool_use", "id": "toolu_kyo979", "name": "list_datasources", "input": {}
    }));
    raw
}

struct TwoTurns {
    db: kyomi_core::DbPool,
    session_id: String,
    seen: Arc<std::sync::Mutex<Vec<Vec<Message>>>>,
    captured: serde_json::Value,
    first: Result<String, kyomi_core::Error>,
    /// `None` when turn one failed and no second turn was run.
    second: Option<Result<String, kyomi_core::Error>>,
}

/// Turn one searches (live capture) and is stored; turn two is a new durable run
/// that reads history only from the database.
async fn run_two_turns() -> TwoTurns {
    let captured = captured_search_message();
    let live = crate::anthropic::AnthropicClient::parse_response("claude", &captured, None).expect("parse capture");
    let mut turns = run_turns(vec![live], false).await;
    turns.captured = captured;
    turns
}

/// Turn one is answered by `script` (provider responses, in order); if it
/// succeeds, turn two is a new durable run that reads history only from the
/// database.
async fn run_turns(script: Vec<crate::types::LLMResponse>, fail_when_exhausted: bool) -> TwoTurns {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    let key = Arc::new([7u8; 32]);
    let session_id = uuid::Uuid::new_v4().to_string();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));

    let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
    let mut first = adapter_with_server_tools(&db, &session_id, &key,
        CapturedThenText { script: std::sync::Mutex::new(script.into()), fail_when_exhausted, seen: seen.clone() });
    let first = first.chat(ChatParams {
        message: "what is new?", cancel_token: CancellationToken::new(), current_time_user_tz: None,
        message_source: Some("web"), user_id: Some("user-a"), user_message_persistence: &user,
        assistant_message_persistence: &assistant,
    }).await;
    let run = assistant.durable_run().expect("durable");
    let store = kyomi_auth::conversation_events::ConversationStore::new(&db, &key, "user-a", "ws-1");
    let answer = match &first {
        Ok(answer) => answer.clone(),
        Err(_) => {
            return TwoTurns { db, session_id, seen, captured: serde_json::Value::Null, first, second: None };
        }
    };
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
        run: DurableRun { conversation_id: run.conversation_id.clone(), run_id, lease: claim.snapshot.lease.expect("lease"), context: Arc::new(claim.context), billing: Default::default() },
    };
    let mut second = adapter_with_server_tools(&db, &session_id, &key,
        CapturedThenText { script: std::sync::Mutex::new(Default::default()), fail_when_exhausted: false, seen: seen.clone() });
    let second = second.chat(ChatParams {
        message: "and who wrote it?", cancel_token: CancellationToken::new(), current_time_user_tz: None,
        message_source: Some("web"), user_id: Some("user-a"), user_message_persistence: &user2,
        assistant_message_persistence: &assistant2,
    }).await;
    TwoTurns { db, session_id, seen, captured: serde_json::Value::Null, first: Ok(answer), second: Some(second) }
}

/// KYO-969: a later user turn sends the provider's stored server_tool_use /
/// web_search_tool_result blocks back with their encrypted fields unchanged,
/// through the production durable path (claim, fenced write, claim again).
#[tokio::test]
async fn a_later_user_turn_resends_the_stored_server_blocks_verbatim() {
    let turns = run_two_turns().await;
    turns.second.as_ref().expect("turn two ran").as_ref().expect("turn two");
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
    let live = crate::anthropic::AnthropicClient::parse_response("claude", &captured, None).expect("parse capture");
    let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
    let mut adapter = adapter_with_server_tools(&db, &session_id, &key,
        CapturedThenText { script: std::sync::Mutex::new(vec![live].into()), fail_when_exhausted: false, seen: Arc::new(std::sync::Mutex::new(Vec::new())) });
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

/// KYO-979: a fetch that arrives in the same response as the model's client tool
/// calls is still recorded as a source card, and the footer does not have to
/// stand in for it.
#[tokio::test]
async fn a_fetch_in_the_same_response_as_client_tool_calls_is_recorded_as_a_source_card() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    let key = Arc::new([7u8; 32]);
    let session_id = uuid::Uuid::new_v4().to_string();
    let captured = fetch_alongside_client_tool_calls();
    let live = crate::anthropic::AnthropicClient::parse_response("claude", &captured, None).expect("parse capture");
    let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
    let mut adapter = adapter_with_server_tools(&db, &session_id, &key,
        CapturedThenText { script: std::sync::Mutex::new(vec![live].into()), fail_when_exhausted: false, seen: Arc::new(std::sync::Mutex::new(Vec::new())) });
    let answer = adapter.chat(ChatParams {
        message: "look at kyomi.ai and list my datasources", cancel_token: CancellationToken::new(), current_time_user_tz: None,
        message_source: Some("web"), user_id: Some("user-a"), user_message_persistence: &user,
        assistant_message_persistence: &assistant,
    }).await.expect("turn");
    assert!(!answer.contains("**Sources**"), "the fetch card replaces its footer source: {answer}");

    let run = assistant.durable_run().expect("durable");
    let store = kyomi_auth::conversation_events::ConversationStore::new(&db, &key, "user-a", "ws-1");
    let batch = store.replay(&run.conversation_id, 0, agent_runtime::MAX_REPLAY_EVENTS).await.expect("replay events");
    let provider_id = captured["content"][0]["id"].as_str().unwrap();
    let outcome = batch.events.iter().find_map(|event| match &event.payload {
        agent_runtime::PublicPayload::ToolOutcome { tool_call_id, text, .. } if tool_call_id.as_str() == provider_id => Some(text.preview.clone()),
        _ => None,
    }).expect("the provider fetch is recorded even though the response also called a client tool");
    let shown: serde_json::Value = serde_json::from_str(&outcome).expect("the card payload is JSON");
    assert_eq!(shown["tool"], "web_fetch");
    assert_eq!(shown["url"], captured["content"][1]["content"]["url"]);
}

/// A live `pause_turn` (one fetch still pending, no result yet) and its live
/// continuation, which begins with that fetch's result block and does not repeat
/// the `server_tool_use` block — the provider's documented continuation shape.
/// A client tool call is added to the continuation, as production had (KYO-979).
fn paused_single_fetch_chain() -> (serde_json::Value, serde_json::Value) {
    let paused_live: serde_json::Value =
        serde_json::from_str(include_str!("../web/fixtures/anthropic_pause_turn_message.json")).expect("paused fixture");
    let continuation_live: serde_json::Value =
        serde_json::from_str(include_str!("../web/fixtures/anthropic_pause_turn_continuation_message.json")).expect("continuation fixture");
    let paused_content = paused_live["content"].as_array().expect("content");
    let pending = paused_content
        .iter()
        .find(|block| {
            block["type"] == "server_tool_use"
                && !paused_content.iter().any(|result| {
                    result["tool_use_id"] == block["id"]
                        && result["type"].as_str().is_some_and(|kind| kind.ends_with("_tool_result"))
                })
        })
        .expect("the paused call without a result")
        .clone();
    let pending_id = pending["id"].as_str().expect("id");
    let result = continuation_live["content"]
        .as_array()
        .expect("content")
        .iter()
        .find(|block| block["type"] == "web_fetch_tool_result" && block["tool_use_id"] == pending_id)
        .expect("the continuation's result for the paused call")
        .clone();
    let paused = serde_json::json!({
        "id": "msg_pause_kyo979", "type": "message", "role": "assistant",
        "stop_reason": "pause_turn",
        "content": [paused_live["content"][0].clone(), pending],
        "usage": {"input_tokens": 10, "output_tokens": 5, "server_tool_use": {"web_fetch_requests": 1}},
    });
    let continuation = serde_json::json!({
        "id": "msg_continuation_kyo979", "type": "message", "role": "assistant",
        "stop_reason": "tool_use",
        "content": [
            result,
            {"type": "text", "text": "The page is fetched."},
            {"type": "tool_use", "id": "toolu_kyo979", "name": "list_datasources", "input": {}}
        ],
        "usage": {"input_tokens": 20, "output_tokens": 10, "server_tool_use": {"web_fetch_requests": 1}},
    });
    (paused, continuation)
}

/// KYO-979: a fetch started in a `pause_turn` response and returned at the start
/// of its continuation — with the expected `tool_use_id` but no repeated
/// `server_tool_use` block — is still recorded as a source card, in a response
/// that also calls client tools. The footer must not have to stand in for it.
#[tokio::test]
async fn a_paused_fetch_result_in_a_continuation_with_client_tool_calls_is_recorded_as_a_source_card() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    let key = Arc::new([7u8; 32]);
    let session_id = uuid::Uuid::new_v4().to_string();
    let (paused, continuation) = paused_single_fetch_chain();
    let pending_url = paused["content"][1]["input"]["url"].as_str().expect("url").to_string();
    let provider_id = paused["content"][1]["id"].as_str().expect("id").to_string();
    let paused = crate::anthropic::AnthropicClient::parse_response("claude", &paused, None).expect("parse paused");
    let continuation = crate::anthropic::AnthropicClient::parse_response("claude", &continuation, None).expect("parse continuation");
    let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
    let mut adapter = adapter_with_server_tools(&db, &session_id, &key,
        CapturedThenText { script: std::sync::Mutex::new(vec![paused, continuation].into()), fail_when_exhausted: false, seen: Arc::new(std::sync::Mutex::new(Vec::new())) });

    // The live UI consumes exactly these frames.
    let manager = kyomi_auth::websocket::WebSocketManager::new(None, db.clone());
    let (_, mut receiver) = manager.connect("user-a").expect("connect");
    let tracker = Arc::new(tokio::sync::Mutex::new(crate::thinking::AgentThinkingTracker::new(
        crate::thinking::AgentThinkingTrackerConfig {
            workspace_id: "ws-1".into(), session_id: session_id.clone(), user_id: "user-a".into(),
            message_id: assistant.tag_id().expect("assistant id").into(), ws_manager: manager,
            workspace_user_ids: None, context_type: Some("chat".into()), context_window: 0,
            incremental_flush: None,
        },
    )));
    adapter.set_thinking_tracker(tracker);
    let answer = adapter.chat(ChatParams {
        message: "fetch n=7 and list my datasources", cancel_token: CancellationToken::new(), current_time_user_tz: None,
        message_source: Some("web"), user_id: Some("user-a"), user_message_persistence: &user,
        assistant_message_persistence: &assistant,
    }).await.expect("turn");
    assert!(!answer.contains("**Sources**"), "the fetch card replaces its footer source: {answer}");

    let mut cards = Vec::new();
    while let Ok(frame) = receiver.try_recv() {
        let message: kyomi_core::WebSocketMessage = serde_json::from_str(&frame).expect("frame");
        if message.message_type != kyomi_core::MessageType::AgentThinking { continue; }
        let event = message.data.as_ref().and_then(|data| data.get("event")).expect("thinking event");
        if event.get("event_type").and_then(|kind| kind.as_str()) != Some("tool_execution_end") { continue; }
        let schema = event.get("data").and_then(|data| data.get("schema")).expect("card schema");
        if schema.get("tool").and_then(|tool| tool.as_str()) == Some("web_fetch") {
            cards.push(schema.clone());
        }
    }
    assert_eq!(cards.len(), 1, "one web fetch card, streamed to the UI: {cards:?}");
    assert_eq!(cards[0]["input"]["url"], pending_url);
    assert_eq!(cards[0]["output"]["url"], pending_url);
    assert!(cards[0]["output"]["title"].as_str().expect("title").contains("httpbin.org/get?n=7"));

    let run = assistant.durable_run().expect("durable");
    let store = kyomi_auth::conversation_events::ConversationStore::new(&db, &key, "user-a", "ws-1");
    let batch = store.replay(&run.conversation_id, 0, agent_runtime::MAX_REPLAY_EVENTS).await.expect("replay events");
    assert!(batch.events.iter().any(|event| matches!(&event.payload,
        agent_runtime::PublicPayload::ToolOutcome { tool_call_id, .. } if tool_call_id.as_str() == provider_id)),
        "the card is recorded durably under the provider's call id");
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
fn a_stored_row_is_attached_exactly_and_a_message_without_one_has_none() {
    let blocks = captured_search_message()["content"].clone();
    let row = |content: Option<serde_json::Value>| chat_service::AgentMessage {
        provider_content: content,
        ..agent_message("m", "assistant", "text")
    };
    assert_eq!(db_message_to_agent_message(&row(Some(blocks.clone()))).provider_content.as_ref().map(|b| serde_json::Value::Array(b.clone())), Some(blocks));
    assert!(db_message_to_agent_message(&row(None)).provider_content.is_none(), "sessions older than this change have nothing to replay");
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
    let live = crate::anthropic::AnthropicClient::parse_response("claude", &captured_search_message(), None).expect("parse");
    let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut adapter = adapter_with_server_tools(&db, &session_id, &key, CapturedThenText { script: std::sync::Mutex::new(vec![live].into()), fail_when_exhausted: false, seen });
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

fn live_response(raw: &str) -> crate::types::LLMResponse {
    let value: serde_json::Value = serde_json::from_str(raw).expect("fixture");
    crate::anthropic::AnthropicClient::parse_response("claude", &value, None).expect("parse capture")
}

fn live_pause_chain() -> (crate::types::LLMResponse, crate::types::LLMResponse) {
    (
        live_response(include_str!("../web/fixtures/anthropic_pause_turn_message.json")),
        live_response(include_str!("../web/fixtures/anthropic_pause_turn_continuation_message.json")),
    )
}

/// The assistant blocks of a request, in order, as one list.
fn replayed_assistant_blocks(request: &[Message]) -> Vec<serde_json::Value> {
    request.iter()
        .filter(|m| m.role == MessageRole::Assistant)
        .filter_map(|m| m.provider_content.clone())
        .flatten()
        .collect()
}

/// A paused turn and its continuation (live captures; the provider accepts the two
/// together, 200 on 2026-11-17) are stored as the provider returned them and sent
/// back, in order, by a later user turn.
#[tokio::test]
async fn a_normal_pause_chain_replays_through_a_later_user_turn() {
    let (paused, continuation) = live_pause_chain();
    let expected: Vec<serde_json::Value> = paused.raw_response["content"].as_array().unwrap().iter()
        .chain(continuation.raw_response["content"].as_array().unwrap())
        .cloned()
        .collect();
    let turns = run_turns(vec![paused, continuation], false).await;
    turns.first.as_ref().expect("turn one finishes");
    turns.second.as_ref().expect("turn two ran").as_ref().expect("turn two");
    let request = turns.seen.lock().unwrap().last().cloned().expect("turn two request");
    assert_eq!(replayed_assistant_blocks(&request), expected, "paused blocks then the continuation's, exactly as received");
}

/// A bare pause message (blocks only) is never dropped: its blocks are stored
/// with the next row, in order. (Shell derived from the live capture by taking
/// the text out of the paused reply.)
#[tokio::test]
async fn a_bare_pause_message_is_stored_with_the_next_row_and_replayed_in_order() {
    let (mut paused, continuation) = live_pause_chain();
    let mut raw = paused.raw_response.clone();
    raw["content"].as_array_mut().unwrap().retain(|block| block["type"] != "text");
    paused.raw_response = raw;
    paused.content = String::new();
    let expected: Vec<serde_json::Value> = paused.raw_response["content"].as_array().unwrap().iter()
        .chain(continuation.raw_response["content"].as_array().unwrap())
        .cloned()
        .collect();
    let turns = run_turns(vec![paused, continuation], false).await;
    turns.second.as_ref().expect("turn two ran").as_ref().expect("turn two");
    let request = turns.seen.lock().unwrap().last().cloned().expect("turn two request");
    assert_eq!(replayed_assistant_blocks(&request), expected, "shell blocks first, then the continuation's");
    let assistant_rows = request.iter().filter(|m| m.role == MessageRole::Assistant).count();
    assert_eq!(assistant_rows, 1, "one stored row carries the whole logical answer");
}

/// A turn the provider keeps pausing past the bound fails like any failed run and
/// leaves nothing of its paused chain stored: no blocks, no partial text row.
#[tokio::test]
async fn a_run_that_hits_the_pause_bound_leaves_no_stored_rows_from_the_run() {
    let (paused, _) = live_pause_chain();
    let script = vec![paused; 20];
    let turns = run_turns(script, false).await;
    let error = turns.first.as_ref().expect_err("the run fails");
    assert!(error.to_string().contains("Web research did not finish"), "{error}");
    assert!(turns.second.is_none());
    assert_eq!(
        kyomi_core::db_fetch_scalar!(&turns.db, i64, "SELECT COUNT(*) FROM chat_message_provider_content").unwrap(),
        0,
        "no blocks stored"
    );
    let key = [7u8; 32];
    let history = chat_service::get_agent_messages(&turns.db, &key, &turns.session_id, None).await.unwrap();
    assert!(
        history.iter().all(|m| m.role != "assistant" || !m.content.contains("I'll fetch each URL")),
        "no partial text row from the paused chain: {:?}", history.iter().map(|m| (m.role.clone(), m.content.chars().take(30).collect::<String>())).collect::<Vec<_>>()
    );
}

/// A provider error inside a paused chain: the run fails and nothing of the chain
/// (a text-bearing paused row, its blocks) is stored.
#[tokio::test]
async fn a_provider_error_inside_a_pause_chain_leaves_no_stored_rows_from_the_run() {
    let (paused, _) = live_pause_chain();
    let turns = run_turns(vec![paused.clone(), paused], true).await;
    let error = turns.first.as_ref().expect_err("the run fails");
    assert!(error.to_string().contains("provider down"), "{error}");
    assert_eq!(
        kyomi_core::db_fetch_scalar!(&turns.db, i64, "SELECT COUNT(*) FROM chat_message_provider_content").unwrap(),
        0,
        "no blocks stored"
    );
    let key = [7u8; 32];
    let history = chat_service::get_agent_messages(&turns.db, &key, &turns.session_id, None).await.unwrap();
    assert!(history.iter().all(|m| m.role != "assistant" || !m.content.contains("I'll fetch each URL")), "no partial text row");
}
