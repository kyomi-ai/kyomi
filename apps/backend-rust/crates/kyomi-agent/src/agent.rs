// SPDX-License-Identifier: AGPL-3.0-or-later

//! Core agent loop with iterative tool execution.
//!
//! Implements [`CustomAgent`], the main orchestrator that manages the
//! conversation with the LLM, executes tool calls, validates ChartML
//! blocks, and handles cancellation.
//!
//! # Architecture
//!
//! The agent loop mirrors the Python `CustomAgent.chat()`:
//!
//! 1. Build user message with metadata prefix
//! 2. Enter iteration loop (up to `max_iterations`)
//! 3. Call LLM with conversation history and tool definitions
//! 4. If no tool calls: validate ChartML, return response
//! 5. If tool calls: execute each tool, add results, continue loop
//! 6. If a "final" tool is called and there is response content, return

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use regex::Regex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::provider::LLMProvider;
use crate::tools::{ToolContext, ToolFilter, ToolRegistry, FINAL_TOOL_NAMES};
use crate::types::{Message, Tool, ToolCall};

// ---------------------------------------------------------------------------
// Callback type aliases (to satisfy clippy::type_complexity)
// ---------------------------------------------------------------------------

/// Callback for thinking content: `(thinking_text)`.
type ThinkingCallback = Box<dyn Fn(&str) + Send + Sync>;

/// Callback for token usage: `(input_tokens, output_tokens, cost)`.
type TokenUsageCallback = Box<dyn Fn(u32, u32, Option<f64>) + Send + Sync>;

/// Callback for tool start: `(tool_name, arguments)`.
type ToolStartCallback = Box<dyn Fn(&str, &serde_json::Value) + Send + Sync>;

/// Callback for tool end: `(tool_name, result, success)`.
type ToolEndCallback = Box<dyn Fn(&str, &str, bool) + Send + Sync>;

// ---------------------------------------------------------------------------
// AgentConfig
// ---------------------------------------------------------------------------

/// Configuration for the agent loop.
pub struct AgentConfig {
    /// Maximum number of LLM iterations before giving up.
    pub max_iterations: u32,
    /// Sampling temperature (0.0-1.0). `None` uses model default.
    pub temperature: Option<f32>,
    /// Maximum tokens to generate per LLM call.
    pub max_tokens: u32,
    /// Tool names that signal the loop should stop after execution.
    pub final_tool_names: HashSet<String>,
    /// Whether to log full LLM context (for debugging).
    pub log_context: bool,
    /// Filter controlling which tools are exposed to the LLM.
    ///
    /// Different agent contexts need different tool sets:
    /// - Chat/Slack: exclude copilot-only and MCP-only tools
    /// - Copilot: exclude MCP-only tools (copilot tools are available)
    /// - Watch execution: only data query tools via `include_only`
    /// - Trial chat: restricted subset via `include_only`
    pub tool_filter: ToolFilter,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 25,
            temperature: None,
            max_tokens: 4096,
            final_tool_names: FINAL_TOOL_NAMES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            log_context: false,
            // Default: chat context — exclude copilot-only and MCP-only tools.
            // This is the safe default matching the Python backend's behaviour.
            tool_filter: ToolFilter {
                exclude_copilot_only: true,
                exclude_mcp_only: true,
                include_only: None,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// AgentState
// ---------------------------------------------------------------------------

/// Mutable conversation state tracked across iterations.
#[derive(Default)]
pub struct AgentState {
    /// Full conversation message history.
    pub messages: Vec<Message>,
    /// Total number of LLM calls made across all `chat()` invocations.
    pub global_iteration: u32,
    /// Input tokens from the most recent LLM call.
    pub last_input_tokens: u32,
    /// Summary of earlier conversation (set after context compaction).
    pub compacted_summary: Option<String>,
    /// Index into `messages` marking where recent (post-compaction) messages start.
    pub messages_since_compaction_index: usize,
}

impl AgentState {
    /// Create initial state with an empty conversation.
    pub fn new() -> Self {
        Self::default()
    }
}

// ---------------------------------------------------------------------------
// AgentCallbacks
// ---------------------------------------------------------------------------

/// Callbacks for streaming agent progress to the caller.
///
/// All callbacks are optional. They are invoked synchronously from the
/// async agent loop, so implementations should be fast (e.g., send to
/// a channel rather than doing I/O).
#[derive(Default)]
pub struct AgentCallbacks {
    /// Called when the LLM emits thinking/reasoning content.
    pub on_thinking: Option<ThinkingCallback>,
    /// Called after each LLM call with (input_tokens, output_tokens, cost).
    pub on_token_usage: Option<TokenUsageCallback>,
    /// Called before a tool is executed with (tool_name, arguments).
    pub on_tool_start: Option<ToolStartCallback>,
    /// Called after a tool completes with (tool_name, result, success).
    pub on_tool_end: Option<ToolEndCallback>,
    /// Called when the agent is preparing its final response.
    pub on_preparing_response: Option<Box<dyn Fn() + Send + Sync>>,
}

// ---------------------------------------------------------------------------
// CustomAgent
// ---------------------------------------------------------------------------

/// The core agent that orchestrates LLM calls and tool execution.
///
/// Manages conversation state, calls the Anthropic API in a loop,
/// executes tools, validates ChartML blocks, and supports cancellation.
pub struct CustomAgent {
    /// LLM provider (Anthropic, OpenAI, Gemini, etc.).
    client: Box<dyn LLMProvider>,
    /// Agent configuration.
    config: AgentConfig,
    /// Conversation state.
    state: AgentState,
    /// Progress callbacks.
    callbacks: AgentCallbacks,
    /// Tool registry (shared, immutable after construction).
    registry: Arc<ToolRegistry>,
    /// Context passed to tool execution.
    tool_context: ToolContext,
    /// Map of user_id -> display name for message attribution.
    user_names: HashMap<String, String>,
}

impl CustomAgent {
    /// Create a new agent.
    pub fn new(
        client: Box<dyn LLMProvider>,
        config: AgentConfig,
        registry: Arc<ToolRegistry>,
        tool_context: ToolContext,
        user_names: HashMap<String, String>,
    ) -> Self {
        Self {
            client,
            config,
            state: AgentState::new(),
            callbacks: AgentCallbacks::default(),
            registry,
            tool_context,
            user_names,
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Read-only access to the agent state.
    pub fn state(&self) -> &AgentState {
        &self.state
    }

    /// Mutable access to the agent state (for loading history, compaction).
    pub fn state_mut(&mut self) -> &mut AgentState {
        &mut self.state
    }

    /// Read-only access to the agent configuration.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Mutable access to the callbacks (for setting up streaming).
    pub fn callbacks_mut(&mut self) -> &mut AgentCallbacks {
        &mut self.callbacks
    }

    // -----------------------------------------------------------------------
    // Chat (main entry point)
    // -----------------------------------------------------------------------

    /// Run the agent loop for a single user message.
    ///
    /// Returns the final text response from the LLM after zero or more
    /// tool execution iterations.
    ///
    /// # Arguments
    /// * `user_message` - The user's input text.
    /// * `cancel_token` - Token to signal cancellation (e.g., user disconnects).
    /// * `current_time_user_tz` - User's local time in ISO format (for metadata).
    /// * `message_source` - Source identifier (e.g., "web", "slack", "mcp").
    /// * `user_id` - User ID for attribution in shared conversations.
    ///
    /// # Errors
    /// Returns `kyomi_core::Error::Internal` on cancellation or LLM failures.
    pub async fn chat(
        &mut self,
        user_message: &str,
        cancel_token: CancellationToken,
        current_time_user_tz: Option<&str>,
        message_source: Option<&str>,
        user_id: Option<&str>,
    ) -> kyomi_core::Result<String> {
        // Build metadata prefix.
        let metadata_prefix = build_metadata_prefix(current_time_user_tz, message_source);

        // Build the full message content with metadata.
        let full_content = if metadata_prefix.is_empty() {
            user_message.to_string()
        } else {
            format!("{metadata_prefix}{user_message}")
        };

        // Create user message with optional user attribution.
        let msg = if let Some(uid) = user_id {
            Message::user_with_id(&full_content, uid)
        } else {
            Message::user(&full_content)
        };

        self.state.messages.push(msg);

        // Ephemeral messages from ChartML validation retries.  These are
        // included in the LLM context so the model can see what it got wrong,
        // but they are never added to `self.state.messages` and therefore
        // never persisted to the database.
        let mut chartml_retry_messages: Vec<Message> = Vec::new();

        // Iteration loop.
        for iteration in 0..self.config.max_iterations {
            // Check cancellation.
            if cancel_token.is_cancelled() {
                return Err(kyomi_core::Error::Internal(
                    "Request cancelled".into(),
                ));
            }

            self.state.global_iteration += 1;

            // Inject warning once at 80% of the iteration limit.
            let threshold = (self.config.max_iterations as f32 * 0.8) as u32;
            if iteration == threshold {
                let remaining = self.config.max_iterations - iteration;
                let warning = format!(
                    "\u{26a0}\u{fe0f} IMPORTANT: You have only {remaining} iterations remaining. \
                     Please wrap up your analysis and provide a final response soon."
                );
                self.state.messages.push(Message::user(warning));
            }

            // Build LLM context (handles compaction), then append any
            // ephemeral ChartML retry messages so the LLM sees them.
            let mut llm_messages = self.build_llm_context();
            llm_messages.extend(chartml_retry_messages.iter().cloned());

            // Get tool definitions.
            let tools = self.registry.get_tool_definitions(&self.config.tool_filter);

            if self.config.log_context {
                debug!(
                    message_count = llm_messages.len(),
                    tool_count = tools.len(),
                    iteration = iteration,
                    "agent loop: calling LLM"
                );
            }

            // Call the LLM with cancellation support.
            let response = tokio::select! {
                result = self.call_llm(&llm_messages, &tools) => result?,
                _ = cancel_token.cancelled() => {
                    return Err(kyomi_core::Error::Internal(
                        "Request cancelled".into(),
                    ));
                }
            };

            // Check cancellation after LLM response.
            if cancel_token.is_cancelled() {
                return Err(kyomi_core::Error::Internal(
                    "Request cancelled".into(),
                ));
            }

            // Track token usage.
            self.state.last_input_tokens = response.usage.input_tokens;
            if let Some(ref cb) = self.callbacks.on_token_usage {
                cb(
                    response.usage.input_tokens,
                    response.usage.output_tokens,
                    response.cost,
                );
            }

            // No tool calls -- this is the final response.
            if response.tool_calls.is_none() {
                if let Some(ref cb) = self.callbacks.on_preparing_response {
                    cb();
                }

                // Validate ChartML blocks if present (YAML + SQL dry-run).
                if has_chartml_blocks(&response.content)
                    && let Some(error_msg) = self.validate_chartml_blocks(&response.content).await
                {
                    warn!(error = %error_msg, "ChartML validation failed, asking LLM to fix");
                    // Store as ephemeral retry context — NOT in self.state.messages.
                    chartml_retry_messages.push(
                        Message::assistant(response.content.clone()),
                    );
                    chartml_retry_messages.push(Message::user(format!(
                        "\u{1f916} SYSTEM: Automatic ChartML validation failed. The user has NOT seen your response yet. \
                         Please fix the following errors and then repeat your FULL response:\n\n{error_msg}"
                    )));
                    continue;
                }

                return Ok(response.content);
            }

            // Has tool calls -- process them.

            // If there were pending retry messages, the LLM has moved on to
            // tool calls, so the retry context is no longer relevant.  Clear
            // it so it doesn't accumulate stale context.
            chartml_retry_messages.clear();

            // Fire thinking callback if there is content alongside tool calls.
            if !response.content.is_empty()
                && let Some(ref cb) = self.callbacks.on_thinking
            {
                cb(&response.content);
            }

            // Add assistant message with tool calls.
            // Safety: guarded by `response.tool_calls.is_none()` early-return above.
            let tool_calls = response.tool_calls.expect("guarded by is_none check above");
            self.state.messages.push(Message::assistant_with_tool_calls(
                response.content.clone(),
                tool_calls.clone(),
            ));

            // Check cancellation before tool execution.
            if cancel_token.is_cancelled() {
                return Err(kyomi_core::Error::Internal(
                    "Request cancelled".into(),
                ));
            }

            // Execute each tool call.
            let mut has_final_tool = false;
            for tool_call in &tool_calls {
                let result = self.execute_tool(tool_call).await;
                self.state.messages.push(Message::tool_result(
                    &tool_call.id,
                    &tool_call.name,
                    &result,
                ));

                if self.config.final_tool_names.contains(&tool_call.name) {
                    has_final_tool = true;
                }
            }

            // Check cancellation after tool execution.
            if cancel_token.is_cancelled() {
                return Err(kyomi_core::Error::Internal(
                    "Request cancelled".into(),
                ));
            }

            // Check if assistant content has ChartML blocks -> validate (YAML + SQL) -> return if valid.
            if has_chartml_blocks(&response.content) {
                if let Some(error_msg) = self.validate_chartml_blocks(&response.content).await {
                    // Validation failed — store error as ephemeral retry context.
                    // The assistant message (with tool calls) is already persisted
                    // above, but the error instruction stays ephemeral.
                    warn!(error = %error_msg, "ChartML validation failed in tool response, asking LLM to fix");
                    chartml_retry_messages.push(Message::user(format!(
                        "\u{1f916} SYSTEM: Automatic ChartML validation failed. The user has NOT seen your response yet. \
                         Please fix the following errors and then repeat your FULL response:\n\n{error_msg}"
                    )));
                    continue;
                }

                // Validation passed — return as final response.
                if let Some(ref cb) = self.callbacks.on_preparing_response {
                    cb();
                }
                return Ok(response.content);
            }

            // If a final tool was called, try to extract final content.
            if has_final_tool {
                let final_content = if !response.content.trim().is_empty() {
                    // Use the text content from the response.
                    Some(response.content.clone())
                } else {
                    // No text content — check for 'summary' arg in the final tool call.
                    tool_calls
                        .iter()
                        .filter(|tc| self.config.final_tool_names.contains(&tc.name))
                        .find_map(|tc| {
                            tc.arguments
                                .get("summary")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        })
                };

                if let Some(content) = final_content {
                    info!("final tool called, using extracted content as response");
                    if let Some(ref cb) = self.callbacks.on_preparing_response {
                        cb();
                    }
                    return Ok(content);
                }
            }
        }

        // Exhausted all iterations.
        info!(
            max_iterations = self.config.max_iterations,
            "agent loop exhausted max iterations"
        );
        Ok(
            "I apologize, but I've reached my maximum number of iterations for this request. \
             Please try rephrasing your question or breaking it into smaller parts."
                .to_string(),
        )
    }

    // -----------------------------------------------------------------------
    // Internal: LLM call
    // -----------------------------------------------------------------------

    /// Call the Anthropic API with the current context.
    async fn call_llm(
        &self,
        messages: &[Message],
        tools: &[Tool],
    ) -> kyomi_core::Result<crate::types::LLMResponse> {
        self.client
            .complete(
                messages,
                tools,
                self.config.temperature,
                self.config.max_tokens,
                &self.user_names,
            )
            .await
    }

    // -----------------------------------------------------------------------
    // Internal: Tool execution
    // -----------------------------------------------------------------------

    /// Execute a single tool call and return the result string.
    async fn execute_tool(&self, tool_call: &ToolCall) -> String {
        let Some(tool) = self.registry.get_tool(&tool_call.name) else {
            let available = self.registry.tool_names().join(", ");
            warn!(tool = %tool_call.name, available = %available, "unknown tool requested by LLM");
            return format!(
                "Error: Unknown tool '{}'. Available tools: {}",
                tool_call.name, available
            );
        };

        if let Some(ref cb) = self.callbacks.on_tool_start {
            cb(&tool_call.name, &tool_call.arguments);
        }

        match tool
            .execute(tool_call.arguments.clone(), &self.tool_context)
            .await
        {
            Ok(result) => {
                if let Some(ref cb) = self.callbacks.on_tool_end {
                    cb(&tool_call.name, &result, true);
                }
                result
            }
            Err(e) => {
                let error_msg = format!("Tool '{}' failed: {}", tool_call.name, e);
                warn!(tool = %tool_call.name, error = %e, "tool execution failed");
                if let Some(ref cb) = self.callbacks.on_tool_end {
                    cb(&tool_call.name, &error_msg, false);
                }
                error_msg
            }
        }
    }

    // -----------------------------------------------------------------------
    // Internal: LLM context building
    // -----------------------------------------------------------------------

    /// Build the message list to send to the LLM.
    ///
    /// If no compaction has occurred, returns the full message history.
    /// If compacted, returns the compacted summary followed by recent messages.
    fn build_llm_context(&self) -> Vec<Message> {
        let Some(ref summary) = self.state.compacted_summary else {
            return self.state.messages.clone();
        };

        let mut context = Vec::new();

        // Preserve system prompt if present.
        if let Some(first) = self.state.messages.first()
            && first.role == crate::types::MessageRole::System
        {
            context.push(first.clone());
        }

        // Add compacted summary as a user message.
        context.push(Message::user(format!(
            "## Prior Conversation Context\n{summary}\n\n---\n\
             The above summarizes our earlier conversation. Continue from here."
        )));

        // Add acknowledgment as an assistant message.
        context.push(Message::assistant(
            "I understand the context from our previous conversation. \
             I'll continue from where we left off.",
        ));

        // Add recent messages (post-compaction).
        let recent_start = self.state.messages_since_compaction_index;
        if recent_start < self.state.messages.len() {
            context.extend_from_slice(&self.state.messages[recent_start..]);
        }

        context
    }

    // -----------------------------------------------------------------------
    // Internal: ChartML validation with SQL dry-run
    // -----------------------------------------------------------------------

    /// Validate ChartML blocks including SQL dry-run against actual datasources.
    ///
    /// First runs YAML structure validation (required keys). If that passes,
    /// extracts SQL queries and datasource slugs from each block and runs a
    /// dry-run against the real provider to catch invalid SQL before the user
    /// sees it.
    ///
    /// Returns `None` if all blocks are valid, or `Some(error_message)` if
    /// any block fails validation.
    async fn validate_chartml_blocks(&self, text: &str) -> Option<String> {
        // Step 1: YAML structure validation (fast, synchronous).
        if let Some(yaml_errors) = validate_chartml_blocks(text) {
            return Some(yaml_errors);
        }

        // Trial mode: skip SQL dry-run (datasources are not in PostgreSQL,
        // /trial/query handles execution directly via sample ClickHouse).
        if self.tool_context.is_trial {
            return None;
        }

        // Step 2: SQL dry-run via shared utility (same code path as dashboard tools).
        crate::tools::query_utils::validate_chartml_sql(
            &self.tool_context.query_context(),
            text,
        )
        .await
    }

}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the metadata prefix for a user message.
///
/// Format: `[source: X, user_local_time: Y] ` -- only if values are present.
fn build_metadata_prefix(
    current_time_user_tz: Option<&str>,
    message_source: Option<&str>,
) -> String {
    let mut parts = Vec::new();

    if let Some(source) = message_source {
        parts.push(format!("source: {source}"));
    }
    if let Some(time) = current_time_user_tz {
        parts.push(format!("user_local_time: {time}"));
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!("[{}] ", parts.join(", "))
    }
}

/// Check if text contains ChartML code blocks.
fn has_chartml_blocks(text: &str) -> bool {
    text.contains("```chartml")
}

/// Validate ChartML blocks in the text (YAML structure only, no SQL).
///
/// Extracts all ` ```chartml ... ``` ` blocks, parses each as YAML, and
/// checks for required keys (`data` and `visualize`).
///
/// Returns `None` if all blocks are valid, or `Some(error_message)` if
/// any block fails validation.
fn validate_chartml_blocks(text: &str) -> Option<String> {
    static CHARTML_RE: OnceLock<Regex> = OnceLock::new();
    let re = CHARTML_RE.get_or_init(|| Regex::new(r"```chartml\s*\n([\s\S]*?)\n```").expect("valid regex literal"));
    let mut errors = Vec::new();

    let mut found_any = false;
    for (i, cap) in re.captures_iter(text).enumerate() {
        found_any = true;
        let block_content = &cap[1];

        // Try to parse as YAML.
        let parsed: Result<serde_yaml::Value, _> = serde_yaml::from_str(block_content);
        match parsed {
            Ok(value) => {
                // Check for required keys.
                let mapping = value.as_mapping();
                let data_key = serde_yaml::Value::String("data".to_string());
                let visualize_key = serde_yaml::Value::String("visualize".to_string());
                let has_data = mapping
                    .map(|m| m.contains_key(&data_key))
                    .unwrap_or(false);
                let has_visualize = mapping
                    .map(|m| m.contains_key(&visualize_key))
                    .unwrap_or(false);

                if !has_data {
                    errors.push(format!("Block {}: missing required key 'data'", i + 1));
                }
                if !has_visualize {
                    errors.push(format!(
                        "Block {}: missing required key 'visualize'",
                        i + 1
                    ));
                }
            }
            Err(e) => {
                errors.push(format!("Block {}: invalid YAML: {}", i + 1, e));
            }
        }
    }

    if !found_any {
        // No blocks found despite has_chartml_blocks returning true.
        // This could happen with malformed blocks (no closing ```).
        return None;
    }

    if errors.is_empty() {
        None
    } else {
        Some(errors.join("; "))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Metadata prefix tests -----------------------------------------------

    #[test]
    fn metadata_prefix_both_present() {
        let result = build_metadata_prefix(Some("2025-01-15T10:30:00+11:00"), Some("web"));
        assert_eq!(
            result,
            "[source: web, user_local_time: 2025-01-15T10:30:00+11:00] "
        );
    }

    #[test]
    fn metadata_prefix_source_only() {
        let result = build_metadata_prefix(None, Some("slack"));
        assert_eq!(result, "[source: slack] ");
    }

    #[test]
    fn metadata_prefix_time_only() {
        let result = build_metadata_prefix(Some("2025-01-15T10:30:00+11:00"), None);
        assert_eq!(
            result,
            "[user_local_time: 2025-01-15T10:30:00+11:00] "
        );
    }

    #[test]
    fn metadata_prefix_both_missing() {
        let result = build_metadata_prefix(None, None);
        assert_eq!(result, "");
    }

    // -- ChartML helpers tests -----------------------------------------------

    #[test]
    fn has_chartml_blocks_positive() {
        let text = "Here is a chart:\n```chartml\ntype: chart\n```\nDone.";
        assert!(has_chartml_blocks(text));
    }

    #[test]
    fn has_chartml_blocks_negative() {
        let text = "Here is some code:\n```python\nprint('hello')\n```";
        assert!(!has_chartml_blocks(text));
    }

    #[test]
    fn has_chartml_blocks_empty() {
        assert!(!has_chartml_blocks(""));
    }

    #[test]
    fn has_chartml_blocks_partial_match() {
        // Contains the string but not as a code block opening.
        let text = "Use ```chartml syntax for charts.";
        assert!(has_chartml_blocks(text));
    }

    #[test]
    fn validate_chartml_blocks_valid() {
        let text = "Chart:\n```chartml\ndata:\n  query: SELECT 1\nvisualize:\n  type: bar\n```";
        assert!(validate_chartml_blocks(text).is_none());
    }

    #[test]
    fn validate_chartml_blocks_invalid_yaml() {
        let text = "Chart:\n```chartml\n{invalid: yaml: [:\n```";
        let result = validate_chartml_blocks(text);
        assert!(result.is_some());
        assert!(result.unwrap().contains("invalid YAML"));
    }

    #[test]
    fn validate_chartml_blocks_missing_data_key() {
        let text = "Chart:\n```chartml\nvisualize:\n  type: bar\n```";
        let result = validate_chartml_blocks(text);
        assert!(result.is_some());
        assert!(result.unwrap().contains("missing required key 'data'"));
    }

    #[test]
    fn validate_chartml_blocks_missing_visualize_key() {
        let text = "Chart:\n```chartml\ndata:\n  query: SELECT 1\n```";
        let result = validate_chartml_blocks(text);
        assert!(result.is_some());
        assert!(
            result.unwrap().contains("missing required key 'visualize'")
        );
    }

    #[test]
    fn validate_chartml_blocks_missing_both_keys() {
        let text = "Chart:\n```chartml\ntitle: My Chart\n```";
        let result = validate_chartml_blocks(text);
        assert!(result.is_some());
        let err = result.unwrap();
        assert!(err.contains("missing required key 'data'"));
        assert!(err.contains("missing required key 'visualize'"));
    }

    #[test]
    fn validate_chartml_blocks_multiple_blocks_one_invalid() {
        let text = "\
Chart 1:\n```chartml\ndata:\n  query: SELECT 1\nvisualize:\n  type: bar\n```\n\
Chart 2:\n```chartml\ntitle: Bad\n```";
        let result = validate_chartml_blocks(text);
        assert!(result.is_some());
        let err = result.unwrap();
        // First block is valid, second is missing keys.
        assert!(err.contains("Block 2"));
    }

    #[test]
    fn validate_chartml_blocks_no_blocks() {
        let text = "No chartml here.";
        assert!(validate_chartml_blocks(text).is_none());
    }

    #[test]
    fn validate_chartml_blocks_partial_closing_fence() {
        // ```chartml block without a closing ``` — the regex won't match, so
        // has_chartml_blocks returns true but validate_chartml_blocks finds no
        // captures and returns None (no error).
        let text = "Chart:\n```chartml\ndata:\n  query: SELECT 1\nvisualize:\n  type: bar\n";
        assert!(has_chartml_blocks(text));
        // Regex requires `\n` ``` ` so unclosed blocks are not captured.
        assert!(validate_chartml_blocks(text).is_none());
    }

    // -- build_llm_context tests ---------------------------------------------

    #[test]
    fn build_llm_context_without_compaction() {
        let state = AgentState {
            messages: vec![
                Message::system("You are helpful."),
                Message::user("Hello"),
                Message::assistant("Hi there!"),
            ],
            compacted_summary: None,
            ..Default::default()
        };

        // Without compaction, build_llm_context returns all messages.
        // We verify the logic directly since constructing a full CustomAgent
        // requires a ToolContext with real DB/Redis connections.
        assert!(state.compacted_summary.is_none());
        let context = state.messages.clone();
        assert_eq!(context.len(), 3);
        assert_eq!(context[0].content, "You are helpful.");
        assert_eq!(context[1].content, "Hello");
        assert_eq!(context[2].content, "Hi there!");
    }

    #[test]
    fn build_llm_context_with_compaction() {
        let state = AgentState {
            messages: vec![
                Message::system("You are helpful."),
                Message::user("Old message 1"),
                Message::assistant("Old response 1"),
                Message::user("Recent message"),
                Message::assistant("Recent response"),
            ],
            global_iteration: 5,
            last_input_tokens: 1000,
            compacted_summary: Some("User asked about revenue. Agent queried the database.".into()),
            messages_since_compaction_index: 3,
        };

        // Simulate build_llm_context logic (same as the method).
        let summary = state.compacted_summary.as_ref().unwrap();
        let mut context = Vec::new();

        // System prompt.
        if let Some(first) = state.messages.first()
            && first.role == crate::types::MessageRole::System
        {
            context.push(first.clone());
        }

        // Compacted summary.
        context.push(Message::user(format!(
            "## Prior Conversation Context\n{summary}\n\n---\n\
             The above summarizes our earlier conversation. Continue from here."
        )));

        // Acknowledgment.
        context.push(Message::assistant(
            "I understand the context from our previous conversation. \
             I'll continue from where we left off.",
        ));

        // Recent messages.
        context.extend_from_slice(&state.messages[state.messages_since_compaction_index..]);

        // Verify structure.
        assert_eq!(context.len(), 5); // system + summary + ack + 2 recent
        assert_eq!(context[0].content, "You are helpful.");
        assert!(context[1].content.contains("Prior Conversation Context"));
        assert!(context[1]
            .content
            .contains("User asked about revenue"));
        assert!(context[2]
            .content
            .contains("I understand the context"));
        assert_eq!(context[3].content, "Recent message");
        assert_eq!(context[4].content, "Recent response");
    }

    // -- AgentConfig tests ---------------------------------------------------

    #[test]
    fn agent_config_default_values() {
        let config = AgentConfig::default();
        assert_eq!(config.max_iterations, 25);
        assert!(config.temperature.is_none());
        assert_eq!(config.max_tokens, 4096);
        assert!(!config.log_context);
        assert!(config.final_tool_names.contains("save_learning"));
    }

    // -- AgentCallbacks tests ------------------------------------------------

    #[test]
    fn agent_callbacks_default_all_none() {
        let callbacks = AgentCallbacks::default();
        assert!(callbacks.on_thinking.is_none());
        assert!(callbacks.on_token_usage.is_none());
        assert!(callbacks.on_tool_start.is_none());
        assert!(callbacks.on_tool_end.is_none());
        assert!(callbacks.on_preparing_response.is_none());
    }

    // -- AgentState tests ----------------------------------------------------

    #[test]
    fn agent_state_new_is_empty() {
        let state = AgentState::new();
        assert!(state.messages.is_empty());
        assert_eq!(state.global_iteration, 0);
        assert_eq!(state.last_input_tokens, 0);
        assert!(state.compacted_summary.is_none());
        assert_eq!(state.messages_since_compaction_index, 0);
    }

    #[test]
    fn agent_state_default_same_as_new() {
        let state = AgentState::default();
        assert!(state.messages.is_empty());
        assert_eq!(state.global_iteration, 0);
    }

    // -- Contract: AgentConfig default final_tool_names ----------------------

    #[test]
    fn agent_config_final_tool_names_contains_save_learning() {
        let config = AgentConfig::default();
        assert!(config.final_tool_names.contains("save_learning"));
    }

    #[test]
    fn agent_config_final_tool_names_matches_constant() {
        // Verify the default config final_tool_names contains all items from FINAL_TOOL_NAMES.
        let config = AgentConfig::default();
        for name in FINAL_TOOL_NAMES {
            assert!(
                config.final_tool_names.contains(*name),
                "FINAL_TOOL_NAMES '{name}' missing from AgentConfig::default()"
            );
        }
    }

    // -- Contract: Metadata prefix ordering ---------------------------------

    #[test]
    fn metadata_prefix_source_comes_before_time() {
        let result = build_metadata_prefix(Some("2025-01-15T10:00:00+00:00"), Some("web"));
        // Source must come before time in the prefix.
        let source_pos = result.find("source:").unwrap();
        let time_pos = result.find("user_local_time:").unwrap();
        assert!(source_pos < time_pos);
    }

    #[test]
    fn metadata_prefix_has_trailing_space() {
        let result = build_metadata_prefix(Some("2025-01-15T10:00:00+00:00"), Some("web"));
        assert!(result.ends_with("] "));
    }

    #[test]
    fn metadata_prefix_slack_source() {
        let result = build_metadata_prefix(None, Some("slack"));
        assert_eq!(result, "[source: slack] ");
    }

    #[test]
    fn metadata_prefix_mcp_source() {
        let result = build_metadata_prefix(None, Some("mcp"));
        assert_eq!(result, "[source: mcp] ");
    }

    // -- Contract: ChartML validation edge cases ----------------------------

    #[test]
    fn validate_chartml_blocks_valid_with_extra_keys() {
        // Additional keys beyond data and visualize are allowed.
        let text = "Chart:\n```chartml\ntitle: Revenue\ndata:\n  query: SELECT 1\nvisualize:\n  type: bar\nlayout:\n  colSpan: 6\n```";
        assert!(validate_chartml_blocks(text).is_none());
    }

    #[test]
    fn validate_chartml_blocks_multiple_valid_blocks() {
        let text = "\
Chart 1:\n```chartml\ndata:\n  query: SELECT 1\nvisualize:\n  type: bar\n```\n\
Chart 2:\n```chartml\ndata:\n  query: SELECT 2\nvisualize:\n  type: line\n```";
        assert!(validate_chartml_blocks(text).is_none());
    }

    #[test]
    fn validate_chartml_blocks_empty_yaml() {
        // A chartml block with just whitespace should fail (missing required keys).
        let text = "```chartml\n  \n```";
        // This either produces an error or returns None (empty content might not match regex).
        // Validate we don't panic.
        let _result = validate_chartml_blocks(text);
    }

    #[test]
    fn has_chartml_blocks_case_sensitive() {
        // Must be exactly ```chartml, not ```ChartML.
        assert!(!has_chartml_blocks("```ChartML\ndata:\n  query: SELECT 1\n```"));
        assert!(!has_chartml_blocks("```CHARTML\ndata:\n  query: SELECT 1\n```"));
    }

    #[test]
    fn validate_chartml_blocks_malformed_closing_fence() {
        // If there is no closing ``` after ```chartml, the regex won't match.
        let text = "```chartml\ndata:\n  query: SELECT 1\nvisualize:\n  type: bar";
        let result = validate_chartml_blocks(text);
        // Should return None (no blocks found by the regex).
        assert!(result.is_none());
    }

    // -- Contract: build_llm_context without system prompt -------------------

    #[test]
    fn build_llm_context_compaction_without_system_prompt() {
        // When there is no system prompt at the start of messages,
        // compaction should still work correctly.
        let state = AgentState {
            messages: vec![
                Message::user("Old question"),
                Message::assistant("Old answer"),
                Message::user("New question"),
            ],
            compacted_summary: Some("Previous discussion about revenue.".into()),
            messages_since_compaction_index: 2,
            ..Default::default()
        };

        // Simulate the build_llm_context logic.
        let summary = state.compacted_summary.as_ref().unwrap();
        let mut context = Vec::new();

        // First message is a User, not System, so no system prompt preserved.
        if let Some(first) = state.messages.first()
            && first.role == crate::types::MessageRole::System
        {
            context.push(first.clone());
        }

        context.push(Message::user(format!(
            "## Prior Conversation Context\n{summary}\n\n---\n\
             The above summarizes our earlier conversation. Continue from here."
        )));
        context.push(Message::assistant(
            "I understand the context from our previous conversation. \
             I'll continue from where we left off.",
        ));
        context.extend_from_slice(&state.messages[state.messages_since_compaction_index..]);

        // 3 messages: summary + ack + 1 recent (no system prompt).
        assert_eq!(context.len(), 3);
        assert!(context[0].content.contains("Prior Conversation Context"));
        assert_eq!(context[2].content, "New question");
    }

    // -- Contract: AgentState mutation --------------------------------------

    #[test]
    fn agent_state_messages_can_be_pushed() {
        let mut state = AgentState::new();
        state.messages.push(Message::system("prompt"));
        state.messages.push(Message::user("hello"));
        assert_eq!(state.messages.len(), 2);
    }

    #[test]
    fn agent_state_global_iteration_increments() {
        let mut state = AgentState::new();
        assert_eq!(state.global_iteration, 0);
        state.global_iteration += 1;
        assert_eq!(state.global_iteration, 1);
    }

    // -- Contract: Iteration warning threshold calculation -------------------

    #[test]
    fn iteration_warning_threshold_at_80_percent() {
        let config = AgentConfig::default();
        let threshold = (config.max_iterations as f32 * 0.8) as u32;
        // 25 * 0.8 = 20.0
        assert_eq!(threshold, 20);
    }

    #[test]
    fn iteration_warning_threshold_custom_max() {
        let config = AgentConfig {
            max_iterations: 10,
            ..Default::default()
        };
        let threshold = (config.max_iterations as f32 * 0.8) as u32;
        assert_eq!(threshold, 8);
    }

    // -- Contract: Max iterations apology message ---------------------------

    #[test]
    fn max_iterations_apology_message_content() {
        // The apology message returned when iterations are exhausted.
        let apology = "I apologize, but I've reached my maximum number of iterations for this request. \
             Please try rephrasing your question or breaking it into smaller parts.";
        // Verify the key phrases that the frontend might rely on.
        assert!(apology.contains("maximum number of iterations"));
        assert!(apology.contains("rephrasing your question"));
    }

    // -- Contract: AgentCallbacks can be set ---------------------------------

    #[test]
    fn agent_callbacks_can_set_on_thinking() {
        let callbacks = AgentCallbacks {
            on_thinking: Some(Box::new(|_thought: &str| {})),
            ..Default::default()
        };
        assert!(callbacks.on_thinking.is_some());
    }

    #[test]
    fn agent_callbacks_can_set_on_token_usage() {
        let callbacks = AgentCallbacks {
            on_token_usage: Some(Box::new(
                |_input: u32, _output: u32, _cost: Option<f64>| {},
            )),
            ..Default::default()
        };
        assert!(callbacks.on_token_usage.is_some());
    }

    #[test]
    fn agent_callbacks_can_set_on_tool_start() {
        let callbacks = AgentCallbacks {
            on_tool_start: Some(Box::new(
                |_name: &str, _args: &serde_json::Value| {},
            )),
            ..Default::default()
        };
        assert!(callbacks.on_tool_start.is_some());
    }

    #[test]
    fn agent_callbacks_can_set_on_tool_end() {
        let callbacks = AgentCallbacks {
            on_tool_end: Some(Box::new(
                |_name: &str, _result: &str, _success: bool| {},
            )),
            ..Default::default()
        };
        assert!(callbacks.on_tool_end.is_some());
    }

    #[test]
    fn agent_callbacks_can_set_on_preparing_response() {
        let callbacks = AgentCallbacks {
            on_preparing_response: Some(Box::new(|| {})),
            ..Default::default()
        };
        assert!(callbacks.on_preparing_response.is_some());
    }

}
