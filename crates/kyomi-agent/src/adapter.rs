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
//! `message_source` columns. [`db_message_to_agent_message`] rebuilds the
//! identical prefix from those columns for every later turn (KYO-506) — see
//! its doc for why a `AdapterPersists` row (Slack, copilot, watch) never
//! needs this: its `content` already carries the prefix as literal text.

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
    /// Historical behaviour — Slack, copilot, and watch execution all use
    /// this.
    AdapterPersists(Option<String>),
    /// The caller already wrote the user-message row to the DB *before*
    /// the agent was spawned (KYO-492 —
    /// `kyomi_auth::chat_service::prepare_chat_dispatch`). The adapter must
    /// neither re-load that row into context (`load_context` filters it out
    /// via [`drop_pre_persisted_message`]) nor write it a second time
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
    pub assistant_message_id: Option<&'a str>,
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
        }
    }

    /// Wire a thinking tracker to the agent's callbacks.
    ///
    /// The tracker methods are async but callbacks are synchronous closures.
    /// We use `tokio::task::spawn` to bridge the gap — each callback fires
    /// an async task that runs the tracker method. This keeps the agent loop
    /// non-blocking while still delivering events in near-real-time.
    pub fn set_thinking_tracker(&mut self, tracker: Arc<tokio::sync::Mutex<AgentThinkingTracker>>) {
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

        // Get session info for metadata restoration.
        let session = chat_service::get_session(&self.db, session_id).await?;
        let Some(session) = session else {
            return Ok(false);
        };

        // Restore agent state from session config if available.
        if let Some(ref config) = session.config
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

        // Load all messages from DB.
        let db_messages = chat_service::get_agent_messages(
            &self.db,
            &self.encryption_key,
            session_id,
            None, // load all messages
        )
        .await?;

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
    /// Saves any intermediate messages (tool calls, tool results) that the
    /// agent generated during the loop, and updates the session config with
    /// the current agent state.
    pub async fn persist_after_chat(&mut self) -> kyomi_core::Result<()> {
        let Some(ref session_id) = self.session_id else {
            return Ok(());
        };

        let state = self.agent.state();
        let total_messages = state.messages.len();

        // Save any messages beyond what we loaded from the DB.
        // The first `messages_loaded_count` messages came from the DB;
        // everything after is new (user message, tool calls, tool results, etc.).
        if total_messages > self.messages_loaded_count {
            let new_messages = &state.messages[self.messages_loaded_count..];

            for msg in new_messages {
                // Skip system messages (they're part of the prompt, not stored).
                if msg.role == MessageRole::System {
                    continue;
                }

                // Skip the one message the caller already persisted before
                // calling chat() (KYO-492) — see
                // UserMessagePersistence::CallerPersisted. Every other new
                // message (a second user turn within the same run, every
                // assistant/tool message, and — critically — any
                // AdapterPersists row, e.g. Slack's caller-chosen id) is
                // still persisted here.
                if !should_persist_new_message(
                    msg,
                    self.user_message_persistence.caller_persisted_id(),
                ) {
                    continue;
                }

                let role = match msg.role {
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::Tool => "tool",
                    MessageRole::System => continue,
                };

                let tool_calls_json = msg
                    .tool_calls
                    .as_ref()
                    .map(|tc| serde_json::to_value(tc).unwrap_or_default());

                chat_service::add_message(
                    &self.db,
                    &self.encryption_key,
                    session_id,
                    role,
                    &msg.content,
                    None, // metadata
                    msg.message_id.as_deref(), // use tagged ID if set, else auto-generate
                    None, // current_time_user_tz
                    // message_source: always None here — a message reaching
                    // this branch of `chat()` (AdapterPersists — Slack,
                    // copilot, watch) has its content built by
                    // `agent.chat()`'s `build_metadata_prefix`, so any
                    // source/local-time annotation is already baked into
                    // `msg.content` as literal text. Recording it again in
                    // this row's own columns would make
                    // `db_message_to_agent_message` reconstruct a *second*
                    // prefix on top of the one already there the next time
                    // this row is loaded (KYO-506).
                    None,
                    if msg.role == MessageRole::User {
                        msg.user_id.as_deref()
                    } else {
                        None
                    },
                    msg.tool_call_id.as_deref(),
                    msg.name.as_deref(),
                    tool_calls_json.as_ref(),
                )
                .await?;
            }
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

        chat_service::update_session(&self.db, session_id, None, None, Some(&metadata)).await?;

        info!(
            session_id = %session_id,
            new_messages = total_messages.saturating_sub(self.messages_loaded_count),
            "Persisted agent state after chat"
        );

        Ok(())
    }

    /// Run the agent loop for a user message.
    ///
    /// Handles context loading (lazy), delegation to the agent, and
    /// post-chat persistence. All new messages (user, tool, assistant)
    /// are saved to the DB by `persist_after_chat()`.
    ///
    /// If `assistant_message_id` is provided, it will be set on the final
    /// assistant response message before persistence, ensuring the DB record
    /// matches the ID used for WebSocket streaming and UI display.
    pub async fn chat(&mut self, params: ChatParams<'_>) -> kyomi_core::Result<String> {
        // Record how this turn's user message is persisted before
        // load_context() runs, so the filter it applies (and the
        // persist-skip below) see it.
        self.user_message_persistence = params.user_message_persistence.clone();

        // Lazy context loading on first call.
        if !self.context_loaded {
            self.load_context().await?;
        }

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
        if let Some(amid) = params.assistant_message_id {
            self.tag_last_assistant_message_id(amid);
        }

        // Persist all new messages and metadata regardless of success/failure.
        // We log at error level (not warn) because message persistence failures
        // mean data loss — the user's conversation may not be saved.
        if let Err(e) = self.persist_after_chat().await {
            error!(error = %e, "Failed to persist agent state after chat — messages may be lost");
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
/// - A row written by `chat_service::prepare_chat_dispatch` has both columns
///   populated and a raw `content` — reconstruction here is exactly what
///   restores the annotation the live turn saw.
/// - A row written by `ChatAgentAdapter::persist_after_chat` (the
///   `AdapterPersists` paths — Slack, copilot, watch) has both columns
///   `None` (see that call site) and a `content` that already carries the
///   prefix as literal text — `build_metadata_prefix(None, None)` returns
///   an empty string, so `content` passes through unchanged and is never
///   double-prefixed.
/// - A row written before this column pair existed has both columns `None`
///   for the same reason as above: no annotation is fabricated, `content`
///   passes through unchanged.
/// - A row with `current_time_user_tz` but no `message_source` (or vice
///   versa) — e.g. `copilot_service::prepare_copilot_message`, which never
///   captures a source — gets the partial annotation `build_metadata_prefix`
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
        // An AdapterPersists row (Slack/copilot/watch, via
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
        // write site that never captured a source (e.g.
        // copilot_service::prepare_copilot_message) or a row written before
        // the message_source column existed. The reconstructed annotation
        // must degrade to time-only: no source may ever be invented.
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
        // None means no row was pre-persisted (copilot/watch paths) —
        // behaviour must be unchanged from before KYO-492: every new
        // message gets persisted.
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
        let agent = CustomAgent::new(
            Box::new(UnusedProvider),
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
}
