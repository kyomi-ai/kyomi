// SPDX-License-Identifier: AGPL-3.0-or-later

//! OpenAI-compatible HTTP client for chat completions.
//!
//! Calls `POST {base_url}` using `reqwest`. Supports any OpenAI-compatible API
//! (OpenAI, Azure OpenAI, OpenRouter, Groq, Together, Ollama, vLLM, etc.) via
//! a configurable `base_url`.
//!
//! Handles message/tool conversion, retry logic with exponential backoff,
//! and token usage tracking.

use std::collections::HashMap;

use serde_json::json;
use tracing::{debug, info};

use crate::types::{AgentTokenUsage, LLMResponse, Message, MessageRole, Tool, ToolCall};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default OpenAI Chat Completions API endpoint.
const OPENAI_API_URL: &str = "https://api.openai.com/v1";

/// Default model for chat completions.
pub const DEFAULT_MODEL: &str = "gpt-4o-mini";

// ---------------------------------------------------------------------------
// OpenAIProvider
// ---------------------------------------------------------------------------

/// HTTP client for any OpenAI-compatible chat completions API.
///
/// Handles message conversion and retry logic.
/// Supports OpenAI, Azure OpenAI, OpenRouter, Groq, Together, Ollama,
/// vLLM, and any service that implements the OpenAI chat completions spec.
pub struct OpenAIProvider {
    base: crate::provider::ProviderBase,
    ctx_window: u32,
}

impl OpenAIProvider {
    /// Create a new OpenAI-compatible provider.
    ///
    /// # Arguments
    /// * `api_key` - API key for authentication.
    /// * `model` - Model name; uses [`DEFAULT_MODEL`] if `None`.
    /// * `base_url` - API endpoint URL; uses OpenAI's default if `None`.
    pub fn new(
        api_key: String,
        model: Option<String>,
        base_url: Option<String>,
    ) -> kyomi_core::Result<Self> {
        Ok(Self {
            base: crate::provider::ProviderBase::new(
                api_key,
                model,
                DEFAULT_MODEL,
                base_url,
                OPENAI_API_URL,
            )?,
            ctx_window: 0,
        })
    }

    /// Create a provider pointing at a custom API URL.
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
            ctx_window: 0,
        })
    }

    /// Set the context window size (called by the factory when known).
    pub fn set_context_window(&mut self, size: u32) {
        self.ctx_window = size;
    }

    /// Return the context window size.
    pub fn context_window(&self) -> u32 {
        self.ctx_window
    }

    /// Return the model name this provider is configured with.
    pub fn model(&self) -> &str {
        self.base.model()
    }

    // -----------------------------------------------------------------------
    // Message & Tool Conversion
    // -----------------------------------------------------------------------

    /// Convert internal [`Message`] list to OpenAI API format.
    ///
    /// Returns a flat array of messages suitable for the `messages` field
    /// in the OpenAI chat completions request. System messages are inline
    /// (not extracted separately like Anthropic).
    ///
    /// User messages are annotated with sender attribution using the provided
    /// `user_names` map (user_id -> display name), matching the pattern
    /// established in the Anthropic provider.
    pub fn convert_messages_to_openai(
        messages: &[Message],
        user_names: &HashMap<String, String>,
    ) -> Vec<serde_json::Value> {
        let mut openai_messages: Vec<serde_json::Value> = Vec::new();

        for msg in messages {
            match msg.role {
                MessageRole::System => {
                    openai_messages.push(json!({
                        "role": "system",
                        "content": msg.content
                    }));
                }

                MessageRole::User => {
                    let content = crate::provider::format_user_message(msg, user_names);
                    openai_messages.push(json!({
                        "role": "user",
                        "content": content
                    }));
                }

                MessageRole::Assistant => {
                    if let Some(tool_calls) = &msg.tool_calls {
                        let openai_tool_calls: Vec<serde_json::Value> = tool_calls
                            .iter()
                            .map(|tc| {
                                // OpenAI requires `arguments` to be a JSON string,
                                // not a JSON object.
                                let arguments_str = tc.arguments.to_string();
                                json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": arguments_str
                                    }
                                })
                            })
                            .collect();

                        let mut assistant_msg = json!({
                            "role": "assistant",
                            "tool_calls": openai_tool_calls
                        });

                        // Include content only if non-empty.
                        if !msg.content.is_empty() {
                            assistant_msg["content"] =
                                serde_json::Value::String(msg.content.clone());
                        }

                        openai_messages.push(assistant_msg);
                    } else {
                        openai_messages.push(json!({
                            "role": "assistant",
                            "content": msg.content
                        }));
                    }
                }

                MessageRole::Tool => {
                    openai_messages.push(json!({
                        "role": "tool",
                        "tool_call_id": msg.tool_call_id,
                        "content": msg.content
                    }));
                }
            }
        }

        openai_messages
    }

    /// Convert [`Tool`] definitions to OpenAI's function calling format.
    ///
    /// Each tool becomes: `{ "type": "function", "function": { "name", "description", "parameters" } }`
    pub fn convert_tools_to_openai(tools: &[Tool]) -> Vec<serde_json::Value> {
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters
                    }
                })
            })
            .collect()
    }

    /// Build the JSON request body for the chat completions API.
    ///
    /// Split out of [`complete`](Self::complete) so the wire shape can be
    /// asserted without an HTTP round-trip. Most importantly, the `tools` key
    /// is **omitted entirely** for an empty slice — the agent relies on that
    /// to force a no-tools wrap-up turn when its iteration budget runs out.
    pub(crate) fn build_request_body(
        model: &str,
        openai_messages: Vec<serde_json::Value>,
        tools: &[Tool],
        temperature: Option<f32>,
        max_tokens: u32,
    ) -> serde_json::Value {
        let mut body = json!({
            "model": model,
            "messages": openai_messages,
            "max_tokens": max_tokens,
        });

        // Omit tools field entirely if empty — some OpenAI-compatible APIs
        // error on an empty tools array.
        if !tools.is_empty() {
            body["tools"] = json!(Self::convert_tools_to_openai(tools));
        }

        if let Some(temp) = temperature {
            body["temperature"] = json!(temp);
        }

        body
    }

    // -----------------------------------------------------------------------
    // API Call
    // -----------------------------------------------------------------------

    /// Send a completion request to the OpenAI-compatible chat completions API.
    ///
    /// # Arguments
    /// * `messages` - Conversation history.
    /// * `tools` - Available tools (may be empty).
    /// * `temperature` - Sampling temperature; `None` uses model default.
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
        let openai_messages = Self::convert_messages_to_openai(messages, user_names);

        let message_count = openai_messages.len();
        let body = Self::build_request_body(
            &self.base.model,
            openai_messages,
            tools,
            temperature,
            max_tokens,
        );

        debug!(
            model = %self.base.model,
            message_count,
            tool_count = tools.len(),
            "calling OpenAI-compatible API"
        );

        // Call with retry using shared exponential backoff utility.
        let response_json = kyomi_core::retry::retry_with_backoff(|| self.call_api(&body)).await?;

        // Parse response.
        let response = Self::parse_response(&self.base.model, &response_json)?;

        Ok(response)
    }

    /// Send a single HTTP request to the OpenAI-compatible API.
    async fn call_api(&self, body: &serde_json::Value) -> kyomi_core::Result<serde_json::Value> {
        crate::provider::maybe_log_llm("openai", "request", body);

        let mut request = self
            .base
            .client
            .post(format!("{}/chat/completions", self.base.base_url.trim_end_matches('/')))
            .header("Authorization", format!("Bearer {}", self.base.api_key))
            .header("Content-Type", "application/json");

        if self.base.base_url.contains("openrouter.ai") {
            request = request
                .header("X-OpenRouter-Title", "Kyomi")
                .header("HTTP-Referer", "https://kyomi.ai");
        }

        let response = request
            .json(body)
            .send()
            .await
            .map_err(|e| {
                kyomi_core::Error::ServiceUnavailable(format!("OpenAI API request failed: {e}"))
            })?;

        let status = response.status();

        if status.is_success() {
            let body_bytes = response.bytes().await.map_err(|e| {
                kyomi_core::Error::ServiceUnavailable(format!(
                    "OpenAI API: failed to read response body: {e}"
                ))
            })?;

            let json: serde_json::Value = serde_json::from_slice(&body_bytes).map_err(|e| {
                let body_preview =
                    String::from_utf8_lossy(&body_bytes[..body_bytes.len().min(500)]);
                tracing::error!(
                    "OpenAI API: failed to parse JSON response. Preview: {}",
                    body_preview
                );
                kyomi_core::Error::Internal(format!(
                    "OpenAI API: failed to parse response: {e}. Body preview: {body_preview}"
                ))
            })?;

            crate::provider::maybe_log_llm("openai", "response", &json);
            return Ok(json);
        }

        // Error response — try to extract the error message from the body.
        let error_body = response.text().await.unwrap_or_default();
        let error_msg = crate::provider::extract_error_message(&error_body)
            .unwrap_or_else(|| format!("HTTP {status}"));

        match status.as_u16() {
            401 => Err(kyomi_core::Error::Unauthorized(format!(
                "OpenAI API authentication failed: {error_msg}"
            ))),
            400 => Err(kyomi_core::Error::BadRequest(format!(
                "OpenAI API bad request: {error_msg}"
            ))),
            429 => Err(kyomi_core::Error::TooManyRequests(
                format!("OpenAI API rate limited: {error_msg}"),
                0,
            )),
            502..=504 => Err(kyomi_core::Error::ServiceUnavailable(format!(
                "OpenAI API unavailable ({status}): {error_msg}"
            ))),
            _ if status.is_server_error() => Err(kyomi_core::Error::ServiceUnavailable(format!(
                "OpenAI API server error ({status}): {error_msg}"
            ))),
            _ => Err(kyomi_core::Error::Internal(format!(
                "OpenAI API error ({status}): {error_msg}"
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // Response Parsing
    // -----------------------------------------------------------------------

    /// Parse the OpenAI API JSON response into an [`LLMResponse`].
    ///
    /// Extracts content and tool calls from `choices[0].message`, and maps
    /// `finish_reason` to internal format.
    fn parse_response(
        model: &str,
        response: &serde_json::Value,
    ) -> kyomi_core::Result<LLMResponse> {
        // Extract the first choice's message.
        let message = response
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .ok_or_else(|| {
                kyomi_core::Error::Internal(
                    "OpenAI API response missing 'choices[0].message'".into(),
                )
            })?;

        // Extract content (may be null).
        // OpenAI o-series and OpenRouter models may return reasoning in
        // `reasoning_content` or `reasoning` fields. These are extracted
        // separately into thinking_content — they must NOT be used as a
        // fallback for content.
        let content = message
            .get("content")
            .and_then(|c| c.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("")
            .to_string();

        // Extract reasoning/thinking content from structured fields.
        // This is separate from the <thinking>...</thinking> tag stripping
        // which handles models that embed reasoning inside content.
        let thinking_content = message
            .get("reasoning_content")
            .and_then(|r| r.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                message
                    .get("reasoning")
                    .and_then(|r| r.as_str())
                    .filter(|s| !s.is_empty())
            })
            .map(|s| s.to_string());

        // Strip <thinking>...</thinking> tags from content for models that
        // embed chain-of-thought inside the content field (e.g. Qwen3).
        let content = if let Some(pos) = content.find("</thinking>") {
            content[pos + "</thinking>".len()..].trim().to_string()
        } else if content.starts_with("<thinking>") || content.contains("\n<thinking>") {
            String::new()
        } else {
            content
        };

        // Extract tool calls if present.
        let tool_calls = message
            .get("tool_calls")
            .and_then(|tc| tc.as_array())
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|call| {
                        let id = call.get("id")?.as_str()?.to_string();
                        let function = call.get("function")?;
                        let name = function.get("name")?.as_str()?.to_string();
                        // OpenAI returns `arguments` as a JSON string — parse it.
                        let arguments_str = function.get("arguments")?.as_str()?;
                        let arguments: serde_json::Value =
                            serde_json::from_str(arguments_str).unwrap_or(json!({}));
                        Some(ToolCall {
                            id,
                            name,
                            arguments,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|calls| !calls.is_empty());

        // Map finish_reason to internal format.
        let raw_finish_reason = response
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(|fr| fr.as_str())
            .unwrap_or("unknown");

        let finish_reason = match raw_finish_reason {
            "stop" => "end_turn".to_string(),
            "tool_calls" => "tool_use".to_string(),
            "length" => "max_tokens".to_string(),
            other => other.to_string(),
        };

        // Extract usage.
        let usage = Self::parse_usage(response);

        // Extract cost from OpenRouter responses (`usage.cost` in USD).
        // Direct API providers (Anthropic, Gemini, plain OpenAI) don't return
        // this field, so it remains None for those callers.
        let cost = response
            .get("usage")
            .and_then(|u| u.get("cost"))
            .and_then(|v| v.as_f64());

        info!(
            model = model,
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            reasoning_tokens = usage.reasoning_tokens,
            cost = ?cost,
            finish_reason = %finish_reason,
            "OpenAI API call complete"
        );

        Ok(LLMResponse {
            content,
            finish_reason,
            usage,
            tool_calls,
            cost,
            thinking_content,
        })
    }

    /// Parse the `usage` object from the OpenAI response.
    ///
    /// Extracts `reasoning_tokens` from `completion_tokens_details` when present
    /// (o-series models). Reasoning tokens are a subset of `completion_tokens`,
    /// not additional — they're tracked separately for cost transparency.
    fn parse_usage(response: &serde_json::Value) -> AgentTokenUsage {
        let usage = response.get("usage");
        let reasoning_tokens = usage
            .and_then(|u| u.get("completion_tokens_details"))
            .and_then(|d| d.get("reasoning_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        AgentTokenUsage {
            input_tokens: usage
                .and_then(|u| u.get("prompt_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            output_tokens: usage
                .and_then(|u| u.get("completion_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            // OpenAI doesn't expose prompt caching tokens the same way.
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            reasoning_tokens,
        }
    }
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
        let msgs = OpenAIProvider::convert_messages_to_openai(&messages, &names);

        // System message stays inline in OpenAI format.
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "You are helpful.");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "hi");
    }

    #[test]
    fn convert_user_message_with_attribution() {
        let messages = vec![Message::user_with_id("show data", "user-1234-5678-abcd")];
        let mut names = HashMap::new();
        names.insert("user-1234-5678-abcd".to_string(), "Alice".to_string());
        let msgs = OpenAIProvider::convert_messages_to_openai(&messages, &names);

        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "[Alice (678-abcd)]: show data");
    }

    #[test]
    fn convert_assistant_message_simple() {
        let messages = vec![Message::assistant("I can help you.")];
        let names = HashMap::new();
        let msgs = OpenAIProvider::convert_messages_to_openai(&messages, &names);

        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[0]["content"], "I can help you.");
        // No tool_calls field when there are no tool calls.
        assert!(msgs[0].get("tool_calls").is_none());
    }

    #[test]
    fn convert_assistant_with_tool_calls() {
        let tool_calls = vec![ToolCall {
            id: "call_abc".into(),
            name: "search_catalog".into(),
            arguments: json!({"query": "revenue"}),
        }];
        let messages = vec![Message::assistant_with_tool_calls(
            "Let me search.",
            tool_calls,
        )];
        let names = HashMap::new();
        let msgs = OpenAIProvider::convert_messages_to_openai(&messages, &names);

        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        assert_eq!(msg["role"], "assistant");
        assert_eq!(msg["content"], "Let me search.");

        let tc = msg["tool_calls"].as_array().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0]["id"], "call_abc");
        assert_eq!(tc[0]["type"], "function");
        assert_eq!(tc[0]["function"]["name"], "search_catalog");
        // Arguments must be a JSON string, not an object.
        let args = tc[0]["function"]["arguments"].as_str().unwrap();
        assert_eq!(args, r#"{"query":"revenue"}"#);
    }

    #[test]
    fn convert_assistant_with_tool_calls_empty_content() {
        let tool_calls = vec![ToolCall {
            id: "call_abc".into(),
            name: "search_catalog".into(),
            arguments: json!({"query": "revenue"}),
        }];
        let messages = vec![Message::assistant_with_tool_calls("", tool_calls)];
        let names = HashMap::new();
        let msgs = OpenAIProvider::convert_messages_to_openai(&messages, &names);

        let msg = &msgs[0];
        // Empty content should NOT include content field.
        assert!(msg.get("content").is_none());
        // But tool_calls should be present.
        assert!(msg.get("tool_calls").is_some());
    }

    #[test]
    fn convert_assistant_with_multiple_tool_calls() {
        let tool_calls = vec![
            ToolCall {
                id: "call_1".into(),
                name: "search_catalog".into(),
                arguments: json!({"query": "revenue"}),
            },
            ToolCall {
                id: "call_2".into(),
                name: "get_table_info".into(),
                arguments: json!({"table_name": "sales.orders"}),
            },
        ];
        let messages = vec![Message::assistant_with_tool_calls(
            "Searching multiple.",
            tool_calls,
        )];
        let names = HashMap::new();
        let msgs = OpenAIProvider::convert_messages_to_openai(&messages, &names);

        let tc = msgs[0]["tool_calls"].as_array().unwrap();
        assert_eq!(tc.len(), 2);
        assert_eq!(tc[0]["function"]["name"], "search_catalog");
        assert_eq!(tc[1]["function"]["name"], "get_table_info");
    }

    #[test]
    fn convert_tool_result_message() {
        let messages = vec![Message::tool_result(
            "call_abc",
            "search_catalog",
            r#"{"found": true}"#,
        )];
        let names = HashMap::new();
        let msgs = OpenAIProvider::convert_messages_to_openai(&messages, &names);

        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "tool");
        assert_eq!(msgs[0]["tool_call_id"], "call_abc");
        assert_eq!(msgs[0]["content"], r#"{"found": true}"#);
    }

    #[test]
    fn convert_full_tool_use_conversation() {
        // Realistic: system, user, assistant+tool, tool_result, assistant.
        let messages = vec![
            Message::system("You are a helpful data analyst."),
            Message::user("Show me monthly revenue."),
            Message::assistant_with_tool_calls(
                "Let me search for revenue tables.",
                vec![ToolCall {
                    id: "call_1".into(),
                    name: "search_catalog".into(),
                    arguments: json!({"query": "revenue monthly"}),
                }],
            ),
            Message::tool_result(
                "call_1",
                "search_catalog",
                r#"{"tables": ["finance.revenue"]}"#,
            ),
            Message::assistant("Here is the monthly revenue data."),
        ];
        let names = HashMap::new();
        let msgs = OpenAIProvider::convert_messages_to_openai(&messages, &names);

        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[2]["role"], "assistant");
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(msgs[4]["role"], "assistant");
    }

    // -- Tool conversion tests ----------------------------------------------

    #[test]
    fn convert_tools_empty() {
        let result = OpenAIProvider::convert_tools_to_openai(&[]);
        assert!(result.is_empty());
    }

    // -- Request body tests --------------------------------------------------

    /// `CustomAgent::chat` forces its final wrap-up turn by passing an empty
    /// tool slice, relying on the request carrying no `tools` key at all — an
    /// empty `tools: []` array would leave the model free to call a tool (and
    /// makes several OpenAI-compatible gateways reject the request outright).
    #[test]
    fn build_request_body_omits_tools_key_when_slice_is_empty() {
        let body = OpenAIProvider::build_request_body(
            "gpt-test",
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
        let body = OpenAIProvider::build_request_body(
            "gpt-test",
            vec![json!({"role": "user", "content": "hi"})],
            &tools,
            None,
            1024,
        );

        let sent = body["tools"]
            .as_array()
            .expect("`tools` must be present for a non-empty slice");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["function"]["name"], "search_catalog");
    }

    #[test]
    fn convert_tools_single() {
        let tools = vec![Tool {
            name: "search_catalog".into(),
            description: "Search for tables.".into(),
            parameters: json!({"type": "object", "properties": {"query": {"type": "string"}}}),
        }];
        let result = OpenAIProvider::convert_tools_to_openai(&tools);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["type"], "function");
        assert_eq!(result[0]["function"]["name"], "search_catalog");
        assert_eq!(result[0]["function"]["description"], "Search for tables.");
        assert_eq!(result[0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn convert_tools_multiple() {
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
        ];
        let result = OpenAIProvider::convert_tools_to_openai(&tools);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["function"]["name"], "tool_a");
        assert_eq!(result[1]["function"]["name"], "tool_b");
    }

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
        let result = OpenAIProvider::convert_tools_to_openai(&tools);

        assert_eq!(result[0]["function"]["parameters"]["type"], "object");
        let required = result[0]["function"]["parameters"]["required"]
            .as_array()
            .unwrap();
        assert!(required.contains(&json!("sql_query")));
        assert!(required.contains(&json!("datasource")));
    }

    // -- Response parsing tests ---------------------------------------------

    #[test]
    fn parse_response_text_only() {
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Hello, how can I help?"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 20
            }
        });
        let response = OpenAIProvider::parse_response("gpt-4o-mini", &response).unwrap();

        assert_eq!(response.content, "Hello, how can I help?");
        assert_eq!(response.finish_reason, "end_turn");
        assert_eq!(response.usage.input_tokens, 100);
        assert_eq!(response.usage.output_tokens, 20);
        assert!(response.tool_calls.is_none());
        assert!(response.cost.is_none());
        assert!(response.thinking_content.is_none());
    }

    #[test]
    fn parse_response_with_tool_calls() {
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Let me search.",
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "search_catalog",
                            "arguments": "{\"query\":\"revenue\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 500,
                "completion_tokens": 50
            }
        });
        let response = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();

        assert_eq!(response.content, "Let me search.");
        assert_eq!(response.finish_reason, "tool_use");

        let tool_calls = response.tool_calls.unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_123");
        assert_eq!(tool_calls[0].name, "search_catalog");
        assert_eq!(tool_calls[0].arguments["query"], "revenue");
    }

    #[test]
    fn parse_response_null_content_with_tool_calls() {
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_456",
                        "type": "function",
                        "function": {
                            "name": "get_table_info",
                            "arguments": "{\"table\":\"orders\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 200,
                "completion_tokens": 30
            }
        });
        let response = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();

        assert_eq!(response.content, "");
        assert!(response.tool_calls.is_some());
        assert_eq!(response.tool_calls.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn parse_response_multiple_tool_calls() {
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Searching...",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "search_catalog",
                                "arguments": "{\"query\":\"sales\"}"
                            }
                        },
                        {
                            "id": "call_2",
                            "type": "function",
                            "function": {
                                "name": "get_table_info",
                                "arguments": "{\"table_name\":\"orders\"}"
                            }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 500,
                "completion_tokens": 100
            }
        });
        let response = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();

        let tool_calls = response.tool_calls.unwrap();
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].id, "call_1");
        assert_eq!(tool_calls[0].name, "search_catalog");
        assert_eq!(tool_calls[1].id, "call_2");
        assert_eq!(tool_calls[1].name, "get_table_info");
    }

    // -- Finish reason mapping tests ----------------------------------------

    #[test]
    fn parse_response_finish_reason_stop() {
        let response = json!({
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let response = OpenAIProvider::parse_response("gpt-4o-mini", &response).unwrap();
        assert_eq!(response.finish_reason, "end_turn");
    }

    #[test]
    fn parse_response_finish_reason_tool_calls() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "c1", "type": "function",
                        "function": {"name": "t", "arguments": "{}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let response = OpenAIProvider::parse_response("gpt-4o-mini", &response).unwrap();
        assert_eq!(response.finish_reason, "tool_use");
    }

    #[test]
    fn parse_response_finish_reason_length() {
        let response = json!({
            "choices": [{"message": {"content": "truncated..."}, "finish_reason": "length"}],
            "usage": {"prompt_tokens": 50000, "completion_tokens": 4096}
        });
        let response = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();
        assert_eq!(response.finish_reason, "max_tokens");
    }

    #[test]
    fn parse_response_finish_reason_missing_defaults_to_unknown() {
        let response = json!({
            "choices": [{"message": {"content": "Hello"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let response = OpenAIProvider::parse_response("gpt-4o-mini", &response).unwrap();
        assert_eq!(response.finish_reason, "unknown");
    }

    // -- Usage parsing tests ------------------------------------------------

    #[test]
    fn parse_response_missing_usage_defaults_to_zero() {
        let response = json!({
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}]
        });
        let response = OpenAIProvider::parse_response("gpt-4o-mini", &response).unwrap();

        assert_eq!(response.usage.input_tokens, 0);
        assert_eq!(response.usage.output_tokens, 0);
        assert_eq!(response.usage.cache_creation_input_tokens, 0);
        assert_eq!(response.usage.cache_read_input_tokens, 0);
    }

    #[test]
    fn parse_response_cache_tokens_always_zero() {
        let response = json!({
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1000, "completion_tokens": 500}
        });
        let response = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();

        // OpenAI doesn't expose cache tokens the way Anthropic does.
        assert_eq!(response.usage.cache_creation_input_tokens, 0);
        assert_eq!(response.usage.cache_read_input_tokens, 0);
    }

    // -- Reasoning content extraction tests ----------------------------------

    #[test]
    fn parse_response_reasoning_content_field() {
        // OpenAI o-series returns reasoning_content alongside content.
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "The answer is 42.",
                    "reasoning_content": "Let me think step by step about this..."
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 100, "completion_tokens": 50}
        });
        let response = OpenAIProvider::parse_response("o3", &response).unwrap();

        assert_eq!(response.content, "The answer is 42.");
        assert_eq!(
            response.thinking_content.as_deref(),
            Some("Let me think step by step about this...")
        );
    }

    #[test]
    fn parse_response_reasoning_field_openrouter() {
        // OpenRouter returns reasoning in a separate `reasoning` field.
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Here is my answer.",
                    "reasoning": "I considered multiple approaches..."
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 100, "completion_tokens": 50}
        });
        let response = OpenAIProvider::parse_response("o3", &response).unwrap();

        assert_eq!(response.content, "Here is my answer.");
        assert_eq!(
            response.thinking_content.as_deref(),
            Some("I considered multiple approaches...")
        );
    }

    #[test]
    fn parse_response_reasoning_not_used_as_content_fallback() {
        // When content is null and reasoning exists, content must stay empty.
        // Previously, reasoning was used as a fallback for content — this test
        // ensures that no longer happens.
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "reasoning": "Deep thinking here..."
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 100, "completion_tokens": 50}
        });
        let response = OpenAIProvider::parse_response("o3", &response).unwrap();

        assert_eq!(response.content, "");
        assert_eq!(
            response.thinking_content.as_deref(),
            Some("Deep thinking here...")
        );
    }

    #[test]
    fn parse_response_no_reasoning_fields() {
        // Standard response without reasoning — thinking_content must be None.
        let response = json!({
            "choices": [{"message": {"content": "Hello!"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let response = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();

        assert_eq!(response.content, "Hello!");
        assert!(response.thinking_content.is_none());
    }

    #[test]
    fn parse_response_empty_reasoning_fields_are_none() {
        // Empty reasoning strings should produce None, not Some("").
        let response = json!({
            "choices": [{
                "message": {
                    "content": "Answer.",
                    "reasoning_content": "",
                    "reasoning": ""
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let response = OpenAIProvider::parse_response("o3", &response).unwrap();

        assert!(response.thinking_content.is_none());
    }

    #[test]
    fn parse_response_reasoning_content_takes_precedence_over_reasoning() {
        // When both fields exist, reasoning_content should take precedence.
        let response = json!({
            "choices": [{
                "message": {
                    "content": "Answer.",
                    "reasoning_content": "From reasoning_content field",
                    "reasoning": "From reasoning field"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let response = OpenAIProvider::parse_response("o3", &response).unwrap();

        assert_eq!(
            response.thinking_content.as_deref(),
            Some("From reasoning_content field")
        );
    }

    // -- Reasoning tokens parsing tests --------------------------------------

    #[test]
    fn parse_response_reasoning_tokens_from_completion_details() {
        let response = json!({
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "completion_tokens_details": {
                    "reasoning_tokens": 30
                }
            }
        });
        let response = OpenAIProvider::parse_response("o3", &response).unwrap();

        assert_eq!(response.usage.output_tokens, 50);
        assert_eq!(response.usage.reasoning_tokens, 30);
    }

    #[test]
    fn parse_response_reasoning_tokens_missing_details_is_zero() {
        let response = json!({
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 50}
        });
        let response = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();

        assert_eq!(response.usage.reasoning_tokens, 0);
    }

    #[test]
    fn parse_response_reasoning_tokens_missing_field_is_zero() {
        // completion_tokens_details exists but without reasoning_tokens.
        let response = json!({
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "completion_tokens_details": {
                    "accepted_prediction_tokens": 0
                }
            }
        });
        let response = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();

        assert_eq!(response.usage.reasoning_tokens, 0);
    }

    // -- Missing/malformed response tests -----------------------------------

    #[test]
    fn parse_response_missing_choices_is_error() {
        let response = json!({
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let result = OpenAIProvider::parse_response("gpt-4o-mini", &response);
        assert!(result.is_err());
    }

    #[test]
    fn parse_response_empty_choices_is_error() {
        let response = json!({
            "choices": [],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let result = OpenAIProvider::parse_response("gpt-4o-mini", &response);
        assert!(result.is_err());
    }

    // -- Retry classification tests (via kyomi_core::Error::is_transient) ----

    #[test]
    fn rate_limit_error_is_transient() {
        let err = kyomi_core::Error::TooManyRequests("OpenAI API rate limited".into(), 0);
        assert!(err.is_transient());
    }

    #[test]
    fn service_unavailable_is_transient() {
        let err = kyomi_core::Error::ServiceUnavailable("OpenAI API server error (503)".into());
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
    fn internal_error_is_not_transient() {
        let err = kyomi_core::Error::Internal("failed to parse response".into());
        assert!(!err.is_transient());
    }

    // -- Constructor tests --------------------------------------------------

    #[test]
    fn provider_default_model_and_url() {
        let provider = OpenAIProvider::with_base_url(
            "test-key".into(),
            None,
            "http://localhost:8080/mock".into(),
        )
        .unwrap();
        assert_eq!(provider.model(), DEFAULT_MODEL);
        assert_eq!(provider.base.base_url, "http://localhost:8080/mock");
    }

    #[test]
    fn provider_custom_model() {
        let provider = OpenAIProvider::with_base_url(
            "test-key".into(),
            Some("gpt-4o".into()),
            "http://localhost:8080".into(),
        )
        .unwrap();
        assert_eq!(provider.model(), "gpt-4o");
    }

    // -- Arguments stringification contract ---------------------------------

    #[test]
    fn tool_call_arguments_are_stringified_json() {
        // This is a critical contract: OpenAI requires `arguments` to be a
        // JSON string, not a JSON object. Verify that our conversion does this.
        let tool_calls = vec![ToolCall {
            id: "call_1".into(),
            name: "query".into(),
            arguments: json!({"sql": "SELECT 1", "limit": 10}),
        }];
        let messages = vec![Message::assistant_with_tool_calls("", tool_calls)];
        let names = HashMap::new();
        let msgs = OpenAIProvider::convert_messages_to_openai(&messages, &names);

        let args = &msgs[0]["tool_calls"][0]["function"]["arguments"];
        // Must be a string, not an object.
        assert!(args.is_string());
        // Parse it back and verify structure.
        let parsed: serde_json::Value = serde_json::from_str(args.as_str().unwrap()).unwrap();
        assert_eq!(parsed["sql"], "SELECT 1");
        assert_eq!(parsed["limit"], 10);
    }

    // -- Response parsing: arguments deserialization -------------------------

    #[test]
    fn parse_response_deserializes_tool_call_arguments() {
        // OpenAI returns arguments as a JSON string. Verify we parse it back
        // into a serde_json::Value for internal use.
        let response = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "query_datasource",
                            "arguments": "{\"sql_query\":\"SELECT * FROM orders\",\"datasource\":\"prod-pg\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 100, "completion_tokens": 30}
        });
        let response = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();

        let tc = &response.tool_calls.unwrap()[0];
        assert_eq!(tc.arguments["sql_query"], "SELECT * FROM orders");
        assert_eq!(tc.arguments["datasource"], "prod-pg");
    }

    #[test]
    fn parse_response_malformed_arguments_defaults_to_empty_object() {
        // If arguments is not valid JSON, we should default to {}.
        let response = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "some_tool",
                            "arguments": "not valid json {"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let response = OpenAIProvider::parse_response("gpt-4o-mini", &response).unwrap();

        let tc = &response.tool_calls.unwrap()[0];
        assert!(tc.arguments.is_object());
        assert!(tc.arguments.as_object().unwrap().is_empty());
    }

    // -- Cost extraction tests -------------------------------------------------

    #[test]
    fn parse_response_cost_from_openrouter() {
        let response = json!({
            "choices": [{"message": {"content": "Hello"}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 4,
                "total_tokens": 14,
                "cost": 0.00014
            }
        });
        let response = OpenAIProvider::parse_response("anthropic/claude-sonnet-4", &response).unwrap();
        assert_eq!(response.cost, Some(0.00014));
    }

    #[test]
    fn parse_response_cost_absent_for_direct_openai() {
        let response = json!({
            "choices": [{"message": {"content": "Hello"}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 4,
                "total_tokens": 14
            }
        });
        let response = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();
        assert!(response.cost.is_none());
    }

    #[test]
    fn parse_response_cost_zero_is_some() {
        // Free models on OpenRouter report cost: 0
        let response = json!({
            "choices": [{"message": {"content": "Hello"}, "finish_reason": "stop"}],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 4,
                "cost": 0.0
            }
        });
        let response = OpenAIProvider::parse_response("meta-llama/llama-3-8b", &response).unwrap();
        assert_eq!(response.cost, Some(0.0));
    }
}
