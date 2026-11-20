// SPDX-License-Identifier: AGPL-3.0-or-later

//! Live Anthropic verification for KYO-975 (conversation compaction).
//! `#[ignore]`d: each run spends real money. Run with the key in the
//! environment only (never printed):
//!
//! ```text
//! ANTHROPIC_API_KEY="$(cat <key file>)" cargo test -p kyomi-agent --lib \
//!     compaction_live_tests -- --ignored --nocapture --test-threads=1
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::execution::{AgentExecutionConfig, AgentExecutionEnv, execute_agent_chat};
use crate::provider::{LLMProviderConfig, ProviderKind, RequestOptions, create_provider};
use crate::types::Message;
use kyomi_auth::{chat_service, websocket::WebSocketManager};
use kyomi_core::enums::WorkspaceRole;
use kyomi_core::{kv_store_memory::InMemoryKVStore, platform::PlatformRegistry};

const MODEL: &str = "claude-haiku-5-5";
/// Below the seeded history's estimate and above the compacted context plus
/// the fixed request prefix, so the run compacts once and then stays below it.
const THRESHOLD: u32 = 15_000;

fn key() -> String {
    std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY (live tests only)")
}

/// One real Anthropic web-search response's content blocks (server_tool_use +
/// web_search_tool_result with the provider's encrypted_content), so the stored
/// block replayed into the main and summary requests is byte-for-byte what the
/// provider returned.
async fn live_web_blocks() -> Vec<Value> {
    let client = create_provider(LLMProviderConfig {
        provider: ProviderKind::Anthropic,
        api_key: key(),
        model: Some(MODEL.into()),
        base_url: None,
        context_window: 0,
    })
    .expect("anthropic client");
    let options = RequestOptions::new(1_024).with_server_tools(vec![json!({
        "type": "web_search_20250305",
        "name": "web_search",
        "max_uses": 1
    })]);
    let mut messages = vec![
        Message::system("You are a search assistant. Use the web search tool once and quote one result."),
        Message::user("Search the web for a current fact about the Eiffel Tower and quote one sentence."),
    ];
    let mut blocks: Vec<Value> = Vec::new();
    let mut last = client
        .complete(&messages, &[], &options, &HashMap::new())
        .await
        .expect("live web search");
    blocks.extend(last.raw_response["content"].as_array().cloned().unwrap_or_default());
    for _ in 0..2 {
        if blocks.iter().any(|block| block["type"] == "web_search_tool_result") {
            break;
        }
        messages.push(
            Message::assistant(&last.content)
                .with_provider_content(last.raw_response["content"].as_array().cloned()),
        );
        last = client
            .complete(&messages, &[], &options, &HashMap::new())
            .await
            .expect("live web search continuation");
        blocks.extend(last.raw_response["content"].as_array().cloned().unwrap_or_default());
    }
    assert!(
        blocks.iter().any(|block| block["type"] == "web_search_tool_result"),
        "the live search returned a complete result: {blocks:?}"
    );
    blocks
}

fn config(session_id: &str, message: &str) -> AgentExecutionConfig {
    AgentExecutionConfig {
        session_id: session_id.to_string(),
        user_id: "user-a".to_string(),
        workspace_id: "ws-1".to_string(),
        message: message.to_string(),
        workspace_roles: vec![WorkspaceRole::WorkspaceAdmin],
        // A small fixed prefix keeps the threshold meaningful. The full tool
        // registry stays available so the summarised range can carry a client
        // tool pair and a provider web block with their matching definitions.
        system_prompt: Some(
            "You are a concise analytics assistant. Follow the user's instructions exactly."
                .to_string(),
        ),
        // Only the tool the scenario needs: the summary request must carry a
        // client tool_use/tool_result pair, but the full registry's schemas
        // would dominate a low-threshold live run.
        tools_subset: Some(vec!["get_workspace_info".to_string()]),
        max_iterations: 12,
        max_tokens: 4_096,
        component: "custom_agent".to_string(),
        ..AgentExecutionConfig::default()
    }
}

/// Seed the conversation: large revenue turns, one assistant turn carrying a
/// stored provider web-search block, and a client tool_use/tool_result pair.
/// The first eight messages are inside the summarised range.
async fn seed_history(
    db: &kyomi_core::DbPool,
    key: &[u8; 32],
    session_id: &str,
    web_blocks: &[Value],
) {
    let tool_calls = json!([{
        "id": "call_1",
        "name": "get_workspace_info",
        "arguments": {},
        "arguments_error": null
    }]);
    for i in 0..13u8 {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        let message_role = if i == 4 { "tool" } else { role };
        let large = i < 9;
        let content = if large {
            format!(
                "We are investigating the Q4 revenue shortfall (step {i}): {}",
                "x".repeat(12_000)
            )
        } else {
            format!("noted step {i}")
        };
        let tool_calls = (i == 3).then(|| tool_calls.clone());
        let tool_call_id = (i == 4).then_some("call_1");
        let tool_name = (i == 4).then_some("get_workspace_info");
        let content = if i == 4 {
            r#"{"workspace_name": "Workspace"}"#.to_string()
        } else {
            content
        };
        let provider_content = (i == 1).then(|| Value::Array(web_blocks.to_vec()));
        let message = chat_service::NewMessage {
            role: message_role,
            content: &content,
            metadata: None,
            message_id: None,
            current_time_user_tz: None,
            message_source: (message_role == "user").then_some("web"),
            sent_by_user_id: (message_role == "user").then_some("user-a"),
            tool_call_id,
            tool_name,
            tool_calls: tool_calls.as_ref(),
            status: chat_service::MessageStatus::Complete,
        };
        chat_service::insert_message(
            db,
            key,
            session_id,
            &message,
            provider_content.as_ref(),
        )
        .await
        .expect("seed history");
    }
}

/// Live scenario: the seeded conversation compacts once; the summary request
/// carries the stored client tool pair and web block with their definitions;
/// the summary + cut index persist encrypted; a second run restores and uses
/// the summary.
#[tokio::test]
#[ignore = "live: spends real Anthropic tokens"]
async fn compaction_compacts_persists_and_restores_on_a_real_conversation() {
    let db = crate::test_support::test_pool().await;
    crate::test_support::seed_user_and_workspace(&db).await;
    let encryption_key: Arc<[u8; 32]> = Arc::new([7u8; 32]);
    let session_id = format!("live-compaction-{}", chrono::Utc::now().timestamp());
    chat_service::create_session_with_id(&db, "user-a", "ws-1", &session_id, None, "chat", None)
        .await
        .expect("create session");
    let web_blocks = live_web_blocks().await;
    seed_history(&db, &encryption_key, &session_id, &web_blocks).await;

    let kv = InMemoryKVStore::new_pool();
    let embedding = kyomi_embed::LazyEmbedding::new();
    let ws_manager = WebSocketManager::new(None, db.clone());
    let mut app = kyomi_core::Config::test_config();
    app.self_hosted = false;
    app.llm_provider = Some("anthropic".to_string());
    app.llm_api_key = Some(key());
    app.llm_model = Some(MODEL.to_string());
    app.llm_title_model = Some(MODEL.to_string());
    app.llm_compaction.threshold_tokens = Some(THRESHOLD);
    // The Anthropic backend attaches the provider web-search tool to the run,
    // so the stored web block can be replayed into the summary request.
    app.web_tools.search.provider = Some("anthropic".to_string());
    let app_config = Arc::new(app);
    let platforms = Arc::new(PlatformRegistry::new());

    let run = |message: &str| {
        let config = config(&session_id, message);
        async {
            execute_agent_chat(
                config,
                AgentExecutionEnv {
                    db: &db,
                    kv: &kv,
                    encryption_key: &encryption_key,
                    embedding: &embedding,
                    ws_manager: &ws_manager,
                    app_config: &app_config,
                    connect_registry: None,
                    platforms: platforms.clone(),
                },
            )
            .await
        }
    };

    let capture = kyomi_test_tracing::capture_tracing();
    let first = run("Call the get_workspace_info tool once, then answer with the workspace name.")
        .await
        .expect("the first live turn compacts and completes");
    if first.status != "completed" {
        for (level, message) in capture.events() {
            eprintln!("CAPTURE {level} {message}");
        }
    }
    assert_eq!(first.status, "completed", "{:?}", first.error);
    assert!(
        first.response_text.to_lowercase().contains("workspace"),
        "{}",
        first.response_text
    );

    let resolved = capture
        .events_at(tracing::Level::INFO)
        .into_iter()
        .map(|(_, message)| message)
        .find(|message| message.contains("compaction threshold resolved"))
        .expect("the run logs the resolved window and threshold");
    for expected in [
        format!("model={MODEL}"),
        "context_window=1000000".to_string(),
        "context_window_source=\"provider\"".to_string(),
        format!("compaction_threshold={THRESHOLD}"),
        "compaction_threshold_source=\"instance_absolute\"".to_string(),
    ] {
        assert!(resolved.contains(&expected), "{expected} in {resolved}");
    }
    eprintln!("KYO-975 live: {resolved}");
    let compacted = capture
        .events_at(tracing::Level::INFO)
        .into_iter()
        .map(|(_, message)| message)
        .find(|message| message.contains("conversation compacted"))
        .expect("the run compacts at the threshold");
    eprintln!("KYO-975 live: {compacted}");
    let summary_log = capture
        .events_at(tracing::Level::INFO)
        .into_iter()
        .map(|(_, message)| message)
        .find(|message| message.contains("conversation compaction summary"))
        .expect("the summary call logs which model wrote it");
    assert!(
        summary_log.contains(&format!("summary_model=\"{MODEL}\"")),
        "{summary_log}"
    );
    assert!(summary_log.contains("summary_window=1000000"), "{summary_log}");
    eprintln!("KYO-975 live: {summary_log}");

    // Persisted state: encrypted summary + cut index.
    let raw = chat_service::get_session(&db, &session_id)
        .await
        .expect("read session")
        .expect("session exists")
        .config
        .expect("persisted config");
    let restored = kyomi_auth::encryption::restore_chat_config(&raw, &encryption_key)
        .expect("restore encrypted config");
    let summary = restored
        .pointer("/agent_state/compacted_summary")
        .and_then(Value::as_str)
        .expect("a persisted summary");
    assert!(!summary.trim().is_empty());
    let index = restored
        .pointer("/agent_state/messages_since_compaction_index")
        .and_then(Value::as_u64)
        .expect("a persisted cut index") as usize;
    assert!(index > 0);
    eprintln!("KYO-975 live: persisted summary ({} chars), cut index {index}", summary.len());
    assert!(
        !raw.to_string().contains(summary),
        "the summary is stored encrypted, never as plaintext"
    );

    // The turn's reported usage includes the summary call: it must equal the
    // run's billed row, which shares the ledger.
    if let kyomi_core::DbPool::Sqlite(sq) = &db {
        let (row_in, row_out): (i32, i32) = sqlx::query_as(
            "SELECT input_tokens, output_tokens FROM api_usage_log ORDER BY id DESC LIMIT 1",
        )
        .fetch_one(sq)
        .await
        .expect("the run's usage row");
        let reported = first.token_usage.as_ref().expect("reported token usage");
        assert_eq!(reported["input_tokens"].as_i64(), Some(i64::from(row_in)));
        assert_eq!(reported["output_tokens"].as_i64(), Some(i64::from(row_out)));
    }

    // Tool-pair safety in the kept tail. `execute_agent_chat` puts the system
    // prompt first, so the DB index is the state index minus one.
    let messages = chat_service::get_agent_messages(&db, &encryption_key, &session_id, None)
        .await
        .expect("read history");
    let kept = &messages[index.saturating_sub(1)..];
    assert!(!kept.is_empty(), "the kept tail has the recent turns");
    assert_ne!(kept[0].role, "tool", "the cut never starts the tail with a tool result");
    let mut called = std::collections::HashSet::new();
    for message in kept {
        if let Some(calls) = &message.tool_calls {
            for call in calls.as_array().into_iter().flatten() {
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    called.insert(id.to_string());
                }
            }
        }
        if message.role == "tool" {
            let id = message.tool_call_id.as_deref().expect("tool row carries its call id");
            assert!(called.contains(id), "kept tool result {id} has its call in the kept tail");
        }
    }

    // A second run (a fresh `execute_agent_chat`, like after a restart) loads
    // the summary + index and answers from the compacted context.
    let second = run("What earlier topic does the summary of our conversation mention?").await;
    let second = second.expect("the second live turn completes after restoring the summary");
    assert_eq!(second.status, "completed", "{:?}", second.error);
    assert!(
        second.response_text.to_lowercase().contains("revenue"),
        "{}",
        second.response_text
    );

    // Running cost tally (provider cost from the per-call metadata).
    if let kyomi_core::DbPool::Sqlite(sq) = &db {
        let rows: Vec<(Option<String>,)> =
            sqlx::query_as("SELECT extra_metadata FROM api_usage_log ORDER BY id")
                .fetch_all(sq)
                .await
                .expect("read usage rows");
        let mut total = 0.0;
        let mut calls = 0;
        for (metadata,) in rows {
            let Some(metadata) = metadata else { continue };
            let value: Value = serde_json::from_str(&metadata).expect("usage metadata json");
            for call in value["calls"].as_array().into_iter().flatten() {
                calls += 1;
                total += call["cost_usd"].as_f64().unwrap_or(0.0);
            }
        }
        eprintln!("KYO-975 live tally: {calls} Anthropic calls, provider cost ${total:.6}");
    }

    eprintln!("KYO-975 live: first answer: {}", first.response_text.chars().take(200).collect::<String>());
    eprintln!("KYO-975 live: second answer: {}", second.response_text.chars().take(200).collect::<String>());
}
