// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat agent adapter — wraps [`CustomAgent`] with persistence, context loading,
//! and message round-tripping.
//!
//! [`ChatAgentAdapter`] is the bridge between the agent loop and the database.
//! It handles:
//! - Loading conversation history from the DB into the agent's state
//! - Persisting ALL new messages (user, assistant, tool) after each chat
//! - Wiring the thinking tracker to agent callbacks
//!
//! ## Cache-hit principle
//!
//! Messages are stored in the DB exactly as the LLM sees them, or with
//! enough information to reconstruct that byte-identical form on read.
//! Loading them back for the next turn must produce the same prefix the
//! live turn saw, maximising prompt cache hits.
//!
//! For most roles that means storing `content` verbatim. User messages sent
//! via `chat_service::prepare_chat_dispatch` (KYO-492) are the one
//! exception: `content` is stored raw (never annotated), and the
//! `[source: X, user_local_time: Y]` prefix `agent.chat()`'s
//! `build_metadata_prefix` builds for the live LLM call is instead
//! recoverable from that row's own `current_time_user_tz` /
//! `message_source` columns. `kyomi_auth::copilot_service::prepare_copilot_message`
//! (KYO-554) stores its user message the same way, via
//! `UserMessagePersistence::CallerPersisted`.
//! [`db_message_to_agent_message`] rebuilds the identical prefix from those
//! columns for every later turn (KYO-506) — see its doc for why an
//! `AdapterPersists` row (Slack, watch) never needs this: its `content`
//! already carries the prefix as literal text.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use kyomi_auth::chat_service;
use kyomi_core::DbPool;

use crate::agent::{build_metadata_prefix, CustomAgent};
use crate::thinking::AgentThinkingTracker;
use crate::types::{Message, MessageRole, ToolCall};

// ---------------------------------------------------------------------------
// ChatAgentAdapter
// ---------------------------------------------------------------------------

/// Adapter wrapping a [`CustomAgent`] with database persistence and context management.
///
/// One adapter is created per user message exchange. It:
/// 1. Loads existing conversation history from the DB
/// 2. Delegates to `CustomAgent::chat()` for the LLM loop
/// 3. Persists ALL new messages (user, assistant, tool) back to the DB
pub struct ChatAgentAdapter {
    agent: CustomAgent,
    pub user_id: String,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub component: String,
    context_loaded: bool,
    db: DbPool,
    encryption_key: Arc<[u8; 32]>,
    /// Number of messages loaded from the DB during context loading.
    /// Used to determine which new messages need to be persisted.
    messages_loaded_count: usize,
    /// Set from [`ChatParams::user_message_persistence`] at the top of
    /// [`ChatAgentAdapter::chat`], before `load_context()` runs. See that
    /// field's doc for the contract this enforces.
    user_message_persistence: UserMessagePersistence,
    /// Set from [`ChatParams::assistant_message_persistence`] at the top of
    /// [`ChatAgentAdapter::chat`]. See [`AssistantMessagePersistence`] for
    /// the contract this enforces on [`ChatAgentAdapter::persist_after_chat`].
    assistant_message_persistence: AssistantMessagePersistence,
    /// Index into `agent.state().messages` up to which messages have
    /// already been durably written — by the incremental per-iteration
    /// writer ([`ChatAgentAdapter::wire_incremental_persister`]), not just
    /// by `persist_after_chat`'s own tail pass (KYO-493 phase 3).
    /// Re-initialized to `messages_loaded_count` at the top of every
    /// [`ChatAgentAdapter::chat`] call, once `load_context()` has run.
    /// `Arc<Mutex<_>>` because the incremental writer is a callback closure
    /// (`Fn`, not `FnMut`) invoked from inside `CustomAgent::chat()` — see
    /// [`crate::agent::IterationBoundaryCallback`]'s doc for why that
    /// closure's own sequencing already rules out concurrent access; the
    /// lock exists only to satisfy interior mutability, not to arbitrate a
    /// real race.
    persisted_up_to: Arc<tokio::sync::Mutex<usize>>,
    /// Set by [`ChatAgentAdapter::set_thinking_tracker`]. Consulted by
    /// [`ChatAgentAdapter::wire_incremental_persister`] to force a
    /// thinking-events flush at every iteration boundary (KYO-493 phase 3)
    /// — kept as a field (rather than folded only into the 5 sync callbacks
    /// `set_thinking_tracker` wires) because the iteration-boundary hook is
    /// wired separately, inside `chat()`, once `messages_loaded_count` is
    /// known.
    thinking_tracker: Option<Arc<tokio::sync::Mutex<AgentThinkingTracker>>>,
    persistence_error: Arc<tokio::sync::Mutex<Option<String>>>,
}

/// How the user's message for a given turn reaches the database.
///
/// This used to be a single `Option<String>` (`user_message_id`) doing two
/// unrelated jobs at once: stamping the id used for WebSocket streaming,
/// *and* signalling "already durably written — do not persist again".
/// Slack only ever wanted the first: it mints its own id but still relies
/// on [`ChatAgentAdapter::persist_after_chat`] to do the actual write.
/// Collapsing both back into one `Option` makes that distinction impossible
/// to see at the call site — splitting them into variants makes the
/// mistake unrepresentable.
#[derive(Debug, Clone)]
pub enum UserMessagePersistence {
    /// The adapter persists the user message itself, at the end of the run,
    /// in [`ChatAgentAdapter::persist_after_chat`]. `Some(id)` stamps that
    /// row with a caller-chosen id (matching the id already streamed to the
    /// client over WebSocket); `None` lets the database layer generate one.
    /// Historical behaviour — Slack (`Some(id)`, minted up front but never
    /// pre-persisted) and watch execution (`None`) use this.
    AdapterPersists(Option<String>),
    /// The caller already wrote the user-message row to the DB *before*
    /// the agent was spawned (KYO-492 —
    /// `kyomi_auth::chat_service::prepare_chat_dispatch`; KYO-554 —
    /// `kyomi_auth::copilot_service::prepare_copilot_message`). The adapter
    /// must neither re-load that row into context (`load_context` filters
    /// it out via [`drop_pre_persisted_message`]) nor write it a second time
    /// (`persist_after_chat` skips it via [`should_persist_new_message`]).
    CallerPersisted(String),
}

impl Default for UserMessagePersistence {
    /// `AdapterPersists(None)` — no pre-persisted row, no caller-chosen id.
    /// This is pre-KYO-492 behaviour: the adapter both generates and
    /// persists the message id itself.
    fn default() -> Self {
        Self::AdapterPersists(None)
    }
}

impl UserMessagePersistence {
    /// The id to stamp on the newly-appended user message, if any.
    ///
    /// Fires for *either* variant whenever a concrete id is available —
    /// this is what keeps the id used for WebSocket streaming in sync with
    /// the id ultimately written to the DB (whichever side writes it), and
    /// for `CallerPersisted` is also the id `should_persist_new_message`
    /// and `drop_pre_persisted_message` match against.
    fn tag_id(&self) -> Option<&str> {
        match self {
            Self::AdapterPersists(id) => id.as_deref(),
            Self::CallerPersisted(id) => Some(id.as_str()),
        }
    }

    /// The id of a row the caller already wrote to the DB — `Some` only
    /// for `CallerPersisted`. Drives both the skip-persist
    /// (`should_persist_new_message`) and drop-from-loaded-context
    /// (`drop_pre_persisted_message`) behaviour; `AdapterPersists` never
    /// triggers either, even when it carries an id.
    fn caller_persisted_id(&self) -> Option<&str> {
        match self {
            Self::CallerPersisted(id) => Some(id.as_str()),
            Self::AdapterPersists(_) => None,
        }
    }
}

/// How the assistant's reply for a given turn reaches the database.
///
/// Mirrors [`UserMessagePersistence`] on the other side of the same turn —
/// same shape, same reason: which arm applies is a claim about what the
/// caller already did to the DB, and folding it back into a bare
/// `Option<String>` (as this crate did before KYO-493) makes "an id was
/// minted" and "a row was pre-inserted for that id" indistinguishable at
/// the call site, which is exactly the ambiguity that caused KYO-572.
#[derive(Debug, Clone)]
pub enum AssistantMessagePersistence {
    /// [`ChatAgentAdapter::persist_after_chat`] INSERTs the final assistant
    /// message itself, once the agent loop produces it — pre-KYO-493
    /// behaviour, and still correct for every caller that does not
    /// pre-insert a placeholder: copilot (KYO-572 — deliberately no
    /// placeholder, see `kyomi_auth::copilot_service::prepare_copilot_message`),
    /// Slack, and watch execution. `Some(id)` stamps the row with a
    /// caller-chosen id (kept in sync with the id already used for
    /// WebSocket streaming / thinking-tracker attribution); `None` lets the
    /// database layer generate one.
    AdapterInserts(Option<String>),
    /// The caller already INSERTed an empty placeholder row for this id
    /// before the agent ran, with `status = 'in_progress'` (KYO-493 —
    /// `kyomi_auth::chat_service::prepare_chat_dispatch`).
    /// `persist_after_chat` must UPDATE that row — via
    /// `kyomi_auth::chat_service::finalize_assistant_placeholder` — never
    /// INSERT a second one under the same primary key, when it reaches the
    /// message tagged with this id. `status` itself is finalized separately
    /// by `kyomi_agent::execution::execute_agent_chat`, which is the one
    /// place that knows the turn's true terminal outcome.
    CallerPreInserted(String),
    /// All writes are committed under the durable run lease.
    Durable { message_id: String, run: DurableRun },
}

#[derive(Debug, Clone)]
pub struct DurableRun {
    pub conversation_id: agent_runtime::ConversationId,
    pub run_id: agent_runtime::RunId,
    pub lease: agent_runtime::Lease,
    pub context: Arc<kyomi_auth::conversation_events::ClaimedConversationContext>,
}

impl Default for AssistantMessagePersistence {
    /// `AdapterInserts(None)` — no pre-inserted row, no caller-chosen id.
    /// Pre-KYO-493 behaviour: the adapter both generates and inserts the
    /// message id itself.
    fn default() -> Self {
        Self::AdapterInserts(None)
    }
}

impl AssistantMessagePersistence {
    /// The id to stamp on the final assistant message in agent state, if
    /// any — fires for either variant whenever a concrete id is available.
    /// This is what keeps the id used for WebSocket streaming / the
    /// thinking tracker in sync with the id ultimately written to the DB.
    ///
    /// `pub(crate)`: also called from `crate::execution::execute_agent_chat`
    /// to resolve the id it hands the thinking tracker and
    /// `AgentExecutionResult`, before `ChatParams` is even built.
    pub(crate) fn tag_id(&self) -> Option<&str> {
        match self {
            Self::AdapterInserts(id) => id.as_deref(),
            Self::CallerPreInserted(id) => Some(id.as_str()),
            Self::Durable { message_id, .. } => Some(message_id.as_str()),
        }
    }

    /// The id of a row the caller already pre-inserted — `Some` only for
    /// `CallerPreInserted`. Drives the UPDATE-instead-of-INSERT branch in
    /// `persist_after_chat`; `AdapterInserts` never triggers it, even when
    /// it carries an id.
    fn preinserted_id(&self) -> Option<&str> {
        match self {
            Self::CallerPreInserted(id) => Some(id.as_str()),
            Self::Durable { message_id, .. } => Some(message_id.as_str()),
            Self::AdapterInserts(_) => None,
        }
    }

    pub(crate) fn durable_run(&self) -> Option<&DurableRun> {
        match self {
            Self::Durable { run, .. } => Some(run),
            _ => None,
        }
    }
}

/// Arguments for [`ChatAgentAdapter::chat`] — one agent turn.
///
/// Packaged into a struct to keep the public signature under clippy's
/// `too_many_arguments` threshold while keeping every field explicit at
/// the call site.
pub struct ChatParams<'a> {
    pub message: &'a str,
    pub cancel_token: CancellationToken,
    pub current_time_user_tz: Option<&'a str>,
    pub message_source: Option<&'a str>,
    pub user_id: Option<&'a str>,
    /// How the user's message for this turn reaches the database — see
    /// [`UserMessagePersistence`] for the two arms and the contract each
    /// enforces.
    pub user_message_persistence: &'a UserMessagePersistence,
    /// How the assistant's reply for this turn reaches the database — see
    /// [`AssistantMessagePersistence`] for the two arms and the contract
    /// each enforces.
    pub assistant_message_persistence: &'a AssistantMessagePersistence,
}

impl ChatAgentAdapter {
    /// Create a new adapter.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent: CustomAgent,
        user_id: String,
        workspace_id: String,
        session_id: Option<String>,
        component: String,
        db: DbPool,
        encryption_key: Arc<[u8; 32]>,
    ) -> Self {
        Self {
            agent,
            user_id,
            workspace_id,
            session_id,
            component,
            context_loaded: false,
            db,
            encryption_key,
            messages_loaded_count: 0,
            user_message_persistence: UserMessagePersistence::default(),
            assistant_message_persistence: AssistantMessagePersistence::default(),
            persisted_up_to: Arc::new(tokio::sync::Mutex::new(0)),
            thinking_tracker: None,
            persistence_error: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Wire a thinking tracker to the agent's callbacks.
    ///
    /// The tracker methods are async but callbacks are synchronous closures.
    /// We use `tokio::task::spawn` to bridge the gap — each callback fires
    /// an async task that runs the tracker method. This keeps the agent loop
    /// non-blocking while still delivering events in near-real-time. Because
    /// each callback's DB/WS work happens inside a *detached* spawn (fired
    /// and forgotten, not awaited here), completion order across callbacks
    /// is not guaranteed — two callbacks invoked close together in the loop
    /// can have their spawned tasks finish in either order.
    ///
    /// Also stores `tracker` on `self` (KYO-493 phase 3): `chat()` wires a
    /// *second*, separate hook — `on_iteration_boundary`
    /// ([`Self::wire_incremental_persister`]) — once `messages_loaded_count`
    /// is known, and that hook needs the same tracker to force a
    /// thinking-events flush at each iteration boundary. Unlike the five
    /// callbacks below, that hook is awaited in-line by the agent loop, not
    /// spawned — see [`crate::agent::IterationBoundaryCallback`]'s doc.
    pub fn set_thinking_tracker(&mut self, tracker: Arc<tokio::sync::Mutex<AgentThinkingTracker>>) {
        self.thinking_tracker = Some(tracker.clone());
        let callbacks = self.agent.callbacks_mut();

        // on_thinking -> tracker.agent_thought(thought)
        // Note: error handling for Redis publish is inside the tracker methods themselves.
        let tracker_thinking = tracker.clone();
        callbacks.on_thinking = Some(Box::new(move |thought: &str| {
            let tracker = tracker_thinking.clone();
            let thought = thought.to_string();
            tokio::task::spawn(async move {
                tracker.lock().await.agent_thought(&thought).await;
            });
        }));

        // on_token_usage -> accumulate + tracker.update_token_usage(...)
        let tracker_usage = tracker.clone();
        callbacks.on_token_usage = Some(Box::new(
            move |input_tokens: u32, output_tokens: u32, cost: Option<f64>| {
                let tracker = tracker_usage.clone();
                tokio::task::spawn(async move {
                    tracker
                        .lock()
                        .await
                        .update_token_usage(input_tokens, output_tokens, cost)
                        .await;
                });
            },
        ));

        // on_tool_start -> tracker.tool_execution_started(tool_name, tool_input)
        let tracker_tool_start = tracker.clone();
        callbacks.on_tool_start = Some(Box::new(
            move |tool_name: &str, tool_input: &serde_json::Value| {
                let tracker = tracker_tool_start.clone();
                let name = tool_name.to_string();
                let input = tool_input.clone();
                tokio::task::spawn(async move {
                    tracker
                        .lock()
                        .await
                        .tool_execution_started(&name, &input)
                        .await;
                });
            },
        ));

        // on_tool_end -> tracker.tool_execution_completed(tool_name, result, success)
        let tracker_tool_end = tracker.clone();
        callbacks.on_tool_end = Some(Box::new(
            move |tool_name: &str, result: &str, success: bool| {
                let tracker = tracker_tool_end.clone();
                let name = tool_name.to_string();
                let result_str = result.to_string();
                tokio::task::spawn(async move {
                    tracker
                        .lock()
                        .await
                        .tool_execution_completed(&name, &result_str, success)
                        .await;
                });
            },
        ));

        // on_preparing_response -> tracker.preparing_response()
        let tracker_preparing = tracker;
        callbacks.on_preparing_response = Some(Box::new(move || {
            let tracker = tracker_preparing.clone();
            tokio::task::spawn(async move {
                tracker.lock().await.preparing_response().await;
            });
        }));
    }

    /// Load existing conversation context from the database.
    ///
    /// Restores agent metadata (iteration counter, compaction state) and
    /// message history so the agent can continue where it left off.
    ///
    /// Returns `true` if context was loaded, `false` if no session or no
    /// messages exist.
    pub async fn load_context(&mut self) -> kyomi_core::Result<bool> {
        let Some(ref session_id) = self.session_id else {
            return Ok(false);
        };

        // Durable history and compaction state were read under the claim lock.
        // Subsequent edits or newly accepted turns cannot alter this execution.
        let (session_config, db_messages) = if let Some(run) = self.assistant_message_persistence.durable_run() {
            (run.context.config.clone(), run.context.messages.clone())
        } else {
            let Some(session) = chat_service::get_session(&self.db, session_id).await? else {
                return Ok(false);
            };
            let messages = chat_service::get_agent_messages(&self.db, &self.encryption_key, session_id, None).await?;
            let config = session.config.as_ref()
                .map(|config| kyomi_auth::encryption::restore_chat_config(config, &self.encryption_key))
                .transpose()?;
            (config, messages)
        };

        // Restore agent state from session config if available.
        if let Some(ref config) = session_config
            && let Some(agent_state) = config.get("agent_state")
        {
            let state = self.agent.state_mut();

            if let Some(gi) = agent_state.get("global_iteration").and_then(|v| v.as_u64()) {
                state.global_iteration = gi as u32;
            }
            if let Some(summary) = agent_state
                .get("compacted_summary")
                .and_then(|v| v.as_str())
                && !summary.is_empty()
            {
                state.compacted_summary = Some(summary.to_string());
            }
            if let Some(idx) = agent_state
                .get("messages_since_compaction_index")
                .and_then(|v| v.as_u64())
            {
                state.messages_since_compaction_index = idx as usize;
            }
            if let Some(lit) = agent_state
                .get("last_input_tokens")
                .and_then(|v| v.as_u64())
            {
                state.last_input_tokens = lit as u32;
            }
        }

        // Drop the row the caller already persisted before calling chat()
        // (KYO-492) — otherwise it would both be loaded into context here
        // AND re-appended by CustomAgent::chat(), so the LLM sees the same
        // user turn twice and persist_after_chat would try to insert it a
        // second time.
        let db_messages = drop_pre_persisted_message(
            db_messages,
            self.user_message_persistence.caller_persisted_id(),
        );

        if db_messages.is_empty() {
            self.messages_loaded_count = self.agent.state().messages.len();
            self.context_loaded = true;
            return Ok(false);
        }

        // Convert DB messages to agent Message structs.
        let state = self.agent.state_mut();
        for msg in &db_messages {
            let agent_msg = db_message_to_agent_message(msg);
            state.messages.push(agent_msg);
        }

        // Record total message count (including the system message already in state
        // before DB messages were appended) so persist_after_chat slices correctly.
        self.messages_loaded_count = state.messages.len();
        self.context_loaded = true;

        info!(
            session_id = %session_id,
            message_count = db_messages.len(),
            "Loaded agent context from database"
        );

        Ok(true)
    }

    /// Persist new messages and agent metadata to the database after a chat.
    ///
    /// Saves whatever intermediate messages (tool calls, tool results)
    /// [`Self::wire_incremental_persister`]'s per-iteration writer hasn't
    /// already saved during the loop (KYO-493 phase 3 — before that writer
    /// existed, this persisted every new message; now it's the catch-up
    /// pass over `persisted_up_to..`, which is everything for a caller that
    /// never wires the incremental writer, e.g. a hand-built adapter in a
    /// test), and updates the session config with the current agent state.
    pub async fn persist_after_chat(&mut self) -> kyomi_core::Result<()> {
        let Some(ref session_id) = self.session_id else {
            return Ok(());
        };

        let state = self.agent.state();
        let total_messages = state.messages.len();
        let start = *self.persisted_up_to.lock().await;

        if total_messages > start {
            let new_messages = &state.messages[start..];
            let caller_persisted_user_id = self.user_message_persistence.caller_persisted_id();
            let preinserted_assistant_id = self.assistant_message_persistence.preinserted_id();

            for (offset, msg) in new_messages.iter().enumerate() {
                persist_one_new_message(
                    &self.db,
                    &self.encryption_key,
                    session_id,
                    msg,
                    caller_persisted_user_id,
                    preinserted_assistant_id,
                    MessagePersistenceScope {
                        durable_run: self.assistant_message_persistence.durable_run(),
                        user_id: &self.user_id, workspace_id: &self.workspace_id,
                        message_index: start + offset,
                    },
                )
                .await?;
            }

            *self.persisted_up_to.lock().await = total_messages;
        }

        // Save agent metadata to session config.
        let metadata = serde_json::json!({
            "agent_state": {
                "global_iteration": state.global_iteration,
                "compacted_summary": state.compacted_summary,
                "messages_since_compaction_index": state.messages_since_compaction_index,
                "last_input_tokens": state.last_input_tokens,
            }
        });

        if let Some(run) = self.assistant_message_persistence.durable_run() {
            kyomi_auth::conversation_events::ConversationStore::new(
                &self.db, &self.encryption_key, &self.user_id, &self.workspace_id,
            ).fenced_session_metadata(
                &run.conversation_id, &run.run_id, &run.lease,
                chrono::Utc::now().timestamp_millis(), &metadata,
            ).await.map_err(|e| kyomi_core::Error::ServiceUnavailable(e.to_string()))?;
        } else {
            chat_service::update_session(&self.db, session_id, None, None, Some(&metadata)).await?;
        }

        info!(
            session_id = %session_id,
            new_messages = total_messages.saturating_sub(start),
            "Persisted agent state after chat"
        );

        Ok(())
    }

    /// Wire the per-iteration incremental persister and the paired
    /// thinking-events flush trigger (KYO-493 phase 3).
    ///
    /// Must run after `load_context()` — it needs the final
    /// `messages_loaded_count` to seed `persisted_up_to` — and before
    /// `self.agent.chat()`, since the callback it installs
    /// (`on_iteration_boundary`) fires from inside that call. A no-op when
    /// `self.session_id` is `None`: there is no row to persist into.
    ///
    /// Unlike [`Self::set_thinking_tracker`]'s five callbacks, this hook is
    /// awaited in-line by the agent loop rather than spawned — see
    /// [`crate::agent::IterationBoundaryCallback`]'s doc for why that's
    /// what lets `persisted_up_to` be tracked safely.
    fn wire_incremental_persister(&mut self, cancel_token: tokio_util::sync::CancellationToken) {
        let Some(ref session_id) = self.session_id else {
            return;
        };

        let persisted_up_to = Arc::new(tokio::sync::Mutex::new(self.messages_loaded_count));
        self.persisted_up_to = persisted_up_to.clone();

        let db = self.db.clone();
        let encryption_key = self.encryption_key.clone();
        let session_id = session_id.clone();
        let user_message_persistence = self.user_message_persistence.clone();
        let assistant_message_persistence = self.assistant_message_persistence.clone();
        let tracker = self.thinking_tracker.clone();
        let durable_run = self.assistant_message_persistence.durable_run().cloned();
        let persistence_error = self.persistence_error.clone();
        let user_id = self.user_id.clone();
        let workspace_id = self.workspace_id.clone();
        // `CustomAgent::chat()` pushes exactly one new User-role message —
        // this turn's own — and always as the very first thing it does,
        // before the iteration loop even starts; ChartML-retry and
        // budget-notice messages are deliberately kept out of
        // `state.messages` (see that function's doc), so no later message
        // in this turn is ever User-role. That means the CallerPersisted
        // row (KYO-492/554 — already durably written by
        // `prepare_chat_dispatch`/`prepare_copilot_message` before the
        // agent ran) is always at exactly this global index when one
        // exists, which is what this hook uses to skip it — NOT
        // `should_persist_new_message`'s id match. That id match only
        // works once `ChatAgentAdapter::chat()` tags the message
        // (`tag_first_new_user_message_id`), which happens *after* the
        // whole loop returns — i.e. after every iteration boundary this
        // hook could ever fire for has already run. Without this
        // positional skip, the first iteration boundary of a
        // CallerPersisted turn would see an untagged, still-`None`
        // `message_id`, `should_persist_new_message` would not recognise
        // it as already covered, and the user row would be inserted a
        // second time.
        let caller_persisted_user_row_index = self
            .user_message_persistence
            .caller_persisted_id()
            .is_some()
            .then_some(self.messages_loaded_count);

        let callback: crate::agent::IterationBoundaryCallback =
            Box::new(move |messages: &[Message]| {
                let db = db.clone();
                let encryption_key = encryption_key.clone();
                let session_id = session_id.clone();
                // Resolved fresh (and converted to owned strings) on every
                // invocation rather than borrowed from the closure's
                // captured enums — the returned future's lifetime is tied
                // to `messages`, not to these, so it must not borrow them.
                let caller_persisted_user_id =
                    user_message_persistence.caller_persisted_id().map(str::to_string);
                let preinserted_assistant_id =
                    assistant_message_persistence.preinserted_id().map(str::to_string);
                let persisted_up_to = persisted_up_to.clone();
                let tracker = tracker.clone();
                let durable_run = durable_run.clone();
                let persistence_error = persistence_error.clone();
                let cancel_token = cancel_token.clone();
                let user_id = user_id.clone();
                let workspace_id = workspace_id.clone();

                Box::pin(async move {
                    {
                        let mut idx = persisted_up_to.lock().await;
                        let start = *idx;
                        if messages.len() > start {
                            // Advance past only what actually got written —
                            // a failure here is logged and left for
                            // persist_after_chat's own catch-up pass to
                            // retry at the end of the turn (KYO-493 ticket:
                            // "must NOT kill the agent run").
                            let mut advanced = 0usize;
                            for (offset, msg) in messages[start..].iter().enumerate() {
                                let global_index = start + offset;
                                if Some(global_index) == caller_persisted_user_row_index {
                                    // The row prepare_chat_dispatch/
                                    // prepare_copilot_message already wrote
                                    // — see this method's doc above. Not a
                                    // failure; counts toward `advanced`.
                                    advanced += 1;
                                    continue;
                                }
                                match persist_one_new_message(
                                    &db,
                                    &encryption_key,
                                    &session_id,
                                    msg,
                                    caller_persisted_user_id.as_deref(),
                                    preinserted_assistant_id.as_deref(),
                                    MessagePersistenceScope {
                                        durable_run: durable_run.as_ref(),
                                        user_id: &user_id, workspace_id: &workspace_id,
                                        message_index: global_index,
                                    },
                                )
                                .await
                                {
                                    Ok(()) => advanced += 1,
                                    Err(e) => {
                                        if durable_run.is_some() {
                                            *persistence_error.lock().await = Some(e.to_string());
                                            cancel_token.cancel();
                                        }
                                        error!(
                                            session_id = %session_id,
                                            error = %e,
                                            "Failed to incrementally persist an agent message \
                                             mid-run (KYO-493) — will retry when the turn finishes"
                                        );
                                        break;
                                    }
                                }
                            }
                            *idx = start + advanced;
                        }
                    }

                    if let Some(tracker) = tracker {
                        let mut tracker = tracker.lock().await;
                        if let Some(run) = durable_run.as_ref() {
                            let events = tracker.get_events_for_storage();
                            let result = kyomi_auth::conversation_events::ConversationStore::new(
                                &db, &encryption_key, &user_id, &workspace_id,
                            ).fenced_thinking_events(
                                &run.conversation_id, &run.run_id, &run.lease,
                                chrono::Utc::now().timestamp_millis(), &events,
                            ).await;
                            if let Err(error) = result {
                                *persistence_error.lock().await = Some(error.to_string());
                                cancel_token.cancel();
                                error!(session_id = %session_id, %error, "Failed to persist durable thinking previews");
                            }
                        } else {
                            tracker.flush_at_iteration_boundary().await;
                        }
                    }
                })
            });

        self.agent.callbacks_mut().on_iteration_boundary = Some(callback);
    }

    /// Run the agent loop for a user message.
    ///
    /// Handles context loading (lazy), delegation to the agent, and
    /// post-chat persistence. All new messages (user, tool, assistant)
    /// are saved to the DB by `persist_after_chat()`.
    ///
    /// `params.assistant_message_persistence` is set on the final assistant
    /// response message before persistence, ensuring the DB record matches
    /// the ID used for WebSocket streaming and UI display — and, for
    /// `CallerPreInserted`, that `persist_after_chat` UPDATEs the
    /// pre-inserted placeholder row rather than inserting a second one.
    pub async fn chat(&mut self, params: ChatParams<'_>) -> kyomi_core::Result<String> {
        // Record how this turn's user/assistant messages are persisted
        // before load_context() runs, so the filter it applies (and the
        // persist-skip below) see it.
        self.user_message_persistence = params.user_message_persistence.clone();
        self.assistant_message_persistence = params.assistant_message_persistence.clone();

        // Lazy context loading on first call.
        if !self.context_loaded {
            self.load_context().await?;
        }

        // KYO-493 phase 3: must run after load_context() (needs
        // messages_loaded_count) and before self.agent.chat() (the
        // callback it installs fires from inside that call).
        self.wire_incremental_persister(params.cancel_token.clone());

        // Run the agent loop.
        let result = self
            .agent
            .chat(
                params.message,
                params.cancel_token,
                params.current_time_user_tz,
                params.message_source,
                params.user_id,
            )
            .await;

        // Tag the agent-appended user message with the known ID so the DB
        // record — whichever side ends up writing it — matches the ID
        // returned to the frontend. For `CallerPersisted`, this is also
        // what makes the persist-skip above fire: should_persist_new_message
        // matches on this exact message_id, so tagging it is what lets
        // persist_after_chat recognise "this is the row already written by
        // prepare_chat_dispatch" and skip it, rather than by slice
        // arithmetic or role.
        if let Some(umid) = params.user_message_persistence.tag_id() {
            self.tag_first_new_user_message_id(umid);
        }

        // Tag the final assistant message with the known ID so that the DB
        // record matches the ID used for WebSocket streaming.
        if let Some(amid) = params.assistant_message_persistence.tag_id() {
            self.tag_last_assistant_message_id(amid);
        }

        // Durable turns must surface every persistence failure before their worker
        // can commit a terminal result. Legacy callers retain their existing
        // delivery behavior and log a failed tail write with the affected IDs.
        if self.assistant_message_persistence.durable_run().is_some() {
            if let Some(error) = self.persistence_error.lock().await.take() {
                return Err(kyomi_core::Error::ServiceUnavailable(error));
            }
            self.persist_after_chat().await?;
            return result;
        }
        if let Err(e) = self.persist_after_chat().await {
            error!(
                session_id = self.session_id.as_deref().unwrap_or("<none>"),
                assistant_message_id = params.assistant_message_persistence.tag_id().unwrap_or("<none>"),
                user_id = %self.user_id,
                error = %e,
                "Failed to persist agent state after chat — messages may be lost"
            );
        }

        result
    }

    /// Set the `message_id` on the last assistant message in the agent state.
    ///
    /// This ensures that when `persist_after_chat()` saves the message, the DB
    /// record uses the same ID that was used for WebSocket streaming events.
    fn tag_last_assistant_message_id(&mut self, message_id: &str) {
        let state = self.agent.state_mut();
        for msg in state.messages.iter_mut().rev() {
            if msg.role == MessageRole::Assistant {
                msg.message_id = Some(message_id.to_string());
                break;
            }
        }
    }

    /// Set the `message_id` on the first new user message (after loaded
    /// messages) to `message_id` — see [`UserMessagePersistence::tag_id`].
    /// For `CallerPersisted`, this both preserves the "persisted id ==
    /// streamed id" guarantee and is what lets `persist_after_chat` (via
    /// [`should_persist_new_message`]) recognise that message as already
    /// stored and skip it. For `AdapterPersists(Some(id))`, it simply
    /// ensures the row `persist_after_chat` is about to write uses the
    /// caller-chosen id.
    fn tag_first_new_user_message_id(&mut self, message_id: &str) {
        let state = self.agent.state_mut();
        for msg in state.messages[self.messages_loaded_count..].iter_mut() {
            if msg.role == MessageRole::User {
                msg.message_id = Some(message_id.to_string());
                break;
            }
        }
    }

    /// Read-only access to the underlying agent state.
    pub fn agent_state(&self) -> &crate::agent::AgentState {
        self.agent.state()
    }

    /// Read-only access to the agent model name.
    pub fn model_name(&self) -> &str {
        // The model is stored on the AnthropicClient, but we expose it
        // through the adapter for convenience. For now, return the default.
        crate::anthropic::DEFAULT_MODEL
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Whether a newly-produced agent message still needs to be written to the
/// DB by `persist_after_chat`.
///
/// `caller_persisted_id` is [`UserMessagePersistence::caller_persisted_id`]
/// — `Some` exactly when the caller (e.g.
/// `kyomi_auth::chat_service::prepare_chat_dispatch`, KYO-492) already
/// wrote a user message row before the agent loop started
/// (`CallerPersisted`). The one new message whose `message_id` matches it
/// (tagged by `tag_first_new_user_message_id`) must not be written again.
/// Every other new message — a second user turn within the same run, every
/// assistant/tool message, and *every* `AdapterPersists` row regardless of
/// whether it carries a caller-chosen id (e.g. Slack) — still needs
/// persisting. When `caller_persisted_id` is `None`, every new message is
/// persisted, matching pre-KYO-492 behaviour exactly.
fn should_persist_new_message(msg: &Message, caller_persisted_id: Option<&str>) -> bool {
    match caller_persisted_id {
        // Only a User-role message can be the pre-persisted row — an
        // assistant or tool message must never be skipped, even if it
        // happened to carry the same id (message ids are UUIDs, so this is
        // theoretical, but the predicate should not rely on that).
        Some(id) => !(msg.role == MessageRole::User && msg.message_id.as_deref() == Some(id)),
        None => true,
    }
}

/// Persist one newly-produced agent message to the DB, applying the same
/// skip / UPDATE-vs-INSERT rules every caller of it must use (KYO-493 phase
/// 3) — shared by `persist_after_chat`'s tail pass and
/// `ChatAgentAdapter::wire_incremental_persister`'s per-iteration writer, so
/// the two paths cannot drift out of sync with each other. Message ordering
/// (which one fires for a given `msg` first) is the caller's
/// responsibility; this function's job is only "should this be a no-op,
/// an UPDATE, or an INSERT" — the same three-way decision
/// `persist_after_chat` made inline before this extraction.
///
/// `Ok(())` for a system message, or the one row already covered by
/// `caller_persisted_user_id` (see [`should_persist_new_message`]), is a
/// deliberate no-op, not a skipped write that still needs a retry — a
/// caller tracking a persisted-up-to index may advance past it. Only a
/// real persistence attempt that failed returns `Err`.
struct MessagePersistenceScope<'a> {
    durable_run: Option<&'a DurableRun>,
    user_id: &'a str,
    workspace_id: &'a str,
    message_index: usize,
}

async fn persist_one_new_message(
    db: &DbPool,
    encryption_key: &Arc<[u8; 32]>,
    session_id: &str,
    msg: &Message,
    caller_persisted_user_id: Option<&str>,
    preinserted_assistant_id: Option<&str>,
    scope: MessagePersistenceScope<'_>,
) -> kyomi_core::Result<()> {
    let MessagePersistenceScope { durable_run, user_id, workspace_id, message_index } = scope;
    // Skip system messages (they're part of the prompt, not stored).
    if msg.role == MessageRole::System {
        return Ok(());
    }

    // Skip the one message the caller already persisted before calling
    // chat() (KYO-492) — see UserMessagePersistence::CallerPersisted. Every
    // other new message (a second user turn within the same run, every
    // assistant/tool message, and — critically — any AdapterPersists row,
    // e.g. Slack's caller-chosen id) is still persisted here.
    if !should_persist_new_message(msg, caller_persisted_user_id) {
        return Ok(());
    }

    let role = match msg.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
        MessageRole::System => return Ok(()), // unreachable — guarded above
    };

    let tool_calls_json = msg
        .tool_calls
        .as_ref()
        .map(|tc| serde_json::to_value(tc).unwrap_or_default());

    if let Some(run) = durable_run {
        let write = kyomi_auth::conversation_events::MessageWrite {
            message_id: msg.message_id.clone().unwrap_or_else(|| format!("{}:message:{message_index}", run.run_id.as_str())),
            role: role.to_string(),
            content: msg.content.clone(),
            tool_call_id: msg.tool_call_id.clone(),
            name: msg.name.clone(),
            tool_calls: tool_calls_json.clone(),
        };
        kyomi_auth::conversation_events::ConversationStore::new(db, encryption_key, user_id, workspace_id)
            .fenced_message(&run.conversation_id, &run.run_id, &run.lease,
                chrono::Utc::now().timestamp_millis(), &write)
            .await.map_err(|e| kyomi_core::Error::ServiceUnavailable(e.to_string()))?;
        return Ok(());
    }

    // KYO-493: the one message tagged with
    // `AssistantMessagePersistence::CallerPreInserted`'s id already has a
    // row — `prepare_chat_dispatch` wrote it before the agent was spawned.
    // UPDATE it instead of INSERTing a second row under the same primary
    // key (the exact collision KYO-572 fixed for copilot, which is why
    // copilot's placeholder-less path — `AdapterInserts` — must never take
    // this branch and always falls through to the plain INSERT below).
    if msg.role == MessageRole::Assistant
        && preinserted_assistant_id.is_some_and(|id| msg.message_id.as_deref() == Some(id))
    {
        chat_service::finalize_assistant_placeholder(
            db,
            encryption_key,
            msg.message_id.as_deref().expect("matched Some above"),
            &msg.content,
            msg.tool_call_id.as_deref(),
            msg.name.as_deref(),
            tool_calls_json.as_ref(),
        )
        .await?;
        return Ok(());
    }

    chat_service::add_message(
        db,
        encryption_key,
        session_id,
        role,
        &msg.content,
        None, // metadata
        msg.message_id.as_deref(), // use tagged ID if set, else auto-generate
        None, // current_time_user_tz
        // message_source: always None here — a *user*-role message
        // reaching this branch (an AdapterPersists row — Slack, watch; or a
        // second user turn within one CallerPersisted run, e.g.
        // copilot/chat mid-conversation follow-ups the loop itself
        // injects) has its content built by `agent.chat()`'s
        // `build_metadata_prefix`, so any source/local-time annotation is
        // already baked into `msg.content` as literal text. Recording it
        // again in this row's own columns would make
        // `db_message_to_agent_message` reconstruct a *second* prefix on
        // top of the one already there the next time this row is loaded
        // (KYO-506). Assistant/tool messages (including copilot's, which
        // is CallerPersisted for its one user row since KYO-554) always
        // reach this branch too, but never carry a prefix to begin with.
        None,
        if msg.role == MessageRole::User {
            msg.user_id.as_deref()
        } else {
            None
        },
        msg.tool_call_id.as_deref(),
        msg.name.as_deref(),
        tool_calls_json.as_ref(),
        chat_service::MessageStatus::Complete,
    )
    .await?;
    Ok(())
}

/// Drop the DB row already covered by `caller_persisted_id`
/// ([`UserMessagePersistence::caller_persisted_id`]) from a freshly-loaded
/// message history, before it is converted into agent `Message`s and
/// pushed onto `state.messages`.
///
/// Without this, the row `prepare_chat_dispatch` (KYO-492) already wrote
/// would be loaded back into context here AND re-appended by
/// `CustomAgent::chat()`, so the LLM would see the same user turn twice.
/// Preserves the order and content of every other row. When
/// `caller_persisted_id` is `None` (including every `AdapterPersists` row),
/// the list is returned unchanged.
fn drop_pre_persisted_message(
    db_messages: Vec<chat_service::AgentMessage>,
    caller_persisted_id: Option<&str>,
) -> Vec<chat_service::AgentMessage> {
    match caller_persisted_id {
        Some(id) => db_messages.into_iter().filter(|m| m.message_id != id).collect(),
        None => db_messages,
    }
}

/// Convert a database `AgentMessage` to an agent `Message`.
///
/// For a user message, reconstructs the `[source: X, user_local_time: Y]`
/// prefix `agent.chat()`'s `build_metadata_prefix` builds ahead of `content`
/// for the live LLM call, from that row's `current_time_user_tz` /
/// `message_source` columns (KYO-506). This is safe to apply unconditionally
/// rather than only for rows known to need it:
///
/// - A row written by `chat_service::prepare_chat_dispatch` or
///   `copilot_service::prepare_copilot_message` (KYO-554) has both columns
///   populated and a raw `content` — reconstruction here is exactly what
///   restores the annotation the live turn saw.
/// - A row written by `ChatAgentAdapter::persist_after_chat` under the
///   `AdapterPersists` paths (Slack, watch) has both columns `None` (see
///   that call site) and a `content` that already carries the prefix as
///   literal text — `build_metadata_prefix(None, None)` returns an empty
///   string, so `content` passes through unchanged and is never
///   double-prefixed.
/// - A row written before this column pair existed has both columns `None`
///   for the same reason as above: no annotation is fabricated, `content`
///   passes through unchanged.
/// - A row with `current_time_user_tz` but no `message_source` (or vice
///   versa) — e.g. a row written before the `message_source` column existed,
///   or before KYO-554 gave `copilot_service::prepare_copilot_message` a
///   source to record — gets the partial annotation `build_metadata_prefix`
///   already supports; no source is ever invented for it.
fn db_message_to_agent_message(msg: &chat_service::AgentMessage) -> Message {
    match msg.role.as_str() {
        "user" => {
            let prefix = build_metadata_prefix(
                msg.current_time_user_tz.as_deref(),
                msg.message_source.as_deref(),
            );
            let content = if prefix.is_empty() {
                msg.content.clone()
            } else {
                format!("{prefix}{}", msg.content)
            };
            if let Some(ref uid) = msg.sent_by_user_id {
                Message::user_with_id(&content, uid)
            } else {
                Message::user(&content)
            }
        }
        "assistant" => {
            if let Some(ref tc_json) = msg.tool_calls {
                // Parse tool_calls JSON into Vec<ToolCall>.
                let tool_calls: Vec<ToolCall> = serde_json::from_value(tc_json.clone())
                    .unwrap_or_default();
                if tool_calls.is_empty() {
                    Message::assistant(&msg.content)
                } else {
                    Message::assistant_with_tool_calls(&msg.content, tool_calls)
                }
            } else {
                Message::assistant(&msg.content)
            }
        }
        "tool" => Message::tool_result(
            msg.tool_call_id.as_deref().unwrap_or(""),
            msg.tool_name.as_deref().unwrap_or(""),
            &msg.content,
        ),
        "system" => Message::system(&msg.content),
        _ => {
            warn!(role = %msg.role, "Unknown message role in DB, treating as user");
            Message::user(&msg.content)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_message_to_agent_message_user() {
        let msg = chat_service::AgentMessage {
            message_id: "m1".into(),
            role: "user".into(),
            content: "Hello".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::User);
        assert_eq!(result.content, "Hello");
    }

    #[test]
    fn db_message_to_agent_message_assistant() {
        let msg = chat_service::AgentMessage {
            message_id: "m2".into(),
            role: "assistant".into(),
            content: "Here are the results.".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Assistant);
        assert_eq!(result.content, "Here are the results.");
        assert!(result.tool_calls.is_none());
    }

    #[test]
    fn db_message_to_agent_message_assistant_with_tool_calls() {
        let tc = serde_json::json!([{
            "id": "tc_001",
            "name": "search_catalog",
            "arguments": {"query": "revenue"}
        }]);
        let msg = chat_service::AgentMessage {
            message_id: "m3".into(),
            role: "assistant".into(),
            content: "Let me search.".into(),
            tool_calls: Some(tc),
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Assistant);
        let tc = result.tool_calls.as_ref().expect("should have tool calls");
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].name, "search_catalog");
    }

    #[test]
    fn db_message_to_agent_message_tool() {
        let msg = chat_service::AgentMessage {
            message_id: "m4".into(),
            role: "tool".into(),
            content: r#"{"tables": []}"#.into(),
            tool_calls: None,
            tool_call_id: Some("tc_001".into()),
            tool_name: Some("search_catalog".into()),
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Tool);
        assert_eq!(result.tool_call_id.as_deref(), Some("tc_001"));
        assert_eq!(result.name.as_deref(), Some("search_catalog"));
    }

    #[test]
    fn db_message_to_agent_message_system() {
        let msg = chat_service::AgentMessage {
            message_id: "m5".into(),
            role: "system".into(),
            content: "You are helpful.".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::System);
    }

    #[test]
    fn db_message_to_agent_message_unknown_role() {
        let msg = chat_service::AgentMessage {
            message_id: "m6".into(),
            role: "unknown".into(),
            content: "mystery message".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        // Falls back to user.
        assert_eq!(result.role, MessageRole::User);
    }

    // -- Contract: db_message_to_agent_message edge cases -------------------

    #[test]
    fn db_message_to_agent_message_assistant_with_empty_tool_calls_array() {
        // An empty tool_calls array should be treated as no tool calls.
        let tc = serde_json::json!([]);
        let msg = chat_service::AgentMessage {
            message_id: "m7".into(),
            role: "assistant".into(),
            content: "No tools needed.".into(),
            tool_calls: Some(tc),
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Assistant);
        // Empty tool_calls array -> treated as plain assistant message.
        assert!(result.tool_calls.is_none());
    }

    #[test]
    fn db_message_to_agent_message_assistant_with_multiple_tool_calls() {
        let tc = serde_json::json!([
            {"id": "tc_1", "name": "search_catalog", "arguments": {"query": "rev"}},
            {"id": "tc_2", "name": "get_table_info", "arguments": {"table_name": "orders"}},
            {"id": "tc_3", "name": "query_datasource", "arguments": {"sql_query": "SELECT 1", "datasource": "pg"}}
        ]);
        let msg = chat_service::AgentMessage {
            message_id: "m8".into(),
            role: "assistant".into(),
            content: "Let me investigate all these things.".into(),
            tool_calls: Some(tc),
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Assistant);
        let tool_calls = result.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 3);
        assert_eq!(tool_calls[0].name, "search_catalog");
        assert_eq!(tool_calls[1].name, "get_table_info");
        assert_eq!(tool_calls[2].name, "query_datasource");
    }

    #[test]
    fn db_message_to_agent_message_tool_with_missing_call_id() {
        // tool_call_id is None — should default to empty string.
        let msg = chat_service::AgentMessage {
            message_id: "m9".into(),
            role: "tool".into(),
            content: "result data".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Tool);
        assert_eq!(result.tool_call_id.as_deref(), Some(""));
        assert_eq!(result.name.as_deref(), Some(""));
    }

    #[test]
    fn db_message_to_agent_message_user_preserves_content() {
        // An AdapterPersists row (Slack/watch, via
        // ChatAgentAdapter::persist_after_chat): the prefix is already baked
        // into `content` as literal text and both new columns are `None` —
        // db_message_to_agent_message must not reconstruct a second prefix
        // on top of it.
        let msg = chat_service::AgentMessage {
            message_id: "m10".into(),
            role: "user".into(),
            content: "[source: web, user_local_time: 2025-01-15T10:00:00+11:00] Show me monthly revenue broken down by region and product category.".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::User);
        // Full content including metadata prefix is preserved, unchanged.
        assert_eq!(
            result.content,
            "[source: web, user_local_time: 2025-01-15T10:00:00+11:00] Show me monthly revenue broken down by region and product category.",
            "content with a prefix already baked in must never gain a second one"
        );
    }

    // -- Contract: db_message_to_agent_message reconstructs the metadata
    // -- prefix from current_time_user_tz / message_source (KYO-506) --------
    //
    // chat_service::prepare_chat_dispatch (KYO-492) stores the RAW user
    // message plus these two columns, unlike the AdapterPersists rows above
    // whose prefix is baked into `content` itself. Without reconstruction
    // here, get_agent_messages hands back the raw text and a later turn's
    // rebuilt LLM context silently loses every earlier turn's
    // source/local-time annotation — this is the exact regression KYO-506
    // fixes.

    #[test]
    fn db_message_to_agent_message_reconstructs_full_prefix_from_columns() {
        let msg = chat_service::AgentMessage {
            message_id: "m10d".into(),
            role: "user".into(),
            content: "what was Q4 revenue".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: Some("2026-08-23T09:00:00+00:00".into()),
            message_source: Some("web".into()),
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::User);
        assert_eq!(
            result.content,
            "[source: web, user_local_time: 2026-08-23T09:00:00+00:00] what was Q4 revenue",
            "both columns present must reconstruct the exact prefix build_metadata_prefix \
             built for the live turn"
        );
    }

    #[test]
    fn db_message_to_agent_message_reconstructs_time_only_when_source_is_absent() {
        // A row with current_time_user_tz but no message_source — either a
        // write site that never captures a source, or a row written before
        // the message_source column existed (e.g. any
        // copilot_service::prepare_copilot_message row from before
        // KYO-554). The reconstructed annotation must degrade to
        // time-only: no source may ever be invented.
        let msg = chat_service::AgentMessage {
            message_id: "m10e".into(),
            role: "user".into(),
            content: "what was Q4 revenue".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: Some("2026-08-23T09:00:00+00:00".into()),
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(
            result.content,
            "[user_local_time: 2026-08-23T09:00:00+00:00] what was Q4 revenue",
            "a missing message_source must never be papered over with a fabricated source"
        );
        assert!(
            !result.content.contains("source:"),
            "no source annotation may appear when message_source is None"
        );
    }

    #[test]
    fn db_message_to_agent_message_reconstructs_source_only_when_time_is_absent() {
        let msg = chat_service::AgentMessage {
            message_id: "m10f".into(),
            role: "user".into(),
            content: "what was Q4 revenue".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: Some("slack".into()),
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(
            result.content,
            "[source: slack] what was Q4 revenue",
            "a missing current_time_user_tz must never be papered over with a fabricated time"
        );
    }

    #[test]
    fn db_message_to_agent_message_adds_no_prefix_for_a_pre_kyo_506_row() {
        // A row written before either column existed: both are None and
        // `content` is raw (never had a prefix baked in). Reconstruction
        // must leave it exactly as stored, not merely "without a source" —
        // there must be no bracket annotation at all.
        let msg = chat_service::AgentMessage {
            message_id: "m10g".into(),
            role: "user".into(),
            content: "what was Q4 revenue".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.content, "what was Q4 revenue");
    }

    #[test]
    fn db_message_to_agent_message_user_preserves_user_id() {
        let msg = chat_service::AgentMessage {
            message_id: "m10b".into(),
            role: "user".into(),
            content: "Hello".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: Some("user-abc-12345678".into()),
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::User);
        assert_eq!(result.user_id.as_deref(), Some("user-abc-12345678"));
    }

    #[test]
    fn db_message_to_agent_message_user_without_user_id() {
        let msg = chat_service::AgentMessage {
            message_id: "m10c".into(),
            role: "user".into(),
            content: "Hello".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::User);
        assert!(result.user_id.is_none());
    }

    #[test]
    fn db_message_to_agent_message_assistant_empty_content_with_tool_calls() {
        // Assistant message with empty content but valid tool calls.
        let tc = serde_json::json!([
            {"id": "tc_1", "name": "search_catalog", "arguments": {"query": "sales"}}
        ]);
        let msg = chat_service::AgentMessage {
            message_id: "m11".into(),
            role: "assistant".into(),
            content: "".into(),
            tool_calls: Some(tc),
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::Assistant);
        assert_eq!(result.content, "");
        assert!(result.tool_calls.is_some());
        assert_eq!(result.tool_calls.as_ref().unwrap().len(), 1);
    }

    // -- Contract: Round-trip: agent Message -> serialize -> deserialize -----

    #[test]
    fn agent_message_roundtrip_user() {
        let msg = Message::user("Show me data");
        let json = serde_json::to_string(&msg).unwrap();
        let restored: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.role, MessageRole::User);
        assert_eq!(restored.content, "Show me data");
    }

    #[test]
    fn agent_message_roundtrip_assistant_with_tool_calls() {
        let tool_calls = vec![
            ToolCall {
                id: "tc_1".into(),
                name: "search_catalog".into(),
                arguments: serde_json::json!({"query": "orders"}),
                arguments_error: None,
            },
        ];
        let msg = Message::assistant_with_tool_calls("Investigating.", tool_calls);
        let json = serde_json::to_string(&msg).unwrap();
        let restored: Message = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.role, MessageRole::Assistant);
        assert_eq!(restored.content, "Investigating.");
        let tc = restored.tool_calls.unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].id, "tc_1");
        assert_eq!(tc[0].name, "search_catalog");
        assert_eq!(tc[0].arguments["query"], "orders");
    }

    #[test]
    fn agent_message_roundtrip_tool_result() {
        let msg = Message::tool_result("tc_abc", "query_datasource", r#"{"rows": [{"id": 1}]}"#);
        let json = serde_json::to_string(&msg).unwrap();
        let restored: Message = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.role, MessageRole::Tool);
        assert_eq!(restored.tool_call_id.as_deref(), Some("tc_abc"));
        assert_eq!(restored.name.as_deref(), Some("query_datasource"));
        assert_eq!(restored.content, r#"{"rows": [{"id": 1}]}"#);
    }

    // -- Contract: Tool call deserialization from DB JSON format -------------

    #[test]
    fn tool_call_deserialization_from_db_format() {
        // DB stores tool_calls as a JSON array.
        let db_json = serde_json::json!([
            {
                "id": "toolu_abc123",
                "name": "search_catalog",
                "arguments": {"query": "revenue", "datasource": "prod-pg"}
            }
        ]);
        let tool_calls: Vec<ToolCall> =
            serde_json::from_value(db_json).expect("should deserialize tool_calls from DB format");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "toolu_abc123");
        assert_eq!(tool_calls[0].arguments["query"], "revenue");
    }

    #[test]
    fn tool_call_deserialization_invalid_json_defaults_to_empty() {
        // When tool_calls JSON is malformed, serde_json::from_value returns
        // an error, and the code falls back to unwrap_or_default().
        let bad_json = serde_json::json!("not an array");
        let tool_calls: Vec<ToolCall> = serde_json::from_value(bad_json).unwrap_or_default();
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn tool_call_deserialization_partial_fields() {
        // Missing optional-like fields should still deserialize (arguments can be null).
        let json = serde_json::json!([
            {"id": "tc_1", "name": "list_datasources", "arguments": null}
        ]);
        let tool_calls: Vec<ToolCall> = serde_json::from_value(json).unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "list_datasources");
        assert!(tool_calls[0].arguments.is_null());
    }

    // -- Contract: db_message_to_agent_message different role values ---------

    #[test]
    fn db_message_to_agent_message_capitalized_role_fallback() {
        // Role values that don't match lowercase fall back to user.
        let msg = chat_service::AgentMessage {
            message_id: "m12".into(),
            role: "Assistant".into(), // capital A
            content: "response".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        // Falls back to user since "Assistant" != "assistant".
        assert_eq!(result.role, MessageRole::User);
    }

    #[test]
    fn db_message_to_agent_message_function_role_fallback() {
        // "function" is not a valid role, falls back to user.
        let msg = chat_service::AgentMessage {
            message_id: "m13".into(),
            role: "function".into(),
            content: "some content".into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        };
        let result = db_message_to_agent_message(&msg);
        assert_eq!(result.role, MessageRole::User);
    }

    // -- Contract: should_persist_new_message (KYO-492) ----------------------

    #[test]
    fn should_persist_new_message_skips_the_message_tagged_with_the_pre_persisted_id() {
        let mut msg = Message::user("hello");
        msg.message_id = Some("pre-persisted-id".into());
        assert!(
            !should_persist_new_message(&msg, Some("pre-persisted-id")),
            "the exact row prepare_chat_dispatch already wrote must not be re-persisted"
        );
    }

    #[test]
    fn should_persist_new_message_persists_a_different_user_message() {
        // A second user turn within the same run (message_id unset, or set
        // to something other than the pre-persisted id) must still be
        // persisted — this is not "skip all user messages", only the one.
        let unset = Message::user("a later turn in the same run");
        assert!(
            should_persist_new_message(&unset, Some("pre-persisted-id")),
            "a user message that isn't the pre-persisted row must still be persisted"
        );

        let mut different_id = Message::user("another later turn");
        different_id.message_id = Some("some-other-id".into());
        assert!(should_persist_new_message(&different_id, Some("pre-persisted-id")));
    }

    #[test]
    fn should_persist_new_message_always_persists_assistant_and_tool_messages() {
        let mut assistant = Message::assistant("here's the answer");
        assistant.message_id = Some("pre-persisted-id".into());
        assert!(
            should_persist_new_message(&assistant, Some("pre-persisted-id")),
            "only a User-role message can be the pre-persisted row; an assistant \
             message must never be skipped even if it happened to carry the same id"
        );

        let tool = Message::tool_result("tc_1", "search_catalog", "{}");
        assert!(should_persist_new_message(&tool, Some("pre-persisted-id")));
    }

    #[test]
    fn should_persist_new_message_persists_everything_when_none() {
        // None means no row was pre-persisted (watch execution's
        // AdapterPersists(None) path) — behaviour must be unchanged from
        // before KYO-492: every new message gets persisted.
        let user = Message::user("hello");
        let assistant = Message::assistant("hi there");
        let tool = Message::tool_result("tc_1", "search_catalog", "{}");

        assert!(should_persist_new_message(&user, None));
        assert!(should_persist_new_message(&assistant, None));
        assert!(should_persist_new_message(&tool, None));
    }

    // -- Contract: UserMessagePersistence::caller_persisted_id (KYO-492 review) --
    //
    // The two arms of UserMessagePersistence exist specifically so that
    // Slack's `AdapterPersists(Some(id))` — a caller-chosen id with no
    // pre-persisted row — can never be mistaken for `CallerPersisted(id)`.
    // A naive rename back to a single `Option<String>` (treating "carries
    // an id" as "already persisted") would make this test fail: Slack
    // would silently stop having its user messages persisted at all.

    #[test]
    fn adapter_persists_with_id_does_not_report_a_caller_persisted_row() {
        let persistence = UserMessagePersistence::AdapterPersists(Some("slack-minted-id".into()));
        assert_eq!(
            persistence.caller_persisted_id(),
            None,
            "AdapterPersists must never be read as a caller-persisted row, even when \
             it carries an id — Slack mints its own id (routes.rs) but still relies \
             on persist_after_chat to do the actual write"
        );
        // tag_id still fires — the id is used for WS-streaming / DB-row
        // matching, just not for skip-persist.
        assert_eq!(persistence.tag_id(), Some("slack-minted-id"));
    }

    #[test]
    fn should_persist_new_message_persists_the_row_for_adapter_persists_with_id() {
        // End-to-end through should_persist_new_message (not just the enum
        // accessor): a message tagged with the AdapterPersists id must
        // still be persisted — this is the exact case that broke when the
        // field was a bare Option<String> and the fix incorrectly proposed
        // renaming it in place. See KYO-492 review finding 2.
        let mut msg = Message::user("hello from slack");
        msg.message_id = Some("slack-minted-id".into());

        let persistence = UserMessagePersistence::AdapterPersists(Some("slack-minted-id".into()));
        assert!(
            should_persist_new_message(&msg, persistence.caller_persisted_id()),
            "AdapterPersists(Some(id)) must still be persisted by persist_after_chat — \
             only CallerPersisted skips"
        );
    }

    #[test]
    fn should_persist_new_message_skips_the_row_for_caller_persisted() {
        let mut msg = Message::user("hello from web chat");
        msg.message_id = Some("caller-written-id".into());

        let persistence = UserMessagePersistence::CallerPersisted("caller-written-id".into());
        assert!(
            !should_persist_new_message(&msg, persistence.caller_persisted_id()),
            "CallerPersisted must still be skipped — prepare_chat_dispatch already \
             wrote this row (KYO-492)"
        );
    }

    // -- Contract: drop_pre_persisted_message (KYO-492) -----------------------

    fn agent_message(message_id: &str, role: &str, content: &str) -> chat_service::AgentMessage {
        chat_service::AgentMessage {
            message_id: message_id.into(),
            role: role.into(),
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            sent_by_user_id: None,
            current_time_user_tz: None,
            message_source: None,
        }
    }

    #[test]
    fn drop_pre_persisted_message_removes_exactly_the_one_row() {
        let db_messages = vec![
            agent_message("m1", "system", "you are helpful"),
            agent_message("m2", "user", "first turn"),
            agent_message("m3", "assistant", "first reply"),
            agent_message("m4", "user", "second turn — the pre-persisted row"),
        ];

        let filtered = drop_pre_persisted_message(db_messages, Some("m4"));

        assert_eq!(
            filtered.len(),
            3,
            "exactly one row (m4) must be dropped, none of the others"
        );
        // Order and content of the surviving rows must be preserved.
        assert_eq!(filtered[0].message_id, "m1");
        assert_eq!(filtered[0].content, "you are helpful");
        assert_eq!(filtered[1].message_id, "m2");
        assert_eq!(filtered[1].content, "first turn");
        assert_eq!(filtered[2].message_id, "m3");
        assert_eq!(filtered[2].content, "first reply");
        assert!(
            filtered.iter().all(|m| m.message_id != "m4"),
            "the pre-persisted row must not survive the filter"
        );
    }

    #[test]
    fn drop_pre_persisted_message_is_a_no_op_when_none() {
        let db_messages = vec![
            agent_message("m1", "user", "hello"),
            agent_message("m2", "assistant", "hi"),
        ];

        let filtered = drop_pre_persisted_message(db_messages.clone(), None);

        assert_eq!(filtered.len(), db_messages.len());
        assert_eq!(filtered[0].message_id, "m1");
        assert_eq!(filtered[1].message_id, "m2");
    }

    // -- Note: Adapter integration contracts --------------------------------
    //
    // The following contract requires a real DB/Redis connection and is
    // covered by an integration test rather than a unit test:
    //
    // - `ChatAgentAdapter::persist_after_chat` when `session_id` is None:
    //   should skip persistence entirely and log a warning.
    //
    // `load_context` restoring history end to end — including the KYO-506
    // metadata-prefix reconstruction — is covered below.

    // -- Contract: ChatAgentAdapter::load_context, end to end (KYO-506) -----

    /// An [`LLMProvider`] that is never called. `load_context()` only reads
    /// the DB and pushes onto `agent.state_mut()` — the LLM is never
    /// consulted — so any real provider would be dead weight here.
    struct UnusedProvider;

    #[async_trait::async_trait]
    impl crate::provider::LLMProvider for UnusedProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::types::Tool],
            _temperature: Option<f32>,
            _max_tokens: u32,
            _user_names: &std::collections::HashMap<String, String>,
        ) -> kyomi_core::Result<crate::types::LLMResponse> {
            unimplemented!("load_context() never calls the LLM provider")
        }

        fn model(&self) -> &str {
            "unused"
        }
    }

    /// Build a `ChatAgentAdapter` wired to a fresh in-memory `db`, for
    /// `user_id`/`session_id`. The wrapped `CustomAgent` is never asked to
    /// `chat()` in these tests — only `load_context()` is exercised.
    fn adapter_over(
        db: kyomi_core::DbPool,
        user_id: &str,
        session_id: &str,
        encryption_key: Arc<[u8; 32]>,
    ) -> ChatAgentAdapter {
        adapter_with_provider(db, user_id, session_id, encryption_key, Box::new(UnusedProvider))
    }

    /// An [`LLMProvider`] that always replies with fixed text and no tool
    /// calls, so `CustomAgent::chat()`'s iteration loop terminates after
    /// exactly one round-trip. Unlike [`UnusedProvider`], this lets a test
    /// drive a full turn through [`ChatAgentAdapter::chat`] end to end
    /// (KYO-554).
    struct TextReplyProvider {
        reply: &'static str,
    }

    #[async_trait::async_trait]
    impl crate::provider::LLMProvider for TextReplyProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::types::Tool],
            _temperature: Option<f32>,
            _max_tokens: u32,
            _user_names: &std::collections::HashMap<String, String>,
        ) -> kyomi_core::Result<crate::types::LLMResponse> {
            Ok(crate::types::LLMResponse {
                content: self.reply.to_string(),
                finish_reason: "end_turn".to_string(),
                usage: crate::types::AgentTokenUsage::default(),
                tool_calls: None,
                cost: None,
                thinking_content: None,
            })
        }

        fn model(&self) -> &str {
            "text-reply-test-provider"
        }
    }

    /// An [`LLMProvider`] whose very first call fails — no message is ever
    /// appended to agent state, so `tag_last_assistant_message_id` has
    /// nothing to tag and the pre-inserted placeholder is left completely
    /// untouched by `persist_after_chat` (KYO-493's terminal-state tests
    /// need exactly this: a clean failure with no intermediate tool-call
    /// message for `tag_last_assistant_message_id`'s "lands on an
    /// intermediate row" edge case to complicate the assertion).
    struct FailingProvider {
        message: &'static str,
    }

    #[async_trait::async_trait]
    impl crate::provider::LLMProvider for FailingProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::types::Tool],
            _temperature: Option<f32>,
            _max_tokens: u32,
            _user_names: &std::collections::HashMap<String, String>,
        ) -> kyomi_core::Result<crate::types::LLMResponse> {
            Err(kyomi_core::Error::Internal(self.message.to_string()))
        }

        fn model(&self) -> &str {
            "failing-test-provider"
        }
    }

    /// Same as [`adapter_over`] but with an explicit provider, for tests
    /// that need to drive `ChatAgentAdapter::chat()` (not just
    /// `load_context()`) and therefore need a provider that actually
    /// replies instead of panicking.
    fn adapter_with_provider(
        db: kyomi_core::DbPool,
        user_id: &str,
        session_id: &str,
        encryption_key: Arc<[u8; 32]>,
        provider: Box<dyn crate::provider::LLMProvider>,
    ) -> ChatAgentAdapter {
        let agent = CustomAgent::new(
            provider,
            crate::agent::AgentConfig::default(),
            Arc::new(crate::tools::ToolRegistry::new()),
            crate::test_support::build_ctx(db.clone()),
            std::collections::HashMap::new(),
        );

        ChatAgentAdapter::new(
            agent,
            user_id.to_string(),
            "ws-1".to_string(),
            Some(session_id.to_string()),
            "custom_agent".to_string(),
            db,
            encryption_key,
        )
    }

    #[tokio::test]
    async fn load_context_reconstructs_the_metadata_prefix_from_stored_columns() {
        // KYO-506: chat_service::prepare_chat_dispatch (KYO-492) stores a
        // user message's RAW content plus current_time_user_tz/message_source
        // in their own columns, not the metadata-prefixed content
        // agent.chat() builds for the live LLM call. Before this fix,
        // load_context() (via get_agent_messages + db_message_to_agent_message)
        // handed that raw content straight to the agent, so turn 2's rebuilt
        // context silently lost turn 1's source/local-time annotation. This
        // test seeds turn 1's row exactly the way prepare_chat_dispatch does,
        // then asserts that loading context for turn 2 restores the
        // annotation.
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key: Arc<[u8; 32]> = Arc::new([7u8; 32]);

        chat_service::create_session_with_id(&db, "user-a", "ws-1", "sess-1", None, "chat", None)
            .await
            .expect("create session");

        chat_service::add_message(
            &db,
            &key,
            "sess-1",
            "user",
            "what was Q4 revenue",
            None,                                      // metadata
            None,                                       // message_id
            Some("2026-08-23T09:00:00+00:00"),          // current_time_user_tz
            Some("web"),                                 // message_source
            Some("user-a"),                              // sent_by_user_id
            None,                                        // tool_call_id
            None,                                        // tool_name
            None,                                        // tool_calls
            chat_service::MessageStatus::Complete,
        )
        .await
        .expect("store turn 1's user message the way prepare_chat_dispatch does");

        let mut adapter = adapter_over(db, "user-a", "sess-1", key);

        // This is exactly what runs at the top of turn 2, before the new
        // user message is appended.
        let loaded = adapter.load_context().await.expect("load_context should succeed");
        assert!(loaded, "a session with one stored message must report context loaded");

        let messages = &adapter.agent.state().messages;
        assert_eq!(messages.len(), 1, "exactly turn 1's user message must be loaded");
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(
            messages[0].content,
            "[source: web, user_local_time: 2026-08-23T09:00:00+00:00] what was Q4 revenue",
            "turn 2's rebuilt context must carry turn 1's source + local-time \
             annotation, not just its raw stored text"
        );
    }

    #[tokio::test]
    async fn load_context_reconstructs_time_only_for_a_row_with_no_message_source() {
        // A row with current_time_user_tz but no message_source (a write
        // site that never captured a source, or a pre-KYO-506 row) must
        // reconstruct a time-only annotation through the full load_context
        // path — never a fabricated source.
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key: Arc<[u8; 32]> = Arc::new([7u8; 32]);

        chat_service::create_session_with_id(&db, "user-a", "ws-1", "sess-2", None, "chat", None)
            .await
            .expect("create session");

        chat_service::add_message(
            &db,
            &key,
            "sess-2",
            "user",
            "what was Q4 revenue",
            None,
            None,
            Some("2026-08-23T09:00:00+00:00"), // current_time_user_tz
            None,                               // message_source — never captured
            Some("user-a"),
            None,
            None,
            None,
            chat_service::MessageStatus::Complete,
        )
        .await
        .expect("store a row with time but no source");

        let mut adapter = adapter_over(db, "user-a", "sess-2", key);
        adapter.load_context().await.expect("load_context should succeed");

        let messages = &adapter.agent.state().messages;
        assert_eq!(
            messages[0].content,
            "[user_local_time: 2026-08-23T09:00:00+00:00] what was Q4 revenue",
            "a missing message_source must never be papered over with a fabricated source"
        );
    }

    // -- Contract: a copilot turn must not double-write the user message
    // -- (KYO-554) --------------------------------------------------------
    //
    // `kyomi_ui::server_fns::copilot::send_copilot_message` calls
    // `kyomi_auth::copilot_service::prepare_copilot_message` to write the
    // user's message row up front, then drives the agent loop with
    // `UserMessagePersistence::CallerPersisted(prep.user_message_id)` — the
    // same contract `chat_service::prepare_chat_dispatch` (KYO-492) uses.
    // `CallerPersisted`'s `caller_persisted_id()` returns the id
    // `prepare_copilot_message` already wrote, so `load_context`'s
    // `drop_pre_persisted_message` drops that row before it re-enters
    // context, and `should_persist_new_message` skips writing it again in
    // `persist_after_chat`. Before this fix, copilot used
    // `AdapterPersists(None)`, which reports no `caller_persisted_id` at
    // all — both of those steps became no-ops, so the row survived into
    // context, `CustomAgent::chat()` pushed the same text again as a new
    // `Message`, and `persist_after_chat` persisted it a second time. This
    // test drives a full turn exactly the way `copilot.rs` configures it
    // and counts the resulting "user"-role rows.

    #[tokio::test]
    async fn copilot_turn_via_adapter_persists_exactly_one_user_message() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key: Arc<[u8; 32]> = Arc::new([7u8; 32]);

        chat_service::create_session_with_id(
            &db,
            "user-a",
            "ws-1",
            "sess-copilot",
            None,
            "dashboard_copilot",
            None,
        )
        .await
        .expect("create session");

        let config = kyomi_core::Config::test_config();

        // Exactly what send_copilot_message does before spawning agent
        // execution: validate, check capabilities, verify session access,
        // and store the user message + assistant placeholder.
        let prep = kyomi_auth::copilot_service::prepare_copilot_message(
            kyomi_auth::copilot_service::CopilotMessageInputs {
                db: &db,
                encryption_key: &key,
                config: &config,
                workspace_id: "ws-1",
                user_id: "user-a",
                session_id: "sess-copilot",
                message: "what was Q4 revenue",
                content: None,
                current_time_user_tz: Some("2026-08-23T09:00:00+00:00"),
                message_source: Some("web"),
            },
        )
        .await
        .expect("prepare_copilot_message should succeed, exactly as send_copilot_message calls it");

        let mut adapter = adapter_with_provider(
            db.clone(),
            "user-a",
            "sess-copilot",
            key.clone(),
            Box::new(TextReplyProvider {
                reply: "Q4 revenue was $1M.",
            }),
        );

        // Exactly the persistence choice send_copilot_message configures
        // post-fix: CallerPersisted(user_message_id) — the row
        // prepare_copilot_message already wrote.
        let persistence = UserMessagePersistence::CallerPersisted(prep.user_message_id.clone());
        // Exactly the persistence choice send_copilot_message configures:
        // AdapterInserts(Some(id)) — copilot mints the assistant id but
        // (KYO-572) deliberately never pre-inserts a placeholder for it.
        let assistant_persistence =
            AssistantMessagePersistence::AdapterInserts(Some(prep.assistant_message_id.clone()));
        adapter
            .chat(ChatParams {
                message: &prep.user_message,
                cancel_token: CancellationToken::new(),
                current_time_user_tz: Some("2026-08-23T09:00:00+00:00"),
                message_source: Some("web"),
                user_id: Some("user-a"),
                user_message_persistence: &persistence,
                assistant_message_persistence: &assistant_persistence,
            })
            .await
            .expect("chat() should succeed");

        let stored = chat_service::get_agent_messages(&db, &key, "sess-copilot", None)
            .await
            .expect("get_agent_messages should succeed");

        let user_rows: Vec<&chat_service::AgentMessage> =
            stored.iter().filter(|m| m.role == "user").collect();

        assert_eq!(
            user_rows.len(),
            1,
            "exactly one user-role row must exist for this single turn; found {}: {:?}",
            user_rows.len(),
            user_rows.iter().map(|m| &m.content).collect::<Vec<_>>()
        );
    }

    // -- Contract: the assistant's real reply must be durably persisted
    // -- (KYO-572) ----------------------------------------------------------
    //
    // Before the fix, `copilot_service::prepare_copilot_message` INSERTed an
    // empty assistant placeholder row up front. `persist_after_chat` then
    // tried to INSERT the real assistant reply under the *same*
    // `message_id` — `chat_messages.message_id` is the primary key and
    // `add_message` has no ON CONFLICT handling, so that second INSERT
    // violated the PK, aborted `persist_after_chat` mid-loop, and the error
    // was swallowed by `chat()`'s `if let Err(e) = ...` logging. The DB row
    // was left permanently empty.
    //
    // This test deliberately does NOT use `get_agent_messages` — it filters
    // out empty-content assistant rows with no tool_calls, which is exactly
    // why the pre-existing test above stayed green through this bug. It
    // reads back via `get_session_messages` (no such filter) and asserts the
    // assistant row's decrypted content equals the provider's real reply.
    #[tokio::test]
    async fn copilot_turn_via_adapter_persists_real_assistant_content() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key: Arc<[u8; 32]> = Arc::new([7u8; 32]);

        chat_service::create_session_with_id(
            &db,
            "user-a",
            "ws-1",
            "sess-copilot-2",
            None,
            "dashboard_copilot",
            None,
        )
        .await
        .expect("create session");

        let config = kyomi_core::Config::test_config();

        // Exactly what send_copilot_message does before spawning agent
        // execution: validate, check capabilities, verify session access,
        // store the user message, and mint the assistant message id.
        let prep = kyomi_auth::copilot_service::prepare_copilot_message(
            kyomi_auth::copilot_service::CopilotMessageInputs {
                db: &db,
                encryption_key: &key,
                config: &config,
                workspace_id: "ws-1",
                user_id: "user-a",
                session_id: "sess-copilot-2",
                message: "what was Q4 revenue",
                content: None,
                current_time_user_tz: Some("2026-08-23T09:00:00+00:00"),
                message_source: Some("web"),
            },
        )
        .await
        .expect("prepare_copilot_message should succeed, exactly as send_copilot_message calls it");

        const REAL_REPLY: &str = "Q4 revenue was $1M, up 12% quarter over quarter.";

        let mut adapter = adapter_with_provider(
            db.clone(),
            "user-a",
            "sess-copilot-2",
            key.clone(),
            Box::new(TextReplyProvider { reply: REAL_REPLY }),
        );

        let persistence = UserMessagePersistence::CallerPersisted(prep.user_message_id.clone());
        let assistant_persistence =
            AssistantMessagePersistence::AdapterInserts(Some(prep.assistant_message_id.clone()));
        adapter
            .chat(ChatParams {
                message: &prep.user_message,
                cancel_token: CancellationToken::new(),
                current_time_user_tz: Some("2026-08-23T09:00:00+00:00"),
                message_source: Some("web"),
                user_id: Some("user-a"),
                user_message_persistence: &persistence,
                assistant_message_persistence: &assistant_persistence,
            })
            .await
            .expect("chat() should succeed");

        // get_session_messages does NOT filter out empty-content assistant
        // rows — unlike get_agent_messages, it is the right read path to
        // prove the row actually holds the real reply.
        let stored = chat_service::get_session_messages(&db, &key, "sess-copilot-2", 100)
            .await
            .expect("get_session_messages should succeed");

        let assistant_rows: Vec<&chat_service::MessageItem> = stored
            .iter()
            .filter(|m| m.message_type == "assistant")
            .collect();

        assert_eq!(
            assistant_rows.len(),
            1,
            "exactly one assistant-role row must exist for this single turn; found {}: {:?}",
            assistant_rows.len(),
            assistant_rows.iter().map(|m| &m.content).collect::<Vec<_>>()
        );
        assert_eq!(
            assistant_rows[0].message_id, prep.assistant_message_id,
            "the persisted row must carry the same id used for WebSocket streaming"
        );
        assert_eq!(
            assistant_rows[0].content, REAL_REPLY,
            "the assistant row must hold the provider's real reply, not be left empty by a \
             swallowed primary-key collision (KYO-572)"
        );
    }

    // -- Contract: chat's pre-inserted placeholder is UPDATEd, never
    // -- INSERTed a second time, and terminal outcomes persist correctly
    // -- (KYO-493) ----------------------------------------------------------
    //
    // Unlike copilot (KYO-572, deliberately no placeholder), the main chat
    // surface (`kyomi_auth::chat_service::prepare_chat_dispatch`) DOES
    // pre-insert an empty, `status = 'in_progress'` placeholder before the
    // agent runs, and configures `AssistantMessagePersistence::CallerPreInserted`
    // over it. These tests drive that real dispatch seam (not a hand-rolled
    // stand-in) followed by a real turn, so a regression back to plain
    // INSERT would fail these exactly as the collision failed KYO-572's.
    //
    // The last two compose the same finalization steps
    // `kyomi_agent::execution::execute_agent_chat`'s step 13/15 perform
    // (`classify_agent_failure` + `finalize_terminal_write` +
    // `chat_service::update_message`) around a real `adapter.chat()` call —
    // `execute_agent_chat` itself resolves a real, network-calling LLM
    // provider from workspace config, which makes it impractical to drive
    // directly in a unit test (see that function's own doc).

    /// Submit through the production durable acceptance seam. Callers must
    /// claim its run before using the resulting placeholder.
    async fn submit_durable_turn(
        db: &kyomi_core::DbPool,
        key: &Arc<[u8; 32]>,
        session_id: &str,
        message: &str,
    ) -> (UserMessagePersistence, String) {
        let outcome = kyomi_auth::chat_service::prepare_chat_dispatch(
            kyomi_auth::chat_service::ChatDispatchParams {
                db,
                encryption_key: key,
                ws_manager: None,
                user_id: "user-a",
                workspace_id: "ws-1",
                user_display_name: "User A",
                session_id,
                is_new_session: true,
                message,
                current_time_user_tz: None,
                message_source: Some("web"),
                skip_ai: false,
                client_msg_id: None,
                owner_instance: "test-instance",
                execution_context: None,
            },
        )
        .await
        .expect("prepare_chat_dispatch should succeed, exactly as send_chat_message calls it");

        let kyomi_auth::chat_service::ChatDispatchOutcome::Ready {
            user_message_id, assistant_message_id, ..
        } = outcome
        else {
            panic!("skip_ai=false must return Ready");
        };

        (
            UserMessagePersistence::CallerPersisted(user_message_id),
            assistant_message_id,
        )
    }

    /// Legacy CallerPreInserted compatibility uses an ordinary placeholder,
    /// with no journal run. Durable fixtures must never use this variant.
    async fn dispatch_chat_turn(
        db: &kyomi_core::DbPool,
        key: &Arc<[u8; 32]>,
        session_id: &str,
        message: &str,
    ) -> (UserMessagePersistence, AssistantMessagePersistence) {
        chat_service::create_session_with_id(db, "user-a", "ws-1", session_id, None, "chat", None)
            .await.expect("create legacy conversation");
        let user_id = chat_service::add_message(db, key, session_id, "user", message,
            None, None, None, Some("web"), Some("user-a"), None, None, None,
            chat_service::MessageStatus::Complete).await.expect("legacy caller persists user");
        let assistant_id = chat_service::add_message(db, key, session_id, "assistant", "",
            None, None, None, None, None, None, None, None,
            chat_service::MessageStatus::InProgress).await.expect("legacy caller inserts placeholder");
        let journal_rows = kyomi_core::db_fetch_scalar!(db, i64,
            "SELECT COUNT(*) FROM conversation_runs WHERE session_id=$1", session_id).expect("check legacy scope");
        assert_eq!(journal_rows, 0, "legacy fixture cannot bypass durable ownership");
        (UserMessagePersistence::CallerPersisted(user_id), AssistantMessagePersistence::CallerPreInserted(assistant_id))
    }

    async fn claim_durable_turn(
        db: &kyomi_core::DbPool,
        key: &Arc<[u8; 32]>,
        session_id: &str,
        claim_time: i64,
        ttl: i64,
    ) -> (UserMessagePersistence, AssistantMessagePersistence) {
        claim_durable_message(db, key, session_id, "hello", claim_time, ttl).await
    }

    async fn claim_durable_message(
        db: &kyomi_core::DbPool,
        key: &Arc<[u8; 32]>,
        session_id: &str,
        message: &str,
        claim_time: i64,
        ttl: i64,
    ) -> (UserMessagePersistence, AssistantMessagePersistence) {
        let (user, assistant) = submit_durable_turn(db, key, session_id, message).await;
        let store = kyomi_auth::conversation_events::ConversationStore::new(db, key, "user-a", "ws-1");
        let conversation = agent_runtime::ConversationId(session_id.to_string());
        let run_id = store.find_run_by_assistant(&conversation, assistant.as_str())
            .await.expect("lookup accepted run").expect("accepted run");
        let claimed = store.claim(&conversation, &run_id, "worker", claim_time, ttl).await.expect("claim");
        (user, AssistantMessagePersistence::Durable {
            message_id: assistant.as_str().to_string(),
            run: DurableRun { conversation_id: conversation, run_id, lease: claimed.snapshot.lease.expect("lease"), context: Arc::new(claimed.context) },
        })
    }

    #[tokio::test]
    async fn durable_context_is_fixed_at_claim_and_next_claim_sees_committed_terminal() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key = Arc::new([7u8; 32]);
        let session_id = uuid::Uuid::new_v4().to_string();
        let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
        chat_service::add_message(&db, &key, &session_id, "assistant", "late history", None, None, None, None, None, None, None, None, chat_service::MessageStatus::Complete).await.expect("commit an edit after claim");
        chat_service::update_session(&db, &session_id, None, None, Some(&serde_json::json!({"agent_state":{"compacted_summary":"changed after claim"}}))).await.expect("commit compaction edit after claim");
        let mut first = adapter_over(db.clone(), "user-a", &session_id, key.clone());
        first.user_message_persistence = user;
        first.assistant_message_persistence = assistant.clone();
        first.load_context().await.expect("restore claim snapshot");
        assert!(!first.agent_state().messages.iter().any(|message| message.content == "late history"));
        assert_eq!(first.agent_state().compacted_summary, None);
        let owned = assistant.durable_run().expect("durable");
        let store = kyomi_auth::conversation_events::ConversationStore::new(&db, &key, "user-a", "ws-1");
        store.finish(&owned.conversation_id, &owned.run_id, &owned.lease, chrono::Utc::now().timestamp_millis(), agent_runtime::RunState::Completed, "first committed answer").await.expect("commit preceding terminal result");
        let accepted = chat_service::prepare_chat_dispatch(chat_service::ChatDispatchParams {
            db: &db, encryption_key: &key, ws_manager: None, user_id: "user-a", workspace_id: "ws-1", user_display_name: "Test",
            session_id: &session_id, is_new_session: false, message: "next turn", current_time_user_tz: None, message_source: Some("web"),
            skip_ai: false, client_msg_id: Some("next-claim-request"), owner_instance: "test", execution_context: None,
        }).await.expect("accept next turn");
        let chat_service::ChatDispatchOutcome::Ready {run_id,user_message_id,assistant_message_id,..} = accepted else {panic!("ready");};
        let run_id = agent_runtime::RunId(run_id);
        let claim = store.claim(&owned.conversation_id, &run_id, "second-worker", chrono::Utc::now().timestamp_millis(), 30_000).await.expect("claim next turn");
        let mut next = adapter_over(db.clone(), "user-a", &session_id, key.clone());
        next.user_message_persistence = UserMessagePersistence::CallerPersisted(user_message_id);
        next.assistant_message_persistence = AssistantMessagePersistence::Durable {
            message_id: assistant_message_id,
            run: DurableRun {conversation_id:owned.conversation_id.clone(),run_id,lease:claim.snapshot.lease.expect("lease"),context:Arc::new(claim.context)},
        };
        next.load_context().await.expect("restore next claim snapshot");
        assert!(next.agent_state().messages.iter().any(|message| message.content == "first committed answer"));
        assert!(next.agent_state().messages.iter().any(|message| message.content == "late history"));
        assert_eq!(next.agent_state().compacted_summary.as_deref(), Some("changed after claim"));
    }

    #[tokio::test]
    async fn durable_adapter_keeps_placeholder_running_until_atomic_finish() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key = Arc::new([7u8; 32]);
        let session_id = uuid::Uuid::new_v4().to_string();
        let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
        let mut adapter = adapter_with_provider(db.clone(), "user-a", &session_id, key.clone(), Box::new(TextReplyProvider { reply: "durable answer" }));
        let answer = adapter.chat(ChatParams {
            message: "hello", cancel_token: CancellationToken::new(), current_time_user_tz: None,
            message_source: Some("web"), user_id: Some("user-a"), user_message_persistence: &user,
            assistant_message_persistence: &assistant,
        }).await.expect("provider and fenced writes succeed");
        let amid = assistant.tag_id().expect("assistant id");
        assert_eq!(chat_service::get_message_status(&db, amid).await.expect("read").expect("row").0, chat_service::MessageStatus::InProgress);
        let run = assistant.durable_run().expect("durable");
        kyomi_auth::conversation_events::ConversationStore::new(&db, &key, "user-a", "ws-1")
            .finish(&run.conversation_id, &run.run_id, &run.lease, chrono::Utc::now().timestamp_millis(), agent_runtime::RunState::Completed, &answer)
            .await.expect("atomic terminal commit");
        let rows = chat_service::get_session_messages(&db, &key, &session_id, 100).await.expect("read projections");
        let terminal = rows.iter().filter(|row| row.message_id == amid).collect::<Vec<_>>();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].content, "durable answer");
        assert_eq!(terminal[0].status, "complete");
    }

    #[tokio::test]
    async fn durable_compaction_state_is_encrypted_and_restored_by_legacy_context_loading() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key = Arc::new([7u8; 32]);
        let session_id = uuid::Uuid::new_v4().to_string();
        let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
        const SUMMARY: &str = "private warehouse summary retained across adapter boundaries";
        let mut durable = adapter_with_provider(db.clone(), "user-a", &session_id, key.clone(), Box::new(TextReplyProvider { reply: "answer" }));
        durable.agent.state_mut().compacted_summary = Some(SUMMARY.to_string());
        durable.chat(ChatParams {
            message: "hello", cancel_token: CancellationToken::new(), current_time_user_tz: None,
            message_source: Some("web"), user_id: Some("user-a"), user_message_persistence: &user,
            assistant_message_persistence: &assistant,
        }).await.expect("persist durable compaction state");
        let raw = chat_service::get_session(&db, &session_id).await.expect("raw session").expect("session");
        assert!(!raw.config.expect("persisted state").to_string().contains(SUMMARY), "compatibility config keeps sensitive summaries encrypted");
        let mut legacy = adapter_over(db.clone(), "user-a", &session_id, key.clone());
        legacy.load_context().await.expect("legacy adapter restores encrypted config");
        assert_eq!(legacy.agent_state().compacted_summary.as_deref(), Some(SUMMARY));
    }

    #[tokio::test]
    async fn durable_initial_turn_with_system_prompt_does_not_duplicate_user_at_iteration_boundary() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key = Arc::new([7u8; 32]);
        let session_id = uuid::Uuid::new_v4().to_string();
        let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
        let provider = crate::test_support::ScriptedProvider {
            script: std::sync::Mutex::new(vec![crate::test_support::Reply::ToolCall, crate::test_support::Reply::Text("answer".into())].into()),
            after_script: crate::test_support::Reply::Text("unexpected".into()),
            calls: Arc::new(std::sync::Mutex::new(Vec::new())),
        };
        let mut adapter = adapter_with_provider(db.clone(), "user-a", &session_id, key.clone(), Box::new(provider));
        // execute_agent_chat inserts a system message before load_context. With no
        // prior history it must still count toward the incremental write index.
        adapter.agent.state_mut().messages.push(Message::system("system prompt"));
        adapter.chat(ChatParams {
            message: "hello", cancel_token: CancellationToken::new(), current_time_user_tz: None,
            message_source: Some("web"), user_id: Some("user-a"), user_message_persistence: &user,
            assistant_message_persistence: &assistant,
        }).await.expect("complete scripted turn");
        let history = chat_service::get_agent_messages(&db, &key, &session_id, None).await.expect("committed history");
        assert_eq!(history.iter().filter(|message| message.role == "user").count(), 1, "accepted user row is never reinserted after a tool round");
        assert!(history.iter().any(|message| message.role == "tool"), "complete tool history is retained");
    }

    #[tokio::test]
    async fn durable_history_retry_uses_position_identity_without_collapsing_equal_messages() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key = Arc::new([7u8; 32]);
        let session_id = uuid::Uuid::new_v4().to_string();
        let (_, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
        let message = Message::assistant("repeated complete response");
        for index in [42, 42, 43] {
            persist_one_new_message(&db, &key, &session_id, &message, None, assistant.tag_id(), MessagePersistenceScope {
                durable_run: assistant.durable_run(), user_id: "user-a", workspace_id: "ws-1", message_index: index,
            }).await.expect("fenced immutable response commit/retry");
        }
        let history = chat_service::get_agent_messages(&db, &key, &session_id, None).await.expect("load committed history");
        assert_eq!(history.iter().filter(|row| row.content == message.content).count(), 2,
            "one retried position and a distinct identical response each have one record");
    }

    #[tokio::test]
    async fn durable_adapter_propagates_stale_fence_without_writing_messages() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key = Arc::new([7u8; 32]);
        let session_id = uuid::Uuid::new_v4().to_string();
        let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis() - 1_000, 1).await;
        let mut adapter = adapter_with_provider(db.clone(), "user-a", &session_id, key.clone(), Box::new(TextReplyProvider { reply: "must not persist" }));
        assert!(adapter.chat(ChatParams {
            message: "hello", cancel_token: CancellationToken::new(), current_time_user_tz: None,
            message_source: Some("web"), user_id: Some("user-a"), user_message_persistence: &user,
            assistant_message_persistence: &assistant,
        }).await.is_err(), "stale persistence must fail the execution");
        let rows = chat_service::get_session_messages(&db, &key, &session_id, 100).await.expect("read projections");
        assert_eq!(rows.len(), 2, "only atomic submission rows survive");
        assert!(rows.iter().all(|row| row.content != "must not persist"));
        assert_eq!(rows.iter().find(|row| row.message_id == assistant.tag_id().expect("id")).expect("placeholder").status, "in_progress");
    }

    #[tokio::test]
    async fn durable_first_provider_error_has_one_truthful_terminal_projection() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key = Arc::new([7u8; 32]);
        let session_id = uuid::Uuid::new_v4().to_string();
        let (user, assistant) = claim_durable_turn(&db, &key, &session_id, chrono::Utc::now().timestamp_millis(), 30_000).await;
        let mut adapter = adapter_with_provider(db.clone(), "user-a", &session_id, key.clone(), Box::new(FailingProvider { message: "provider cancelled upstream" }));
        let error = adapter.chat(ChatParams {
            message: "hello", cancel_token: CancellationToken::new(), current_time_user_tz: None,
            message_source: Some("web"), user_id: Some("user-a"), user_message_persistence: &user,
            assistant_message_persistence: &assistant,
        }).await.expect_err("first provider fails");
        let (status, response) = crate::execution::classify_agent_failure(&error);
        let (state, content) = crate::durable_chat::terminal_outcome(status, &response);
        let run = assistant.durable_run().expect("durable");
        kyomi_auth::conversation_events::ConversationStore::new(&db, &key, "user-a", "ws-1")
            .finish(&run.conversation_id, &run.run_id, &run.lease, chrono::Utc::now().timestamp_millis(), state, content)
            .await.expect("persist failure before any provider response");
        let rows = chat_service::get_session_messages(&db, &key, &session_id, 100).await.expect("read projections");
        let assistants = rows.iter().filter(|row| row.message_type == "assistant").collect::<Vec<_>>();
        assert_eq!(assistants.len(), 1);
        assert_eq!(assistants[0].content, content);
        assert_eq!(assistants[0].status, "error");
    }

    #[tokio::test]
    async fn preinserted_placeholder_completed_run_updates_the_row_not_insert() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key: Arc<[u8; 32]> = Arc::new([7u8; 32]);
        let session_id = uuid::Uuid::new_v4().to_string();

        const REAL_REPLY: &str = "Q4 revenue was $1M.";
        let (user_persistence, assistant_persistence) =
            dispatch_chat_turn(&db, &key, &session_id, "what was Q4 revenue").await;
        let assistant_message_id = assistant_persistence.tag_id().expect("CallerPreInserted has an id").to_string();

        let mut adapter = adapter_with_provider(
            db.clone(),
            "user-a",
            &session_id,
            key.clone(),
            Box::new(TextReplyProvider { reply: REAL_REPLY }),
        );
        adapter
            .chat(ChatParams {
                message: "what was Q4 revenue",
                cancel_token: CancellationToken::new(),
                current_time_user_tz: None,
                message_source: Some("web"),
                user_id: Some("user-a"),
                user_message_persistence: &user_persistence,
                assistant_message_persistence: &assistant_persistence,
            })
            .await
            .expect("chat() should succeed");

        let stored = chat_service::get_session_messages(&db, &key, &session_id, 100)
            .await
            .expect("get_session_messages should succeed");
        let assistant_rows: Vec<&chat_service::MessageItem> =
            stored.iter().filter(|m| m.message_type == "assistant").collect();

        assert_eq!(
            assistant_rows.len(),
            1,
            "exactly one assistant row must exist — a regression back to plain INSERT \
             would either duplicate-key-fail (swallowed by persist_after_chat's \
             error-log-only handling, per KYO-572) or produce a second row; found {}: {:?}",
            assistant_rows.len(),
            assistant_rows.iter().map(|m| &m.content).collect::<Vec<_>>()
        );
        assert_eq!(assistant_rows[0].message_id, assistant_message_id);
        assert_eq!(
            assistant_rows[0].content, REAL_REPLY,
            "the placeholder's content must have been UPDATEd to the real reply"
        );
    }

    #[tokio::test]
    async fn preinserted_placeholder_in_loop_error_persists_error_text_and_status() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key: Arc<[u8; 32]> = Arc::new([7u8; 32]);
        let session_id = uuid::Uuid::new_v4().to_string();

        let (user_persistence, assistant_persistence) =
            dispatch_chat_turn(&db, &key, &session_id, "what was Q4 revenue").await;
        let assistant_message_id = assistant_persistence.tag_id().expect("has an id").to_string();

        let mut adapter = adapter_with_provider(
            db.clone(),
            "user-a",
            &session_id,
            key.clone(),
            Box::new(FailingProvider { message: "workspace AI config could not be loaded" }),
        );
        let result = adapter
            .chat(ChatParams {
                message: "what was Q4 revenue",
                cancel_token: CancellationToken::new(),
                current_time_user_tz: None,
                message_source: Some("web"),
                user_id: Some("user-a"),
                user_message_persistence: &user_persistence,
                assistant_message_persistence: &assistant_persistence,
            })
            .await;
        let err = result.expect_err("FailingProvider must make chat() return Err");

        // Exactly what execute_agent_chat's step 13/15 do with that Err.
        let (exec_status, response_text) = crate::execution::classify_agent_failure(&err);
        assert_eq!(exec_status, "error");
        let (content, message_status) =
            crate::execution::finalize_terminal_write(exec_status, &response_text);
        chat_service::update_message(&db, &key, &assistant_message_id, content, None, Some(message_status))
            .await
            .expect("update_message should succeed");

        let (status, _owner) = chat_service::get_message_status(&db, &assistant_message_id)
            .await
            .expect("get_message_status should succeed")
            .expect("the placeholder row must still exist");
        assert_eq!(status, chat_service::MessageStatus::Error);

        let stored = chat_service::get_session_messages(&db, &key, &session_id, 100)
            .await
            .expect("get_session_messages should succeed");
        let assistant_rows: Vec<&chat_service::MessageItem> =
            stored.iter().filter(|m| m.message_type == "assistant").collect();
        assert_eq!(
            assistant_rows.len(),
            1,
            "exactly one assistant row, carrying the placeholder id, even though the \
             turn never produced a real answer"
        );
        assert!(
            assistant_rows[0].content.contains("workspace AI config could not be loaded"),
            "today an in-loop error is streamed to the client but never persisted \
             (KYO-493) — this must no longer be true; got {:?}",
            assistant_rows[0].content
        );
    }

    #[tokio::test]
    async fn preinserted_placeholder_cancelled_run_persists_cancellation_notice_and_status() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key: Arc<[u8; 32]> = Arc::new([7u8; 32]);
        let session_id = uuid::Uuid::new_v4().to_string();

        let (user_persistence, assistant_persistence) =
            dispatch_chat_turn(&db, &key, &session_id, "what was Q4 revenue").await;
        let assistant_message_id = assistant_persistence.tag_id().expect("has an id").to_string();

        // Matches the exact text crate::agent constructs on cancellation
        // (e.g. agent.rs's cancel-token checks) — classify_agent_failure
        // keys off this substring.
        let mut adapter = adapter_with_provider(
            db.clone(),
            "user-a",
            &session_id,
            key.clone(),
            Box::new(FailingProvider { message: "Request cancelled" }),
        );
        let result = adapter
            .chat(ChatParams {
                message: "what was Q4 revenue",
                cancel_token: CancellationToken::new(),
                current_time_user_tz: None,
                message_source: Some("web"),
                user_id: Some("user-a"),
                user_message_persistence: &user_persistence,
                assistant_message_persistence: &assistant_persistence,
            })
            .await;
        let err = result.expect_err("FailingProvider must make chat() return Err");

        let (exec_status, response_text) = crate::execution::classify_agent_failure(&err);
        assert_eq!(exec_status, "cancelled");
        assert_eq!(response_text, "Request was cancelled.");
        let (content, message_status) =
            crate::execution::finalize_terminal_write(exec_status, &response_text);
        chat_service::update_message(&db, &key, &assistant_message_id, content, None, Some(message_status))
            .await
            .expect("update_message should succeed");

        let (status, _owner) = chat_service::get_message_status(&db, &assistant_message_id)
            .await
            .expect("get_message_status should succeed")
            .expect("the placeholder row must still exist");
        assert_eq!(status, chat_service::MessageStatus::Cancelled);

        let stored = chat_service::get_session_messages(&db, &key, &session_id, 100)
            .await
            .expect("get_session_messages should succeed");
        let assistant_rows: Vec<&chat_service::MessageItem> =
            stored.iter().filter(|m| m.message_type == "assistant").collect();
        assert_eq!(assistant_rows.len(), 1);
        assert_eq!(assistant_rows[0].content, "Request was cancelled.");
    }

    // -- Contract: a watch turn must not double-write the user message,
    // -- and must record message_source (KYO-573) --------------------------
    //
    // `kyomi_agent::watch_execution::execute_watch_inner` pre-writes the
    // human-readable watch prompt ("Monitor: {name}\n\n{prompt}") as a user
    // message row before spawning the agent, via
    // `watch_execution::prepare_watch_dispatch` — which mints the row's id
    // up front and returns `UserMessagePersistence::CallerPersisted` over
    // that id — the same contract KYO-554 established for copilot via
    // `copilot_service::prepare_copilot_message`. Before this fix
    // `execute_watch_inner` used `AdapterPersists(None)` over the
    // pre-written row instead: `caller_persisted_id()` returned `None`, so
    // `load_context` never dropped the pre-written row from context and
    // `CustomAgent::chat()` pushed the *enhanced* watch prompt
    // (workspace-learnings prefix + watch.prompt — different text from the
    // pre-written row) as a second user message, which `persist_after_chat`
    // then persisted for real — two distinct user rows per watch run. The
    // pre-written row was also always stored with `message_source: None`,
    // even though the exec config passed to the agent loop already knew the
    // source was "Kyomi Watch" — so a later turn's `load_context` could
    // never reconstruct the `[source: Kyomi Watch, ...]` annotation
    // (KYO-506).
    //
    // This helper calls `prepare_watch_dispatch` itself — the same
    // production function `execute_watch_inner` calls — rather than
    // replicating its body, so a regression in that function's behaviour
    // (e.g. reverting to `AdapterPersists(None)`) is guaranteed to fail
    // these tests too. It then drives one full turn exactly the way
    // `execute_watch_inner` configures it post-fix, and returns every
    // `user`-role row left in the session for the two tests below to make
    // their own assertions against.
    async fn run_watch_turn_and_fetch_user_rows(
        db: kyomi_core::DbPool,
        key: Arc<[u8; 32]>,
        session_id: &str,
    ) -> Vec<chat_service::AgentMessage> {
        let watch_name = "Revenue Monitor";
        let watch_prompt = "Alert me if revenue drops more than 10% week over week.";

        // Exactly what execute_watch_inner does before spawning agent
        // execution: go through the real dispatch-preparation seam, which
        // mints the id, pre-writes the human-readable prompt as the durable
        // user row with message_source recorded, and hands back the
        // persistence value to drive the turn with.
        let watch_dispatch = crate::watch_execution::prepare_watch_dispatch(
            &db,
            &key,
            session_id,
            watch_name,
            watch_prompt,
            "user-a",
        )
        .await
        .expect("prepare_watch_dispatch should succeed, exactly as execute_watch_inner calls it");

        let mut adapter = adapter_with_provider(
            db.clone(),
            "user-a",
            session_id,
            key.clone(),
            Box::new(TextReplyProvider {
                reply: r#"{"should_alert": false, "reason": "no anomaly"}"#,
            }),
        );

        // Exactly the persistence choice execute_watch_inner configures
        // post-fix: the `UserMessagePersistence::CallerPersisted` value
        // `prepare_watch_dispatch` returned, over the row it already wrote
        // above. The LLM turn itself still uses the *enhanced* prompt
        // (workspace-learnings prefix + watch.prompt) — distinct text from
        // the pre-written row — exactly as the exec config's `message`
        // field does.
        let enhanced_watch_prompt = format!("Workspace learnings: none.\n{watch_prompt}");
        adapter
            .chat(ChatParams {
                message: &enhanced_watch_prompt,
                cancel_token: CancellationToken::new(),
                current_time_user_tz: None,
                message_source: Some("Kyomi Watch"),
                user_id: Some("user-a"),
                user_message_persistence: &watch_dispatch.user_message_persistence,
                assistant_message_persistence: &AssistantMessagePersistence::AdapterInserts(None),
            })
            .await
            .expect("chat() should succeed");

        let user_rows: Vec<chat_service::AgentMessage> =
            chat_service::get_agent_messages(&db, &key, session_id, None)
                .await
                .expect("get_agent_messages should succeed")
                .into_iter()
                .filter(|m| m.role == "user")
                .collect();

        // Sanity check baked into the shared helper (not just the "exactly
        // one row" test below): whichever row survived must be the one
        // `prepare_watch_dispatch` pre-wrote, identified by the id it
        // minted and handed back in `watch_dispatch.user_message_id` — not
        // some other row a broken implementation might persist instead.
        if let [only_row] = user_rows.as_slice() {
            assert_eq!(
                only_row.message_id, watch_dispatch.user_message_id,
                "the surviving user row must be the one prepare_watch_dispatch pre-wrote"
            );
        }

        user_rows
    }

    #[tokio::test]
    async fn watch_turn_via_adapter_persists_exactly_one_user_message() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key: Arc<[u8; 32]> = Arc::new([9u8; 32]);

        chat_service::create_session_with_id(
            &db,
            "user-a",
            "ws-1",
            "sess-watch-1",
            Some("Watch: Revenue Monitor"),
            "watch_execution",
            None,
        )
        .await
        .expect("create session");

        let user_rows =
            run_watch_turn_and_fetch_user_rows(db.clone(), key.clone(), "sess-watch-1").await;

        assert_eq!(
            user_rows.len(),
            1,
            "exactly one user-role row must exist for this single watch turn; found {}: {:?}",
            user_rows.len(),
            user_rows.iter().map(|m| &m.content).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn watch_turn_via_adapter_persists_kyomi_watch_message_source() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key: Arc<[u8; 32]> = Arc::new([9u8; 32]);

        chat_service::create_session_with_id(
            &db,
            "user-a",
            "ws-1",
            "sess-watch-2",
            Some("Watch: Revenue Monitor"),
            "watch_execution",
            None,
        )
        .await
        .expect("create session");

        let user_rows =
            run_watch_turn_and_fetch_user_rows(db.clone(), key.clone(), "sess-watch-2").await;

        assert_eq!(
            user_rows.len(),
            1,
            "sanity: exactly one user row must exist before checking its message_source"
        );
        assert_eq!(
            user_rows[0].message_source.as_deref(),
            Some("Kyomi Watch"),
            "the persisted watch user row must carry message_source so a later turn's \
             load_context can reconstruct the [source: Kyomi Watch, ...] annotation; got {:?}",
            user_rows[0].message_source
        );
    }

    // -- Contract: intermediate messages are persisted at each agent-loop
    // -- iteration boundary, not only at the end of the turn (KYO-493
    // -- phase 3) --------------------------------------------------------
    //
    // Before this fix, `persist_after_chat` was the *only* writer of new
    // messages, and it ran once, after `CustomAgent::chat()`'s whole
    // iteration loop returned. A chat re-entered mid-run therefore saw no
    // tool-call/tool-result rows at all for the turn in progress — the
    // failure scenario the ticket describes. These tests drive a real
    // multi-iteration turn (ToolCall -> ToolCall -> Text) through the real
    // `prepare_chat_dispatch` seam and a tool that snapshots the DB from
    // *inside* the agent loop, between the first and second tool calls —
    // exactly "mid-run".

    /// A snapshot of DB state [`ProbeTool`] takes from inside the agent
    /// loop, between the two tool-calling iterations of the ToolCall ->
    /// ToolCall -> Text script.
    struct ProbeSnapshot {
        /// Non-user rows (assistant-with-tool-calls + tool-result) visible
        /// via `get_agent_messages` at snapshot time.
        intermediate_rows: usize,
        /// The pre-inserted placeholder's `status` column at snapshot time.
        placeholder_status: chat_service::MessageStatus,
        /// Length of the placeholder's `extra_metadata.thinking_events`
        /// array at snapshot time, as `get_session_messages` (the real UI
        /// read path) sees it.
        thinking_events_len: usize,
    }

    /// Registered under the same name [`crate::test_support::NOOP_TOOL_NAME`]
    /// (`"noop"`) that every [`crate::test_support::Reply::ToolCall`] in the
    /// ToolCall -> ToolCall -> Text script targets — a plain
    /// [`crate::test_support::NoopTool`] can't observe anything, so this
    /// stands in for it. Behaves identically to `NoopTool` (returns
    /// `"noop"`) except on its second invocation, where it reads the DB
    /// first and stashes a [`ProbeSnapshot`] into `snapshot` — capturing
    /// exactly the state between iteration 1 (already complete, including
    /// whatever iteration 1's own boundary hook flushed) and iteration 2
    /// (whose own messages/flush have not happened yet). This is
    /// deterministic — no timing assumption — because tool execution is
    /// strictly sequential within `CustomAgent::chat()`'s loop: iteration
    /// 2's tool cannot run before iteration 1's iteration-boundary hook
    /// (awaited in-line, not spawned — see
    /// `ChatAgentAdapter::wire_incremental_persister`'s doc) has returned.
    struct ProbeTool {
        calls: std::sync::atomic::AtomicUsize,
        db: kyomi_core::DbPool,
        key: Arc<[u8; 32]>,
        session_id: String,
        assistant_message_id: String,
        snapshot: Arc<std::sync::Mutex<Option<ProbeSnapshot>>>,
    }

    #[async_trait::async_trait]
    impl crate::tools::AgentTool for ProbeTool {
        fn name(&self) -> &str {
            "noop"
        }

        fn description(&self) -> &str {
            "Does nothing, except snapshot DB state on its second call."
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }

        async fn execute(
            &self,
            _args: serde_json::Value,
            _ctx: &crate::tools::ToolContext,
        ) -> kyomi_core::Result<String> {
            let call_number = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if call_number == 2 {
                let rows = chat_service::get_agent_messages(&self.db, &self.key, &self.session_id, None)
                    .await
                    .expect("get_agent_messages should succeed inside the probe");
                let intermediate_rows = rows.iter().filter(|m| m.role != "user").count();

                let (placeholder_status, _owner) =
                    chat_service::get_message_status(&self.db, &self.assistant_message_id)
                        .await
                        .expect("get_message_status should succeed inside the probe")
                        .expect("the placeholder row must still exist mid-run");

                let stored = chat_service::get_session_messages(&self.db, &self.key, &self.session_id, 100)
                    .await
                    .expect("get_session_messages should succeed inside the probe");
                let thinking_events_len = stored
                    .iter()
                    .find(|m| m.message_id == self.assistant_message_id)
                    .map(|m| m.thinking_events.len())
                    .unwrap_or(0);

                *self.snapshot.lock().expect("snapshot mutex") = Some(ProbeSnapshot {
                    intermediate_rows,
                    placeholder_status,
                    thinking_events_len,
                });
            }
            Ok("noop".to_string())
        }
    }

    /// Build a `ChatAgentAdapter` wired to `ScriptedProvider`'s
    /// ToolCall -> ToolCall -> Text script, a `ProbeTool` (registered under
    /// `noop`, replacing `NoopTool`), and — since a mid-run
    /// `get_session_messages` read must see thinking_events too (KYO-493
    /// AC) — a real `AgentThinkingTracker` with `incremental_flush` set,
    /// exactly as `execute_agent_chat` wires one for a `CallerPreInserted`
    /// turn. Returns the adapter and the snapshot handle the probe fills in.
    fn adapter_with_probe(
        db: kyomi_core::DbPool,
        key: Arc<[u8; 32]>,
        session_id: &str,
        assistant_message_id: &str,
        durable: bool,
    ) -> (ChatAgentAdapter, Arc<std::sync::Mutex<Option<ProbeSnapshot>>>) {
        let script = vec![
            crate::test_support::Reply::ToolCall,
            crate::test_support::Reply::ToolCall,
            crate::test_support::Reply::Text("Revenue was $1M, up 12% QoQ.".to_string()),
        ];
        let provider = crate::test_support::ScriptedProvider {
            script: std::sync::Mutex::new(script.into()),
            after_script: crate::test_support::Reply::Text("unexpected".to_string()),
            calls: Arc::new(std::sync::Mutex::new(Vec::new())),
        };

        let snapshot = Arc::new(std::sync::Mutex::new(None));
        let mut registry = crate::tools::ToolRegistry::new();
        registry.register(Arc::new(ProbeTool {
            calls: std::sync::atomic::AtomicUsize::new(0),
            db: db.clone(),
            key: key.clone(),
            session_id: session_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            snapshot: snapshot.clone(),
        }));

        let agent = CustomAgent::new(
            Box::new(provider),
            crate::agent::AgentConfig::default(),
            Arc::new(registry),
            crate::test_support::build_ctx(db.clone()),
            std::collections::HashMap::new(),
        );

        let mut adapter = ChatAgentAdapter::new(
            agent,
            "user-a".to_string(),
            "ws-1".to_string(),
            Some(session_id.to_string()),
            "custom_agent".to_string(),
            db.clone(),
            key.clone(),
        );

        let tracker = crate::thinking::AgentThinkingTracker::new(crate::thinking::AgentThinkingTrackerConfig {
            session_id: session_id.to_string(),
            user_id: "user-a".to_string(),
            message_id: assistant_message_id.to_string(),
            ws_manager: kyomi_auth::websocket::WebSocketManager::new(None, db.clone()),
            workspace_user_ids: None,
            context_type: Some("chat".to_string()),
            context_window: 0,
            incremental_flush: (!durable).then_some(crate::thinking::IncrementalFlushTarget {
                db: db.clone(),
                encryption_key: key.clone(),
            }),
        });
        adapter.set_thinking_tracker(Arc::new(tokio::sync::Mutex::new(tracker)));

        (adapter, snapshot)
    }

    #[tokio::test]
    async fn intermediate_messages_and_thinking_events_are_visible_mid_run() {
        assert_intermediate_visibility(false).await;
    }

    #[tokio::test]
    async fn durable_intermediate_messages_and_thinking_events_are_visible_mid_run() {
        assert_intermediate_visibility(true).await;
    }

    async fn assert_intermediate_visibility(durable: bool) {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key: Arc<[u8; 32]> = Arc::new([7u8; 32]);
        let session_id = uuid::Uuid::new_v4().to_string();

        let (user_persistence, assistant_persistence) = if durable {
            claim_durable_message(&db, &key, &session_id, "investigate revenue", chrono::Utc::now().timestamp_millis(), 30_000).await
        } else {
            dispatch_chat_turn(&db, &key, &session_id, "investigate revenue").await
        };
        let assistant_message_id = assistant_persistence.tag_id().expect("has an id").to_string();

        let (mut adapter, snapshot) =
            adapter_with_probe(db.clone(), key.clone(), &session_id, &assistant_message_id, durable);

        adapter
            .chat(ChatParams {
                message: "investigate revenue",
                cancel_token: CancellationToken::new(),
                current_time_user_tz: None,
                message_source: Some("web"),
                user_id: Some("user-a"),
                user_message_persistence: &user_persistence,
                assistant_message_persistence: &assistant_persistence,
            })
            .await
            .expect("chat() should succeed");

        // -- Mid-run snapshot, captured from inside the agent loop --------
        let snapshot = snapshot
            .lock()
            .expect("snapshot mutex")
            .take()
            .expect("the probe's second call must have captured a snapshot");

        assert_eq!(
            snapshot.intermediate_rows, 2,
            "today: no intermediate rows exist until the whole turn finishes (KYO-493). \
             After the fix: iteration 1's assistant-with-tool-calls + tool-result rows \
             must already be durable by the time iteration 2's tool starts executing — \
             found {} instead of 2",
            snapshot.intermediate_rows
        );
        assert_eq!(
            snapshot.placeholder_status,
            chat_service::MessageStatus::InProgress,
            "the placeholder must still read in_progress mid-run — no terminal status \
             is written until the turn actually finishes"
        );
        assert!(
            snapshot.thinking_events_len >= 1,
            "today: the placeholder's extra_metadata carries no thinking_events until \
             the turn finishes. After the fix: a mid-run get_session_messages read must \
             already see iteration 1's completed tool-call step; found {} events",
            snapshot.thinking_events_len
        );

        if let Some(run) = assistant_persistence.durable_run() {
            assert_eq!(chat_service::get_message_status(&db, &assistant_message_id).await.expect("status").expect("placeholder").0,
                chat_service::MessageStatus::InProgress, "adapter cannot finalize a durable placeholder");
            kyomi_auth::conversation_events::ConversationStore::new(&db, &key, "user-a", "ws-1")
                .finish(&run.conversation_id, &run.run_id, &run.lease, chrono::Utc::now().timestamp_millis(), agent_runtime::RunState::Completed, "Revenue was $1M, up 12% QoQ.")
                .await.expect("worker commits terminal answer");
        }

        // -- After the full run: no duplicates, correct final content -----
        // get_agent_messages returns every row except an *empty*
        // assistant placeholder (no content AND no tool_calls) — the
        // finalized placeholder has real content, so it's included here
        // too, alongside the 4 intermediate rows.
        let all_rows = chat_service::get_agent_messages(&db, &key, &session_id, None)
            .await
            .expect("get_agent_messages should succeed");
        let user_rows = all_rows.iter().filter(|m| m.role == "user").count();
        let non_user_rows = all_rows.iter().filter(|m| m.role != "user").count();

        assert_eq!(
            user_rows, 1,
            "exactly one user row — the incremental writer's positional skip of the \
             CallerPersisted row must hold even though that row is untagged while the \
             loop is still running; found {user_rows}"
        );
        assert_eq!(
            non_user_rows, 5,
            "exactly 5 non-user rows — 2 tool-calling iterations x (1 \
             assistant-with-tool-calls + 1 tool-result) plus the 1 finalized assistant \
             answer — with no duplicates from persist_after_chat's own catch-up pass \
             re-writing what the incremental writer already wrote; found {non_user_rows}"
        );

        let stored = chat_service::get_session_messages(&db, &key, &session_id, 100)
            .await
            .expect("get_session_messages should succeed");
        let assistant_rows: Vec<&chat_service::MessageItem> =
            stored.iter().filter(|m| m.message_type == "assistant").collect();
        assert_eq!(
            assistant_rows.len(),
            1,
            "exactly one final assistant row (get_session_messages filters out the \
             tool_calls IS NOT NULL intermediate rows) — found {}",
            assistant_rows.len()
        );
        assert_eq!(assistant_rows[0].message_id, assistant_message_id);
        assert_eq!(assistant_rows[0].content, "Revenue was $1M, up 12% QoQ.");
    }

    #[tokio::test]
    async fn incremental_persist_failure_does_not_kill_the_run_and_is_caught_up_at_the_end() {
        // KYO-493 ticket: "Any persistence failure mid-run must be logged
        // loudly but must NOT kill the agent run; the final write still
        // happens." Simulated by using a session_id that does not exist in
        // `chat_sessions` for the *incremental* writes — SQLite's foreign
        // key on chat_messages.session_id makes every incremental INSERT
        // fail — while still asserting the turn completes successfully and
        // the caller sees the real answer.
        //
        // This can't drive persist_after_chat's own catch-up pass to
        // succeed afterward (the FK violation is structural, not
        // transient) — it only proves the "non-fatal" half of the AC. The
        // "still caught up" half — persisted_up_to doesn't advance past a
        // failed message, and persist_after_chat's end-of-run catch-up
        // pass successfully persists it once the DB is healthy again — is
        // proven separately by
        // `persist_after_chat_catches_up_messages_left_unpersisted_by_a_transient_incremental_failure`
        // below, which simulates a *transient* failure (a healthy DB,
        // `persisted_up_to` simply stuck short of `state.messages.len()`)
        // rather than this test's permanent one.
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key: Arc<[u8; 32]> = Arc::new([7u8; 32]);
        // Deliberately never created via create_session_with_id — the FK
        // on chat_messages.session_id has nothing to reference.
        let session_id = uuid::Uuid::new_v4().to_string();
        let assistant_message_id = uuid::Uuid::new_v4().to_string();

        // A plain ToolCall -> ToolCall -> Text script over the ordinary
        // NoopTool — unlike `adapter_with_probe`, this test has no
        // pre-inserted placeholder to probe (`AdapterInserts`, not
        // `CallerPreInserted`), so it doesn't need `ProbeTool`'s DB reads.
        let script = vec![
            crate::test_support::Reply::ToolCall,
            crate::test_support::Reply::ToolCall,
            crate::test_support::Reply::Text("Revenue was $1M, up 12% QoQ.".to_string()),
        ];
        let provider = crate::test_support::ScriptedProvider {
            script: std::sync::Mutex::new(script.into()),
            after_script: crate::test_support::Reply::Text("unexpected".to_string()),
            calls: Arc::new(std::sync::Mutex::new(Vec::new())),
        };
        let mut registry = crate::tools::ToolRegistry::new();
        registry.register(Arc::new(crate::test_support::NoopTool));
        let agent = CustomAgent::new(
            Box::new(provider),
            crate::agent::AgentConfig::default(),
            Arc::new(registry),
            crate::test_support::build_ctx(db.clone()),
            std::collections::HashMap::new(),
        );
        let mut adapter = ChatAgentAdapter::new(
            agent,
            "user-a".to_string(),
            "ws-1".to_string(),
            Some(session_id.clone()),
            "custom_agent".to_string(),
            db.clone(),
            key.clone(),
        );

        let user_persistence = UserMessagePersistence::AdapterPersists(None);
        let assistant_persistence =
            AssistantMessagePersistence::AdapterInserts(Some(assistant_message_id.clone()));

        let result = adapter
            .chat(ChatParams {
                message: "investigate revenue",
                cancel_token: CancellationToken::new(),
                current_time_user_tz: None,
                message_source: Some("web"),
                user_id: Some("user-a"),
                user_message_persistence: &user_persistence,
                assistant_message_persistence: &assistant_persistence,
            })
            .await;

        let response = result.expect(
            "an incremental persist failure (FK violation — no such session) must not \
             kill the agent run; the caller must still get the real answer back",
        );
        assert_eq!(response, "Revenue was $1M, up 12% QoQ.");
    }

    // -- Contract: persist_after_chat catches up messages a *transient*
    // -- incremental failure left behind (KYO-493 review) -------------------
    //
    // `incremental_persist_failure_does_not_kill_the_run_and_is_caught_up_
    // at_the_end` above uses a session that never exists in `chat_sessions`
    // at all, so the FK violation it drives is permanent — persist_after_
    // chat's own catch-up pass hits the exact same violation and fails
    // too. That test can only prove the "non-fatal" half of the AC. It
    // says nothing about whether the catch-up pass actually persists what
    // was left behind once the DB is healthy again, which is the more
    // common real case (a lock timeout, a momentary connection hiccup —
    // the kind of failure `wire_incremental_persister`'s error branch is
    // designed to let a later, healthy write recover from).
    //
    // This test drives that case directly: two messages already durably
    // written (exactly the shape `persist_one_new_message` would have left
    // them — this is what a *successful* incremental write looks like),
    // and two more sitting in `agent.state_mut().messages` past
    // `persisted_up_to`, which is left stuck at 2 — precisely how
    // `wire_incremental_persister`'s error branch leaves it after a failed
    // attempt (advance only past what actually succeeded, log, break). The
    // DB itself is healthy throughout — nothing here is broken except the
    // in-memory bookkeeping, which is exactly what makes this "transient"
    // rather than the sibling test's "permanent". Calling
    // `persist_after_chat()` directly (never `chat()`, so the agent loop
    // and the incremental writer are never involved) isolates the one
    // thing under test: the catch-up pass alone, over a healthy DB.
    #[tokio::test]
    async fn persist_after_chat_catches_up_messages_left_unpersisted_by_a_transient_incremental_failure() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key: Arc<[u8; 32]> = Arc::new([7u8; 32]);
        let session_id = uuid::Uuid::new_v4().to_string();
        chat_service::create_session_with_id(&db, "user-a", "ws-1", &session_id, None, "chat", None)
            .await
            .expect("create session");

        // Rows already durably written before the transient failure — a
        // one-tool-call round, seeded directly rather than replayed
        // through persist_one_new_message, so this test doesn't depend on
        // that function already being correct to set up its own fixture.
        let already_persisted_assistant_id = chat_service::add_message(
            &db,
            &key,
            &session_id,
            "assistant",
            "Let me check the catalog.",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            chat_service::MessageStatus::Complete,
        )
        .await
        .expect("seed the already-persisted assistant row");
        let already_persisted_tool_id = chat_service::add_message(
            &db,
            &key,
            &session_id,
            "tool",
            "5 rows",
            None,
            None,
            None,
            None,
            None,
            Some("tc_1"),
            Some("search_catalog"),
            None,
            chat_service::MessageStatus::Complete,
        )
        .await
        .expect("seed the already-persisted tool row");

        // A bare adapter over the same (healthy) session — UnusedProvider
        // is fine since persist_after_chat() is called directly, never
        // chat(): the agent loop, and therefore the incremental writer,
        // never runs.
        let mut adapter = adapter_over(db.clone(), "user-a", &session_id, key.clone());

        // The in-memory state a real turn would have reached: the two
        // already-persisted messages at indices 0-1 (their exact content
        // doesn't matter — persist_after_chat never looks at anything
        // before persisted_up_to), then two more at indices 2-3 that the
        // incremental writer never got to.
        {
            let state = adapter.agent.state_mut();
            state.messages.push(Message::assistant("placeholder — already persisted, index 0"));
            state.messages.push(Message::tool_result("tc_1", "search_catalog", "placeholder — already persisted, index 1"));
            state.messages.push(Message::assistant_with_tool_calls(
                "",
                vec![ToolCall {
                    id: "tc_2".to_string(),
                    name: "query_datasource".to_string(),
                    arguments: serde_json::json!({}),
                    arguments_error: None,
                }],
            ));
            state.messages.push(Message::tool_result("tc_2", "query_datasource", "10 rows"));
        }
        // Exactly how wire_incremental_persister's error branch leaves it
        // after a transient failure: advanced past the 2 that succeeded,
        // stuck short of state.messages.len() (4).
        *adapter.persisted_up_to.lock().await = 2;

        adapter
            .persist_after_chat()
            .await
            .expect("persist_after_chat must succeed — the DB is healthy, only the \
                     in-memory persisted_up_to bookkeeping was left behind");

        let all_rows = chat_service::get_agent_messages(&db, &key, &session_id, None)
            .await
            .expect("get_agent_messages should succeed");

        // Every remaining message lands exactly once: 2 seeded (already
        // persisted) + 2 caught up = 4, not 6 (which duplicates from
        // re-persisting the first 2 would produce) and not 2 (which
        // skipping the catch-up entirely would produce).
        assert_eq!(
            all_rows.len(),
            4,
            "expected exactly 4 rows (2 already-persisted + 2 caught up); found {}: {:?}",
            all_rows.len(),
            all_rows.iter().map(|m| (&m.role, &m.content)).collect::<Vec<_>>()
        );

        // Messages persisted earlier are not duplicated — each seeded id
        // appears exactly once.
        let ids: Vec<&str> = all_rows.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(
            ids.iter().filter(|id| **id == already_persisted_assistant_id).count(),
            1,
            "the already-persisted assistant row must not be duplicated by the catch-up pass"
        );
        assert_eq!(
            ids.iter().filter(|id| **id == already_persisted_tool_id).count(),
            1,
            "the already-persisted tool row must not be duplicated by the catch-up pass"
        );

        // Order is correct: the 2 seeded rows first (they were written
        // first, in an earlier wall-clock moment), then the 2 the
        // catch-up pass wrote — in the same order they were pushed onto
        // state.messages.
        assert_eq!(all_rows[0].message_id, already_persisted_assistant_id);
        assert_eq!(all_rows[1].message_id, already_persisted_tool_id);
        assert_eq!(
            all_rows[2].role, "assistant",
            "the catch-up pass's first newly-written row must be the assistant-with-\
             tool-calls message at state.messages[2]"
        );
        assert_eq!(
            all_rows[3].role, "tool",
            "the catch-up pass's second newly-written row must be the tool-result \
             message at state.messages[3]"
        );
        assert_eq!(
            all_rows[3].tool_name.as_deref(),
            Some("query_datasource"),
            "the caught-up tool row must be the real one from state.messages[3], not a \
             stale or swapped one"
        );
        assert_eq!(all_rows[3].content, "10 rows");
    }
}
