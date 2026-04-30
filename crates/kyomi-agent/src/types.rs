// SPDX-License-Identifier: AGPL-3.0-or-later

//! LLM types for the agent system.
//!
//! Defines the core types used across the agent: messages, tool calls,
//! LLM responses, tool definitions, and token usage tracking.
//! These types match the Python `Message`, `LLMResponse`, `Tool`, and
//! `ToolCall` from `agent/llm_providers.py`.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// MessageRole
// ---------------------------------------------------------------------------

/// Role of a message in the agent conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// System prompt — always first in the conversation.
    System,
    /// User-sent message.
    User,
    /// Assistant (LLM) response.
    Assistant,
    /// Tool result returned to the LLM.
    Tool,
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

/// A message in the agent conversation.
///
/// Represents all message types: system prompts, user messages, assistant
/// responses (with optional tool calls), and tool results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// The role of the message sender.
    pub role: MessageRole,
    /// The text content of the message.
    pub content: String,
    /// Tool calls requested by the assistant (only on assistant messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// ID of the tool call this message is a result for (only on tool messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool name for tool result messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// User ID of the message sender (for user attribution in shared conversations).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// ISO 8601 timestamp when the message was created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Database message ID (present when loaded from DB).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

impl Message {
    /// Create a system prompt message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            user_id: None,
            timestamp: None,
            message_id: None,
        }
    }

    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            user_id: None,
            timestamp: None,
            message_id: None,
        }
    }

    /// Create a user message with user attribution.
    pub fn user_with_id(content: impl Into<String>, user_id: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            user_id: Some(user_id.into()),
            timestamp: None,
            message_id: None,
        }
    }

    /// Create an assistant message with text content only.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            user_id: None,
            timestamp: None,
            message_id: None,
        }
    }

    /// Create an assistant message with tool calls.
    pub fn assistant_with_tool_calls(
        content: impl Into<String>,
        tool_calls: Vec<ToolCall>,
    ) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
            user_id: None,
            timestamp: None,
            message_id: None,
        }
    }

    /// Create a tool result message.
    pub fn tool_result(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
            user_id: None,
            timestamp: None,
            message_id: None,
        }
    }
}

// ---------------------------------------------------------------------------
// ToolCall
// ---------------------------------------------------------------------------

/// A tool call requested by the LLM.
///
/// The assistant may return one or more tool calls in a single response.
/// Each must be executed and its result returned before continuing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique ID for this tool call (assigned by the LLM).
    pub id: String,
    /// Name of the tool to invoke.
    pub name: String,
    /// JSON arguments to pass to the tool.
    pub arguments: serde_json::Value,
}

// ---------------------------------------------------------------------------
// LLMResponse
// ---------------------------------------------------------------------------

/// Response from the LLM after a completion call.
///
/// Contains the generated text, tool calls (if any), token usage statistics,
/// and an estimated cost for the call.
#[derive(Debug, Clone)]
pub struct LLMResponse {
    /// The text content of the response.
    pub content: String,
    /// Why the LLM stopped generating (e.g., "end_turn", "tool_use", "max_tokens").
    pub finish_reason: String,
    /// Token usage statistics for this call.
    pub usage: AgentTokenUsage,
    /// Tool calls requested by the LLM (present when finish_reason is "tool_use").
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Estimated cost in USD for this API call.
    pub cost: Option<f64>,
}

// ---------------------------------------------------------------------------
// AgentTokenUsage
// ---------------------------------------------------------------------------

/// Token usage statistics from an LLM call.
///
/// Includes prompt caching metrics from Anthropic's prompt caching beta.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentTokenUsage {
    /// Number of input tokens (regular, non-cached).
    pub input_tokens: u32,
    /// Number of output tokens generated.
    pub output_tokens: u32,
    /// Tokens written to the prompt cache (charged at 1.25x input price).
    pub cache_creation_input_tokens: u32,
    /// Tokens read from the prompt cache (charged at 0.1x input price).
    pub cache_read_input_tokens: u32,
}

impl AgentTokenUsage {
    /// Total tokens consumed (input + output).
    pub fn total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }
}

// ---------------------------------------------------------------------------
// Tool
// ---------------------------------------------------------------------------

/// A tool definition for the LLM.
///
/// Describes a function the LLM can call, including its name, description,
/// and JSON Schema for the parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    /// Unique name of the tool (e.g., "search_catalog").
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: String,
    /// JSON Schema describing the tool's input parameters.
    pub parameters: serde_json::Value,
}

// ---------------------------------------------------------------------------
// ToolAnnotations
// ---------------------------------------------------------------------------

/// Annotations for MCP tool compatibility.
///
/// These hints help MCP clients understand tool behavior without executing them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolAnnotations {
    /// Human-readable title for the tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Hint that the tool only reads data and has no side effects.
    #[serde(rename = "readOnlyHint", skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    /// Hint that the tool may modify or delete data.
    #[serde(rename = "destructiveHint", skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    /// Hint that calling the tool repeatedly with the same input yields the same result.
    #[serde(rename = "idempotentHint", skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    /// Hint that the tool interacts with external systems.
    #[serde(rename = "openWorldHint", skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_role_serializes_to_lowercase() {
        assert_eq!(
            serde_json::to_string(&MessageRole::System).unwrap(),
            "\"system\""
        );
        assert_eq!(
            serde_json::to_string(&MessageRole::User).unwrap(),
            "\"user\""
        );
        assert_eq!(
            serde_json::to_string(&MessageRole::Assistant).unwrap(),
            "\"assistant\""
        );
        assert_eq!(
            serde_json::to_string(&MessageRole::Tool).unwrap(),
            "\"tool\""
        );
    }

    #[test]
    fn message_role_deserializes_from_lowercase() {
        let role: MessageRole = serde_json::from_str("\"system\"").unwrap();
        assert_eq!(role, MessageRole::System);

        let role: MessageRole = serde_json::from_str("\"tool\"").unwrap();
        assert_eq!(role, MessageRole::Tool);
    }

    #[test]
    fn message_system_constructor() {
        let msg = Message::system("You are a helpful assistant.");
        assert_eq!(msg.role, MessageRole::System);
        assert_eq!(msg.content, "You are a helpful assistant.");
        assert!(msg.tool_calls.is_none());
        assert!(msg.tool_call_id.is_none());
        assert!(msg.name.is_none());
        assert!(msg.user_id.is_none());
    }

    #[test]
    fn message_user_constructor() {
        let msg = Message::user("Show me revenue");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "Show me revenue");
    }

    #[test]
    fn message_user_with_id_constructor() {
        let msg = Message::user_with_id("Hello", "user-abc-123");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "Hello");
        assert_eq!(msg.user_id.as_deref(), Some("user-abc-123"));
    }

    #[test]
    fn message_assistant_constructor() {
        let msg = Message::assistant("Here are the results.");
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content, "Here are the results.");
        assert!(msg.tool_calls.is_none());
    }

    #[test]
    fn message_assistant_with_tool_calls_constructor() {
        let tool_calls = vec![ToolCall {
            id: "tc_001".into(),
            name: "search_catalog".into(),
            arguments: serde_json::json!({"query": "revenue"}),
        }];
        let msg = Message::assistant_with_tool_calls("Let me search.", tool_calls);
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content, "Let me search.");
        let tc = msg.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].name, "search_catalog");
    }

    #[test]
    fn message_tool_result_constructor() {
        let msg = Message::tool_result("tc_001", "search_catalog", r#"{"tables": []}"#);
        assert_eq!(msg.role, MessageRole::Tool);
        assert_eq!(msg.tool_call_id.as_deref(), Some("tc_001"));
        assert_eq!(msg.name.as_deref(), Some("search_catalog"));
        assert_eq!(msg.content, r#"{"tables": []}"#);
    }

    #[test]
    fn message_serialization_skips_none_fields() {
        let msg = Message::user("Hello");
        let json = serde_json::to_value(&msg).unwrap();

        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "Hello");
        // None fields should be absent
        assert!(json.get("tool_calls").is_none());
        assert!(json.get("tool_call_id").is_none());
        assert!(json.get("name").is_none());
        assert!(json.get("user_id").is_none());
        assert!(json.get("timestamp").is_none());
        assert!(json.get("message_id").is_none());
    }

    #[test]
    fn message_deserialization_with_optional_fields() {
        let json = serde_json::json!({
            "role": "assistant",
            "content": "I found results.",
            "message_id": "msg-123"
        });
        let msg: Message = serde_json::from_value(json).unwrap();
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content, "I found results.");
        assert_eq!(msg.message_id.as_deref(), Some("msg-123"));
        assert!(msg.tool_calls.is_none());
    }

    #[test]
    fn tool_call_serialization() {
        let tc = ToolCall {
            id: "toolu_abc123".into(),
            name: "query_datasource".into(),
            arguments: serde_json::json!({
                "sql_query": "SELECT 1",
                "datasource": "production-postgres"
            }),
        };
        let json = serde_json::to_value(&tc).unwrap();
        assert_eq!(json["id"], "toolu_abc123");
        assert_eq!(json["name"], "query_datasource");
        assert_eq!(json["arguments"]["sql_query"], "SELECT 1");
    }

    #[test]
    fn token_usage_default() {
        let usage = AgentTokenUsage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_creation_input_tokens, 0);
        assert_eq!(usage.cache_read_input_tokens, 0);
        assert_eq!(usage.total_tokens(), 0);
    }

    #[test]
    fn token_usage_total_tokens() {
        let usage = AgentTokenUsage {
            input_tokens: 1500,
            output_tokens: 500,
            cache_creation_input_tokens: 100,
            cache_read_input_tokens: 200,
        };
        assert_eq!(usage.total_tokens(), 2000);
    }

    #[test]
    fn tool_definition_serialization() {
        let tool = Tool {
            name: "search_catalog".into(),
            description: "Search for tables.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
        };
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["name"], "search_catalog");
        assert_eq!(json["parameters"]["type"], "object");
    }

    #[test]
    fn tool_annotations_default_is_empty() {
        let ann = ToolAnnotations::default();
        assert!(ann.title.is_none());
        assert!(ann.read_only_hint.is_none());
        assert!(ann.destructive_hint.is_none());
        assert!(ann.idempotent_hint.is_none());
        assert!(ann.open_world_hint.is_none());
    }

    #[test]
    fn tool_annotations_serialization_skips_none() {
        let ann = ToolAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        };
        let json = serde_json::to_value(&ann).unwrap();
        assert_eq!(json["readOnlyHint"], true);
        assert!(json.get("title").is_none());
        assert!(json.get("destructiveHint").is_none());
    }

    #[test]
    fn tool_annotations_rename_fields() {
        let ann = ToolAnnotations {
            title: Some("Test Tool".into()),
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            idempotent_hint: Some(true),
            open_world_hint: Some(false),
        };
        let json = serde_json::to_value(&ann).unwrap();
        assert_eq!(json["readOnlyHint"], true);
        assert_eq!(json["destructiveHint"], false);
        assert_eq!(json["idempotentHint"], true);
        assert_eq!(json["openWorldHint"], false);
    }

    // -- Contract: Message round-trip serialization --------------------------

    #[test]
    fn message_user_roundtrip_serialize_deserialize() {
        let msg = Message::user("What is the revenue?");
        let json = serde_json::to_string(&msg).unwrap();
        let restored: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.role, MessageRole::User);
        assert_eq!(restored.content, "What is the revenue?");
        assert!(restored.tool_calls.is_none());
        assert!(restored.user_id.is_none());
    }

    #[test]
    fn message_assistant_with_tool_calls_roundtrip() {
        let tool_calls = vec![
            ToolCall {
                id: "toolu_abc".into(),
                name: "search_catalog".into(),
                arguments: serde_json::json!({"query": "revenue"}),
            },
            ToolCall {
                id: "toolu_def".into(),
                name: "query_datasource".into(),
                arguments: serde_json::json!({"sql_query": "SELECT 1", "datasource": "pg"}),
            },
        ];
        let msg = Message::assistant_with_tool_calls("Let me investigate.", tool_calls);

        let json = serde_json::to_string(&msg).unwrap();
        let restored: Message = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.role, MessageRole::Assistant);
        assert_eq!(restored.content, "Let me investigate.");
        let tc = restored.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 2);
        assert_eq!(tc[0].id, "toolu_abc");
        assert_eq!(tc[0].name, "search_catalog");
        assert_eq!(tc[0].arguments["query"], "revenue");
        assert_eq!(tc[1].id, "toolu_def");
        assert_eq!(tc[1].name, "query_datasource");
        assert_eq!(tc[1].arguments["datasource"], "pg");
    }

    #[test]
    fn message_tool_result_roundtrip() {
        let msg = Message::tool_result("tc_001", "search_catalog", r#"{"found": true}"#);
        let json = serde_json::to_string(&msg).unwrap();
        let restored: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.role, MessageRole::Tool);
        assert_eq!(restored.tool_call_id.as_deref(), Some("tc_001"));
        assert_eq!(restored.name.as_deref(), Some("search_catalog"));
        assert_eq!(restored.content, r#"{"found": true}"#);
    }

    #[test]
    fn message_system_roundtrip() {
        let msg = Message::system("You are a data analyst.");
        let json = serde_json::to_string(&msg).unwrap();
        let restored: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.role, MessageRole::System);
        assert_eq!(restored.content, "You are a data analyst.");
    }

    // -- Contract: ToolCall deserialization from various JSON formats ---------

    #[test]
    fn tool_call_deserialization_standard_format() {
        let json = serde_json::json!({
            "id": "toolu_abc123",
            "name": "query_datasource",
            "arguments": {"sql_query": "SELECT * FROM users", "datasource": "prod-pg"}
        });
        let tc: ToolCall = serde_json::from_value(json).unwrap();
        assert_eq!(tc.id, "toolu_abc123");
        assert_eq!(tc.name, "query_datasource");
        assert_eq!(tc.arguments["sql_query"], "SELECT * FROM users");
        assert_eq!(tc.arguments["datasource"], "prod-pg");
    }

    #[test]
    fn tool_call_deserialization_empty_arguments() {
        let json = serde_json::json!({
            "id": "toolu_xyz",
            "name": "list_datasources",
            "arguments": {}
        });
        let tc: ToolCall = serde_json::from_value(json).unwrap();
        assert_eq!(tc.name, "list_datasources");
        assert!(tc.arguments.as_object().unwrap().is_empty());
    }

    #[test]
    fn tool_call_deserialization_null_arguments() {
        let json = serde_json::json!({
            "id": "toolu_001",
            "name": "some_tool",
            "arguments": null
        });
        let tc: ToolCall = serde_json::from_value(json).unwrap();
        assert!(tc.arguments.is_null());
    }

    #[test]
    fn tool_call_deserialization_nested_arguments() {
        let json = serde_json::json!({
            "id": "toolu_nested",
            "name": "complex_tool",
            "arguments": {
                "filters": {"region": "US", "year": 2025},
                "limit": 10,
                "include_totals": true
            }
        });
        let tc: ToolCall = serde_json::from_value(json).unwrap();
        assert_eq!(tc.arguments["filters"]["region"], "US");
        assert_eq!(tc.arguments["limit"], 10);
        assert_eq!(tc.arguments["include_totals"], true);
    }

    #[test]
    fn tool_call_array_deserialization() {
        // This is what comes back from the DB as stored tool_calls JSON.
        let json = serde_json::json!([
            {"id": "tc_1", "name": "search_catalog", "arguments": {"query": "rev"}},
            {"id": "tc_2", "name": "get_table_info", "arguments": {"table_name": "orders"}}
        ]);
        let tool_calls: Vec<ToolCall> = serde_json::from_value(json).unwrap();
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].id, "tc_1");
        assert_eq!(tool_calls[0].name, "search_catalog");
        assert_eq!(tool_calls[1].id, "tc_2");
        assert_eq!(tool_calls[1].name, "get_table_info");
    }

    // -- Contract: MessageRole rejects invalid values ------------------------

    #[test]
    fn message_role_rejects_uppercase() {
        let result: Result<MessageRole, _> = serde_json::from_str("\"System\"");
        assert!(result.is_err());
    }

    #[test]
    fn message_role_rejects_unknown_role() {
        let result: Result<MessageRole, _> = serde_json::from_str("\"function\"");
        assert!(result.is_err());
    }

    // -- Contract: AgentTokenUsage total_tokens only counts input + output --------

    #[test]
    fn token_usage_total_tokens_excludes_cache() {
        // total_tokens is defined as input + output (not including cache tokens).
        // This is an explicit contract: cache tokens are tracked separately.
        let usage = AgentTokenUsage {
            input_tokens: 500,
            output_tokens: 200,
            cache_creation_input_tokens: 1000,
            cache_read_input_tokens: 3000,
        };
        // total is 500 + 200 = 700 (cache tokens NOT included).
        assert_eq!(usage.total_tokens(), 700);
    }

    // -- Contract: Message with all optional fields set ----------------------

    #[test]
    fn message_full_deserialization_all_fields() {
        let json = serde_json::json!({
            "role": "user",
            "content": "Show revenue",
            "user_id": "user-abc-12345678",
            "timestamp": "2025-01-15T10:30:00Z",
            "message_id": "msg-uuid-123"
        });
        let msg: Message = serde_json::from_value(json).unwrap();
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "Show revenue");
        assert_eq!(msg.user_id.as_deref(), Some("user-abc-12345678"));
        assert_eq!(msg.timestamp.as_deref(), Some("2025-01-15T10:30:00Z"));
        assert_eq!(msg.message_id.as_deref(), Some("msg-uuid-123"));
    }

    // -- Contract: Tool definition JSON Schema structure ----------------------

    #[test]
    fn tool_definition_parameters_has_json_schema_structure() {
        let tool = Tool {
            name: "query_datasource".into(),
            description: "Execute SQL query.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "sql_query": {
                        "type": "string",
                        "description": "The SQL query to execute"
                    },
                    "datasource": {
                        "type": "string",
                        "description": "Datasource slug"
                    }
                },
                "required": ["sql_query", "datasource"]
            }),
        };
        let json = serde_json::to_value(&tool).unwrap();
        // JSON Schema structure contract.
        assert_eq!(json["parameters"]["type"], "object");
        assert!(json["parameters"]["properties"].is_object());
        assert!(json["parameters"]["required"].is_array());
        let required = json["parameters"]["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("sql_query")));
        assert!(required.contains(&serde_json::json!("datasource")));
    }

    // -- Contract: ToolAnnotations roundtrip ---------------------------------

    #[test]
    fn tool_annotations_full_roundtrip() {
        let ann = ToolAnnotations {
            title: Some("Search Catalog".into()),
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            idempotent_hint: Some(true),
            open_world_hint: Some(true),
        };
        let json = serde_json::to_string(&ann).unwrap();
        let restored: ToolAnnotations = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.title.as_deref(), Some("Search Catalog"));
        assert_eq!(restored.read_only_hint, Some(true));
        assert_eq!(restored.destructive_hint, Some(false));
        assert_eq!(restored.idempotent_hint, Some(true));
        assert_eq!(restored.open_world_hint, Some(true));
    }

    // -- Contract: AgentTokenUsage serialization roundtrip ------------------------

    #[test]
    fn token_usage_serialization_roundtrip() {
        let usage = AgentTokenUsage {
            input_tokens: 1500,
            output_tokens: 300,
            cache_creation_input_tokens: 5000,
            cache_read_input_tokens: 12000,
        };
        let json = serde_json::to_string(&usage).unwrap();
        let restored: AgentTokenUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.input_tokens, 1500);
        assert_eq!(restored.output_tokens, 300);
        assert_eq!(restored.cache_creation_input_tokens, 5000);
        assert_eq!(restored.cache_read_input_tokens, 12000);
    }

    // -- Contract: Message::assistant_with_tool_calls empty content ----------

    #[test]
    fn message_assistant_with_tool_calls_empty_content_preserved() {
        let tool_calls = vec![ToolCall {
            id: "tc_1".into(),
            name: "search_catalog".into(),
            arguments: serde_json::json!({"query": "revenue"}),
        }];
        let msg = Message::assistant_with_tool_calls("", tool_calls);
        assert_eq!(msg.content, "");
        assert!(msg.tool_calls.is_some());
        assert_eq!(msg.tool_calls.as_ref().unwrap().len(), 1);
    }

    // -- Contract: Message constructors never set unrelated fields -----------

    #[test]
    fn message_user_with_id_does_not_set_tool_fields() {
        let msg = Message::user_with_id("hello", "uid-123");
        assert!(msg.tool_calls.is_none());
        assert!(msg.tool_call_id.is_none());
        assert!(msg.name.is_none());
        assert!(msg.timestamp.is_none());
        assert!(msg.message_id.is_none());
    }

    #[test]
    fn message_tool_result_does_not_set_user_id() {
        let msg = Message::tool_result("tc_1", "search_catalog", "result");
        assert!(msg.user_id.is_none());
        assert!(msg.timestamp.is_none());
        assert!(msg.message_id.is_none());
        assert!(msg.tool_calls.is_none());
    }
}
