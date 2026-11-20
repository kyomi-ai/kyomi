// SPDX-License-Identifier: AGPL-3.0-or-later

//! Context compaction for the agent system.
//!
//! When a conversation reaches the run's resolved input-token threshold, older
//! messages are summarized into a compact summary. This module provides the
//! utility functions for deciding when to compact, splitting messages, building
//! the summarization prompt, and reconstructing context.
//!
//! # Architecture
//!
//! The compaction flow:
//!
//! 1. [`CustomAgent::maybe_compact`](crate::agent::CustomAgent) checks
//!    [`should_compact`] at the start of each agent-loop iteration (and before
//!    the forced wrap-up call). It uses the actual input token count from the
//!    previous LLM call **and** a character-based estimate of the context that
//!    would be sent; either reaching the run's resolved threshold (the model's
//!    context window times the configured fraction, or an absolute override)
//!    triggers compaction (KYO-975).
//! 2. If compaction is needed, [`split_messages_for_compaction`] divides
//!    messages into "to summarize" and "to keep" groups at a tool-pair-safe
//!    boundary.
//! 3. [`build_compaction_prompt`] creates a summarization prompt from the
//!    messages that need compacting.
//! 4. The agent sends this prompt to the run's fast model (the workspace title
//!    model, else `LLM_TITLE_MODEL`, else the setup's own model) when that
//!    model's own context window can accept the summarised region; otherwise
//!    the chat model writes the one summary call.
//! 5. The agent stores the summary and the cut index on
//!    [`AgentState`](crate::agent::AgentState); the adapter persists them with
//!    the session, and
//!    [`build_llm_context`](crate::agent::CustomAgent) reconstructs the LLM
//!    message list from the summary + recent messages on later calls.

use crate::provider::{LLMProvider, RequestOptions};
use crate::types::{Message, MessageRole};

/// What the agent needs to compact: the threshold the run resolved (KYO-975)
/// and the fast model that writes the summary.
pub struct CompactionRuntime {
    /// Input-token count at or above which older turns are summarised.
    pub threshold_tokens: u32,
    /// Fast model used only for the summary (the workspace title model, else
    /// `LLM_TITLE_MODEL`, else the setup's own model).
    pub client: Box<dyn LLMProvider>,
    /// Request options for the summary call (title/background sampling).
    pub options: RequestOptions,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find the largest byte index `<= max_bytes` that lies on a UTF-8 character
/// boundary. Equivalent to `str::floor_char_boundary` (nightly-only as of
/// Rust 1.85).
pub(crate) fn floor_char_boundary(s: &str, max_bytes: usize) -> usize {
    if max_bytes >= s.len() {
        return s.len();
    }
    // Walk backwards from `max_bytes` until we find a byte that is not a
    // continuation byte (0b10xxxxxx).
    let bytes = s.as_bytes();
    let mut i = max_bytes;
    while i > 0 && (bytes[i] & 0b1100_0000) == 0b1000_0000 {
        i -= 1;
    }
    i
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Rough character-to-token ratio for fallback estimation.
pub const CHARS_PER_TOKEN: u32 = 4;

/// Number of recent messages to keep uncompacted (last 3 user+assistant exchanges).
pub const MESSAGES_TO_KEEP: usize = 6;

// ---------------------------------------------------------------------------
// Public functions
// ---------------------------------------------------------------------------

/// Check whether the conversation context should be compacted.
///
/// Returns `true` if **either** the actual input token count from the last LLM
/// call or the character-based estimate (total chars / [`CHARS_PER_TOKEN`],
/// including the system prompt length) is at or above `threshold_tokens`.
///
/// Both are considered because the actual count describes the *previous*
/// request: the new user message (or appended tool results) can push the next
/// request over the window even when the last one was below it.
///
/// `threshold_tokens` is the value a run resolved from the model's context
/// window and the configured fraction/absolute override (KYO-975); there is no
/// built-in default here.
pub fn should_compact(
    last_input_tokens: Option<u32>,
    messages: &[Message],
    system_prompt_len: usize,
    threshold_tokens: u32,
) -> bool {
    last_input_tokens.is_some_and(|actual| actual > 0 && actual >= threshold_tokens)
        || estimate_tokens(messages, system_prompt_len) >= threshold_tokens
}

/// Character-based token estimate for `messages` plus `system_prompt_len`.
pub fn estimate_tokens(messages: &[Message], system_prompt_len: usize) -> u32 {
    let mut total_chars: usize = system_prompt_len;

    for msg in messages {
        total_chars += msg.content.len();

        // Stored provider blocks (web search/fetch results) are sent with the
        // message and count as input tokens, so they belong in the estimate (KYO-969).
        if let Some(ref blocks) = msg.provider_content {
            total_chars += blocks.iter().map(|block| block.to_string().len()).sum::<usize>();
        }

        // Include tool call arguments in the estimate.
        if let Some(ref tool_calls) = msg.tool_calls {
            for tc in tool_calls {
                // Arguments are serde_json::Value -- serialized length is a
                // reasonable proxy for the token cost.
                total_chars += tc.arguments.to_string().len();
            }
        }
    }

    (total_chars / CHARS_PER_TOKEN as usize) as u32
}

/// Split messages into (to_summarize, to_keep) for compaction.
///
/// - If `messages.len() <= MESSAGES_TO_KEEP`, all messages are kept and
///   the "to summarize" vec is empty.
/// - Otherwise, at least the last [`MESSAGES_TO_KEEP`] messages are kept.
/// - The cut never separates a tool call from its result:
///   * if the first kept message would be a tool result, the cut moves back so
///     the assistant call and every result stay together in the **kept** part;
///   * if the first kept message is an assistant message directly preceded by
///     another assistant message, the cut moves back over the whole run. A
///     `pause_turn` chain and a provider-executed server tool's result are
///     stored as consecutive assistant messages, and splitting them would keep
///     a result whose call was summarised away;
///   * if a kept assistant message carries a provider server-tool result, the
///     cut moves back to the message that announced its `server_tool_use`, even
///     when client tool results sit between them (the deferred-call shape).
///
/// **Note:** The input should NOT include the system prompt message --
/// the caller should strip it before calling this function and re-add it
/// when building the final context.
pub fn split_messages_for_compaction(messages: &[Message]) -> (Vec<Message>, Vec<Message>) {
    if messages.len() <= MESSAGES_TO_KEEP {
        return (Vec::new(), messages.to_vec());
    }

    let mut cutoff = messages.len() - MESSAGES_TO_KEEP;
    while cutoff > 0
        && (messages[cutoff].role == MessageRole::Tool
            || (messages[cutoff].role == MessageRole::Assistant
                && messages[cutoff - 1].role == MessageRole::Assistant))
    {
        cutoff -= 1;
    }
    // A provider server tool's result can arrive in a later assistant message
    // (a `pause_turn` continuation or a deferred call behind client tool
    // results), without repeating the `server_tool_use`. If a kept result's use
    // would be summarised, move the cut back before the use's message.
    loop {
        let mut move_to = cutoff;
        for message in &messages[cutoff..] {
            for id in server_result_use_ids(message) {
                if let Some(index) = messages
                    .iter()
                    .position(|candidate| server_use_ids(candidate).any(|use_id| use_id == id))
                    && index < move_to
                {
                    move_to = index;
                }
            }
        }
        if move_to == cutoff {
            break;
        }
        cutoff = move_to;
    }
    // The cut reached the start of the history (for example a history that
    // begins with a tool result, or with a server-tool use whose result is in
    // the kept tail): there is no safe cut, so keep everything rather than
    // build a request the provider will reject.
    if cutoff == 0 {
        return (Vec::new(), messages.to_vec());
    }

    let to_summarize = messages[..cutoff].to_vec();
    let to_keep = messages[cutoff..].to_vec();

    (to_summarize, to_keep)
}

/// The `server_tool_use` ids in a message's stored provider blocks.
fn server_use_ids(message: &Message) -> impl Iterator<Item = &str> {
    message
        .provider_content
        .as_deref()
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("server_tool_use"))
        .filter_map(|block| block.get("id").and_then(serde_json::Value::as_str))
}

/// The `tool_use_id` of each provider server-tool result block in a message.
fn server_result_use_ids(message: &Message) -> impl Iterator<Item = &str> {
    message
        .provider_content
        .as_deref()
        .into_iter()
        .flatten()
        .filter(|block| {
            block
                .get("type")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|kind| kind != "tool_result" && kind.ends_with("_tool_result"))
        })
        .filter_map(|block| block.get("tool_use_id").and_then(serde_json::Value::as_str))
}

/// Build a summarization prompt from messages that need compacting.
///
/// The resulting string is intended to be sent to a fast model (Haiku)
/// as the user message content. It asks the model to preserve key
/// questions, data discovered, queries executed, insights found, and
/// context needed for continuation.
pub fn build_compaction_prompt(messages_to_summarize: &[Message]) -> String {
    let mut conversation_lines = Vec::new();

    for msg in messages_to_summarize {
        let role_label = match msg.role {
            MessageRole::User => "USER".to_string(),
            MessageRole::Assistant => "ASSISTANT".to_string(),
            MessageRole::Tool => {
                if let Some(ref name) = msg.name {
                    format!("TOOL RESULT ({name})")
                } else {
                    "TOOL RESULT".to_string()
                }
            }
            MessageRole::System => "SYSTEM".to_string(),
        };

        let mut content = msg.content.clone();

        // Truncate very long tool results for the summary prompt.
        // Use floor_char_boundary to avoid panicking on multi-byte UTF-8.
        if msg.role == MessageRole::Tool && content.len() > 2000 {
            let boundary = floor_char_boundary(&content, 2000);
            content.truncate(boundary);
            content.push_str("\n... [truncated for summary]");
        }

        conversation_lines.push(format!("**{role_label}**:\n{content}\n"));

        // Include tool call names if present.
        if let Some(ref tool_calls) = msg.tool_calls {
            for tc in tool_calls {
                conversation_lines.push(format!("  -> Called tool: {}", tc.name));
            }
        }
    }

    let conversation_text = conversation_lines.join("\n");

    format!(
        "You are summarizing a conversation between a user and an AI data analyst assistant.\n\
         \n\
         Create a comprehensive summary that captures:\n\
         1. **Key Questions Asked**: What did the user want to know?\n\
         2. **Data Discovered**: Tables, schemas, columns that were explored\n\
         3. **Queries Executed**: Important SQL queries and their results (include actual numbers/findings)\n\
         4. **Insights Found**: Key findings, patterns, or conclusions\n\
         5. **Context for Continuation**: Any important context needed to continue the conversation\n\
         \n\
         Be thorough - this summary will replace the original messages in the LLM's context.\n\
         Preserve specific details like table names, column names, query results, and numbers.\n\
         \n\
         ---\n\
         CONVERSATION TO SUMMARIZE:\n\
         \n\
         {conversation_text}\n\
         \n\
         ---\n\
         COMPREHENSIVE SUMMARY:"
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- should_compact tests ------------------------------------------------

    /// The threshold these tests pass in; resolution itself is tested below.
    const THRESHOLD: u32 = 150_000;

    #[test]
    fn the_character_estimate_counts_stored_provider_blocks() {
        // Text alone is far below the threshold; the stored search results are not.
        let blocks = vec![serde_json::json!({"type": "text", "text": "x".repeat(CHARS_PER_TOKEN as usize * THRESHOLD as usize)})];
        let plain = vec![Message::user("q"), Message::assistant("a")];
        let with_blocks = vec![Message::user("q"), Message::assistant("a").with_provider_content(Some(blocks))];
        assert!(!should_compact(None, &plain, 0, THRESHOLD));
        assert!(should_compact(None, &with_blocks, 0, THRESHOLD));
    }

    #[test]
    fn should_compact_returns_false_below_threshold_with_actual_tokens() {
        // 100K tokens is well below the 150K threshold.
        let result = should_compact(Some(100_000), &[], 0, THRESHOLD);
        assert!(!result);
    }

    #[test]
    fn should_compact_returns_true_at_threshold_with_actual_tokens() {
        // Exactly at threshold should trigger compaction.
        let result = should_compact(Some(THRESHOLD), &[], 0, THRESHOLD);
        assert!(result);
    }

    #[test]
    fn should_compact_returns_true_above_threshold_with_actual_tokens() {
        let result = should_compact(Some(THRESHOLD + 10_000), &[], 0, THRESHOLD);
        assert!(result);
    }

    #[test]
    fn should_compact_fallback_estimation_below_threshold() {
        // With None for actual tokens, use character estimation.
        // 1000 chars / 4 = 250 tokens -- well below threshold.
        let messages = vec![Message::user("x".repeat(1000))];
        let result = should_compact(None, &messages, 0, THRESHOLD);
        assert!(!result);
    }

    #[test]
    fn should_compact_fallback_estimation_above_threshold() {
        // Need 150K tokens * 4 chars/token = 600K chars to trigger.
        // Create a message with enough content.
        let big_content = "x".repeat(600_001);
        let messages = vec![Message::user(big_content)];
        let result = should_compact(None, &messages, 0, THRESHOLD);
        assert!(result);
    }

    #[test]
    fn should_compact_fallback_includes_system_prompt_length() {
        // System prompt of 599_996 chars + message of 4 chars = 600_000 chars
        // 600_000 / 4 = 150_000 tokens = threshold.
        let messages = vec![Message::user("abcd")];
        let system_prompt_len = 599_996;
        let result = should_compact(None, &messages, system_prompt_len, THRESHOLD);
        assert!(result);
    }

    #[test]
    fn should_compact_zero_actual_tokens_uses_fallback() {
        // Some(0) should fall through to the estimation path.
        let messages = vec![Message::user("short message")];
        let result = should_compact(Some(0), &messages, 0, THRESHOLD);
        assert!(!result);
    }

    // -- Resolved thresholds (KYO-975) --------------------------------------

    #[test]
    fn a_one_million_token_model_compacts_at_the_derived_threshold_not_150k() {
        use kyomi_core::llm_compaction::{LlmCompactionSettings, resolve};

        let resolved = resolve(1_000_000, &LlmCompactionSettings::default(), &LlmCompactionSettings::default());
        assert_eq!(resolved.threshold_tokens, 750_000);
        assert!(!should_compact(Some(150_000), &[], 0, resolved.threshold_tokens));
        assert!(!should_compact(Some(749_999), &[], 0, resolved.threshold_tokens));
        assert!(should_compact(Some(750_000), &[], 0, resolved.threshold_tokens));
        assert!(should_compact(Some(750_001), &[], 0, resolved.threshold_tokens));
    }

    #[test]
    fn a_two_hundred_thousand_token_model_compacts_at_its_own_threshold() {
        use kyomi_core::llm_compaction::{LlmCompactionSettings, resolve};

        let resolved = resolve(200_000, &LlmCompactionSettings::default(), &LlmCompactionSettings::default());
        assert_eq!(resolved.threshold_tokens, 150_000);
        assert!(!should_compact(Some(149_999), &[], 0, resolved.threshold_tokens));
        assert!(should_compact(Some(150_000), &[], 0, resolved.threshold_tokens));
        assert!(should_compact(Some(150_001), &[], 0, resolved.threshold_tokens));
    }

    #[test]
    fn an_unknown_model_uses_the_documented_default_window() {
        use kyomi_core::llm_compaction::{ContextWindowSource, LlmCompactionSettings, resolve};

        let resolved = resolve(0, &LlmCompactionSettings::default(), &LlmCompactionSettings::default());
        assert_eq!(resolved.context_window, kyomi_core::llm_compaction::DEFAULT_CONTEXT_WINDOW);
        assert_eq!(resolved.context_window_source, ContextWindowSource::Default);
        assert_eq!(resolved.threshold_tokens, 150_000);
        assert!(!should_compact(Some(149_999), &[], 0, resolved.threshold_tokens));
        assert!(should_compact(Some(150_000), &[], 0, resolved.threshold_tokens));
        assert!(should_compact(Some(150_001), &[], 0, resolved.threshold_tokens));
    }

    #[test]
    fn a_below_threshold_actual_still_compacts_when_the_new_turn_pushes_the_estimate_over() {
        // The restored/previous call was below the threshold, but the next turn
        // added ~175K tokens: the estimate must win (KYO-975 review).
        let huge = vec![Message::user("x".repeat(700_000))];
        assert!(should_compact(Some(140_000), &huge, 0, 150_000));

        let small = vec![Message::user("short")];
        assert!(!should_compact(Some(140_000), &small, 0, 150_000));
    }

    // -- split_messages_for_compaction tests ----------------------------------

    #[test]
    fn split_messages_with_enough_messages() {
        let messages: Vec<Message> = (0..10)
            .map(|i| Message::user(format!("Message {i}")))
            .collect();

        let (to_summarize, to_keep) = split_messages_for_compaction(&messages);

        assert_eq!(to_summarize.len(), 4); // 10 - 6 = 4
        assert_eq!(to_keep.len(), MESSAGES_TO_KEEP);
        assert_eq!(to_summarize[0].content, "Message 0");
        assert_eq!(to_summarize[3].content, "Message 3");
        assert_eq!(to_keep[0].content, "Message 4");
        assert_eq!(to_keep[5].content, "Message 9");
    }

    #[test]
    fn split_messages_with_few_messages_returns_empty_first() {
        let messages = vec![
            Message::user("Hello"),
            Message::assistant("Hi there!"),
        ];

        let (to_summarize, to_keep) = split_messages_for_compaction(&messages);

        assert!(to_summarize.is_empty());
        assert_eq!(to_keep.len(), 2);
        assert_eq!(to_keep[0].content, "Hello");
        assert_eq!(to_keep[1].content, "Hi there!");
    }

    #[test]
    fn split_messages_with_exactly_messages_to_keep() {
        let messages: Vec<Message> = (0..MESSAGES_TO_KEEP)
            .map(|i| Message::user(format!("Message {i}")))
            .collect();

        let (to_summarize, to_keep) = split_messages_for_compaction(&messages);

        assert!(to_summarize.is_empty());
        assert_eq!(to_keep.len(), MESSAGES_TO_KEEP);
    }

    #[test]
    fn split_messages_empty_input() {
        let (to_summarize, to_keep) = split_messages_for_compaction(&[]);
        assert!(to_summarize.is_empty());
        assert!(to_keep.is_empty());
    }

    // -- build_compaction_prompt tests ----------------------------------------

    #[test]
    fn build_compaction_prompt_includes_message_content() {
        let messages = vec![
            Message::user("What is our revenue?"),
            Message::assistant("Let me check the database."),
            Message::tool_result("tc_1", "query_datasource", "Revenue: $1.2M"),
        ];

        let prompt = build_compaction_prompt(&messages);

        assert!(prompt.contains("What is our revenue?"));
        assert!(prompt.contains("Let me check the database."));
        assert!(prompt.contains("Revenue: $1.2M"));
        assert!(prompt.contains("TOOL RESULT (query_datasource)"));
        assert!(prompt.contains("Key Questions Asked"));
        assert!(prompt.contains("Data Discovered"));
        assert!(prompt.contains("COMPREHENSIVE SUMMARY:"));
    }

    #[test]
    fn build_compaction_prompt_truncates_long_tool_results() {
        let long_result = "x".repeat(3000);
        let messages = vec![
            Message::tool_result("tc_1", "query_datasource", &long_result),
        ];

        let prompt = build_compaction_prompt(&messages);

        // The truncated content should be present.
        assert!(prompt.contains("[truncated for summary]"));
        // The full 3000-char content should NOT be present.
        assert!(!prompt.contains(&long_result));
    }

    // -- Constants tests -----------------------------------------------------

    #[test]
    fn constants_are_reasonable_values() {
        assert_eq!(CHARS_PER_TOKEN, 4);
        assert_eq!(MESSAGES_TO_KEEP, 6);
        // MESSAGES_TO_KEEP should be even (pairs of user+assistant).
        assert_eq!(MESSAGES_TO_KEEP % 2, 0);
    }
}

// ---------------------------------------------------------------------------
// Contract tests — behavioral contracts for the compaction system
// ---------------------------------------------------------------------------

#[cfg(test)]
mod contract_tests {
    use super::*;
    use crate::types::ToolCall;

    const THRESHOLD: u32 = 150_000;

    // -- Large message handling -----------------------------------------------

    #[test]
    fn split_handles_100_messages() {
        let messages: Vec<Message> = (0..100)
            .map(|i| Message::user(format!("Message {i}")))
            .collect();

        let (to_summarize, to_keep) = split_messages_for_compaction(&messages);

        assert_eq!(to_summarize.len(), 94); // 100 - 6
        assert_eq!(to_keep.len(), MESSAGES_TO_KEEP);

        // First summarized message should be message 0
        assert_eq!(to_summarize[0].content, "Message 0");
        // Last summarized message should be message 93
        assert_eq!(to_summarize[93].content, "Message 93");
        // First kept message should be message 94
        assert_eq!(to_keep[0].content, "Message 94");
        // Last kept message should be message 99
        assert_eq!(to_keep[5].content, "Message 99");
    }

    #[test]
    fn split_handles_exactly_messages_to_keep_plus_one() {
        let messages: Vec<Message> = (0..=MESSAGES_TO_KEEP)
            .map(|i| Message::user(format!("Message {i}")))
            .collect();

        let (to_summarize, to_keep) = split_messages_for_compaction(&messages);

        assert_eq!(to_summarize.len(), 1);
        assert_eq!(to_keep.len(), MESSAGES_TO_KEEP);
        assert_eq!(to_summarize[0].content, "Message 0");
    }

    // -- Tool-pair-safe cut points (KYO-975) ----------------------------------

    fn tool_call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: serde_json::json!({"sql": "SELECT 1"}),
            arguments_error: None,
        }
    }

    fn assistant_with_calls(ids: &[&str]) -> Message {
        Message::assistant_with_tool_calls(
            "",
            ids.iter().map(|id| tool_call(id, "query_datasource")).collect(),
        )
    }

    #[test]
    fn the_cut_does_not_start_the_kept_tail_with_a_tool_result() {
        // A naive `len - 6` cut lands exactly on tc_2's result. The cut must
        // move back before tc_2's assistant call so call and result stay
        // together in the kept tail.
        let messages = vec![
            Message::user("first"),
            assistant_with_calls(&["tc_1"]),
            Message::tool_result("tc_1", "query_datasource", "one"),
            assistant_with_calls(&["tc_2"]),
            Message::tool_result("tc_2", "query_datasource", "two"),
            Message::user("q2"),
            Message::assistant("a2"),
            Message::user("q3"),
            Message::assistant("a3"),
            Message::user("q4"),
        ];

        let (to_summarize, to_keep) = split_messages_for_compaction(&messages);

        assert_eq!(to_summarize.len(), 3, "tc_1's whole batch is summarised");
        assert_eq!(to_keep[0].role, MessageRole::Assistant);
        assert_eq!(to_keep[0].tool_calls.as_ref().unwrap()[0].id, "tc_2");
        assert_eq!(to_keep[1].role, MessageRole::Tool);
        assert_eq!(to_keep[1].tool_call_id.as_deref(), Some("tc_2"));
    }

    #[test]
    fn the_cut_moves_past_every_result_of_a_multi_call_batch() {
        let messages = vec![
            Message::user("first"),
            assistant_with_calls(&["tc_1", "tc_2"]),
            Message::tool_result("tc_1", "query_datasource", "one"),
            Message::tool_result("tc_2", "query_datasource", "two"),
            Message::user("q2"),
            Message::assistant("a2"),
            Message::user("q3"),
            Message::assistant("a3"),
            Message::user("q4"),
        ];

        let (to_summarize, to_keep) = split_messages_for_compaction(&messages);

        assert_eq!(to_summarize.len(), 1, "only the first user message is summarised");
        assert_eq!(to_keep[0].role, MessageRole::Assistant);
        assert_eq!(to_keep[0].tool_calls.as_ref().unwrap().len(), 2);
        assert_eq!(to_keep[1].tool_call_id.as_deref(), Some("tc_1"));
        assert_eq!(to_keep[2].tool_call_id.as_deref(), Some("tc_2"));
    }

    #[test]
    fn the_cut_keeps_a_deferred_server_tool_use_with_its_result() {
        // A server call is announced on assistant A, client tool results follow,
        // and the server result arrives on assistant B (KYO-979's deferred
        // shape). The naive cut at index 6 keeps B's result and summarises A.
        let messages = vec![
            Message::user("u0"),
            Message::user("u1"),
            Message::user("u2"),
            Message::user("u3"),
            Message::assistant("").with_provider_content(Some(vec![
                serde_json::json!({"type": "server_tool_use", "id": "srv1", "name": "web_search", "input": {"query": "x"}}),
            ])),
            Message::tool_result("tc_1", "query_datasource", "client result"),
            Message::assistant("").with_provider_content(Some(vec![
                serde_json::json!({"type": "web_search_tool_result", "tool_use_id": "srv1", "content": []}),
            ])),
            Message::user("u4"),
            Message::assistant("a4"),
            Message::user("u5"),
            Message::assistant("a5"),
            Message::user("u6"),
        ];

        let (to_summarize, to_keep) = split_messages_for_compaction(&messages);

        assert_eq!(to_summarize.len(), 4, "the server use's message joins the kept tail");
        assert_eq!(to_keep[0].role, MessageRole::Assistant);
        assert!(server_use_ids(&to_keep[0]).any(|id| id == "srv1"));
        assert!(to_keep.iter().any(|m| server_result_use_ids(m).any(|id| id == "srv1")));
    }

    #[test]
    fn a_plain_history_still_keeps_exactly_the_last_six_messages() {
        let messages: Vec<Message> = (0..10)
            .map(|i| Message::user(format!("Message {i}")))
            .collect();

        let (to_summarize, to_keep) = split_messages_for_compaction(&messages);

        assert_eq!(to_summarize.len(), 4);
        assert_eq!(to_keep.len(), MESSAGES_TO_KEEP);
    }

    #[test]
    fn the_cut_keeps_a_run_of_consecutive_assistant_messages_together() {
        // A pause_turn chain (or a provider server-tool use + its next-message
        // result) is stored as consecutive assistant messages. A naive cut at
        // index 6 would keep chain2/chain3 and summarise chain1 (with the use).
        let messages = vec![
            Message::user("u0"),
            Message::user("u1"),
            Message::user("u2"),
            Message::user("u3"),
            Message::user("u4"),
            Message::assistant("chain1"),
            Message::assistant("chain2"),
            Message::assistant("chain3"),
            Message::user("u5"),
            Message::assistant("a5"),
            Message::user("u6"),
            Message::assistant("a6"),
        ];

        let (to_summarize, to_keep) = split_messages_for_compaction(&messages);

        assert_eq!(to_summarize.len(), 5, "the whole chain joins the kept tail");
        assert_eq!(to_keep[0].content, "chain1");
        assert_eq!(to_keep[1].content, "chain2");
        assert_eq!(to_keep[2].content, "chain3");
    }

    #[test]
    fn a_history_with_no_non_tool_message_to_keep_is_not_compacted() {
        let messages: Vec<Message> = (0..10)
            .map(|i| Message::tool_result(format!("tc_{i}"), "query_datasource", "r"))
            .collect();

        let (to_summarize, to_keep) = split_messages_for_compaction(&messages);

        assert!(to_summarize.is_empty(), "no safe cut: every message is a tool result");
        assert_eq!(to_keep.len(), messages.len());
    }

    #[test]
    fn an_old_unpaired_tool_result_moves_into_the_summarised_part() {
        // The first message is a tool result with no call (broken history), but
        // the kept tail still starts at a non-tool message, so the cut is safe.
        let mut messages = vec![Message::tool_result("tc_1", "query_datasource", "one")];
        messages.extend((0..9).map(|i| Message::user(format!("Message {i}"))));

        let (to_summarize, to_keep) = split_messages_for_compaction(&messages);

        assert_eq!(to_summarize.len(), 4);
        assert_eq!(to_keep[0].role, MessageRole::User);
        assert!(to_keep.iter().all(|m| m.role != MessageRole::Tool));
    }

    // -- Message type preservation in compaction prompt ------------------------

    #[test]
    fn compaction_prompt_includes_tool_call_names() {
        let mut assistant_msg = Message::assistant("Let me query the database.");
        assistant_msg.tool_calls = Some(vec![ToolCall {
            id: "tc_1".into(),
            name: "query_datasource".into(),
            arguments: serde_json::json!({"sql": "SELECT 1"}),
            arguments_error: None,
        }]);

        let messages = vec![
            Message::user("What is our revenue?"),
            assistant_msg,
            Message::tool_result("tc_1", "query_datasource", "Revenue: $1.2M"),
        ];

        let prompt = build_compaction_prompt(&messages);

        assert!(prompt.contains("USER"));
        assert!(prompt.contains("ASSISTANT"));
        assert!(prompt.contains("TOOL RESULT (query_datasource)"));
        assert!(prompt.contains("Called tool: query_datasource"));
        assert!(prompt.contains("What is our revenue?"));
        assert!(prompt.contains("Revenue: $1.2M"));
    }

    #[test]
    fn compaction_prompt_includes_multiple_tool_calls() {
        let mut msg = Message::assistant("Checking data.");
        msg.tool_calls = Some(vec![
            ToolCall {
                id: "tc_1".into(),
                name: "search_catalog".into(),
                arguments: serde_json::json!({"query": "revenue"}),
                arguments_error: None,
            },
            ToolCall {
                id: "tc_2".into(),
                name: "query_datasource".into(),
                arguments: serde_json::json!({"sql": "SELECT 1"}),
                arguments_error: None,
            },
        ]);

        let messages = vec![msg];
        let prompt = build_compaction_prompt(&messages);

        assert!(prompt.contains("Called tool: search_catalog"));
        assert!(prompt.contains("Called tool: query_datasource"));
    }

    #[test]
    fn compaction_prompt_labels_system_messages() {
        let messages = vec![Message::system("You are a helpful assistant.")];
        let prompt = build_compaction_prompt(&messages);
        assert!(prompt.contains("**SYSTEM**:"));
    }

    #[test]
    fn compaction_prompt_tool_result_without_name() {
        let mut msg = Message::tool_result("tc_1", "my_tool", "result");
        msg.name = None; // Remove the name
        let messages = vec![msg];
        let prompt = build_compaction_prompt(&messages);
        assert!(prompt.contains("TOOL RESULT"));
    }


    // -- Compaction prompt for large tool results -----------------------------

    #[test]
    fn compaction_prompt_truncates_multiple_large_tool_results() {
        let long_result1 = "a".repeat(5000);
        let long_result2 = "b".repeat(5000);
        let messages = vec![
            Message::tool_result("tc_1", "tool_a", &long_result1),
            Message::tool_result("tc_2", "tool_b", &long_result2),
        ];

        let prompt = build_compaction_prompt(&messages);

        // Both should be truncated
        let truncation_count = prompt.matches("[truncated for summary]").count();
        assert_eq!(truncation_count, 2);
    }

    // -- should_compact with tool call arguments ------------------------------

    #[test]
    fn should_compact_includes_tool_call_argument_sizes() {
        // Create a message with large tool call arguments
        let large_args = serde_json::json!({"sql": "x".repeat(100_000)});
        let mut msg = Message::assistant("running query");
        msg.tool_calls = Some(vec![ToolCall {
            id: "tc_1".into(),
            name: "query_datasource".into(),
            arguments: large_args,
            arguments_error: None,
        }]);

        // With 100K chars in args alone, at 4 chars/token = 25K tokens
        // This is well below threshold, but ensures args ARE counted
        let result = should_compact(None, &[msg.clone()], 0, THRESHOLD);
        // 100K chars + some overhead < 600K chars needed for 150K tokens
        assert!(!result);

        // Now add enough system prompt to push over the edge
        // Need total of 600K chars = 150K tokens
        // Already have ~100K in args, need 500K more
        let result = should_compact(None, &[msg], 500_001, THRESHOLD);
        assert!(result);
    }


    // -- Role labels in compaction prompt -------------------------------------

    #[test]
    fn compaction_prompt_uses_correct_role_labels() {
        let messages = vec![
            Message::user("User message"),
            Message::assistant("Assistant message"),
            Message::system("System message"),
        ];

        let prompt = build_compaction_prompt(&messages);
        assert!(prompt.contains("**USER**:"));
        assert!(prompt.contains("**ASSISTANT**:"));
        assert!(prompt.contains("**SYSTEM**:"));
    }

    // -- UTF-8 safe truncation ------------------------------------------------

    #[test]
    fn floor_char_boundary_on_ascii() {
        assert_eq!(super::floor_char_boundary("hello world", 5), 5);
        assert_eq!(super::floor_char_boundary("hello", 10), 5); // beyond end
    }

    #[test]
    fn floor_char_boundary_on_multibyte() {
        // 'é' is 2 bytes (0xC3, 0xA9). "café" = [c, a, f, 0xC3, 0xA9] = 5 bytes
        let s = "café";
        assert_eq!(s.len(), 5);
        // Truncating at byte 4 lands inside 'é', should back up to byte 3
        assert_eq!(super::floor_char_boundary(s, 4), 3);
        // Truncating at byte 5 is at the end, fine
        assert_eq!(super::floor_char_boundary(s, 5), 5);
        // Truncating at byte 3 is right before 'é'
        assert_eq!(super::floor_char_boundary(s, 3), 3);
    }

    #[test]
    fn floor_char_boundary_on_emoji() {
        // '🎉' is 4 bytes. "hi🎉" = [h, i, 0xF0, 0x9F, 0x8E, 0x89] = 6 bytes
        let s = "hi🎉";
        assert_eq!(s.len(), 6);
        // Truncating at byte 3, 4, or 5 should back up to byte 2 (before emoji)
        assert_eq!(super::floor_char_boundary(s, 3), 2);
        assert_eq!(super::floor_char_boundary(s, 4), 2);
        assert_eq!(super::floor_char_boundary(s, 5), 2);
        // Truncating at byte 6 is the end
        assert_eq!(super::floor_char_boundary(s, 6), 6);
    }

    #[test]
    fn compaction_prompt_truncates_multibyte_tool_result_safely() {
        // Create a tool result with multi-byte characters near the 2000-byte boundary.
        // Each '🎉' is 4 bytes. 500 emojis = 2000 bytes exactly.
        // Add one more character to exceed the limit.
        let content: String = "🎉".repeat(500) + "extra";
        assert!(content.len() > 2000);

        let messages = vec![Message::tool_result("tc_1", "tool_a", &content)];
        let prompt = build_compaction_prompt(&messages);

        // Should not panic, and should contain the truncation marker
        assert!(prompt.contains("[truncated for summary]"));
        // The full content should NOT be present
        assert!(!prompt.contains("extra"));
    }
}
