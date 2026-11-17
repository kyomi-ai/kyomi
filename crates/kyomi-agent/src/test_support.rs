// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared test fixtures for this crate's tool tests.
//!
//! [`test_pool`] and [`build_ctx`] were duplicated four times across this
//! crate before KYO-537 — `tools/catalog.rs` (twice, as `test_pool` /
//! `build_ctx`), `agent.rs`'s `test_tool_context`, `execution.rs`'s
//! `test_tool_context_for_roles`, and `tools/watch.rs`'s `unused_ctx` — see
//! `docs/CODING_STANDARDS.md`'s "third copy of a test helper is the
//! extraction trigger" rule. Database scaffolding now comes from
//! `kyomi_test_db`; this module composes its migrated in-memory SQLite
//! [`kyomi_core::DbPool`] into a fully-populated [`ToolContext`] for this
//! crate's tool tests. Tool-specific fixtures remain here.
//!
//! # What this harness covers
//!
//! [`test_pool`] runs the *real* `apps/server/migrations-sqlite` migration
//! set against a fresh `sqlite::memory:` database — every table and index a
//! production SQLite deployment would have, exercised through the actual
//! tool `execute()` path end-to-end (mirrors the pattern this crate already
//! used in `tools/catalog.rs` and `tools/watch.rs`'s `broadcast_routing`
//! tests).
//!
//! [`build_ctx`] populates every field of [`ToolContext`] with a workable
//! default: user `"user-a"` in workspace `"ws-1"`, an in-memory KV store, a
//! zeroed AES key, an *unloaded* [`kyomi_embed::LazyEmbedding`] (call
//! [`loaded_embedding`] and overwrite `ctx.embedding` for any tool whose
//! `execute()` calls `wait_ready()`), no Connect registry, and an empty
//! platform registry. Every field is `pub`, so a test that needs a
//! non-default value (a different `user_id`, `workspace_roles`, a real
//! `ws_manager` wired to a `WebSocketManager` under test) overwrites just
//! that field on the returned struct rather than reimplementing the other
//! thirteen.
//!
//! # What this harness does NOT cover
//!
//! - **Postgres.** Every pool here is SQLite; this harness does not exercise
//!   this crate's Postgres-only SQL arms (e.g. the pgvector `<=>` queries in
//!   `tools/knowledge.rs::search_knowledge_chunks`). See
//!   `kyomi_auth::test_pg` for the crate that carries the real-Postgres
//!   pattern for its own service-layer tests.
//! - **The embedding model.** `build_ctx`'s `embedding` field is unloaded by
//!   default — a tool that calls `wait_ready()` on it without a test first
//!   swapping in [`loaded_embedding`] will hang forever (no background
//!   loader is ever spawned in-process). This is deliberate: loading the
//!   real embedding model is measurably slow, so tests that don't need it
//!   shouldn't pay for it.
//! - **Multi-connection WebSocket fan-out.** `build_ctx`'s `ws_manager` is a
//!   fresh, connectionless [`kyomi_auth::websocket::WebSocketManager`] — a
//!   test asserting *what* gets broadcast (not just that a DB write
//!   succeeded) must call `.connect(user_id)` itself and read from the
//!   returned receiver, as `tools/watch.rs`'s `broadcast_routing` tests do.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
pub(crate) use kyomi_test_db::test_pool;
use kyomi_test_db::{seed_membership, seed_user, seed_workspace_with_name, sqlite_pool};

use crate::provider::LLMProvider;
use crate::tools::{AgentTool, ToolContext};
use crate::types::{AgentTokenUsage, LLMResponse, Message, Tool, ToolCall};

/// Build a fully-populated [`ToolContext`] over `db`, for user `"user-a"`
/// in workspace `"ws-1"`. Every field is `pub`; overwrite whichever ones a
/// given test needs to differ from these defaults (see module docs).
pub(crate) fn build_ctx(db: kyomi_core::DbPool) -> ToolContext {
    ToolContext {
        ws_manager: kyomi_auth::websocket::WebSocketManager::new(None, db.clone()),
        db,
        kv: kyomi_core::kv_store_memory::InMemoryKVStore::new_pool(),
        user_id: "user-a".to_string(),
        workspace_id: "ws-1".to_string(),
        encryption_key: Arc::new([0u8; 32]),
        embedding: kyomi_embed::LazyEmbedding::new(),
        config: Arc::new(kyomi_core::Config::test_config()),
        session_id: None,
        supports_mcp_apps: false,
        workspace_roles: Vec::new(),
        connect_registry: None,
        platforms: Arc::new(kyomi_core::platform::PlatformRegistry::new()),
        user_display_name: "User A".to_string(),
        document_id: None,
    }
}

/// A [`kyomi_embed::LazyEmbedding`] with the real model already loaded,
/// shared process-wide behind a `OnceLock` so the (measurably slow) model
/// load happens at most once per test binary run, no matter how many tests
/// across this crate need it.
///
/// Assign the result to `ctx.embedding` for any tool whose `execute()` calls
/// `ctx.embedding.wait_ready()` — `build_ctx`'s default `embedding` is
/// deliberately unloaded (see module docs) and `wait_ready()` on it hangs
/// forever in a test binary, since nothing ever calls `.set()`.
pub(crate) fn loaded_embedding() -> kyomi_embed::LazyEmbedding {
    static EMBED: std::sync::OnceLock<kyomi_embed::LazyEmbedding> = std::sync::OnceLock::new();
    EMBED
        .get_or_init(|| {
            kyomi_embed::LazyEmbedding::loaded(
                kyomi_embed::EmbeddingService::new().expect("load embedding model for tests"),
            )
        })
        .clone()
}

/// Insert a minimal `users` + `workspaces` + `workspace_users` row set: user
/// `"user-a"` owning workspace `"ws-1"` — the identities [`build_ctx`]
/// defaults to. Shared seeding step for tests across `tools/dashboard.rs`,
/// `tools/knowledge.rs`, and `tools/copilot.rs` that need a real
/// owner/workspace row to satisfy `dashboard_service`'s ownership and
/// free-tier-limit lookups. The `workspace_users` row matters even for
/// single-user tests: `WebSocketManager::broadcast_to_workspace` (which
/// every dashboard create/modify/delete broadcast goes through) looks up
/// recipients from that table, not from `workspaces.owner_user_id` — a test
/// that omits it will seed a workspace whose broadcasts silently reach
/// nobody, not even the owner.
pub(crate) async fn seed_user_and_workspace(db: &kyomi_core::DbPool) {
    // NOTE: KYO-751 adopts the shared helpers' generic panic diagnostics;
    // row values and the user -> workspace -> membership insert order stay the same.
    let sq = sqlite_pool(db);
    seed_user(sq, "user-a", "a@test.local").await;
    seed_workspace_with_name(sq, "ws-1", "user-a", "Workspace").await;
    seed_membership(sq, "ws-1", "user-a", "workspace_admin", true).await;
}

// ---------------------------------------------------------------------------
// ScriptedProvider — a fake LLMProvider driven by a fixed reply script
// ---------------------------------------------------------------------------
//
// Originally private to `agent.rs`'s own test module. Extracted here (KYO-493
// phase 3) once `adapter.rs`'s tests needed the same ToolCall-then-Text
// scripting to drive a full `ChatAgentAdapter::chat()` turn — see
// `docs/standards/code-organization/third-copy-of-test-helper-is-extraction-trigger.md`.
// `agent.rs`'s own tests (which exercise `CustomAgent` directly, not through
// the adapter) now import these from here too, rather than each crate module
// keeping its own copy.

/// Name of the single tool [`ScriptedProvider`]'s tool-call replies target.
pub(crate) const NOOP_TOOL_NAME: &str = "noop";

/// A tool that does nothing, registered so the tool slice the agent hands to
/// the LLM during the loop is non-empty.
pub(crate) struct NoopTool;

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
pub(crate) enum Reply {
    /// A plain text answer — ends the agent loop. Carries the default
    /// (zero) token usage; use `TextWithUsage` when a test needs to drive
    /// `max_total_tokens`.
    Text(String),
    /// A text answer that also carries provider thinking text (KYO-974).
    TextWithThinking(String, String),
    /// A single tool call — drives the loop into another iteration. Carries
    /// the default (zero) token usage; use `ToolCallWithUsage` when a test
    /// needs to drive `max_total_tokens`.
    ToolCall,
    /// A single tool call carrying explicit token usage — needed by tests
    /// that must control exactly how many billable tokens each iteration
    /// contributes.
    ToolCallWithUsage(AgentTokenUsage),
    /// A single tool call preceded by `tokio::time::sleep(duration)` —
    /// needed by tests that drive a wall-clock guard under a paused tokio
    /// clock (`#[tokio::test(start_paused = true)]`) rather than a real
    /// sleep.
    SlowToolCall(std::time::Duration),
    /// A fully specified response (e.g. a provider-executed server-tool turn,
    /// KYO-969).
    Response(Box<LLMResponse>),
    /// A provider failure.
    Failure(String),
    /// A single tool call whose arguments the provider could not parse
    /// (KYO-535) — carries `finish_reason: "max_tokens"`, matching the
    /// truncation-mid-payload shape the ticket traces to. Drives the loop
    /// into another iteration, same as `ToolCall`, but the noop tool must
    /// NOT be executed for it.
    TruncatedToolCall,
}

/// Build a single-tool-call [`LLMResponse`] carrying the given usage.
///
/// Shared by the `ToolCall`, `ToolCallWithUsage`, and `SlowToolCall`
/// branches of [`ScriptedProvider::complete`] so the noop-tool-call shape
/// (id, name, empty arguments) is defined exactly once.
pub(crate) fn tool_call_response(usage: AgentTokenUsage) -> LLMResponse {
    LLMResponse {
        raw_response: serde_json::Value::Null,
        content: String::new(),
        finish_reason: "tool_use".to_string(),
        usage,
        tool_calls: Some(vec![ToolCall {
            id: "tc_1".to_string(),
            name: NOOP_TOOL_NAME.to_string(),
            arguments: json!({}),
            arguments_error: None,
        }]),
        cost: None,
        thinking_content: None,
        call_detail: None,
    }
}

/// One recorded `LLMProvider::complete` invocation.
#[derive(Debug)]
pub(crate) struct RecordedCall {
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,
    /// The provider-executed tools this request carried.
    pub server_tools: Vec<serde_json::Value>,
}

impl RecordedCall {
    /// Messages in this call that carry an iteration-budget notice.
    pub(crate) fn budget_notices(&self) -> Vec<&Message> {
        self.messages
            .iter()
            .filter(|m| m.content.contains(crate::agent::BUDGET_NOTICE_PREFIX))
            .collect()
    }
}

/// An [`LLMProvider`] that records every call it receives and replays a
/// fixed script of replies.
pub(crate) struct ScriptedProvider {
    pub script: Mutex<VecDeque<Reply>>,
    /// Replayed once the script runs out (e.g. "keeps calling tools").
    pub after_script: Reply,
    pub calls: Arc<Mutex<Vec<RecordedCall>>>,
}

#[async_trait]
impl LLMProvider for ScriptedProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[Tool],
        options: &crate::provider::RequestOptions,
        _user_names: &HashMap<String, String>,
    ) -> kyomi_core::Result<LLMResponse> {
        self.calls
            .lock()
            .expect("call log mutex")
            .push(RecordedCall {
                messages: messages.to_vec(),
                tools: tools.to_vec(),
                server_tools: options.server_tools.clone(),
            });

        let reply = self
            .script
            .lock()
            .expect("script mutex")
            .pop_front()
            .unwrap_or_else(|| self.after_script.clone());

        match reply {
            Reply::Text(content) => Ok(LLMResponse {
        raw_response: serde_json::Value::Null,
                content,
                finish_reason: "end_turn".to_string(),
                usage: AgentTokenUsage::default(),
                tool_calls: None,
                cost: None,
                thinking_content: None,
                call_detail: None,
            }),
            Reply::TextWithThinking(content, thinking) => Ok(LLMResponse {
                raw_response: serde_json::Value::Null,
                content,
                finish_reason: "end_turn".to_string(),
                usage: AgentTokenUsage::default(),
                tool_calls: None,
                cost: None,
                thinking_content: Some(thinking),
                call_detail: None,
            }),
            Reply::ToolCall => Ok(tool_call_response(AgentTokenUsage::default())),
            Reply::ToolCallWithUsage(usage) => Ok(tool_call_response(usage)),
            Reply::SlowToolCall(duration) => {
                tokio::time::sleep(duration).await;
                Ok(tool_call_response(AgentTokenUsage::default()))
            }
            Reply::Response(response) => Ok(*response),
            Reply::Failure(message) => Err(kyomi_core::Error::Internal(message)),
            Reply::TruncatedToolCall => Ok(LLMResponse {
        raw_response: serde_json::Value::Null,
                content: String::new(),
                finish_reason: "max_tokens".to_string(),
                usage: AgentTokenUsage::default(),
                tool_calls: Some(vec![ToolCall {
                    id: "tc_truncated".to_string(),
                    name: NOOP_TOOL_NAME.to_string(),
                    arguments: json!({}),
                    arguments_error: Some(
                        "arguments could not be parsed as JSON (finish_reason=max_tokens): \
                         EOF while parsing an object"
                            .to_string(),
                    ),
                }]),
                cost: None,
                thinking_content: None,
                call_detail: None,
            }),
        }
    }

    fn model(&self) -> &str {
        "scripted-model"
    }
}

// ---------------------------------------------------------------------------
// Shared tool-failure fixtures (KYO-970): used by the agent loop's tests in
// `agent.rs` (legacy, non-runtime path) and `adapter.rs` (durable runtime path).
// ---------------------------------------------------------------------------

/// An [`LLMProvider`] that overrides a [`ScriptedProvider`]'s tool calls with a
/// scripted list: response `i` carries `requested[i*batch..(i+1)*batch]`, then the
/// inner provider's text reply. Used to drive real tool outcomes through the
/// durable `ChatAgentAdapter::chat()` path.
pub(crate) struct ActualToolsProvider {
    pub(crate) inner: ScriptedProvider,
    pub(crate) requested: Vec<(&'static str, serde_json::Value)>,
    pub(crate) index: std::sync::atomic::AtomicUsize,
    pub(crate) truncation: Truncation,
    /// Tool calls returned per model response (1 for every pre-existing test).
    pub(crate) batch: usize,
}

/// Which returned tool calls carry an unparsable-arguments error.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Truncation {
    /// None of them.
    Never,
    /// Only the first call of the first response (the pre-existing `truncated` flag).
    FirstCall,
    /// Every call of every response.
    EveryCall,
}
#[async_trait::async_trait]
impl LLMProvider for ActualToolsProvider {
    async fn complete(&self, messages: &[Message], tools: &[Tool], options: &crate::provider::RequestOptions,
        names: &HashMap<String, String>) -> kyomi_core::Result<LLMResponse> {
        let mut response = self.inner.complete(messages, tools, options, names).await?;
        let index = self.index.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let start = index * self.batch;
        let end = (start + self.batch).min(self.requested.len());
        if start < end {
            let truncate = |offset: usize| match self.truncation {
                Truncation::Never => false,
                Truncation::FirstCall => index == 0 && offset == 0,
                Truncation::EveryCall => true,
            };
            let calls: Vec<ToolCall> = self.requested[start..end]
                .iter()
                .enumerate()
                .map(|(offset, (name, arguments))| ToolCall {
                    id: format!("provider-call-{}", start + offset),
                    name: (*name).into(),
                    arguments: arguments.clone(),
                    arguments_error: truncate(offset)
                        .then(|| "arguments could not be parsed as JSON (finish_reason=max_tokens): EOF".to_string()),
                })
                .collect();
            if calls.iter().any(|call| call.arguments_error.is_some()) {
                response.finish_reason = "max_tokens".into();
            }
            response.tool_calls = Some(calls);
        }
        Ok(response)
    }
    fn model(&self) -> &str { "actual-tool-outcome-fixture" }
}

/// The outcome a [`FailingTool`] produces: the failure classes a tool can report,
/// plus one success, so tests can pin how each is classified and budgeted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    BadRequest,
    ServerError,
    Timeout,
    UnknownResult,
    Panic,
    ResultPanic,
    TransportError,
    NotFound,
    Forbidden,
    BadRequestAfterEffect,
    MutatingUnknown,
    RejectedResult,
    Succeeded,
}

/// A tool that fails (or succeeds) in the way its [`FailureKind`] selects, counting
/// invocations. `name` lets one test registry hold several failure classes.
pub(crate) struct FailingTool {
    pub(crate) kind: FailureKind,
    pub(crate) name: &'static str,
    pub(crate) calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl AgentTool for FailingTool {
    fn name(&self) -> &str { self.name }
    fn description(&self) -> &str { "Failure-class fixture" }
    fn parameters_schema(&self) -> serde_json::Value { serde_json::json!({"type":"object","properties":{}}) }
    fn bad_request_is_pre_effect(&self) -> bool {
        self.kind != FailureKind::BadRequestAfterEffect
    }
    fn result_domain_outcome(&self, text: &str) -> agent_runtime::DomainOutcome {
        if self.kind == FailureKind::ResultPanic {
            panic!("result classification exploded");
        }
        if self.kind == FailureKind::UnknownResult {
            agent_runtime::DomainOutcome::Unknown
        } else if text.contains("\"error\"") || text.contains("\"valid\":false") {
            agent_runtime::DomainOutcome::Rejected
        } else {
            agent_runtime::DomainOutcome::Succeeded
        }
    }
    async fn execute(&self, _: serde_json::Value, ctx: &ToolContext) -> kyomi_core::Result<String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match self.kind {
            FailureKind::BadRequest => Err(kyomi_core::Error::BadRequest("Missing required parameter 'x'".into())),
            FailureKind::ServerError => Err(kyomi_core::Error::ServiceUnavailable("provider server exploded".into())),
            FailureKind::Timeout => Err(kyomi_core::Error::ServiceUnavailable("timed out waiting for provider".into())),
            FailureKind::UnknownResult => Ok("{\"error\":\"timeout after commit; unknown_after_commit\"}".into()),
            FailureKind::Panic => panic!("tool exploded"),
            FailureKind::ResultPanic => Ok("{\"ok\":true}".into()),
            FailureKind::TransportError => Err(kyomi_core::Error::DatasourceConnection("connection reset by peer".into())),
            FailureKind::NotFound => Err(kyomi_core::Error::NotFound("datasource 'missing' not found".into())),
            FailureKind::Forbidden => Err(kyomi_core::Error::Forbidden("document scope denied".into())),
            FailureKind::BadRequestAfterEffect => {
                kyomi_core::db_execute!(&ctx.db, "UPDATE workspaces SET name='mutated once' WHERE workspace_id = $1", &ctx.workspace_id).unwrap();
                Err(kyomi_core::Error::BadRequest("partial effect then validation failed".into()))
            }
            FailureKind::MutatingUnknown => {
                kyomi_core::db_execute!(&ctx.db, "UPDATE workspaces SET name='mutated once' WHERE workspace_id = $1", &ctx.workspace_id).unwrap();
                Err(kyomi_core::Error::ServiceUnavailable("mutated then lost acknowledgement".into()))
            }
            FailureKind::RejectedResult => Ok("{\"valid\":false}".into()),
            FailureKind::Succeeded => Ok("{\"ok\":true}".into()),
        }
    }
}

/// A [`FailingTool`] that rejects every call with a definite pre-effect rejection
/// (`BadRequest`), like a SQL analysis error the model can repair.
pub(crate) fn rejecting_tool(calls: &Arc<std::sync::atomic::AtomicUsize>) -> Arc<FailingTool> {
    Arc::new(FailingTool { kind: FailureKind::BadRequest, name: "rejecting_tool", calls: calls.clone() })
}

/// A tool that always succeeds, counting invocations (used to prove a success
/// resets a consecutive-failure budget).
pub(crate) struct SucceedingTool {
    pub(crate) calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl AgentTool for SucceedingTool {
    fn name(&self) -> &str { "succeeding_tool" }
    fn description(&self) -> &str { "Succeeding fixture" }
    fn parameters_schema(&self) -> serde_json::Value { serde_json::json!({"type":"object","properties":{}}) }
    async fn execute(&self, _: serde_json::Value, _: &ToolContext) -> kyomi_core::Result<String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok("{\"ok\":true}".into())
    }
}

// ---------------------------------------------------------------------------
// FakeHttpServer — a loopback HTTP server that records requests (KYO-973)
// ---------------------------------------------------------------------------
//
// Real HTTP over a real socket, so provider clients are exercised end to end
// (headers, URL, body) with canned vendor responses instead of a mocked trait.

/// One request received by a [`FakeHttpServer`].
#[derive(Debug, Clone)]
pub(crate) struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: std::collections::HashMap<String, String>,
    pub body: String,
}

impl RecordedRequest {
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

type FakeHandler = Arc<dyn Fn(&RecordedRequest) -> (u16, String) + Send + Sync>;

pub(crate) struct FakeHttpServer {
    pub base_url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeHttpServer {
    pub(crate) async fn start(
        handler: impl Fn(&RecordedRequest) -> (u16, String) + Send + Sync + 'static,
    ) -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<RecordedRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let handler: FakeHandler = Arc::new(handler);
        let log = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { break };
                let handler = handler.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 8192];
                    let header_end = loop {
                        let n = stream.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break p;
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                    let mut lines = head.lines();
                    let mut first = lines.next().unwrap_or_default().split_whitespace();
                    let method = first.next().unwrap_or_default().to_string();
                    let path = first.next().unwrap_or_default().to_string();
                    let headers: std::collections::HashMap<String, String> = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                        .collect();
                    let want: usize = headers
                        .get("content-length")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    let mut body = buf[header_end + 4..].to_vec();
                    while body.len() < want {
                        let n = stream.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        body.extend_from_slice(&chunk[..n]);
                    }
                    let req = RecordedRequest {
                        method,
                        path,
                        headers,
                        body: String::from_utf8_lossy(&body).to_string(),
                    };
                    let (status, reply) = handler(&req);
                    log.lock().unwrap().push(req);
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Self { base_url, requests, task }
    }

    /// Requests received so far, oldest first.
    pub(crate) fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for FakeHttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A minimal Anthropic Messages API success body with one text block.
pub(crate) fn anthropic_text_reply(text: &str) -> String {
    json!({
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "content": [{"type": "text", "text": text}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 11, "output_tokens": 7}
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// OpenAI-compatible HTTP fake — a real network boundary for whole-turn tests
// ---------------------------------------------------------------------------

/// Serve `reply` as the body of every `POST /chat/completions` and return the
/// running server; point `LLM_BASE_URL` at its `uri()`. The code under test
/// (`execute_agent_chat`) builds its real OpenAI provider and speaks real HTTP
/// to this server; only the remote LLM is faked. Keep the server alive for the
/// duration of the turn.
pub(crate) async fn spawn_openai_fake(reply: serde_json::Value) -> wiremock::MockServer {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(reply))
        .mount(&server)
        .await;
    server
}
