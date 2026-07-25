// SPDX-License-Identifier: AGPL-3.0-or-later

//! Thinking tracker for streaming agent progress to the frontend.
//!
//! [`AgentThinkingTracker`] records every event that occurs during an agent
//! loop iteration (tool start/end, thoughts, token usage) and publishes
//! them to Redis channels so the WebSocket layer can forward them to the
//! browser in real time.
//!
//! The tracker owns all state for a single user message — timestamps,
//! active tool tracking, cumulative token usage — and produces a final
//! event list suitable for database persistence via
//! [`AgentThinkingTracker::get_events_for_storage`].

use std::collections::HashMap;
use std::sync::OnceLock;

use kyomi_auth::websocket::WebSocketManager;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::debug;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maps tool names to human-readable labels for UI display.
pub const TOOL_FRIENDLY_NAMES: &[(&str, &str)] = &[
    // Data & queries
    ("browse_catalog", "Browsing catalog"),
    ("search_catalog", "Searching for tables"),
    ("list_datasources", "Listing datasources"),
    ("get_table_info", "Getting table information"),
    ("sample_table", "Sampling table data"),
    ("query_datasource", "Querying your data"),
    ("validate_sql", "Validating SQL"),
    ("estimate_query_cost", "Estimating query costs"),
    ("forecast_data", "Generating forecast"),
    ("get_workspace_info", "Getting workspace info"),
    // Dashboards
    ("search_dashboards", "Searching dashboards"),
    ("get_dashboard_info", "Getting dashboard details"),
    ("create_dashboard", "Creating dashboard"),
    ("modify_dashboard", "Updating dashboard"),
    ("delete_dashboard", "Deleting dashboard"),
    ("update_dashboard", "Updating dashboard"),
    // Charts
    ("get_chartml_spec", "Looking up ChartML spec"),
    ("validate_chartml", "Validating ChartML"),
    ("update_chart", "Updating chart"),
    ("render_chart", "Rendering chart"),
    // Knowledge
    ("search_knowledge", "Searching knowledge base"),
    ("write_knowledge_file", "Writing knowledge file"),
    ("read_knowledge_file", "Reading knowledge file"),
    ("list_knowledge_files", "Browsing knowledge files"),
    ("edit_knowledge_file", "Editing knowledge file"),
    // Watches
    ("create_watch", "Creating watch"),
    ("preview_watch", "Previewing watch"),
    ("update_watch", "Updating watch"),
    ("update_watch_draft", "Drafting watch update"),
    ("search_watches", "Searching watches"),
    ("delete_watch", "Deleting watch"),
    ("get_watch_info", "Getting watch details"),
    ("trigger_watch", "Triggering watch"),
    // Documentation
    ("browse_resources", "Browsing documentation"),
    ("read_resource", "Reading documentation"),
    // Analytics
    ("create_analytics_site", "Creating analytics site"),
    ("list_analytics_sites", "Listing analytics sites"),
    ("update_analytics_site", "Updating analytics site"),
    ("delete_analytics_site", "Deleting analytics site"),
];

/// Look up the friendly display name for a tool.
///
/// Returns a static fallback for unknown tools because we cannot return a
/// dynamically-formatted `&str` from this function.
fn get_friendly_name(tool_name: &str) -> &'static str {
    TOOL_FRIENDLY_NAMES
        .iter()
        .find(|(name, _)| *name == tool_name)
        .map(|(_, friendly)| *friendly)
        .unwrap_or("Using tool")
}

// ---------------------------------------------------------------------------
// ThinkingEventType
// ---------------------------------------------------------------------------

/// Discriminant for the kind of thinking event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingEventType {
    AgentStart,
    AgentThought,
    ToolExecutionStart,
    ToolExecutionEnd,
    AgentDecision,
    AgentComplete,
    Error,
}

// ---------------------------------------------------------------------------
// AgentThinkingEvent
// ---------------------------------------------------------------------------

/// A single event in the agent thinking timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentThinkingEvent {
    pub event_type: ThinkingEventType,
    /// ISO 8601 UTC timestamp.
    pub timestamp: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// True when the description was truncated and the full text is available
    /// on demand from the `thinking_event_details` table.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_full_text: bool,
}

// ---------------------------------------------------------------------------
// AgentThinkingTracker
// ---------------------------------------------------------------------------

/// Tracks and broadcasts agent thinking events in real time.
///
/// One tracker is created per user message. It accumulates events, publishes
/// them to Redis for WebSocket delivery, and provides the full event list
/// for database persistence after the agent loop completes.
pub struct AgentThinkingTracker {
    session_id: String,
    message_id: String,
    workspace_user_ids: Vec<String>,
    context_type: Option<String>,
    events: Vec<AgentThinkingEvent>,
    start_time: tokio::time::Instant,
    /// Maps tool_name -> index into `events` for the start event.
    active_tools: HashMap<String, usize>,
    /// Maps tool_name -> wall-clock start instant.
    tool_start_times: HashMap<String, tokio::time::Instant>,
    event_counter: u32,
    total_input_tokens: u32,
    total_output_tokens: u32,
    total_cost: f64,
    /// Token count from the most recent LLM request (not cumulative).
    /// Used to display context window utilisation in the UI.
    last_input_tokens: u32,
    /// Context window size for the model in use (0 = unknown).
    context_window: u32,
    /// WebSocket manager for delivering thinking events.
    /// Handles both standalone (direct local) and multi-replica (Redis pub/sub)
    /// delivery automatically.
    ws_manager: WebSocketManager,
    /// Full (untruncated) reasoning text for events that exceeded the 200-char
    /// display limit. Keyed by event_id; persisted to `thinking_event_details`
    /// after the agent loop completes.
    full_texts: HashMap<String, String>,
}

impl AgentThinkingTracker {
    /// Create a new tracker for a single message exchange.
    ///
    /// If `workspace_user_ids` is `None`, the tracker broadcasts only to the
    /// requesting user.
    pub fn new(
        session_id: String,
        user_id: String,
        message_id: String,
        ws_manager: WebSocketManager,
        workspace_user_ids: Option<Vec<String>>,
        context_type: Option<String>,
        context_window: u32,
    ) -> Self {
        let ws_users = workspace_user_ids.unwrap_or_else(|| vec![user_id]);
        Self {
            session_id,
            message_id,
            workspace_user_ids: ws_users,
            context_type,
            events: Vec::new(),
            start_time: tokio::time::Instant::now(),
            active_tools: HashMap::new(),
            tool_start_times: HashMap::new(),
            event_counter: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            last_input_tokens: 0,
            context_window,
            ws_manager,
            full_texts: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Event ID generation
    // -----------------------------------------------------------------------

    fn generate_event_id(&mut self) -> String {
        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        self.event_counter += 1;
        format!("{}-{:03}", timestamp_ms, self.event_counter)
    }

    // -----------------------------------------------------------------------
    // Redis publishing
    // -----------------------------------------------------------------------

    /// Serialize and deliver a thinking event via the WebSocket manager.
    ///
    /// The WebSocket manager handles both standalone (direct local delivery)
    /// and multi-replica (Redis pub/sub) modes automatically.
    async fn send_event(&self, event: &AgentThinkingEvent, is_update: bool) {
        let mut event_obj = serde_json::json!({
            "event_id": event.event_id,
            "event_type": event.event_type,
            "timestamp": event.timestamp,
            "title": event.title,
            "description": event.description,
            "data": event.data,
            "duration_ms": event.duration_ms,
            "is_update": is_update,
            "context_type": self.context_type,
        });
        if event.has_full_text {
            event_obj["has_full_text"] = serde_json::Value::Bool(true);
        }
        let thinking_data = serde_json::json!({ "event": event_obj });

        for uid in &self.workspace_user_ids {
            kyomi_auth::websocket::helpers::send_agent_thinking(
                &self.ws_manager,
                uid,
                &self.session_id,
                thinking_data.clone(),
                Some(&self.message_id),
            )
            .await;
        }
    }

    // -----------------------------------------------------------------------
    // Event recording helpers
    // -----------------------------------------------------------------------

    /// Store an event, assigning an ID if missing.
    ///
    /// Returns a clone of the event (with ID assigned) so callers can
    /// immediately pass it to [`send_event`] without a second lookup.
    fn add_event(&mut self, mut event: AgentThinkingEvent) -> AgentThinkingEvent {
        if event.event_id.is_none() {
            event.event_id = Some(self.generate_event_id());
        }
        let result = event.clone();
        self.events.push(event);
        result
    }

    // -----------------------------------------------------------------------
    // Public event methods
    // -----------------------------------------------------------------------

    /// Record the start of the agent loop.
    pub async fn agent_started(&mut self, title: &str, description: &str) {
        let event = AgentThinkingEvent {
            event_type: ThinkingEventType::AgentStart,
            timestamp: chrono::Utc::now().to_rfc3339(),
            title: title.to_string(),
            event_id: None,
            description: Some(description.to_string()),
            data: None,
            duration_ms: None,
            has_full_text: false,
        };
        let event = self.add_event(event);
        self.send_event(&event, false).await;
    }

    /// Record a thinking/reasoning fragment from the LLM.
    ///
    /// The thought text is cleaned (memory blocks removed, prefixes stripped,
    /// generic patterns skipped) before recording. If cleaning produces
    /// nothing useful, no event is emitted. When the full text exceeds the
    /// 200-char display limit, it is stashed for later persistence to the
    /// `thinking_event_details` table.
    pub async fn agent_thought(&mut self, thought: &str) {
        let Some(cleaned) = clean_thought(thought) else {
            return;
        };
        let has_full_text = cleaned.full_text.is_some();
        let event = AgentThinkingEvent {
            event_type: ThinkingEventType::AgentThought,
            timestamp: chrono::Utc::now().to_rfc3339(),
            title: "Planning".to_string(),
            event_id: None,
            description: Some(cleaned.display),
            data: None,
            duration_ms: None,
            has_full_text,
        };
        let event = self.add_event(event);
        if let Some(full_text) = cleaned.full_text
            && let Some(ref eid) = event.event_id
        {
            self.full_texts.insert(eid.clone(), full_text);
        }
        self.send_event(&event, false).await;
    }

    /// Record the start of a tool execution.
    ///
    /// Duplicate starts for the same tool (while already active) are ignored.
    pub async fn tool_execution_started(
        &mut self,
        tool_name: &str,
        tool_input: &serde_json::Value,
    ) {
        if self.active_tools.contains_key(tool_name) {
            debug!(tool = %tool_name, "tool already active, skipping duplicate start event");
            return;
        }

        self.tool_start_times
            .insert(tool_name.to_string(), tokio::time::Instant::now());

        let friendly_name = get_friendly_name(tool_name);
        let schema = format_tool_schema(tool_name, tool_input, true);

        let event = AgentThinkingEvent {
            event_type: ThinkingEventType::ToolExecutionStart,
            timestamp: chrono::Utc::now().to_rfc3339(),
            title: format!("\u{23f3} {friendly_name}"), // hourglass emoji
            event_id: None,
            description: Some("Working on it...".to_string()),
            data: Some(serde_json::json!({
                "tool_name": tool_name,
                "schema": schema,
                "status": "processing",
            })),
            duration_ms: None,
            has_full_text: false,
        };

        let event = self.add_event(event);
        let event_index = self.events.len() - 1;
        self.active_tools.insert(tool_name.to_string(), event_index);

        self.send_event(&event, false).await;
    }

    /// Record the completion of a tool execution.
    ///
    /// Updates the original start event in place with duration and result
    /// information, then re-publishes it as an update.
    pub async fn tool_execution_completed(
        &mut self,
        tool_name: &str,
        tool_output: &str,
        success: bool,
    ) {
        let Some(&event_index) = self.active_tools.get(tool_name) else {
            debug!(tool = %tool_name, "received completion for tool with no active event");
            return;
        };

        let duration_ms = self
            .tool_start_times
            .get(tool_name)
            .map(|start| start.elapsed().as_millis() as u64);

        let friendly_name = get_friendly_name(tool_name);

        // Parse output as JSON for schema formatting.
        let output_value: serde_json::Value = serde_json::from_str(tool_output)
            .unwrap_or_else(|_| serde_json::Value::String(tool_output.to_string()));
        let output_schema = format_tool_schema(tool_name, &output_value, false);

        // Merge output schema into the existing input schema.
        let existing_event = &self.events[event_index];
        let mut merged_schema = existing_event
            .data
            .as_ref()
            .and_then(|d| d.get("schema"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        if let (Some(merged_obj), Some(output_obj)) =
            (merged_schema.as_object_mut(), output_schema.as_object())
        {
            for (k, v) in output_obj {
                merged_obj.insert(k.clone(), v.clone());
            }
        }

        let (title, description) = if success {
            let duration_text = duration_ms.map(|d| format!("{d}ms")).unwrap_or_default();
            (
                format!("\u{2705} {friendly_name}"), // checkmark emoji
                format!("Done! ({duration_text})"),
            )
        } else {
            let duration_text = duration_ms
                .map(|d| format!(" ({d}ms)"))
                .unwrap_or_default();
            (
                format!("\u{274c} {friendly_name}"), // cross-mark emoji
                format!("Failed{duration_text}"),
            )
        };

        let updated_event = AgentThinkingEvent {
            event_type: existing_event.event_type.clone(),
            timestamp: existing_event.timestamp.clone(),
            title,
            event_id: existing_event.event_id.clone(),
            description: Some(description),
            data: Some(serde_json::json!({
                "tool_name": tool_name,
                "schema": merged_schema,
                "success": success,
                "status": "completed",
            })),
            duration_ms,
            has_full_text: false,
        };

        self.events[event_index] = updated_event.clone();
        self.send_event(&updated_event, true).await;

        // Clean up active tracking.
        self.active_tools.remove(tool_name);
        self.tool_start_times.remove(tool_name);
    }

    /// Record that the agent is preparing its final response.
    pub async fn preparing_response(&mut self) {
        let event = AgentThinkingEvent {
            event_type: ThinkingEventType::AgentThought,
            timestamp: chrono::Utc::now().to_rfc3339(),
            title: "Preparing your response".to_string(),
            event_id: None,
            description: None,
            data: None,
            duration_ms: None,
            has_full_text: false,
        };
        let event = self.add_event(event);
        self.send_event(&event, false).await;
    }

    /// Record that the agent is compacting conversation context.
    pub async fn compacting_context(&mut self) {
        let event = AgentThinkingEvent {
            event_type: ThinkingEventType::AgentThought,
            timestamp: chrono::Utc::now().to_rfc3339(),
            title: "Compacting conversation".to_string(),
            event_id: None,
            description: None,
            data: None,
            duration_ms: None,
            has_full_text: false,
        };
        let event = self.add_event(event);
        self.send_event(&event, false).await;
    }

    /// Record that the agent has completed its work.
    pub async fn agent_completed(&mut self, result: &str) {
        let total_duration = self.start_time.elapsed().as_millis() as u64;
        let event = AgentThinkingEvent {
            event_type: ThinkingEventType::AgentComplete,
            timestamp: chrono::Utc::now().to_rfc3339(),
            title: "Analysis complete".to_string(),
            event_id: None,
            description: Some(format!("Analysis completed in {total_duration}ms")),
            data: Some(serde_json::json!({"result": result})),
            duration_ms: Some(total_duration),
            has_full_text: false,
        };
        let event = self.add_event(event);
        self.send_event(&event, false).await;
    }

    /// Update cumulative token usage and broadcast a usage event.
    pub async fn update_token_usage(
        &mut self,
        input_tokens: u32,
        output_tokens: u32,
        cost: Option<f64>,
    ) {
        self.total_input_tokens += input_tokens;
        self.total_output_tokens += output_tokens;
        self.last_input_tokens = input_tokens;
        if let Some(c) = cost {
            self.total_cost += c;
        }

        let token_data = serde_json::json!({
            "context_type": self.context_type,
            "token_usage": {
                "input_tokens": self.total_input_tokens,
                "output_tokens": self.total_output_tokens,
                "total_tokens": self.total_input_tokens + self.total_output_tokens,
                "cost": self.total_cost,
                "context_tokens": self.last_input_tokens,
                "context_window": self.context_window,
            }
        });

        for uid in &self.workspace_user_ids {
            kyomi_auth::websocket::helpers::send_token_usage_update(
                &self.ws_manager,
                uid,
                &self.session_id,
                token_data.clone(),
                Some(&self.message_id),
            )
            .await;
        }
    }

    // -----------------------------------------------------------------------
    // Storage / accessors
    // -----------------------------------------------------------------------

    /// Produce a serializable list of events for database persistence.
    pub fn get_events_for_storage(&self) -> Vec<serde_json::Value> {
        self.events
            .iter()
            .map(|event| {
                let mut obj = serde_json::json!({
                    "event_id": event.event_id,
                    "event_type": event.event_type,
                    "timestamp": event.timestamp,
                    "title": event.title,
                    "description": event.description,
                    "data": event.data,
                    "duration_ms": event.duration_ms,
                });
                if event.has_full_text {
                    obj["has_full_text"] = serde_json::Value::Bool(true);
                }
                obj
            })
            .collect()
    }

    /// Full reasoning texts that were truncated for display.
    ///
    /// Returns `(event_id, full_text)` pairs for persistence to the
    /// `thinking_event_details` table.
    pub fn full_texts_for_storage(&self) -> &HashMap<String, String> {
        &self.full_texts
    }

    /// Read-only access to the recorded events.
    pub fn events(&self) -> &[AgentThinkingEvent] {
        &self.events
    }

    /// Cumulative input tokens across all LLM calls.
    pub fn total_input_tokens(&self) -> u32 {
        self.total_input_tokens
    }

    /// Cumulative output tokens across all LLM calls.
    pub fn total_output_tokens(&self) -> u32 {
        self.total_output_tokens
    }

    /// Cumulative cost across all LLM calls.
    pub fn total_cost(&self) -> f64 {
        self.total_cost
    }

    /// Token count from the most recent LLM request.
    pub fn last_input_tokens(&self) -> u32 {
        self.last_input_tokens
    }

    /// Context window size for the model in use.
    pub fn context_window(&self) -> u32 {
        self.context_window
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Result of cleaning a thought — truncated display text plus optional full text.
struct CleanedThought {
    display: String,
    full_text: Option<String>,
}

/// Clean up LLM thinking text for user-facing display.
///
/// Removes `<memory>` blocks, strips common filler prefixes, skips
/// generic patterns (action/observation markers), and truncates to a
/// reasonable length. When the cleaned text exceeds 200 characters,
/// `full_text` contains the untruncated version for on-demand retrieval.
fn clean_thought(thought: &str) -> Option<CleanedThought> {
    // 1. Remove <memory>...</memory> blocks.
    static MEMORY_RE: OnceLock<Regex> = OnceLock::new();
    let re = MEMORY_RE.get_or_init(|| Regex::new(r"(?is)<memory>.*?</memory>").expect("valid regex literal"));
    let cleaned = re.replace_all(thought, "");
    let mut cleaned = cleaned.trim().to_string();

    // 2. Remove common filler prefixes (first match wins).
    let prefixes = [
        "Thought: ",
        "I'll first ",
        "I need to ",
        "Let me ",
        "I should ",
        "I will ",
        "Now I'll ",
        "Next, I'll ",
        "Response: ",
    ];
    for prefix in &prefixes {
        if cleaned.starts_with(prefix) {
            cleaned = cleaned[prefix.len()..].to_string();
            break;
        }
    }

    // 3. Skip generic patterns that add no value.
    let skip_patterns = [
        "Action:",
        "Observation:",
        "Final Answer:",
        "Let me think about this",
        "I understand you want",
    ];
    for pattern in &skip_patterns {
        if cleaned.contains(pattern) {
            return None;
        }
    }

    // 4. Reject overly short results.
    if cleaned.trim().len() <= 10 {
        return None;
    }

    // 5. Truncate to 200 characters for streaming display.
    let full_text = if cleaned.len() > 200 {
        let full = cleaned.clone();
        let boundary = cleaned
            .char_indices()
            .take_while(|(i, _)| *i < 200)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(200);
        // Safe: `boundary` is a char's own end offset (i + c.len_utf8()),
        // always a valid UTF-8 boundary. See KYO-211.
        cleaned = format!("{}...", &cleaned[..boundary]);
        Some(full)
    } else {
        None
    };

    Some(CleanedThought { display: cleaned, full_text })
}

/// Format tool input/output for structured frontend rendering.
///
/// Returns a JSON object with a `tool` name and either an `input` or
/// `output` key, extracting the most relevant fields for each known tool.
fn format_tool_schema(
    tool_name: &str,
    data: &serde_json::Value,
    is_input: bool,
) -> serde_json::Value {
    let key = if is_input { "input" } else { "output" };

    match tool_name {
        "query_datasource" | "bigquery_query" => {
            if is_input {
                let sql = data
                    .get("sql_query")
                    .or_else(|| data.get("sql"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let datasource = data.get("datasource").and_then(|v| v.as_str());
                let mut input = serde_json::json!({"sql": sql});
                if let Some(ds) = datasource {
                    input["datasource"] = serde_json::Value::String(ds.to_string());
                }
                serde_json::json!({"tool": tool_name, "input": input})
            } else {
                serde_json::json!({"tool": tool_name, "output": data})
            }
        }
        "search_knowledge" | "search_catalog" | "bigquery_search" => {
            if is_input {
                let query = data.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let datasource = data.get("datasource").and_then(|v| v.as_str());
                let mut input = serde_json::json!({"query": query});
                if let Some(ds) = datasource {
                    input["datasource"] = serde_json::Value::String(ds.to_string());
                }
                serde_json::json!({"tool": tool_name, "input": input})
            } else {
                serde_json::json!({"tool": tool_name, "output": data})
            }
        }
        "get_table_info" | "bigquery_table_info" => {
            if is_input {
                let table = data
                    .get("table_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let datasource = data.get("datasource").and_then(|v| v.as_str());
                let mut input = serde_json::json!({"table_name": table});
                if let Some(ds) = datasource {
                    input["datasource"] = serde_json::Value::String(ds.to_string());
                }
                serde_json::json!({"tool": tool_name, "input": input})
            } else {
                serde_json::json!({"tool": tool_name, "output": data})
            }
        }
        "validate_sql" => {
            if is_input {
                let sql = data.get("sql").and_then(|v| v.as_str()).unwrap_or("");
                let datasource = data
                    .get("datasource")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                serde_json::json!({
                    "tool": tool_name,
                    "input": {"sql": sql, "datasource": datasource}
                })
            } else {
                serde_json::json!({"tool": tool_name, "output": data})
            }
        }
        // Default: pass data through as-is.
        _ => {
            serde_json::json!({"tool": tool_name, key: data})
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- clean_thought tests ------------------------------------------------

    #[test]
    fn clean_thought_removes_memory_blocks() {
        let input = "Before <memory>secret stuff</memory> after the block";
        let result = clean_thought(input).unwrap().display;
        assert!(!result.contains("memory"));
        assert!(!result.contains("secret stuff"));
        assert!(result.contains("Before"));
        assert!(result.contains("after the block"));
    }

    #[test]
    fn clean_thought_removes_multiline_memory_blocks() {
        let input = "Start\n<memory>\nline1\nline2\n</memory>\nEnd of thought here";
        let result = clean_thought(input).unwrap().display;
        assert!(!result.contains("line1"));
        assert!(result.contains("Start"));
        assert!(result.contains("End of thought"));
    }

    #[test]
    fn clean_thought_strips_prefix_thought() {
        let result = clean_thought("Thought: analyze the revenue data for Q4").unwrap().display;
        assert_eq!(result, "analyze the revenue data for Q4");
    }

    #[test]
    fn clean_thought_strips_prefix_let_me() {
        let result = clean_thought("Let me check the sales table structure").unwrap().display;
        assert_eq!(result, "check the sales table structure");
    }

    #[test]
    fn clean_thought_strips_prefix_i_need_to() {
        let result = clean_thought("I need to query the database for metrics").unwrap().display;
        assert_eq!(result, "query the database for metrics");
    }

    #[test]
    fn clean_thought_strips_only_first_matching_prefix() {
        // "I will " prefix matches, but "I need to" inside remaining text is kept.
        let result = clean_thought("I will check if I need to do more").unwrap().display;
        assert_eq!(result, "check if I need to do more");
    }

    #[test]
    fn clean_thought_skips_action_pattern() {
        assert!(clean_thought("Action: query_datasource").is_none());
    }

    #[test]
    fn clean_thought_skips_observation_pattern() {
        assert!(clean_thought("The Observation: shows results from the query").is_none());
    }

    #[test]
    fn clean_thought_skips_final_answer_pattern() {
        assert!(clean_thought("Final Answer: The revenue was $1M last quarter").is_none());
    }

    #[test]
    fn clean_thought_skips_let_me_think() {
        // No strippable prefix, so the skip pattern "Let me think about this" matches.
        assert!(clean_thought("OK, Let me think about this for a moment longer").is_none());
    }

    #[test]
    fn clean_thought_skips_i_understand() {
        assert!(clean_thought("I understand you want to see revenue data").is_none());
    }

    #[test]
    fn clean_thought_truncates_long_text() {
        let long_text = "A".repeat(300);
        let cleaned = clean_thought(&long_text).unwrap();
        // 200 chars + "..."
        assert!(cleaned.display.ends_with("..."));
        assert!(cleaned.display.len() <= 204); // 200 + "..."
        assert!(cleaned.full_text.is_some());
        assert_eq!(cleaned.full_text.unwrap().len(), 300);
    }

    #[test]
    fn clean_thought_truncates_at_char_boundary() {
        // Mix in some multi-byte characters near the 200-char boundary.
        let mut text = "A".repeat(198);
        text.push_str("\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}"); // e-acute (2-byte UTF-8)
        let cleaned = clean_thought(&text).unwrap();
        assert!(cleaned.display.ends_with("..."));
        // The result should be valid UTF-8 (no panics).
        assert!(cleaned.display.len() > 100);
        assert!(cleaned.full_text.is_some());
    }

    #[test]
    fn clean_thought_returns_none_for_empty() {
        assert!(clean_thought("").is_none());
    }

    #[test]
    fn clean_thought_returns_none_for_short() {
        assert!(clean_thought("ok").is_none());
        assert!(clean_thought("yes sure").is_none());
    }

    #[test]
    fn clean_thought_returns_none_for_exactly_10_chars() {
        assert!(clean_thought("0123456789").is_none());
    }

    #[test]
    fn clean_thought_returns_some_for_11_chars() {
        assert!(clean_thought("01234567890").is_some());
    }

    // -- format_tool_schema tests -------------------------------------------

    #[test]
    fn format_tool_schema_query_datasource_input() {
        let input = serde_json::json!({
            "sql_query": "SELECT * FROM sales",
            "datasource": "prod-pg"
        });
        let result = format_tool_schema("query_datasource", &input, true);
        assert_eq!(result["tool"], "query_datasource");
        assert_eq!(result["input"]["sql"], "SELECT * FROM sales");
        assert_eq!(result["input"]["datasource"], "prod-pg");
    }

    #[test]
    fn format_tool_schema_query_datasource_input_sql_key() {
        // Falls back to "sql" key when "sql_query" is absent.
        let input = serde_json::json!({"sql": "SELECT 1"});
        let result = format_tool_schema("query_datasource", &input, true);
        assert_eq!(result["input"]["sql"], "SELECT 1");
    }

    #[test]
    fn format_tool_schema_query_datasource_output() {
        let output = serde_json::json!({"rows": [{"id": 1}]});
        let result = format_tool_schema("query_datasource", &output, false);
        assert_eq!(result["tool"], "query_datasource");
        assert_eq!(result["output"], output);
    }

    #[test]
    fn format_tool_schema_search_catalog_input() {
        let input = serde_json::json!({
            "query": "revenue tables",
            "datasource": "analytics-bq"
        });
        let result = format_tool_schema("search_catalog", &input, true);
        assert_eq!(result["tool"], "search_catalog");
        assert_eq!(result["input"]["query"], "revenue tables");
        assert_eq!(result["input"]["datasource"], "analytics-bq");
    }

    #[test]
    fn format_tool_schema_search_catalog_no_datasource() {
        let input = serde_json::json!({"query": "users"});
        let result = format_tool_schema("search_catalog", &input, true);
        assert_eq!(result["input"]["query"], "users");
        assert!(result["input"].get("datasource").is_none());
    }

    #[test]
    fn format_tool_schema_get_table_info_input() {
        let input = serde_json::json!({
            "table_name": "public.orders",
            "datasource": "prod-pg"
        });
        let result = format_tool_schema("get_table_info", &input, true);
        assert_eq!(result["tool"], "get_table_info");
        assert_eq!(result["input"]["table_name"], "public.orders");
        assert_eq!(result["input"]["datasource"], "prod-pg");
    }

    #[test]
    fn format_tool_schema_validate_sql_input() {
        let input = serde_json::json!({
            "sql": "SELECT * FROM users",
            "datasource": "prod-pg"
        });
        let result = format_tool_schema("validate_sql", &input, true);
        assert_eq!(result["tool"], "validate_sql");
        assert_eq!(result["input"]["sql"], "SELECT * FROM users");
        assert_eq!(result["input"]["datasource"], "prod-pg");
    }

    #[test]
    fn format_tool_schema_validate_sql_output() {
        let output = serde_json::json!({"valid": true});
        let result = format_tool_schema("validate_sql", &output, false);
        assert_eq!(result["tool"], "validate_sql");
        assert_eq!(result["output"]["valid"], true);
    }

    #[test]
    fn format_tool_schema_unknown_tool_input() {
        let input = serde_json::json!({"key": "value"});
        let result = format_tool_schema("unknown_tool", &input, true);
        assert_eq!(result["tool"], "unknown_tool");
        assert_eq!(result["input"]["key"], "value");
    }

    #[test]
    fn format_tool_schema_unknown_tool_output() {
        let output = serde_json::json!({"result": 42});
        let result = format_tool_schema("unknown_tool", &output, false);
        assert_eq!(result["tool"], "unknown_tool");
        assert_eq!(result["output"]["result"], 42);
    }

    #[test]
    fn format_tool_schema_bigquery_query_input() {
        let input = serde_json::json!({"sql_query": "SELECT 1", "datasource": "bq"});
        let result = format_tool_schema("bigquery_query", &input, true);
        assert_eq!(result["tool"], "bigquery_query");
        assert_eq!(result["input"]["sql"], "SELECT 1");
    }

    #[test]
    fn format_tool_schema_bigquery_search_input() {
        let input = serde_json::json!({"query": "sales"});
        let result = format_tool_schema("bigquery_search", &input, true);
        assert_eq!(result["tool"], "bigquery_search");
        assert_eq!(result["input"]["query"], "sales");
    }

    #[test]
    fn format_tool_schema_bigquery_table_info_input() {
        let input = serde_json::json!({"table_name": "dataset.table"});
        let result = format_tool_schema("bigquery_table_info", &input, true);
        assert_eq!(result["tool"], "bigquery_table_info");
        assert_eq!(result["input"]["table_name"], "dataset.table");
    }

    // -- format_tool_schema edge cases -----------------------------------

    #[test]
    fn format_tool_schema_query_datasource_missing_sql_keys() {
        // Neither sql_query nor sql present — should return empty string.
        let input = serde_json::json!({"datasource": "pg"});
        let result = format_tool_schema("query_datasource", &input, true);
        assert_eq!(result["input"]["sql"], "");
    }

    #[test]
    fn format_tool_schema_search_catalog_missing_query() {
        let input = serde_json::json!({"datasource": "bq"});
        let result = format_tool_schema("search_catalog", &input, true);
        assert_eq!(result["input"]["query"], "");
    }

    #[test]
    fn format_tool_schema_get_table_info_missing_table_name() {
        let input = serde_json::json!({"datasource": "pg"});
        let result = format_tool_schema("get_table_info", &input, true);
        assert_eq!(result["input"]["table_name"], "");
    }

    #[test]
    fn format_tool_schema_validate_sql_missing_fields() {
        let input = serde_json::json!({});
        let result = format_tool_schema("validate_sql", &input, true);
        assert_eq!(result["input"]["sql"], "");
        assert_eq!(result["input"]["datasource"], "");
    }

    #[test]
    fn format_tool_schema_default_key_is_input_for_true() {
        let input = serde_json::json!({"foo": "bar"});
        let result = format_tool_schema("my_custom_tool", &input, true);
        assert_eq!(result["tool"], "my_custom_tool");
        assert_eq!(result["input"]["foo"], "bar");
        assert!(result.get("output").is_none());
    }

    #[test]
    fn format_tool_schema_default_key_is_output_for_false() {
        let output = serde_json::json!({"result": 42});
        let result = format_tool_schema("my_custom_tool", &output, false);
        assert_eq!(result["tool"], "my_custom_tool");
        assert_eq!(result["output"]["result"], 42);
        assert!(result.get("input").is_none());
    }

    // -- ThinkingEventType serialization tests ------------------------------

    #[test]
    fn thinking_event_type_serializes_snake_case() {
        let json = serde_json::to_string(&ThinkingEventType::AgentStart).unwrap();
        assert_eq!(json, "\"agent_start\"");

        let json = serde_json::to_string(&ThinkingEventType::AgentThought).unwrap();
        assert_eq!(json, "\"agent_thought\"");

        let json = serde_json::to_string(&ThinkingEventType::ToolExecutionStart).unwrap();
        assert_eq!(json, "\"tool_execution_start\"");

        let json = serde_json::to_string(&ThinkingEventType::ToolExecutionEnd).unwrap();
        assert_eq!(json, "\"tool_execution_end\"");

        let json = serde_json::to_string(&ThinkingEventType::AgentDecision).unwrap();
        assert_eq!(json, "\"agent_decision\"");

        let json = serde_json::to_string(&ThinkingEventType::AgentComplete).unwrap();
        assert_eq!(json, "\"agent_complete\"");

        let json = serde_json::to_string(&ThinkingEventType::Error).unwrap();
        assert_eq!(json, "\"error\"");
    }

    #[test]
    fn thinking_event_type_deserializes_snake_case() {
        let event: ThinkingEventType = serde_json::from_str("\"agent_start\"").unwrap();
        assert_eq!(event, ThinkingEventType::AgentStart);

        let event: ThinkingEventType = serde_json::from_str("\"tool_execution_end\"").unwrap();
        assert_eq!(event, ThinkingEventType::ToolExecutionEnd);
    }

    // -- AgentThinkingEvent serialization tests ----------------------------------

    #[test]
    fn thinking_event_skips_none_fields() {
        let event = AgentThinkingEvent {
            event_type: ThinkingEventType::AgentThought,
            timestamp: "2025-01-15T10:00:00Z".to_string(),
            title: "Planning".to_string(),
            event_id: None,
            description: None,
            data: None,
            duration_ms: None,
            has_full_text: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert!(json.get("event_id").is_none());
        assert!(json.get("description").is_none());
        assert!(json.get("data").is_none());
        assert!(json.get("duration_ms").is_none());
        // Required fields are present.
        assert_eq!(json["event_type"], "agent_thought");
        assert_eq!(json["title"], "Planning");
    }

    #[test]
    fn thinking_event_includes_some_fields() {
        let event = AgentThinkingEvent {
            event_type: ThinkingEventType::ToolExecutionStart,
            timestamp: "2025-01-15T10:00:00Z".to_string(),
            title: "Querying".to_string(),
            event_id: Some("123-001".to_string()),
            description: Some("Working on it...".to_string()),
            data: Some(serde_json::json!({"tool_name": "query_datasource"})),
            duration_ms: Some(150),
            has_full_text: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event_id"], "123-001");
        assert_eq!(json["description"], "Working on it...");
        assert_eq!(json["data"]["tool_name"], "query_datasource");
        assert_eq!(json["duration_ms"], 150);
    }

    #[test]
    fn thinking_event_round_trip() {
        let event = AgentThinkingEvent {
            event_type: ThinkingEventType::AgentComplete,
            timestamp: "2025-01-15T10:00:00Z".to_string(),
            title: "Done".to_string(),
            event_id: Some("abc-001".to_string()),
            description: Some("Completed in 500ms".to_string()),
            data: Some(serde_json::json!({"result": "success"})),
            duration_ms: Some(500),
            has_full_text: false,
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: AgentThinkingEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.event_type, ThinkingEventType::AgentComplete);
        assert_eq!(deserialized.title, "Done");
        assert_eq!(deserialized.event_id, Some("abc-001".to_string()));
        assert_eq!(deserialized.duration_ms, Some(500));
    }

    // -- get_friendly_name tests --------------------------------------------

    #[test]
    fn get_friendly_name_known_tool() {
        assert_eq!(get_friendly_name("search_catalog"), "Searching for tables");
        assert_eq!(get_friendly_name("query_datasource"), "Querying your data");
        assert_eq!(get_friendly_name("validate_sql"), "Validating SQL");
        assert_eq!(get_friendly_name("forecast_data"), "Generating forecast");
    }

    #[test]
    fn get_friendly_name_unknown_tool() {
        assert_eq!(get_friendly_name("totally_unknown"), "Using tool");
        assert_eq!(get_friendly_name(""), "Using tool");
    }

    #[test]
    fn get_friendly_name_long_tool_name() {
        // A very long or unexpected tool name should still return the static
        // fallback without panicking.
        let long_name = "a".repeat(500);
        assert_eq!(get_friendly_name(&long_name), "Using tool");
    }

    // -- generate_event_id tests --------------------------------------------

    #[test]
    fn generate_event_id_format_and_incrementing() {
        // We need a RedisPool for the tracker, but generate_event_id
        // does not use Redis, so we test the format by building a tracker
        // only if Redis is available, otherwise we test the logic directly.
        //
        // Since generate_event_id is a method on the tracker, we test the
        // format pattern and incrementing counter directly with a minimal
        // struct approach.

        // Test the counter incrementing logic manually.
        let mut counter: u32 = 0;
        let ids: Vec<String> = (0..3)
            .map(|_| {
                let ts = chrono::Utc::now().timestamp_millis();
                counter += 1;
                format!("{}-{:03}", ts, counter)
            })
            .collect();

        // Each ID should have the format: <millis>-<3-digit-counter>
        for (i, id) in ids.iter().enumerate() {
            let parts: Vec<&str> = id.rsplitn(2, '-').collect();
            assert_eq!(parts.len(), 2);
            // Counter part is zero-padded 3 digits.
            let counter_str = parts[0];
            assert_eq!(counter_str.len(), 3);
            let counter_val: u32 = counter_str.parse().unwrap();
            assert_eq!(counter_val, (i + 1) as u32);
        }

        // Counter should increment.
        assert_eq!(counter, 3);
    }

    // -- get_events_for_storage tests ---------------------------------------

    #[test]
    fn get_events_for_storage_produces_correct_json() {
        // Verify the serialization format without needing Redis.
        let events = vec![
            AgentThinkingEvent {
                event_type: ThinkingEventType::AgentStart,
                timestamp: "2025-01-15T10:00:00Z".to_string(),
                title: "Starting analysis".to_string(),
                event_id: Some("100-001".to_string()),
                description: Some("Analyzing your question".to_string()),
                data: None,
                duration_ms: None,
                has_full_text: false,
            },
            AgentThinkingEvent {
                event_type: ThinkingEventType::ToolExecutionStart,
                timestamp: "2025-01-15T10:00:01Z".to_string(),
                title: "Querying data".to_string(),
                event_id: Some("100-002".to_string()),
                description: Some("Working on it...".to_string()),
                data: Some(serde_json::json!({"tool_name": "query_datasource"})),
                duration_ms: None,
                has_full_text: false,
            },
            AgentThinkingEvent {
                event_type: ThinkingEventType::AgentComplete,
                timestamp: "2025-01-15T10:00:05Z".to_string(),
                title: "Analysis complete".to_string(),
                event_id: Some("100-003".to_string()),
                description: Some("Completed in 5000ms".to_string()),
                data: Some(serde_json::json!({"result": "done"})),
                duration_ms: Some(5000),
                has_full_text: false,
            },
        ];

        // Mimic get_events_for_storage logic.
        let storage: Vec<serde_json::Value> = events
            .iter()
            .map(|event| {
                serde_json::json!({
                    "event_id": event.event_id,
                    "event_type": event.event_type,
                    "timestamp": event.timestamp,
                    "title": event.title,
                    "description": event.description,
                    "data": event.data,
                    "duration_ms": event.duration_ms,
                })
            })
            .collect();

        assert_eq!(storage.len(), 3);

        // First event.
        assert_eq!(storage[0]["event_id"], "100-001");
        assert_eq!(storage[0]["event_type"], "agent_start");
        assert_eq!(storage[0]["title"], "Starting analysis");
        assert_eq!(
            storage[0]["description"],
            "Analyzing your question"
        );
        assert!(storage[0]["data"].is_null());
        assert!(storage[0]["duration_ms"].is_null());

        // Second event.
        assert_eq!(storage[1]["event_id"], "100-002");
        assert_eq!(storage[1]["event_type"], "tool_execution_start");
        assert_eq!(
            storage[1]["data"]["tool_name"],
            "query_datasource"
        );

        // Third event.
        assert_eq!(storage[2]["event_id"], "100-003");
        assert_eq!(storage[2]["event_type"], "agent_complete");
        assert_eq!(storage[2]["duration_ms"], 5000);
        assert_eq!(storage[2]["data"]["result"], "done");
    }

    // -- Contract: Event ID format ------------------------------------------

    #[test]
    fn event_id_format_matches_pattern() {
        // Event IDs follow the format: {timestamp_ms}-{3-digit-counter}
        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        let counter = 1_u32;
        let event_id = format!("{}-{:03}", timestamp_ms, counter);

        // Should be parseable.
        let parts: Vec<&str> = event_id.rsplitn(2, '-').collect();
        assert_eq!(parts.len(), 2);
        let counter_str = parts[0];
        assert_eq!(counter_str.len(), 3);
        let parsed_counter: u32 = counter_str.parse().unwrap();
        assert_eq!(parsed_counter, 1);
        let parsed_ts: i64 = parts[1].parse().unwrap();
        assert!(parsed_ts > 0);
    }

    #[test]
    fn event_id_counter_increments_sequentially() {
        // Simulate the counter logic without needing a full tracker.
        let mut counter: u32 = 0;
        let ids: Vec<String> = (0..5)
            .map(|_| {
                let ts = chrono::Utc::now().timestamp_millis();
                counter += 1;
                format!("{}-{:03}", ts, counter)
            })
            .collect();

        for (i, id) in ids.iter().enumerate() {
            let parts: Vec<&str> = id.rsplitn(2, '-').collect();
            let counter_val: u32 = parts[0].parse().unwrap();
            assert_eq!(counter_val, (i + 1) as u32);
        }
    }

    #[test]
    fn event_id_counter_zero_padded() {
        let ts = chrono::Utc::now().timestamp_millis();
        let id_1 = format!("{}-{:03}", ts, 1_u32);
        let id_10 = format!("{}-{:03}", ts, 10_u32);
        let id_100 = format!("{}-{:03}", ts, 100_u32);

        assert!(id_1.ends_with("-001"));
        assert!(id_10.ends_with("-010"));
        assert!(id_100.ends_with("-100"));
    }

    // -- Contract: All friendly names are defined ----------------------------

    #[test]
    fn all_friendly_names_are_non_empty() {
        for (name, friendly) in TOOL_FRIENDLY_NAMES {
            assert!(!name.is_empty(), "Tool name should not be empty");
            assert!(
                !friendly.is_empty(),
                "Friendly name for '{name}' should not be empty"
            );
        }
    }

    #[test]
    fn friendly_names_cover_all_common_tools() {
        let expected_tools = [
            "search_knowledge",
            "list_datasources",
            "get_table_info",
            "query_datasource",
            "get_chartml_spec",
            "validate_sql",
            "forecast_data",
        ];
        for tool in &expected_tools {
            assert_ne!(
                get_friendly_name(tool),
                "Using tool",
                "Tool '{tool}' should have a specific friendly name, not the fallback"
            );
        }
    }

    // -- Contract: clean_thought handles various prefix combinations --------

    #[test]
    fn clean_thought_strips_prefix_i_will() {
        let result = clean_thought("I will query the orders table next").unwrap().display;
        assert_eq!(result, "query the orders table next");
    }

    #[test]
    fn clean_thought_strips_prefix_now_ill() {
        let result = clean_thought("Now I'll check if the dates match").unwrap().display;
        assert_eq!(result, "check if the dates match");
    }

    #[test]
    fn clean_thought_strips_prefix_next_ill() {
        let result = clean_thought("Next, I'll validate the SQL query").unwrap().display;
        assert_eq!(result, "validate the SQL query");
    }

    #[test]
    fn clean_thought_strips_prefix_response() {
        let result = clean_thought("Response: here is the data analysis").unwrap().display;
        assert_eq!(result, "here is the data analysis");
    }

    #[test]
    fn clean_thought_strips_prefix_i_should() {
        let result = clean_thought("I should look at the revenue table").unwrap().display;
        assert_eq!(result, "look at the revenue table");
    }

    #[test]
    fn clean_thought_strips_prefix_ill_first() {
        let result = clean_thought("I'll first check the table schema").unwrap().display;
        assert_eq!(result, "check the table schema");
    }

    // -- Contract: clean_thought memory block edge cases ---------------------

    #[test]
    fn clean_thought_memory_block_case_insensitive() {
        let input = "Start <MEMORY>should be removed</MEMORY> end of thought here";
        let result = clean_thought(input).unwrap().display;
        assert!(!result.contains("should be removed"));
        assert!(result.contains("Start"));
    }

    #[test]
    fn clean_thought_multiple_memory_blocks() {
        let input = "A <memory>block1</memory> B <memory>block2</memory> C end of thought here";
        let result = clean_thought(input).unwrap().display;
        assert!(!result.contains("block1"));
        assert!(!result.contains("block2"));
        assert!(result.contains("A"));
        assert!(result.contains("B"));
        assert!(result.contains("C"));
    }

    // -- Contract: AgentThinkingEvent with all event types -----------------------

    #[test]
    fn thinking_event_agent_start_serializes() {
        let event = AgentThinkingEvent {
            event_type: ThinkingEventType::AgentStart,
            timestamp: "2025-01-15T10:00:00Z".into(),
            title: "Starting".into(),
            event_id: Some("id-001".into()),
            description: Some("desc".into()),
            data: None,
            duration_ms: None,
            has_full_text: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event_type"], "agent_start");
    }

    #[test]
    fn thinking_event_agent_decision_serializes() {
        let event = AgentThinkingEvent {
            event_type: ThinkingEventType::AgentDecision,
            timestamp: "2025-01-15T10:00:00Z".into(),
            title: "Deciding".into(),
            event_id: None,
            description: None,
            data: None,
            duration_ms: None,
            has_full_text: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event_type"], "agent_decision");
    }

    #[test]
    fn thinking_event_error_serializes() {
        let event = AgentThinkingEvent {
            event_type: ThinkingEventType::Error,
            timestamp: "2025-01-15T10:00:00Z".into(),
            title: "Error occurred".into(),
            event_id: Some("err-001".into()),
            description: Some("Tool failed".into()),
            data: Some(serde_json::json!({"error": "timeout"})),
            duration_ms: None,
            has_full_text: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event_type"], "error");
        assert_eq!(json["title"], "Error occurred");
        assert_eq!(json["data"]["error"], "timeout");
    }

    // -- Contract: ThinkingEventType all variants deserialize ---------------

    #[test]
    fn thinking_event_type_all_variants_deserialize() {
        let variants = [
            ("\"agent_start\"", ThinkingEventType::AgentStart),
            ("\"agent_thought\"", ThinkingEventType::AgentThought),
            ("\"tool_execution_start\"", ThinkingEventType::ToolExecutionStart),
            ("\"tool_execution_end\"", ThinkingEventType::ToolExecutionEnd),
            ("\"agent_decision\"", ThinkingEventType::AgentDecision),
            ("\"agent_complete\"", ThinkingEventType::AgentComplete),
            ("\"error\"", ThinkingEventType::Error),
        ];
        for (json_str, expected) in &variants {
            let deserialized: ThinkingEventType = serde_json::from_str(json_str).unwrap();
            assert_eq!(deserialized, *expected, "Failed for {json_str}");
        }
    }

    // -- Contract: get_events_for_storage empty events ----------------------

    #[test]
    fn get_events_for_storage_empty_returns_empty() {
        let events: Vec<AgentThinkingEvent> = vec![];
        let storage: Vec<serde_json::Value> = events
            .iter()
            .map(|event| {
                serde_json::json!({
                    "event_id": event.event_id,
                    "event_type": event.event_type,
                    "timestamp": event.timestamp,
                    "title": event.title,
                    "description": event.description,
                    "data": event.data,
                    "duration_ms": event.duration_ms,
                })
            })
            .collect();
        assert!(storage.is_empty());
    }

    // -- Contract: get_events_for_storage format stability ------------------

    #[test]
    fn get_events_for_storage_always_includes_required_fields() {
        let event = AgentThinkingEvent {
            event_type: ThinkingEventType::AgentThought,
            timestamp: "2025-06-01T12:00:00Z".into(),
            title: "Thinking about data".into(),
            event_id: None,
            description: None,
            data: None,
            duration_ms: None,
            has_full_text: false,
        };

        let storage = serde_json::json!({
            "event_id": event.event_id,
            "event_type": event.event_type,
            "timestamp": event.timestamp,
            "title": event.title,
            "description": event.description,
            "data": event.data,
            "duration_ms": event.duration_ms,
        });

        // These fields should always be present in the storage format.
        assert!(storage.get("event_type").is_some());
        assert!(storage.get("timestamp").is_some());
        assert!(storage.get("title").is_some());
        // Even if None, the keys are present (as JSON null).
        assert!(storage.get("event_id").is_some());
        assert!(storage.get("description").is_some());
        assert!(storage.get("data").is_some());
        assert!(storage.get("duration_ms").is_some());
    }
}
