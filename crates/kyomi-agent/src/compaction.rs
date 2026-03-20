// SPDX-License-Identifier: AGPL-3.0-or-later

//! Context compaction for the agent system.
//!
//! When conversations approach the token limit (~200K context window),
//! old messages are summarized into a compact summary. This module provides
//! the utility functions for deciding when to compact, splitting messages,
//! building the summarization prompt, and reconstructing context.
//!
//! # Architecture
//!
//! The compaction flow:
//!
//! 1. After each LLM call, check [`should_compact`] using the actual
//!    input token count (or a character-based estimate as fallback).
//! 2. If compaction is needed, [`split_messages_for_compaction`] divides
//!    messages into "to summarize" and "to keep" groups.
//! 3. [`build_compaction_prompt`] creates a summarization prompt from the
//!    messages that need compacting.
//! 4. The caller sends this prompt to a fast model (e.g., Haiku).
//! 5. [`build_context_with_compaction`] reconstructs the LLM message list
//!    using the summary + recent messages.
//!
//! The agent's [`build_llm_context`](crate::agent::CustomAgent) method
//! already handles the compacted state during the agent loop -- this module
//! provides the preparation utilities.

use crate::types::{Message, MessageRole};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find the largest byte index `<= max_bytes` that lies on a UTF-8 character
/// boundary. Equivalent to `str::floor_char_boundary` (nightly-only as of
/// Rust 1.85).
fn floor_char_boundary(s: &str, max_bytes: usize) -> usize {
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

/// Token threshold for triggering compaction (~75% of 200K context window).
pub const COMPACTION_TOKEN_THRESHOLD: u32 = 150_000;

/// Rough character-to-token ratio for fallback estimation.
pub const CHARS_PER_TOKEN: u32 = 4;

/// Number of recent messages to keep uncompacted (last 3 user+assistant exchanges).
pub const MESSAGES_TO_KEEP: usize = 6;

// ---------------------------------------------------------------------------
// Public functions
// ---------------------------------------------------------------------------

/// Check whether the conversation context should be compacted.
///
/// Prefers the actual input token count from the last LLM call if available.
/// Falls back to a character-based estimate (total chars / [`CHARS_PER_TOKEN`])
/// that includes the system prompt length.
///
/// Returns `true` if the estimated or actual token count is at or above
/// [`COMPACTION_TOKEN_THRESHOLD`].
pub fn should_compact(
    last_input_tokens: Option<u32>,
    messages: &[Message],
    system_prompt_len: usize,
) -> bool {
    // Prefer actual token count from the LLM response.
    if let Some(actual) = last_input_tokens
        && actual > 0
    {
        return actual >= COMPACTION_TOKEN_THRESHOLD;
    }

    // Fallback: estimate from message content lengths.
    let mut total_chars: usize = system_prompt_len;

    for msg in messages {
        total_chars += msg.content.len();

        // Include tool call arguments in the estimate.
        if let Some(ref tool_calls) = msg.tool_calls {
            for tc in tool_calls {
                // Arguments are serde_json::Value -- serialized length is a
                // reasonable proxy for the token cost.
                total_chars += tc.arguments.to_string().len();
            }
        }
    }

    let estimated_tokens = total_chars as u32 / CHARS_PER_TOKEN;
    estimated_tokens >= COMPACTION_TOKEN_THRESHOLD
}

/// Split messages into (to_summarize, to_keep) for compaction.
///
/// - If `messages.len() <= MESSAGES_TO_KEEP`, all messages are kept and
///   the "to summarize" vec is empty.
/// - Otherwise, the last [`MESSAGES_TO_KEEP`] messages are kept and the
///   rest are returned for summarization.
///
/// **Note:** The input should NOT include the system prompt message --
/// the caller should strip it before calling this function and re-add it
/// when building the final context.
pub fn split_messages_for_compaction(messages: &[Message]) -> (Vec<Message>, Vec<Message>) {
    if messages.len() <= MESSAGES_TO_KEEP {
        return (Vec::new(), messages.to_vec());
    }

    let cutoff = messages.len() - MESSAGES_TO_KEEP;
    let to_summarize = messages[..cutoff].to_vec();
    let to_keep = messages[cutoff..].to_vec();

    (to_summarize, to_keep)
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

/// Build the LLM message list when a compacted summary is available.
///
/// Produces the following sequence:
///
/// 1. System message (`system_prompt`)
/// 2. User message with the prior conversation summary
/// 3. Assistant acknowledgment message
/// 4. All `recent_messages` (the [`MESSAGES_TO_KEEP`] messages)
///
/// This is the message list that should be sent to the LLM instead of
/// the full conversation history.
pub fn build_context_with_compaction(
    system_prompt: &str,
    compacted_summary: &str,
    recent_messages: &[Message],
) -> Vec<Message> {
    let mut context = Vec::with_capacity(3 + recent_messages.len());

    // 1. System prompt.
    context.push(Message::system(system_prompt));

    // 2. Compacted summary as user message.
    context.push(Message::user(format!(
        "## Prior Conversation Context\n\n{compacted_summary}"
    )));

    // 3. Assistant acknowledgment.
    context.push(Message::assistant(
        "I understand the context from our prior conversation. \
         I'll continue from where we left off, keeping in mind the data sources, \
         queries, and insights we've already explored. How can I help you next?",
    ));

    // 4. Recent messages.
    context.extend_from_slice(recent_messages);

    context
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- should_compact tests ------------------------------------------------

    #[test]
    fn should_compact_returns_false_below_threshold_with_actual_tokens() {
        // 100K tokens is well below the 150K threshold.
        let result = should_compact(Some(100_000), &[], 0);
        assert!(!result);
    }

    #[test]
    fn should_compact_returns_true_at_threshold_with_actual_tokens() {
        // Exactly at threshold should trigger compaction.
        let result = should_compact(Some(COMPACTION_TOKEN_THRESHOLD), &[], 0);
        assert!(result);
    }

    #[test]
    fn should_compact_returns_true_above_threshold_with_actual_tokens() {
        let result = should_compact(Some(COMPACTION_TOKEN_THRESHOLD + 10_000), &[], 0);
        assert!(result);
    }

    #[test]
    fn should_compact_fallback_estimation_below_threshold() {
        // With None for actual tokens, use character estimation.
        // 1000 chars / 4 = 250 tokens -- well below threshold.
        let messages = vec![Message::user("x".repeat(1000))];
        let result = should_compact(None, &messages, 0);
        assert!(!result);
    }

    #[test]
    fn should_compact_fallback_estimation_above_threshold() {
        // Need 150K tokens * 4 chars/token = 600K chars to trigger.
        // Create a message with enough content.
        let big_content = "x".repeat(600_001);
        let messages = vec![Message::user(big_content)];
        let result = should_compact(None, &messages, 0);
        assert!(result);
    }

    #[test]
    fn should_compact_fallback_includes_system_prompt_length() {
        // System prompt of 599_996 chars + message of 4 chars = 600_000 chars
        // 600_000 / 4 = 150_000 tokens = threshold.
        let messages = vec![Message::user("abcd")];
        let system_prompt_len = 599_996;
        let result = should_compact(None, &messages, system_prompt_len);
        assert!(result);
    }

    #[test]
    fn should_compact_zero_actual_tokens_uses_fallback() {
        // Some(0) should fall through to the estimation path.
        let messages = vec![Message::user("short message")];
        let result = should_compact(Some(0), &messages, 0);
        assert!(!result);
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

    // -- build_context_with_compaction tests ----------------------------------

    #[test]
    fn build_context_with_compaction_produces_correct_structure() {
        let recent = vec![
            Message::user("Recent question"),
            Message::assistant("Recent answer"),
        ];

        let context = build_context_with_compaction(
            "You are a data analyst.",
            "User explored revenue tables.",
            &recent,
        );

        // 1 system + 1 summary + 1 ack + 2 recent = 5
        assert_eq!(context.len(), 5);

        // System prompt.
        assert_eq!(context[0].role, MessageRole::System);
        assert_eq!(context[0].content, "You are a data analyst.");

        // Summary user message.
        assert_eq!(context[1].role, MessageRole::User);
        assert!(context[1].content.contains("Prior Conversation Context"));
        assert!(context[1]
            .content
            .contains("User explored revenue tables."));

        // Assistant acknowledgment.
        assert_eq!(context[2].role, MessageRole::Assistant);
        assert!(context[2]
            .content
            .contains("I understand the context from our prior conversation"));

        // Recent messages.
        assert_eq!(context[3].content, "Recent question");
        assert_eq!(context[4].content, "Recent answer");
    }

    #[test]
    fn build_context_with_compaction_empty_recent_messages() {
        let context = build_context_with_compaction(
            "System prompt.",
            "Summary of prior conversation.",
            &[],
        );

        // 1 system + 1 summary + 1 ack = 3 (no recent messages)
        assert_eq!(context.len(), 3);
        assert_eq!(context[0].role, MessageRole::System);
        assert_eq!(context[1].role, MessageRole::User);
        assert_eq!(context[2].role, MessageRole::Assistant);
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
        assert_eq!(COMPACTION_TOKEN_THRESHOLD, 150_000);
        assert_eq!(CHARS_PER_TOKEN, 4);
        assert_eq!(MESSAGES_TO_KEEP, 6);

        // Threshold should be less than 200K context window.
        assert!(COMPACTION_TOKEN_THRESHOLD < 200_000);
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

    // -- Message type preservation in compaction prompt ------------------------

    #[test]
    fn compaction_prompt_includes_tool_call_names() {
        let mut assistant_msg = Message::assistant("Let me query the database.");
        assistant_msg.tool_calls = Some(vec![ToolCall {
            id: "tc_1".into(),
            name: "query_datasource".into(),
            arguments: serde_json::json!({"sql": "SELECT 1"}),
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
            },
            ToolCall {
                id: "tc_2".into(),
                name: "query_datasource".into(),
                arguments: serde_json::json!({"sql": "SELECT 1"}),
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

    // -- System prompt not duplicated in build_context -------------------------

    #[test]
    fn build_context_has_exactly_one_system_message() {
        let recent = vec![
            Message::user("Question"),
            Message::assistant("Answer"),
        ];

        let context = build_context_with_compaction(
            "System prompt here.",
            "Summary of conversation.",
            &recent,
        );

        let system_count = context
            .iter()
            .filter(|m| m.role == MessageRole::System)
            .count();
        assert_eq!(
            system_count, 1,
            "Should have exactly one system message, got {system_count}"
        );
    }

    #[test]
    fn build_context_system_prompt_is_first_message() {
        let context = build_context_with_compaction(
            "My system prompt.",
            "Summary.",
            &[Message::user("Hi")],
        );

        assert_eq!(context[0].role, MessageRole::System);
        assert_eq!(context[0].content, "My system prompt.");
    }

    // -- Empty summary handling -----------------------------------------------

    #[test]
    fn build_context_with_empty_summary() {
        let context = build_context_with_compaction(
            "System.",
            "",
            &[Message::user("Hello")],
        );

        // Should still produce the structure even with empty summary
        assert_eq!(context.len(), 4); // system + summary + ack + 1 recent
        assert_eq!(context[1].role, MessageRole::User);
        assert!(context[1].content.contains("Prior Conversation Context"));
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
        }]);

        // With 100K chars in args alone, at 4 chars/token = 25K tokens
        // This is well below threshold, but ensures args ARE counted
        let result = should_compact(None, &[msg.clone()], 0);
        // 100K chars + some overhead < 600K chars needed for 150K tokens
        assert!(!result);

        // Now add enough system prompt to push over the edge
        // Need total of 600K chars = 150K tokens
        // Already have ~100K in args, need 500K more
        let result = should_compact(None, &[msg], 500_001);
        assert!(result);
    }

    // -- Context message ordering ---------------------------------------------

    #[test]
    fn build_context_preserves_recent_message_order() {
        let recent = vec![
            Message::user("First question"),
            Message::assistant("First answer"),
            Message::user("Second question"),
            Message::assistant("Second answer"),
        ];

        let context = build_context_with_compaction(
            "System.",
            "Summary.",
            &recent,
        );

        // Recent messages start at index 3 (after system + summary + ack)
        assert_eq!(context[3].content, "First question");
        assert_eq!(context[4].content, "First answer");
        assert_eq!(context[5].content, "Second question");
        assert_eq!(context[6].content, "Second answer");
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
