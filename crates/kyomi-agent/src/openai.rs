// SPDX-License-Identifier: AGPL-3.0-or-later

//! OpenAI-compatible HTTP client for chat completions.
//!
//! Calls `POST {base_url}` using `reqwest`. Supports any OpenAI-compatible API
//! (OpenAI, Azure OpenAI, OpenRouter, Groq, Together, Ollama, vLLM, etc.) via
//! a configurable `base_url`.
//!
//! Handles message/tool conversion, retry logic with exponential backoff,
//! token usage tracking, and cost estimation.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::json;
use tracing::{debug, info, warn};

use crate::types::{LLMResponse, Message, MessageRole, Tool, ToolCall, TokenUsage};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default OpenAI Chat Completions API endpoint.
const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";

/// Default model for chat completions.
pub const DEFAULT_MODEL: &str = "gpt-4o-mini";

/// Maximum retry attempts for transient API errors.
const MAX_RETRY_ATTEMPTS: usize = 5;

/// Exponential backoff delays between retry attempts (matches Anthropic: 4s, 8s, 16s, 32s, 60s).
const RETRY_DELAYS: [Duration; 5] = [
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(32),
    Duration::from_secs(60),
];

// ---------------------------------------------------------------------------
// Model Pricing
// ---------------------------------------------------------------------------

/// Look up pricing for a model using substring matching.
///
/// Returns `None` for unknown models.
fn get_model_pricing(model: &str) -> Option<crate::pricing::ModelPricing> {
    // Order matters: check more specific substrings first to avoid
    // "gpt-4o" matching before "gpt-4o-mini".
    if model.contains("gpt-4o-mini") {
        Some(crate::pricing::ModelPricing {
            input: 0.15,
            output: 0.60,
        })
    } else if model.contains("gpt-4.1-nano") {
        Some(crate::pricing::ModelPricing {
            input: 0.10,
            output: 0.40,
        })
    } else if model.contains("gpt-4.1-mini") {
        Some(crate::pricing::ModelPricing {
            input: 0.40,
            output: 1.60,
        })
    } else if model.contains("gpt-4.1") {
        Some(crate::pricing::ModelPricing {
            input: 2.00,
            output: 8.00,
        })
    } else if model.contains("gpt-4o") {
        Some(crate::pricing::ModelPricing {
            input: 2.50,
            output: 10.00,
        })
    } else if model.contains("o4-mini") || model.contains("o3-mini") {
        Some(crate::pricing::ModelPricing {
            input: 1.10,
            output: 4.40,
        })
    } else if model.contains("o3") {
        Some(crate::pricing::ModelPricing {
            input: 2.00,
            output: 8.00,
        })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// OpenAIProvider
// ---------------------------------------------------------------------------

/// HTTP client for any OpenAI-compatible chat completions API.
///
/// Handles message conversion, retry logic, and cost estimation.
/// Supports OpenAI, Azure OpenAI, OpenRouter, Groq, Together, Ollama,
/// vLLM, and any service that implements the OpenAI chat completions spec.
pub struct OpenAIProvider {
    base: crate::provider::ProviderBase,
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
        })
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
                    let content = Self::format_user_message(msg, user_names);
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

    /// Format a user message with sender attribution.
    ///
    /// If the message has a `user_id` and a matching name is found in `user_names`,
    /// formats as `[Name (last8chars)]: content`. Otherwise returns content as-is.
    ///
    /// This matches the Anthropic provider's `format_user_message` behavior exactly.
    fn format_user_message(msg: &Message, user_names: &HashMap<String, String>) -> String {
        let Some(user_id) = &msg.user_id else {
            return msg.content.clone();
        };

        let id_short = if user_id.len() >= 8 {
            &user_id[user_id.len() - 8..]
        } else {
            user_id.as_str()
        };

        if let Some(name) = user_names.get(user_id.as_str()) {
            format!("[{name} ({id_short})]: {}", msg.content)
        } else {
            format!("[User ({id_short})]: {}", msg.content)
        }
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

        // Build request body.
        let mut body = json!({
            "model": self.base.model,
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

        debug!(
            model = %self.base.model,
            message_count = openai_messages.len(),
            tool_count = tools.len(),
            "calling OpenAI-compatible API"
        );


        // Call with retry.
        let response_json = self.call_with_retry(&body).await?;

        // Parse response.
        Self::parse_response(&self.base.model, &response_json)
    }

    /// Execute the HTTP POST with retry logic.
    ///
    /// Retries up to [`MAX_RETRY_ATTEMPTS`] times with exponential backoff
    /// for transient errors (429 rate limit, 5xx server errors).
    /// Non-retryable errors (401 auth, 400 bad request) fail immediately.
    async fn call_with_retry(
        &self,
        body: &serde_json::Value,
    ) -> kyomi_core::Result<serde_json::Value> {
        let mut last_error = None;

        for (attempt, delay) in RETRY_DELAYS.iter().enumerate() {
            match self.call_api(body).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    if !Self::is_retryable(&e) {
                        return Err(e);
                    }
                    last_error = Some(e);
                    if attempt < RETRY_DELAYS.len() - 1 {
                        warn!(
                            attempt = attempt + 1,
                            max_attempts = MAX_RETRY_ATTEMPTS,
                            delay_secs = delay.as_secs(),
                            "OpenAI API transient error, retrying"
                        );
                        tokio::time::sleep(*delay).await;
                    }
                }
            }
        }

        // All retries exhausted — return the last error.
        Err(last_error.unwrap_or_else(|| {
            kyomi_core::Error::Internal("OpenAI API: all retries exhausted".into())
        }))
    }

    /// Send a single HTTP request to the OpenAI-compatible API.
    async fn call_api(
        &self,
        body: &serde_json::Value,
    ) -> kyomi_core::Result<serde_json::Value> {
        crate::provider::maybe_log_llm("openai", "request", body);

        let response = self
            .base
            .client
            .post(&self.base.base_url)
            .header("Authorization", format!("Bearer {}", self.base.api_key))
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| {
                kyomi_core::Error::Internal(format!("OpenAI API request failed: {e}"))
            })?;

        let status = response.status();

        if status.is_success() {
            let json: serde_json::Value = response.json().await.map_err(|e| {
                kyomi_core::Error::Internal(format!(
                    "OpenAI API: failed to parse response: {e}"
                ))
            })?;
            crate::provider::maybe_log_llm("openai", "response", &json);
            return Ok(json);
        }

        // Error response — try to extract the error message from the body.
        let error_body = response.text().await.unwrap_or_default();
        let error_msg =
            extract_error_message(&error_body).unwrap_or_else(|| format!("HTTP {status}"));

        match status.as_u16() {
            401 => Err(kyomi_core::Error::Unauthorized(format!(
                "OpenAI API authentication failed: {error_msg}"
            ))),
            400 => Err(kyomi_core::Error::BadRequest(format!(
                "OpenAI API bad request: {error_msg}"
            ))),
            429 => Err(kyomi_core::Error::Internal(format!(
                "OpenAI API rate limited: {error_msg}"
            ))),
            _ if status.is_server_error() => Err(kyomi_core::Error::Internal(format!(
                "OpenAI API server error ({status}): {error_msg}"
            ))),
            _ => Err(kyomi_core::Error::Internal(format!(
                "OpenAI API error ({status}): {error_msg}"
            ))),
        }
    }

    /// Check whether an error is transient and should be retried.
    fn is_retryable(error: &kyomi_core::Error) -> bool {
        match error {
            // Auth and bad request errors are permanent — do not retry.
            kyomi_core::Error::Unauthorized(_) | kyomi_core::Error::BadRequest(_) => false,
            // Internal errors from 429 and 5xx are retryable.
            kyomi_core::Error::Internal(msg) => {
                msg.contains("rate limited") || msg.contains("server error")
            }
            _ => false,
        }
    }

    // -----------------------------------------------------------------------
    // Response Parsing
    // -----------------------------------------------------------------------

    /// Parse the OpenAI API JSON response into an [`LLMResponse`].
    ///
    /// Extracts content and tool calls from `choices[0].message`, maps
    /// `finish_reason` to internal format, and calculates cost.
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
        // Some models (e.g. Qwen3) embed chain-of-thought inside
        // `<think>...</think>` tags in the content field. Strip that so only
        // the final answer is returned to the user.
        let raw_content = message
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("");
        let content = if let Some(pos) = raw_content.find("</think>") {
            // Thinking block completed — take everything after it.
            raw_content[pos + "</think>".len()..].trim().to_string()
        } else if raw_content.starts_with("<think>") || raw_content.contains("\n<think>") {
            // Truncated thinking (hit max_tokens mid-think) — discard all of it.
            String::new()
        } else {
            raw_content.to_string()
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

        // Calculate cost.
        let cost = calculate_cost(model, &usage);

        info!(
            model = model,
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            cost = format!("${cost:.6}"),
            finish_reason = %finish_reason,
            "OpenAI API call complete"
        );

        Ok(LLMResponse {
            content,
            finish_reason,
            usage,
            tool_calls,
            cost: Some(cost),
        })
    }

    /// Parse the `usage` object from the OpenAI response.
    fn parse_usage(response: &serde_json::Value) -> TokenUsage {
        let usage = response.get("usage");
        TokenUsage {
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
        }
    }
}

// ---------------------------------------------------------------------------
// Cost Calculation (free function, testable independently)
// ---------------------------------------------------------------------------

/// Calculate estimated cost in USD for an OpenAI API call.
///
/// Looks up the model's pricing (with gpt-4o-mini as fallback for unknown models)
/// and delegates to [`crate::pricing::calculate_cost`]. OpenAI does not expose
/// prompt cache tokens, so `cache_creation_input_tokens` and
/// `cache_read_input_tokens` are always 0 — the cache terms evaluate to zero
/// and the formula reduces to `input_cost + output_cost`.
pub fn calculate_cost(model: &str, usage: &TokenUsage) -> f64 {
    let pricing = get_model_pricing(model).unwrap_or_else(|| {
        warn!(
            model = model,
            "unknown model for cost calculation, using gpt-4o-mini pricing as fallback"
        );
        crate::pricing::ModelPricing {
            input: 0.15,
            output: 0.60,
        }
    });

    crate::pricing::calculate_cost(&pricing, usage)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Try to extract a human-readable error message from an OpenAI error response body.
///
/// OpenAI errors follow: `{ "error": { "message": "...", "type": "...", ... } }`
fn extract_error_message(body: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    json.get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(String::from)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- User attribution tests ---------------------------------------------

    #[test]
    fn format_user_message_no_user_id() {
        let msg = Message::user("hello");
        let names = HashMap::new();
        let result = OpenAIProvider::format_user_message(&msg, &names);
        assert_eq!(result, "hello");
    }

    #[test]
    fn format_user_message_with_known_user() {
        let msg = Message::user_with_id("hello", "user-abcd-1234-efgh");
        let mut names = HashMap::new();
        names.insert("user-abcd-1234-efgh".to_string(), "Jason Adams".to_string());
        let result = OpenAIProvider::format_user_message(&msg, &names);
        assert_eq!(result, "[Jason Adams (234-efgh)]: hello");
    }

    #[test]
    fn format_user_message_with_unknown_user() {
        let msg = Message::user_with_id("hello", "user-abcd-1234-efgh");
        let names = HashMap::new();
        let result = OpenAIProvider::format_user_message(&msg, &names);
        assert_eq!(result, "[User (234-efgh)]: hello");
    }

    #[test]
    fn format_user_message_short_user_id() {
        let msg = Message::user_with_id("hello", "abc");
        let names = HashMap::new();
        let result = OpenAIProvider::format_user_message(&msg, &names);
        assert_eq!(result, "[User (abc)]: hello");
    }

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
        let messages = vec![Message::assistant_with_tool_calls("Let me search.", tool_calls)];
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
        let messages =
            vec![Message::assistant_with_tool_calls("Searching multiple.", tool_calls)];
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
        let result = OpenAIProvider::parse_response("gpt-4o-mini", &response).unwrap();

        assert_eq!(result.content, "Hello, how can I help?");
        assert_eq!(result.finish_reason, "end_turn");
        assert_eq!(result.usage.input_tokens, 100);
        assert_eq!(result.usage.output_tokens, 20);
        assert!(result.tool_calls.is_none());
        assert!(result.cost.is_some());
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
        let result = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();

        assert_eq!(result.content, "Let me search.");
        assert_eq!(result.finish_reason, "tool_use");

        let tool_calls = result.tool_calls.unwrap();
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
        let result = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();

        assert_eq!(result.content, "");
        assert!(result.tool_calls.is_some());
        assert_eq!(result.tool_calls.as_ref().unwrap().len(), 1);
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
        let result = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();

        let tool_calls = result.tool_calls.unwrap();
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
        let result = OpenAIProvider::parse_response("gpt-4o-mini", &response).unwrap();
        assert_eq!(result.finish_reason, "end_turn");
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
        let result = OpenAIProvider::parse_response("gpt-4o-mini", &response).unwrap();
        assert_eq!(result.finish_reason, "tool_use");
    }

    #[test]
    fn parse_response_finish_reason_length() {
        let response = json!({
            "choices": [{"message": {"content": "truncated..."}, "finish_reason": "length"}],
            "usage": {"prompt_tokens": 50000, "completion_tokens": 4096}
        });
        let result = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();
        assert_eq!(result.finish_reason, "max_tokens");
    }

    #[test]
    fn parse_response_finish_reason_missing_defaults_to_unknown() {
        let response = json!({
            "choices": [{"message": {"content": "Hello"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let result = OpenAIProvider::parse_response("gpt-4o-mini", &response).unwrap();
        assert_eq!(result.finish_reason, "unknown");
    }

    // -- Usage parsing tests ------------------------------------------------

    #[test]
    fn parse_response_missing_usage_defaults_to_zero() {
        let response = json!({
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}]
        });
        let result = OpenAIProvider::parse_response("gpt-4o-mini", &response).unwrap();

        assert_eq!(result.usage.input_tokens, 0);
        assert_eq!(result.usage.output_tokens, 0);
        assert_eq!(result.usage.cache_creation_input_tokens, 0);
        assert_eq!(result.usage.cache_read_input_tokens, 0);
    }

    #[test]
    fn parse_response_cache_tokens_always_zero() {
        let response = json!({
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1000, "completion_tokens": 500}
        });
        let result = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();

        // OpenAI doesn't expose cache tokens the way Anthropic does.
        assert_eq!(result.usage.cache_creation_input_tokens, 0);
        assert_eq!(result.usage.cache_read_input_tokens, 0);
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

    // -- Cost calculation tests ---------------------------------------------

    #[test]
    fn cost_calculation_gpt4o() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let cost = calculate_cost("gpt-4o", &usage);
        // 1M input * $2.50/M + 1M output * $10.00/M = $12.50
        assert!((cost - 12.50).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_gpt4o_mini() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let cost = calculate_cost("gpt-4o-mini", &usage);
        // 1M * $0.15/M + 1M * $0.60/M = $0.75
        assert!((cost - 0.75).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_gpt41() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        let cost = calculate_cost("gpt-4.1", &usage);
        // 1M * $2.00/M + 1M * $8.00/M = $10.00
        assert!((cost - 10.00).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_gpt41_mini() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        let cost = calculate_cost("gpt-4.1-mini", &usage);
        // 1M * $0.40/M + 1M * $1.60/M = $2.00
        assert!((cost - 2.00).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_gpt41_nano() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        let cost = calculate_cost("gpt-4.1-nano", &usage);
        // 1M * $0.10/M + 1M * $0.40/M = $0.50
        assert!((cost - 0.50).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_o3() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        let cost = calculate_cost("o3", &usage);
        // 1M * $2.00/M + 1M * $8.00/M = $10.00
        assert!((cost - 10.00).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_o3_mini_not_confused_with_o3() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        let cost = calculate_cost("o3-mini", &usage);
        // o3-mini: 1M * $1.10/M + 1M * $4.40/M = $5.50
        assert!((cost - 5.50).abs() < 0.001);
        // Ensure it's NOT priced as o3 ($10.00)
        assert!((cost - 10.00).abs() > 1.0);
    }

    #[test]
    fn cost_calculation_o4_mini() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        let cost = calculate_cost("o4-mini", &usage);
        // 1M * $1.10/M + 1M * $4.40/M = $5.50
        assert!((cost - 5.50).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_unknown_model_uses_fallback() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        let cost = calculate_cost("some-custom-model", &usage);
        // Fallback is gpt-4o-mini: 1M * $0.15/M + 1M * $0.60/M = $0.75
        assert!((cost - 0.75).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_zero_tokens() {
        let usage = TokenUsage::default();
        let cost = calculate_cost("gpt-4o", &usage);
        assert!((cost - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cost_calculation_substring_matching() {
        // Model names with extra suffixes should still match via substring.
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            ..Default::default()
        };
        // "gpt-4o-mini-2024-07-18" contains "gpt-4o-mini"
        let cost = calculate_cost("gpt-4o-mini-2024-07-18", &usage);
        assert!((cost - 0.15).abs() < 0.001);
    }

    // -- Model pricing tests ------------------------------------------------

    #[test]
    fn model_pricing_all_known_models() {
        assert!(get_model_pricing("gpt-4o").is_some());
        assert!(get_model_pricing("gpt-4o-mini").is_some());
        assert!(get_model_pricing("gpt-4.1").is_some());
        assert!(get_model_pricing("gpt-4.1-mini").is_some());
        assert!(get_model_pricing("gpt-4.1-nano").is_some());
        assert!(get_model_pricing("o3").is_some());
        assert!(get_model_pricing("o4-mini").is_some());
    }

    #[test]
    fn model_pricing_unknown_returns_none() {
        assert!(get_model_pricing("claude-sonnet-4-5-20250929").is_none());
        assert!(get_model_pricing("unknown-model").is_none());
    }

    #[test]
    fn model_pricing_gpt4o_mini_not_confused_with_gpt4o() {
        // "gpt-4o-mini" should match mini pricing, not gpt-4o pricing.
        let mini = get_model_pricing("gpt-4o-mini").unwrap();
        let full = get_model_pricing("gpt-4o").unwrap();
        assert!(mini.input < full.input);
        assert!(mini.output < full.output);
    }

    // -- Retry logic tests --------------------------------------------------

    #[test]
    fn is_retryable_rate_limit() {
        let err = kyomi_core::Error::Internal("OpenAI API rate limited: too fast".into());
        assert!(OpenAIProvider::is_retryable(&err));
    }

    #[test]
    fn is_retryable_server_error() {
        let err = kyomi_core::Error::Internal("OpenAI API server error (500): oops".into());
        assert!(OpenAIProvider::is_retryable(&err));
    }

    #[test]
    fn is_not_retryable_auth() {
        let err = kyomi_core::Error::Unauthorized("invalid key".into());
        assert!(!OpenAIProvider::is_retryable(&err));
    }

    #[test]
    fn is_not_retryable_bad_request() {
        let err = kyomi_core::Error::BadRequest("invalid params".into());
        assert!(!OpenAIProvider::is_retryable(&err));
    }

    #[test]
    fn is_not_retryable_not_found() {
        let err = kyomi_core::Error::NotFound("missing".into());
        assert!(!OpenAIProvider::is_retryable(&err));
    }

    #[test]
    fn is_not_retryable_generic_internal() {
        let err = kyomi_core::Error::Internal("Something unexpected happened".into());
        assert!(!OpenAIProvider::is_retryable(&err));
    }

    // -- Error message extraction tests -------------------------------------

    #[test]
    fn extract_error_message_valid_json() {
        let body = r#"{"error": {"message": "Invalid API key", "type": "invalid_request_error"}}"#;
        assert_eq!(
            extract_error_message(body),
            Some("Invalid API key".to_string())
        );
    }

    #[test]
    fn extract_error_message_no_message_field() {
        let body = r#"{"error": {"type": "server_error"}}"#;
        assert_eq!(extract_error_message(body), None);
    }

    #[test]
    fn extract_error_message_invalid_json() {
        let body = "not json";
        assert_eq!(extract_error_message(body), None);
    }

    #[test]
    fn extract_error_message_empty_body() {
        assert_eq!(extract_error_message(""), None);
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
        let result = OpenAIProvider::parse_response("gpt-4o", &response).unwrap();

        let tc = &result.tool_calls.unwrap()[0];
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
        let result = OpenAIProvider::parse_response("gpt-4o-mini", &response).unwrap();

        let tc = &result.tool_calls.unwrap()[0];
        assert!(tc.arguments.is_object());
        assert!(tc.arguments.as_object().unwrap().is_empty());
    }
}
