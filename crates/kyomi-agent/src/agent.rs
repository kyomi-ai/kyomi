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
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
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
    if error.contains("invalid YAML") || error.contains(" yaml ") {
        "yaml_parse"
    } else if error.contains("missing required key") {
        "missing_key"
    } else if error.contains("SQL error") || error.contains("sql error") || error.contains(" sql ") {
        "sql_error"
    } else if error.contains(" schema ") || error.contains(" conversion ") || error.contains(" schema_initialization ") {
        "schema_error"
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
/// **This is not user-facing text on its own.** Exactly one prefix is
/// prepended before it reaches a user, by `classify_agent_failure`
/// (`crates/kyomi-agent/src/execution.rs`): `"I encountered an error while
/// processing your request: "`. The `internal: ` variant tag that
/// `Error::Internal`'s `Display` also carries is *not* part of that prefix —
/// [`kyomi_core::Error::user_message`] strips it before the chat wrapper ever
/// sees this string (KYO-350). So this constant must still read as a
/// lowercase, non-apologising *clause*, not a standalone sentence addressed
/// to the user — what the user reads is `"I encountered an error while
/// processing your request: "` immediately followed by this string, one
/// sentence, no `internal:` in the middle.
///
/// It must also avoid the substring `cancelled`. That check still runs
/// against `Display` (`error.to_string()` in `classify_agent_failure`), not
/// `user_message()` — for `Error::Internal` specifically, `Display` is
/// `"internal: "` plus this string, so the substring constraint is unchanged
/// by the KYO-350 fix even though the *rendered* user copy no longer carries
/// that prefix.
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

/// How many times one run resends a `pause_turn` assistant message before
/// giving up and answering with what it has.
const MAX_PAUSE_CONTINUATIONS: u32 = 8;

/// Strict budget: stop a turn after this many failed tool calls, counted since
/// the last success, that are NOT repairable rejections (see
/// [`is_repairable_rejection`]). That is every unknown-outcome failure (the call
/// may have had an effect) and every rejection that happens before a tool runs
/// (an unknown tool name, unparsable or truncated arguments), which gives the
/// model nothing to repair incrementally. Tool failures are returned to the
/// model so it can repair; this bound prevents an unbounded error loop. Counted
/// separately from [`MAX_CONSECUTIVE_REJECTED_TOOL_FAILURES`]: neither counter
/// resets the other, only a success resets both.
const MAX_CONSECUTIVE_TOOL_FAILURES: usize = 5;

/// Repairable budget: stop a turn after this many repairable rejections, counted
/// since the last success. A repairable rejection (see
/// [`is_repairable_rejection`]) is a definite, effect-free rejection returned by
/// a real tool that ran with parsed arguments (for example a SQL analysis
/// error). Fixing a hard query legitimately takes several such repairs, each
/// with a different call, so this budget is larger than
/// [`MAX_CONSECUTIVE_TOOL_FAILURES`]. Counted independently of it: a failure of
/// the other class does not reset this count, so a turn can run up to 4 strict
/// plus 9 repairable failures before either cap trips. A genuinely stuck model
/// repeating the same call is still caught by [`MAX_SAME_FAILING_TOOL_CALLS`],
/// and the turn's iteration and duration budgets still apply.
const MAX_CONSECUTIVE_REJECTED_TOOL_FAILURES: usize = 10;

/// Stop a turn when the same tool call (name + arguments) fails this many times,
/// even when other successful calls reset the general failure count. A success
/// of the same call clears its count.
const MAX_SAME_FAILING_TOOL_CALLS: usize = 3;

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

/// Callback fired at each agent-loop iteration boundary (KYO-493 phase 3),
/// with the full message history so far (`&self.state.messages`).
///
/// Unlike the callbacks above, this one is awaited directly and in-line in
/// the loop — never a detached `tokio::spawn` — so invocations are strictly
/// sequential with no risk of the out-of-order completion the other
/// callbacks' fire-and-forget spawns accept (see
/// `ChatAgentAdapter::set_thinking_tracker`'s doc). That's what lets the
/// adapter track a persisted-up-to index safely: a callback body can commit
/// to "everything before index N is durably written" without a concurrent,
/// still-running earlier call being able to invalidate it.
///
/// The `for<'a>` HRTB lets the closure borrow the messages slice rather
/// than requiring the caller to clone the whole history every iteration —
/// the returned future is free to hold that borrow across its own awaits,
/// same as `async-trait`'s desugaring.
pub(crate) type IterationBoundaryCallback =
    Box<dyn for<'a> Fn(&'a [Message]) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> + Send + Sync>;

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
    /// Sampling temperature, passed through as configured. `None` omits the field.
    pub temperature: Option<serde_json::Value>,
    /// Reasoning effort passed to the provider unchanged. `None` omits it.
    pub reasoning_effort: Option<String>,
    /// Maximum tokens to generate per LLM call.
    pub max_tokens: u32,
    /// Provider-executed web tools (KYO-969). Which of them a turn sends is decided
    /// by [`CustomAgent::call_options`].
    pub server_tools: Vec<crate::web::server_tools::ServerTool>,
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

impl AgentConfig {
    /// The per-request generation options this config asks providers for. Does NOT
    /// include `server_tools`: those are decided per turn by
    /// [`CustomAgent::call_options`].
    pub fn request_options(&self) -> crate::provider::RequestOptions {
        crate::provider::RequestOptions::new(self.max_tokens)
            .with_temperature(self.temperature.clone())
            .with_reasoning_effort(self.reasoning_effort.clone())
    }
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
            reasoning_effort: None,
            max_tokens: 4096,
            server_tools: Vec::new(),
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
pub(crate) enum AgentProgress {
    Thinking(String),
    Usage(u32, u32, Option<f64>),
    /// Usage of an auxiliary call (the compaction summary) that belongs to the
    /// run's reported totals but not its conversation context size (KYO-975).
    BackgroundUsage(u32, u32, Option<f64>),
    ToolStart(String, serde_json::Value),
    ToolEnd(String, String, bool),
    Preparing,
}
pub(crate) type ProgressCallback = Box<dyn Fn(AgentProgress) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

#[derive(Default)]
pub struct AgentCallbacks {
    pub(crate) on_progress: Option<ProgressCallback>,
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
    /// Called at each agent-loop iteration boundary — after a tool-call
    /// round's results are all appended to `state.messages`, before the
    /// next LLM call — with the full message history so far (KYO-493 phase
    /// 3). See [`IterationBoundaryCallback`] for why this one is awaited
    /// in-line rather than spawned like the callbacks above.
    pub on_iteration_boundary: Option<IterationBoundaryCallback>,
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
    runtime: Option<(agent_runtime::ExecutionContext, Arc<dyn agent_runtime::EventSink>)>,
    /// Tool registry (shared, immutable after construction).
    registry: Arc<ToolRegistry>,
    /// Context passed to tool execution.
    tool_context: ToolContext,
    /// Map of user_id -> display name for message attribution.
    user_names: HashMap<String, String>,
    /// Sources the provider's web tools used during this run (normalised);
    /// shown to the user as a footer on the final answer.
    web_sources: Vec<crate::web::WebSource>,
    /// Calls the previous response announced without a result yet. The next
    /// response returns their results without repeating the `server_tool_use`
    /// block, so the parser pairs them with these.
    pending_server_calls: Vec<crate::web::server_tools::ServerToolCall>,
    /// Provider-reported web calls used so far in this run, per tool: the per-turn
    /// allowance is spent across every request of the run, not per request.
    web_usage: HashMap<kyomi_core::web_tools::WebToolKind, u32>,
    /// Which of the configured provider tools this turn may use (indices into
    /// `config.server_tools`), decided once when the turn's first request is built:
    /// the monthly budget is checked then and not mid-turn.
    turn_server_tools: tokio::sync::OnceCell<Vec<usize>>,
    /// The provider tools the most recent request carried: a `pause_turn`
    /// continuation resends exactly these.
    last_server_tools: std::sync::Mutex<Vec<serde_json::Value>>,
    /// Where the open chain of `pause_turn` messages starts in the state (`None`
    /// outside a chain).
    pause_chain_start: Option<usize>,
    /// Compaction for this run: the resolved threshold and the fast model that
    /// writes the summary (KYO-975). `None` when the caller did not configure
    /// it (batch watch runs) or the run has no summariser.
    compaction: Option<crate::compaction::CompactionRuntime>,
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
            runtime: None,
            registry,
            tool_context,
            user_names,
            web_sources: Vec::new(),
            pending_server_calls: Vec::new(),
            web_usage: HashMap::new(),
            turn_server_tools: tokio::sync::OnceCell::new(),
            last_server_tools: std::sync::Mutex::new(Vec::new()),
            pause_chain_start: None,
            compaction: None,
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

    /// Give this run its compaction threshold and fast model (KYO-975). The
    /// caller resolves the threshold from the provider's context window and the
    /// instance/workspace settings, and builds the summariser from the title
    /// model. Without this, the agent never compacts.
    pub fn set_compaction(&mut self, compaction: crate::compaction::CompactionRuntime) {
        self.compaction = Some(compaction);
    }

    /// Read-only access to the agent configuration.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Mutable access to the callbacks (for setting up streaming).
    pub fn callbacks_mut(&mut self) -> &mut AgentCallbacks {
        &mut self.callbacks
    }

    pub(crate) fn set_runtime(&mut self, context: agent_runtime::ExecutionContext, sink: Arc<dyn agent_runtime::EventSink>) {
        self.runtime = Some((context, sink));
    }
    async fn progress(&self, progress: AgentProgress) {
        if let Some(callback) = &self.callbacks.on_progress { callback(progress).await; }
    }

    async fn record_validation(&self, passed: bool, diagnostic: &str) -> kyomi_core::Result<()> {
        if let Some((context, sink)) = &self.runtime {
            let key = format!("model-{}:validation", self.state.global_iteration);
            let (text, detail) = context.text(&key, diagnostic, 8000);
            sink.commit(&context.command(&key, agent_runtime::Payload::Public(agent_runtime::PublicPayload::Validation { passed, text }), detail))
                .await.map_err(runtime_error)?;
        }
        Ok(())
    }

    async fn preparing(&self) -> kyomi_core::Result<()> {
        if let Some((context, sink)) = &self.runtime {
            let key = format!("model-{}:preparing", self.state.global_iteration);
            let (text, detail) = context.text(&key, "Preparing response", 200);
            sink.commit(&context.command(&key, agent_runtime::Payload::Public(agent_runtime::PublicPayload::Planning { text }), detail))
                .await.map_err(runtime_error)?;
        } else {
            self.progress(AgentProgress::Preparing).await;
        }
        Ok(())
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
        self.pause_chain_start = None;
        self.web_sources.clear();
        self.pending_server_calls.clear();
        // A restored input count belongs to the previous turn (and possibly a
        // different model after a model switch), so it cannot drive this run's
        // threshold decisions; the context estimate covers the first check.
        self.state.last_input_tokens = 0;
        let result = self.run_chat(user_message, cancel_token, current_time_user_tz, message_source, user_id).await;
        // A failed run keeps none of an open `pause_turn` chain (blocks that end in a
        // call without its result are rejected by the provider on every later turn);
        // everything else the run recorded stays, as for any failed run.
        if result.is_err()
            && let Some(start) = self.pause_chain_start.take()
        {
            self.state.messages.truncate(start);
        }
        result
    }

    async fn run_chat(
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
        self.turn_server_tools = tokio::sync::OnceCell::new();

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
        let mut pause_continuations: u32 = 0;
        // Whether the previous response was a `pause_turn` (so this request continues it).
        let mut last_was_pause = false;
        // Which condition ended the loop. Overwritten if a non-iteration
        // guard fires; left at `Iterations` if the loop runs to completion.
        let mut stop_reason = StopReason::Iterations;

        // Bounded error-loop guard: tool failures are returned to the model,
        // but repeated failures must not run forever.
        let mut consecutive_tool_failures = 0usize;
        let mut consecutive_rejected_failures = 0usize;
        let mut same_call_failures: HashMap<String, (usize, String)> = HashMap::new();
        let mut last_tool_error = String::new();

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
            let (mut llm_messages, compaction_tokens) =
                self.maybe_compact(&cancel_token).await?;
            accumulated_tokens += compaction_tokens;
            // A `pause_turn` continuation must end with the paused assistant message
            // exactly as received: nothing may follow it.
            let pausing = last_was_pause;
            if !pausing {
                llm_messages.extend(chartml_retry_messages.iter().cloned());
                // Budget notice last: maximum recency, immediately before the call.
                if let Some(ref notice) = budget_notice {
                    llm_messages.push(notice.clone());
                }
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
            let call_started = std::time::Instant::now();
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

            // Provider-executed web tools (KYO-969): usage, sources, verbatim blocks.
            let server_activity = self.note_server_tools(&response, call_started.elapsed()).await;

            // Usage and cost were already accounted for inside `call_llm`, at the
            // moment the provider returned (KYO-974); only loop state is kept here.
            self.state.last_input_tokens = response.usage.input_tokens
                + response.usage.cache_creation_input_tokens
                + response.usage.cache_read_input_tokens;

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
            if self.runtime.is_none() && let Some(thinking) = &response.thinking_content {
                self.progress(AgentProgress::Thinking(thinking.clone())).await;
            }

            // `pause_turn`: the provider paused a long server-tool turn. Resend the
            // paused assistant message unchanged (its blocks verbatim) to continue;
            // the request carries the same tools as the turn's first request. The
            // paused messages and the final one are one logical answer and are
            // stored exactly as received. A turn still paused after the bound fails
            // like any other failed run: nothing of its paused chain is kept.
            if response.tool_calls.is_none() && response.finish_reason == "pause_turn" {
                if pause_continuations >= MAX_PAUSE_CONTINUATIONS {
                    return Err(kyomi_core::Error::Internal(format!(
                        "Web research did not finish: the provider paused it {MAX_PAUSE_CONTINUATIONS} times. Try a narrower question."
                    )));
                }
                pause_continuations += 1;
                self.pause_chain_start.get_or_insert(self.state.messages.len());
                self.push_assistant(
                    Message::assistant(&response.content).with_provider_content(server_activity.provider_content.clone()),
                );
                last_was_pause = true;
                continue;
            }
            last_was_pause = false;

            // No tool calls -- this is the final response.
            if response.tool_calls.is_none() {
                let content = final_response_text(response);

                if let Some(ref cb) = self.callbacks.on_preparing_response {
                    cb();
                }
                self.preparing().await?;

                // Validate ChartML blocks if present (YAML + SQL dry-run).
                if has_chartml_blocks(&content)
                    && let Some(error_msg) = self.validate_chartml_blocks(&content).await
                {
                    self.record_validation(false, &error_msg).await?;
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
                    if self.pause_chain_start.is_some() {
                        // The failed answer's blocks resolve the chain's open call, so
                        // they join the chain's stored blocks (only its text is ephemeral).
                        self.push_assistant(
                            Message::assistant("").with_provider_content(server_activity.provider_content.clone()),
                        );
                        chartml_retry_messages.push(Message::assistant(content));
                    } else {
                        // The failed answer keeps its provider blocks on the ephemeral message.
                        chartml_retry_messages.push(
                            Message::assistant(content).with_provider_content(server_activity.provider_content.clone()),
                        );
                    }
                    chartml_retry_messages.push(Message::user(format!(
                        "\u{1f916} SYSTEM: Automatic ChartML validation failed. The user has NOT seen your response yet. \
                         Please fix the following errors and then repeat your FULL response:\n\n{error_msg}"
                    )));
                    continue;
                }

                if has_chartml_blocks(&content) { self.record_validation(true, "Response schema and SQL validation passed").await?; }
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

                // Show the sources the provider's web tools used (provider terms
                // require citations to be displayed).
                let content = self.with_web_sources(content);

                // Push final response to state so persist_after_chat saves it. A
                // server-tool turn keeps the provider's own blocks (encrypted_content
                // and citations' encrypted_index included) so the next user turn can
                // send them back unchanged; the displayed text carries the sources
                // footer, the stored blocks do not.
                self.pause_chain_start = None;
                self.push_assistant(
                    Message::assistant(&content).with_provider_content(server_activity.provider_content.clone()),
                );
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
            if self.runtime.is_none() && !response.content.is_empty() {
                self.progress(AgentProgress::Thinking(response.content.clone())).await;
            }

            // Add assistant message with tool calls.
            // Safety: guarded by `response.tool_calls.is_none()` early-return above.
            let tool_calls = response.tool_calls.expect("guarded by is_none check above");
            self.pause_chain_start = None;
            self.push_assistant(
                Message::assistant_with_tool_calls(
                    if self.runtime.is_some() && has_chartml_blocks(&response.content) { String::new() } else { response.content.clone() },
                    tool_calls.clone(),
                )
                .with_provider_content(server_activity.provider_content.clone()),
            );

            // Complete compatibility transcript is visible before the first delayed tool.
            if self.runtime.is_some() && let Some(callback) = &self.callbacks.on_iteration_boundary {
                callback(&self.state.messages).await;
            }
            // Check cancellation before tool execution.
            if cancel_token.is_cancelled() {
                return Err(kyomi_core::Error::Internal("Request cancelled".into()));
            }

            // Execute each tool call.
            for tool_call in &tool_calls {
                if cancel_token.is_cancelled() {
                    return Err(kyomi_core::Error::Internal("Request cancelled".into()));
                }
                // KYO-535: a provider marks `arguments_error` when it could
                // not parse this call's arguments — almost always a
                // `max_tokens` cutoff mid-payload. Do NOT execute the tool
                // against the `{}` placeholder: the tool would report a
                // generic "missing required parameter", which reads to the
                // model as "I forgot a field" and provokes a retry of the
                // *identical* call. (Production incident: four identical
                // retries, ~50s of generation each, until the copilot's
                // 3-minute `max_duration` guard tripped.)
                //
                // The turn itself must NOT be short-circuited here, though —
                // a retry with a *smaller* payload is a legitimate recovery,
                // and the model can only take it if the loop keeps going.
                // So: skip execution, tell the model the truth (truncated,
                // not malformed; send less), and `continue` the `for` loop
                // exactly like a normal tool result — the outer `while` loop
                // then gives the model another iteration to retry smaller.
                let outcome = if let Some((context, sink)) = &self.runtime {
                    let tool = RuntimeTool { agent: self, call: tool_call, finish_reason: &response.finish_reason };
                    context.tool(sink.as_ref(), &tool,
                        agent_runtime::ToolCallId(context.identity("tool", &format!("call-{}-{}", self.state.global_iteration, tool_call.id))),
                        tool_call.name.clone(), tool_call.arguments.clone()).await.map_err(runtime_error)?
                } else if tool_call.arguments_error.is_some() {
                    tool_outcome(self, tool_call, &response.finish_reason).await
                } else {
                    self.execute_tool(tool_call).await
                };
                let succeeded = outcome.succeeded();
                let rejected = is_repairable_rejection(tool_call, &outcome);
                let result = outcome.text;
                self.state.messages.push(Message::tool_result(
                    &tool_call.id,
                    &tool_call.name,
                    &result,
                ));
                if self.runtime.is_some() && let Some(callback) = &self.callbacks.on_iteration_boundary {
                    callback(&self.state.messages).await;
                }
                if succeeded {
                    consecutive_tool_failures = 0;
                    consecutive_rejected_failures = 0;
                    // A success clears this call's repeat count; other calls'
                    // counts survive so alternating failures cannot evade the
                    // same-call guard.
                    same_call_failures.remove(&tool_call_fingerprint(tool_call));
                } else {
                    if rejected {
                        consecutive_rejected_failures += 1;
                    } else {
                        consecutive_tool_failures += 1;
                    }
                    last_tool_error = result.clone();
                    let entry = same_call_failures
                        .entry(tool_call_fingerprint(tool_call))
                        .or_insert_with(|| (0, String::new()));
                    entry.0 += 1;
                    entry.1 = result.clone();
                }
            }

            // Check cancellation after tool execution.
            if cancel_token.is_cancelled() {
                return Err(kyomi_core::Error::Internal("Request cancelled".into()));
            }

            // Iteration boundary (KYO-493 phase 3): the assistant
            // tool-calls message and every tool-result message this
            // iteration produced are now in `state.messages`. Fire before
            // the ChartML check below, which can return early with a
            // terminal assistant message of its own — that message must
            // never be visible to this callback (it isn't durable yet;
            // `persist_after_chat`/`execute_agent_chat`'s step 14 own it).
            if let Some(ref cb) = self.callbacks.on_iteration_boundary {
                cb(&self.state.messages).await;
            }

            // Bounded error-loop guard: checked after the whole tool batch so
            // every assistant tool call in this response has a matching tool
            // result, even when the guard ends the turn.
            let (repeated_call_failures, repeated_call_error) = same_call_failures
                .values()
                .max_by_key(|(count, _)| *count)
                .map(|(count, error)| (*count, error.as_str()))
                .unwrap_or((0, ""));
            if consecutive_tool_failures >= MAX_CONSECUTIVE_TOOL_FAILURES
                || consecutive_rejected_failures >= MAX_CONSECUTIVE_REJECTED_TOOL_FAILURES
                || repeated_call_failures >= MAX_SAME_FAILING_TOOL_CALLS
            {
                let guard_error = if repeated_call_failures >= MAX_SAME_FAILING_TOOL_CALLS {
                    repeated_call_error
                } else {
                    last_tool_error.as_str()
                };
                let message = tool_failure_guard_message(
                    consecutive_tool_failures + consecutive_rejected_failures,
                    repeated_call_failures,
                    guard_error,
                );
                if let Some(ref cb) = self.callbacks.on_preparing_response {
                    cb();
                }
                self.preparing().await?;
                self.state.messages.push(Message::assistant(&message));
                return Ok(message);
            }

            // Check if assistant content has ChartML blocks -> validate (YAML + SQL) -> return if valid.
            if has_chartml_blocks(&response.content) {
                if let Some(error_msg) = self.validate_chartml_blocks(&response.content).await {
                    // Validation failed — store error as ephemeral retry context.
                    // The assistant message (with tool calls) is already persisted
                    // above, but the error instruction stays ephemeral.
                    self.record_validation(false, &error_msg).await?;
                    warn!(error = %error_msg, "ChartML validation failed in tool response, asking LLM to fix");
                    // Log validation failure for prompt-tuning analysis.
                    self.log_chartml_validation_error(
                        &response.content,
                        &error_msg,
                        chartml_retry_messages.len() as i32,
                        "chat",
                    )
                    .await;
                    if self.runtime.is_some() { chartml_retry_messages.push(Message::assistant(response.content.clone())); }
                    chartml_retry_messages.push(Message::user(format!(
                        "\u{1f916} SYSTEM: Automatic ChartML validation failed. The user has NOT seen your response yet. \
                         Please fix the following errors and then repeat your FULL response:\n\n{error_msg}"
                    )));
                    continue;
                }

                self.record_validation(true, "Response schema and SQL validation passed").await?;
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
                self.preparing().await?;
                let content = self.with_web_sources(response.content);
                self.state.messages.push(Message::assistant(&content));
                return Ok(content);
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

        // A run that ends inside a `pause_turn` chain did not finish its research.
        if self.pause_chain_start.is_some() {
            return Err(kyomi_core::Error::Internal(
                "Web research did not finish before the request's limits were reached. Try a narrower question.".into(),
            ));
        }

        // Cancellation is checked before every other LLM call in this loop;
        // the wrap-up call is no exception.
        if cancel_token.is_cancelled() {
            return Err(kyomi_core::Error::Internal("Request cancelled".into()));
        }
        // The wrap-up is another provider call, so it gets the same compaction
        // decision as every loop iteration (KYO-975). Its own summary tokens
        // cannot affect a later guard (the wrap-up is the final call).
        let (mut wrap_up_messages, _) = self.maybe_compact(&cancel_token).await?;
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
        self.preparing().await?;

        // The wrap-up is an LLM call like any other, so it counts.
        self.state.global_iteration += 1;

        let wrap_up_started = std::time::Instant::now();
        let wrap_up = tokio::select! {
            result = self.call_llm_with(&wrap_up_messages, &[], false) => result,
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

        // The wrap-up can carry the result of a call deferred before the budget
        // ran out. Record it exactly like a loop response: usage, card, sources
        // and the verbatim blocks (KYO-979).
        let server_activity = self.note_server_tools(&response, wrap_up_started.elapsed()).await;

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
                    self.record_validation(false, &error_msg).await?;
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
                None => {
                    self.record_validation(true, "Response schema and SQL validation passed").await?;
                    content
                },
            }
        } else {
            content
        };

        let content = self.with_web_sources(content);
        self.state.messages.push(Message::assistant(&content).with_provider_content(server_activity.provider_content));
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
        self.call_llm_with(messages, tools, true).await
    }

    /// `allow_server_tools = false` is for calls that must not use any tool (the
    /// forced wrap-up): provider web tools are then not sent. The history is sent
    /// as stored, server-tool blocks included.
    async fn call_llm_with(
        &self,
        messages: &[Message],
        tools: &[Tool],
        allow_server_tools: bool,
    ) -> kyomi_core::Result<crate::types::LLMResponse> {
        let provider = RuntimeProvider { agent: self, messages, tools, allow_server_tools };
        if let Some((context, sink)) = &self.runtime {
            context.complete(&provider, sink.as_ref(), &agent_runtime::ModelCallId(format!("model-{}", self.state.global_iteration)))
                .await.map_err(runtime_error)
        } else {
            let options = self.call_options(allow_server_tools).await;
            let response = self.client.complete(messages, tools, &options, &self.user_names).await?;
            self.account_usage(&response).await;
            Ok(response)
        }
    }

    /// Account for one model call the moment the provider returns it, before any
    /// cancellation check or persistence step can drop a response that was paid
    /// for (KYO-974). Every call goes through here: loop turns and the
    /// max-iterations wrap-up, on both the legacy and the durable path. Feeds the
    /// usage callback and, on the legacy path, the thinking tracker (the durable
    /// path's tracker is fed by the committed usage event). Billing itself is the
    /// metered client's job: it records every response inside the provider call.
    async fn account_usage(&self, response: &crate::types::LLMResponse) {
        let total_input = response.usage.input_tokens
            + response.usage.cache_creation_input_tokens
            + response.usage.cache_read_input_tokens;
        if self.runtime.is_none() {
            self.progress(AgentProgress::Usage(total_input, response.usage.output_tokens, response.cost)).await;
        }
        if let Some(ref cb) = self.callbacks.on_token_usage {
            cb(total_input, response.usage.output_tokens, response.cost);
        }
    }

    /// The request options for the next LLM call.
    ///
    /// Provider-executed web tools. A `pause_turn` continuation resends exactly the
    /// tools of the request it continues (the provider rejects a continuation
    /// without its tool). Any other request offers the tools the turn may use, each
    /// with the per-turn allowance still left as `max_uses`: the monthly budget is
    /// checked once, when the turn's first request is built, never mid-turn. A tool
    /// whose allowance is used up is left out of a request that is not a
    /// continuation. `allow_server_tools = false` (the forced wrap-up call) sends
    /// none. The history is sent exactly as stored, stored server-tool blocks
    /// included, whichever tools are offered.
    async fn call_options(&self, allow_server_tools: bool) -> crate::provider::RequestOptions {
        let mut options = self.config.request_options();
        if allow_server_tools {
            options.server_tools = if self.pause_chain_start.is_some() {
                self.last_server_tools.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
            } else {
                self.turn_request_server_tools().await
            };
        }
        *self.last_server_tools.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = options.server_tools.clone();
        options
    }

    /// The provider tools a non-continuation request of this turn carries.
    async fn turn_request_server_tools(&self) -> Vec<serde_json::Value> {
        let allowed = self.turn_server_tools.get_or_init(|| self.turn_start_server_tools()).await;
        if allowed.is_empty() {
            return Vec::new();
        }
        let caps = &self.tool_context.config.web_tools.caps;
        allowed
            .iter()
            .filter_map(|&index| {
                let tool = &self.config.server_tools[index];
                let per_turn = match tool.kind {
                    kyomi_core::web_tools::WebToolKind::Search => caps.per_turn_search,
                    kyomi_core::web_tools::WebToolKind::Fetch => caps.per_turn_fetch,
                };
                let remaining = per_turn.saturating_sub(self.web_usage.get(&tool.kind).copied().unwrap_or(0));
                (remaining > 0).then(|| tool.with_max_uses(remaining))
            })
            .collect()
    }

    /// The options of a request of the current turn, or (`new_turn`) of the first
    /// request of a new one.
    #[cfg(test)]
    async fn request_options_for_call(&mut self, new_turn: bool) -> crate::provider::RequestOptions {
        if new_turn {
            self.turn_server_tools = tokio::sync::OnceCell::new();
        }
        self.call_options(true).await
    }

    /// The configured provider tools this turn may use (indices into
    /// `config.server_tools`), decided at the start of the turn: the monthly budget
    /// and the per-turn allowance. Each tool left out is logged once.
    async fn turn_start_server_tools(&self) -> Vec<usize> {
        let ctx = &self.tool_context;
        let caps = &ctx.config.web_tools.caps;
        let mut allowed = Vec::new();
        for (index, tool) in self.config.server_tools.iter().enumerate() {
            let per_turn = match tool.kind {
                kyomi_core::web_tools::WebToolKind::Search => caps.per_turn_search,
                kyomi_core::web_tools::WebToolKind::Fetch => caps.per_turn_fetch,
            };
            let remaining = per_turn.saturating_sub(self.web_usage.get(&tool.kind).copied().unwrap_or(0));
            let refusal = if remaining == 0 {
                Some(crate::web::caps::CapRefusal::PerTurn.to_string())
            } else {
                match crate::web::caps::check_monthly(&ctx.kv, caps, &ctx.workspace_id, tool.kind).await {
                    Ok(Ok(())) => None,
                    Ok(Err(r)) => Some(r.to_string()),
                    Err(e) => Some(format!("budget check failed: {e}")),
                }
            };
            match refusal {
                None => allowed.push(index),
                Some(reason) => warn!(tool = tool.kind.tool_name(), reason = %reason, "provider web tool withheld from this turn"),
            }
        }
        allowed
    }

    /// Append an assistant message to the state. A bare `pause_turn` message (blocks
    /// only: no text, no tool calls) is never stored as a row of its own; its
    /// blocks are carried, in order, into the next assistant message, so the whole
    /// logical answer is stored exactly as the provider returned it.
    fn push_assistant(&mut self, mut message: Message) {
        let is_bare_pause = |m: &Message| {
            m.role == crate::types::MessageRole::Assistant
                && m.content.is_empty()
                && m.tool_calls.as_ref().is_none_or(|calls| calls.is_empty())
                && m.provider_content.is_some()
        };
        if self.state.messages.last().is_some_and(is_bare_pause) {
            let carried = self.state.messages.pop().and_then(|m| m.provider_content).unwrap_or_default();
            let own = message.provider_content.take().unwrap_or_default();
            message.provider_content = Some(carried.into_iter().chain(own).collect());
        }
        self.state.messages.push(message);
    }

    /// Append the sources the provider's web tools used in this run to a final
    /// answer (provider terms require citations to be displayed) and clear them.
    fn with_web_sources(&mut self, content: String) -> String {
        let sources = crate::web::server_tools::dedupe(std::mem::take(&mut self.web_sources));
        crate::web::server_tools::append_footer(&content, &crate::web::server_tools::sources_footer(&sources))
    }

    /// Record what the provider's web tools did in one response: log it, feed
    /// the monthly budget from the provider-reported usage, and remember the
    /// sources for the answer footer. Returns the parsed activity.
    async fn note_server_tools(&mut self, response: &crate::types::LLMResponse, latency: Duration) -> crate::web::server_tools::ServerToolActivity {
        if self.config.server_tools.is_empty() {
            return Default::default();
        }
        let pending = std::mem::take(&mut self.pending_server_calls);
        let activity = match self.config.server_tools.first().map(|t| t.backend) {
            Some(kyomi_core::web_tools::WebBackendKind::OpenRouter) => crate::web::server_tools::parse_openrouter(&response.raw_response),
            _ => crate::web::server_tools::parse_anthropic_with_pending(&response.raw_response, &pending),
        };
        // A server call the provider announced without a result is returned by
        // the next response (its continuation) without repeating the
        // `server_tool_use` block. That happens when the turn paused
        // (`pause_turn`) and when a server call is deferred behind client tool
        // results in the same parallel group (`stop_reason: "tool_use"`).
        // Carry it into that response so it can still be carded (KYO-979).
        self.pending_server_calls = activity.pending.clone();
        let ctx = &self.tool_context;
        for (tool, calls) in [
            (kyomi_core::web_tools::WebToolKind::Search, activity.searches),
            (kyomi_core::web_tools::WebToolKind::Fetch, activity.fetches),
        ] {
            if calls == 0 {
                continue;
            }
            *self.web_usage.entry(tool).or_default() += calls;
            if let Err(e) = crate::web::caps::add_usage(&ctx.kv, &ctx.workspace_id, tool, calls).await {
                warn!(error = %e, "failed to record web tool usage");
            }
            // Per call: backend, latency of the model call that ran the tool, result/citation
            // count, and the monthly quota left (workspace / instance) after counting it.
            let remaining = crate::web::caps::remaining_monthly(&ctx.kv, &ctx.config.web_tools.caps, &ctx.workspace_id, tool).await.ok();
            let record = crate::web::WebCallRecord {
                backend: self.config.server_tools.first().map(|t| t.backend.to_string()).unwrap_or_default(),
                tool: tool.tool_name(),
                billable_calls: calls,
                latency_ms: latency.as_millis() as u64,
                result_count: activity.sources.len(),
                workspace_remaining: remaining.map(|r| r.0),
                global_remaining: remaining.map(|r| r.1),
            };
            record.log();
        }
        for error in &activity.errors {
            warn!(error = %error, "provider web tool reported an error");
        }
        // Sources shown as cards need no footer; the footer lists the rest (runs
        // without events: Slack, copilot, watches; calls whose card could not be
        // recorded; citations that belong to no recorded call).
        let carded = self.show_server_tool_calls(&activity).await;
        self.web_sources.extend(activity.sources.iter().filter(|source| !carded.contains(&source.url)).cloned());
        activity
    }

    /// Record each provider-executed call as a tool event carrying the normalised
    /// sources, so the UI renders it with the same source card as a Kyomi-executed
    /// search or fetch. Returns the URLs now shown as cards: none without a
    /// runtime, and a call that cannot be recorded is logged and not counted.
    async fn show_server_tool_calls(&self, activity: &crate::web::server_tools::ServerToolActivity) -> std::collections::HashSet<String> {
        let mut carded = std::collections::HashSet::new();
        let Some((context, sink)) = &self.runtime else { return carded };
        for (index, call) in activity.calls.iter().enumerate() {
            let id = call.id.clone().unwrap_or_else(|| format!("model-{}:web:{index}", self.state.global_iteration));
            let name = call.tool.tool_name();
            let arguments = match call.tool {
                kyomi_core::web_tools::WebToolKind::Search => serde_json::json!({"query": call.subject}),
                kyomi_core::web_tools::WebToolKind::Fetch => serde_json::json!({"url": call.subject}),
            };
            let shown = ProviderToolDisplay {
                text: crate::tools::web::display_output(call.tool, &call.subject, &call.sources, call.error.as_deref()),
                succeeded: call.error.is_none(),
            };
            match context.tool(sink.as_ref(), &shown, agent_runtime::ToolCallId(id), name.to_string(), arguments).await {
                Ok(_) => carded.extend(call.sources.iter().map(|source| source.url.clone())),
                Err(error) => warn!(%error, tool = name, "could not record a provider web call as a source card; its sources are listed in the answer instead"),
            }
        }
        carded
    }

    // -----------------------------------------------------------------------
    // Internal: Tool execution
    // -----------------------------------------------------------------------

    /// Execute a single tool call and return its classified outcome. Tool
    /// failures are model-facing results, never turn-ending errors.
    async fn execute_tool(&self, tool_call: &ToolCall) -> agent_runtime::ToolOutcome {
        if self.registry.get_tool(&tool_call.name).is_none() {
            let outcome = tool_outcome(self, tool_call, "").await;
            warn!(tool = %tool_call.name, error = %outcome.text, "unknown tool requested by LLM");
            return outcome;
        }
        self.progress(AgentProgress::ToolStart(tool_call.name.clone(), tool_call.arguments.clone())).await;
        if let Some(ref cb) = self.callbacks.on_tool_start {
            cb(&tool_call.name, &tool_call.arguments);
        }
        let outcome = tool_outcome(self, tool_call, "").await;
        let success = outcome.succeeded();
        if !success {
            warn!(tool = %tool_call.name, error = %outcome.text, "tool execution failed");
        }
        self.progress(AgentProgress::ToolEnd(tool_call.name.clone(), outcome.text.clone(), success)).await;
        if let Some(ref cb) = self.callbacks.on_tool_end {
            cb(&tool_call.name, &outcome.text, success);
        }
        outcome
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
            "{}{summary}{}",
            crate::compaction::COMPACTION_SUMMARY_PREFIX,
            crate::compaction::COMPACTION_SUMMARY_SUFFIX
        )));

        // Add acknowledgment as an assistant message.
        context.push(Message::assistant(crate::compaction::COMPACTION_ACK));

        // Add recent messages (post-compaction).
        let recent_start = self.state.messages_since_compaction_index;
        if recent_start < self.state.messages.len() {
            context.extend_from_slice(&self.state.messages[recent_start..]);
        }

        context
    }

    /// Compact if needed and return the message list to send.
    ///
    /// Called at the start of every loop iteration — so it sees the previous
    /// model call's input size, and also estimates the context of the new turn
    /// (the previous count alone would miss the message the user just added) —
    /// and before the forced wrap-up call. The adapter persists the summary and
    /// cut index with the session, so the next run starts from the compacted
    /// state.
    ///
    /// The estimate is taken over the context that would actually be sent,
    /// never over `state.messages`: the latter still holds the turns an earlier
    /// compaction replaced, so counting it would re-trigger compaction on every
    /// iteration.
    ///
    /// A failed or empty summary fails the run. Carrying on uncompacted would
    /// only defer the context-limit failure to the next provider call.
    ///
    /// Returns the context to send and the summary call's billable tokens (0
    /// when nothing compacted), so the run's `max_total_tokens` guard covers
    /// the extra call too.
    async fn maybe_compact(
        &mut self,
        cancel_token: &CancellationToken,
    ) -> kyomi_core::Result<(Vec<Message>, u64)> {
        // Built once: the caller sends exactly this list, so the decision and
        // the request cannot disagree.
        let context = self.build_llm_context();
        let Some(threshold_tokens) = self.compaction.as_ref().map(|c| c.threshold_tokens) else {
            return Ok((context, 0));
        };
        // A `pause_turn` chain is one logical answer whose blocks must be
        // resent exactly as received. Compacting it would rewrite the prefix
        // mid-chain (and a failed chain is truncated back to its start), so wait
        // until the chain completes.
        if self.pause_chain_start.is_some() {
            return Ok((context, 0));
        }
        let actual = (self.state.last_input_tokens > 0).then_some(self.state.last_input_tokens);
        // Strip the system prompt: `split_messages_for_compaction` and the
        // summary prompt take only the conversation turns.
        let system_offset = usize::from(
            context
                .first()
                .is_some_and(|message| message.role == crate::types::MessageRole::System),
        );
        let system_prompt_len = context
            .first()
            .filter(|_| system_offset == 1)
            .map_or(0, |message| message.content.len());
        let conversation = &context[system_offset..];
        if !crate::compaction::should_compact(actual, conversation, system_prompt_len, threshold_tokens) {
            return Ok((context, 0));
        }

        let already_compacted = self.state.compacted_summary.is_some();
        let estimate = crate::compaction::estimate_tokens(conversation, system_prompt_len);
        let (to_summarize, to_keep) = crate::compaction::split_messages_for_compaction(conversation);
        // When a summary already exists, `build_llm_context` starts with the
        // old summary and its acknowledgment; only the messages after them are
        // real state entries that the new cut index can advance over.
        let summarised_state_messages = if already_compacted {
            to_summarize.len().saturating_sub(2)
        } else {
            to_summarize.len()
        };
        if summarised_state_messages == 0 || to_summarize.is_empty() {
            let window = self
                .compaction
                .as_ref()
                .expect("checked for the threshold above")
                .context_window;
            return Err(kyomi_core::Error::Internal(format!(
                "conversation is over the compaction threshold but no older turns can be summarised: \
                 context_window={window} threshold={threshold_tokens} estimate={estimate}"
            )));
        }

        // Even an empty summary could not bring the kept turns under the
        // threshold: fail before paying for a summary call that cannot help
        // (and that a later turn would repeat).
        let kept_tail_estimate = crate::compaction::estimate_tokens(
            &to_keep,
            system_prompt_len
                + crate::compaction::COMPACTION_SUMMARY_PREFIX.len()
                + crate::compaction::COMPACTION_SUMMARY_SUFFIX.len()
                + crate::compaction::COMPACTION_ACK.len(),
        );
        if kept_tail_estimate >= threshold_tokens {
            let window = self
                .compaction
                .as_ref()
                .expect("checked for the threshold above")
                .context_window;
            return Err(kyomi_core::Error::Internal(format!(
                "the kept turns alone are over the compaction threshold, so a summary cannot reduce them: \
                 context_window={window} threshold={threshold_tokens} estimate={kept_tail_estimate}; \
                 lower the compaction threshold or shorten the recent turns"
            )));
        }

        let mut summary_messages = to_summarize;
        summary_messages.push(Message::user(crate::compaction::COMPACTION_INSTRUCTION));
        let response = {
            let compaction = self
                .compaction
                .as_ref()
                .expect("checked for the threshold above");
            // The run's fast/title model writes the summary. There is no
            // fallback: if its own reported window cannot accept the region, the
            // run fails with numbers the operator can act on. A provider that
            // reports no window gets the request; a provider rejection fails the
            // run loudly.
            let prompt_tokens = u64::from(crate::compaction::estimate_tokens(&summary_messages, 0));
            let fast_window = u64::from(compaction.client.context_window());
            // 25% covers token-dense SQL/JSON (about 3 chars/token) plus the
            // summary's own output budget.
            let required = prompt_tokens + prompt_tokens / 4 + u64::from(compaction.options.max_tokens);
            if fast_window > 0 && required > fast_window {
                return Err(kyomi_core::Error::Internal(format!(
                    "the summary model cannot accept the conversation summary: model={} window={fast_window} \
                     needed_tokens~{} required threshold={threshold_tokens} context_window={}; \
                     choose a title model with a larger window or lower the compaction threshold",
                    compaction.client.model(),
                    required,
                    compaction.context_window,
                )));
            }
            info!(
                session_id = self.tool_context.session_id.as_deref().unwrap_or(""),
                summary_model = compaction.client.model(),
                summary_prompt_tokens = prompt_tokens,
                summary_window = fast_window,
                "conversation compaction summary"
            );
            tokio::select! {
                result = compaction.client.complete(
                    &summary_messages,
                    &[],
                    &compaction.options,
                    &self.user_names,
                ) => result?,
                _ = cancel_token.cancelled() => {
                    return Err(kyomi_core::Error::Internal("Request cancelled".into()));
                }
            }
        };
        let call_tokens = u64::from(response.usage.input_tokens)
            + u64::from(response.usage.cache_creation_input_tokens)
            + u64::from(response.usage.output_tokens);
        // This call is billed with the run; count it in the turn's reported
        // totals without changing the conversation's context size.
        let summary_input = u32::try_from(
            u64::from(response.usage.input_tokens)
                + u64::from(response.usage.cache_creation_input_tokens)
                + u64::from(response.usage.cache_read_input_tokens),
        )
        .unwrap_or(u32::MAX);
        self.record_background_usage(summary_input, response.usage.output_tokens, response.cost)
            .await;
        let summary = response.content.trim();
        if summary.is_empty() {
            return Err(kyomi_core::Error::Internal(
                "conversation compaction returned an empty summary".into(),
            ));
        }

        let next_index = if already_compacted {
            self.state.messages_since_compaction_index + summarised_state_messages
        } else {
            system_offset + summarised_state_messages
        };
        info!(
            session_id = self.tool_context.session_id.as_deref().unwrap_or(""),
            summarised_messages = summarised_state_messages,
            kept_messages = to_keep.len(),
            threshold_tokens,
            input_tokens = actual.unwrap_or(0),
            "conversation compacted"
        );
        self.state.compacted_summary = Some(summary.to_string());
        self.state.messages_since_compaction_index = next_index;
        // The pre-compaction input size no longer describes the context the
        // next call sends; let the next check use the (small) estimate until
        // the provider reports the new size.
        self.state.last_input_tokens = 0;

        // A compaction that still leaves the context over the threshold would
        // run again on every iteration (each time summarising the previous
        // summary). Fail with the numbers instead of billing a loop.
        let rebuilt = self.build_llm_context();
        let rebuilt_offset = usize::from(
            rebuilt
                .first()
                .is_some_and(|message| message.role == crate::types::MessageRole::System),
        );
        let rebuilt_system_len = rebuilt
            .first()
            .filter(|_| rebuilt_offset == 1)
            .map_or(0, |message| message.content.len());
        let rebuilt_estimate =
            crate::compaction::estimate_tokens(&rebuilt[rebuilt_offset..], rebuilt_system_len);
        if rebuilt_estimate >= threshold_tokens {
            let window = self
                .compaction
                .as_ref()
                .expect("checked for the threshold above")
                .context_window;
            return Err(kyomi_core::Error::Internal(format!(
                "conversation is still over the compaction threshold after summarising: \
                 context_window={window} threshold={threshold_tokens} estimate={rebuilt_estimate}; \
                 lower the compaction threshold or shorten the recent turns"
            )));
        }
        Ok((rebuilt, call_tokens))
    }

    /// Add an auxiliary model call's usage to the run's reported totals without
    /// touching the conversation context size (the compaction summary, KYO-975).
    async fn record_background_usage(
        &self,
        input_tokens: u32,
        output_tokens: u32,
        cost: Option<f64>,
    ) {
        if let Some(cb) = &self.callbacks.on_progress {
            cb(AgentProgress::BackgroundUsage(input_tokens, output_tokens, cost)).await;
        }
    }

    // -----------------------------------------------------------------------
    // Internal: ChartML validation with SQL dry-run
    // -----------------------------------------------------------------------

    /// Validate ChartML blocks including SQL dry-run against actual datasources.
    ///
    /// First validates the authoritative ChartML JSON Schema. If that passes,
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
    /// [`Self::validate_chartml_blocks`] (each block's schema first, followed
    /// by all SQL sources of schema-valid blocks),
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

        // Validate every schema-valid block's SQL even if another block fails schema,
        // so strip-and-degrade never preserves an unchecked SQL chart.
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
///
/// `pub(crate)` (not private) so [`crate::adapter::db_message_to_agent_message`]
/// can reuse it to reconstruct the prefix on read — see that function's doc
/// for why (KYO-506).
pub(crate) fn build_metadata_prefix(
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

/// Build the tool-result text sent back to the model for a [`ToolCall`] the
/// provider marked as having unparseable arguments (`arguments_error`,
/// KYO-535).
///
/// Must read as "your last message got cut off, send it again smaller" —
/// never as a missing-parameter error, which is what provoked the model into
/// retrying the identical call in the production incident this exists to
/// fix. Names `response_finish_reason` explicitly when it is `"max_tokens"`,
/// since that is the strongest available confirmation of *why* the
/// arguments were truncated.
fn truncated_arguments_message(tool_call: &ToolCall, response_finish_reason: &str) -> String {
    let mut message = format!(
        "Tool call '{}' was not executed: its arguments were cut off before they \
         finished streaming, so they could not be parsed ({}). This is NOT a missing \
         parameter — do not resend the identical call. Instead, send the call again \
         with a smaller payload so the full arguments fit within the response length.",
        tool_call.name,
        tool_call
            .arguments_error
            .as_deref()
            .unwrap_or("no further detail available"),
    );
    if response_finish_reason == "max_tokens" {
        message.push_str(
            " The response was truncated because it exceeded the model's max_tokens limit.",
        );
    }
    message
}

/// Check if text contains ChartML code blocks.
pub(crate) fn has_chartml_blocks(text: &str) -> bool {
    text.contains("```chartml")
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
/// the shared markdown scanner's document order, regardless of the 1-based block numbers embedded in the
/// human-readable message text. This is the same indexing contract
/// [`crate::tools::query_utils::chartml_sql_block_errors`] and
/// [`strip_chartml_blocks`] use — every function in this file and
/// `query_utils.rs` that produces or consumes a block index agrees on it. A
/// block that passes validation contributes no entry.
#[cfg(test)]
fn chartml_block_errors(text: &str) -> Vec<(usize, String)> {
    kyomi_core::chartml_validation::validate_markdown_schema(text)
        .into_iter().map(|e| (e.block - 1, e.to_string())).collect()
}

/// Validate ChartML blocks in the text (YAML structure only, no SQL).
///
/// Extracts all ` ```chartml ... ``` ` blocks, parses each as YAML, and
/// validates the authoritative ChartML JSON Schema. Thin aggregate wrapper
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
/// Indices use the shared scanner's document order. Replacement is copied
/// literally, including dollar signs, and unclosed trailing blocks are covered.
fn strip_chartml_blocks_with(text: &str, failing_indices: &[usize], replacement: &str) -> String {
    kyomi_core::chartml_validation::strip_markdown_blocks(text, failing_indices, replacement)
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

fn runtime_error(error: agent_runtime::ExecutionError) -> kyomi_core::Error {
    kyomi_core::Error::ServiceUnavailable(error.to_string())
}
struct RuntimeProvider<'a> {
    agent: &'a CustomAgent,
    messages: &'a [Message],
    tools: &'a [Tool],
    allow_server_tools: bool,
}
#[async_trait::async_trait]
impl agent_runtime::CompleteProvider for RuntimeProvider<'_> {
    type Response = crate::types::LLMResponse;
    async fn complete(&self) -> Result<(Self::Response, agent_runtime::ResponseRecord), agent_runtime::ExecutionError> {
        let options = self.agent.call_options(self.allow_server_tools).await;
        let response = self.agent.client.complete(self.messages, self.tools, &options, &self.agent.user_names).await
            .map_err(|error| agent_runtime::ExecutionError::Provider(error.to_string()))?;
        // Before persistence: a paid response is accounted even if committing it fails.
        self.agent.account_usage(&response).await;
        let mut planning = Vec::new();
        if let Some(thought) = response.thinking_content.as_deref().and_then(crate::thinking::clean_thought) {
            planning.push(thought.full_text.unwrap_or(thought.display));
        }
        // A chart-bearing candidate becomes public only after product schema/SQL validation.
        if response.tool_calls.is_some() && !has_chartml_blocks(&response.content)
            && let Some(thought) = crate::thinking::clean_thought(&response.content)
        {
            planning.push(thought.full_text.unwrap_or(thought.display));
        }
        let raw = if response.raw_response.is_null() {
            serde_json::to_value(&response).map_err(|error| agent_runtime::ExecutionError::Provider(error.to_string()))?
        } else { response.raw_response.clone() };
        let record = agent_runtime::ResponseRecord { continuation: raw.clone(), raw, cost: response.cost,
            candidate: response.content.clone(), planning,
            usage: agent_runtime::Usage {
                input_tokens: u64::from(response.usage.input_tokens) + u64::from(response.usage.cache_creation_input_tokens)
                    + u64::from(response.usage.cache_read_input_tokens),
                output_tokens: u64::from(response.usage.output_tokens),
            },
        };
        Ok((response, record))
    }
}
/// A provider-executed call, already finished by the provider: recording it only
/// stores its normalised result for display; nothing runs. A result the provider
/// flagged as an error is recorded as a rejection, so the card reads as failed.
struct ProviderToolDisplay {
    text: String,
    succeeded: bool,
}
#[async_trait::async_trait]
impl agent_runtime::ToolExecution for ProviderToolDisplay {
    async fn execute(&self) -> agent_runtime::ToolOutcome {
        agent_runtime::ToolOutcome {
            transport: agent_runtime::TransportOutcome::Completed,
            domain: if self.succeeded { agent_runtime::DomainOutcome::Succeeded } else { agent_runtime::DomainOutcome::Rejected },
            text: self.text.clone(),
        }
    }
}

struct RuntimeTool<'a> {
    agent: &'a CustomAgent,
    call: &'a ToolCall,
    finish_reason: &'a str,
}
#[async_trait::async_trait]
impl agent_runtime::ToolExecution for RuntimeTool<'_> {
    async fn execute(&self) -> agent_runtime::ToolOutcome {
        tool_outcome(self.agent, self.call, self.finish_reason).await
    }
}

/// Model-facing text prefix for a call to a tool name that is not registered.
/// This is persisted wire text: [`unknown_tool_text`] formats it and
/// [`is_repairable_rejection`] reads it back from the stored outcome, so it must
/// not change without migrating how stored outcomes are classified.
const UNKNOWN_TOOL_PREFIX: &str = "Error: Unknown tool '";

/// The model-facing text for a call to an unregistered tool. The single producer
/// of [`UNKNOWN_TOOL_PREFIX`] text.
fn unknown_tool_text(name: &str, available: &str) -> String {
    format!("{UNKNOWN_TOOL_PREFIX}{name}'. Available tools: {available}")
}

/// Whether a failed tool call is a *repairable rejection*: a definite
/// (`DomainOutcome::Rejected`), effect-free rejection returned by a real tool
/// that ran with parsed arguments. This includes a real tool's own
/// `BadRequest` (for example a missing required parameter). A rejection that
/// happens before any tool runs (unparsable or truncated arguments, an unknown
/// tool name) is not repairable in this sense and keeps the strict failure
/// budget.
///
/// Derived only from the call and outcome values (`arguments_error` travels with
/// the serialized `ToolCall`; domain, transport and text come from the stored
/// outcome), never from live registry state. The unknown-tool case requires
/// `TransportOutcome::Failed`, the only transport [`tool_outcome`] produces for
/// it, so a real tool that merely returns similar text cannot be misread.
fn is_repairable_rejection(call: &ToolCall, outcome: &agent_runtime::ToolOutcome) -> bool {
    let unknown_tool = outcome.transport == agent_runtime::TransportOutcome::Failed
        && outcome.text.starts_with(UNKNOWN_TOOL_PREFIX);
    outcome.domain == agent_runtime::DomainOutcome::Rejected
        && call.arguments_error.is_none()
        && !unknown_tool
}

/// Execute one tool call and classify every failure into a model-facing
/// `ToolOutcome`. A panic inside the tool is caught and becomes an Unknown
/// outcome with verify-before-retry guidance; it never escapes the turn.
async fn tool_outcome(
    agent: &CustomAgent,
    call: &ToolCall,
    finish_reason: &str,
) -> agent_runtime::ToolOutcome {
    use agent_runtime::{DomainOutcome, ToolOutcome, TransportOutcome};
    if call.arguments_error.is_some() {
        return ToolOutcome { transport: TransportOutcome::Failed, domain: DomainOutcome::Rejected,
            text: truncated_arguments_message(call, finish_reason) };
    }
    let Some(tool) = agent.registry.get_tool(&call.name) else {
        return ToolOutcome { transport: TransportOutcome::Failed, domain: DomainOutcome::Rejected,
            text: unknown_tool_text(&call.name, &agent.registry.tool_names().join(", ")) };
    };
    // The panic boundary covers the whole tool call, including the tool's own
    // result classification: a buggy tool must never escape the turn.
    let executed = std::panic::AssertUnwindSafe(async {
        match tool.execute(call.arguments.clone(), &agent.tool_context).await {
            Ok(text) => ToolOutcome { transport: TransportOutcome::Completed,
                domain: tool.result_domain_outcome(&text), text },
            Err(error) => ToolOutcome { transport: TransportOutcome::Failed,
                domain: if matches!(&error, kyomi_core::Error::BadRequest(_))
                    && tool.bad_request_is_pre_effect()
                {
                    DomainOutcome::Rejected
                } else {
                    tool.failure_domain_outcome()
                },
                text: format!("Tool '{}' failed: {}", call.name, error) },
        }
    })
    .catch_unwind()
    .await;
    let mut outcome = match executed {
        Ok(outcome) => outcome,
        Err(panic) => ToolOutcome { transport: TransportOutcome::Failed, domain: DomainOutcome::Unknown,
            text: format!("Tool '{}' panicked: {}", call.name, panic_message(panic)) },
    };
    outcome.ensure_unknown_guidance();
    outcome
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn tool_call_fingerprint(call: &ToolCall) -> String {
    format!("{}:{}", call.name, serde_json::to_string(&call.arguments).unwrap_or_default())
}

/// Prefix of the bounded-loop-guard terminal message. Consumers that expect a
/// structured response (watch) use this to avoid retrying the whole agent.
pub(crate) const TOOL_FAILURE_GUARD_PREFIX: &str = "I stopped after ";

pub(crate) fn is_tool_failure_guard_message(text: &str) -> bool {
    text.starts_with(TOOL_FAILURE_GUARD_PREFIX)
        && text.contains("to avoid looping")
        && text.contains("I did not retry automatically")
}

fn tool_failure_guard_message(consecutive: usize, same_call: usize, error: &str) -> String {
    let reason = if same_call >= MAX_SAME_FAILING_TOOL_CALLS {
        format!("the same tool call failed {same_call} times")
    } else {
        format!("{consecutive} consecutive tool calls failed")
    };
    let preview: String = error.chars().take(800).collect();
    format!(
        "I stopped after {reason} to avoid looping. The last tool result was:\n\n{preview}\n\nI did not retry automatically. Please check the error and try a different approach."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_and_dev_server_profiles_unwind_so_tool_panics_are_contained() {
        let manifest = include_str!("../../../Cargo.toml");
        let block = |name: &str| {
            manifest
                .split(&format!("[profile.{name}]"))
                .nth(1)
                .and_then(|rest| rest.split("\n[").next())
                .unwrap_or("")
        };
        for name in ["release", "dev-server"] {
            let profile = block(name);
            assert!(
                !profile.contains("panic = \"abort\""),
                "the shipping {name} profile must unwind or the agent's catch_unwind boundary is inert: {profile}"
            );
        }
        assert!(
            block("release").contains("panic = \"unwind\""),
            "the release profile must set panic = \"unwind\" explicitly"
        );
    }

    fn call_with(arguments_error: Option<&str>) -> ToolCall {
        ToolCall {
            id: "tc".into(),
            name: "some_tool".into(),
            arguments: serde_json::json!({}),
            arguments_error: arguments_error.map(str::to_string),
        }
    }

    fn outcome_with(
        transport: agent_runtime::TransportOutcome,
        domain: agent_runtime::DomainOutcome,
        text: &str,
    ) -> agent_runtime::ToolOutcome {
        agent_runtime::ToolOutcome { transport, domain, text: text.into() }
    }

    /// KYO-970: the single definition of a repairable rejection. Only a Rejected
    /// outcome from a real tool with parsed arguments qualifies; every other
    /// failure keeps the strict budget. Pure function of the call and outcome.
    #[test]
    fn repairable_rejection_classification_matrix() {
        use agent_runtime::{DomainOutcome as D, TransportOutcome as T};
        let ok_call = call_with(None);
        // Real tool, definite rejection (Err(BadRequest) pre-effect, or Ok(error JSON)).
        assert!(is_repairable_rejection(&ok_call, &outcome_with(T::Failed, D::Rejected, "Tool 'x' failed: bad")));
        assert!(is_repairable_rejection(&ok_call, &outcome_with(T::Completed, D::Rejected, "{\"error\":\"bad sql\"}")));
        // Rejected before any tool ran: truncated/unparsable arguments.
        assert!(!is_repairable_rejection(&call_with(Some("EOF")), &outcome_with(T::Failed, D::Rejected, "Tool call 'x' was not executed")));
        // Rejected before any tool ran: unknown tool name (text produced by tool_outcome).
        let unknown = unknown_tool_text("nope", "a, b");
        assert!(!is_repairable_rejection(&ok_call, &outcome_with(T::Failed, D::Rejected, &unknown)));
        // A real tool that merely RETURNS similar text (transport Completed) is still repairable.
        assert!(is_repairable_rejection(&ok_call, &outcome_with(T::Completed, D::Rejected, &unknown)));
        // Unknown outcome (may have had an effect) and success are never repairable rejections.
        assert!(!is_repairable_rejection(&ok_call, &outcome_with(T::Failed, D::Unknown, "boom")));
        assert!(!is_repairable_rejection(&ok_call, &outcome_with(T::Unknown, D::Unknown, "lost")));
        assert!(!is_repairable_rejection(&ok_call, &outcome_with(T::Completed, D::Succeeded, "{}")));
    }

    /// Agent on the legacy (non-runtime) path whose model issues one tool call per
    /// response, `names[i]` on response `i` with distinct arguments, then answers "done".
    async fn legacy_agent(names: Vec<&'static str>) -> CustomAgent {
        let requested: Vec<(&'static str, serde_json::Value)> = names
            .iter()
            .enumerate()
            .map(|(i, name)| (*name, serde_json::json!({ "n": i })))
            .collect();
        let mut script: Vec<Reply> = vec![Reply::ToolCall; requested.len()];
        script.push(Reply::Text("done".to_string()));
        let provider = ActualToolsProvider {
            inner: ScriptedProvider {
                script: Mutex::new(script.into()),
                after_script: Reply::Text("unexpected".to_string()),
                calls: Arc::new(Mutex::new(Vec::new())),
            },
            requested,
            index: std::sync::atomic::AtomicUsize::new(0),
            truncation: Truncation::Never,
            batch: 1,
        };
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(FailingTool {
            kind: FailureKind::BadRequest,
            name: "legacy_reject",
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }));
        CustomAgent::new(
            Box::new(provider),
            AgentConfig { max_iterations: 25, ..Default::default() },
            Arc::new(registry),
            test_tool_context().await,
            HashMap::new(),
        )
    }

    /// Non-runtime path: nine distinct repairable rejections do not stop the turn.
    #[tokio::test]
    async fn legacy_path_nine_repairable_rejections_do_not_stop_the_turn() {
        let mut agent = legacy_agent(vec!["legacy_reject"; 9]).await;
        let answer = agent.chat("hello", CancellationToken::new(), None, None, None).await.unwrap();
        assert_eq!(answer, "done");
    }

    /// Non-runtime path: the tenth consecutive repairable rejection stops the turn.
    #[tokio::test]
    async fn legacy_path_ten_repairable_rejections_stop_the_turn() {
        let mut agent = legacy_agent(vec!["legacy_reject"; 12]).await;
        let answer = agent.chat("hello", CancellationToken::new(), None, None, None).await.unwrap();
        assert!(answer.contains("10 consecutive tool calls failed"), "{answer}");
    }

    /// Non-runtime path: unknown-tool rejections keep the strict budget of five.
    #[tokio::test]
    async fn legacy_path_unknown_tool_rejections_keep_the_strict_budget() {
        let mut agent = legacy_agent(vec!["no_such_tool"; 7]).await;
        let answer = agent.chat("hello", CancellationToken::new(), None, None, None).await.unwrap();
        assert!(answer.contains("5 consecutive tool calls failed"), "{answer}");
    }

    #[test]
    fn tool_failure_guard_message_is_recognizable_without_matching_normal_answers() {
        let message = tool_failure_guard_message(5, 1, "boom");
        assert!(is_tool_failure_guard_message(&message));
        assert!(!is_tool_failure_guard_message("Revenue grew 12% last quarter."));
    }

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
        let text = "Chart:\n```chartml\ntype: chart\nversion: 1\ndata:\n  datasource: test\n  query: SELECT 1\nvisualize:\n  type: bar\n```";
        assert!(validate_chartml_blocks(text).is_none());
    }

    #[test]
    fn validate_chartml_blocks_invalid_yaml() {
        let text = "Chart:\n```chartml\n{invalid: yaml: [:\n```";
        let result = validate_chartml_blocks(text);
        assert!(result.is_some());
        assert!(result.unwrap().contains("yaml"));
    }

    #[test]
    fn validate_chartml_blocks_missing_data_key() {
        let text = "Chart:\n```chartml\nvisualize:\n  type: bar\n```";
        let result = validate_chartml_blocks(text);
        assert!(result.is_some());
        assert!(result.unwrap().contains("data"));
    }

    #[test]
    fn validate_chartml_blocks_missing_visualize_key() {
        let text = "Chart:\n```chartml\ntype: chart\nversion: 1\ndata:\n  datasource: test\n  query: SELECT 1\n```";
        let result = validate_chartml_blocks(text);
        assert!(result.is_some());
        assert!(result.unwrap().contains("visualize"));
    }

    #[test]
    fn validate_chartml_blocks_missing_both_keys() {
        let text = "Chart:\n```chartml\ntitle: My Chart\n```";
        let result = validate_chartml_blocks(text);
        assert!(result.is_some());
        let err = result.unwrap();
        assert!(err.contains("data"));
        assert!(err.contains("visualize"));
    }

    #[test]
    fn validate_chartml_blocks_multiple_blocks_one_invalid() {
        let text = "\
Chart 1:\n```chartml\ntype: chart\nversion: 1\ndata:\n  datasource: test\n  query: SELECT 1\nvisualize:\n  type: bar\n```\n\
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
        let text = "Chart:\n```chartml\ntype: chart\nversion: 99";
        assert!(has_chartml_blocks(text));
        assert!(validate_chartml_blocks(text).is_some());
    }

    // -- strip_chartml_blocks tests (KYO-347) --------------------------------

    #[test]
    fn strip_chartml_blocks_single_block() {
        let text = "Before\n```chartml\ntype: chart\nversion: 1\ndata:\n  x: 1\n```\nAfter";
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
        let text = "Intro paragraph with **markdown**.\n\n```chartml\ntype: chart\nversion: 1\ndata:\n  x: 1\n```\n\nOutro paragraph.";
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
    fn strip_chartml_blocks_unterminated_fence_is_stripped() {
        let text = "Prose\n```chartml\ntype: chart\nversion: 99";
        assert_eq!(strip_chartml_blocks(text, &[0]), format!("Prose\n{CHARTML_STRIPPED_NOTE}"));
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
        let text = "Chart:\n```chartml\ntype: chart\nversion: 1\ntitle: Revenue\ndata:\n  datasource: test\n  query: SELECT 1\nvisualize:\n  type: bar\nlayout:\n  colSpan: 6\n```";
        assert!(validate_chartml_blocks(text).is_none());
    }

    #[test]
    fn validate_chartml_blocks_multiple_valid_blocks() {
        let text = "\
Chart 1:\n```chartml\ntype: chart\nversion: 1\ndata:\n  datasource: test\n  query: SELECT 1\nvisualize:\n  type: bar\n```\n\
Chart 2:\n```chartml\ntype: chart\nversion: 1\ndata:\n  datasource: test\n  query: SELECT 2\nvisualize:\n  type: line\n```";
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
            "```ChartML\ndata:\n  datasource: test\n  query: SELECT 1\n```"
        ));
        assert!(!has_chartml_blocks(
            "```CHARTML\ndata:\n  datasource: test\n  query: SELECT 1\n```"
        ));
    }

    #[test]
    fn validate_chartml_blocks_malformed_closing_fence() {
        assert!(validate_chartml_blocks("```chartml\ntype: chart\nversion: 99").is_some());
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

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use serde_json::json;

    use crate::types::{AgentTokenUsage, MessageRole};

    // ScriptedProvider, Reply, NoopTool, RecordedCall — extracted to
    // `crate::test_support` (KYO-493 phase 3) so `adapter.rs`'s tests can
    // drive the same ToolCall-then-Text scripting through a full
    // `ChatAgentAdapter::chat()` turn without a second copy. See that
    // module's doc comment on the extraction.
    use crate::test_support::{ActualToolsProvider, FailingTool, FailureKind, NoopTool, RecordedCall, Reply, ScriptedProvider, Truncation};

    /// Build a `ToolContext` over an in-memory sqlite pool and in-memory KV
    /// store. `session_id: None` keeps the agent off the ChartML validation
    /// log writes, so no migrated schema is required.
    ///
    /// Delegates to the shared `crate::test_support` fixture (KYO-537) —
    /// see that module's docs for exactly what it does and does not cover.
    async fn test_tool_context() -> ToolContext {
        crate::test_support::build_ctx(crate::test_support::test_pool().await)
    }

    /// Build an agent wired to a scripted provider and a one-tool registry,
    /// with full control over the config fields a test cares about.
    async fn scripted_agent_with_config(
        config: AgentConfig,
        script: Vec<Reply>,
        after_script: Reply,
    ) -> (CustomAgent, Arc<Mutex<Vec<RecordedCall>>>) {
        let (agent, calls, _ledger) = scripted_metered_agent(config, script, after_script).await;
        (agent, calls)
    }

    /// As [`scripted_agent_with_config`], with the provider wrapped in the same
    /// `MeteredClient` production uses, and its ledger returned.
    async fn scripted_metered_agent(
        config: AgentConfig,
        script: Vec<Reply>,
        after_script: Reply,
    ) -> (CustomAgent, Arc<Mutex<Vec<RecordedCall>>>, crate::usage_ledger::UsageLedger) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let provider = ScriptedProvider {
            script: Mutex::new(script.into()),
            after_script,
            calls: Arc::clone(&calls),
        };

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(NoopTool));

        let metered = crate::usage_ledger::MeteredClient::new(Box::new(provider));
        let ledger = metered.ledger();
        let agent = CustomAgent::new(
            Box::new(metered),
            config,
            Arc::new(registry),
            test_tool_context().await,
            HashMap::new(),
        );

        (agent, calls, ledger)
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
            "Here is your chart:\n```chartml\ntype: chart\nversion: 1\ndata:\n  provider: inline\n  rows: []\nvisualize:\n  type: bar\n```\nDone.";
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
        let mixed_chartml = "First chart:\n```chartml\ntype: chart\nversion: 1\ndata:\n  provider: inline\n  rows: []\n\
visualize:\n  type: bar\n```\nSecond chart:\n```chartml\nvisualize:\n  type: line\n```\nDone.";
        let script = vec![Reply::ToolCall; 2];
        let (mut agent, _calls) =
            scripted_agent(2, script, Reply::Text(mixed_chartml.to_string())).await;

        let answer = agent
            .chat("hello", CancellationToken::new(), None, None, None)
            .await
            .expect("chat should complete — strip-and-degrade, not an error");

        let expected = format!(
            "First chart:\n```chartml\ntype: chart\nversion: 1\ndata:\n  provider: inline\n  rows: []\nvisualize:\n  type: bar\n```\n\
Second chart:\n{CHARTML_STRIPPED_NOTE}\nDone."
        );
        assert_eq!(
            answer, expected,
            "the valid first block must survive byte-identically; only the invalid \
             second block becomes the stripped-note text"
        );
    }

    #[tokio::test]
    async fn wrap_up_chartml_mixed_schema_failure_still_checks_other_sql_blocks() {
        let inline = "```chartml\ntype: chart\nversion: 1\ndata: {provider: inline, rows: []}\nvisualize: {type: bar}\n```";
        let unavailable = "```chartml\ntype: chart\nversion: 1\ndata: {datasource: absent, query: SELECT 1}\nvisualize: {type: bar}\n```";
        let invalid = "```chartml\ntype: chart\nversion: 1\ndata: {provider: inline, rows: []}\nvisualize: {type: wrong}\n```";
        let content = format!("{inline}\n{unavailable}\n{invalid}");
        let (mut agent, _) = scripted_agent(2, vec![Reply::ToolCall; 2], Reply::Text(content)).await;
        let answer = agent.chat("hello", CancellationToken::new(), None, None, None).await.unwrap();
        assert_eq!(answer, format!("{inline}\n{CHARTML_STRIPPED_NOTE}\n{CHARTML_STRIPPED_NOTE}"));
    }

    #[tokio::test]
    async fn wrap_up_chartml_unclosed_named_query_and_dependencies_are_stripped() {
        let inline = "```chartml\ntype: chart\nversion: 1\ndata: {provider: inline, rows: []}\nvisualize: {type: bar}\n```";
        let definition = "```chartml\ntype: source\nversion: 1\nname: shared\ndatasource: absent\nquery: SELECT {{missing}}\n```";
        let dependent = "```chartml\ntype: chart\nversion: 1\ndata: {query: shared}\nvisualize: {type: bar}\n```";
        let unclosed = "```chartml\ntype: chart\nversion: 1\ndata: {query: {datasource: absent, query: SELECT 1}}\nvisualize: {type: bar}";
        let content = format!("{inline}\n{definition}\n{dependent}\n{unclosed}");
        let (mut agent, _) = scripted_agent(2, vec![Reply::ToolCall; 2], Reply::Text(content)).await;
        let answer = agent.chat("hello", CancellationToken::new(), None, None, None).await.unwrap();
        assert_eq!(answer, format!("{inline}\n{CHARTML_STRIPPED_NOTE}\n{CHARTML_STRIPPED_NOTE}\n{CHARTML_STRIPPED_NOTE}"));
        assert_eq!(agent.state().messages.last().unwrap().content, answer);
    }

    #[tokio::test]
    async fn wrap_up_chartml_bare_fence_prefix_and_lost_defaults_are_stripped() {
        let prefix = "```\n```chartml\ntype: chart\nversion: 99";
        let (mut agent, _) = scripted_agent(2, vec![Reply::ToolCall; 2], Reply::Text(prefix.into())).await;
        let answer = agent.chat("hello", CancellationToken::new(), None, None, None).await.unwrap();
        assert_eq!(answer, format!("```\n{CHARTML_STRIPPED_NOTE}"));
        let owner = "```chartml\n- type: params\n  version: 1\n  name: defaults\n  params: [{id: amount, type: number, label: Amount, default: 2}]\n- type: source\n  version: 1\n  name: bad\n  datasource: absent\n  query: BAD SQL\n```";
        let consumer = "```chartml\ntype: chart\nversion: 1\ndata: {datasource: absent, query: 'SELECT {{amount}}'}\nvisualize: {type: table}\n```";
        let (mut agent, _) = scripted_agent(2, vec![Reply::ToolCall; 2], Reply::Text(format!("{owner}\n{consumer}"))).await;
        let answer = agent.chat("hello", CancellationToken::new(), None, None, None).await.unwrap();
        assert_eq!(answer, format!("{CHARTML_STRIPPED_NOTE}\n{CHARTML_STRIPPED_NOTE}"));
    }

    #[tokio::test]
    async fn final_chartml_bare_prefix_and_parameter_failures_trigger_retry() {
        for content in [
            "```\n```chartml\ntype: chart\nversion: 99",
            "```chartml\n- type: params\n  version: 1\n  name: defaults\n  params: [{id: amount, type: number, label: Amount, default: 2}]\n- type: source\n  version: 1\n  name: bad\n  datasource: absent\n  query: BAD SQL\n```\n```chartml\ntype: chart\nversion: 1\ndata: {datasource: absent, query: 'SELECT {{amount}}'}\nvisualize: {type: table}\n```"
        ] {
            let (mut agent, _) = scripted_agent(6, vec![Reply::Text(content.into()), Reply::Text("Corrected answer".into())], Reply::Text("unused".into())).await;
            let answer = agent.chat("hello", CancellationToken::new(), None, None, None).await.unwrap();
            assert_eq!(answer, "Corrected answer");
            assert_eq!(agent.state().messages.last().unwrap().content, answer);
        }
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
    async fn provider_thinking_reaches_the_thinking_callback() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let (mut agent, _calls) = scripted_agent_with_config(
            AgentConfig::default(),
            vec![Reply::TextWithThinking("Done.".into(), "I compared the two metrics.".into())],
            Reply::Text("unused".into()),
        )
        .await;
        let sink = seen.clone();
        agent.callbacks_mut().on_thinking = Some(Box::new(move |t: &str| sink.lock().unwrap().push(t.to_string())));

        agent.chat("hi", CancellationToken::new(), None, None, None).await.unwrap();

        assert_eq!(*seen.lock().unwrap(), vec!["I compared the two metrics.".to_string()]);
    }

    #[tokio::test]
    async fn usage_ledger_records_every_model_call_of_the_run() {
        let usage = |w, r| AgentTokenUsage {
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_input_tokens: w,
            cache_read_input_tokens: r,
            reasoning_tokens: 0,
        };
        let (mut agent, _calls, ledger) = scripted_metered_agent(
            AgentConfig::default(),
            vec![Reply::ToolCallWithUsage(usage(5000, 0)), Reply::ToolCallWithUsage(usage(0, 5000))],
            Reply::Text("done".into()),
        )
        .await;

        agent.chat("hi", CancellationToken::new(), None, None, None).await.unwrap();

        let t = ledger.totals();
        assert_eq!((t.cache_creation_input_tokens, t.cache_read_input_tokens), (5000, 5000));
        assert_eq!(ledger.extra_metadata().unwrap()["calls"].as_array().unwrap().len(), 3);
    }

    /// The forced wrap-up turn after `max_iterations` is a paid provider call
    /// carrying the full context; it must be in the ledger like any other
    /// (the lifecycle review's blocker: it used to bypass accounting).
    #[tokio::test]
    async fn wrap_up_call_after_max_iterations_is_accounted() {
        let (mut agent, calls, ledger) = scripted_metered_agent(
            AgentConfig { max_iterations: 2, max_duration: None, ..Default::default() },
            vec![Reply::ToolCall; 2],
            Reply::Text("wrapped up".into()),
        )
        .await;
        let tokens = Arc::new(Mutex::new(0u32));
        let sink = tokens.clone();
        agent.callbacks_mut().on_token_usage = Some(Box::new(move |_, _, _| *sink.lock().unwrap() += 1));

        agent.chat("hi", CancellationToken::new(), None, None, None).await.unwrap();

        let provider_calls = calls.lock().unwrap().len();
        assert_eq!(provider_calls, 3, "2 loop iterations + the wrap-up");
        assert_eq!(ledger.extra_metadata().unwrap()["calls"].as_array().unwrap().len(), provider_calls);
        assert_eq!(*tokens.lock().unwrap() as usize, provider_calls, "usage callback also sees every call");
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

    // -----------------------------------------------------------------------
    // Truncated tool-call arguments (KYO-535)
    // -----------------------------------------------------------------------

    #[test]
    fn truncated_arguments_message_names_truncation_not_a_missing_parameter() {
        let tool_call = ToolCall {
            id: "tc_1".to_string(),
            name: "update_dashboard".to_string(),
            arguments: json!({}),
            arguments_error: Some(
                "arguments could not be parsed as JSON (finish_reason=max_tokens): EOF".into(),
            ),
        };
        let message = truncated_arguments_message(&tool_call, "max_tokens");

        assert!(
            message.contains("truncat") || message.contains("cut off"),
            "message must name truncation: {message}"
        );
        assert!(
            !message.contains("Missing required parameter"),
            "message must not read as a missing parameter: {message}"
        );
        assert!(
            message.contains("max_tokens"),
            "message must say so explicitly when finish_reason is max_tokens: {message}"
        );
        assert!(
            message.contains("update_dashboard"),
            "message should name the tool call: {message}"
        );
    }

    #[test]
    fn truncated_arguments_message_omits_max_tokens_line_when_finish_reason_differs() {
        // The provider can set arguments_error without the response's overall
        // finish_reason being exactly "max_tokens" (e.g. Gemini's finish_reason
        // is overridden to "tool_use" whenever any tool call is present — see
        // GeminiProvider::parse_response). The base truncation wording must
        // still be there; only the max_tokens-specific sentence is conditional.
        let tool_call = ToolCall {
            id: "tc_1".to_string(),
            name: "update_dashboard".to_string(),
            arguments: json!({}),
            arguments_error: Some(
                "arguments ('args') were missing from the functionCall \
                 (finish_reason=max_tokens)"
                    .into(),
            ),
        };
        let message = truncated_arguments_message(&tool_call, "tool_use");

        assert!(
            message.contains("truncat") || message.contains("cut off"),
            "base wording must still name truncation: {message}"
        );
        assert!(
            !message.contains("Missing required parameter"),
            "message must not read as a missing parameter: {message}"
        );
        assert!(
            !message.contains("max_tokens limit"),
            "the finish_reason-specific sentence must not fire when \
             response_finish_reason isn't literally \"max_tokens\": {message}"
        );
    }

    #[tokio::test]
    async fn truncated_tool_call_is_never_executed_and_the_loop_continues() {
        // Full agent-loop test: a max_tokens response carrying a tool call
        // with arguments_error must not execute the tool, must produce a
        // tool-result message naming truncation (not "Missing required
        // parameter"), and — critically — must let the loop continue so the
        // model gets a chance to retry with a smaller payload, rather than
        // aborting the turn.
        let (mut agent, _calls) = scripted_agent(
            5,
            vec![Reply::TruncatedToolCall],
            Reply::Text("done".to_string()),
        )
        .await;

        let tool_start_count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&tool_start_count);
        agent.callbacks_mut().on_tool_start = Some(Box::new(move |_name, _args| {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        let answer = agent
            .chat("hello", CancellationToken::new(), None, None, None)
            .await
            .expect("chat should complete, not error — a truncated call must not abort the turn");

        assert_eq!(
            answer, "done",
            "the loop must continue past the truncated call to the next iteration"
        );
        assert_eq!(
            tool_start_count.load(Ordering::SeqCst),
            0,
            "the noop tool must never execute when arguments_error is set"
        );

        let tool_results: Vec<&Message> = agent
            .state()
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::Tool)
            .collect();
        assert_eq!(tool_results.len(), 1, "exactly one tool-result message: {tool_results:?}");
        let result_text = &tool_results[0].content;
        assert!(
            result_text.contains("truncat") || result_text.contains("cut off"),
            "tool result must name truncation: {result_text}"
        );
        assert!(
            !result_text.contains("Missing required parameter"),
            "tool result must not read as a missing parameter: {result_text}"
        );
    }

    // -- provider-executed web tools (KYO-969) ----------------------------

    fn server_tool_agent_config() -> AgentConfig {
        AgentConfig {
            max_iterations: 6,
            server_tools: vec![crate::web::server_tools::ServerTool {
                kind: kyomi_core::web_tools::WebToolKind::Search,
                backend: kyomi_core::web_tools::WebBackendKind::Anthropic,
                entry: serde_json::json!({"type": "web_search_20250305", "name": "web_search", "max_uses": 5}),
            }],
            ..Default::default()
        }
    }

    fn server_response(finish_reason: &str, tool_calls: Option<Vec<ToolCall>>) -> crate::types::LLMResponse {
        let raw = crate::web::server_tools::tests_support::search_response_with_stop(finish_reason);
        crate::types::LLMResponse {
            raw_response: raw,
            content: "I'll search for that.".into(),
            finish_reason: finish_reason.into(),
            usage: Default::default(),
            tool_calls,
            cost: None,
            thinking_content: None,
            call_detail: None,
        }
    }

    #[tokio::test]
    async fn pause_turn_resends_the_paused_assistant_message_verbatim_and_continues() {
        let paused = server_response("pause_turn", None);
        let verbatim = paused.raw_response["content"].as_array().unwrap().clone();
        let (mut agent, calls) = scripted_agent_with_config(
            server_tool_agent_config(),
            vec![Reply::Response(Box::new(paused))],
            Reply::Text("final answer".into()),
        )
        .await;
        let answer = agent.chat("hi", CancellationToken::new(), None, None, None).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "pause_turn must trigger exactly one continuation call");
        let second = &calls[1].messages;
        let last = second.last().unwrap();
        assert_eq!(last.role, MessageRole::Assistant, "continuation ends with the paused assistant message, no injected user turn");
        assert_eq!(last.provider_content.as_ref().unwrap(), &verbatim, "blocks incl. encrypted_content unchanged");
        assert!(answer.starts_with("final answer"));
        assert!(answer.ends_with("**Sources**\n- [A](https://a.example/x)"), "sources footer: {answer}");
    }

    #[tokio::test]
    async fn pause_turn_continuations_are_bounded() {
        let (mut agent, calls) = scripted_agent_with_config(
            AgentConfig { max_iterations: 50, ..server_tool_agent_config() },
            vec![],
            Reply::Response(Box::new(server_response("pause_turn", None))),
        )
        .await;
        let _ = agent.chat("hi", CancellationToken::new(), None, None, None).await;
        assert!(calls.lock().unwrap().len() as u32 <= MAX_PAUSE_CONTINUATIONS + 1);
    }

    #[tokio::test]
    async fn server_blocks_survive_a_following_client_tool_call_verbatim() {
        let tool_call = ToolCall { id: "tc_1".into(), name: crate::test_support::NOOP_TOOL_NAME.into(), arguments: serde_json::json!({}), arguments_error: None };
        let mixed = server_response("tool_use", Some(vec![tool_call]));
        let verbatim = mixed.raw_response["content"].as_array().unwrap().clone();
        let (mut agent, calls) = scripted_agent_with_config(
            server_tool_agent_config(),
            vec![Reply::Response(Box::new(mixed))],
            Reply::Text("done".into()),
        )
        .await;
        agent.chat("hi", CancellationToken::new(), None, None, None).await.unwrap();
        let calls = calls.lock().unwrap();
        let assistant = calls[1].messages.iter().find(|m| m.role == MessageRole::Assistant).unwrap();
        assert_eq!(assistant.provider_content.as_ref().unwrap(), &verbatim);
        assert!(assistant.tool_calls.is_some());
    }

    #[tokio::test]
    async fn provider_usage_feeds_the_monthly_budget() {
        let (mut agent, _calls) = scripted_agent_with_config(
            server_tool_agent_config(),
            vec![Reply::Response(Box::new(server_response("end_turn", None)))],
            Reply::Text("done".into()),
        )
        .await;
        agent.chat("hi", CancellationToken::new(), None, None, None).await.unwrap();
        let ctx = &agent.tool_context;
        let caps = kyomi_core::web_tools::WebCaps { workspace_monthly: 1, ..Default::default() };
        let refusal = crate::web::caps::check_monthly(&ctx.kv, &caps, &ctx.workspace_id, kyomi_core::web_tools::WebToolKind::Search)
            .await
            .unwrap();
        assert!(refusal.is_err(), "one provider-reported search must count against a cap of 1");
    }

    #[tokio::test]
    async fn spent_budget_withholds_the_server_tool_from_the_request() {
        let (mut agent, _calls) = scripted_agent_with_config(server_tool_agent_config(), vec![], Reply::Text("x".into())).await;
        assert_eq!(agent.request_options_for_call(true).await.server_tools.len(), 1);
        let ctx = &agent.tool_context;
        let cap = ctx.config.web_tools.caps.workspace_monthly;
        crate::web::caps::add_usage(&ctx.kv, &ctx.workspace_id, kyomi_core::web_tools::WebToolKind::Search, cap).await.unwrap();
        assert!(agent.request_options_for_call(true).await.server_tools.is_empty(), "spent at the start of the turn: withheld");
    }

    #[tokio::test]
    async fn openrouter_usage_and_sources_are_recorded_from_the_live_shaped_response() {
        let raw: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(format!("{}/src/web/fixtures/openrouter_search_chat_completion.json", env!("CARGO_MANIFEST_DIR"))).unwrap(),
        )
        .unwrap();
        let response = crate::types::LLMResponse {
            raw_response: raw,
            content: "answer".into(),
            finish_reason: "stop".into(),
            usage: Default::default(),
            tool_calls: None,
            cost: None,
            thinking_content: None,
            call_detail: None,
        };
        let mut config = server_tool_agent_config();
        config.server_tools[0].backend = kyomi_core::web_tools::WebBackendKind::OpenRouter;
        let (mut agent, _calls) = scripted_agent_with_config(config, vec![Reply::Response(Box::new(response))], Reply::Text("x".into())).await;
        let answer = agent.chat("hi", CancellationToken::new(), None, None, None).await.unwrap();
        assert!(answer.contains("**Sources**") && answer.contains("blog.rust-lang.org"), "{answer}");
        let ctx = &agent.tool_context;
        let caps = kyomi_core::web_tools::WebCaps { workspace_monthly: 1, ..Default::default() };
        let refusal = crate::web::caps::check_monthly(&ctx.kv, &caps, &ctx.workspace_id, kyomi_core::web_tools::WebToolKind::Search).await.unwrap();
        assert!(refusal.is_err());
    }

    // -- review fixes (KYO-969) -------------------------------------------

    fn live_pause_response() -> crate::types::LLMResponse {
        let raw: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(format!("{}/src/web/fixtures/anthropic_pause_turn_message.json", env!("CARGO_MANIFEST_DIR"))).unwrap(),
        )
        .unwrap();
        crate::types::LLMResponse {
            raw_response: raw,
            content: "Fetching them one at a time.".into(),
            finish_reason: "pause_turn".into(),
            usage: Default::default(),
            tool_calls: None,
            cost: None,
            thinking_content: None,
            call_detail: None,
        }
    }

    fn fetch_agent_config() -> AgentConfig {
        let mut config = server_tool_agent_config();
        config.server_tools[0].kind = kyomi_core::web_tools::WebToolKind::Fetch;
        config.server_tools[0].entry = serde_json::json!({"type": "web_fetch_20250910", "name": "web_fetch", "max_uses": 5});
        config
    }

    /// The live pause used 10 fetches (its capture request allowed 25); give the
    /// agent's run allowance room for that so the tool is not withheld.
    fn allow_many_fetches(agent: &mut CustomAgent) {
        let mut config = (*agent.tool_context.config).clone();
        config.web_tools.caps.per_turn_fetch = 50;
        agent.tool_context.config = Arc::new(config);
    }

    #[tokio::test]
    async fn live_captured_pause_turn_continues_with_the_paused_content_unchanged() {
        let paused = live_pause_response();
        let verbatim = paused.raw_response["content"].as_array().unwrap().clone();
        let (mut agent, calls) = scripted_agent_with_config(fetch_agent_config(), vec![Reply::Response(Box::new(paused))], Reply::Text("all done".into())).await;
        allow_many_fetches(&mut agent);
        let answer = agent.chat("go", CancellationToken::new(), None, None, None).await.unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].messages.last().unwrap().provider_content.as_ref().unwrap(), &verbatim);
        assert!(answer.starts_with("all done"));
    }

    #[tokio::test]
    async fn a_turn_still_paused_after_the_bound_fails_and_keeps_nothing_of_its_paused_chain() {
        let (mut agent, calls) = scripted_agent_with_config(
            AgentConfig { max_iterations: 50, ..fetch_agent_config() },
            vec![],
            Reply::Response(Box::new(live_pause_response())),
        )
        .await;
        allow_many_fetches(&mut agent);
        let error = agent.chat("go", CancellationToken::new(), None, None, None).await.expect_err("the run fails");
        assert!(error.to_string().contains("Web research did not finish"), "{error}");
        assert_eq!(calls.lock().unwrap().len() as u32, MAX_PAUSE_CONTINUATIONS + 1);
        assert!(
            agent.state().messages.iter().all(|m| m.role != crate::types::MessageRole::Assistant && m.provider_content.is_none()),
            "none of the paused messages is kept: {:?}", agent.state().messages.iter().map(|m| (m.role.clone(), m.content.len())).collect::<Vec<_>>()
        );
    }

    /// Whatever ends a run inside a `pause_turn` chain (a provider error, the
    /// iteration limit), the run fails and keeps none of the paused chain.
    #[tokio::test]
    async fn any_failure_inside_a_pause_chain_keeps_none_of_the_chain() {
        // A provider error after two pauses.
        let (mut agent, _calls) = scripted_agent_with_config(
            fetch_agent_config(),
            vec![Reply::Response(Box::new(live_pause_response())), Reply::Response(Box::new(live_pause_response())), Reply::Failure("provider down".into())],
            Reply::Text("unused".into()),
        )
        .await;
        let error = agent.chat("go", CancellationToken::new(), None, None, None).await.expect_err("fails");
        assert!(error.to_string().contains("provider down"), "{error}");
        assert!(agent.state().messages.iter().all(|m| m.role != crate::types::MessageRole::Assistant), "error: nothing of the chain kept");

        // The iteration limit reached while still paused.
        let (mut agent, _calls) = scripted_agent_with_config(
            AgentConfig { max_iterations: 3, ..fetch_agent_config() },
            vec![],
            Reply::Response(Box::new(live_pause_response())),
        )
        .await;
        let error = agent.chat("go", CancellationToken::new(), None, None, None).await.expect_err("fails");
        assert!(error.to_string().contains("Web research did not finish"), "{error}");
        assert!(agent.state().messages.iter().all(|m| m.role != crate::types::MessageRole::Assistant), "limit: nothing of the chain kept");
    }

    /// A ChartML-invalid final answer after a pause chain: its blocks resolve the
    /// chain's open call and are stored with the chain; a failure afterwards keeps
    /// none of it.
    #[tokio::test]
    async fn a_chartml_retry_after_a_pause_chain_keeps_the_chain_resolved() {
        let paused = live_pause_response();
        let mut invalid = {
            let raw: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(format!("{}/src/web/fixtures/anthropic_pause_turn_continuation_message.json", env!("CARGO_MANIFEST_DIR"))).unwrap(),
            )
            .unwrap();
            crate::anthropic::AnthropicClient::parse_response("claude", &raw, None).unwrap()
        };
        invalid.content = "```\n```chartml\ntype: chart\nversion: 99".into();
        let expected: Vec<serde_json::Value> = paused.raw_response["content"].as_array().unwrap().iter()
            .chain(invalid.raw_response["content"].as_array().unwrap())
            .cloned()
            .collect();

        let (mut agent, _calls) = scripted_agent_with_config(
            fetch_agent_config(),
            vec![Reply::Response(Box::new(paused.clone())), Reply::Response(Box::new(invalid.clone())), Reply::Text("Corrected answer".into())],
            Reply::Text("unused".into()),
        )
        .await;
        let answer = agent.chat("go", CancellationToken::new(), None, None, None).await.unwrap();
        assert!(answer.starts_with("Corrected answer"), "{answer}");
        let stored: Vec<serde_json::Value> = agent.state().messages.iter()
            .filter(|m| m.role == crate::types::MessageRole::Assistant)
            .filter_map(|m| m.provider_content.clone())
            .flatten()
            .collect();
        assert_eq!(stored, expected, "the chain's blocks, the invalid answer's included, exactly as received");

        // A failure after the invalid answer keeps nothing of the chain.
        let (mut agent, _calls) = scripted_agent_with_config(
            fetch_agent_config(),
            vec![Reply::Response(Box::new(paused)), Reply::Response(Box::new(invalid)), Reply::Failure("provider down".into())],
            Reply::Text("unused".into()),
        )
        .await;
        agent.chat("go", CancellationToken::new(), None, None, None).await.expect_err("fails");
        assert!(agent.state().messages.iter().all(|m| m.role != crate::types::MessageRole::Assistant));
    }

    #[tokio::test]
    async fn a_pause_continuation_carries_the_same_tools_even_after_the_allowance_is_used() {
        let (mut agent, calls) = scripted_agent_with_config(
            server_tool_agent_config(),
            vec![Reply::Response(Box::new(server_response("pause_turn", None)))],
            Reply::Text("done".into()),
        )
        .await;
        agent.chat("hi", CancellationToken::new(), None, None, None).await.unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].server_tools.len(), 1);
        assert_eq!(calls[1].server_tools, calls[0].server_tools, "the continuation carries the first request's tools");
    }

    /// A bare `pause_turn` message (blocks, no text) is not stored as a row of its
    /// own: its blocks are carried, in order, into the next assistant message.
    #[tokio::test]
    async fn a_bare_pause_message_is_carried_into_the_next_message_not_dropped() {
        let mut paused = server_response("pause_turn", None);
        paused.content = String::new();
        let first_blocks = paused.raw_response["content"].as_array().unwrap().clone();
        let (mut agent, _calls) = scripted_agent_with_config(
            server_tool_agent_config(),
            vec![Reply::Response(Box::new(paused)), Reply::Response(Box::new(server_response("end_turn", None)))],
            Reply::Text("unused".into()),
        )
        .await;
        agent.chat("hi", CancellationToken::new(), None, None, None).await.unwrap();
        let assistants: Vec<_> = agent.state().messages.iter().filter(|m| m.role == crate::types::MessageRole::Assistant).collect();
        assert_eq!(assistants.len(), 1, "the bare pause message is not a row of its own");
        let second_blocks = server_response("end_turn", None).raw_response["content"].as_array().unwrap().clone();
        let expected: Vec<_> = first_blocks.into_iter().chain(second_blocks).collect();
        assert_eq!(assistants[0].provider_content.as_ref(), Some(&expected), "paused blocks first, then the final's, exactly as received");
    }

    #[tokio::test]
    async fn a_pause_continuation_is_never_followed_by_the_budget_notice_or_retry_messages() {
        // 10 iterations: the 70% budget notice is due by iteration 7; the pause happens late.
        let mut script: Vec<Reply> = vec![Reply::ToolCall; 7];
        script.push(Reply::Response(Box::new(live_pause_response())));
        let (mut agent, calls) = scripted_agent_with_config(
            AgentConfig { max_iterations: 10, ..fetch_agent_config() },
            script,
            Reply::Text("done".into()),
        )
        .await;
        allow_many_fetches(&mut agent);
        agent.chat("go", CancellationToken::new(), None, None, None).await.unwrap();
        let calls = calls.lock().unwrap();
        let last_call = calls.last().unwrap();
        let continuation = &calls[calls.len() - 1].messages;
        assert!(continuation.last().unwrap().provider_content.is_some(), "ends with the paused message: {last_call:?}");
        assert!(continuation.iter().filter(|m| m.content.contains(crate::agent::BUDGET_NOTICE_PREFIX)).count() == 0);
    }

    #[tokio::test]
    async fn the_forced_wrap_up_call_sends_no_server_tools_and_cites_sources() {
        // Exhaust the iteration budget with tool calls; one of them carries a server-tool search.
        let mixed = server_response("tool_use", Some(vec![ToolCall { id: "tc_1".into(), name: crate::test_support::NOOP_TOOL_NAME.into(), arguments: serde_json::json!({}), arguments_error: None }]));
        let seen = Arc::new(Mutex::new(Vec::<usize>::new()));
        struct Spy(Arc<Mutex<Vec<usize>>>, ScriptedProvider);
        #[async_trait::async_trait]
        impl crate::provider::LLMProvider for Spy {
            async fn complete(&self, m: &[Message], t: &[Tool], o: &crate::provider::RequestOptions, u: &HashMap<String, String>) -> kyomi_core::Result<crate::types::LLMResponse> {
                self.0.lock().unwrap().push(o.server_tools.len());
                self.1.complete(m, t, o, u).await
            }
            fn model(&self) -> &str { "spy" }
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let inner = ScriptedProvider { script: Mutex::new(vec![Reply::Response(Box::new(mixed))].into()), after_script: Reply::ToolCall, calls: Arc::clone(&calls) };
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(NoopTool));
        let mut agent = CustomAgent::new(
            Box::new(Spy(Arc::clone(&seen), inner)),
            AgentConfig { max_iterations: 3, ..server_tool_agent_config() },
            Arc::new(registry),
            test_tool_context().await,
            HashMap::new(),
        );
        let _ = agent.chat("hi", CancellationToken::new(), None, None, None).await;
        let seen = seen.lock().unwrap();
        assert_eq!(*seen.last().unwrap(), 0, "wrap-up must not send provider web tools: {seen:?}");
        assert!(seen[0] == 1, "ordinary calls do: {seen:?}");
    }

    #[tokio::test]
    async fn the_per_turn_allowance_is_spent_across_requests_but_a_continuation_resends_its_tools() {
        let (mut agent, _calls) = scripted_agent_with_config(server_tool_agent_config(), vec![], Reply::Text("x".into())).await;
        let first = agent.request_options_for_call(true).await;
        assert_eq!(first.server_tools[0]["max_uses"], 5);
        agent.web_usage.insert(kyomi_core::web_tools::WebToolKind::Search, 3);
        assert_eq!(agent.request_options_for_call(false).await.server_tools[0]["max_uses"], 2, "only what is left");
        // A `pause_turn` continuation resends the tools of the request it continues,
        // whatever the allowance now says (the provider rejects one without them).
        agent.pause_chain_start = Some(0);
        agent.web_usage.insert(kyomi_core::web_tools::WebToolKind::Search, 5);
        let continuation = agent.request_options_for_call(false).await;
        assert_eq!(continuation.server_tools[0]["max_uses"], 2, "same tools as the request being continued");
        agent.pause_chain_start = None;
        assert!(agent.request_options_for_call(false).await.server_tools.is_empty(), "spent: left out of an ordinary request");
        // The next turn finds the allowance spent at its start.
        assert!(agent.request_options_for_call(true).await.server_tools.is_empty());
    }

    fn openrouter_server_tool_agent_config() -> AgentConfig {
        AgentConfig {
            max_iterations: 6,
            server_tools: vec![crate::web::server_tools::ServerTool {
                kind: kyomi_core::web_tools::WebToolKind::Search,
                backend: kyomi_core::web_tools::WebBackendKind::OpenRouter,
                entry: serde_json::json!({"type": "openrouter:web_search", "parameters": {"max_uses": 5}}),
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_session_holding_personal_data_still_gets_the_anthropic_provider_web_tool() {
        // KYO-981 (owner decision): provider-run web tools are no longer
        // withheld from a session that holds restricted personal-MCP data.
        let (mut agent, _calls) = scripted_agent_with_config(server_tool_agent_config(), vec![], Reply::Text("x".into())).await;
        crate::test_support::seed_user_and_workspace(&agent.tool_context.db).await;
        crate::test_support::seed_restricted_session(&agent.tool_context.db, "s-web").await;
        agent.tool_context.session_id = Some("s-web".into());
        let options = agent.request_options_for_call(false).await;
        assert_eq!(options.server_tools.len(), 1, "restricted session keeps its Anthropic web tool");
        assert_eq!(options.server_tools[0]["type"], "web_search_20250305");
    }

    #[tokio::test]
    async fn a_session_holding_personal_data_still_gets_the_openrouter_provider_web_tool() {
        let (mut agent, _calls) = scripted_agent_with_config(openrouter_server_tool_agent_config(), vec![], Reply::Text("x".into())).await;
        crate::test_support::seed_user_and_workspace(&agent.tool_context.db).await;
        crate::test_support::seed_restricted_session(&agent.tool_context.db, "s-web").await;
        agent.tool_context.session_id = Some("s-web".into());
        let options = agent.request_options_for_call(false).await;
        assert_eq!(options.server_tools.len(), 1, "restricted session keeps its OpenRouter web tool");
        assert_eq!(options.server_tools[0]["type"], "openrouter:web_search");
    }

    #[tokio::test]
    async fn the_durable_runtime_path_keeps_the_raw_provider_response_for_blocks_usage_and_storage() {
        use agent_runtime::CompleteProvider;
        let paused = live_pause_response();
        let raw = paused.raw_response.clone();
        let (agent, _calls) = scripted_agent_with_config(fetch_agent_config(), vec![Reply::Response(Box::new(paused))], Reply::Text("x".into())).await;
        let provider = RuntimeProvider { agent: &agent, messages: &[], tools: &[], allow_server_tools: true };
        let (response, record) = provider.complete().await.unwrap();
        assert_eq!(response.raw_response, raw, "the loop reads server blocks, usage and sources from this");
        assert_eq!(record.raw, raw, "the restricted store keeps the complete provider payload");
        assert_eq!(record.continuation, raw);
    }

    /// A tool that restricts the session while the run is in progress (as a private
    /// document read does), to exercise a restriction that appears mid-run.
    struct RestrictSessionTool;
    #[async_trait::async_trait]
    impl crate::tools::AgentTool for RestrictSessionTool {
        fn name(&self) -> &str { "list_datasources" }
        fn description(&self) -> &str { "restricts the session" }
        fn parameters_schema(&self) -> serde_json::Value { serde_json::json!({"type": "object", "properties": {}}) }
        async fn execute(&self, _args: serde_json::Value, ctx: &crate::tools::ToolContext) -> kyomi_core::Result<String> {
            kyomi_core::db_execute!(&ctx.db, "INSERT INTO personal_mcp_private_contexts (owner_user_id,workspace_id) VALUES ('user-a','ws-1')").unwrap();
            kyomi_auth::personal_mcp_privacy::inherit_private_document_context(&ctx.db, "user-a", "ws-1", Some("s-late"), None).await.unwrap();
            Ok("restricted".into())
        }
    }

    #[tokio::test]
    async fn sources_gathered_before_a_mid_run_restriction_are_still_shown_and_later_requests_keep_the_tool() {
        // Sources are inbound web data, not outbound private data: showing them discloses
        // nothing. A restriction that appears mid-run does not withhold the tool either.
        let tool_call = ToolCall { id: "tc_1".into(), name: "list_datasources".into(), arguments: serde_json::json!({}), arguments_error: None };
        let first = server_response("tool_use", Some(vec![tool_call]));
        let seen = Arc::new(Mutex::new(Vec::<usize>::new()));
        struct Spy(Arc<Mutex<Vec<usize>>>, ScriptedProvider);
        #[async_trait::async_trait]
        impl crate::provider::LLMProvider for Spy {
            async fn complete(&self, m: &[Message], t: &[Tool], o: &crate::provider::RequestOptions, u: &HashMap<String, String>) -> kyomi_core::Result<crate::types::LLMResponse> {
                self.0.lock().unwrap().push(o.server_tools.len());
                self.1.complete(m, t, o, u).await
            }
            fn model(&self) -> &str { "spy" }
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let inner = ScriptedProvider { script: Mutex::new(vec![Reply::Response(Box::new(first))].into()), after_script: Reply::Text("final".into()), calls };
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(RestrictSessionTool));
        let ctx = test_tool_context().await;
        crate::test_support::seed_user_and_workspace(&ctx.db).await;
        kyomi_core::db_execute!(&ctx.db, "INSERT INTO chat_sessions (session_id,user_id,workspace_id,session_type) VALUES ('s-late','user-a','ws-1','chat')").unwrap();
        let mut ctx = ctx;
        ctx.session_id = Some("s-late".into());
        let mut agent = CustomAgent::new(Box::new(Spy(Arc::clone(&seen), inner)), server_tool_agent_config(), Arc::new(registry), ctx, HashMap::new());
        let answer = agent.chat("hi", CancellationToken::new(), None, None, None).await.unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![1, 1], "both requests carry the tool, including the one after the restriction");
        assert!(answer.starts_with("final") && answer.contains("**Sources**"), "{answer}");
    }

    #[tokio::test]
    async fn each_provider_web_call_is_logged_with_latency_result_count_and_remaining_quota() {
        let capture = kyomi_test_tracing::capture_tracing();
        let (mut agent, _calls) = scripted_agent_with_config(
            server_tool_agent_config(),
            vec![Reply::Response(Box::new(server_response("end_turn", None)))],
            Reply::Text("x".into()),
        )
        .await;
        agent.chat("hi", CancellationToken::new(), None, None, None).await.unwrap();
        let caps = agent.tool_context.config.web_tools.caps.clone();
        let line = capture
            .events_at(tracing::Level::INFO)
            .into_iter()
            .map(|(_, message)| message)
            .find(|message| message.contains("provider web tool used"))
            .expect("one structured line per provider web call");
        for expected in [
            "backend=anthropic".to_string(),
            "tool=\"web_search\"".to_string(),
            "billable_calls=1".to_string(),
            "result_count=1".to_string(),
            "latency_ms=".to_string(),
            format!("workspace_remaining=Some({})", caps.workspace_monthly - 1),
            format!("global_remaining=Some({})", caps.global_monthly - 1),
        ] {
            assert!(line.contains(&expected), "{expected} in {line}");
        }
    }

    // -----------------------------------------------------------------------
    // Compaction trigger (KYO-975)
    // -----------------------------------------------------------------------

    fn compaction_runtime(
        threshold_tokens: u32,
        script: Vec<Reply>,
        calls: Arc<Mutex<Vec<RecordedCall>>>,
    ) -> crate::compaction::CompactionRuntime {
        crate::compaction::CompactionRuntime {
            threshold_tokens,
            context_window: 200_000,
            client: Box::new(ScriptedProvider {
                script: Mutex::new(script.into()),
                after_script: Reply::Text("unused summary".into()),
                calls,
            }),
            options: crate::provider::RequestOptions::new(512),
        }
    }

    #[tokio::test]
    async fn the_run_compacts_at_the_threshold_and_the_next_call_uses_the_summary() {
        let summary_calls = Arc::new(Mutex::new(Vec::new()));
        let (mut agent, calls) = scripted_agent(
            10,
            vec![
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(1_000)),
                Reply::Text("final answer".into()),
            ],
            Reply::Text("unexpected".into()),
        )
        .await;
        agent.set_compaction(compaction_runtime(
            1_000,
            vec![Reply::Text("the earlier turns were about revenue".into())],
            Arc::clone(&summary_calls),
        ));
        // Mirror the production call path (`execute_agent_chat` inserts the
        // system prompt before the conversation).
        agent
            .state_mut()
            .messages
            .push(Message::system("system prompt"));

        let answer = agent
            .chat("first question", CancellationToken::new(), None, None, None)
            .await
            .expect("the run compacts and completes");
        assert_eq!(answer, "final answer");

        let summaries = summary_calls.lock().expect("summary call log");
        assert_eq!(summaries.len(), 1, "exactly one summary call");
        assert_eq!(
            summaries[0].messages.first().expect("older turns").content,
            "first question"
        );
        assert_eq!(
            summaries[0].messages.last().expect("instruction").content,
            crate::compaction::COMPACTION_INSTRUCTION
        );

        let state = agent.state();
        assert_eq!(
            state.compacted_summary.as_deref(),
            Some("the earlier turns were about revenue")
        );
        assert_eq!(
            state.messages_since_compaction_index, 2,
            "system + the one summarised user message"
        );

        let recorded = std::mem::take(&mut *calls.lock().expect("call log mutex"));
        let final_call = recorded.last().expect("final call");
        assert!(
            final_call
                .messages
                .iter()
                .any(|message| message.content.contains("## Prior Conversation Context")),
            "the post-compaction call carries the summary: {:?}",
            final_call.messages
        );
        assert!(
            final_call.messages.iter().all(|message| message.content != "first question"),
            "the summarised turn is no longer sent: {:?}",
            final_call.messages
        );
    }

    #[tokio::test]
    async fn a_run_below_the_threshold_does_not_compact() {
        let summary_calls = Arc::new(Mutex::new(Vec::new()));
        let (mut agent, _calls) = scripted_agent(
            10,
            vec![
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::Text("final answer".into()),
            ],
            Reply::Text("unexpected".into()),
        )
        .await;
        agent.set_compaction(compaction_runtime(
            1_000,
            vec![Reply::Text("should not be used".into())],
            Arc::clone(&summary_calls),
        ));
        let background = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&background);
        agent.callbacks_mut().on_progress = Some(Box::new(move |progress| {
            let sink = Arc::clone(&sink);
            Box::pin(async move {
                if let AgentProgress::BackgroundUsage(input, output, cost) = progress {
                    sink.lock().expect("progress log").push((input, output, cost));
                }
            })
        }));

        agent
            .chat("first question", CancellationToken::new(), None, None, None)
            .await
            .expect("the run completes");

        assert!(summary_calls.lock().expect("summary call log").is_empty());
        assert_eq!(agent.state().compacted_summary, None);
        assert_eq!(agent.state().messages_since_compaction_index, 0);
        assert!(
            background.lock().expect("progress log").is_empty(),
            "a run that does not compact reports no auxiliary usage"
        );
    }

    #[tokio::test]
    async fn a_second_check_does_not_recompact_after_the_context_shrank() {
        let summary_calls = Arc::new(Mutex::new(Vec::new()));
        let (mut agent, _calls) = scripted_agent(1, vec![], Reply::Text("unused".into())).await;
        agent.set_compaction(compaction_runtime(
            1_000,
            vec![
                Reply::Text("short summary".into()),
                Reply::Text("short summary two".into()),
            ],
            Arc::clone(&summary_calls),
        ));
        {
            let state = agent.state_mut();
            state.messages.push(Message::system("system prompt"));
            for _ in 0..14 {
                state.messages.push(Message::user("x".repeat(4_000)));
            }
            for _ in 0..6 {
                state.messages.push(Message::user("y".repeat(100)));
            }
            state.last_input_tokens = 1_000;
        }

        agent
            .maybe_compact(&CancellationToken::new())
            .await
            .expect("the first compaction succeeds");
        assert!(agent.state().compacted_summary.is_some());
        assert_eq!(
            agent.state().messages_since_compaction_index,
            15,
            "system + the 14 summarised messages"
        );
        assert_eq!(summary_calls.lock().expect("summary call log").len(), 1);

        // New turns after the compaction grow the kept tail beyond
        // MESSAGES_TO_KEEP, so the next check has something it could summarise.
        // The kept tail (~10 short messages + the summary) stays well below the
        // threshold; a check over the raw state would still see the 14 large
        // summarised messages and fire again.
        {
            let state = agent.state_mut();
            for _ in 0..4 {
                state.messages.push(Message::user("z".repeat(100)));
            }
        }

        agent
            .maybe_compact(&CancellationToken::new())
            .await
            .expect("the second check succeeds");
        assert_eq!(
            summary_calls.lock().expect("summary call log").len(),
            1,
            "the summarized-away messages must not re-trigger compaction"
        );
    }

    #[tokio::test]
    async fn an_open_pause_chain_is_not_compacted() {
        let summary_calls = Arc::new(Mutex::new(Vec::new()));
        let (mut agent, _calls) = scripted_agent(1, vec![], Reply::Text("unused".into())).await;
        agent.set_compaction(compaction_runtime(
            1_000,
            vec![Reply::Text("must not be used".into())],
            Arc::clone(&summary_calls),
        ));
        {
            let state = agent.state_mut();
            state.messages.push(Message::system("system prompt"));
            for _ in 0..14 {
                state.messages.push(Message::user("x".repeat(4_000)));
            }
            state.last_input_tokens = 1_000;
        }
        agent.pause_chain_start = Some(agent.state().messages.len() - 1);

        let (context, summary_tokens) = agent
            .maybe_compact(&CancellationToken::new())
            .await
            .expect("an open chain is simply left alone");
        assert_eq!(summary_tokens, 0);
        assert!(summary_calls.lock().expect("summary call log").is_empty());
        assert!(context.len() >= agent.state().messages.len());
        assert_eq!(agent.state().compacted_summary, None);
    }

    #[tokio::test]
    async fn a_second_compaction_advances_the_cut_index_and_replaces_the_summary() {
        let summary_calls = Arc::new(Mutex::new(Vec::new()));
        let (mut agent, _calls) = scripted_agent(1, vec![], Reply::Text("unused".into())).await;
        agent.set_compaction(compaction_runtime(
            1_000,
            vec![Reply::Text("second summary".into())],
            Arc::clone(&summary_calls),
        ));
        {
            let state = agent.state_mut();
            state.messages.push(Message::system("system prompt"));
            for i in 0..10 {
                state.messages.push(Message::user(format!("old {i}")));
            }
            state.compacted_summary = Some("first summary".into());
            state.messages_since_compaction_index = 11; // system + 10 old
            for i in 0..10 {
                state.messages.push(Message::user(format!("recent {i} {}", "y".repeat(500))));
            }
            state.last_input_tokens = 1_000;
        }

        let (context, _summary_tokens) = agent
            .maybe_compact(&CancellationToken::new())
            .await
            .expect("the second compaction succeeds");
        assert_eq!(summary_calls.lock().expect("summary call log").len(), 1);
        assert_eq!(
            agent.state().compacted_summary.as_deref(),
            Some("second summary")
        );
        assert_eq!(
            agent.state().messages_since_compaction_index, 15,
            "the old index (11) advances over the 4 recent messages summarised"
        );
        assert!(context.iter().any(|m| m.content.contains("second summary")));
    }

    #[tokio::test]
    async fn a_stored_web_block_reaches_the_summary_request_unchanged() {
        let block = serde_json::json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srv1",
            "content": [{"type": "web_search_result", "url": "https://example.com/q4"}]
        });
        let summary_calls = Arc::new(Mutex::new(Vec::new()));
        let (mut agent, _calls) = scripted_agent(1, vec![], Reply::Text("unused".into())).await;
        agent.set_compaction(compaction_runtime(
            1_000,
            vec![Reply::Text("a summary".into())],
            Arc::clone(&summary_calls),
        ));
        {
            let state = agent.state_mut();
            state.messages.push(Message::system("system prompt"));
            for i in 0..14 {
                let message = if i == 2 {
                    Message::assistant("search findings").with_provider_content(Some(vec![block.clone()]))
                } else {
                    Message::user(format!("turn {i}"))
                };
                state.messages.push(message);
            }
            state.last_input_tokens = 1_000;
        }

        agent
            .maybe_compact(&CancellationToken::new())
            .await
            .expect("the summary call succeeds");
        let calls = summary_calls.lock().expect("summary call log");
        assert_eq!(calls.len(), 1);
        let summary = &calls[0].messages;
        assert!(
            summary.iter().any(|message| message
                .provider_content
                .as_ref()
                .is_some_and(|blocks| blocks.contains(&block))),
            "the stored web block reaches the summary request unchanged: {summary:?}"
        );
        assert_eq!(
            summary.last().expect("instruction appended").content,
            crate::compaction::COMPACTION_INSTRUCTION,
            "the only added text is the summary instruction"
        );
    }

    #[tokio::test]
    async fn a_summary_model_too_small_fails_the_run_loudly() {
        struct SmallWindowClient;
        #[async_trait::async_trait]
        impl LLMProvider for SmallWindowClient {
            async fn complete(
                &self,
                _messages: &[Message],
                _tools: &[Tool],
                _options: &crate::provider::RequestOptions,
                _user_names: &HashMap<String, String>,
            ) -> kyomi_core::Result<crate::types::LLMResponse> {
                panic!("the fit check must reject the region before calling the model")
            }
            fn model(&self) -> &str {
                "small-window-summariser"
            }
            fn context_window(&self) -> u32 {
                10
            }
        }

        let (mut agent, _calls) = scripted_agent(
            10,
            vec![
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(1_000)),
                Reply::Text("should not be reached".into()),
            ],
            Reply::Text("unexpected".into()),
        )
        .await;
        agent.state_mut().messages.push(Message::system("system prompt"));
        agent.set_compaction(crate::compaction::CompactionRuntime {
            threshold_tokens: 1_000,
            context_window: 200_000,
            client: Box::new(SmallWindowClient),
            options: crate::provider::RequestOptions::new(512),
        });

        let error = agent
            .chat("first question", CancellationToken::new(), None, None, None)
            .await
            .expect_err("a summary model that cannot fit the region fails the run");
        let message = error.to_string();
        assert!(message.contains("summary model cannot accept"), "{message}");
        assert!(message.contains("window=10"), "{message}");
    }

    #[tokio::test]
    async fn a_kept_tail_over_the_threshold_fails_before_a_summary() {
        let summary_calls = Arc::new(Mutex::new(Vec::new()));
        let (mut agent, _calls) = scripted_agent(1, vec![], Reply::Text("unused".into())).await;
        agent.set_compaction(compaction_runtime(
            100,
            vec![Reply::Text("short summary".into())],
            Arc::clone(&summary_calls),
        ));
        {
            let state = agent.state_mut();
            state.messages.push(Message::system("system prompt"));
            for _ in 0..14 {
                state.messages.push(Message::user("x".repeat(4_000)));
            }
            // The kept tail itself is ~6,000 estimate tokens, far over 100.
            for _ in 0..6 {
                state.messages.push(Message::user("y".repeat(4_000)));
            }
            state.last_input_tokens = 1_000;
        }

        let error = agent
            .maybe_compact(&CancellationToken::new())
            .await
            .expect_err("a kept tail that is already over the threshold fails");
        let message = error.to_string();
        assert!(message.contains("kept turns alone"), "{message}");
        assert!(message.contains("threshold=100"), "{message}");
        assert!(
            summary_calls.lock().expect("summary log").is_empty(),
            "no summary call is paid for when the kept tail cannot fit"
        );
    }

    #[tokio::test]
    async fn a_summary_that_still_leaves_the_context_over_the_threshold_fails_after_the_call() {
        let summary_calls = Arc::new(Mutex::new(Vec::new()));
        let (mut agent, _calls) = scripted_agent(1, vec![], Reply::Text("unused".into())).await;
        agent.set_compaction(compaction_runtime(
            100,
            vec![Reply::Text("s".repeat(1_000))],
            Arc::clone(&summary_calls),
        ));
        {
            let state = agent.state_mut();
            state.messages.push(Message::system("system prompt"));
            for _ in 0..8 {
                state.messages.push(Message::user("x".repeat(400)));
            }
            // The kept tail is small enough for the pre-call check; a summary
            // larger than the threshold still pushes the rebuilt context over.
            for _ in 0..6 {
                state.messages.push(Message::user("y".repeat(20)));
            }
            state.last_input_tokens = 1_000;
        }

        let error = agent
            .maybe_compact(&CancellationToken::new())
            .await
            .expect_err("a summary that cannot get under the threshold fails");
        let message = error.to_string();
        assert!(
            message.contains("still over the compaction threshold after summarising"),
            "{message}"
        );
        assert!(message.contains("threshold=100"), "{message}");
        assert_eq!(summary_calls.lock().expect("summary log").len(), 1);
    }

    #[tokio::test]
    async fn a_context_over_the_threshold_with_nothing_to_summarise_fails_the_run() {
        let summary_calls = Arc::new(Mutex::new(Vec::new()));
        let (mut agent, _calls) = scripted_agent(1, vec![], Reply::Text("unused".into())).await;
        agent.set_compaction(compaction_runtime(
            100,
            vec![Reply::Text("must not be used".into())],
            Arc::clone(&summary_calls),
        ));
        {
            let state = agent.state_mut();
            state.messages.push(Message::system("system prompt"));
            for _ in 0..5 {
                state.messages.push(Message::user("x".repeat(4_000)));
            }
            state.last_input_tokens = 1_000;
        }

        let error = agent
            .maybe_compact(&CancellationToken::new())
            .await
            .expect_err("nothing can be summarised, so the run fails instead of looping");
        let message = error.to_string();
        assert!(message.contains("no older turns can be summarised"), "{message}");
        assert!(message.contains("threshold=100"), "{message}");
        assert!(summary_calls.lock().expect("summary log").is_empty());
    }

    #[tokio::test]
    async fn the_summary_usage_is_reported_as_background_usage() {
        let summary_usage = crate::types::AgentTokenUsage {
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_input_tokens: 10,
            cache_read_input_tokens: 5,
            reasoning_tokens: 0,
        };
        let summary_response = crate::types::LLMResponse {
            raw_response: serde_json::Value::Null,
            content: "summary of the earlier turns".into(),
            finish_reason: "end_turn".into(),
            usage: summary_usage,
            tool_calls: None,
            cost: Some(0.5),
            thinking_content: None,
            call_detail: None,
        };
        let (mut agent, _calls) = scripted_agent(
            10,
            vec![
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(1_000)),
                Reply::Text("final".into()),
            ],
            Reply::Text("unexpected".into()),
        )
        .await;
        agent.state_mut().messages.push(Message::system("system prompt"));
        agent.set_compaction(compaction_runtime(
            1_000,
            vec![Reply::Response(Box::new(summary_response))],
            Arc::new(Mutex::new(Vec::new())),
        ));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        agent.callbacks_mut().on_progress = Some(Box::new(move |progress| {
            let sink = Arc::clone(&sink);
            Box::pin(async move {
                if let AgentProgress::BackgroundUsage(input, output, cost) = progress {
                    sink.lock().expect("progress log").push((input, output, cost));
                }
            })
        }));

        let answer = agent
            .chat("first question", CancellationToken::new(), None, None, None)
            .await
            .expect("the run completes");
        assert_eq!(answer, "final");
        assert_eq!(
            *seen.lock().expect("progress log"),
            vec![(115, 20, Some(0.5))],
            "the summary's billable input/output/cost are reported without changing context tokens"
        );
    }

    #[tokio::test]
    async fn a_failed_summary_fails_the_run_loudly() {
        let (mut agent, _calls) = scripted_agent(
            10,
            vec![
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(1_000)),
                Reply::Text("should not be reached".into()),
            ],
            Reply::Text("unexpected".into()),
        )
        .await;
        agent.set_compaction(compaction_runtime(
            1_000,
            vec![Reply::Failure("summary model unavailable".into())],
            Arc::new(Mutex::new(Vec::new())),
        ));

        let error = agent
            .chat("first question", CancellationToken::new(), None, None, None)
            .await
            .expect_err("a failed summary fails the run");
        assert!(error.to_string().contains("summary model unavailable"), "{error}");
    }

    #[tokio::test]
    async fn an_empty_summary_fails_the_run_loudly() {
        let (mut agent, _calls) = scripted_agent(
            10,
            vec![
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(1_000)),
                Reply::Text("should not be reached".into()),
            ],
            Reply::Text("unexpected".into()),
        )
        .await;
        agent.set_compaction(compaction_runtime(
            1_000,
            vec![Reply::Text("   ".into())],
            Arc::new(Mutex::new(Vec::new())),
        ));

        let error = agent
            .chat("first question", CancellationToken::new(), None, None, None)
            .await
            .expect_err("an empty summary fails the run");
        assert!(error.to_string().contains("empty summary"), "{error}");
    }

    #[tokio::test]
    async fn the_summary_calls_tokens_count_toward_the_run_token_ceiling() {
        let summary_usage = billable_usage(100_000);
        let summary_response = crate::types::LLMResponse {
            raw_response: serde_json::Value::Null,
            content: "summary of the earlier turns".into(),
            finish_reason: "end_turn".into(),
            usage: summary_usage,
            tool_calls: None,
            cost: None,
            thinking_content: None,
            call_detail: None,
        };
        // Four tool rounds: the compaction fires at the top of the fourth
        // iteration, so its tokens are counted before the fifth iteration's
        // guard runs.
        let (mut agent, calls) = scripted_agent_with_config(
            AgentConfig {
                max_iterations: 25,
                max_duration: None,
                max_total_tokens: Some(100_000),
                ..Default::default()
            },
            vec![
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(10)),
                Reply::ToolCallWithUsage(billable_usage(1_000)),
                Reply::ToolCallWithUsage(billable_usage(10)),
            ],
            Reply::Text("Summary from partial data.".to_string()),
        )
        .await;
        agent.set_compaction(compaction_runtime(
            1_000,
            vec![Reply::Response(Box::new(summary_response))],
            Arc::new(Mutex::new(Vec::new())),
        ));
        // Mirror the production call path (the system prompt precedes the
        // conversation).
        agent.state_mut().messages.push(Message::system("system prompt"));

        let answer = agent
            .chat("first question", CancellationToken::new(), None, None, None)
            .await
            .expect("chat completes with the wrap-up");
        assert_eq!(answer, "Summary from partial data.");

        let recorded = std::mem::take(&mut *calls.lock().expect("call log mutex"));
        assert_eq!(
            recorded.len(),
            5,
            "4 loop calls (the last one after the summary) plus the wrap-up: {recorded:?}"
        );
        assert!(
            recorded.last().expect("has calls").tools.is_empty(),
            "the token ceiling fires after the summary call, producing a tool-less wrap-up"
        );
    }
}
