// SPDX-License-Identifier: AGPL-3.0-or-later

//! Live Anthropic verification for KYO-974. `#[ignore]`d: each run spends real
//! money. Run with the key in the environment only (never printed):
//!
//! ```text
//! ANTHROPIC_API_KEY="$(cat <key file>)" cargo test -p kyomi-agent --lib \
//!     anthropic_live_tests -- --ignored --nocapture --test-threads=1
//! ```

use std::collections::HashMap;

use serde_json::json;

use crate::anthropic::AnthropicClient;
use crate::provider::RequestOptions;
use crate::types::{LLMResponse, Message, Tool};

/// Spend ceiling per scenario (USD); the run aborts past it.
const SCENARIO_CAP_USD: f64 = 0.08;

fn key() -> String {
    std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY (live tests only)")
}

fn tools() -> Vec<Tool> {
    vec![Tool {
        name: "lookup_metric".into(),
        description: "Look up the current value of a named business metric.".into(),
        parameters: json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}),
    }]
}

/// ~4k-token system prompt, unique per run so the first call writes the cache.
fn system_prompt(nonce: &str) -> String {
    let mut s = format!("You are a careful analytics assistant. Run id: {nonce}.\n");
    for i in 0..120 {
        s.push_str(&format!(
            "Guideline {i}: when answering, state which metric you used, quote its value, and keep the final answer under 80 words; never invent metrics.\n"
        ));
    }
    s
}

fn metric_value(name: &str) -> &'static str {
    if name == "top_region" {
        r#"{"value": "emea"}"#
    } else if name.starts_with("revenue_") {
        r#"{"value": 91000, "unit": "usd"}"#
    } else if name.contains("churn") { r#"{"value": 0.042, "unit": "ratio"}"# } else { r#"{"value": 183000, "unit": "usd"}"# }
}

fn show(label: &str, r: &LLMResponse) {
    let d = r.call_detail.as_ref().expect("detail");
    let u = &r.usage;
    eprintln!(
        "{label}: fresh_in={} out={} think_tok={} cache_write={} (5m={},1h={}) cache_read={} cost=${:.6} stop={} tier={:?} msg={:?} req={:?} thinking_chars={}",
        u.input_tokens, u.output_tokens, u.reasoning_tokens, u.cache_creation_input_tokens,
        d.cache_creation_5m, d.cache_creation_1h, u.cache_read_input_tokens, d.cost_usd,
        r.finish_reason, d.service_tier, d.message_id, d.request_id,
        r.thinking_content.as_ref().map_or(0, String::len),
    );
    let types: Vec<_> = r.raw_response["content"].as_array().map_or(vec![], |c| c.iter().map(|b| b["type"].as_str().unwrap_or("?")).collect());
    eprintln!("  block types: {types:?}");
    if let Some(t) = &r.thinking_content {
        eprintln!("  thinking: {}", t.chars().take(160).collect::<String>().replace('\n', " "));
    }
}

/// Run the tool loop. `replay` sends the assistant turn back as the raw
/// content blocks the API returned (thinking blocks and signatures unchanged)
/// instead of Kyomi's stateless reconstruction.
async fn tool_loop(model: &str, effort: &str, replay: bool) -> f64 {
    tool_loop_with(model, effort, replay, PARALLEL_PROMPT).await
}

const PARALLEL_PROMPT: &str = "Look up the metrics 'monthly_revenue' and 'churn_rate' (call the tool for each), then tell me which is larger in absolute terms and why that comparison is or is not meaningful.";

/// Each lookup depends on the previous result, so the loop needs several rounds.
const DEPENDENT_PROMPT: &str = "Call lookup_metric with name 'top_region' and wait for the answer. Then call it with name 'revenue_<region>' (region lowercased, from that answer), one call per turn, and wait. Then call it with name 'churn_<region>'. One tool call per turn, never several at once. Finish with a one-sentence summary.";

async fn tool_loop_with(model: &str, effort: &str, replay: bool, prompt: &str) -> f64 {
    let client = AnthropicClient::new(key(), Some(model.into())).unwrap();
    let nonce = format!("{model}-{replay}-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap());
    let opts = RequestOptions::new(8192).with_reasoning_effort(Some(effort.into()));
    let tools = tools();
    let mut messages = vec![
        Message::system(system_prompt(&nonce)),
        Message::user(prompt),
    ];
    let mut total = 0.0;
    let mut raw_assistant: Vec<serde_json::Value> = Vec::new();
    eprintln!("== {model} effort={effort} replay={replay}");
    for round in 0..5 {
        let response = if replay && !raw_assistant.is_empty() {
            post_with_replay(&client, model, &messages, &tools, &opts, &raw_assistant).await
        } else {
            client.complete(&messages, &tools, &opts, &HashMap::new()).await.unwrap()
        };
        show(&format!("call {round}"), &response);
        total += response.cost.unwrap();
        assert!(total < SCENARIO_CAP_USD, "scenario spend cap exceeded: ${total}");
        let Some(calls) = response.tool_calls.clone() else {
            eprintln!("  final: {}", response.content.chars().take(240).collect::<String>().replace('\n', " "));
            break;
        };
        raw_assistant.push(json!({"round": round, "content": response.raw_response["content"].clone()}));
        messages.push(Message::assistant_with_tool_calls(response.content.clone(), calls.clone()));
        for c in &calls {
            let name = c.arguments["name"].as_str().unwrap_or_default();
            messages.push(Message::tool_result(&c.id, &c.name, metric_value(name)));
        }
    }
    eprintln!("== total ${total:.6}");
    total
}

/// Same request as `complete`, but each earlier assistant tool-call turn is
/// replaced by the raw blocks the API returned for it.
async fn post_with_replay(
    client: &AnthropicClient,
    model: &str,
    messages: &[Message],
    tools: &[Tool],
    opts: &RequestOptions,
    raw_turns: &[serde_json::Value],
) -> LLMResponse {
    let (system, mut msgs) = AnthropicClient::convert_messages_to_anthropic(messages, &HashMap::new());
    let mut turn = 0;
    for m in msgs.iter_mut() {
        if m["role"] == "assistant" && m["content"].is_array() {
            m["content"] = raw_turns[turn]["content"].clone();
            turn += 1;
        }
    }
    assert_eq!(turn, raw_turns.len());
    let body = AnthropicClient::build_request_body(model, system, msgs, tools, opts);
    client.post_for_live_test(&body).await
}

#[tokio::test]
#[ignore = "live: spends money"]
async fn haiku_5_5_stateless_vs_replay() {
    let a = tool_loop("claude-haiku-5-5", "high", false).await;
    let b = tool_loop("claude-haiku-5-5", "high", true).await;
    eprintln!("haiku stateless ${a:.6} vs replay ${b:.6}");
}

#[tokio::test]
#[ignore = "live: spends money"]
async fn haiku_5_5_dependent_chain_stateless_vs_replay() {
    let mut t = [0.0; 2];
    for (i, replay) in [false, true].into_iter().enumerate() {
        t[i] = tool_loop_with("claude-haiku-5-5", "high", replay, DEPENDENT_PROMPT).await;
    }
    eprintln!("dependent chain: stateless ${:.6} vs replay ${:.6}", t[0], t[1]);
}

#[tokio::test]
#[ignore = "live: spends money"]
async fn sonnet_5_5_chat_with_thinking_and_tool_loop() {
    let t = tool_loop("claude-sonnet-5-5", "high", false).await;
    eprintln!("sonnet total ${t:.6}");
}

#[tokio::test]
#[ignore = "live: spends money"]
async fn sonnet_5_5_dependent_chain_shows_summarized_thinking() {
    let t = tool_loop_with(
        "claude-sonnet-5-5",
        "high",
        false,
        "Think carefully. Customer A pays 40% more than B; B pays half of C; C pays $120. Work out what each pays, then call lookup_metric with name 'top_region', then 'revenue_<region>' (lowercased region from the answer), one call per turn, and finally explain whether A's payment exceeds that region's revenue.",
    )
    .await;
    eprintln!("sonnet chain total ${t:.6}");
}
