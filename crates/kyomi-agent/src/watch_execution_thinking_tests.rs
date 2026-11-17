// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-976: a watch run's thinking trace must land on the assistant row the
//! run actually saves, and its full reasoning text must be retrievable.
//!
//! These drive the real `execute_agent_chat` with the watch persistence
//! config over real HTTP to a local OpenAI-compatible fake, then read the
//! result back through the same `chat_service` functions the log viewer uses.

use std::sync::Arc;

use super::*;
use crate::test_support::{seed_user_and_workspace, spawn_openai_fake, test_pool};
use kyomi_test_tracing::capture_tracing;

const KEY: [u8; 32] = [0u8; 32];

fn long_reasoning() -> String {
    // Over the tracker's 200-char display limit so a full-text row is stashed.
    format!(
        "Comparing the weekly revenue series against the trailing average. {}",
        "The deviation is within tolerance and no alert is warranted. ".repeat(8)
    )
    .trim_end()
    .to_string()
}

async fn fake_llm() -> String {
    spawn_openai_fake(serde_json::json!({
        "model": "fake-model",
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "{\"should_alert\": false, \"reason\": \"ok\"}",
                "reasoning_content": long_reasoning(),
            },
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5},
    }))
    .await
}

struct Run {
    db: kyomi_core::DbPool,
    session_id: String,
    result: crate::execution::AgentExecutionResult,
}

async fn run_watch_turn(config_for: impl FnOnce(&str) -> AgentExecutionConfig) -> Run {
    let db = test_pool().await;
    seed_user_and_workspace(&db).await;
    let session_id = uuid::Uuid::new_v4().to_string();
    chat_service::create_session_with_id(
        &db,
        "user-a",
        "ws-1",
        &session_id,
        Some("Watch: t"),
        "watch_execution",
        None,
    )
    .await
    .expect("create watch session");

    let mut app_config = kyomi_core::Config::test_config();
    app_config.self_hosted = false;
    app_config.llm_provider = Some("openai".into());
    app_config.llm_api_key = Some("test-key".into());
    app_config.llm_model = Some("fake-model".into());
    app_config.llm_base_url = Some(fake_llm().await);
    let app_config = Arc::new(app_config);

    let kv = kyomi_core::kv_store_memory::InMemoryKVStore::new_pool();
    let embedding = kyomi_embed::LazyEmbedding::new();
    let ws_manager = kyomi_auth::websocket::WebSocketManager::new(None, db.clone());
    let key = Arc::new(KEY);
    let env = crate::execution::AgentExecutionEnv {
        db: &db,
        kv: &kv,
        encryption_key: &key,
        embedding: &embedding,
        ws_manager: &ws_manager,
        app_config: &app_config,
        connect_registry: None,
        platforms: Arc::new(kyomi_core::platform::PlatformRegistry::new()),
    };
    let result = crate::execution::execute_agent_chat(config_for(&session_id), env)
        .await
        .expect("watch turn completes");
    Run { db, session_id, result }
}

fn watch_config(session_id: &str) -> AgentExecutionConfig {
    build_watch_retry_config(
        session_id,
        "user-a",
        "ws-1",
        "You are a watch.".to_string(),
        Vec::new(),
        "Check revenue.".to_string(),
    )
}

/// Test 1: the assistant row the watch saves carries the thinking events and
/// is the row `AgentExecutionResult.assistant_message_id` names.
#[tokio::test]
async fn watch_turn_attaches_thinking_events_to_the_saved_assistant_row() {
    let run = run_watch_turn(watch_config).await;

    let messages = chat_service::get_session_messages(&run.db, &KEY, &run.session_id, 100)
        .await
        .expect("load session messages");
    let assistant = messages
        .iter()
        .find(|m| m.message_type == "assistant")
        .expect("assistant row was saved");

    assert_eq!(
        assistant.message_id, run.result.assistant_message_id,
        "the saved assistant row must carry the id the run reports"
    );
    assert!(
        !assistant.thinking_events.is_empty(),
        "the assistant row's thinking_events must not be empty"
    );
}

/// Test 2: the full reasoning text is stored against that same message id
/// and comes back through `get_thinking_event_detail`; no store warning.
#[tokio::test]
async fn watch_turn_stores_full_reasoning_text_retrievable_by_message_id() {
    let logs = capture_tracing();
    let run = run_watch_turn(watch_config).await;

    let messages = chat_service::get_session_messages(&run.db, &KEY, &run.session_id, 100)
        .await
        .expect("load session messages");
    let assistant = messages
        .iter()
        .find(|m| m.message_type == "assistant")
        .expect("assistant row was saved");
    let event = assistant
        .thinking_events
        .iter()
        .find(|e| e.get("has_full_text").and_then(|v| v.as_bool()) == Some(true))
        .expect("a reasoning event with full text");
    let event_id = event["event_id"].as_str().expect("event_id");

    let full = chat_service::get_thinking_event_detail(
        &run.db,
        &KEY,
        &assistant.message_id,
        event_id,
        "user-a",
        "ws-1",
    )
    .await
    .expect("detail lookup")
    .expect("full text is stored for the saved message id");
    assert_eq!(full, long_reasoning());

    assert!(
        !logs.has_message_containing(tracing::Level::WARN, "Failed to store thinking event details"),
        "no store-details warning may be logged: {:?}",
        logs.events()
    );
}

/// Test 5: when the terminal `update_message` matches no row, the failure is
/// logged at error level with the session and message ids instead of being
/// swallowed. A `CallerPreInserted` id whose placeholder row was never
/// written is exactly "no row carries this id".
#[tokio::test]
async fn terminal_update_matching_no_row_is_logged_at_error_level() {
    let logs = capture_tracing();
    let missing_id = uuid::Uuid::new_v4().to_string();
    let run = run_watch_turn(|session_id| {
        let mut config = watch_config(session_id);
        config.assistant_message_persistence =
            AssistantMessagePersistence::CallerPreInserted(missing_id.clone());
        config
    })
    .await;

    assert_eq!(run.result.assistant_message_id, missing_id);
    let errors = logs.events_at(tracing::Level::ERROR);
    assert!(
        errors.iter().any(|(_, msg)| msg.contains("matched no row")
            && msg.contains(&run.session_id)
            && msg.contains(&missing_id)),
        "an unmatched terminal update must be logged at error level with both ids: {errors:?}"
    );
}
