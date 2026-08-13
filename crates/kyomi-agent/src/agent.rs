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

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use regex::{Regex, Replacer};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::provider::LLMProvider;
use crate::tools::{ToolContext, ToolFilter, ToolRegistry};
use crate::types::{Message, Tool, ToolCall};

// ---------------------------------------------------------------------------
// ChartML validation logging
// ---------------------------------------------------------------------------

/// SQL to record a ChartML validation failure for prompt-tuning analysis.
pub(crate) const CHARTML_VALIDATION_LOG_INSERT_SQL: &str =
    "INSERT INTO chartml_validation_log \
     (session_id, workspace_id, user_id, raw_response, error_message, error_type, \
      retry_attempt, component, model) \
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)";

/// SQL to back-fill `retry_succeeded` once a session finishes.
///
/// Only rows that are still `NULL` (written during the current session) are
/// updated, so a single UPDATE covers all retries for the session at once.
pub(crate) const CHARTML_VALIDATION_LOG_UPDATE_SQL: &str =
    "UPDATE chartml_validation_log SET retry_succeeded = $1 \
     WHERE session_id = $2 AND workspace_id = $3 AND retry_succeeded IS NULL";

/// Classify a ChartML validation error string into a coarse category.
///
/// The categories correspond to the `error_type` column and are used for
/// aggregation in prompt-tuning queries.
pub(crate) fn classify_chartml_error(error: &str) -> &'static str {
    if error.contains("invalid YAML") {
        "yaml_parse"
    } else if error.contains("missing required key") {
        "missing_key"
    } else if error.contains("SQL error") || error.contains("sql error") {
        "sql_error"
    } else {
        "unknown"
    }
}

// ---------------------------------------------------------------------------
// Iteration budget notices
// ---------------------------------------------------------------------------

/// Opening of the ephemeral iteration-budget notice.
///
/// The notice is wrapped in a `<system-reminder>` tag and written in harness
/// voice so the model reads it as tooling output rather than user speech. It
/// is appended to the tail of the LLM request and is never pushed to
/// [`AgentState::messages`], so `AgentAdapter::persist_after_chat` never sees
/// it, it never reaches `chat_messages`, and it is never reloaded on a later
/// turn of the same session.
///
/// It is deliberately kept out of the system prompt: Anthropic requests set a
/// request-level `cache_control` breakpoint, and the cached prefix must stay
/// byte-identical when the notice fires.
pub(crate) const BUDGET_NOTICE_PREFIX: &str = "<system-reminder>Iteration budget:";

/// Ephemeral instruction sent with the forced wrap-up turn.
///
/// Paired with an empty tool slice, which every provider translates into a
/// request carrying no `tools` key at all, so the model cannot emit another
/// tool call and must answer from what it already gathered.
///
/// Says "tool-use budget" rather than "iteration budget" because three
/// independent conditions can trigger this turn (see [`StopReason`]) —
/// iterations exhausted, the wall-clock deadline, or the token ceiling —
/// and the wording must stay accurate regardless of which one fired.
pub(crate) const WRAP_UP_INSTRUCTION: &str =
    "<system-reminder>The tool-use budget for this turn is exhausted and no tools \
     are available for this request. Answer the user now, using only what has already been \
     gathered in this conversation. Say plainly which parts could not be completed, but still \
     give the best answer the collected information supports. This notice comes from the \
     harness, not from the user; do not quote it or reply to it.</system-reminder>";

/// Message body of the [`kyomi_core::Error::Internal`] returned when the
/// wrap-up turn itself fails — the turn produced nothing, and reporting that as
/// a successful answer would hide the provider failure behind a plausible
/// reply.
///
/// **This is not user-facing text on its own.** Two prefixes are prepended
/// before it reaches a user:
///
/// 1. `Error::Internal`'s `Display`, `internal: {0}` —
///    `crates/kyomi-core/src/error.rs:55`.
/// 2. The chat error wrapper, `I encountered an error while processing your
///    request: {error_str}` — `crates/kyomi-agent/src/execution.rs:450`.
///
/// So it must read as a lowercase, non-apologising *clause*, not a standalone
/// sentence addressed to the user: what the user reads is
/// `"I encountered an error while processing your request: internal: "`
/// immediately followed by this string, and the whole thing has to parse as one
/// sentence. It must also avoid the substring `cancelled`, which
/// `execution.rs:439` uses to classify the turn as a user cancellation.
///
/// Reworded in KYO-344 review follow-up; the previous first-person apology
/// produced two apologies and a stray `internal:` mid-sentence. Reworded
/// again in KYO-345 from "iteration budget" to "tool-use budget" — the
/// wrap-up turn this message reports on can now be forced by three
/// independent conditions (see [`StopReason`]), not iteration count alone.
pub(crate) const WRAP_UP_FAILED_MESSAGE: &str =
    "the tool-use budget for this request was exhausted and the final summary could not be \
     generated; try rephrasing the question or breaking it into smaller parts";

/// Fractions of the iteration budget (in tenths) at which a notice fires.
///
/// Ascending order is required: the crossing count in `chat()` treats the last
/// entry as the most urgent one.
const BUDGET_NOTICE_TENTHS: [u32; 2] = [7, 9];

/// Zero-based iteration indices at which the graduated budget notices fire.
///
/// Derived from the configured ceiling, never hardcoded: a caller with
/// `max_iterations = 25` gets `[17, 22]`, one with `20` gets `[14, 18]`, so
/// changing the ceiling (KYO-345) moves the notices with it.
///
/// Small ceilings collapse both entries onto the same index (`3` yields
/// `[2, 2]`) and very small ones onto `0`. Neither is special-cased here —
/// `chat()` fires the highest *un-fired* threshold that the current iteration
/// has crossed, so a collapsed pair produces exactly one notice.
fn budget_notice_thresholds(max_iterations: u32) -> [u32; BUDGET_NOTICE_TENTHS.len()] {
    // u64 keeps the product overflow-free for any u32 ceiling; the quotient can
    // never exceed `max_iterations`, so narrowing back to u32 is exact.
    BUDGET_NOTICE_TENTHS.map(|tenths| (u64::from(max_iterations) * u64::from(tenths) / 10) as u32)
}

/// Render the ephemeral iteration-budget notice.
///
/// `used` is the number of iterations already consumed when the notice fires.
/// `final_notice` selects the stronger wording carried by the last (most
/// urgent) threshold.
fn budget_notice_text(used: u32, max_iterations: u32, final_notice: bool) -> String {
    let remaining = max_iterations.saturating_sub(used);
    let guidance = if final_notice {
        "Stop gathering new information and produce the final answer now from what is already \
         in context. If the budget runs out, one further request will be made with all tools \
         withheld."
    } else {
        "Begin consolidating: prefer answering from what has already been gathered over opening \
         new lines of investigation."
    };
    format!(
        "{BUDGET_NOTICE_PREFIX} {used} of {max_iterations} tool-use iterations for this turn \
         have been used, {remaining} remaining. {guidance} This notice comes from the harness, \
         not from the user; do not quote it or reply to it.</system-reminder>"
    )
}

// ---------------------------------------------------------------------------
// Non-iteration stop conditions (KYO-345)
// ---------------------------------------------------------------------------

/// Which condition ended the agent loop before the LLM produced a final
/// answer on its own (i.e. before `response.tool_calls.is_none()`).
///
/// Reported alongside iterations used, elapsed time, and accumulated tokens
/// in the exhaustion `info!` in [`CustomAgent::chat`], so the three causes
/// stay separable in telemetry — "the model kept calling tools for 50
/// turns," "a single slow tool call blew the wall-clock deadline," and "the
/// model looped through a context-heavy tool" are different operational
/// problems and must not collapse into one counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopReason {
    /// `AgentConfig::max_iterations` tool-use iterations were consumed.
    Iterations,
    /// `AgentConfig::max_duration` wall-clock elapsed.
    Deadline,
    /// `AgentConfig::max_total_tokens` cumulative billable tokens were
    /// consumed.
    TokenBudget,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StopReason::Iterations => "iterations",
            StopReason::Deadline => "deadline",
            StopReason::TokenBudget => "token_budget",
        })
    }
}

/// Check the two non-iteration stop conditions.
///
/// Pure — no I/O, no side effects, no access to `self` — so it can be
/// unit-tested directly without driving the agent loop. Deliberately does
/// **not** check `max_iterations`; that bound is the `for` loop's own range
/// in [`CustomAgent::chat`], not this function's concern.
///
/// Returns `None` if neither limit is configured, or if the configured
/// limit(s) have not yet been reached. When both limits are breached on the
/// same check, [`StopReason::Deadline`] is reported — an arbitrary but
/// fixed tie-break, so the same inputs always report the same reason.
pub(crate) fn check_stop_conditions(
    max_duration: Option<Duration>,
    max_total_tokens: Option<u64>,
    elapsed: Duration,
    accumulated_tokens: u64,
) -> Option<StopReason> {
    if max_duration.is_some_and(|limit| elapsed >= limit) {
        return Some(StopReason::Deadline);
    }
    if max_total_tokens.is_some_and(|limit| accumulated_tokens >= limit) {
        return Some(StopReason::TokenBudget);
    }
    None
}

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
    /// Wall-clock deadline for a single `chat()` call, checked at the top of
    /// every loop iteration via [`tokio::time::Instant::elapsed`]. `None`
    /// disables the guard.
    ///
    /// `max_iterations` alone cannot bound wall-clock time: a single slow
    /// tool call or LLM round-trip can consume most of a request's latency
    /// budget without touching the iteration counter. This is the
    /// independent guard for that case, so raising `max_iterations` (KYO-345
    /// raised chat's to 50) cannot silently double worst-case latency.
    pub max_duration: Option<Duration>,
    /// Cumulative billable-token ceiling across a single `chat()` call.
    /// `None` disables the guard.
    ///
    /// **Deliberately excludes `cache_read_input_tokens`.** Anthropic (and
    /// other providers') prompt-cache reads are billed at roughly a tenth of
    /// a fresh input token, so counting them at full weight would turn this
    /// guard into a measure of context size rather than the cost proxy it is
    /// meant to be — a long conversation with a large cached prefix would
    /// trip the ceiling on cache traffic alone, punishing exactly the case
    /// prompt caching exists to make cheap. Each iteration instead
    /// contributes `input_tokens + cache_creation_input_tokens +
    /// output_tokens` — the three components that are billed at (near) full
    /// price. Do not "fix" this to include cache reads without re-deriving
    /// the cost model first.
    pub max_total_tokens: Option<u64>,
    /// Sampling temperature (0.0-1.0). `None` uses model default.
    pub temperature: Option<f32>,
    /// Maximum tokens to generate per LLM call.
    pub max_tokens: u32,
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
            // Conservative library defaults: a caller that does not think
            // about these guards should not silently inherit interactive
            // chat's more generous ceiling (KYO-345).
            max_duration: Some(Duration::from_secs(15 * 60)),
            max_total_tokens: Some(1_500_000),
            temperature: None,
            max_tokens: 4096,
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

        // Ephemeral iteration-budget notice.  Like the ChartML retry messages
        // it is appended to the LLM context and never added to
        // `self.state.messages`, so it is never persisted and never reloaded
        // on a later turn.  Unlike them it must survive tool-call iterations,
        // so it is deliberately *not* cleared where they are cleared below.
        let mut budget_notice: Option<Message> = None;
        // How many notice thresholds have already fired during this call.
        let mut budget_notices_fired = 0usize;
        let budget_thresholds = budget_notice_thresholds(self.config.max_iterations);

        // Wall-clock start of this `chat()` call, for `max_duration`.
        // `tokio::time::Instant`, not `std::time::Instant`: the former
        // advances under a paused test clock (`#[tokio::test(start_paused =
        // true)]`), which the deadline test relies on for determinism.
        let loop_start = Instant::now();
        // Cumulative billable tokens across this call, for `max_total_tokens`.
        // Excludes cache reads — see `AgentConfig::max_total_tokens` doc.
        let mut accumulated_tokens: u64 = 0;
        // Number of LLM calls made in the loop below (not counting the
        // wrap-up call), for the exhaustion `info!`.
        let mut iterations_used: u32 = 0;
        // Which condition ended the loop. Overwritten if a non-iteration
        // guard fires; left at `Iterations` if the loop runs to completion.
        let mut stop_reason = StopReason::Iterations;

        // Iteration loop.
        for iteration in 0..self.config.max_iterations {
            // Check cancellation.
            if cancel_token.is_cancelled() {
                return Err(kyomi_core::Error::Internal("Request cancelled".into()));
            }

            if let Some(reason) = check_stop_conditions(
                self.config.max_duration,
                self.config.max_total_tokens,
                loop_start.elapsed(),
                accumulated_tokens,
            ) {
                stop_reason = reason;
                break;
            }

            self.state.global_iteration += 1;

            // Graduated budget pressure.  Counting how many thresholds this
            // iteration has crossed (rather than testing `iteration ==
            // threshold`) means a threshold can never be skipped, and that
            // thresholds which collapse onto the same index for a small
            // ceiling fire once rather than twice.  A later notice *replaces*
            // the earlier one — `Option` makes stacking structurally
            // impossible.
            let crossed = budget_thresholds
                .iter()
                .filter(|&&threshold| iteration >= threshold)
                .count();
            if crossed > budget_notices_fired {
                budget_notices_fired = crossed;
                budget_notice = Some(Message::user(budget_notice_text(
                    iteration,
                    self.config.max_iterations,
                    crossed == budget_thresholds.len(),
                )));
            }

            // Build LLM context (handles compaction), then append any
            // ephemeral ChartML retry messages so the LLM sees them.
            let mut llm_messages = self.build_llm_context();
            llm_messages.extend(chartml_retry_messages.iter().cloned());
            // Budget notice last: maximum recency, immediately before the call.
            if let Some(ref notice) = budget_notice {
                llm_messages.push(notice.clone());
            }

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
                return Err(kyomi_core::Error::Internal("Request cancelled".into()));
            }

            // This LLM call happened, so it counts toward the exhaustion log
            // regardless of which branch below the response takes next
            // (return, continue, or fall through to tool execution).
            iterations_used += 1;

            // Track token usage.
            // Total input tokens = regular + cache-creation + cache-read.
            // `input_tokens` alone excludes cached tokens, which makes the
            // context display wildly wrong when prompt caching is active.
            let total_input = response.usage.input_tokens
                + response.usage.cache_creation_input_tokens
                + response.usage.cache_read_input_tokens;
            self.state.last_input_tokens = total_input;
            if let Some(ref cb) = self.callbacks.on_token_usage {
                cb(
                    total_input,
                    response.usage.output_tokens,
                    response.cost,
                );
            }

            // Billable-cost accumulator for `max_total_tokens`. Deliberately
            // excludes `cache_read_input_tokens` — see the field doc on
            // `AgentConfig::max_total_tokens`. Not the same quantity as
            // `total_input` above, which includes cache reads for the
            // (unrelated) context-size display.
            accumulated_tokens += u64::from(response.usage.input_tokens)
                + u64::from(response.usage.cache_creation_input_tokens)
                + u64::from(response.usage.output_tokens);

            // Fire thinking callback with structured reasoning content
            // (e.g., OpenAI o-series reasoning_content/reasoning fields).
            // This is separate from content-based thinking (the "Thinking..."
            // callback that fires when content accompanies tool calls).
            if let Some(ref thinking) = response.thinking_content
                && !thinking.is_empty()
                && let Some(ref cb) = self.callbacks.on_thinking
            {
                cb(thinking);
            }

            // No tool calls -- this is the final response.
            if response.tool_calls.is_none() {
                let content = final_response_text(response);

                if let Some(ref cb) = self.callbacks.on_preparing_response {
                    cb();
                }

                // Validate ChartML blocks if present (YAML + SQL dry-run).
                if has_chartml_blocks(&content)
                    && let Some(error_msg) = self.validate_chartml_blocks(&content).await
                {
                    warn!(error = %error_msg, "ChartML validation failed, asking LLM to fix");
                    // Log validation failure for prompt-tuning analysis.
                    self.log_chartml_validation_error(
                        &content,
                        &error_msg,
                        chartml_retry_messages.len() as i32 / 2,
                        "chat",
                    )
                    .await;
                    // Store as ephemeral retry context — NOT in self.state.messages.
                    chartml_retry_messages.push(Message::assistant(content));
                    chartml_retry_messages.push(Message::user(format!(
                        "\u{1f916} SYSTEM: Automatic ChartML validation failed. The user has NOT seen your response yet. \
                         Please fix the following errors and then repeat your FULL response:\n\n{error_msg}"
                    )));
                    continue;
                }

                // Validation passed — mark any previous retries for this session as succeeded.
                if let Some(ref sid) = self.tool_context.session_id
                    && let Err(e) = kyomi_core::db_execute!(
                        self.tool_context.db,
                        CHARTML_VALIDATION_LOG_UPDATE_SQL,
                        true,
                        sid,
                        &self.tool_context.workspace_id
                    )
                {
                    warn!(error = %e, "Failed to update ChartML validation retry_succeeded");
                }

                // Push final response to state so persist_after_chat saves it.
                self.state.messages.push(Message::assistant(&content));
                return Ok(content);
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
                return Err(kyomi_core::Error::Internal("Request cancelled".into()));
            }

            // Execute each tool call.
            for tool_call in &tool_calls {
                let result = self.execute_tool(tool_call).await;
                self.state.messages.push(Message::tool_result(
                    &tool_call.id,
                    &tool_call.name,
                    &result,
                ));
            }

            // Check cancellation after tool execution.
            if cancel_token.is_cancelled() {
                return Err(kyomi_core::Error::Internal("Request cancelled".into()));
            }

            // Check if assistant content has ChartML blocks -> validate (YAML + SQL) -> return if valid.
            if has_chartml_blocks(&response.content) {
                if let Some(error_msg) = self.validate_chartml_blocks(&response.content).await {
                    // Validation failed — store error as ephemeral retry context.
                    // The assistant message (with tool calls) is already persisted
                    // above, but the error instruction stays ephemeral.
                    warn!(error = %error_msg, "ChartML validation failed in tool response, asking LLM to fix");
                    // Log validation failure for prompt-tuning analysis.
                    self.log_chartml_validation_error(
                        &response.content,
                        &error_msg,
                        chartml_retry_messages.len() as i32,
                        "chat",
                    )
                    .await;
                    chartml_retry_messages.push(Message::user(format!(
                        "\u{1f916} SYSTEM: Automatic ChartML validation failed. The user has NOT seen your response yet. \
                         Please fix the following errors and then repeat your FULL response:\n\n{error_msg}"
                    )));
                    continue;
                }

                // Validation passed — mark any previous retries for this session as succeeded.
                if let Some(ref sid) = self.tool_context.session_id
                    && let Err(e) = kyomi_core::db_execute!(
                        self.tool_context.db,
                        CHARTML_VALIDATION_LOG_UPDATE_SQL,
                        true,
                        sid,
                        &self.tool_context.workspace_id
                    )
                {
                    warn!(error = %e, "Failed to update ChartML validation retry_succeeded");
                }

                // Return as final response.
                if let Some(ref cb) = self.callbacks.on_preparing_response {
                    cb();
                }
                self.state
                    .messages
                    .push(Message::assistant(&response.content));
                return Ok(response.content);
            }

        }

        // The loop above ended without the LLM producing a final answer —
        // either `max_iterations` ran out, or a non-iteration guard broke
        // out early (`stop_reason` was set to `Deadline`/`TokenBudget` at
        // the break site above; it stays `Iterations` otherwise).
        info!(
            max_iterations = self.config.max_iterations,
            iterations_used,
            elapsed_secs = loop_start.elapsed().as_secs_f64(),
            accumulated_tokens,
            stop_reason = %stop_reason,
            "agent loop exhausted tool-use budget"
        );

        // Mark any pending ChartML validation rows as not succeeded.
        if let Some(ref sid) = self.tool_context.session_id
            && let Err(e) = kyomi_core::db_execute!(
                self.tool_context.db,
                CHARTML_VALIDATION_LOG_UPDATE_SQL,
                false,
                sid,
                &self.tool_context.workspace_id
            )
        {
            warn!(error = %e, "Failed to update ChartML validation retry_succeeded");
        }

        // Cancellation is checked before every other LLM call in this loop;
        // the wrap-up call is no exception.
        if cancel_token.is_cancelled() {
            return Err(kyomi_core::Error::Internal("Request cancelled".into()));
        }

        // Forced wrap-up turn: everything gathered so far, plus an ephemeral
        // instruction to answer from it, and **no tools**.  With an empty tool
        // slice every provider omits the `tools` key entirely, so the model
        // cannot request another tool and must produce prose.  The instruction
        // stays out of `self.state.messages` for the same reason the budget
        // notice does — it must never reach the transcript.
        let mut wrap_up_messages = self.build_llm_context();
        // Carry over any ChartML retry messages still pending when the budget
        // ran out, in the same order the loop above uses (context, then
        // retry messages, then the most-recent ephemeral instruction last —
        // see the `llm_messages` construction near the top of the loop).
        // Without this the model has no memory of why its last chart attempt
        // failed and is likely to re-emit the same invalid block into the
        // wrap-up answer, which strip-and-degrade would then have to remove.
        wrap_up_messages.extend(chartml_retry_messages.iter().cloned());
        wrap_up_messages.push(Message::user(WRAP_UP_INSTRUCTION));

        if let Some(ref cb) = self.callbacks.on_preparing_response {
            cb();
        }

        // The wrap-up is an LLM call like any other, so it counts.
        self.state.global_iteration += 1;

        let wrap_up = tokio::select! {
            result = self.call_llm(&wrap_up_messages, &[]) => result,
            _ = cancel_token.cancelled() => {
                return Err(kyomi_core::Error::Internal(
                    "Request cancelled".into(),
                ));
            }
        };

        let response = match wrap_up {
            Ok(response) => response,
            Err(e) => {
                error!(
                    error = %e,
                    "wrap-up LLM call failed after exhausting max iterations"
                );
                return Err(kyomi_core::Error::Internal(
                    WRAP_UP_FAILED_MESSAGE.to_string(),
                ));
            }
        };

        let content = final_response_text(response);

        // Strip-and-degrade: the wrap-up turn has no tool-use budget left,
        // so a failing ChartML block cannot be sent back to the model for a
        // retry the way the two earlier validation sites (no-tool-calls and
        // tool-calls paths, above) do. Validate anyway, and on failure
        // replace the offending block(s) with a plain-text note rather than
        // returning (and persisting) a broken render.
        let content = if has_chartml_blocks(&content) {
            match self.validate_chartml_blocks_detailed(&content).await {
                Some((failing_indices, error_msg)) => {
                    warn!(
                        error = %error_msg,
                        "ChartML validation failed on the wrap-up turn — stripping the \
                         block(s) rather than retrying, since no tool-use budget remains"
                    );
                    // `retry_attempt` here is an upper bound, not an exact
                    // count: the no-tool-calls loop path pushes 2 ephemeral
                    // messages per retry (assistant + user) while the
                    // tool-calls path pushes 1 (user only), and the wrap-up
                    // can be reached after either shape. Using `len()`
                    // directly (rather than the no-tool-calls path's `/ 2`
                    // convention) never undercounts, at the cost of
                    // overcounting when the last retries came from the
                    // no-tool-calls path.
                    self.log_chartml_validation_error(
                        &content,
                        &error_msg,
                        chartml_retry_messages.len() as i32,
                        "chat_wrap_up",
                    )
                    .await;
                    // Only the failing block(s) are replaced — any other
                    // ChartML block in the same response is left
                    // byte-identical. See `strip_chartml_blocks`'s doc
                    // comment for the index contract.
                    strip_chartml_blocks(&content, &failing_indices)
                }
                None => content,
            }
        } else {
            content
        };

        self.state.messages.push(Message::assistant(&content));
        Ok(content)
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
    /// any block fails validation. Thin wrapper over
    /// [`Self::validate_chartml_blocks_detailed`] that discards the failing
    /// block indices — used by the two loop call sites (no-tool-calls and
    /// tool-calls paths), which retry the whole turn on failure and never
    /// need to know which specific block was at fault.
    async fn validate_chartml_blocks(&self, text: &str) -> Option<String> {
        self.validate_chartml_blocks_detailed(text)
            .await
            .map(|(_, message)| message)
    }

    /// Detailed sibling of [`Self::validate_chartml_blocks`] used only by the
    /// wrap-up path (`chat()`'s forced final turn), which has no retry budget
    /// left and must strip only the specific block(s) that failed rather than
    /// the whole response — see [`strip_chartml_blocks`].
    ///
    /// Runs the identical two-step validation as
    /// [`Self::validate_chartml_blocks`] (YAML structure first, short-
    /// circuiting before the SQL dry-run on failure — same as that method),
    /// so the two must never diverge in what they consider invalid. Returns
    /// `None` if all blocks are valid; otherwise the 0-based indices of every
    /// failing block (see [`chartml_block_errors`] for the indexing
    /// contract) plus the same joined error message
    /// [`Self::validate_chartml_blocks`] would have returned.
    async fn validate_chartml_blocks_detailed(&self, text: &str) -> Option<(Vec<usize>, String)> {
        fn split(errors: Vec<(usize, String)>) -> Option<(Vec<usize>, String)> {
            if errors.is_empty() {
                return None;
            }
            let indices = errors.iter().map(|(i, _)| *i).collect();
            let message = errors.iter().map(|(_, msg)| msg.as_str()).collect::<Vec<_>>().join("; ");
            Some((indices, message))
        }

        // Step 1: YAML structure validation (fast, synchronous).
        let yaml_errors = chartml_block_errors(text);
        if !yaml_errors.is_empty() {
            return split(yaml_errors);
        }

        // Step 2: SQL dry-run via shared utility (same code path as dashboard tools).
        let sql_errors = crate::tools::query_utils::chartml_sql_block_errors(
            &self.tool_context.query_context(),
            text,
        )
        .await;
        split(sql_errors)
    }

    // -----------------------------------------------------------------------
    // Internal: validation logging
    // -----------------------------------------------------------------------

    /// Persist a ChartML validation failure to the database for prompt tuning.
    ///
    /// This is fire-and-forget observability — errors are logged at `warn!`
    /// level and never propagated to the caller.
    async fn log_chartml_validation_error(
        &self,
        raw_response: &str,
        error_msg: &str,
        retry_attempt: i32,
        component: &str,
    ) {
        let error_type = classify_chartml_error(error_msg);
        let session_id = self.tool_context.session_id.as_deref().unwrap_or("");
        let model = self.client.model();

        if let Err(e) = kyomi_core::db_execute!(
            self.tool_context.db,
            CHARTML_VALIDATION_LOG_INSERT_SQL,
            session_id,
            &self.tool_context.workspace_id,
            &self.tool_context.user_id,
            raw_response,
            error_msg,
            error_type,
            retry_attempt,
            component,
            model
        ) {
            warn!(error = %e, "Failed to log ChartML validation error");
        }
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

/// Extract the user-visible text of a final LLM response.
///
/// Some models (e.g. MiMo) put the full answer in the reasoning field and
/// leave `content` empty. Use the reasoning text in that case so the user sees
/// something rather than a blank message — but only when the model chose to
/// stop, not when it was truncated by `max_tokens`.
fn final_response_text(response: crate::types::LLMResponse) -> String {
    if response.content.is_empty() && response.finish_reason == "end_turn" {
        response.thinking_content.unwrap_or_default()
    } else {
        response.content
    }
}

/// Check if text contains ChartML code blocks.
fn has_chartml_blocks(text: &str) -> bool {
    text.contains("```chartml")
}

/// Compiled regex for extracting ChartML fenced code blocks.
///
/// Mirrors `crates/kyomi-agent/src/tools/query_utils.rs`'s `chartml_re()` —
/// same literal pattern, because [`crate::tools::query_utils::extract_chartml_queries`]
/// (the SQL dry-run path) must recognize exactly the blocks this module's
/// validation and stripping both operate on. If the pattern ever needs to
/// change, it must change in both places together.
static CHARTML_RE: OnceLock<Regex> = OnceLock::new();

fn chartml_re() -> &'static Regex {
    CHARTML_RE
        .get_or_init(|| Regex::new(r"```chartml\s*\n([\s\S]*?)\n```").expect("valid regex literal"))
}

/// Plain-text note substituted for a ChartML block stripped by
/// [`strip_chartml_blocks`].
///
/// Unlike [`WRAP_UP_FAILED_MESSAGE`], this string is never embedded inside a
/// larger sentence — it lands verbatim in the middle of the model's prose
/// wrap-up answer, in place of a block that failed validation with no
/// tool-use budget left to retry. It must therefore read as a complete,
/// standalone sentence: plain, short, and non-apologising, matching the tone
/// `WRAP_UP_FAILED_MESSAGE` documents for user-facing exhaustion text.
pub(crate) const CHARTML_STRIPPED_NOTE: &str = "A chart could not be generated for this answer.";

/// Per-block YAML structure validation errors.
///
/// The primitive both [`validate_chartml_blocks`] (aggregate) and
/// [`CustomAgent::validate_chartml_blocks_detailed`] (per-block) build on.
/// Each entry's `usize` is the block's **0-based position** in
/// `chartml_re()`'s capture-iteration order (`captures_iter(text)
/// .enumerate()`), regardless of the 1-based block numbers embedded in the
/// human-readable message text. This is the same indexing contract
/// [`crate::tools::query_utils::chartml_sql_block_errors`] and
/// [`strip_chartml_blocks`] use — every function in this file and
/// `query_utils.rs` that produces or consumes a block index agrees on it. A
/// block that passes validation contributes no entry.
fn chartml_block_errors(text: &str) -> Vec<(usize, String)> {
    let re = chartml_re();
    let mut errors = Vec::new();

    for (i, cap) in re.captures_iter(text).enumerate() {
        let block_content = &cap[1];
        let mut block_errors = Vec::new();

        // Try to parse as YAML.
        let parsed: Result<serde_yaml::Value, _> = serde_yaml::from_str(block_content);
        match parsed {
            Ok(value) => {
                // Check for required keys.
                let mapping = value.as_mapping();
                let data_key = serde_yaml::Value::String("data".to_string());
                let visualize_key = serde_yaml::Value::String("visualize".to_string());
                let has_data = mapping.map(|m| m.contains_key(&data_key)).unwrap_or(false);
                let has_visualize = mapping
                    .map(|m| m.contains_key(&visualize_key))
                    .unwrap_or(false);

                if !has_data {
                    block_errors.push(format!("Block {}: missing required key 'data'", i + 1));
                }
                if !has_visualize {
                    block_errors
                        .push(format!("Block {}: missing required key 'visualize'", i + 1));
                }
            }
            Err(e) => {
                block_errors.push(format!("Block {}: invalid YAML: {}", i + 1, e));
            }
        }

        if !block_errors.is_empty() {
            errors.push((i, block_errors.join("; ")));
        }
    }

    errors
}

/// Validate ChartML blocks in the text (YAML structure only, no SQL).
///
/// Extracts all ` ```chartml ... ``` ` blocks, parses each as YAML, and
/// checks for required keys (`data` and `visualize`). Thin aggregate wrapper
/// over [`chartml_block_errors`] — see that function for the per-block
/// primitive.
///
/// `#[cfg(test)]`: production code needs failing-block *indices*
/// ([`CustomAgent::validate_chartml_blocks_detailed`] calls
/// [`chartml_block_errors`] directly for that), so the message-only
/// aggregate has no remaining production caller. Kept for the YAML-structure
/// unit tests below, which pin the aggregate error-message contract without
/// needing the index bookkeeping.
///
/// Returns `None` if all blocks are valid, or `Some(error_message)` if
/// any block fails validation.
#[cfg(test)]
fn validate_chartml_blocks(text: &str) -> Option<String> {
    let errors = chartml_block_errors(text);
    if errors.is_empty() {
        None
    } else {
        let message = errors.iter().map(|(_, msg)| msg.as_str()).collect::<Vec<_>>().join("; ");
        Some(message)
    }
}

/// Replace only the ChartML blocks at `failing_indices` with a literal
/// `replacement`, leaving every other block byte-identical. Used by
/// [`strip_chartml_blocks`]; split out so tests can exercise the
/// `$`-expansion guard directly (see
/// `strip_chartml_blocks_replacement_is_not_dollar_expanded`) without
/// depending on [`CHARTML_STRIPPED_NOTE`] staying free of `$` forever.
///
/// `failing_indices` must be 0-based positions in `chartml_re()`'s
/// capture-iteration order — see [`chartml_block_errors`]'s doc comment for
/// the shared contract. Passing indices computed under a different ordering
/// silently strips the wrong block.
///
/// The replacement is applied via [`regex::NoExpand`], not passed as a bare
/// `&str` to `replace_all`. `regex`'s `Replacer` impl for `&str` expands
/// `$1`/`${name}` capture references, and the pattern here has exactly one
/// capture group holding the raw (possibly invalid) block content —
/// passing `replacement` directly would silently reinject that content into
/// the "sanitized" output the moment `replacement` ever contains a `$`.
/// `NoExpand` closes that off by construction: it copies its string
/// verbatim, so no future edit to a replacement string can reopen this bug
/// without also changing this function.
fn strip_chartml_blocks_with(text: &str, failing_indices: &[usize], replacement: &str) -> String {
    let mut index = 0usize;
    chartml_re()
        .replace_all(text, |caps: &regex::Captures<'_>| {
            let this_index = index;
            index += 1;
            let mut out = String::new();
            if failing_indices.contains(&this_index) {
                regex::NoExpand(replacement).replace_append(caps, &mut out);
            } else {
                out.push_str(&caps[0]);
            }
            out
        })
        .into_owned()
}

/// Replace only the ChartML blocks at `failing_indices` with
/// [`CHARTML_STRIPPED_NOTE`], leaving every other block byte-identical.
///
/// Used only on the wrap-up path (`chat()`'s forced final turn): that turn
/// has no remaining tool-use budget to send a failing block back to the
/// model for a retry, so invalid blocks are stripped instead of returned to
/// the user — but valid blocks in the same response must survive untouched,
/// so the caller must pass the specific failing indices, not blanket-strip
/// every match. See [`strip_chartml_blocks_with`] for the indexing contract
/// and the `$`-expansion guard.
fn strip_chartml_blocks(text: &str, failing_indices: &[usize]) -> String {
    strip_chartml_blocks_with(text, failing_indices, CHARTML_STRIPPED_NOTE)
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
        assert_eq!(result, "[user_local_time: 2025-01-15T10:30:00+11:00] ");
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
        assert!(result.unwrap().contains("missing required key 'visualize'"));
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

    // -- strip_chartml_blocks tests (KYO-347) --------------------------------

    #[test]
    fn strip_chartml_blocks_single_block() {
        let text = "Before\n```chartml\ndata:\n  x: 1\n```\nAfter";
        assert_eq!(
            strip_chartml_blocks(text, &[0]),
            format!("Before\n{CHARTML_STRIPPED_NOTE}\nAfter")
        );
    }

    #[test]
    fn strip_chartml_blocks_multiple_blocks_all_failing() {
        let text = "One\n```chartml\na: 1\n```\nTwo\n```chartml\nb: 2\n```\nThree";
        assert_eq!(
            strip_chartml_blocks(text, &[0, 1]),
            format!("One\n{CHARTML_STRIPPED_NOTE}\nTwo\n{CHARTML_STRIPPED_NOTE}\nThree")
        );
    }

    /// The regression the reviewer caught: `strip_chartml_blocks` must not
    /// blanket-replace every match — a valid block sharing a response with an
    /// invalid one must survive byte-identically.
    #[test]
    fn strip_chartml_blocks_mixed_valid_and_invalid_only_strips_the_invalid_one() {
        let text = "One\n```chartml\nvalid: block\n```\nTwo\n```chartml\ninvalid: block\n```\nThree";
        // Only block 1 (0-based) — the second block — is failing.
        let result = strip_chartml_blocks(text, &[1]);
        assert_eq!(
            result,
            format!("One\n```chartml\nvalid: block\n```\nTwo\n{CHARTML_STRIPPED_NOTE}\nThree")
        );
    }

    /// Pins index alignment against a three-block case rather than a
    /// first/last coincidence: the failing block sits in the middle, with a
    /// valid block on each side.
    #[test]
    fn strip_chartml_blocks_middle_of_three_only_strips_the_middle_one() {
        let text = "\
A\n```chartml\nfirst: valid\n```\n\
B\n```chartml\nsecond: invalid\n```\n\
C\n```chartml\nthird: valid\n```\n\
D";
        let result = strip_chartml_blocks(text, &[1]);
        assert_eq!(
            result,
            format!(
                "A\n```chartml\nfirst: valid\n```\nB\n{CHARTML_STRIPPED_NOTE}\nC\n```chartml\nthird: valid\n```\nD"
            )
        );
    }

    #[test]
    fn strip_chartml_blocks_preserves_surrounding_prose_verbatim() {
        let text = "Intro paragraph with **markdown**.\n\n```chartml\ndata:\n  x: 1\n```\n\nOutro paragraph.";
        let result = strip_chartml_blocks(text, &[0]);
        assert!(
            result.starts_with("Intro paragraph with **markdown**.\n\n"),
            "prose before the block must be untouched: {result}"
        );
        assert!(
            result.ends_with("\n\nOutro paragraph."),
            "prose after the block must be untouched: {result}"
        );
        assert!(!result.contains("```chartml"));
    }

    #[test]
    fn strip_chartml_blocks_no_blocks_returns_input_unchanged() {
        let text = "Just plain prose, no charts here.";
        assert_eq!(strip_chartml_blocks(text, &[]), text);
    }

    #[test]
    fn strip_chartml_blocks_unterminated_fence_is_unchanged() {
        // No closing fence -- the same asymmetry as
        // `validate_chartml_blocks_partial_closing_fence` above: the regex
        // requires `\n` ``` ` to close a block, so an unterminated fence
        // never matches and passes through untouched.
        let text = "```chartml\ndata:\n  x: 1\n";
        assert_eq!(strip_chartml_blocks(text, &[0]), text);
    }

    #[test]
    fn strip_chartml_blocks_empty_failing_indices_leaves_all_blocks_untouched() {
        // No block index is marked failing, even though blocks are present —
        // every match must pass through as the original, unmodified capture.
        let text = "One\n```chartml\na: 1\n```\nTwo\n```chartml\nb: 2\n```\nThree";
        assert_eq!(strip_chartml_blocks(text, &[]), text);
    }

    /// Regression pin for the `$`-expansion trap: `regex`'s `Replacer` impl
    /// for `&str` expands `$1`/`${name}` capture references, and the pattern
    /// has exactly one capture group holding the raw block content. If a
    /// replacement string is ever passed straight to `replace_all` instead of
    /// through `regex::NoExpand`, a `$1` in that string would silently
    /// reinject the failing block's own (invalid) content into the
    /// "sanitized" output. `CHARTML_STRIPPED_NOTE` has no `$` today, so this
    /// exercises the mechanism directly via `strip_chartml_blocks_with`
    /// rather than depending on that staying true.
    #[test]
    fn strip_chartml_blocks_replacement_is_not_dollar_expanded() {
        let text = "```chartml\nSECRET_INVALID_CONTENT\n```";
        let result = strip_chartml_blocks_with(text, &[0], "replaced: $1");
        assert_eq!(result, "replaced: $1");
        assert!(
            !result.contains("SECRET_INVALID_CONTENT"),
            "the raw captured block content must never be reinjected via $-expansion: {result}"
        );
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
        assert!(context[1].content.contains("User asked about revenue"));
        assert!(context[2].content.contains("I understand the context"));
        assert_eq!(context[3].content, "Recent message");
        assert_eq!(context[4].content, "Recent response");
    }

    // -- AgentConfig tests ---------------------------------------------------

    #[test]
    fn agent_config_default_values() {
        let config = AgentConfig::default();
        assert_eq!(config.max_iterations, 25);
        // Conservative library defaults (KYO-345) — a caller that does not
        // think about these guards should not silently inherit interactive
        // chat's more generous ceiling.
        assert_eq!(config.max_duration, Some(Duration::from_secs(15 * 60)));
        assert_eq!(config.max_total_tokens, Some(1_500_000));
        assert!(config.temperature.is_none());
        assert_eq!(config.max_tokens, 4096);
        assert!(!config.log_context);
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
        assert!(!has_chartml_blocks(
            "```ChartML\ndata:\n  query: SELECT 1\n```"
        ));
        assert!(!has_chartml_blocks(
            "```CHARTML\ndata:\n  query: SELECT 1\n```"
        ));
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
            on_token_usage: Some(Box::new(|_input: u32, _output: u32, _cost: Option<f64>| {})),
            ..Default::default()
        };
        assert!(callbacks.on_token_usage.is_some());
    }

    #[test]
    fn agent_callbacks_can_set_on_tool_start() {
        let callbacks = AgentCallbacks {
            on_tool_start: Some(Box::new(|_name: &str, _args: &serde_json::Value| {})),
            ..Default::default()
        };
        assert!(callbacks.on_tool_start.is_some());
    }

    #[test]
    fn agent_callbacks_can_set_on_tool_end() {
        let callbacks = AgentCallbacks {
            on_tool_end: Some(Box::new(|_name: &str, _result: &str, _success: bool| {})),
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

    // -- classify_chartml_error tests ----------------------------------------

    #[test]
    fn classify_chartml_error_types() {
        assert_eq!(
            classify_chartml_error("Block 1: invalid YAML at line 3"),
            "yaml_parse"
        );
        assert_eq!(
            classify_chartml_error("Block 1: missing required key 'data'"),
            "missing_key"
        );
        assert_eq!(
            classify_chartml_error("Block 1: SQL error: syntax error near SELECT"),
            "sql_error"
        );
        assert_eq!(
            classify_chartml_error("Block 1: sql error: unexpected token"),
            "sql_error"
        );
        assert_eq!(classify_chartml_error("some unknown error"), "unknown");
    }

    // -- CHARTML_VALIDATION_LOG_INSERT_SQL sanity checks ---------------------

    #[test]
    fn chartml_validation_log_insert_sql_targets_correct_table() {
        assert!(
            CHARTML_VALIDATION_LOG_INSERT_SQL
                .contains("INSERT INTO chartml_validation_log")
        );
        assert!(CHARTML_VALIDATION_LOG_INSERT_SQL.contains("error_type"));
        assert!(CHARTML_VALIDATION_LOG_INSERT_SQL.contains("raw_response"));
    }

    #[test]
    fn chartml_validation_log_update_sql_targets_correct_table() {
        assert!(
            CHARTML_VALIDATION_LOG_UPDATE_SQL
                .contains("UPDATE chartml_validation_log")
        );
        assert!(CHARTML_VALIDATION_LOG_UPDATE_SQL.contains("retry_succeeded"));
    }

    // -----------------------------------------------------------------------
    // Iteration budget: scripted-provider harness
    // -----------------------------------------------------------------------

    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use serde_json::json;

    use crate::tools::AgentTool;
    use crate::types::{AgentTokenUsage, LLMResponse, MessageRole};

    /// Name of the single tool registered by [`scripted_agent`].
    const NOOP_TOOL_NAME: &str = "noop";

    /// A tool that does nothing, registered so the tool slice the agent hands
    /// to the LLM during the loop is non-empty — without it, "the wrap-up call
    /// passes no tools" would hold vacuously.
    struct NoopTool;

    #[async_trait]
    impl AgentTool for NoopTool {
        fn name(&self) -> &str {
            NOOP_TOOL_NAME
        }

        fn description(&self) -> &str {
            "Does nothing."
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({"type": "object", "properties": {}})
        }

        async fn execute(
            &self,
            _args: serde_json::Value,
            _ctx: &ToolContext,
        ) -> kyomi_core::Result<String> {
            Ok("noop".to_string())
        }
    }

    /// What [`ScriptedProvider`] returns for one `complete` call.
    #[derive(Clone)]
    enum Reply {
        /// A plain text answer — ends the agent loop. Carries the default
        /// (zero) token usage; use `TextWithUsage` when a test needs to
        /// drive `max_total_tokens`.
        Text(String),
        /// A single tool call — drives the loop into another iteration.
        /// Carries the default (zero) token usage; use `ToolCallWithUsage`
        /// when a test needs to drive `max_total_tokens`.
        ToolCall,
        /// A single tool call carrying explicit token usage — needed by the
        /// token-ceiling and cache-read tests, which must control exactly
        /// how many billable tokens each iteration contributes.
        ToolCallWithUsage(AgentTokenUsage),
        /// A single tool call preceded by `tokio::time::sleep(duration)` —
        /// needed by the deadline test, which drives the wall-clock guard
        /// under a paused tokio clock (`#[tokio::test(start_paused =
        /// true)]`) rather than a real sleep.
        SlowToolCall(Duration),
        /// A provider failure.
        Failure(String),
    }

    /// Build a single-tool-call [`LLMResponse`] carrying the given usage.
    ///
    /// Shared by the `ToolCall`, `ToolCallWithUsage`, and `SlowToolCall`
    /// branches of [`ScriptedProvider::complete`] so the noop-tool-call
    /// shape (id, name, empty arguments) is defined exactly once.
    fn tool_call_response(usage: AgentTokenUsage) -> LLMResponse {
        LLMResponse {
            content: String::new(),
            finish_reason: "tool_use".to_string(),
            usage,
            tool_calls: Some(vec![ToolCall {
                id: "tc_1".to_string(),
                name: NOOP_TOOL_NAME.to_string(),
                arguments: json!({}),
            }]),
            cost: None,
            thinking_content: None,
        }
    }

    /// One recorded `LLMProvider::complete` invocation.
    #[derive(Debug)]
    struct RecordedCall {
        messages: Vec<Message>,
        tools: Vec<Tool>,
    }

    impl RecordedCall {
        /// Messages in this call that carry an iteration-budget notice.
        fn budget_notices(&self) -> Vec<&Message> {
            self.messages
                .iter()
                .filter(|m| m.content.contains(BUDGET_NOTICE_PREFIX))
                .collect()
        }
    }

    /// An [`LLMProvider`] that records every call it receives and replays a
    /// fixed script of replies.
    struct ScriptedProvider {
        script: Mutex<VecDeque<Reply>>,
        /// Replayed once the script runs out (e.g. "keeps calling tools").
        after_script: Reply,
        calls: Arc<Mutex<Vec<RecordedCall>>>,
    }

    #[async_trait]
    impl LLMProvider for ScriptedProvider {
        async fn complete(
            &self,
            messages: &[Message],
            tools: &[Tool],
            _temperature: Option<f32>,
            _max_tokens: u32,
            _user_names: &HashMap<String, String>,
        ) -> kyomi_core::Result<LLMResponse> {
            self.calls
                .lock()
                .expect("call log mutex")
                .push(RecordedCall {
                    messages: messages.to_vec(),
                    tools: tools.to_vec(),
                });

            let reply = self
                .script
                .lock()
                .expect("script mutex")
                .pop_front()
                .unwrap_or_else(|| self.after_script.clone());

            match reply {
                Reply::Text(content) => Ok(LLMResponse {
                    content,
                    finish_reason: "end_turn".to_string(),
                    usage: AgentTokenUsage::default(),
                    tool_calls: None,
                    cost: None,
                    thinking_content: None,
                }),
                Reply::ToolCall => Ok(tool_call_response(AgentTokenUsage::default())),
                Reply::ToolCallWithUsage(usage) => Ok(tool_call_response(usage)),
                Reply::SlowToolCall(duration) => {
                    tokio::time::sleep(duration).await;
                    Ok(tool_call_response(AgentTokenUsage::default()))
                }
                Reply::Failure(message) => Err(kyomi_core::Error::Internal(message)),
            }
        }

        fn model(&self) -> &str {
            "scripted-model"
        }
    }

    /// Build a `ToolContext` over an in-memory sqlite pool and in-memory KV
    /// store. `session_id: None` keeps the agent off the ChartML validation
    /// log writes, so no migrated schema is required.
    async fn test_tool_context() -> ToolContext {
        let db = kyomi_core::DbPool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let ws_manager = kyomi_auth::websocket::WebSocketManager::new(None, db.clone());

        ToolContext {
            db,
            kv: kyomi_core::kv_store_memory::InMemoryKVStore::new_pool(),
            user_id: "user-a".to_string(),
            workspace_id: "ws-1".to_string(),
            encryption_key: Arc::new([0u8; 32]),
            embedding: kyomi_embed::LazyEmbedding::new(),
            ws_manager,
            config: Arc::new(kyomi_core::Config::test_config()),
            session_id: None,
            supports_mcp_apps: false,
            workspace_roles: Vec::new(),
            connect_registry: None,
            platforms: Arc::new(kyomi_core::platform::PlatformRegistry::new()),
            user_display_name: "User A".to_string(),
        }
    }

    /// Build an agent wired to a scripted provider and a one-tool registry,
    /// with full control over the config fields a test cares about.
    async fn scripted_agent_with_config(
        config: AgentConfig,
        script: Vec<Reply>,
        after_script: Reply,
    ) -> (CustomAgent, Arc<Mutex<Vec<RecordedCall>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider {
            script: Mutex::new(script.into()),
            after_script,
            calls: Arc::clone(&calls),
        };

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(NoopTool));

        let agent = CustomAgent::new(
            Box::new(provider),
            config,
            Arc::new(registry),
            test_tool_context().await,
            HashMap::new(),
        );

        (agent, calls)
    }

    /// Build an agent wired to a scripted provider and a one-tool registry,
    /// with every stop-condition field but `max_iterations` at its library
    /// default (15 min / 1.5M tokens — high enough that the fast, low-token
    /// scripted tests never trip them incidentally).
    async fn scripted_agent(
        max_iterations: u32,
        script: Vec<Reply>,
        after_script: Reply,
    ) -> (CustomAgent, Arc<Mutex<Vec<RecordedCall>>>) {
        scripted_agent_with_config(
            AgentConfig {
                max_iterations,
                ..Default::default()
            },
            script,
            after_script,
        )
        .await
    }

    /// Run a full budget (`max_iterations` tool-calling iterations) and return
    /// the recorded calls, including the final wrap-up call.
    async fn run_until_exhausted(
        max_iterations: u32,
        wrap_up: Reply,
    ) -> (CustomAgent, Vec<RecordedCall>) {
        let script = vec![Reply::ToolCall; max_iterations as usize];
        let (mut agent, calls) = scripted_agent(max_iterations, script, wrap_up).await;
        agent
            .chat("hello", CancellationToken::new(), None, None, None)
            .await
            .expect("chat should complete");
        // The agent (via its provider) still holds a handle to the log, so
        // take the recorded calls out from under the lock rather than
        // unwrapping the Arc.
        let recorded = std::mem::take(&mut *calls.lock().expect("call log mutex"));
        (agent, recorded)
    }

    // -----------------------------------------------------------------------
    // Iteration budget: thresholds
    // -----------------------------------------------------------------------

    #[test]
    fn budget_notice_thresholds_are_derived_from_the_ceiling() {
        assert_eq!(budget_notice_thresholds(25), [17, 22]);
        assert_eq!(budget_notice_thresholds(20), [14, 18]);
        // Degenerate ceilings collapse both thresholds onto a single index —
        // `chat()` must still fire only one notice for them.
        assert_eq!(budget_notice_thresholds(3), [2, 2]);
        assert_eq!(budget_notice_thresholds(1), [0, 0]);
    }

    // -----------------------------------------------------------------------
    // Iteration budget: the notice is ephemeral
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn budget_notice_never_lands_in_agent_state() {
        // 10 iterations => thresholds at 7 and 9, so both notices fire.
        let (agent, _calls) = run_until_exhausted(10, Reply::Text("wrap up".to_string())).await;

        let leaked: Vec<&Message> = agent
            .state()
            .messages
            .iter()
            .filter(|m| m.content.contains(BUDGET_NOTICE_PREFIX))
            .collect();

        assert!(
            leaked.is_empty(),
            "the budget notice must never reach state.messages — persist_after_chat \
             writes every message past messages_loaded_count to chat_messages; found: {leaked:?}"
        );
    }

    #[tokio::test]
    async fn persistable_messages_contain_only_the_genuine_user_message() {
        // Round-trip stand-in for persist_after_chat: this agent loaded nothing
        // from the DB, so `messages_loaded_count` is 0 and every message in
        // state.messages is one persist_after_chat would write (System aside).
        let (agent, _calls) = run_until_exhausted(10, Reply::Text("wrap up".to_string())).await;

        let user_messages: Vec<&Message> = agent
            .state()
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::User)
            .collect();

        assert_eq!(
            user_messages.len(),
            1,
            "exactly one row would be written with role = \"user\": {user_messages:?}"
        );
        assert_eq!(user_messages[0].content, "hello");
    }

    #[tokio::test]
    async fn budget_notice_reaches_the_llm_from_the_crossing_iteration_onward() {
        let (_agent, calls) = run_until_exhausted(10, Reply::Text("wrap up".to_string())).await;
        assert_eq!(calls.len(), 11, "10 loop iterations plus the wrap-up call");

        for (index, call) in calls.iter().take(7).enumerate() {
            assert!(
                call.budget_notices().is_empty(),
                "no notice may appear before the first threshold (call {index})"
            );
        }

        for (index, call) in calls.iter().enumerate().take(10).skip(7) {
            assert_eq!(
                call.budget_notices().len(),
                1,
                "call {index} must carry exactly one budget notice"
            );
        }

        assert!(
            calls[7]
                .messages
                .last()
                .expect("call 7 has messages")
                .content
                .contains(BUDGET_NOTICE_PREFIX),
            "the notice must sit at the tail of the request, at maximum recency"
        );
    }

    #[tokio::test]
    async fn budget_notice_survives_iterations_that_execute_tool_calls() {
        // The notice fires at iteration 7, which itself executes a tool call —
        // the point where `chartml_retry_messages` is cleared. The notice must
        // not be cleared alongside them.
        let (_agent, calls) = run_until_exhausted(10, Reply::Text("wrap up".to_string())).await;

        assert_eq!(
            calls[7].budget_notices().len(),
            1,
            "the notice fires on iteration 7"
        );
        assert_eq!(
            calls[8].budget_notices().len(),
            1,
            "the notice must survive the tool-call iteration that precedes call 8"
        );
        assert_eq!(
            calls[7].budget_notices()[0].content,
            calls[8].budget_notices()[0].content,
            "call 8 must carry the same notice, not a re-fired one"
        );
    }

    #[tokio::test]
    async fn urgent_notice_replaces_the_earlier_one_rather_than_stacking() {
        let (_agent, calls) = run_until_exhausted(10, Reply::Text("wrap up".to_string())).await;

        assert_eq!(
            calls[7].budget_notices().len(),
            1,
            "the first threshold fires exactly one notice"
        );
        assert_eq!(
            calls[9].budget_notices().len(),
            1,
            "the second threshold must replace the first, not stack with it"
        );
        assert_eq!(
            calls[7].budget_notices()[0].content,
            budget_notice_text(7, 10, false),
            "the first threshold fires the non-urgent wording"
        );
        assert_eq!(
            calls[9].budget_notices()[0].content,
            budget_notice_text(9, 10, true),
            "the second threshold fires the urgent wording"
        );
    }

    #[tokio::test]
    async fn budget_notice_fires_against_a_non_default_ceiling() {
        // 20 iterations (the copilot ceiling) => thresholds at 14 and 18.
        let (_agent, calls) = run_until_exhausted(20, Reply::Text("wrap up".to_string())).await;

        assert!(
            calls[13].budget_notices().is_empty(),
            "70% of 20 is iteration 14, so call 13 must still be clean"
        );
        assert_eq!(
            calls[14].budget_notices().len(),
            1,
            "the first threshold for a ceiling of 20 is iteration 14"
        );
        assert_eq!(
            calls[14].budget_notices()[0].content,
            budget_notice_text(14, 20, false)
        );
        assert_eq!(
            calls[18].budget_notices().len(),
            1,
            "the second threshold for a ceiling of 20 is iteration 18"
        );
        assert_eq!(
            calls[18].budget_notices()[0].content,
            budget_notice_text(18, 20, true)
        );
    }

    #[tokio::test]
    async fn collapsed_thresholds_fire_exactly_one_notice() {
        // 3 iterations => both thresholds land on iteration 2.
        let (_agent, calls) = run_until_exhausted(3, Reply::Text("wrap up".to_string())).await;

        assert!(calls[0].budget_notices().is_empty());
        assert!(calls[1].budget_notices().is_empty());
        assert_eq!(
            calls[2].budget_notices().len(),
            1,
            "thresholds that collapse onto one index must fire once, not twice"
        );
        assert_eq!(
            calls[2].budget_notices()[0].content,
            budget_notice_text(2, 3, true),
            "a collapsed pair fires the most urgent wording"
        );
    }

    // -----------------------------------------------------------------------
    // Iteration budget: exhaustion wrap-up
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn exhausting_the_budget_wraps_up_with_no_tools() {
        let (agent, calls) =
            run_until_exhausted(3, Reply::Text("Revenue grew 12% last quarter.".to_string()))
                .await;

        assert_eq!(calls.len(), 4, "3 loop iterations plus one wrap-up call");
        assert_eq!(
            calls[0].tools.len(),
            1,
            "loop iterations must offer the registered tool, otherwise the \
             empty-tools assertion below is vacuous"
        );
        assert!(
            calls[3].tools.is_empty(),
            "the wrap-up call must withhold every tool: {:?}",
            calls[3].tools
        );
        assert_eq!(
            calls[3]
                .messages
                .last()
                .expect("wrap-up call has messages")
                .content,
            WRAP_UP_INSTRUCTION,
            "the wrap-up instruction must sit at the tail of the request"
        );
        assert!(
            calls[3].budget_notices().is_empty(),
            "the wrap-up request carries its own instruction, not the budget notice"
        );

        assert_eq!(
            agent
                .state()
                .messages
                .last()
                .expect("state has messages")
                .content,
            "Revenue grew 12% last quarter.",
            "the wrap-up answer is the assistant message, not a canned apology"
        );
        assert!(
            !agent
                .state()
                .messages
                .iter()
                .any(|m| m.content == WRAP_UP_INSTRUCTION),
            "the wrap-up instruction is ephemeral and must not be persisted"
        );
    }

    #[tokio::test]
    async fn exhaustion_returns_the_wrap_up_answer_not_a_canned_string() {
        let script = vec![Reply::ToolCall; 3];
        let (mut agent, _calls) = scripted_agent(
            3,
            script,
            Reply::Text("Revenue grew 12% last quarter.".to_string()),
        )
        .await;

        let answer = agent
            .chat("hello", CancellationToken::new(), None, None, None)
            .await
            .expect("chat should complete");

        assert_eq!(answer, "Revenue grew 12% last quarter.");
        assert!(
            !answer.contains(WRAP_UP_FAILED_MESSAGE),
            "the canned exhaustion string must be gone: {answer}"
        );
    }

    #[tokio::test]
    async fn exhaustion_with_a_failing_wrap_up_call_returns_an_error() {
        let script = vec![Reply::ToolCall; 3];
        let (mut agent, _calls) =
            scripted_agent(3, script, Reply::Failure("provider exploded".to_string())).await;

        let result = agent
            .chat("hello", CancellationToken::new(), None, None, None)
            .await;

        let err = result.expect_err(
            "a failed wrap-up call is a real failure and must not be reported as a \
             successful answer",
        );
        assert!(
            err.to_string().contains(WRAP_UP_FAILED_MESSAGE),
            "the error must carry the exhaustion message body: {err}"
        );
        assert_eq!(
            agent
                .state()
                .messages
                .last()
                .expect("state has messages")
                .role,
            MessageRole::Tool,
            "a failed wrap-up must append nothing — the last message is still the \
             final tool result, not a substituted apology"
        );
    }

    // -----------------------------------------------------------------------
    // KYO-347: strip-and-degrade — ChartML validation on the wrap-up path
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn wrap_up_response_with_invalid_chartml_is_stripped_and_not_persisted() {
        // Missing the required `data` key -- fails YAML structure validation
        // (step 1), so this never reaches the SQL dry-run step and needs no
        // real datasource.
        let invalid_chartml =
            "Summary:\n```chartml\nvisualize:\n  type: bar\n```\nThat's the chart.";
        let script = vec![Reply::ToolCall; 2];
        let (mut agent, _calls) =
            scripted_agent(2, script, Reply::Text(invalid_chartml.to_string())).await;

        let answer = agent
            .chat("hello", CancellationToken::new(), None, None, None)
            .await
            .expect("chat should complete — strip-and-degrade, not an error");

        assert!(
            answer.contains(CHARTML_STRIPPED_NOTE),
            "the returned answer must carry the stripped-block note: {answer}"
        );
        assert!(
            !answer.contains("```chartml"),
            "the invalid block must not reach the user: {answer}"
        );
        assert!(
            answer.contains("Summary:") && answer.contains("That's the chart."),
            "surrounding prose must be preserved: {answer}"
        );

        let last_state_message = agent
            .state()
            .messages
            .last()
            .expect("state has messages");
        assert!(
            !last_state_message.content.contains("```chartml"),
            "the invalid block must never reach state.messages, or persist_after_chat \
             would write it straight into chat_messages: {}",
            last_state_message.content
        );
        assert_eq!(
            last_state_message.content, answer,
            "the message pushed to state and the returned answer must be identical — \
             never push one version and return another"
        );
    }

    #[tokio::test]
    async fn wrap_up_response_with_valid_chartml_is_returned_unchanged() {
        // `data` has no nested `query`/`datasource`, so
        // `query_utils::extract_chartml_queries` finds nothing to dry-run and
        // `validate_chartml_sql` returns `None` without touching a real
        // datasource — this block is valid on structure alone.
        let valid_chartml =
            "Here is your chart:\n```chartml\ndata:\n  values: []\nvisualize:\n  type: bar\n```\nDone.";
        let script = vec![Reply::ToolCall; 2];
        let (mut agent, _calls) =
            scripted_agent(2, script, Reply::Text(valid_chartml.to_string())).await;

        let answer = agent
            .chat("hello", CancellationToken::new(), None, None, None)
            .await
            .expect("chat should complete");

        // Exact equality against `valid_chartml` (which is known at authoring
        // time not to contain the note) already pins the absence of
        // `CHARTML_STRIPPED_NOTE` — a separate `!contains` assertion here
        // would be dead weight, not an extra guard.
        assert_eq!(
            answer, valid_chartml,
            "a valid block must pass through the wrap-up path unchanged"
        );
    }

    /// The regression the reviewer caught, exercised end-to-end through
    /// `chat()`: a wrap-up answer with two ChartML blocks, one valid and one
    /// invalid, must keep the valid chart and only degrade the invalid one —
    /// not lose both to a blanket strip.
    #[tokio::test]
    async fn wrap_up_response_with_mixed_valid_and_invalid_chartml_only_strips_the_invalid_block()
    {
        // First block: has `data` and `visualize`, no nested `query`/
        // `datasource`, so it is valid on structure alone (same reasoning as
        // `wrap_up_response_with_valid_chartml_is_returned_unchanged`).
        // Second block: missing the required `data` key entirely.
        let mixed_chartml = "First chart:\n```chartml\ndata:\n  values: []\n\
visualize:\n  type: bar\n```\nSecond chart:\n```chartml\nvisualize:\n  type: line\n```\nDone.";
        let script = vec![Reply::ToolCall; 2];
        let (mut agent, _calls) =
            scripted_agent(2, script, Reply::Text(mixed_chartml.to_string())).await;

        let answer = agent
            .chat("hello", CancellationToken::new(), None, None, None)
            .await
            .expect("chat should complete — strip-and-degrade, not an error");

        let expected = format!(
            "First chart:\n```chartml\ndata:\n  values: []\nvisualize:\n  type: bar\n```\n\
Second chart:\n{CHARTML_STRIPPED_NOTE}\nDone."
        );
        assert_eq!(
            answer, expected,
            "the valid first block must survive byte-identically; only the invalid \
             second block becomes the stripped-note text"
        );
    }

    #[tokio::test]
    async fn wrap_up_response_without_chartml_is_unchanged() {
        // Regression guard on the common path: the new `has_chartml_blocks`
        // check added to the end of `chat()` must not touch a wrap-up answer
        // that never had a chart in it.
        let script = vec![Reply::ToolCall; 2];
        let (mut agent, _calls) = scripted_agent(
            2,
            script,
            Reply::Text("Revenue grew 12% last quarter.".to_string()),
        )
        .await;

        let answer = agent
            .chat("hello", CancellationToken::new(), None, None, None)
            .await
            .expect("chat should complete");

        assert_eq!(answer, "Revenue grew 12% last quarter.");
    }

    #[tokio::test]
    async fn wrap_up_context_carries_pending_chartml_retry_messages() {
        // A response with an invalid ChartML block and no tool calls takes
        // the no-tool-calls validation path (chat(), the `response.tool_calls
        // .is_none()` branch), which queues two ephemeral retry messages and
        // `continue`s. With max_iterations = 1 the loop has no budget left to
        // actually act on the retry, so those messages must still reach the
        // forced wrap-up call — otherwise the model has no memory of why its
        // last chart attempt failed and is likely to repeat it.
        let invalid_chartml = "Here is a chart:\n```chartml\nvisualize:\n  type: bar\n```\n";
        let script = vec![Reply::Text(invalid_chartml.to_string())];
        let (mut agent, calls) = scripted_agent(
            1,
            script,
            Reply::Text("Wrap-up answer, no chart.".to_string()),
        )
        .await;

        let answer = agent
            .chat("hello", CancellationToken::new(), None, None, None)
            .await
            .expect("chat should complete");

        assert_eq!(answer, "Wrap-up answer, no chart.");

        let recorded = std::mem::take(&mut *calls.lock().expect("call log mutex"));
        assert_eq!(
            recorded.len(),
            2,
            "1 loop iteration (validation failure, no retry budget left) plus the \
             wrap-up call: {recorded:?}"
        );

        let wrap_up_call = recorded.last().expect("has calls");
        assert!(
            wrap_up_call
                .messages
                .iter()
                .any(|m| m.content.contains("ChartML validation failed")),
            "the wrap-up call must carry the pending ChartML retry context: {:?}",
            wrap_up_call.messages
        );
        assert_eq!(
            wrap_up_call
                .messages
                .last()
                .expect("wrap-up call has messages")
                .content,
            WRAP_UP_INSTRUCTION,
            "the wrap-up instruction must still be last, after the carried-over retry \
             messages"
        );
    }

    // -----------------------------------------------------------------------
    // KYO-345: non-iteration stop conditions — pure predicate
    // -----------------------------------------------------------------------

    #[test]
    fn check_stop_conditions_never_breaches_with_no_limits_configured() {
        assert_eq!(
            check_stop_conditions(None, None, Duration::from_secs(999_999), u64::MAX),
            None,
            "with both limits disabled, no elapsed time or token count can breach"
        );
    }

    #[test]
    fn check_stop_conditions_deadline_at_below_and_above_the_limit() {
        let limit = Duration::from_secs(60);
        assert_eq!(
            check_stop_conditions(Some(limit), None, Duration::from_secs(59), 0),
            None,
            "below the deadline: no breach"
        );
        assert_eq!(
            check_stop_conditions(Some(limit), None, Duration::from_secs(60), 0),
            Some(StopReason::Deadline),
            "at the deadline: breach"
        );
        assert_eq!(
            check_stop_conditions(Some(limit), None, Duration::from_secs(61), 0),
            Some(StopReason::Deadline),
            "past the deadline: breach"
        );
    }

    #[test]
    fn check_stop_conditions_token_budget_at_below_and_above_the_limit() {
        assert_eq!(
            check_stop_conditions(None, Some(1_000), Duration::ZERO, 999),
            None,
            "below the token budget: no breach"
        );
        assert_eq!(
            check_stop_conditions(None, Some(1_000), Duration::ZERO, 1_000),
            Some(StopReason::TokenBudget),
            "at the token budget: breach"
        );
        assert_eq!(
            check_stop_conditions(None, Some(1_000), Duration::ZERO, 1_001),
            Some(StopReason::TokenBudget),
            "past the token budget: breach"
        );
    }

    #[test]
    fn check_stop_conditions_reports_deadline_when_both_limits_breach() {
        // Both the deadline and the token budget are breached here. The
        // reported reason must be deterministic — always the same one for
        // the same inputs — regardless of which limit a future reader might
        // expect to "win".
        assert_eq!(
            check_stop_conditions(
                Some(Duration::from_secs(1)),
                Some(1),
                Duration::from_secs(2),
                2
            ),
            Some(StopReason::Deadline),
            "deadline is checked first and reported when both breach"
        );
    }

    // -----------------------------------------------------------------------
    // KYO-345: non-iteration stop conditions — scripted-provider integration
    // -----------------------------------------------------------------------

    /// Token usage that contributes a fixed, known amount to
    /// `max_total_tokens`'s accumulator (`input_tokens +
    /// cache_creation_input_tokens + output_tokens`).
    fn billable_usage(tokens: u32) -> AgentTokenUsage {
        AgentTokenUsage {
            input_tokens: tokens,
            output_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            reasoning_tokens: 0,
        }
    }

    #[tokio::test]
    async fn token_ceiling_short_circuits_the_iteration_ceiling() {
        // Each iteration contributes 100_000 billable tokens; the ceiling is
        // 250_000, so the breach fires at the top of the 4th iteration
        // (after 3 iterations have run: 0, 100k, 200k, 300k >= 250k). The
        // script has exactly 3 entries — one per iteration expected to run
        // — so the queue is empty by the wrap-up call and it correctly
        // falls back to `after_script` rather than consuming a 4th scripted
        // tool call (which would make the wrap-up call itself request a
        // tool and return empty content instead of the wrap-up text).
        let script = vec![Reply::ToolCallWithUsage(billable_usage(100_000)); 3];
        let (mut agent, calls) = scripted_agent_with_config(
            AgentConfig {
                max_iterations: 25,
                max_duration: None,
                max_total_tokens: Some(250_000),
                ..Default::default()
            },
            script,
            Reply::Text("Summary from partial data.".to_string()),
        )
        .await;

        let answer = agent
            .chat("hello", CancellationToken::new(), None, None, None)
            .await
            .expect("chat should complete, not error");

        let recorded = std::mem::take(&mut *calls.lock().expect("call log mutex"));
        assert_eq!(
            recorded.len(),
            4,
            "3 tool-calling iterations (accumulating 300_000 >= 250_000 ceiling) \
             plus the wrap-up call — must stop well short of the 25-iteration \
             ceiling: {recorded:?}"
        );
        assert!(
            recorded.last().expect("has calls").tools.is_empty(),
            "the final call must carry no tools"
        );
        assert_eq!(
            recorded
                .last()
                .expect("has calls")
                .messages
                .last()
                .expect("wrap-up call has messages")
                .content,
            WRAP_UP_INSTRUCTION,
            "the final call must carry the wrap-up instruction at its tail"
        );
        assert_eq!(
            answer, "Summary from partial data.",
            "chat() must return the wrap-up's substantive content, not an error \
             or a canned apology"
        );
    }

    #[tokio::test]
    async fn deadline_short_circuits_the_iteration_ceiling() {
        // The script has exactly 1 entry, matching the 1 iteration expected
        // to run before the deadline breach — see the token-ceiling test
        // above for why an oversized script would corrupt the wrap-up call.
        let script = vec![Reply::SlowToolCall(Duration::from_secs(400))];
        let (mut agent, calls) = scripted_agent_with_config(
            AgentConfig {
                max_iterations: 25,
                max_duration: Some(Duration::from_secs(300)),
                max_total_tokens: None,
                ..Default::default()
            },
            script,
            Reply::Text("Summary before the deadline.".to_string()),
        )
        .await;

        // Pause the clock only now, after real setup (the in-memory SQLite
        // pool `scripted_agent_with_config` connects above) has completed
        // using real time. Pausing from the start via `#[tokio::test(
        // start_paused = true)]` races the virtual clock's auto-advance
        // against sqlx's `spawn_blocking` connect work and loses —
        // `PoolTimedOut` — because the executor sees no runnable task and
        // fast-forwards to the next timer deadline before the blocking
        // thread reports back. Pausing here, and sleeping only inside the
        // scripted provider below, avoids that race entirely; the deadline
        // check is still driven by a virtual clock, not a real sleep.
        tokio::time::pause();

        let answer = agent
            .chat("hello", CancellationToken::new(), None, None, None)
            .await
            .expect("chat should complete, not error");

        let recorded = std::mem::take(&mut *calls.lock().expect("call log mutex"));
        assert_eq!(
            recorded.len(),
            2,
            "1 slow tool-calling iteration (elapsed ~400s >= 300s deadline) plus \
             the wrap-up call — must stop well short of the 25-iteration \
             ceiling: {recorded:?}"
        );
        assert!(
            recorded.last().expect("has calls").tools.is_empty(),
            "the final call must carry no tools"
        );
        assert_eq!(
            recorded
                .last()
                .expect("has calls")
                .messages
                .last()
                .expect("wrap-up call has messages")
                .content,
            WRAP_UP_INSTRUCTION,
            "the final call must carry the wrap-up instruction at its tail"
        );
        assert_eq!(
            answer, "Summary before the deadline.",
            "chat() must return the wrap-up's substantive content, not an error \
             or a canned apology"
        );
    }

    #[tokio::test]
    async fn cache_reads_do_not_count_toward_the_token_ceiling() {
        // Every iteration reports 500_000 cache-read tokens and nothing
        // else. If cache reads counted toward `max_total_tokens`, a ceiling
        // of 100_000 would breach after the very first iteration. Because
        // they must not count, the accumulator stays at 0 for the whole
        // run and the loop runs to its full 5-iteration ceiling.
        let cache_heavy = AgentTokenUsage {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 500_000,
            reasoning_tokens: 0,
        };
        let script = vec![Reply::ToolCallWithUsage(cache_heavy); 5];
        let (mut agent, calls) = scripted_agent_with_config(
            AgentConfig {
                max_iterations: 5,
                max_duration: None,
                max_total_tokens: Some(100_000),
                ..Default::default()
            },
            script,
            Reply::Text("Ran to completion.".to_string()),
        )
        .await;

        let answer = agent
            .chat("hello", CancellationToken::new(), None, None, None)
            .await
            .expect("chat should complete, not error");

        let recorded = std::mem::take(&mut *calls.lock().expect("call log mutex"));
        assert_eq!(
            recorded.len(),
            6,
            "5 loop iterations plus the wrap-up call — cache-read tokens must \
             never count toward max_total_tokens, or this would have stopped \
             after 1 iteration: {recorded:?}"
        );
        assert_eq!(answer, "Ran to completion.");
    }
}
