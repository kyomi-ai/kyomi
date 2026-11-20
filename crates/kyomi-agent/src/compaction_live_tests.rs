// SPDX-License-Identifier: AGPL-3.0-or-later

//! Live Anthropic verification for KYO-975 (conversation compaction).
//! `#[ignore]`d: each run spends real money. Run with the key in the
//! environment only (never printed):
//!
//! ```text
//! ANTHROPIC_API_KEY="$(cat <key file>)" cargo test -p kyomi-agent --lib \
//!     compaction_live_tests -- --ignored --nocapture --test-threads=1
//! ```

use std::sync::Arc;

use serde_json::Value;

use crate::execution::{AgentExecutionConfig, AgentExecutionEnv, execute_agent_chat};
use kyomi_auth::{chat_service, websocket::WebSocketManager};
use kyomi_core::enums::WorkspaceRole;
use kyomi_core::{kv_store_memory::InMemoryKVStore, platform::PlatformRegistry};

const MODEL: &str = "claude-haiku-5-5";
/// Above the seeded history's estimate and above the compacted context, so the
/// run compacts once with more than `MESSAGES_TO_KEEP` turns to summarise.
const THRESHOLD: u32 = 15_000;

fn key() -> String {
    std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY (live tests only)")
}

fn config(session_id: &str, message: &str) -> AgentExecutionConfig {
    AgentExecutionConfig {
        session_id: session_id.to_string(),
        user_id: "user-a".to_string(),
        workspace_id: "ws-1".to_string(),
        message: message.to_string(),
        workspace_roles: vec![WorkspaceRole::WorkspaceAdmin],
        // A small fixed prefix keeps the threshold meaningful: the default
        // system prompt alone is larger than a few thousand tokens.
        system_prompt: Some(
            "You are a concise analytics assistant. Follow the user's instructions exactly."
                .to_string(),
        ),
        // Only the harmless tool this scenario needs; the full registry's
        // schemas would dominate the request.
        tools_subset: Some(vec!["get_workspace_info".to_string()]),
        max_iterations: 12,
        max_tokens: 4_096,
        component: "custom_agent".to_string(),
        ..AgentExecutionConfig::default()
    }
}

/// Live scenario: a seeded conversation compacts once, the summary + cut index
/// persist encrypted, a tool call and its result stay together, and a second
/// run restores the summary and answers from it.
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
    // Seed history: eight large turns about the same topic, then four short
    // ones. The cut summarises the first seven, leaving one large turn plus the
    // short tail well below the threshold (the live request's fixed
    // system/tool prefix is part of the provider's input count, so the live
    // threshold is higher than the few-thousand-token unit tests).
    for i in 0..12u8 {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        let content = if i < 8 {
            format!(
                "We are investigating the Q4 revenue shortfall (step {i}): {}",
                "x".repeat(8_000)
            )
        } else {
            format!("noted step {i}")
        };
        chat_service::add_message(
            &db,
            &encryption_key,
            &session_id,
            role,
            &content,
            None,
            None,
            None,
            Some("web"),
            Some("user-a"),
            None,
            None,
            None,
            chat_service::MessageStatus::Complete,
        )
        .await
        .expect("seed history");
    }

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
        for (level, message) in capture.events() { eprintln!("CAPTURE {level} {message}"); }
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
