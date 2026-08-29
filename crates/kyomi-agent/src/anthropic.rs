// SPDX-License-Identifier: AGPL-3.0-or-later

//! Direct HTTP client for the Anthropic Messages API.
//!
//! Calls `POST https://api.anthropic.com/v1/messages` using `reqwest`.
//! Handles prompt caching, retry logic with exponential backoff, and token
//! usage tracking.
//!
//! This is a direct HTTP implementation because no official Anthropic Rust SDK
//! exists. It gives full control over prompt caching headers, tool schemas,
//! and error handling.

use std::collections::HashMap;

use serde_json::json;
use tracing::{debug, info, warn};

use crate::types::{AgentTokenUsage, LLMResponse, Message, MessageRole, Tool, ToolCall};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Anthropic Messages API endpoint.
const ANTHROPIC_API_URL: &str = "https://api.anthropic.com";

/// Required API version header.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Beta header for prompt caching support.
const PROMPT_CACHING_BETA: &str = "prompt-caching-2024-07-31";

/// Default model for chat completions (matches Python's `get_chat_agent` default).
pub const DEFAULT_MODEL: &str = "claude-haiku-4-5-20251001";

/// Model used for lightweight audit tasks (e.g., learning validation).
pub const AUDIT_MODEL: &str = "claude-haiku-4-5-20251001";

// ---------------------------------------------------------------------------
// AnthropicClient
// ---------------------------------------------------------------------------

/// HTTP client for the Anthropic Messages API.
///
/// Handles message conversion, prompt caching, and retry logic.
/// Uses `reqwest` for HTTP calls and supports non-streaming mode (matching
/// the Python backend's behavior).
pub struct AnthropicClient {
    base: crate::provider::ProviderBase,
}

impl AnthropicClient {
    /// Create a new Anthropic client.
    ///
    /// # Arguments
    /// * `api_key` - Anthropic API key.
    /// * `model` - Model name; uses [`DEFAULT_MODEL`] if `None`.
    pub fn new(api_key: String, model: Option<String>) -> kyomi_core::Result<Self> {
        Ok(Self {
            base: crate::provider::ProviderBase::new(
                api_key,
                model,
                DEFAULT_MODEL,
                None,
                ANTHROPIC_API_URL,
            )?,
        })
    }

    /// Create a client pointing at a custom API URL.
    pub fn with_base_url(
        api_key: String,
        model: Option<String>,
        base_url: String,
    ) -> kyomi_core::Result<Self> {
        Ok(Self {
            base: crate::provider::ProviderBase::with_base_url(
                api_key,
                model,
                DEFAULT_MODEL,
                base_url,
            )?,
        })
    }

    /// Return the model name this client is configured with.
    pub fn model(&self) -> &str {
        self.base.model()
    }

    // -----------------------------------------------------------------------
    // Message & Tool Conversion
    // -----------------------------------------------------------------------

    /// Convert internal [`Message`] list to Anthropic API format.
    ///
    /// Returns `(system_prompt, anthropic_messages)` where:
    /// - `system_prompt` is the extracted system message with cache_control (if present).
    /// - `anthropic_messages` is the ordered list of user/assistant/tool messages.
    ///
    /// User messages are annotated with sender attribution using the provided
    /// `user_names` map (user_id -> display name). **The caller is responsible for
    /// populating this map** by looking up user records from the database. The Python
    /// implementation does this lookup inline, but Rust separates the concern to avoid
    /// passing `DbPool` into message conversion. In Phase 9A-5, the
    /// `ChatAgentAdapter::load_context` method populates this map when loading
    /// conversation history.
    pub fn convert_messages_to_anthropic(
        messages: &[Message],
        user_names: &HashMap<String, String>,
    ) -> (Option<serde_json::Value>, Vec<serde_json::Value>) {
        let mut system_prompt = None;
        let mut anthropic_messages: Vec<serde_json::Value> = Vec::new();

        for msg in messages.iter() {
            match msg.role {
                MessageRole::System => {
                    // System prompt with cache_control for prompt caching.
                    system_prompt = Some(json!([
                        {
                            "type": "text",
                            "text": msg.content,
                            "cache_control": {"type": "ephemeral"}
                        }
                    ]));
                }

                MessageRole::User => {
                    let content = crate::provider::format_user_message(msg, user_names);
                    anthropic_messages.push(json!({
                        "role": "user",
                        "content": content
                    }));
                }

                MessageRole::Assistant => {
                    if let Some(tool_calls) = &msg.tool_calls {
                        // Assistant message with tool calls: content array with
                        // text block (if non-empty) + tool_use blocks.
                        let mut content_blocks: Vec<serde_json::Value> = Vec::new();

                        if !msg.content.is_empty() {
                            content_blocks.push(json!({
                                "type": "text",
                                "text": msg.content
                            }));
                        }

                        for tc in tool_calls {
                            content_blocks.push(json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": tc.arguments
                            }));
                        }

                        anthropic_messages.push(json!({
                            "role": "assistant",
                            "content": content_blocks
                        }));
                    } else {
                        // Simple text assistant message.
                        anthropic_messages.push(json!({
                            "role": "assistant",
                            "content": msg.content
                        }));
                    }
                }

                MessageRole::Tool => {
                    // Tool results are sent as user messages with tool_result blocks.
                    anthropic_messages.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": msg.tool_call_id,
                            "content": msg.content
                        }]
                    }));
                }
            }
        }

        (system_prompt, anthropic_messages)
    }

    /// Convert [`Tool`] definitions to Anthropic's tool format.
    ///
    /// Adds `cache_control: {"type": "ephemeral"}` to the **last** tool
    /// so that all tool definitions are cached together.
    pub fn convert_tools_to_anthropic(tools: &[Tool]) -> Vec<serde_json::Value> {
        let len = tools.len();
        tools
            .iter()
            .enumerate()
            .map(|(i, tool)| {
                let mut tool_json = json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.parameters,
                });
                // Cache marker on last tool.
                if i == len - 1 {
                    tool_json["cache_control"] = json!({"type": "ephemeral"});
                }
                tool_json
            })
            .collect()
    }

    /// Build the JSON request body for the Messages API.
    ///
    /// Split out of [`complete`](Self::complete) so the wire shape can be
    /// asserted without an HTTP round-trip. Most importantly, the `tools` key
    /// is **omitted entirely** for an empty slice — the agent relies on that
    /// to force a no-tools wrap-up turn when its tool-use budget runs out.
    pub(crate) fn build_request_body(
        model: &str,
        system_prompt: Option<serde_json::Value>,
        anthropic_messages: Vec<serde_json::Value>,
        tools: &[Tool],
        temperature: Option<f32>,
        max_tokens: u32,
    ) -> serde_json::Value {
        let mut body = json!({
            "model": model,
            "messages": anthropic_messages,
            "max_tokens": max_tokens,
        });

        if let Some(system) = system_prompt {
            body["system"] = system;
        }

        if !tools.is_empty() {
            body["tools"] = json!(Self::convert_tools_to_anthropic(tools));
        }

        if let Some(temp) = temperature {
            body["temperature"] = json!(temp);
        }

        // Request-level prompt caching — Anthropic automatically places the
        // breakpoint at the last cacheable block and extends the cache
        // incrementally as the conversation grows.
        body["cache_control"] = json!({"type": "ephemeral"});

        body
    }

    // -----------------------------------------------------------------------
    // API Call
    // -----------------------------------------------------------------------

    /// Send a completion request to the Anthropic Messages API.
    ///
    /// # Arguments
    /// * `messages` - Conversation history.
    /// * `tools` - Available tools (may be empty).
    /// * `temperature` - Sampling temperature (0.0-1.0); `None` uses model default.
    /// * `max_tokens` - Maximum tokens to generate.
    /// * `user_names` - Map of user_id -> display name for user attribution.
    ///
    /// # Errors
    /// Returns `kyomi_core::Error::Internal` on API errors after retries are exhausted,
    /// or `kyomi_core::Error::Unauthorized` for authentication failures.
    pub async fn complete(
        &self,
        messages: &[Message],
        tools: &[Tool],
        temperature: Option<f32>,
        max_tokens: u32,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        let (system_prompt, anthropic_messages) =
            Self::convert_messages_to_anthropic(messages, user_names);

        let message_count = anthropic_messages.len();
        let body = Self::build_request_body(
            &self.base.model,
            system_prompt,
            anthropic_messages,
            tools,
            temperature,
            max_tokens,
        );

        debug!(
            model = %self.base.model,
            message_count,
            tool_count = tools.len(),
            "calling Anthropic API"
        );

        // Log request if LOG_LLM_CONTEXT is enabled
        maybe_log_llm("request", &body);

        // Call with retry using shared exponential backoff utility.
        let response_json = kyomi_core::retry::retry_with_backoff(|| self.call_api(&body)).await?;

        // Log response if LOG_LLM_CONTEXT is enabled
        maybe_log_llm("response", &response_json);

        // Parse response.
        Self::parse_response(&self.base.model, &response_json)
    }

    /// Send a single HTTP request to the Anthropic Messages API.
    async fn call_api(&self, body: &serde_json::Value) -> kyomi_core::Result<serde_json::Value> {
        let url = format!("{}/v1/messages", self.base.base_url.trim_end_matches('/'));
        let response = self
            .base
            .client
            .post(&url)
            .header("x-api-key", &self.base.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("anthropic-beta", PROMPT_CACHING_BETA)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| {
                kyomi_core::Error::ServiceUnavailable(format!("Anthropic API request failed: {e}"))
            })?;

        let status = response.status();

        if status.is_success() {
            let json: serde_json::Value = response.json().await.map_err(|e| {
                kyomi_core::Error::Internal(format!("Anthropic API: failed to parse response: {e}"))
            })?;
            return Ok(json);
        }

        // Error response — try to extract the error message from the body.
        let error_body = response.text().await.unwrap_or_default();
        let error_msg = crate::provider::extract_error_message(&error_body)
            .unwrap_or_else(|| format!("HTTP {status}"));

        match status.as_u16() {
            401 => Err(kyomi_core::Error::Unauthorized(format!(
                "Anthropic API authentication failed: {error_msg}"
            ))),
            400 => Err(kyomi_core::Error::BadRequest(format!(
                "Anthropic API bad request: {error_msg}"
            ))),
            429 => Err(kyomi_core::Error::TooManyRequests(
                format!("Anthropic API rate limited: {error_msg}"),
                0,
            )),
            // 529 = Anthropic-specific "overloaded"; treat as service unavailable.
            529 | 502 | 503 | 504 => Err(kyomi_core::Error::ServiceUnavailable(format!(
                "Anthropic API unavailable ({status}): {error_msg}"
            ))),
            _ if status.is_server_error() => Err(kyomi_core::Error::ServiceUnavailable(format!(
                "Anthropic API server error ({status}): {error_msg}"
            ))),
            _ => Err(kyomi_core::Error::Internal(format!(
                "Anthropic API error ({status}): {error_msg}"
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // Response Parsing
    // -----------------------------------------------------------------------

    /// Parse the Anthropic API JSON response into an [`LLMResponse`].
    fn parse_response(
        model: &str,
        response: &serde_json::Value,
    ) -> kyomi_core::Result<LLMResponse> {
        // Extract content blocks.
        let content_blocks = response
            .get("content")
            .and_then(|c| c.as_array())
            .ok_or_else(|| {
                kyomi_core::Error::Internal("Anthropic API response missing 'content' array".into())
            })?;

        // Extract stop reason up front (rather than after the block loop
        // below) so a missing-`input` warning inside that loop can name it —
        // "max_tokens" is the usual explanation for a `tool_use` block that
        // got cut off before Anthropic ever wrote its `input` key.
        let finish_reason = response
            .get("stop_reason")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string();

        let mut text_parts: Vec<&str> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in content_blocks {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        text_parts.push(text);
                    }
                }
                Some("tool_use") => {
                    let id = block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    // Unlike OpenAI, Anthropic returns `input` as an
                    // already-parsed JSON value, not a string — there is no
                    // `from_str` call here that can fail on malformed JSON.
                    // But a `max_tokens` cutoff mid-`tool_use` block can omit
                    // the `input` key entirely, and that used to be silently
                    // coerced to `Value::Null` — indistinguishable from a
                    // tool with a genuinely empty schema (KYO-535). Treat an
                    // absent key as a parse failure, the same as OpenAI's.
                    let (arguments, arguments_error) = match block.get("input") {
                        Some(value) => (value.clone(), None),
                        None => {
                            warn!(
                                tool = %name,
                                finish_reason = %finish_reason,
                                "tool_use block missing 'input' — treating as truncated \
                                 rather than empty"
                            );
                            (
                                json!({}),
                                Some(format!(
                                    "arguments ('input') were missing from the tool_use \
                                     block (finish_reason={finish_reason})"
                                )),
                            )
                        }
                    };

                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments,
                        arguments_error,
                    });
                }
                _ => {
                    // Unknown block type — skip.
                    debug!(
                        block_type = ?block.get("type"),
                        "skipping unknown content block type in Anthropic response"
                    );
                }
            }
        }

        let content = text_parts.join("");

        // Extract usage.
        let usage = Self::parse_usage(response);

        info!(
            model = model,
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            cache_write = usage.cache_creation_input_tokens,
            cache_read = usage.cache_read_input_tokens,
            finish_reason = %finish_reason,
            "Anthropic API call complete"
        );

        Ok(LLMResponse {
            content,
            finish_reason,
            usage,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            cost: None,
            thinking_content: None,
        })
    }

    /// Parse the `usage` object from the Anthropic response.
    fn parse_usage(response: &serde_json::Value) -> AgentTokenUsage {
        let usage = response.get("usage");
        AgentTokenUsage {
            input_tokens: usage
                .and_then(|u| u.get("input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            output_tokens: usage
                .and_then(|u| u.get("output_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_creation_input_tokens: usage
                .and_then(|u| u.get("cache_creation_input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_read_input_tokens: usage
                .and_then(|u| u.get("cache_read_input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            reasoning_tokens: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// LLM debug logging (delegates to shared provider::maybe_log_llm)
// ---------------------------------------------------------------------------

fn maybe_log_llm(label: &str, payload: &serde_json::Value) {
    crate::provider::maybe_log_llm("anthropic", label, payload);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Message conversion tests -------------------------------------------

    #[test]
    fn convert_system_message() {
        let messages = vec![Message::system("You are helpful."), Message::user("hi")];
        let names = HashMap::new();
        let (system, msgs) = AnthropicClient::convert_messages_to_anthropic(&messages, &names);

        // System prompt extracted with cache_control.
        let system = system.unwrap();
        assert_eq!(system[0]["type"], "text");
        assert_eq!(system[0]["text"], "You are helpful.");
        assert_eq!(system[0]["cache_control"], json!({"type": "ephemeral"}));

        // Only user message in the messages array.
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    #[test]
    fn convert_assistant_message_simple() {
        let messages = vec![
            Message::system("prompt"),
            Message::user("hello"),
            Message::assistant("I can help you."),
            Message::user("thanks"), // cache marker (index 3)
        ];
        let names = HashMap::new();
        let (_, msgs) = AnthropicClient::convert_messages_to_anthropic(&messages, &names);

        // 3 messages (user, assistant, user) — system is extracted.
        assert_eq!(msgs.len(), 3);

        // Assistant message at index 1 (no cache marker, simple format).
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"], "I can help you.");
    }

    #[test]
    fn convert_assistant_with_tool_calls() {
        let tool_calls = vec![ToolCall {
            id: "toolu_abc".into(),
            name: "search_catalog".into(),
            arguments: json!({"query": "revenue"}),
            arguments_error: None,
        }];
        let messages = vec![
            Message::system("prompt"),
            Message::user("show revenue"),
            Message::assistant_with_tool_calls("Let me search.", tool_calls),
            Message::tool_result("toolu_abc", "search_catalog", r#"{"tables":[]}"#),
            Message::user("ok"), // cache marker (index 4)
        ];
        let names = HashMap::new();
        let (_, msgs) = AnthropicClient::convert_messages_to_anthropic(&messages, &names);

        assert_eq!(msgs.len(), 4);

        // Assistant message should have content array with text + tool_use.
        let assistant = &msgs[1];
        assert_eq!(assistant["role"], "assistant");
        let content = assistant["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Let me search.");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["id"], "toolu_abc");
        assert_eq!(content[1]["name"], "search_catalog");
        assert_eq!(content[1]["input"]["query"], "revenue");
    }

    #[test]
    fn convert_assistant_with_tool_calls_empty_content() {
        let tool_calls = vec![ToolCall {
            id: "toolu_abc".into(),
            name: "search_catalog".into(),
            arguments: json!({"query": "revenue"}),
            arguments_error: None,
        }];
        let messages = vec![
            Message::system("prompt"),
            Message::user("show revenue"),
            Message::assistant_with_tool_calls("", tool_calls),
            Message::user("ok"),
        ];
        let names = HashMap::new();
        let (_, msgs) = AnthropicClient::convert_messages_to_anthropic(&messages, &names);

        // Assistant message with empty content should NOT include a text block.
        let assistant = &msgs[1];
        let content = assistant["content"].as_array().unwrap();
        assert_eq!(content.len(), 1); // Only tool_use, no text block.
        assert_eq!(content[0]["type"], "tool_use");
    }

    #[test]
    fn convert_tool_result_message() {
        let messages = vec![
            Message::system("prompt"),
            Message::user("hello"),
            Message::assistant("searching"),
            Message::tool_result("tc_1", "search_catalog", r#"{"found": true}"#),
            Message::user("thanks"), // cache marker (index 4)
        ];
        let names = HashMap::new();
        let (_, msgs) = AnthropicClient::convert_messages_to_anthropic(&messages, &names);

        // Tool result at index 2 (system excluded): user, assistant, tool, user.
        let tool_msg = &msgs[2];
        assert_eq!(tool_msg["role"], "user");
        let content = tool_msg["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "tc_1");
        assert_eq!(content[0]["content"], r#"{"found": true}"#);
    }

    // -- Tool conversion tests ----------------------------------------------

    #[test]
    fn convert_tools_empty() {
        let result = AnthropicClient::convert_tools_to_anthropic(&[]);
        assert!(result.is_empty());
    }

    // -- Request body tests --------------------------------------------------

    /// `CustomAgent::chat` forces its final wrap-up turn by passing an empty
    /// tool slice, relying on the request carrying no `tools` key at all — an
    /// empty `tools: []` array would leave the model free to call a tool.
    #[test]
    fn build_request_body_omits_tools_key_when_slice_is_empty() {
        let body = AnthropicClient::build_request_body(
            "claude-test",
            None,
            vec![json!({"role": "user", "content": "hi"})],
            &[],
            None,
            1024,
        );

        assert!(
            body.get("tools").is_none(),
            "an empty tool slice must omit the `tools` key entirely, got: {body}"
        );
    }

    #[test]
    fn build_request_body_includes_tools_when_slice_is_non_empty() {
        let tools = vec![Tool {
            name: "search_catalog".into(),
            description: "Search for tables.".into(),
            parameters: json!({"type": "object", "properties": {}}),
        }];
        let body = AnthropicClient::build_request_body(
            "claude-test",
            None,
            vec![json!({"role": "user", "content": "hi"})],
            &tools,
            None,
            1024,
        );

        let sent = body["tools"]
            .as_array()
            .expect("`tools` must be present for a non-empty slice");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["name"], "search_catalog");
    }

    #[test]
    fn convert_tools_single() {
        let tools = vec![Tool {
            name: "search_catalog".into(),
            description: "Search for tables.".into(),
            parameters: json!({"type": "object", "properties": {"query": {"type": "string"}}}),
        }];
        let result = AnthropicClient::convert_tools_to_anthropic(&tools);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "search_catalog");
        assert_eq!(result[0]["description"], "Search for tables.");
        assert_eq!(result[0]["input_schema"]["type"], "object");
        // Single tool should get cache_control (it is the last tool).
        assert_eq!(result[0]["cache_control"], json!({"type": "ephemeral"}));
    }

    #[test]
    fn convert_tools_multiple_cache_on_last() {
        let tools = vec![
            Tool {
                name: "tool_a".into(),
                description: "A".into(),
                parameters: json!({}),
            },
            Tool {
                name: "tool_b".into(),
                description: "B".into(),
                parameters: json!({}),
            },
            Tool {
                name: "tool_c".into(),
                description: "C".into(),
                parameters: json!({}),
            },
        ];
        let result = AnthropicClient::convert_tools_to_anthropic(&tools);

        assert_eq!(result.len(), 3);
        // Only last tool should have cache_control.
        assert!(result[0].get("cache_control").is_none());
        assert!(result[1].get("cache_control").is_none());
        assert_eq!(result[2]["cache_control"], json!({"type": "ephemeral"}));
    }

    // -- Response parsing tests ---------------------------------------------

    #[test]
    fn parse_response_text_only() {
        let response = json!({
            "content": [
                {"type": "text", "text": "Hello, how can I help?"}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 20
            }
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();

        assert_eq!(result.content, "Hello, how can I help?");
        assert_eq!(result.finish_reason, "end_turn");
        assert_eq!(result.usage.input_tokens, 100);
        assert_eq!(result.usage.output_tokens, 20);
        assert!(result.tool_calls.is_none());
        assert!(result.cost.is_none());
    }

    #[test]
    fn parse_response_with_tool_use() {
        let response = json!({
            "content": [
                {"type": "text", "text": "Let me search."},
                {
                    "type": "tool_use",
                    "id": "toolu_123",
                    "name": "search_catalog",
                    "input": {"query": "revenue"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 500,
                "output_tokens": 50,
                "cache_creation_input_tokens": 1000,
                "cache_read_input_tokens": 2000
            }
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();

        assert_eq!(result.content, "Let me search.");
        assert_eq!(result.finish_reason, "tool_use");
        assert_eq!(result.usage.cache_creation_input_tokens, 1000);
        assert_eq!(result.usage.cache_read_input_tokens, 2000);

        let tool_calls = result.tool_calls.unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "toolu_123");
        assert_eq!(tool_calls[0].name, "search_catalog");
        assert_eq!(tool_calls[0].arguments["query"], "revenue");
    }

    #[test]
    fn parse_response_multiple_text_blocks() {
        let response = json!({
            "content": [
                {"type": "text", "text": "Part 1. "},
                {"type": "text", "text": "Part 2."}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();

        assert_eq!(result.content, "Part 1. Part 2.");
    }

    #[test]
    fn parse_response_missing_usage_defaults_to_zero() {
        let response = json!({
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn"
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();

        assert_eq!(result.usage.input_tokens, 0);
        assert_eq!(result.usage.output_tokens, 0);
    }

    // -- Retry classification tests (via kyomi_core::Error::is_transient) ----
    //
    // call_api now produces semantically correct error variants, which
    // kyomi_core::retry::retry_with_backoff classifies via is_transient().

    #[test]
    fn rate_limit_error_is_transient() {
        // 429 → TooManyRequests, which is_transient() returns true.
        let err = kyomi_core::Error::TooManyRequests("Anthropic API rate limited".into(), 0);
        assert!(err.is_transient());
    }

    #[test]
    fn service_unavailable_is_transient() {
        // 502/503/504/529 → ServiceUnavailable, which is_transient() returns true.
        let err = kyomi_core::Error::ServiceUnavailable("Anthropic API unavailable (503)".into());
        assert!(err.is_transient());
    }

    #[test]
    fn auth_error_is_not_transient() {
        let err = kyomi_core::Error::Unauthorized("invalid key".into());
        assert!(!err.is_transient());
    }

    #[test]
    fn bad_request_is_not_transient() {
        let err = kyomi_core::Error::BadRequest("invalid params".into());
        assert!(!err.is_transient());
    }

    #[test]
    fn not_found_is_not_transient() {
        let err = kyomi_core::Error::NotFound("missing".into());
        assert!(!err.is_transient());
    }

    #[test]
    fn internal_error_is_not_transient() {
        // Generic internal errors (parse failures etc.) are not retried.
        let err = kyomi_core::Error::Internal("failed to parse response".into());
        assert!(!err.is_transient());
    }

    // -- Client constructor tests -------------------------------------------

    #[test]
    fn client_default_model() {
        let client = AnthropicClient::new("test-key".into(), None).unwrap();
        assert_eq!(client.model(), DEFAULT_MODEL);
    }

    #[test]
    fn client_custom_model() {
        let client =
            AnthropicClient::new("test-key".into(), Some("claude-haiku-4-5-20251001".into()))
                .unwrap();
        assert_eq!(client.model(), "claude-haiku-4-5-20251001");
    }

    // -- Contract: Assistant message with BOTH text and tool_calls -----------

    #[test]
    fn convert_assistant_with_text_and_multiple_tool_calls() {
        let tool_calls = vec![
            ToolCall {
                id: "toolu_1".into(),
                name: "search_catalog".into(),
                arguments: json!({"query": "revenue"}),
                arguments_error: None,
            },
            ToolCall {
                id: "toolu_2".into(),
                name: "get_table_info".into(),
                arguments: json!({"table_name": "sales.orders", "datasource": "pg"}),
                arguments_error: None,
            },
        ];
        let messages = vec![
            Message::system("prompt"),
            Message::user("show revenue by region"),
            Message::assistant_with_tool_calls(
                "Let me search for revenue tables first.",
                tool_calls,
            ),
            Message::user("continue"),
        ];
        let names = HashMap::new();
        let (_, msgs) = AnthropicClient::convert_messages_to_anthropic(&messages, &names);

        // Assistant message at index 1 (system extracted).
        let assistant = &msgs[1];
        assert_eq!(assistant["role"], "assistant");
        let content = assistant["content"].as_array().unwrap();
        // text block + 2 tool_use blocks = 3 total.
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(
            content[0]["text"],
            "Let me search for revenue tables first."
        );
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["id"], "toolu_1");
        assert_eq!(content[1]["name"], "search_catalog");
        assert_eq!(content[1]["input"]["query"], "revenue");
        assert_eq!(content[2]["type"], "tool_use");
        assert_eq!(content[2]["id"], "toolu_2");
        assert_eq!(content[2]["name"], "get_table_info");
        assert_eq!(content[2]["input"]["table_name"], "sales.orders");
    }

    // -- Contract: User attribution formatting variations --------------------

    #[test]
    fn format_user_message_web_source_formatting() {
        // User messages from shared conversations include user attribution.
        let msg = Message::user_with_id(
            "[source: web, user_local_time: 2025-01-15T10:30:00+11:00] Show revenue",
            "user-abcd-1234-efgh",
        );
        let mut names = HashMap::new();
        names.insert("user-abcd-1234-efgh".to_string(), "Alice Smith".to_string());
        let result = crate::provider::format_user_message(&msg, &names);
        // Should prepend [Alice Smith (234-efgh)]: before the full content.
        assert!(result.starts_with("[Alice Smith (234-efgh)]: "));
        assert!(result.contains("Show revenue"));
    }

    #[test]
    fn format_user_message_api_source_formatting() {
        let msg = Message::user_with_id("[source: api] Query my data", "user-xxxx-yyyy-zzzz");
        let mut names = HashMap::new();
        names.insert("user-xxxx-yyyy-zzzz".to_string(), "Bob Jones".to_string());
        let result = crate::provider::format_user_message(&msg, &names);
        assert!(result.starts_with("[Bob Jones (yyy-zzzz)]: "));
        assert!(result.contains("Query my data"));
    }

    // -- Contract: Tool schema JSON structure --------------------------------

    #[test]
    fn convert_tools_preserves_json_schema_structure() {
        let tools = vec![Tool {
            name: "query_datasource".into(),
            description: "Execute a SQL query.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "sql_query": {
                        "type": "string",
                        "description": "The SQL query"
                    },
                    "datasource": {
                        "type": "string",
                        "description": "Datasource slug"
                    }
                },
                "required": ["sql_query", "datasource"]
            }),
        }];
        let result = AnthropicClient::convert_tools_to_anthropic(&tools);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "query_datasource");
        assert_eq!(result[0]["description"], "Execute a SQL query.");
        // Parameters go under input_schema (Anthropic format).
        assert_eq!(result[0]["input_schema"]["type"], "object");
        let required = result[0]["input_schema"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("sql_query")));
        assert!(required.contains(&json!("datasource")));
    }

    // -- Contract: parse_response stop_reason variants ----------------------

    #[test]
    fn parse_response_stop_reason_tool_use() {
        let response = json!({
            "content": [
                {"type": "text", "text": "Searching..."},
                {"type": "tool_use", "id": "tc_1", "name": "search_catalog", "input": {"query": "rev"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 100, "output_tokens": 30}
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();
        assert_eq!(result.finish_reason, "tool_use");
        assert!(result.tool_calls.is_some());
        assert_eq!(result.tool_calls.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn parse_response_stop_reason_max_tokens() {
        let response = json!({
            "content": [{"type": "text", "text": "This response was truncated because..."}],
            "stop_reason": "max_tokens",
            "usage": {"input_tokens": 50000, "output_tokens": 4096}
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();
        assert_eq!(result.finish_reason, "max_tokens");
        assert!(result.tool_calls.is_none());
    }

    // -- Contract: tool_use block missing 'input' records arguments_error ----

    #[test]
    fn parse_response_tool_use_missing_input_records_arguments_error() {
        // KYO-535: Anthropic's non-streaming API can return a tool_use block
        // whose `input` key is absent entirely when the response was cut off
        // by max_tokens mid-block. That used to be silently coerced to
        // `Value::Null` — indistinguishable from a tool with a genuinely
        // empty schema. It must now be recorded on arguments_error instead.
        let response = json!({
            "content": [
                {"type": "tool_use", "id": "tc_1", "name": "update_dashboard"}
            ],
            "stop_reason": "max_tokens",
            "usage": {"input_tokens": 50000, "output_tokens": 4096}
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();
        assert_eq!(result.finish_reason, "max_tokens");
        let tool_calls = result.tool_calls.expect("tool_use block should still surface");
        let tc = &tool_calls[0];
        assert!(tc.arguments.is_object());
        assert!(tc.arguments.as_object().unwrap().is_empty());
        let error = tc
            .arguments_error
            .as_ref()
            .expect("missing 'input' must set arguments_error, not silently default to null");
        assert!(
            error.contains("max_tokens"),
            "arguments_error should name the max_tokens finish_reason, got: {error}"
        );
    }

    #[test]
    fn parse_response_tool_use_present_empty_input_is_not_an_error() {
        // A tool with a genuinely empty schema sends `"input": {}` — this is
        // legitimate and must NOT be confused with a missing key.
        let response = json!({
            "content": [
                {"type": "tool_use", "id": "tc_1", "name": "list_datasources", "input": {}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 100, "output_tokens": 10}
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();
        let tool_calls = result.tool_calls.unwrap();
        let tc = &tool_calls[0];
        assert!(tc.arguments.as_object().unwrap().is_empty());
        assert!(tc.arguments_error.is_none());
    }

    #[test]
    fn parse_response_stop_reason_missing_defaults_to_unknown() {
        let response = json!({
            "content": [{"type": "text", "text": "Hello"}],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();
        assert_eq!(result.finish_reason, "unknown");
    }

    // -- Contract: parse_response with multiple tool_use blocks -------------

    #[test]
    fn parse_response_multiple_tool_use_blocks() {
        let response = json!({
            "content": [
                {"type": "text", "text": "I'll search and query."},
                {"type": "tool_use", "id": "tc_1", "name": "search_catalog", "input": {"query": "sales"}},
                {"type": "tool_use", "id": "tc_2", "name": "get_table_info", "input": {"table_name": "orders"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 500, "output_tokens": 100}
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();
        assert_eq!(result.content, "I'll search and query.");
        let tool_calls = result.tool_calls.unwrap();
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].id, "tc_1");
        assert_eq!(tool_calls[0].name, "search_catalog");
        assert_eq!(tool_calls[1].id, "tc_2");
        assert_eq!(tool_calls[1].name, "get_table_info");
    }

    // -- Contract: parse_response with unknown block type -------------------

    #[test]
    fn parse_response_skips_unknown_block_type() {
        let response = json!({
            "content": [
                {"type": "text", "text": "Hello"},
                {"type": "thinking", "thinking": "I'm reasoning about this..."},
                {"type": "text", "text": " world"}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 50, "output_tokens": 10}
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();
        // Unknown blocks are silently skipped; text blocks concatenated.
        assert_eq!(result.content, "Hello world");
    }

    // -- Contract: parse_response with tool_use only (no text) ---------------

    #[test]
    fn parse_response_tool_use_only_no_text() {
        let response = json!({
            "content": [
                {"type": "tool_use", "id": "tc_1", "name": "search_catalog", "input": {"query": "rev"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 200, "output_tokens": 30}
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();
        assert_eq!(result.content, ""); // No text content.
        assert!(result.tool_calls.is_some());
        assert_eq!(result.tool_calls.as_ref().unwrap().len(), 1);
    }

    // -- Contract: Token usage with all cache fields populated ---------------

    #[test]
    fn parse_response_full_cache_token_usage() {
        let response = json!({
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 1000,
                "output_tokens": 500,
                "cache_creation_input_tokens": 2000,
                "cache_read_input_tokens": 8000
            }
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();
        assert_eq!(result.usage.input_tokens, 1000);
        assert_eq!(result.usage.output_tokens, 500);
        assert_eq!(result.usage.cache_creation_input_tokens, 2000);
        assert_eq!(result.usage.cache_read_input_tokens, 8000);
        assert!(result.cost.is_none());
    }

    // -- Contract: parse_response missing content array ----------------------

    #[test]
    fn parse_response_missing_content_array_is_error() {
        let response = json!({
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let result = AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response);
        assert!(result.is_err());
    }

    // -- Contract: Conversion of full conversation flow ----------------------

    #[test]
    fn convert_full_tool_use_conversation() {
        // A realistic conversation: system, user, assistant+tool, tool_result, assistant.
        let messages = vec![
            Message::system("You are a helpful data analyst."),
            Message::user("Show me monthly revenue."),
            Message::assistant_with_tool_calls(
                "Let me search for revenue tables.",
                vec![ToolCall {
                    id: "tc_1".into(),
                    name: "search_catalog".into(),
                    arguments: json!({"query": "revenue monthly"}),
                    arguments_error: None,
                }],
            ),
            Message::tool_result(
                "tc_1",
                "search_catalog",
                r#"{"tables": ["finance.revenue"]}"#,
            ),
            Message::assistant("Here is the monthly revenue data."),
        ];
        let names = HashMap::new();
        let (system, msgs) = AnthropicClient::convert_messages_to_anthropic(&messages, &names);

        // System extracted.
        assert!(system.is_some());
        assert_eq!(
            system.unwrap()[0]["text"],
            "You are a helpful data analyst."
        );

        // 4 messages in output: user, assistant+tool, tool_result(user role), assistant.
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[2]["role"], "user"); // tool results are user role
        assert_eq!(msgs[3]["role"], "assistant");

        // Assistant-with-tool-calls block has text + tool_use.
        let assistant_with_tools = msgs[1]["content"].as_array().unwrap();
        assert_eq!(assistant_with_tools[0]["type"], "text");
        assert_eq!(assistant_with_tools[1]["type"], "tool_use");
        assert_eq!(assistant_with_tools[1]["name"], "search_catalog");

        // Tool result block structured correctly under user role.
        let tool_result = msgs[2]["content"].as_array().unwrap();
        assert_eq!(tool_result[0]["type"], "tool_result");
        assert_eq!(tool_result[0]["tool_use_id"], "tc_1");
    }

    // -- Contract: parse_response thinking-only (no text, no tools) ----------

    #[test]
    fn parse_response_thinking_only_no_text_no_tools() {
        // A response with only a "thinking" block (extended thinking) and no
        // text or tool_use blocks. The parser should return empty content and
        // no tool calls.
        let response = json!({
            "content": [
                {"type": "thinking", "thinking": "I need to analyze this carefully..."}
            ],
            "usage": {"input_tokens": 100, "output_tokens": 50},
            "stop_reason": "end_turn"
        });
        let result =
            AnthropicClient::parse_response("claude-sonnet-4-5-20250929", &response).unwrap();
        assert_eq!(result.content, "");
        assert!(result.tool_calls.is_none());
        assert_eq!(result.finish_reason, "end_turn");
    }

    // -- Contract: extract_error_message edge cases -------------------------

    #[test]
    fn extract_error_message_nested_structure() {
        let body = r#"{"error": {"type": "rate_limit_error", "message": "You have exceeded the rate limit."}}"#;
        assert_eq!(
            crate::provider::extract_error_message(body),
            Some("You have exceeded the rate limit.".to_string())
        );
    }

    #[test]
    fn extract_error_message_empty_message() {
        let body = r#"{"error": {"type": "server_error", "message": ""}}"#;
        assert_eq!(
            crate::provider::extract_error_message(body),
            Some(String::new())
        );
    }

    // -- Contract: client with_base_url for testing -------------------------

    #[test]
    fn client_with_base_url() {
        let client = AnthropicClient::with_base_url(
            "test-key".into(),
            None,
            "http://localhost:8080/mock".into(),
        )
        .unwrap();
        assert_eq!(client.model(), DEFAULT_MODEL);
    }
}
