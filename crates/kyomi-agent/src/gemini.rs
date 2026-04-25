// SPDX-License-Identifier: AGPL-3.0-or-later

//! Direct HTTP client for the Google Gemini API.
//!
//! Calls `POST {base_url}/models/{model}:generateContent` using `reqwest`.
//! Handles retry logic with exponential backoff, token usage tracking, and
//! cost estimation.
//!
//! This is a direct HTTP implementation because no official Google Gemini Rust
//! SDK exists that fits our needs. It gives full control over tool schemas,
//! message formatting, and error handling.

use std::collections::HashMap;

use serde_json::json;
use tracing::{debug, info, warn};

use crate::types::{LLMResponse, Message, MessageRole, Tool, ToolCall, TokenUsage};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default base URL for the Gemini API.
const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Default model for Gemini completions.
pub const DEFAULT_MODEL: &str = "gemini-2.5-flash";


// ---------------------------------------------------------------------------
// Model Pricing
// ---------------------------------------------------------------------------

/// Look up pricing for a Gemini model using substring matching.
/// Uses the lower tier (under 200k context) for simplicity.
fn get_model_pricing(model: &str) -> crate::pricing::ModelPricing {
    if model.contains("gemini-2.5-pro") {
        crate::pricing::ModelPricing {
            input: 1.25,
            output: 10.00,
        }
    } else if model.contains("gemini-2.5-flash") {
        crate::pricing::ModelPricing {
            input: 0.15,
            output: 0.60,
        }
    } else if model.contains("gemini-2.0-flash") {
        crate::pricing::ModelPricing {
            input: 0.10,
            output: 0.40,
        }
    } else {
        // Fallback: use gemini-2.0-flash pricing for unknown models.
        warn!(
            model = model,
            "unknown Gemini model for cost calculation, using gemini-2.0-flash pricing as fallback"
        );
        crate::pricing::ModelPricing {
            input: 0.10,
            output: 0.40,
        }
    }
}

// ---------------------------------------------------------------------------
// GeminiProvider
// ---------------------------------------------------------------------------

/// HTTP client for the Google Gemini API.
///
/// Handles message conversion, retry logic, and cost estimation.
pub struct GeminiProvider {
    base: crate::provider::ProviderBase,
}

impl GeminiProvider {
    /// Create a new Gemini provider.
    ///
    /// # Arguments
    /// * `api_key` - Google AI API key.
    /// * `model` - Model name; uses [`DEFAULT_MODEL`] if `None`.
    /// * `base_url` - Base URL; uses [`DEFAULT_BASE_URL`] if `None`.
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
                DEFAULT_BASE_URL,
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

    /// Convert internal [`Message`] list to Gemini API format.
    ///
    /// Returns `(system_instruction, contents)` where:
    /// - `system_instruction` is the extracted system message (if present).
    /// - `contents` is the ordered list of user/model/function messages with
    ///   strict role alternation enforced via merging.
    ///
    /// User messages are annotated with sender attribution using the provided
    /// `user_names` map (user_id -> display name), matching the Anthropic
    /// provider's behavior.
    pub fn convert_messages_to_gemini(
        messages: &[Message],
        user_names: &HashMap<String, String>,
    ) -> (Option<serde_json::Value>, Vec<serde_json::Value>) {
        let mut system_instruction = None;
        let mut contents: Vec<serde_json::Value> = Vec::new();

        for msg in messages {
            match msg.role {
                MessageRole::System => {
                    system_instruction = Some(json!({
                        "parts": [{ "text": &msg.content }]
                    }));
                }

                MessageRole::User => {
                    let content = crate::provider::format_user_message(msg, user_names);
                    let entry = json!({
                        "role": "user",
                        "parts": [{ "text": content }]
                    });
                    Self::push_or_merge(&mut contents, entry, "user");
                }

                MessageRole::Assistant => {
                    let mut parts: Vec<serde_json::Value> = Vec::new();

                    if !msg.content.is_empty() {
                        parts.push(json!({ "text": &msg.content }));
                    }

                    if let Some(tool_calls) = &msg.tool_calls {
                        for tc in tool_calls {
                            parts.push(json!({
                                "functionCall": {
                                    "name": &tc.name,
                                    "args": &tc.arguments
                                }
                            }));
                        }
                    }

                    // If no parts at all (empty content, no tool calls), add empty text.
                    if parts.is_empty() {
                        parts.push(json!({ "text": "" }));
                    }

                    let entry = json!({
                        "role": "model",
                        "parts": parts
                    });
                    Self::push_or_merge(&mut contents, entry, "model");
                }

                MessageRole::Tool => {
                    let tool_name = msg
                        .name
                        .as_deref()
                        .unwrap_or("unknown_tool");

                    // Try to parse content as JSON for structured responses;
                    // fall back to a string wrapper if parsing fails.
                    let result_value: serde_json::Value =
                        serde_json::from_str(&msg.content)
                            .unwrap_or_else(|_| json!({ "output": &msg.content }));

                    let entry = json!({
                        "role": "function",
                        "parts": [{
                            "functionResponse": {
                                "name": tool_name,
                                "response": result_value
                            }
                        }]
                    });
                    Self::push_or_merge(&mut contents, entry, "function");
                }
            }
        }

        (system_instruction, contents)
    }

    /// Push a new message to contents, merging with the last entry if they
    /// share the same role (Gemini requires strict role alternation).
    fn push_or_merge(
        contents: &mut Vec<serde_json::Value>,
        new_entry: serde_json::Value,
        role: &str,
    ) {
        if let Some(last) = contents.last_mut()
            && last.get("role").and_then(|r| r.as_str()) == Some(role)
        {
            // Merge parts into the existing entry.
            if let (Some(existing_parts), Some(new_parts)) = (
                last.get_mut("parts").and_then(|p| p.as_array_mut()),
                new_entry.get("parts").and_then(|p| p.as_array()),
            ) {
                existing_parts.extend(new_parts.iter().cloned());
                return;
            }
        }
        contents.push(new_entry);
    }

    /// Convert [`Tool`] definitions to Gemini's tool format.
    ///
    /// Returns `None` if tools is empty (don't send empty declarations).
    pub fn convert_tools_to_gemini(tools: &[Tool]) -> Option<serde_json::Value> {
        if tools.is_empty() {
            return None;
        }

        let declarations: Vec<serde_json::Value> = tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                })
            })
            .collect();

        Some(json!({
            "functionDeclarations": declarations
        }))
    }

    // -----------------------------------------------------------------------
    // API Call
    // -----------------------------------------------------------------------

    /// Send a completion request to the Gemini API.
    ///
    /// # Arguments
    /// * `messages` - Conversation history.
    /// * `tools` - Available tools (may be empty).
    /// * `temperature` - Sampling temperature; defaults to 0.7.
    /// * `max_tokens` - Maximum tokens to generate.
    /// * `user_names` - Map of user_id -> display name for user attribution.
    pub async fn complete(
        &self,
        messages: &[Message],
        tools: &[Tool],
        temperature: Option<f32>,
        max_tokens: u32,
        user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        let (system_instruction, contents) =
            Self::convert_messages_to_gemini(messages, user_names);

        // Build request body.
        let message_count = contents.len();
        let mut gen_config = json!({
            "maxOutputTokens": max_tokens,
        });
        if let Some(temp) = temperature {
            gen_config["temperature"] = json!(temp);
        }
        let mut body = json!({
            "contents": contents,
            "generationConfig": gen_config,
        });

        if let Some(system) = system_instruction {
            body["systemInstruction"] = system;
        }

        if let Some(tools_json) = Self::convert_tools_to_gemini(tools) {
            body["tools"] = json!([tools_json]);
        }

        debug!(
            model = %self.base.model,
            message_count,
            tool_count = tools.len(),
            "calling Gemini API"
        );

        // Call with retry.
        let response_json =
            kyomi_core::retry::retry_with_backoff(|| self.call_api(&body)).await?;

        // Parse response.
        Self::parse_response(&self.base.model, &response_json)
    }

    /// Send a single HTTP request to the Gemini API.
    async fn call_api(
        &self,
        body: &serde_json::Value,
    ) -> kyomi_core::Result<serde_json::Value> {
        crate::provider::maybe_log_llm("gemini", "request", body);

        let url = format!(
            "{}/models/{}:generateContent",
            self.base.base_url, self.base.model
        );

        let response = self
            .base
            .client
            .post(&url)
            .header("content-type", "application/json")
            .header("x-goog-api-key", &self.base.api_key)
            .json(body)
            .send()
            .await
            .map_err(|e| {
                kyomi_core::Error::ServiceUnavailable(format!("Gemini API request failed: {e}"))
            })?;

        let status = response.status();

        if status.is_success() {
            let json: serde_json::Value = response.json().await.map_err(|e| {
                kyomi_core::Error::Internal(format!(
                    "Gemini API: failed to parse response: {e}"
                ))
            })?;
            crate::provider::maybe_log_llm("gemini", "response", &json);
            return Ok(json);
        }

        // Error response -- try to extract the error message from the body.
        let error_body = response.text().await.unwrap_or_default();
        let error_msg = crate::provider::extract_error_message(&error_body)
            .unwrap_or_else(|| format!("HTTP {status}"));

        match status.as_u16() {
            401 | 403 => Err(kyomi_core::Error::Unauthorized(format!(
                "Gemini API authentication failed: {error_msg}"
            ))),
            400 => Err(kyomi_core::Error::BadRequest(format!(
                "Gemini API bad request: {error_msg}"
            ))),
            429 => Err(kyomi_core::Error::TooManyRequests(
                format!("Gemini API rate limited: {error_msg}"),
                0,
            )),
            502..=504 => Err(kyomi_core::Error::ServiceUnavailable(format!(
                "Gemini API unavailable ({status}): {error_msg}"
            ))),
            _ if status.is_server_error() => Err(kyomi_core::Error::ServiceUnavailable(format!(
                "Gemini API server error ({status}): {error_msg}"
            ))),
            _ => Err(kyomi_core::Error::Internal(format!(
                "Gemini API error ({status}): {error_msg}"
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // Response Parsing
    // -----------------------------------------------------------------------

    /// Parse the Gemini API JSON response into an [`LLMResponse`].
    fn parse_response(
        model: &str,
        response: &serde_json::Value,
    ) -> kyomi_core::Result<LLMResponse> {
        // Extract parts from candidates[0].content.parts.
        let parts = response
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("content"))
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array())
            .ok_or_else(|| {
                kyomi_core::Error::Internal(
                    "Gemini API response missing 'candidates[0].content.parts' array".into(),
                )
            })?;

        let mut text_parts: Vec<&str> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut tool_call_idx: usize = 0;

        for part in parts.iter() {
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                text_parts.push(text);
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let arguments = fc
                    .get("args")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);

                tool_calls.push(ToolCall {
                    id: format!("gemini_call_{tool_call_idx}"),
                    name,
                    arguments,
                });
                tool_call_idx += 1;
            }
        }

        let content = text_parts.join("");

        // Extract usage from usageMetadata.
        let usage = Self::parse_usage(response);

        // Extract and map finish reason.
        let raw_finish_reason = response
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("finishReason"))
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");

        let finish_reason = if !tool_calls.is_empty() {
            "tool_use".to_string()
        } else {
            match raw_finish_reason {
                "STOP" => "end_turn".to_string(),
                "MAX_TOKENS" => "max_tokens".to_string(),
                other => other.to_lowercase(),
            }
        };

        // Calculate cost.
        let cost = calculate_cost(model, &usage);

        info!(
            model = model,
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            cost = format!("${cost:.6}"),
            finish_reason = %finish_reason,
            "Gemini API call complete"
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
            cost: Some(cost),
        })
    }

    /// Parse the `usageMetadata` object from the Gemini response.
    fn parse_usage(response: &serde_json::Value) -> TokenUsage {
        let usage = response.get("usageMetadata");
        TokenUsage {
            input_tokens: usage
                .and_then(|u| u.get("promptTokenCount"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            output_tokens: usage
                .and_then(|u| u.get("candidatesTokenCount"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Cost Calculation (free function, testable independently)
// ---------------------------------------------------------------------------

/// Calculate estimated cost in USD for a Gemini API call.
/// Delegates to [`crate::pricing::calculate_cost_with_fallback`]. Gemini's
/// `get_model_pricing` always returns a value (fallback built in), so the
/// dummy fallback here is never reached.
pub fn calculate_cost(model: &str, usage: &TokenUsage) -> f64 {
    crate::pricing::calculate_cost_with_fallback(
        model,
        usage,
        |m| Some(get_model_pricing(m)),
        crate::pricing::ModelPricing { input: 0.0, output: 0.0 },
        "gemini",
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Message conversion: system extraction ------------------------------

    #[test]
    fn convert_system_message_extracted() {
        let messages = vec![Message::system("You are helpful."), Message::user("hi")];
        let names = HashMap::new();
        let (system, contents) =
            GeminiProvider::convert_messages_to_gemini(&messages, &names);

        let system = system.unwrap();
        assert_eq!(system["parts"][0]["text"], "You are helpful.");

        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "hi");
    }

    #[test]
    fn convert_no_system_message() {
        let messages = vec![Message::user("hello")];
        let names = HashMap::new();
        let (system, contents) =
            GeminiProvider::convert_messages_to_gemini(&messages, &names);

        assert!(system.is_none());
        assert_eq!(contents.len(), 1);
    }

    // -- Message conversion: user messages ----------------------------------

    #[test]
    fn convert_user_message() {
        let messages = vec![Message::user("hello world")];
        let names = HashMap::new();
        let (_, contents) = GeminiProvider::convert_messages_to_gemini(&messages, &names);

        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "hello world");
    }

    #[test]
    fn convert_user_message_with_attribution() {
        let msg = Message::user_with_id("show revenue", "user-abcd-1234-efgh");
        let mut names = HashMap::new();
        names.insert("user-abcd-1234-efgh".to_string(), "Alice".to_string());
        let (_, contents) =
            GeminiProvider::convert_messages_to_gemini(&[msg], &names);

        assert_eq!(contents[0]["parts"][0]["text"], "[Alice (234-efgh)]: show revenue");
    }

    // -- Message conversion: model (assistant) messages ---------------------

    #[test]
    fn convert_assistant_message_simple() {
        let messages = vec![
            Message::user("hello"),
            Message::assistant("I can help you."),
        ];
        let names = HashMap::new();
        let (_, contents) = GeminiProvider::convert_messages_to_gemini(&messages, &names);

        assert_eq!(contents.len(), 2);
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["text"], "I can help you.");
    }

    #[test]
    fn convert_assistant_with_tool_calls() {
        let tool_calls = vec![ToolCall {
            id: "tc_1".into(),
            name: "search_catalog".into(),
            arguments: json!({"query": "revenue"}),
        }];
        let messages = vec![
            Message::user("show revenue"),
            Message::assistant_with_tool_calls("Let me search.", tool_calls),
        ];
        let names = HashMap::new();
        let (_, contents) = GeminiProvider::convert_messages_to_gemini(&messages, &names);

        assert_eq!(contents[1]["role"], "model");
        let parts = contents[1]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "Let me search.");
        assert_eq!(parts[1]["functionCall"]["name"], "search_catalog");
        assert_eq!(parts[1]["functionCall"]["args"]["query"], "revenue");
    }

    #[test]
    fn convert_assistant_with_tool_calls_empty_content() {
        let tool_calls = vec![ToolCall {
            id: "tc_1".into(),
            name: "search_catalog".into(),
            arguments: json!({"query": "revenue"}),
        }];
        let messages = vec![
            Message::user("show revenue"),
            Message::assistant_with_tool_calls("", tool_calls),
        ];
        let names = HashMap::new();
        let (_, contents) = GeminiProvider::convert_messages_to_gemini(&messages, &names);

        let parts = contents[1]["parts"].as_array().unwrap();
        // Empty content not included; only the functionCall part.
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["functionCall"]["name"], "search_catalog");
    }

    #[test]
    fn convert_assistant_with_multiple_tool_calls() {
        let tool_calls = vec![
            ToolCall {
                id: "tc_1".into(),
                name: "search_catalog".into(),
                arguments: json!({"query": "revenue"}),
            },
            ToolCall {
                id: "tc_2".into(),
                name: "get_table_info".into(),
                arguments: json!({"table_name": "orders"}),
            },
        ];
        let messages = vec![
            Message::user("analyze data"),
            Message::assistant_with_tool_calls("Let me look into this.", tool_calls),
        ];
        let names = HashMap::new();
        let (_, contents) = GeminiProvider::convert_messages_to_gemini(&messages, &names);

        let parts = contents[1]["parts"].as_array().unwrap();
        // text + 2 functionCall parts = 3.
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0]["text"], "Let me look into this.");
        assert_eq!(parts[1]["functionCall"]["name"], "search_catalog");
        assert_eq!(parts[2]["functionCall"]["name"], "get_table_info");
    }

    // -- Message conversion: tool results -----------------------------------

    #[test]
    fn convert_tool_result_message() {
        let messages = vec![
            Message::user("search"),
            Message::assistant_with_tool_calls(
                "Searching.",
                vec![ToolCall {
                    id: "tc_1".into(),
                    name: "search_catalog".into(),
                    arguments: json!({"query": "orders"}),
                }],
            ),
            Message::tool_result("tc_1", "search_catalog", r#"{"tables": ["orders"]}"#),
        ];
        let names = HashMap::new();
        let (_, contents) = GeminiProvider::convert_messages_to_gemini(&messages, &names);

        assert_eq!(contents.len(), 3);
        assert_eq!(contents[2]["role"], "function");
        let parts = contents[2]["parts"].as_array().unwrap();
        assert_eq!(parts[0]["functionResponse"]["name"], "search_catalog");
        // Content is valid JSON, so it gets parsed into a structured object
        assert_eq!(
            parts[0]["functionResponse"]["response"]["tables"][0],
            "orders"
        );
    }

    // -- Message conversion: role alternation merging -----------------------

    #[test]
    fn merge_consecutive_user_messages() {
        let messages = vec![
            Message::user("hello"),
            Message::user("how are you"),
        ];
        let names = HashMap::new();
        let (_, contents) = GeminiProvider::convert_messages_to_gemini(&messages, &names);

        // Should be merged into one user message with two parts.
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        let parts = contents[0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "hello");
        assert_eq!(parts[1]["text"], "how are you");
    }

    #[test]
    fn merge_consecutive_model_messages() {
        let messages = vec![
            Message::user("hello"),
            Message::assistant("first response"),
            Message::assistant("second response"),
        ];
        let names = HashMap::new();
        let (_, contents) = GeminiProvider::convert_messages_to_gemini(&messages, &names);

        assert_eq!(contents.len(), 2);
        assert_eq!(contents[1]["role"], "model");
        let parts = contents[1]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "first response");
        assert_eq!(parts[1]["text"], "second response");
    }

    #[test]
    fn merge_consecutive_function_results() {
        let messages = vec![
            Message::user("search"),
            Message::assistant_with_tool_calls(
                "",
                vec![
                    ToolCall {
                        id: "tc_1".into(),
                        name: "search_catalog".into(),
                        arguments: json!({"query": "rev"}),
                    },
                    ToolCall {
                        id: "tc_2".into(),
                        name: "get_table_info".into(),
                        arguments: json!({"table": "t"}),
                    },
                ],
            ),
            Message::tool_result("tc_1", "search_catalog", "result1"),
            Message::tool_result("tc_2", "get_table_info", "result2"),
        ];
        let names = HashMap::new();
        let (_, contents) = GeminiProvider::convert_messages_to_gemini(&messages, &names);

        // user, model, function (merged)
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[2]["role"], "function");
        let parts = contents[2]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["functionResponse"]["name"], "search_catalog");
        assert_eq!(parts[1]["functionResponse"]["name"], "get_table_info");
    }

    #[test]
    fn no_merge_when_roles_alternate() {
        let messages = vec![
            Message::user("hello"),
            Message::assistant("hi"),
            Message::user("thanks"),
        ];
        let names = HashMap::new();
        let (_, contents) = GeminiProvider::convert_messages_to_gemini(&messages, &names);

        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[2]["role"], "user");
    }

    // -- Tool conversion tests ----------------------------------------------

    #[test]
    fn convert_tools_empty() {
        let result = GeminiProvider::convert_tools_to_gemini(&[]);
        assert!(result.is_none());
    }

    #[test]
    fn convert_tools_single() {
        let tools = vec![Tool {
            name: "search_catalog".into(),
            description: "Search for tables.".into(),
            parameters: json!({"type": "object", "properties": {"query": {"type": "string"}}}),
        }];
        let result = GeminiProvider::convert_tools_to_gemini(&tools).unwrap();

        let declarations = result["functionDeclarations"].as_array().unwrap();
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0]["name"], "search_catalog");
        assert_eq!(declarations[0]["description"], "Search for tables.");
        assert_eq!(declarations[0]["parameters"]["type"], "object");
    }

    #[test]
    fn convert_tools_multiple() {
        let tools = vec![
            Tool {
                name: "tool_a".into(),
                description: "A".into(),
                parameters: json!({"type": "object"}),
            },
            Tool {
                name: "tool_b".into(),
                description: "B".into(),
                parameters: json!({"type": "object"}),
            },
        ];
        let result = GeminiProvider::convert_tools_to_gemini(&tools).unwrap();

        let declarations = result["functionDeclarations"].as_array().unwrap();
        assert_eq!(declarations.len(), 2);
        assert_eq!(declarations[0]["name"], "tool_a");
        assert_eq!(declarations[1]["name"], "tool_b");
    }

    #[test]
    fn convert_tools_preserves_json_schema() {
        let tools = vec![Tool {
            name: "query_datasource".into(),
            description: "Execute a SQL query.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "sql_query": { "type": "string", "description": "The SQL query" },
                    "datasource": { "type": "string", "description": "Datasource slug" }
                },
                "required": ["sql_query", "datasource"]
            }),
        }];
        let result = GeminiProvider::convert_tools_to_gemini(&tools).unwrap();

        let decl = &result["functionDeclarations"][0];
        assert_eq!(decl["parameters"]["type"], "object");
        let required = decl["parameters"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("sql_query")));
        assert!(required.contains(&json!("datasource")));
    }

    // -- Response parsing tests ---------------------------------------------

    #[test]
    fn parse_response_text_only() {
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [{ "text": "Hello, how can I help?" }],
                    "role": "model"
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 20
            }
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response).unwrap();

        assert_eq!(result.content, "Hello, how can I help?");
        assert_eq!(result.finish_reason, "end_turn");
        assert_eq!(result.usage.input_tokens, 100);
        assert_eq!(result.usage.output_tokens, 20);
        assert_eq!(result.usage.cache_creation_input_tokens, 0);
        assert_eq!(result.usage.cache_read_input_tokens, 0);
        assert!(result.tool_calls.is_none());
        assert!(result.cost.is_some());
    }

    #[test]
    fn parse_response_with_tool_calls() {
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [
                        { "text": "Let me search." },
                        {
                            "functionCall": {
                                "name": "search_catalog",
                                "args": { "query": "revenue" }
                            }
                        }
                    ],
                    "role": "model"
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 500,
                "candidatesTokenCount": 50
            }
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response).unwrap();

        assert_eq!(result.content, "Let me search.");
        assert_eq!(result.finish_reason, "tool_use");

        let tool_calls = result.tool_calls.unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "gemini_call_0");
        assert_eq!(tool_calls[0].name, "search_catalog");
        assert_eq!(tool_calls[0].arguments["query"], "revenue");
    }

    #[test]
    fn parse_response_tool_calls_only_no_text() {
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "search_catalog",
                            "args": { "query": "rev" }
                        }
                    }],
                    "role": "model"
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 200,
                "candidatesTokenCount": 30
            }
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response).unwrap();

        assert_eq!(result.content, "");
        assert_eq!(result.finish_reason, "tool_use");
        assert!(result.tool_calls.is_some());
        assert_eq!(result.tool_calls.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn parse_response_multiple_tool_calls() {
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [
                        { "text": "I'll search and query." },
                        { "functionCall": { "name": "search_catalog", "args": {"query": "sales"} } },
                        { "functionCall": { "name": "get_table_info", "args": {"table": "orders"} } }
                    ],
                    "role": "model"
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 500,
                "candidatesTokenCount": 100
            }
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response).unwrap();

        assert_eq!(result.content, "I'll search and query.");
        assert_eq!(result.finish_reason, "tool_use");
        let tool_calls = result.tool_calls.unwrap();
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].id, "gemini_call_0");
        assert_eq!(tool_calls[0].name, "search_catalog");
        assert_eq!(tool_calls[1].id, "gemini_call_1");
        assert_eq!(tool_calls[1].name, "get_table_info");
    }

    #[test]
    fn parse_response_generated_tool_call_ids() {
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [
                        { "functionCall": { "name": "tool_a", "args": {} } },
                        { "functionCall": { "name": "tool_b", "args": {} } },
                    ],
                    "role": "model"
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5
            }
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response).unwrap();

        let tool_calls = result.tool_calls.unwrap();
        assert_eq!(tool_calls[0].id, "gemini_call_0");
        assert_eq!(tool_calls[1].id, "gemini_call_1");
    }

    #[test]
    fn parse_response_multiple_text_parts() {
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [
                        { "text": "Part 1. " },
                        { "text": "Part 2." }
                    ],
                    "role": "model"
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5
            }
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response).unwrap();

        assert_eq!(result.content, "Part 1. Part 2.");
    }

    // -- Finish reason mapping tests ----------------------------------------

    #[test]
    fn parse_response_finish_reason_stop() {
        let response = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "ok" }], "role": "model" },
                "finishReason": "STOP"
            }],
            "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 5 }
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response).unwrap();
        assert_eq!(result.finish_reason, "end_turn");
    }

    #[test]
    fn parse_response_finish_reason_max_tokens() {
        let response = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "truncated..." }], "role": "model" },
                "finishReason": "MAX_TOKENS"
            }],
            "usageMetadata": { "promptTokenCount": 50000, "candidatesTokenCount": 4096 }
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response).unwrap();
        assert_eq!(result.finish_reason, "max_tokens");
    }

    #[test]
    fn parse_response_finish_reason_with_tool_calls_overrides() {
        // Even if finishReason is STOP, presence of tool calls means tool_use.
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": { "name": "search", "args": {} }
                    }],
                    "role": "model"
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 5 }
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response).unwrap();
        assert_eq!(result.finish_reason, "tool_use");
    }

    #[test]
    fn parse_response_finish_reason_missing_defaults_to_unknown() {
        let response = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "ok" }], "role": "model" }
            }],
            "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 5 }
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response).unwrap();
        assert_eq!(result.finish_reason, "unknown");
    }

    // -- Usage parsing tests ------------------------------------------------

    #[test]
    fn parse_response_missing_usage_defaults_to_zero() {
        let response = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "ok" }], "role": "model" },
                "finishReason": "STOP"
            }]
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response).unwrap();

        assert_eq!(result.usage.input_tokens, 0);
        assert_eq!(result.usage.output_tokens, 0);
    }

    #[test]
    fn parse_response_cache_tokens_always_zero() {
        let response = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "ok" }], "role": "model" },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 1000,
                "candidatesTokenCount": 500
            }
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response).unwrap();

        assert_eq!(result.usage.cache_creation_input_tokens, 0);
        assert_eq!(result.usage.cache_read_input_tokens, 0);
    }

    // -- Cost calculation tests ---------------------------------------------

    #[test]
    fn cost_calculation_gemini_25_flash() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let cost = calculate_cost("gemini-2.5-flash", &usage);
        // 1M * $0.15/M + 1M * $0.60/M = $0.75
        assert!((cost - 0.75).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_gemini_25_pro() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let cost = calculate_cost("gemini-2.5-pro", &usage);
        // 1M * $1.25/M + 1M * $10.00/M = $11.25
        assert!((cost - 11.25).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_gemini_20_flash() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let cost = calculate_cost("gemini-2.0-flash", &usage);
        // 1M * $0.10/M + 1M * $0.40/M = $0.50
        assert!((cost - 0.50).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_unknown_model_uses_fallback() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let cost = calculate_cost("gemini-unknown-model", &usage);
        // Fallback is gemini-2.0-flash: 1M * $0.10/M + 1M * $0.40/M = $0.50
        assert!((cost - 0.50).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_zero_tokens() {
        let usage = TokenUsage::default();
        let cost = calculate_cost("gemini-2.5-flash", &usage);
        assert!((cost - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cost_calculation_input_only() {
        let usage = TokenUsage {
            input_tokens: 500_000,
            output_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let cost = calculate_cost("gemini-2.5-flash", &usage);
        // 0.5M * $0.15/M = $0.075
        assert!((cost - 0.075).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_output_only() {
        let usage = TokenUsage {
            input_tokens: 0,
            output_tokens: 100_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let cost = calculate_cost("gemini-2.5-flash", &usage);
        // 0.1M * $0.60/M = $0.06
        assert!((cost - 0.06).abs() < 0.001);
    }

    #[test]
    fn cost_calculation_substring_matching() {
        // Model names with version suffixes should still match via substring.
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let cost = calculate_cost("gemini-2.5-pro-latest", &usage);
        // Should match gemini-2.5-pro: 1M * $1.25/M = $1.25
        assert!((cost - 1.25).abs() < 0.001);
    }

    // -- Response parsing: missing candidates is error ----------------------

    #[test]
    fn parse_response_missing_candidates_is_error() {
        let response = json!({
            "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 5 }
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response);
        assert!(result.is_err());
    }

    #[test]
    fn parse_response_empty_candidates_is_error() {
        let response = json!({
            "candidates": [],
            "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 5 }
        });
        let result = GeminiProvider::parse_response("gemini-2.5-flash", &response);
        assert!(result.is_err());
    }

    // -- Retry classification tests (via kyomi_core::Error::is_transient) ----

    #[test]
    fn rate_limit_error_is_transient() {
        let err = kyomi_core::Error::TooManyRequests("Gemini API rate limited".into(), 0);
        assert!(err.is_transient());
    }

    #[test]
    fn service_unavailable_is_transient() {
        let err = kyomi_core::Error::ServiceUnavailable("Gemini API server error (503)".into());
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
    fn provider_default_model() {
        let provider =
            GeminiProvider::with_base_url("test-key".into(), None, "http://localhost".into())
                .unwrap();
        assert_eq!(provider.model(), DEFAULT_MODEL);
    }

    #[test]
    fn provider_custom_model() {
        let provider = GeminiProvider::with_base_url(
            "test-key".into(),
            Some("gemini-2.5-pro".into()),
            "http://localhost".into(),
        )
        .unwrap();
        assert_eq!(provider.model(), "gemini-2.5-pro");
    }

    // -- Full conversation flow test ----------------------------------------

    #[test]
    fn convert_full_tool_use_conversation() {
        let messages = vec![
            Message::system("You are a helpful data analyst."),
            Message::user("Show me monthly revenue."),
            Message::assistant_with_tool_calls(
                "Let me search for revenue tables.",
                vec![ToolCall {
                    id: "tc_1".into(),
                    name: "search_catalog".into(),
                    arguments: json!({"query": "revenue monthly"}),
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
        let (system, contents) =
            GeminiProvider::convert_messages_to_gemini(&messages, &names);

        // System extracted.
        assert!(system.is_some());
        assert_eq!(system.unwrap()["parts"][0]["text"], "You are a helpful data analyst.");

        // 4 entries: user, model(+tool call), function, model.
        assert_eq!(contents.len(), 4);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[2]["role"], "function");
        assert_eq!(contents[3]["role"], "model");
    }

    // -- Model pricing hierarchy test ---------------------------------------

    #[test]
    fn model_pricing_pro_is_most_expensive() {
        let pro = get_model_pricing("gemini-2.5-pro");
        let flash_25 = get_model_pricing("gemini-2.5-flash");
        let flash_20 = get_model_pricing("gemini-2.0-flash");

        assert!(pro.input > flash_25.input);
        assert!(pro.output > flash_25.output);
        assert!(flash_25.input > flash_20.input);
        assert!(flash_25.output > flash_20.output);
    }
}
